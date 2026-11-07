#!/bin/sh
# Builds guesthouse-card-check as a file to hand out: in the image packaging/Containerfile describes, with
# podman or docker, and otherwise with this PC's Rust. Either way it must run on the oldest glibc the app
# runs on, and the build stops if it wouldn't. Needs no root, and publishes nothing.
#
# Usage: tools/card-check/build.sh [--local]
set -eu

# What packaging/build-in-images.sh promises. Ubuntu 24.04 has this one.
GLIBC_CEILING=2.39
PROGRAM=guesthouse-card-check

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$HERE/../.." && pwd)
OUT=${CARGO_TARGET_DIR:-$REPO/target}
BUILD="cargo build --release --locked -p $PROGRAM"

die() {
    printf 'build: %s\n' "$*" >&2
    exit 1
}

build_in_image() {
    engine=$(command -v podman || command -v docker) || return 1
    case ${engine##*/} in
        podman) as_user=--userns=keep-id ;;
        *) as_user=--user=$(id -u):$(id -g) ;;
    esac
    tag=guesthouse-build-app:$(sha256sum <"$REPO/packaging/Containerfile" | cut -c 1-12)
    "$engine" image inspect "$tag" >/dev/null 2>&1 ||
        "$engine" build -t "$tag" -f "$REPO/packaging/Containerfile" "$REPO/packaging" >&2
    mkdir -p "$OUT/home"
    "$engine" run --rm --security-opt label=disable "$as_user" -e "HOME=$OUT/home" \
        -e "CARGO_TARGET_DIR=$OUT" -v "$OUT:$OUT" -v "$REPO:$REPO" -w "$REPO" "$tag" nice -n 19 $BUILD
}

# The highest glibc symbol version the program asks for, which is what the host must have.
glibc_needed() {
    grep -ao 'GLIBC_2\.[0-9.]*' "$1" | sed 's/GLIBC_//; s/\.$//' | sort -Vu | tail -n 1
}

if [ "${1:-}" = --local ]; then
    (cd "$REPO" && $BUILD)
else
    build_in_image || (cd "$REPO" && $BUILD)
fi
cp "$OUT/release/$PROGRAM" "$HERE/$PROGRAM"
needed=$(glibc_needed "$HERE/$PROGRAM")
highest=$(printf '%s\n%s\n' "$needed" "$GLIBC_CEILING" | sort -V | tail -n 1)
[ "$highest" = "$GLIBC_CEILING" ] || die "this build needs glibc $needed, and it must run on $GLIBC_CEILING."
printf '%s needs glibc %s\n' "$HERE/$PROGRAM" "$needed"
sha256sum "$HERE/$PROGRAM"
