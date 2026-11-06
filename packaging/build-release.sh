#!/bin/sh
# Builds what a release publishes: the payload that install.sh downloads, and the source of everything
# in it, QEMU included, which the GPL asks for. It needs no root, and publishes nothing.
#
# Usage: packaging/build-release.sh [--out FOLDER] [STAGE...]
#
# The stages run in this order, and with none named all of them do:
#   qemu      QEMU, its firmware blobs, and the libraries it needs from the build system
#   app       the app, the daemon and their helpers
#   payload   guesthouse-VERSION-x86_64.tar.xz
#   source    guesthouse-VERSION-source.tar.xz, and SHA256SUMS
#
# Run it inside the image from packaging/Containerfile, because what it builds only runs on systems
# with that image's glibc or a newer one. QEMU_UPSTREAM can name a local clone of QEMU.
set -eu

QEMU_TAG=v11.1.3
QEMU_COMMIT=005c61c0ef6f6ae615283440b4bb7be51b2c6ddd
QEMU_UPSTREAM=${QEMU_UPSTREAM:-https://gitlab.com/qemu-project/qemu.git}
APP_ID=net.gpupv.Guesthouse
ARCH=x86_64

# These stay the host's. The C library can't be mixed, and PipeWire's client must match its server.
HOST_LIBRARIES='^(linux-vdso|ld-linux|libc|libm|libdl|libpthread|librt|libresolv|libgcc_s|libpipewire-0\.3)[.-]'

REPO=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
OUT=$REPO/build/release

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'build-release: %s\n' "$*" >&2
    exit 1
}

release_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -n 1
}

# A release has to be something the tag can rebuild, so nothing uncommitted goes into one. An unpacked
# source bundle has no git to ask, and is what it is.
refuse_uncommitted() {
    [ -e "$REPO/.git" ] || return 0
    [ -z "$(git -C "$REPO" status --porcelain)" ] ||
        die "there are uncommitted changes, and a release must be rebuildable from its tag"
}

# The source bundle carries QEMU's tree, and a clone fetches one.
qemu_tree() {
    if [ -d "$REPO/qemu-source" ]; then
        printf '%s' "$REPO/qemu-source"
    else
        printf '%s' "$OUT/qemu-source"
    fi
}

# From upstream, at the exact commit.
fetch_qemu() {
    QEMU_TREE=$(qemu_tree)
    [ ! -d "$QEMU_TREE" ] || return 0
    git clone -q --depth 1 --branch "$QEMU_TAG" "$QEMU_UPSTREAM" "$QEMU_TREE.new"
    [ "$(git -C "$QEMU_TREE.new" rev-parse HEAD)" = "$QEMU_COMMIT" ] ||
        die "$QEMU_TAG is not the commit it should be"
    git -C "$QEMU_TREE.new" -c user.name=release -c user.email=release@localhost \
        am -q "$REPO/qemu/qemu-gpupv.patch"
    mv "$QEMU_TREE.new" "$QEMU_TREE"
}

# DT_RPATH and not the newer DT_RUNPATH, as only the older one also covers what the bundled libraries
# load themselves. That saves rewriting each of them with patchelf.
configure_qemu() {
    build=$OUT/qemu-build
    [ ! -f "$build/build.ninja" ] || return 0
    mkdir -p "$build"
    # shellcheck disable=SC2046 # the file holds one flag a line, and none has a space in it
    (cd "$build" && "$QEMU_TREE/configure" $(cat "$REPO/packaging/qemu-configure-flags") \
        '--extra-ldflags=-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib')
}

build_qemu() {
    targets=qemu-system-$ARCH
    # The Linux route's helper, once the patch has it.
    [ ! -d "$QEMU_TREE/hw/misc/nvfwd" ] || targets="$targets hw/misc/nvfwd/nvfwd-sensord"
    # shellcheck disable=SC2086 # two words on purpose
    ninja -C "$OUT/qemu-build" $targets
}

# Libraries are copied under their sonames, which is what the loader asks for.
bundle_libraries() {
    program=$1
    folder=$(dirname "$program")/lib
    list=$OUT/stage/LIBRARIES.txt
    mkdir -p "$folder"
    : >"$list"
    ldd "$program" | awk '$2 == "=>" && $3 ~ /^\// { print $1, $3 }' | while read -r name path; do
        if printf '%s\n' "$name" | grep -Eq "$HOST_LIBRARIES"; then
            continue
        fi
        cp -L "$path" "$folder/$name"
        chmod u+w "$folder/$name"
        printf '%s %s\n' "$name" "$(rpm -qf "$path" 2>/dev/null || echo unknown)" >>"$list"
    done
}

stage_qemu() {
    fetch_qemu
    configure_qemu
    build_qemu
    target=$OUT/stage/qemu/build
    mkdir -p "$target" "$OUT/stage/qemu/pc-bios"
    cp "$OUT/qemu-build/qemu-system-$ARCH" "$target/"
    [ ! -f "$OUT/qemu-build/hw/misc/nvfwd/nvfwd-sensord" ] ||
        cp "$OUT/qemu-build/hw/misc/nvfwd/nvfwd-sensord" "$target/"
    bundle_libraries "$target/qemu-system-$ARCH"
    # The build folder holds what was generated, like the keymaps, and the source the rest.
    cp -R "$QEMU_TREE/pc-bios/." "$OUT/stage/qemu/pc-bios/"
    cp -RL "$OUT/qemu-build/pc-bios/." "$OUT/stage/qemu/pc-bios/"
    say "QEMU is in $target"
}

# The real backend goes in if the tree has one, and otherwise the daemon says it has none.
daemon_features() {
    [ ! -d "$REPO/crates/backend-nvidia" ] || printf '%s' '--features guesthouse-daemon/nvidia'
}

# What the app calls in GTK and libadwaita. install.sh looks for each in the host's libraries, which
# says exactly whether this build can run there, with no list of versions to keep up.
write_symbol_needs() {
    nm -D --undefined-only "$1" | awk '{ print $2 }' | sed 's/@.*//' |
        grep -E '^(gtk|gdk|gsk|adw)_' | sort -u >"$OUT/stage/needs-symbols"
}

stage_app() {
    target_folder=${CARGO_TARGET_DIR:-$REPO/target}
    # shellcheck disable=SC2046 # empty, or the flag and its value
    (cd "$REPO" && cargo build --release --locked -p guesthouse-app -p guesthouse-app-core \
        -p guesthouse-daemon $(daemon_features))
    mkdir -p "$OUT/stage"
    for program in guesthouse guesthouse-check guesthouse-daemon guesthouse-share guesthouse-task; do
        cp "$target_folder/release/$program" "$OUT/stage/"
    done
    write_symbol_needs "$OUT/stage/guesthouse"
    mkdir -p "$OUT/stage/share"
    cp "$REPO/crates/app/data/$APP_ID.desktop" "$OUT/stage/share/"
    cp -R "$REPO/crates/app/data/icons" "$OUT/stage/share/"
    say "the app is in $OUT/stage"
}

# The highest glibc symbol version anything here asks for, which is what the host must have.
glibc_needed() {
    find "$OUT/stage" -type f \( -perm -u+x -o -name '*.so*' \) -exec grep -aoh 'GLIBC_2\.[0-9.]*' {} + |
        sed 's/GLIBC_//; s/\.$//' | sort -Vu | tail -n 1
}

# The folder names are the app's, in paths.rs and linux/mod.rs.
copy_guest_files() {
    [ ! -d "$REPO/guest" ] || cp -R "$REPO/guest" "$OUT/stage/"
    [ ! -d "$REPO/linux" ] || cp -R "$REPO/linux" "$OUT/stage/"
}

release_parts() {
    parts=app
    [ ! -d "$OUT/stage/guest" ] || parts="$parts windows"
    [ ! -d "$OUT/stage/linux" ] || parts="$parts linux"
    printf '%s' "$parts"
}

# Packs the stage under a folder of the archive's own name, without leaving a second copy behind.
pack_stage() {
    tar -C "$OUT/stage" --transform "s,^\.,$1," --owner=0 --group=0 --sort=name -cJf "$OUT/$1.tar.xz" .
}

stage_payload() {
    version=$(release_version)
    copy_guest_files
    cp "$REPO/install.sh" "$REPO/uninstall.sh" "$REPO/LICENSE" "$REPO/THIRD-PARTY.md" "$OUT/stage/"
    {
        printf 'version=%s\n' "$version"
        printf 'commit=%s\n' "$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo source-bundle)"
        printf 'glibc=%s\n' "$(glibc_needed)"
        printf 'parts=%s\n' "$(release_parts)"
    } >"$OUT/stage/RELEASE"
    pack_stage "guesthouse-$version-$ARCH"
    say "the payload is $OUT/guesthouse-$version-$ARCH.tar.xz"
}

# Everything the GPL asks for in one file: this repo with the patch and these scripts, and QEMU's whole
# tree with its subprojects, so that nothing depends on upstream staying where it is.
stage_source() {
    version=$(release_version)
    name=guesthouse-$version-source
    bundle=$OUT/source-bundle
    [ ! -d "$bundle" ] || rm -rf "${bundle:?}"
    mkdir -p "$bundle/qemu-source"
    git -C "$REPO" archive HEAD | tar -C "$bundle" -xf -
    tar -C "$(qemu_tree)" --exclude=.git -cf - . | tar -C "$bundle/qemu-source" -xf -
    tar -C "$bundle" --transform "s,^\.,$name," --owner=0 --group=0 --sort=name -cJf "$OUT/$name.tar.xz" .
    rm -rf "${bundle:?}"
    (cd "$OUT" && sha256sum "guesthouse-$version-$ARCH.tar.xz" "$name.tar.xz" >SHA256SUMS)
    say "the source is $OUT/$name.tar.xz"
    say "for RELEASE_VERSION, PAYLOAD_SHA256 and SOURCE_SHA256 in install.sh:"
    say "$version"
    cut -d ' ' -f 1 "$OUT/SHA256SUMS"
}

run_stage() {
    case $1 in
        qemu) stage_qemu ;;
        app) stage_app ;;
        payload) stage_payload ;;
        source) stage_source ;;
        *) die "there's no stage called $1" ;;
    esac
}

main() {
    if [ "${1:-}" = --out ]; then
        OUT=$2
        shift 2
    fi
    [ $# -gt 0 ] || set -- qemu app payload source
    refuse_uncommitted
    mkdir -p "$OUT"
    for stage in "$@"; do
        run_stage "$stage"
    done
}

{ main "$@"; }
