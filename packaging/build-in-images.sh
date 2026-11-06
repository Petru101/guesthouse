#!/bin/sh
# Builds a release the way it's published, each part in the image it has to be built in. QEMU and its
# libraries take the glibc of their image, so they're built on the oldest system a release runs on. The
# app calls little of glibc but wants a recent GTK to build with, so it's built on something newer.
#
# Usage: packaging/build-in-images.sh [--out FOLDER] [STAGE...]
# The stages are build-release.sh's. It needs podman or docker and no root, and publishes nothing.
set -eu

APP_IMAGE=guesthouse-build-app
QEMU_IMAGE=guesthouse-build-qemu
# The oldest glibc a release promises to run on. Ubuntu 24.04 has this one.
GLIBC_CEILING=2.39

REPO=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
OUT=$REPO/build/release

die() {
    printf 'build-in-images: %s\n' "$*" >&2
    exit 1
}

find_engine() {
    ENGINE=$(command -v podman || command -v docker) || die "this needs podman or docker."
    case ${ENGINE##*/} in
        podman) AS_USER=--userns=keep-id ;;
        *) AS_USER=--user=$(id -u):$(id -g) ;;
    esac
}

make_image() {
    "$ENGINE" image inspect "$1" >/dev/null 2>&1 ||
        "$ENGINE" build -t "$1" -f "$REPO/packaging/$2" "$REPO/packaging"
}

# A clone holds exactly what's committed, and a git worktree's .git points outside any mount. An
# unpacked source bundle has no git, and is what it is.
find_tree() {
    TREE=$REPO
    [ -e "$REPO/.git" ] || return 0
    [ -z "$(git -C "$REPO" status --porcelain)" ] ||
        die "there are uncommitted changes, and a release must be rebuildable from its tag"
    TREE=$OUT/tree
    [ ! -d "$TREE" ] || rm -rf "${TREE:?}"
    git clone -q "$REPO" "$TREE"
}

in_image() {
    "$ENGINE" run --rm --security-opt label=disable "$AS_USER" -e "HOME=$OUT/home" \
        -e "CARGO_TARGET_DIR=$OUT/target" -v "$OUT:$OUT" -v "$TREE:$TREE" -w "$TREE" "$1" \
        nice -n 19 sh packaging/build-release.sh --out "$OUT" "$2"
}

image_for() {
    case $1 in
        qemu) printf '%s' "$QEMU_IMAGE" ;;
        *) printf '%s' "$APP_IMAGE" ;;
    esac
}

# The two images make this promise, so a change that breaks it stops here and not on someone's PC.
check_glibc_ceiling() {
    [ -f "$OUT/stage/RELEASE" ] || return 0
    needed=$(sed -n 's/^glibc=//p' "$OUT/stage/RELEASE")
    highest=$(printf '%s\n%s\n' "$needed" "$GLIBC_CEILING" | sort -V | tail -n 1)
    [ "$highest" = "$GLIBC_CEILING" ] ||
        die "this release needs glibc $needed, and it must run on $GLIBC_CEILING."
    printf 'this release needs glibc %s\n' "$needed"
}

main() {
    if [ "${1:-}" = --out ]; then
        OUT=$2
        shift 2
    fi
    [ $# -gt 0 ] || set -- qemu app payload source
    find_engine
    make_image "$QEMU_IMAGE" Containerfile.qemu
    make_image "$APP_IMAGE" Containerfile
    mkdir -p "$OUT/home"
    find_tree
    for stage in "$@"; do
        in_image "$(image_for "$stage")" "$stage"
    done
    check_glibc_ceiling
}

{ main "$@"; }
