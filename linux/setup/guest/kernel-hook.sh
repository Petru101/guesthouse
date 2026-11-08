#!/bin/bash
# Builds our modules for a kernel that has just been installed, so its first boot doesn't wait for the
# build. It needs this boot's driver image and the new kernel's headers. Without either, the next boot
# builds instead, as it would anyway, and this never fails the package manager that called it.
# Usage: kernel-hook.sh VERSION, or kernel-hook.sh --paths with pacman's paths on its input.
set -uo pipefail
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
. "$here/common.sh"

# pacman gives each headers file it installed, as usr/lib/modules/VERSION/build/Makefile.
kernels_from_paths() {
    sed -n 's,^/\?usr/lib/modules/\([^/]*\)/build/Makefile$,\1,p'
}

build_for() {
    local kernel=$1 target version image_id log
    target=$root/lib/modules/$kernel/updates/nvfwd
    log=$log_folder/build-$kernel.log
    if [ ! -e "$driver/manifest" ]; then
        echo "nvfwd: this boot has no driver image, so the next boot builds for kernel $kernel"
        return
    fi
    if [ ! -e "$root/lib/modules/$kernel/build/Makefile" ]; then
        echo "nvfwd: kernel $kernel has no headers yet, so the next boot builds for it"
        return
    fi
    version=$(value_of "$driver/manifest" version)
    image_id=$(value_of "$driver/manifest" id)
    [ "$(cat "$target/built-for" 2>/dev/null)" != "$image_id $kernel" ] || return
    echo "nvfwd: building for driver $version on kernel $kernel"
    mkdir -p "$log_folder"
    if "$here/build-parts.sh" "$driver/sources.tar" "$version" "$kernel" > "$log" 2>&1; then
        echo "$image_id $kernel" > "$target/built-for"
    else
        echo "nvfwd: the build for kernel $kernel failed, see $log. The next boot tries again"
    fi
}

if [ "${1:-}" = --paths ]; then
    for kernel in $(kernels_from_paths); do build_for "$kernel"; done
elif [ -n "${1:-}" ]; then
    build_for "$1"
fi
exit 0
