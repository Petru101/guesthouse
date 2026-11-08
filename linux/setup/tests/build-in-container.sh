#!/bin/bash
# Builds our modules and programs in a container of each family, the way setup-own.sh does in a guest:
# its package list, then build-parts.sh against the container's kernel headers package. Nothing is
# loaded, so this says the build works there, and no more.
# Usage: build-in-container.sh SOURCES.tar VERSION [FAMILY...]
# SOURCES.tar is a driver image's sources.tar, VERSION its driver version. Families are fedora,
# debian, arch and suse, and all four by default.
set -euo pipefail
guest=$(CDPATH='' cd -- "$(dirname -- "$0")/../guest" && pwd)
sources=$(realpath "$1")
version=$2
shift 2
families=${*:-fedora debian arch suse}

image_of() {
    case $1 in
    fedora) echo registry.fedoraproject.org/fedora:45 ;;
    debian) echo docker.io/library/debian:13 ;;
    arch) echo docker.io/library/archlinux:latest ;;
    suse) echo registry.opensuse.org/opensuse/tumbleweed:latest ;;
    esac
}

# Runs in the container, as root. A container has no running kernel of its own, so the headers are
# the family's newest, and the build is named the kernel they are for.
inside() {
    cat <<'EOF'
set -euo pipefail
family=$1 version=$2
. /setup/common.sh
. /setup/distro.sh
case $family in
fedora) headers=kernel-devel ;;
debian) headers=linux-headers-amd64 ;;
arch) headers=linux-headers; pacman -Sy --noconfirm > /dev/null ;;
suse) headers=kernel-default-devel ;;
esac
family_refresh "$family" > /dev/null
# shellcheck disable=SC2046
family_install "$family" $(family_packages "$family" /setup) "$headers" > /var/tmp/install.log 2>&1 ||
    { tail -n 30 /var/tmp/install.log; exit 1; }
if [ "$family" = fedora ]; then
    kernel=$(ls /usr/src/kernels | head -n 1)
    mkdir -p "/lib/modules/$kernel"
    ln -sfn "/usr/src/kernels/$kernel" "/lib/modules/$kernel/build"
fi
kernel=$(ls /lib/modules/*/build/Makefile | head -n 1 | cut -d/ -f4)
echo "headers for $kernel"
# As setup-own.sh leaves it, so build-parts.sh knows the family.
mkdir -p /etc/nvfwd
printf 'family=%s\nlibraries=%s\n' "$family" "$(libraries_folder "$family")" > /etc/nvfwd/own-install.conf
/setup/build-parts.sh /sources.tar "$version" "$kernel" > /var/tmp/build.log 2>&1 ||
    { tail -n 40 /var/tmp/build.log; exit 1; }
ls "/lib/modules/$kernel/updates/nvfwd" /opt/nv/built /opt/nv/layer
modinfo -F vermagic "/lib/modules/$kernel/updates/nvfwd/nvidia.ko"
EOF
}

failed=0
for family in $families; do
    echo "==== $family: $(image_of "$family")"
    if podman run --rm -v "$guest:/setup:ro,Z" -v "$sources:/sources.tar:ro,Z" "$(image_of "$family")" \
        bash -c "$(inside)" inside "$family" "$version"; then
        echo "PASS $family"
    else
        echo "FAIL $family"
        failed=$((failed + 1))
    fi
done
[ "$failed" = 0 ]
