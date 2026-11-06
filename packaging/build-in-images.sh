#!/bin/sh
# Builds a release the way it's published, each part in the image it has to be built in. QEMU and its
# libraries take the glibc of their image, so they're built on the oldest system a release runs on. The
# app calls little of glibc but wants a recent GTK to build with, so it's built on something newer.
#
# Usage: packaging/build-in-images.sh [--out FOLDER] [STAGE...]
# The stages are build-release.sh's. It needs podman or docker and no root, and publishes nothing.
set -eu

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

# Named after its definition, so a change to one can't quietly use an image from before it.
make_image() {
    tag=guesthouse-build-$1:$(sha256sum <"$REPO/packaging/$2" | cut -c 1-12)
    "$ENGINE" image inspect "$tag" >/dev/null 2>&1 ||
        "$ENGINE" build -t "$tag" -f "$REPO/packaging/$2" "$REPO/packaging" >&2
    printf '%s' "$tag"
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
        qemu | bundled) printf '%s' "$QEMU_IMAGE" ;;
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

# What's in the payload but isn't ours has to go out with its source, or not at all.
check_bundled_source() {
    [ -f "$OUT/SHA256SUMS" ] || return 0
    grep -q 'bundled-source' "$OUT/SHA256SUMS" ||
        die "SHA256SUMS has no bundled source, and a release can't go out without it."
}

main() {
    if [ "${1:-}" = --out ]; then
        OUT=$2
        shift 2
    fi
    [ $# -gt 0 ] || set -- qemu bundled app payload source
    find_engine
    QEMU_IMAGE=$(make_image qemu Containerfile.qemu)
    APP_IMAGE=$(make_image app Containerfile)
    mkdir -p "$OUT/home"
    find_tree
    for stage in "$@"; do
        in_image "$(image_for "$stage")" "$stage"
    done
    check_glibc_ceiling
    check_bundled_source
}

{ main "$@"; }
