#!/bin/bash
# GNOME on a logind seat, as user desk. With the GPU that is Zink on NVIDIA's Vulkan, on the real
# nvidia-drm. Without it, plain Mesa on the VM's plain screen.
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"
input_port=/dev/virtio-ports/nvfwd.input
session_env=$XDG_RUNTIME_DIR/nvfwd-session.env

gpu_settings() {
    set -a
    . "$here/files/gpu-session.env"
    set +a
    # GLX goes through cast/glx_dri3.c, which says why.
    export __GLX_VENDOR_LIBRARY_NAME=dri3
}

# The second half runs inside the session's own bus. The login gives a user bus too, but its
# socket-started PipeWire is masked and the shell's helpers belong on this one.
if [ "${1:-}" = --inside-bus ]; then
    pipewire &
    sleep 1
    wireplumber &
    sleep 1
    {
        env | grep -E '^(DBUS_SESSION_BUS_ADDRESS|XDG_RUNTIME_DIR|XDG_SESSION_ID|__EGL_VENDOR_LIBRARY_FILENAMES|GBM_BACKEND|GALLIUM_DRIVER|MESA_LOADER_DRIVER_OVERRIDE|VK_DRIVER_FILES|ZINK_DEBUG|__GLX_VENDOR_LIBRARY_NAME)='
        echo WAYLAND_DISPLAY=wayland-0
    } > "$session_env"
    if [ "$(value_of "$state_file" gpu)" = on ]; then
        # The viewer's keys and pointer, once the shell's RemoteDesktop API is up. The frames go by KMS.
        (sleep 10; FWDCAST_INPUT_ONLY=1 exec "$built/fwdcast" 1920 1080 "$input_port") &
    fi
    exec gnome-shell --wayland --display-server --unsafe-mode
fi

export XDG_CURRENT_DESKTOP=ubuntu:GNOME XDG_SESSION_TYPE=wayland GTK_A11Y=none GTK_USE_PORTAL=0
[ "$(value_of "$state_file" gpu)" != on ] || gpu_settings
exec dbus-run-session -- bash "$0" --inside-bus
