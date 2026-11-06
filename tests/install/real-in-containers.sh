#!/bin/bash
# Installs a real release on the oldest systems it promises to run on, in throwaway containers made from
# images already on this PC. The user runs install.sh and has no rights and no sudo. What it says root
# must do is then run word for word by the container's own root, so this tests the package names too,
# which means these containers download those packages.
#
# Usage: tests/install/real-in-containers.sh RELEASE_FOLDER
# ENGINE says which of docker and podman holds the images, and docker is the default.
set -u

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
RELEASE=$(CDPATH='' cd -- "$1" && pwd)
ENGINE=${ENGINE:-docker}
IMAGES='ubuntu:24.04 debian:13'
USER_ID=1000:1000
ANSWERS='yes\nyes\n'
LONGEST_SECONDS=3600
failed=0

as_root() {
    "$ENGINE" exec -e DEBIAN_FRONTEND=noninteractive "$name" sh -c "$1"
}

script_in() {
    "$ENGINE" exec "$@"
}

# What a desktop would have already, and these images don't, and GTK's display for a PC with none.
add_basics() {
    as_root 'apt-get update -q && apt-get install -y -q --no-install-recommends xz-utils libgtk-4-bin' >/dev/null
}

run_on() {
    local image=$1 plan root_command devices=()
    name=guesthouse-real-$$
    [ ! -e /dev/kvm ] || devices=(--device /dev/kvm)
    printf '\n===== %s\n' "$image"
    "$ENGINE" run -d --rm --name "$name" "${devices[@]}" --security-opt label=disable \
        -v "$REPO:$REPO:ro" -v "$RELEASE:$RELEASE:ro" "$image" sleep "$LONGEST_SECONDS" >/dev/null || return 1
    add_basics || return 1
    plan=$(script_in -u "$USER_ID" "$name" sh "$HERE/real-in-container.sh" plan "$REPO" "$RELEASE") || {
        printf '%s\n' "$plan"
        return 1
    }
    printf '%s\n' "$plan"
    root_command=$(printf '%s\n' "$plan" | sed -n "s/^  sudo sh -c '\(.*\)'\$/\1/p")
    if [ -n "$root_command" ]; then
        as_root "$root_command" >/dev/null || return 1
        printf 'root ran that, and it worked\n'
    fi
    python3 -I "$HERE/tty.py" "$ANSWERS" "$ENGINE" exec -it -u "$USER_ID" "$name" \
        sh "$HERE/real-in-container.sh" run "$REPO" "$RELEASE"
}

for image in $IMAGES; do
    if run_on "$image"; then
        printf 'passed on %s\n' "$image"
    else
        printf 'FAILED on %s\n' "$image"
        failed=1
    fi
    "$ENGINE" rm -f "$name" >/dev/null 2>&1
done
exit "$failed"
