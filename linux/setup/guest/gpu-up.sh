#!/bin/bash
# Runs on every boot, before the desktop. Finds out whether this boot gets the GPU, and sets it up.
# A guest must always boot, so whatever goes wrong here turns the GPU off, says why, and exits 0.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
kernel=$(uname -r)
target=$root/lib/modules/$kernel/updates/nvfwd
build_log=$log_folder/build.log
first_check_seconds=60
# The app on the host reads one line from here to say whether the guest has its GPU.
serial=$root/dev/ttyS0
selinux_label=system_u:object_r:lib_t:s0
mkdir -p "$state_folder" "$log_folder" "$driver"

# A guest with no serial port says nothing, and that's no reason to stop.
tell_host() {
    [ -w "$serial" ] && printf 'nvfwd-gpu %s\n' "$1" > "$serial" 2>/dev/null
    return 0
}

gpu_off() {
    echo "GPU off: $1"
    tell_host "off $1"
    "$here/link-driver.sh" unlink
    mountpoint -q "$driver" && umount "$driver"
    printf 'gpu=off\nreason=%s\n' "$1" > "$state_file"
    mkdir -p "$root/run/motd.d"
    printf 'This guest has no GPU on this boot: %s\n' "$1" > "$root/run/motd.d/50-nvfwd"
    exit 0
}

device_present() {
    local device
    for device in "$root"/sys/bus/pci/devices/*; do
        [ "$(cat "$device/vendor" 2>/dev/null)" = "$nvfwd_vendor" ] &&
            [ "$(cat "$device/device" 2>/dev/null)" = "$nvfwd_device" ] && return 0
    done
    return 1
}

# The host leaves its reason in fw_cfg when it starts a VM with no GPU.
host_reason() {
    local reason
    modprobe qemu_fw_cfg
    reason=$(tr '\n' ' ' < "$root/sys/firmware/qemu_fw_cfg/by_name/opt/nvfwd/gpu-off-reason/raw" 2>/dev/null)
    reason=${reason% }
    echo "no nvfwd device. ${reason:-The VM was started without one, and the host gave no reason.}"
}

# With SELinux, the whole image is labelled as libraries, which any program may map.
mount_image() {
    local disk options=ro,nodev,nosuid
    disk=$(blkid -L NVFWD_DRIVER) || return 1
    [ ! -e "$root/sys/fs/selinux/enforce" ] || options=$options,context=$selinux_label
    mount -t iso9660 -o "$options" "$disk" "$driver"
}

# The version only says the host's driver is the one we built for. On a version nobody has tried, the
# allow list or the device may refuse what it needs, and that only shows when something uses the GPU.
first_check() {
    local name=$1 program=$2 output
    output=$(timeout -k 5 "$first_check_seconds" "$built/$program" 2>&1)
    if [ $? != 0 ] || ! echo "$output" | grep -q ' 0 wrong'; then
        gpu_off "$name failed its first check on driver $version (tested: $tested): $(echo "$output" | tail -n 1). The host's calls.log says what the device refused"
    fi
}

# Mutter would take the plain card as a second screen and drive both. It is only for a boot with no GPU.
hide_plain_cards() {
    local card rules=$root/run/udev/rules.d/71-nvfwd-plain-card.rules
    mkdir -p "$(dirname "$rules")"
    : > "$rules"
    for card in "$root"/sys/class/drm/card[0-9]; do
        [ -e "$card" ] || continue
        [ "$(cat "$card/device/device" 2>/dev/null)" = "$nvfwd_device" ] && continue
        echo "SUBSYSTEM==\"drm\", KERNEL==\"$(basename "$card")\", TAG+=\"mutter-device-ignore\"" >> "$rules"
    done
    udevadm control --reload
    udevadm trigger --subsystem-match=drm --action=change
    udevadm settle --timeout=10
}

# Mutter reads the cached mode list without probing, and with fbdev=0 nothing has probed yet.
probe_connectors() {
    local status
    for status in "$root"/sys/class/drm/card[0-9]-*/status; do
        [ ! -e "$status" ] || echo detect > "$status"
    done
}

# Links from an earlier boot must not outlive it, whatever this boot decides.
"$here/link-driver.sh" unlink
udevadm settle --timeout=30
device_present || gpu_off "$(host_reason)"
mount_image || gpu_off "the host offered no driver image"

version=$(value_of "$driver/manifest" version)
image_id=$(value_of "$driver/manifest" id)
tested=$(value_of "$driver/manifest" tested)
[ -n "$version" ] && [ -n "$image_id" ] || gpu_off "the driver image has no usable manifest"

built_for="$image_id $kernel"
if [ "$(cat "$target/built-for" 2>/dev/null)" != "$built_for" ]; then
    echo "Building for driver $version on kernel $kernel"
    if ! "$here/build-parts.sh" "$driver/sources.tar" "$version" > "$build_log" 2>&1; then
        gpu_off "the build for driver $version on kernel $kernel failed, see $build_log"
    fi
    echo "$built_for" > "$target/built-for"
fi

modprobe nvidia || gpu_off "the nvidia module would not load"
host_version=$("$here/driver-version.py" 2>&1) || gpu_off "the host's driver did not answer: $host_version"
# The module cannot unload, but with no user-mode files linked nothing in the guest will use it.
[ "$host_version" = "$version" ] ||
    gpu_off "the image holds driver $version, but the host has $host_version loaded"

"$here/link-driver.sh" link || gpu_off "the driver could not be linked in"
# Before nvidia-drm: if these fail the plain desktop has to come up, and that module can't unload.
first_check CUDA cudavec
first_check Vulkan vkcompute

# A desktop someone installed stays on the plain screen, and the GPU is for its programs.
if [ -n "$own_family" ]; then
    printf 'gpu=on\nversion=%s\ntested=%s\n' "$version" "$tested" > "$state_file"
    tell_host "on $version"
    echo "GPU on: driver $version, for programs. The desktop stays on the plain screen"
    exit 0
fi

# What nvidia-drm leans on. nvidia-drm's options are in /etc/modprobe.d/nvfwd.conf.
modprobe -a drm_kms_helper drm_ttm_helper video drm_display_helper
modprobe nvidia-drm || gpu_off "nvidia-drm would not load"

hide_plain_cards
probe_connectors
printf 'gpu=on\nversion=%s\ntested=%s\n' "$version" "$tested" > "$state_file"
tell_host "on $version"
echo "GPU on: driver $version"
