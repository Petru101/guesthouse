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

# The compiler writes down where each source file was, and panics name the file. Those would be
# folders on the PC that built it, so each is written as a plain name instead. The cargo home holds
# the dependencies' sources. It comes last, as the last prefix that fits wins, and the image's cargo
# home is under the build folder.
remap_flags() { # cargo home
    printf -- '--remap-path-prefix=%s=target --remap-path-prefix=%s=guesthouse --remap-path-prefix=%s=cargo' \
        "$OUT" "$REPO" "$1"
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
    IMAGE_CARGO_HOME=$OUT/home/.cargo
    "$engine" run --rm --security-opt label=disable "$as_user" -e "HOME=$OUT/home" \
        -e "CARGO_TARGET_DIR=$OUT" -e "RUSTFLAGS=$(remap_flags "$IMAGE_CARGO_HOME")" \
        -v "$OUT:$OUT" -v "$REPO:$REPO" -w "$REPO" "$tag" nice -n 19 $BUILD
    BUILT_CARGO_HOME=$IMAGE_CARGO_HOME
}

build_here() {
    BUILT_CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
    (cd "$REPO" && RUSTFLAGS=$(remap_flags "$BUILT_CARGO_HOME") $BUILD)
}

# The highest glibc symbol version the program asks for, which is what the host must have.
glibc_needed() {
    grep -ao 'GLIBC_2\.[0-9.]*' "$1" | sed 's/GLIBC_//; s/\.$//' | sort -Vu | tail -n 1
}

# A folder of this PC's in the program would go to everyone who downloads it.
check_no_folders() {
    for folder in "$REPO" "$BUILT_CARGO_HOME" "$OUT" "$HOME"; do
        ! grep -aq -- "$folder" "$1" || die "the program still names $folder."
    done
}

if [ "${1:-}" = --local ]; then
    build_here
else
    build_in_image || build_here
fi
cp "$OUT/release/$PROGRAM" "$HERE/$PROGRAM"
check_no_folders "$HERE/$PROGRAM"
needed=$(glibc_needed "$HERE/$PROGRAM")
highest=$(printf '%s\n%s\n' "$needed" "$GLIBC_CEILING" | sort -V | tail -n 1)
[ "$highest" = "$GLIBC_CEILING" ] || die "this build needs glibc $needed, and it must run on $GLIBC_CEILING."
printf '%s needs glibc %s\n' "$HERE/$PROGRAM" "$needed"
sha256sum "$HERE/$PROGRAM"
