#!/bin/bash
# Walks the app's pages on GTK's Broadway backend, which draws in software, with a scratch store.
# Nothing appears on the desktop and nothing touches the GPU. Usage: page-shots.sh FOLDER
# Needs a build first: cargo build -p guesthouse-app --example page_shots --bins
set -e
FOLDER=$(realpath -m "${1:?usage: page-shots.sh FOLDER}")
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROGRAM=${CARGO_TARGET_DIR:-$HERE/../../target}/debug/examples/page_shots
BROADWAY_DISPLAY=:49
BROADWAY_PORT=28949
rm -rf "$FOLDER"
mkdir -p "$FOLDER/home" "$FOLDER/shm" "$FOLDER/program/guest" "$FOLDER/shots" "$FOLDER/bin"
# The walk is also what stands in for QEMU, under QEMU's name.
cp "$PROGRAM" "$FOLDER/bin/qemu"
# Its own place, as a socket's path can't be long, and FOLDER's might be.
RUNTIME=$(mktemp -d)
trap 'rm -rf "$RUNTIME"' EXIT
ln -s "$HERE/data" "$FOLDER/program/data"
env -i HOME="$FOLDER/home" PATH=/usr/bin XDG_RUNTIME_DIR="$RUNTIME" \
  GUESTHOUSE_SHARED_MEMORY="$FOLDER/shm" GUESTHOUSE_PROGRAM="$FOLDER/program" QEMU="$FOLDER/bin/qemu" \
  GDK_BACKEND=broadway BROADWAY_DISPLAY=$BROADWAY_DISPLAY GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- bash -c "
    gtk4-broadwayd --address 127.0.0.1 --port $BROADWAY_PORT $BROADWAY_DISPLAY >'$FOLDER/broadway.log' 2>&1 &
    daemon=\$!
    sleep 1
    '$PROGRAM' '$FOLDER/shots'
    code=\$?
    kill \$daemon
    exit \$code"
