#!/bin/bash
# Links the mounted driver into the system, as WSL and NVIDIA's container toolkit do, or takes it out.
# Usage: link-driver.sh link|unlink
# Every link points into /opt/nv, which is how unlink finds them, even with the image gone.
set -euo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
loader_config=$root/etc/ld.so.conf.d/00-nvfwd.conf
binaries=$root/usr/local/bin
gbm=$libraries/gbm
egl_vendors=$root/usr/share/glvnd/egl_vendor.d
egl_platforms=$root/usr/share/egl/egl_external_platform.d
vulkan_drivers=$root/usr/share/vulkan/icd.d
vulkan_layers=$root/usr/share/vulkan/implicit_layer.d

unlink_all() {
    rm -f "$loader_config"
    local folder
    for folder in "$binaries" "$libraries" "$gbm" "$egl_vendors" "$egl_platforms" "$vulkan_drivers" "$vulkan_layers"; do
        [ ! -d "$folder" ] || find "$folder" -maxdepth 1 -type l -lname "$root/opt/nv/*" -delete
    done
}

link_all() {
    mkdir -p "$binaries" "$gbm" "$egl_vendors" "$egl_platforms" "$vulkan_drivers" "$vulkan_layers" "$(dirname "$loader_config")"
    echo "$root/opt/nv/driver/lib" > "$loader_config"
    ln -sf "$driver/bin/nvidia-smi" "$binaries/"
    ln -sf "$driver/lib/libnvidia-allocator.so.1" "$gbm/nvidia-drm_gbm.so"
    ln -sf "$driver/json/10_nvidia.json" "$egl_vendors/"
    ln -sf "$driver/json/nvidia_icd.x86_64.json" "$vulkan_drivers/"
    ln -sf "$driver"/json/egl_external_platform.d/*.json "$egl_platforms/"
    ln -sf "$layers"/*.json "$vulkan_layers/"
    ln -sf "$built/libGLX_dri3.so.0" "$libraries/"
}

case ${1:-} in
link) unlink_all; link_all ;;
unlink) unlink_all ;;
*) echo "usage: link-driver.sh link|unlink"; exit 2 ;;
esac
ldconfig
