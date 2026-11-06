# Sourced by the scripts here. NVFWD_ROOT is empty in a guest. The tests point it at a scratch folder.
root=${NVFWD_ROOT:-}
driver=$root/opt/nv/driver        # the host's read-only image is mounted here
built=$root/opt/nv/built          # what this guest built from the image's sources
layers=$root/opt/nv/layer         # the Vulkan layers. Their manifests name this folder
state_folder=$root/run/nvfwd
state_file=$state_folder/state    # gpu=on or gpu=off, and why. The desktop and the check read it
log_folder=$root/var/log/nvfwd
done_marker=$root/var/lib/nvfwd/first-boot-done
libraries=$root/usr/lib/x86_64-linux-gnu
nvfwd_vendor=0x1234
nvfwd_device=0x11ea

# Reads one key from a file of key=value lines.
value_of() {
    sed -n "s/^$2=//p" "$1" | head -n 1
}
