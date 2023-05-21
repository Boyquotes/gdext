/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Low level bindings to the provided C core API

#![cfg_attr(test, allow(unused))]

// Output of generated code. Mimics the file structure, symbols are re-exported.
#[rustfmt::skip]
#[allow(
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    deref_nullptr,
    clippy::redundant_static_lifetimes
)]
pub(crate) mod gen;

mod godot_ffi;
mod opaque;
mod plugins;

use std::ffi::CStr;
use std::ptr;
// See https://github.com/dtolnay/paste/issues/69#issuecomment-962418430
// and https://users.rust-lang.org/t/proc-macros-using-third-party-crate/42465/4
#[doc(hidden)]
pub use paste;

pub use crate::godot_ffi::{GodotFfi, GodotFuncMarshal, PtrcallType};
pub use gen::central::*;
pub use gen::gdextension_interface::*;
pub use gen::interface::GDExtensionInterface;

// The impls only compile if those are different types -- ensures type safety through patch
trait Distinct {}
impl Distinct for GDExtensionVariantPtr {}
impl Distinct for GDExtensionTypePtr {}
impl Distinct for GDExtensionConstTypePtr {}

// ----------------------------------------------------------------------------------------------------------------------------------------------

#[cfg(feature = "trace")]
#[macro_export]
macro_rules! out {
    ()                          => (eprintln!());
    ($fmt:literal)              => (eprintln!($fmt));
    ($fmt:literal, $($arg:tt)*) => (eprintln!($fmt, $($arg)*));
}

#[cfg(not(feature = "trace"))]
// TODO find a better way than sink-writing to avoid warnings, #[allow(unused_variables)] doesn't work
#[macro_export]
macro_rules! out {
    ()                          => ({});
    ($fmt:literal)              => ({ use std::io::{sink, Write}; let _ = write!(sink(), $fmt); });
    ($fmt:literal, $($arg:tt)*) => ({ use std::io::{sink, Write}; let _ = write!(sink(), $fmt, $($arg)*); };)
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

struct GodotBinding {
    interface: GDExtensionInterface,
    library: GDExtensionClassLibraryPtr,
    method_table: GlobalMethodTable,
}

/// Late-init globals
// Note: static mut is _very_ dangerous. Here a bit less so, since modification happens only once (during init) and no
// &mut references are handed out (except for registry, see below). Overall, UnsafeCell/RefCell + Sync might be a safer abstraction.
static mut BINDING: Option<GodotBinding> = None;

fn is_godot_4_0(get_proc_address: fptr::GetProcAddress) -> bool {
    // In Godot 4.0.x, before the new GetProcAddress mechanism, the init function looked as follows.
    // In place of the `get_proc_address` function pointer, the `p_interface` data pointer was passed.
    //
    // typedef GDExtensionBool (*GDExtensionInitializationFunction)(
    //     const GDExtensionInterface *p_interface,
    //     GDExtensionClassLibraryPtr p_library,
    //     GDExtensionInitialization *r_initialization
    // );
    //
    // Also, the GDExtensionInterface struct was beginning with these fields:
    //
    // typedef struct {
    //     uint32_t version_major;
    //     uint32_t version_minor;
    //     uint32_t version_patch;
    //     const char *version_string;
    //     ...
    // } GDExtensionInterface;
    //
    // As a result, we can try to interpret the function pointer as a legacy GDExtensionInterface data pointer and check if the
    // first fields have values version_major=4 and version_minor=0. This might be deep in UB territory, but the alternative is
    // to not be able to detect Godot 4.0.x at all, and run into UB anyway.

    let data_ptr = get_proc_address as *const u32; // `as` casts just eat this for breakfast.

    // Assumption is that we have at least 8 bytes of memory to safely read from (for both the data and the function case).
    let major = unsafe { data_ptr.read() };
    let minor = unsafe { data_ptr.offset(1).read() };

    return major == 4 && minor == 0;
}

/// # Safety
///
/// - The `interface` pointer must be a valid pointer to a [`GDExtensionInterface`] object.
/// - The `library` pointer must be the pointer given by Godot at initialisation.
/// - This function must not be called from multiple threads.
/// - This function must be called before any use of [`get_library`].
pub unsafe fn initialize(
    get_proc_address: GDExtensionInterfaceGetProcAddress,
    library: GDExtensionClassLibraryPtr,
) {
    out!("Initialize gdext...");

    let get_proc_address = get_proc_address.expect("Invalid get_proc_address function pointer");

    // If we run into an old binary, proceeding would be UB -> panic.
    // TODO in theory, we could provide a compat layer by using the old gdextension_interface.h, either at runtime or via feature.
    if is_godot_4_0(get_proc_address) {
        panic!(
            "gdext was compiled against a newer Godot version (4.1+), but initialized with a legacy (4.0.x) setup.\
            \nIn your .gdextension file, make sure to use `compatibility_minimum = 4.1` under the [configuration] section."
        );
    }

    let get_godot_version = get_proc_address(crate::c_str(b"get_godot_version\0"));
    out!(
        "Loaded get_version function: {:p}",
        get_godot_version.unwrap()
    );

    let interface = GDExtensionInterface::load(get_proc_address);
    out!("Loaded interface.");

    let method_table = GlobalMethodTable::load(&interface);
    out!("Loaded builtin table.");

    BINDING = Some(GodotBinding {
        interface,
        method_table,
        library,
    });
    out!("Assigned binding.");

    let mut version = GDExtensionGodotVersion {
        major: 0,
        minor: 0,
        patch: 0,
        string: std::ptr::null(),
    };
    interface_fn!(get_godot_version)(ptr::addr_of_mut!(version));
    out!("Detected {version:?}");

    println!(
        "Initialize GDExtension API for Rust: {}",
        CStr::from_ptr(version.string)
            .to_str()
            .expect("unknown Godot version")
    );
}

/// # Safety
///
/// Must be called from the same thread as `initialize()` previously.
pub unsafe fn is_initialized() -> bool {
    BINDING.is_some()
}

/// # Safety
///
/// The interface must have been initialised with [`initialize`] before calling this function.
#[inline(always)]
pub unsafe fn get_interface() -> &'static GDExtensionInterface {
    &unwrap_ref_unchecked(&BINDING).interface
}

/// # Safety
///
/// The library must have been initialised with [`initialize`] before calling this function.
#[inline(always)]
pub unsafe fn get_library() -> GDExtensionClassLibraryPtr {
    unwrap_ref_unchecked(&BINDING).library
}

/// # Safety
///
/// The interface must have been initialised with [`initialize`] before calling this function.
#[inline(always)]
pub unsafe fn method_table() -> &'static GlobalMethodTable {
    &unwrap_ref_unchecked(&BINDING).method_table
}

/// Makes sure that Godot is running, or panics. Debug mode only!
macro_rules! debug_assert_godot {
    ($expr:expr) => {
        debug_assert!(
            $expr,
            "Godot engine not available; make sure you are do not call it from unit/doc tests"
        ); // previous message: "unchecked access to Option::None"
    };
}

/// Combination of `as_ref()` and `unwrap_unchecked()`, but without the case differentiation in
/// the former (thus raw pointer access in release mode)
unsafe fn unwrap_ref_unchecked<T>(opt: &Option<T>) -> &T {
    debug_assert_godot!(opt.is_some());

    match opt {
        Some(ref val) => val,
        None => std::hint::unreachable_unchecked(),
    }
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

#[macro_export]
#[doc(hidden)]
macro_rules! builtin_fn {
    ($name:ident $(@1)?) => {
        $crate::method_table().$name
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! builtin_call {
        ($name:ident ( $($args:expr),* $(,)? )) => {
            ($crate::method_table().$name)( $($args),* )
        };
    }

#[macro_export]
#[doc(hidden)]
macro_rules! interface_fn {
    ($name:ident) => {{
        unsafe { $crate::get_interface().$name.unwrap_unchecked() }
    }};
}

/// Verifies a condition at compile time.
// https://blog.rust-lang.org/2021/12/02/Rust-1.57.0.html#panic-in-const-contexts
#[macro_export]
macro_rules! static_assert {
    ($cond:expr) => {
        const _: () = assert!($cond);
    };
    ($cond:expr, $msg:literal) => {
        const _: () = assert!($cond, $msg);
    };
}

/// Verifies at compile time that two types `T` and `U` have the same size.
#[macro_export]
macro_rules! static_assert_eq_size {
    ($T:ty, $U:ty) => {
        godot_ffi::static_assert!(std::mem::size_of::<$T>() == std::mem::size_of::<$U>());
    };
    ($T:ty, $U:ty, $msg:literal) => {
        godot_ffi::static_assert!(std::mem::size_of::<$T>() == std::mem::size_of::<$U>(), $msg);
    };
}

/// Extract value from box before `into_inner()` is stable
#[allow(clippy::boxed_local)] // false positive
pub fn unbox<T>(value: Box<T>) -> T {
    // Deref-move is a Box magic feature; see https://stackoverflow.com/a/42264074
    *value
}

/// Explicitly cast away `const` from a pointer, similar to C++ `const_cast`.
///
/// The `as` conversion simultaneously doing 10 other things, potentially causing unintended transmutations.
pub fn force_mut_ptr<T>(ptr: *const T) -> *mut T {
    ptr as *mut T
}

/// Add `const` to a mut ptr.
pub fn to_const_ptr<T>(ptr: *mut T) -> *const T {
    ptr as *const T
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

/// Convert a GDExtension pointer type to its uninitialized version.
pub trait AsUninit {
    type Ptr;

    #[allow(clippy::wrong_self_convention)]
    fn as_uninit(self) -> Self::Ptr;

    fn force_init(uninit: Self::Ptr) -> Self;
}

macro_rules! impl_as_uninit {
    ($Ptr:ty, $Uninit:ty) => {
        impl AsUninit for $Ptr {
            type Ptr = $Uninit;

            fn as_uninit(self) -> $Uninit {
                self as $Uninit
            }

            fn force_init(uninit: Self::Ptr) -> Self {
                uninit as Self
            }
        }
    };
}

#[rustfmt::skip]
impl_as_uninit!(GDExtensionStringNamePtr, GDExtensionUninitializedStringNamePtr);
impl_as_uninit!(GDExtensionVariantPtr, GDExtensionUninitializedVariantPtr);
impl_as_uninit!(GDExtensionStringPtr, GDExtensionUninitializedStringPtr);
impl_as_uninit!(GDExtensionObjectPtr, GDExtensionUninitializedObjectPtr);
impl_as_uninit!(GDExtensionTypePtr, GDExtensionUninitializedTypePtr);

// ----------------------------------------------------------------------------------------------------------------------------------------------

/// Metafunction to extract inner function pointer types from all the bindgen Option<F> type names.
pub(crate) trait Inner {
    type FnPtr;
}

impl<T> Inner for Option<T> {
    type FnPtr = T;
}

/// Often-used types.
pub(crate) mod fptr {
    pub(crate) type GetProcAddress =
        <crate::GDExtensionInterfaceGetProcAddress as crate::Inner>::FnPtr;
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

/// If `ptr` is not null, returns `Some(mapper(ptr))`; otherwise `None`.
#[inline]
pub fn ptr_then<T, R, F>(ptr: *mut T, mapper: F) -> Option<R>
where
    F: FnOnce(*mut T) -> R,
{
    // Could also use NonNull in signature, but for this project we always deal with FFI raw pointers
    if ptr.is_null() {
        None
    } else {
        Some(mapper(ptr))
    }
}

/// Returns a C `const char*` for a null-terminated byte string.
#[inline]
pub fn c_str(s: &[u8]) -> *const std::ffi::c_char {
    // Ensure null-terminated
    debug_assert!(!s.is_empty() && s[s.len() - 1] == 0);

    s.as_ptr() as *const std::ffi::c_char
}

#[inline]
pub fn c_str_from_str(s: &str) -> *const std::ffi::c_char {
    debug_assert!(s.is_ascii());

    c_str(s.as_bytes())
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

#[doc(hidden)]
#[inline]
pub fn default_call_error() -> GDExtensionCallError {
    GDExtensionCallError {
        error: GDEXTENSION_CALL_OK,
        argument: -1,
        expected: -1,
    }
}

#[doc(hidden)]
#[inline]
#[track_caller] // panic message points to call site
pub fn panic_call_error(
    err: &GDExtensionCallError,
    function_name: &str,
    arg_types: &[VariantType],
) -> ! {
    debug_assert_ne!(err.error, GDEXTENSION_CALL_OK); // already checked outside

    let GDExtensionCallError {
        error,
        argument,
        expected,
    } = *err;

    let argc = arg_types.len();
    let reason = match error {
        GDEXTENSION_CALL_ERROR_INVALID_METHOD => "method not found".to_string(),
        GDEXTENSION_CALL_ERROR_INVALID_ARGUMENT => {
            let from = arg_types[argument as usize];
            let to = VariantType::from_sys(expected as GDExtensionVariantType);
            let i = argument + 1;

            format!("cannot convert argument #{i} from {from:?} to {to:?}")
        }
        GDEXTENSION_CALL_ERROR_TOO_MANY_ARGUMENTS => {
            format!("too many arguments; expected {argument}, but called with {argc}")
        }
        GDEXTENSION_CALL_ERROR_TOO_FEW_ARGUMENTS => {
            format!("too few arguments; expected {argument}, but called with {argc}")
        }
        GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL => "instance is null".to_string(),
        GDEXTENSION_CALL_ERROR_METHOD_NOT_CONST => "method is not const".to_string(), // not handled in Godot
        _ => format!("unknown reason (error code {error})"),
    };

    // Note: Godot also outputs thread ID
    // In Godot source: variant.cpp:3043 or core_bind.cpp:2742
    panic!("Function call failed:  {function_name} -- {reason}.");
}
