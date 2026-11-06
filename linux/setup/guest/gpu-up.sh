#!/bin/bash
# Runs on every boot, before the desktop. Finds out whether this boot gets the GPU, and sets it up.
# A guest must always boot, so whatever goes wrong here turns the GPU off, says why, and exits 0.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
kernel=$(uname -r)
target=$root/lib/modules/$kernel/updates/nvfwd
build_log=$log_folder/build.log
mkdir -p "$state_folder" "$log_folder" "$driver"

gpu_off() {
    echo "GPU off: $1"
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

mount_image() {
    local disk
    disk=$(blkid -L NVFWD_DRIVER) || return 1
    mount -t iso9660 -o ro,nodev,nosuid "$disk" "$driver"
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

# What nvidia-drm leans on. nvidia-drm's options are in /etc/modprobe.d/nvfwd.conf.
modprobe -a drm_kms_helper drm_ttm_helper video drm_display_helper
modprobe nvidia-drm || gpu_off "nvidia-drm would not load"

"$here/link-driver.sh" link || gpu_off "the driver could not be linked in"
hide_plain_cards
probe_connectors
printf 'gpu=on\nversion=%s\ntested=%s\n' "$version" "$(value_of "$driver/manifest" tested)" > "$state_file"
echo "GPU on: driver $version"
