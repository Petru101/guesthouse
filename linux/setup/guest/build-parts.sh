#!/bin/bash
# Builds everything of ours for one driver image and this kernel. Nothing here touches what is
# running: modules go in place, and gpu-up.sh loads them.
# Usage: build-parts.sh SOURCES.tar VERSION [KERNEL]
# KERNEL is the running one unless named, as a kernel package's hook names the one it installs.
set -euo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
sources=$1
version=$2
kernel=${3:-$(uname -r)}
kernel_build=$root/lib/modules/$kernel/build
target=$root/lib/modules/$kernel/updates/nvfwd
# One for each kernel, so a hook's build and a boot's can't meet.
work=$root/var/lib/nvfwd/build-$kernel

# Ubuntu's own install has apt. A guest installed by hand has whatever its family has.
headers=linux-headers-$kernel
if [ -n "$own_family" ]; then
    . "$here/distro.sh"
    headers=$(headers_package "$own_family" "$kernel")
fi

install_headers() {
    if [ -n "$own_family" ]; then
        # At boot this can run before the network is up. A kernel updated on its own, as Fedora's
        # dnf upgrade kernel does, comes without its headers, and they have to be fetched.
        ! command -v nm-online > /dev/null || nm-online -q -t 60 || true
        family_install "$own_family" "$headers"
    else
        apt-get install -y "$headers"
    fi
}

# A new kernel can arrive without its headers. There may be a network to fetch them.
[ -e "$kernel_build/Makefile" ] || install_headers || true
if [ ! -e "$kernel_build/Makefile" ]; then
    echo "no headers for kernel $kernel: install $headers"
    exit 1
fi
rm -rf "$work"
mkdir -p "$work" "$target" "$built" "$layers"
tar -xf "$sources" -C "$work"

make -C "$work/nvfwd/guest" KDIR="$kernel_build" NV_VERSION="$version"
make -C "$work/nvfwd/kapi" KDIR="$kernel_build" NV="$work/nvidia"
# NVIDIA's own nvidia-drm, unchanged. It links against the stand-in as it would the real nvidia-modeset.
# Debian splits a kernel's headers in two, and NVIDIA's build wants both halves named.
kernel_source=$root/lib/modules/$kernel/source
[ -e "$kernel_source/Makefile" ] || kernel_source=$kernel_build
make -C "$work/nvidia/kernel-open" -j"$(nproc)" modules NV_KERNEL_MODULES=nvidia-drm \
    SYSSRC="$kernel_source" SYSOUT="$kernel_build" \
    KBUILD_EXTRA_SYMBOLS="$work/nvfwd/kapi/Module.symvers"
install -m 644 "$work/nvfwd/guest/nvidia.ko" "$work/nvfwd/kapi/nvidia-modeset.ko" \
    "$work/nvidia/kernel-open/nvidia-drm.ko" "$target/"
depmod -a "$kernel"

cd "$work/nvfwd/layer"
gcc -O2 -Wall -Wno-unused -I../guest -shared -fPIC -o "$layers/libVkLayer_nvfwd_wsi.so" wsi_*.c -ldl -lpthread -lwayland-client
gcc -O2 -Wall -shared -fPIC -o "$layers/libVkLayer_zink_on_nvidia.so" zink_on_nvidia.c
cp VkLayer_nvfwd_wsi.json VkLayer_zink_on_nvidia.json "$layers/"

cd "$work/nvfwd/cast"
gcc -O2 -Wall -shared -fPIC -o "$built/libGLX_dri3.so.0" glx_dri3.c -l:libGLX_mesa.so.0
# shellcheck disable=SC2046  # pkg-config's answer is meant to split into arguments
gcc -O2 -Wall -I../guest -o "$built/fwdcast" fwdcast.c $(pkg-config --cflags --libs libpipewire-0.3 gio-2.0) -lvulkan

cd "$work/nvfwd/tests"
glslangValidator -V --vn shader_spv -o shader.h shader.comp
gcc -O2 -o "$built/vkcompute" vkcompute.c -lvulkan
gcc -std=gnu17 -O2 -o "$built/cudavec" cudavec.c -ldl

rm -rf "$work"
