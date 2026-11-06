#!/bin/bash
# Runs in-container.sh on each distro image already on this PC, and pulls nothing. Each runs as a user
# with no rights, in a container that's thrown away. A terminal on this side types the answers, as
# Fedora's images have nothing to make one of their own.
#
# Usage: tests/install/in-containers.sh SCRATCH_FOLDER
set -u

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
SCRATCH=$1
ANSWERS='yes\nyes\n'
failed=0

run_in() {
    local engine=$1 image=$2 wanted=$3 firmware
    firmware=$(mktemp -d "$SCRATCH/firmware.XXXXXX")
    chmod 777 "$firmware"
    printf '== %s\n' "$image"
    local user=(--user 1000:1000)
    [ "$engine" = docker ] || user=(--userns=keep-id)
    if python3 -I "$HERE/tty.py" "$ANSWERS" "$engine" run --rm -it --security-opt label=disable "${user[@]}" \
        -v "$REPO:$REPO:ro" -v "$firmware:/usr/share/qemu/firmware" \
        "$image" sh "$HERE/in-container.sh" "$REPO" "$wanted"; then
        printf 'passed on %s\n' "$image"
    else
        printf 'FAILED on %s\n' "$image"
        failed=1
    fi
}

mkdir -p "$SCRATCH"
for version in 44 45 rawhide; do
    run_in podman "registry.fedoraproject.org/fedora:$version" "dnf install -y gtk4 libadwaita"
done
run_in podman docker.io/library/debian:trixie "apt-get install -y --no-remove libgtk-4-1"
run_in docker kvm-gpupv-build:latest "dnf install -y gtk4 libadwaita"
exit "$failed"
