#!/usr/bin/env bash
# Copyright (c) godot-rust; Bromeon and contributors.
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Restore un-reloaded files on exit (for local testing).
cleanup() {
  echo "[Bash]     Cleanup..."
  git checkout  --quiet ../../rust/src/lib.rs ../rust.gdextension ../MainScene.tscn
}

set -euo pipefail
trap cleanup EXIT

# Restore un-reloaded file (for local testing).
git checkout  --quiet ../../rust/src/lib.rs ../rust.gdextension

# Set up editor file which has scene open, so @tool script loads at startup. Also copy scene file that holds a script.
mkdir -p ../.godot/editor
cp editor_layout.cfg ../.godot/editor/editor_layout.cfg
cp MainScene.tscn ../MainScene.tscn

# Compile original Rust source.
cargoArgs=""
#cargoArgs="--features godot/trace"
cargo build -p hot-reload $cargoArgs

# Wait briefly so artifacts are present on file system.
sleep 0.5

$GODOT4_BIN -e --headless --path .. &
pid=$!
echo "[Bash]     Wait for Godot ready (PID $pid)..."

python orchestrate.py await
python orchestrate.py replace

# Compile updated Rust source.
cargo build -p hot-reload $cargoArgs

# Check if GDEXT_RENAME_SO is set.
if [[ -z "${GDEXT_RENAME_SO+x}" ]]; then
  GDEXT_RENAME_SO="false"
fi

# If GDEXT_RENAME_SO is 'true', we need to rename libhot_reload.so to libhot_reload_new.so after reload.
# TODO this is a workaround, but the .so seems to be not properly un-/reloaded if the same filename is used.
if [[ "${GDEXT_RENAME_SO:-false}" == "true" ]]; then
  echo "[Bash]     Rename libhot_reload.so to libhot_reload_new.so..."
  mv ../../../../target/debug/libhot_reload.so ../../../../target/debug/libhot_reload_new.so

  # Update reference in .gdextension file.
  sed -i 's|libhot_reload.so|libhot_reload_new.so|' ../rust.gdextension
fi


python orchestrate.py notify

echo "[Bash]     Wait for Godot exit..."
wait $pid
status=$?
echo "[Bash]     Godot (PID $pid) has completed with status $status."



