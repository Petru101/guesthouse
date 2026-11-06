#!/bin/sh
# A real install and uninstall of a release on the distro this runs on, as a user with no rights. It's
# meant for a throwaway container, and real-in-containers.sh starts those. Only curl is a stand-in.
#
# Usage: real-in-container.sh plan|run REPO RELEASE_FOLDER
# `plan` shows what root must do. `run` needs that done, and reads two answers from the terminal.
set -eu

step=$1
repo=$2
release=$3
here=$repo/tests/install
T=/tmp/guesthouse-test
H=$T/home
HDR_GTK=4.16
VM_SECONDS=8

fail() {
    printf 'FAILED: %s\n' "$*"
    exit 1
}

set_up() {
    mkdir -p "$H" "$T/bin" "$T/runtime"
    ln -sf "$here/stand-ins/curl" "$T/bin/curl"
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n 1)
    payload=guesthouse-$version-x86_64.tar.xz
    mkdir -p "$T/releases/v$version"
    ln -sf "$release/$payload" "$T/releases/v$version/$payload"
    wanted=$(sed -n "s/  $payload\$//p" "$release/SHA256SUMS")
    sed -e "s/^RELEASE_VERSION=.*/RELEASE_VERSION=\"$version\"/" -e "s/^PAYLOAD_SHA256=.*/PAYLOAD_SHA256=\"$wanted\"/" \
        "$repo/install.sh" >"$T/install.sh"
    HOME=$H
    PATH=$T/bin:$PATH
    STAND_IN_LOG=$T/log
    FAKE_RELEASES=$T/releases
    XDG_RUNTIME_DIR=$T/runtime
    export HOME PATH STAND_IN_LOG FAKE_RELEASES XDG_RUNTIME_DIR
}

listing() {
    (cd "$H" && find . -printf '%y %m %p %l\n' | sort)
}

# Which of the two apps this GTK must get, and whether it did.
check_app() {
    gtk=$("$current/guesthouse-check" | awk -F '\t' '$1 == "gtk" { print $2 }')
    [ -n "$gtk" ] || fail "guesthouse-check found no GTK"
    tar -xJf "$release/$payload" -C "$T" --wildcards '*/guesthouse' '*/guesthouse-no-hdr'
    wanted_app=guesthouse-no-hdr
    [ "$(printf '%s\n%s\n' "$HDR_GTK" "$gtk" | sort -V | tail -n 1)" != "$gtk" ] || wanted_app=guesthouse
    have=$(sha256sum <"$current/guesthouse")
    [ "$have" = "$(sha256sum <"$T/guesthouse-$version-x86_64/$wanted_app")" ] || fail "GTK $gtk didn't get $wanted_app"
    [ ! -e "$current/guesthouse-no-hdr" ] || fail "the other app is still there"
    printf 'GTK %s got %s\n' "$gtk" "$wanted_app"
}

# -r also resolves every symbol, so a library that's here but too old shows too.
check_libraries() {
    for program in guesthouse guesthouse-check guesthouse-daemon guesthouse-share guesthouse-task \
        qemu/build/qemu-system-x86_64 qemu/build/nvfwd-sensord; do
        [ -e "$current/$program" ] || fail "$program isn't there"
        ldd -r "$current/$program" >"$T/ldd" 2>&1 || true
        if grep -E 'not found|undefined symbol|version .* not' "$T/ldd"; then
            fail "$program doesn't resolve"
        fi
        printf 'resolves: %s (%s libraries)\n' "$program" "$(grep -c '=>' "$T/ldd")"
    done
}

check_programs() {
    qemu=$current/qemu/build/qemu-system-x86_64
    "$qemu" -version | head -n 1
    "$qemu" -device help >"$T/devices"
    grep -E 'gpupv|nvfwd' "$T/devices" | cut -d , -f 1
    "$qemu" -display help | grep -qx dbus || fail "QEMU has no D-Bus display"
    echo "QEMU has its D-Bus display"
    "$current/guesthouse" --help | head -n 2
}

# The same choice as Firmware::described_in in the app, with Secure Boot and no keys.
firmware_descriptor() {
    for file in /usr/share/qemu/firmware/*.json; do
        grep -q '"secure-boot"' "$file" && ! grep -q '"enrolled-keys"' "$file" && grep -q 'pc-q35' "$file" || continue
        printf '%s' "$file"
        return
    done
}

boot_plain_vm() {
    if [ ! -w /dev/kvm ]; then
        echo "no KVM in this container, so no VM"
        return
    fi
    descriptor=$(firmware_descriptor)
    [ -n "$descriptor" ] || fail "no firmware descriptor fits"
    code=$(grep -A 3 '"executable"' "$descriptor" | sed -n 's/.*"filename": "\(.*\)".*/\1/p')
    vars=$(grep -A 3 '"nvram-template"' "$descriptor" | sed -n 's/.*"filename": "\(.*\)".*/\1/p')
    printf 'firmware: %s\n' "$code"
    cp "$vars" "$T/vars.fd"
    qemu-img create -f qcow2 "$T/throwaway.qcow2" 64M >/dev/null
    status=0
    timeout "$VM_SECONDS" "$qemu" -L "$current/qemu/pc-bios" -machine q35,accel=kvm,smm=on -m 256 \
        -display none -monitor none -nodefaults -serial file:"$T/serial.txt" \
        -global driver=cfi.pflash01,property=secure,value=on \
        -drive if=pflash,format=raw,unit=0,readonly=on,file="$code" \
        -drive if=pflash,format=raw,unit=1,file="$T/vars.fd" \
        -drive file="$T/throwaway.qcow2",if=virtio || status=$?
    # timeout's own code, which means QEMU was still running when it ended it.
    [ "$status" -eq 124 ] || fail "the VM stopped by itself, with $status"
    printf 'the VM ran %s seconds and wrote %s bytes to its serial port\n' "$VM_SECONDS" "$(wc -c <"$T/serial.txt")"
}

set_up
current=$H/.local/lib/guesthouse/current

if [ "$step" = plan ]; then
    sed -n 2p /etc/os-release
    ldd --version | head -n 1
    sh "$T/install.sh" --dry-run </dev/null
    exit 0
fi

listing >"$T/before"
sh "$T/install.sh" </dev/null
export GUESTHOUSE_PROGRAM="$current"
check_app
check_libraries
check_programs
boot_plain_vm
unset GUESTHOUSE_PROGRAM
sh "$current/uninstall.sh" </dev/null
[ "$(listing)" = "$(cat "$T/before")" ] || fail "HOME isn't what it was"
echo "HOME is as it began"
