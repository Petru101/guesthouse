#!/bin/bash
# A whole install and uninstall with a real release's files, in a fake HOME. Only curl and sudo are
# stand-ins: everything else is this PC's real tools and libraries. The rest of the filesystem is
# read-only, so nothing can reach the real HOME, and at the end HOME must be as it began.
#
# Usage: tests/install/real-run.sh RELEASE_FOLDER SCRATCH_FOLDER
# RELEASE_FOLDER is build-release.sh's --out, and SCRATCH_FOLDER must not exist yet.
set -eu

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
RELEASE=$1
T=$2
VM_SECONDS=8
FIRMWARE=/usr/share/edk2/ovmf/OVMF_CODE.fd

mkdir "$T"
H=$T/home
mkdir -p "$H" "$T/bin" "$T/runtime" "$T/releases"
ln -s "$HERE/stand-ins/curl" "$HERE/stand-ins/sudo" "$T/bin/"
: >"$T/log"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -n 1)
mkdir "$T/releases/v$version"
cp "$RELEASE/guesthouse-$version-x86_64.tar.xz" "$RELEASE/guesthouse-$version-source.tar.xz" "$T/releases/v$version/"
payload=$(sed -n "s/  guesthouse-$version-x86_64.tar.xz$//p" "$RELEASE/SHA256SUMS")
source=$(sed -n "s/  guesthouse-$version-source.tar.xz$//p" "$RELEASE/SHA256SUMS")
sed -e "s/^RELEASE_VERSION=.*/RELEASE_VERSION=\"$version\"/" -e "s/^PAYLOAD_SHA256=.*/PAYLOAD_SHA256=\"$payload\"/" \
    -e "s/^SOURCE_SHA256=.*/SOURCE_SHA256=\"$source\"/" "$REPO/install.sh" >"$T/install.sh"

sandboxed() {
    env -i HOME="$H" PATH="$T/bin:/usr/bin" XDG_RUNTIME_DIR="$T/runtime" STAND_IN_LOG="$T/log" \
        FAKE_RELEASES="$T/releases" TERM=dumb \
        bwrap --ro-bind / / --dev-bind /dev /dev --bind "$T" "$T" "$@"
}

snapshot() {
    (cd "$H" && find . -printf '%y %m %p %l\n' | sort && find . -type f -exec sha256sum {} + | sort -k 2)
}

current=$H/.local/lib/guesthouse/current

snapshot >"$T/before"
printf '== HOME before: %s entries\n' "$(wc -l <"$T/before")"

sandboxed python3 -I "$HERE/tty.py" 'yes\n' sh "$T/install.sh"
printf '\n== installed, %s files, %s\n' "$(grep -c '^file' "$H/.local/lib/guesthouse/manifest")" "$(du -sh "$H" | cut -f 1)"
grep -E '^(format|version|mode)' "$H/.local/lib/guesthouse/manifest"
ls -la "$H/.local/lib/guesthouse"
cat "$H/.local/share/applications/net.gpupv.Guesthouse.desktop"

printf '\n== the real programs start from the install\n'
sandboxed "$current/qemu/build/qemu-system-x86_64" -version | head -n 1
sandboxed env "GUESTHOUSE_PROGRAM=$current" "$current/guesthouse-check"
sandboxed env "GUESTHOUSE_PROGRAM=$current" "$current/guesthouse" --help | head -n 2

printf '\n== a plain VM with no GPU, on the installed QEMU, reaches its firmware\n'
qemu-img create -f qcow2 "$T/throwaway.qcow2" 64M >/dev/null
sandboxed timeout "$VM_SECONDS" "$current/qemu/build/qemu-system-x86_64" -L "$current/qemu/pc-bios" \
    -machine q35,accel=kvm -m 256 -display none -monitor none -serial file:"$T/serial.txt" \
    -drive if=pflash,format=raw,readonly=on,file="$FIRMWARE" -drive file="$T/throwaway.qcow2",if=virtio -nodefaults || true
printf 'the VM wrote %s bytes to its serial port\n' "$(wc -c <"$T/serial.txt")"

printf '\n== sudo was asked for: %s\n' "$(grep -c '^sudo' "$T/log" || true)"

sandboxed python3 -I "$HERE/tty.py" 'yes\n' sh "$current/uninstall.sh"
snapshot >"$T/after"
printf '\n== HOME after: %s entries\n' "$(wc -l <"$T/after")"
if cmp -s "$T/before" "$T/after"; then
    printf 'HOME is exactly as it began\n'
else
    diff "$T/before" "$T/after"
    exit 1
fi
