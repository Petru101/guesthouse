#!/bin/bash
# Installs a real release on the oldest systems it promises to run on, in throwaway containers made from
# images already on this PC. The user runs install.sh and has no rights but sudo, with no password, so
# the installer runs its own root step, the way it does for a real user: the packages, which tests their
# names too and means these containers download them, and QEMU's bridge helper.
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
# Install's question and uninstall's, each typed when it's asked, as the root step's apt-get reads the
# terminal too and would take any answer typed ahead.
ANSWERS='yes\nyes\n'
LONGEST_SECONDS=3600
failed=0

as_root() {
    "$ENGINE" exec -e DEBIAN_FRONTEND=noninteractive "$name" sh -c "$1"
}

script_in() {
    "$ENGINE" exec "$@"
}

# What a desktop would have already, and these images don't, and GTK's display for a PC with none. The
# user needs a name for sudo to take it, and some images have none for 1000.
add_basics() {
    as_root 'apt-get update -q && apt-get install -y -q --no-install-recommends xz-utils libgtk-4-bin sudo' >/dev/null &&
        as_root "getent passwd ${USER_ID%:*} >/dev/null || useradd -m -u ${USER_ID%:*} tester" &&
        as_root "printf '#${USER_ID%:*} ALL=(ALL:ALL) NOPASSWD: ALL\\n' >/etc/sudoers.d/guesthouse-test &&
            chmod 440 /etc/sudoers.d/guesthouse-test"
}

run_on() {
    local image=$1 plan devices=()
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
    TTY_ANSWER_ON='Type yes: ' python3 -I "$HERE/tty.py" "$ANSWERS" "$ENGINE" exec -it -u "$USER_ID" "$name" \
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
