#!/bin/sh
# Builds what a release publishes: the payload that install.sh downloads, and the source of everything
# in it, QEMU included, which the GPL asks for. It needs no root, and publishes nothing.
#
# Usage: packaging/build-release.sh [--out FOLDER] [--display-driver FOLDER] [STAGE...]
#
# The stages run in this order, and with none named all of them do:
#   qemu      QEMU, its firmware blobs, and the libraries it needs from the build system
#   bundled   guesthouse-VERSION-bundled-source.tar, the source of those libraries and blobs
#   app       the app, the daemon and their helpers
#   payload   guesthouse-VERSION-x86_64.tar.xz
#   source    guesthouse-VERSION-source.tar.xz, and SHA256SUMS
#
# check-driver isn't one of these: it only looks at what --display-driver names, and builds nothing.
#
# Everything built here only runs on this system's glibc or a newer one, so a release doesn't run this
# directly: packaging/build-in-images.sh runs each stage in its own image. On its own it builds for
# this PC, which is what install.sh --from-source wants. QEMU_UPSTREAM can name a local clone of QEMU.
#
# The display driver can't be built here, as it needs Windows and Microsoft's kit, so --display-driver
# names the folder holding what drivers/windows-display/BUILDING.md builds: GuesthouseDisplay.dll and
# GuesthouseDisplay.inf. GUESTHOUSE_DISPLAY_DRIVER does the same. Without it a build for this PC still
# works, and its Windows machines stay on the basic display. The source stage refuses, as that's a release.
set -eu

QEMU_TAG=v11.1.3
QEMU_COMMIT=005c61c0ef6f6ae615283440b4bb7be51b2c6ddd
QEMU_UPSTREAM=${QEMU_UPSTREAM:-https://gitlab.com/qemu-project/qemu.git}
# QEMU's own release of that tag. Its firmware blobs are the ones in the tree, and unlike a clone it has
# the source of each in roms/, which a clone leaves empty.
QEMU_RELEASE_URL=https://download.qemu.org/qemu-${QEMU_TAG#v}.tar.xz
QEMU_RELEASE_SHA256=5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22
APP_ID=io.github.xnixy.Guesthouse
ARCH=x86_64
DISPLAY_DRIVER=${GUESTHOUSE_DISPLAY_DRIVER:-}
DRIVER_SOURCE=drivers/windows-display
# What the app wants in the program folder, in app-core's display.rs.
DRIVER_FOLDER=display-driver
DRIVER_BUILT='GuesthouseDisplay.dll GuesthouseDisplay.inf'
DRIVER_SETUP='sign-and-install.ps1 install-take-over.ps1 take-over-display.ps1'
# The driver has code from Microsoft's sample as well as zako-vdd's, so both licences go wherever it does.
DRIVER_FILES='vdd_settings.xml LICENSE LICENSE-MS-PL'

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

# Which package a library came from, and the source package that was built from.
library_package() {
    if command -v dpkg-query >/dev/null 2>&1; then
        package=$(dpkg-query -S "$(readlink -f "$1")" 2>/dev/null | head -n 1 | cut -d : -f 1)
        [ -z "$package" ] ||
            dpkg-query -W -f '${Package} ${Version} source ${source:Package} ${source:Version}' "$package"
    elif command -v rpm >/dev/null 2>&1; then
        rpm -qf "$1"
    fi
}

# Libraries are copied under their sonames, which is what the loader asks for. This reads the program
# where it was built, as a staged one finds the libraries of the last run through its rpath.
bundle_libraries() {
    program=$1
    folder=$2
    list=$OUT/stage/LIBRARIES.txt
    mkdir -p "$folder"
    cat >"$list" <<'EOF'
# The libraries in qemu/build/lib, and the firmware in qemu/pc-bios, aren't mine, and come with their
# licences. Each release has an asset called guesthouse-VERSION-bundled-source.tar with the exact
# source of all of them. Its SOURCES.txt says what's where.
#
# Library, package, its version, and the source package it was built from:
EOF
    ldd "$program" | awk '$2 == "=>" && $3 ~ /^\// { print $1, $3 }' | while read -r name path; do
        if printf '%s\n' "$name" | grep -Eq "$HOST_LIBRARIES"; then
            continue
        fi
        cp -L "$path" "$folder/$name"
        chmod u+w "$folder/$name"
        printf '%s %s\n' "$name" "$(library_package "$path" || echo unknown)" >>"$list"
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
    bundle_libraries "$OUT/qemu-build/qemu-system-$ARCH" "$target/lib"
    # The build folder holds what was generated, like the keymaps, and the source the rest.
    cp -R "$QEMU_TREE/pc-bios/." "$OUT/stage/qemu/pc-bios/"
    cp -RL "$OUT/qemu-build/pc-bios/." "$OUT/stage/qemu/pc-bios/"
    say "QEMU is in $target"
}

# The exact versions the bundled libraries were built from, with Ubuntu's patches and packaging. An
# archive stops listing an old version soon after a new one, and then a newer image is the answer.
fetch_library_sources() {
    awk '$3 != "" && $4 == "source" { print $5 "=" $6 }' "$OUT/stage/LIBRARIES.txt" | sort -u |
        while read -r wanted; do
            (cd "$1" && apt-get source -q --download-only --only-source "$wanted") ||
                die "the archive has no $wanted any more. Remove the qemu image and build again."
            name=${wanted%%=*}
            version=${wanted#*=}
            [ -f "$1/${name}_${version#*:}.dsc" ] || die "$wanted didn't arrive."
        done
}

fetch_firmware_source() {
    curl -fsSL -o "$1/qemu-${QEMU_TAG#v}.tar.xz" "$QEMU_RELEASE_URL"
    [ "$(sha256sum "$1/qemu-${QEMU_TAG#v}.tar.xz" | cut -d ' ' -f 1)" = "$QEMU_RELEASE_SHA256" ] ||
        die "$QEMU_RELEASE_URL isn't the file it should be."
}

write_sources_note() {
    cat <<EOF
The source of everything in a Guesthouse release that isn't mine and isn't in its source bundle.

libraries/  Ubuntu 24.04's source packages for the libraries in qemu/build/lib, at the exact versions
            they were built from: each .dsc with its original source and Ubuntu's changes. debian/rules
            in each is how it was built. To unpack one: dpkg-source -x NAME_VERSION.dsc

firmware/   QEMU's own release of $QEMU_TAG, unmodified. Its pc-bios/ holds the firmware in qemu/pc-bios,
            and roms/ the source of it, with the recipes in roms/Makefile.

recipes/    How the libraries' image and QEMU itself were set up here.

Which library came from which source package:

EOF
    grep -v '^#' "$OUT/stage/LIBRARIES.txt"
}

# Separate from the source bundle, as it's large and building Guesthouse doesn't need it. It's a plain
# tar, as all it holds is packed already.
stage_bundled() {
    command -v apt-get >/dev/null 2>&1 || die "this stage runs in the image from Containerfile.qemu."
    [ -f "$OUT/stage/LIBRARIES.txt" ] || die "the qemu stage comes first."
    name=guesthouse-$(release_version)-bundled-source
    bundle=$OUT/bundled-source
    [ ! -d "$bundle" ] || rm -rf "${bundle:?}"
    mkdir -p "$bundle/libraries" "$bundle/firmware" "$bundle/recipes"
    fetch_library_sources "$bundle/libraries"
    fetch_firmware_source "$bundle/firmware"
    cp "$REPO/packaging/Containerfile.qemu" "$REPO/packaging/qemu-configure-flags" \
        "$REPO/packaging/build-release.sh" "$bundle/recipes/"
    write_sources_note >"$bundle/SOURCES.txt"
    tar -C "$bundle" --transform "s,^\.,$name,S" --owner=0 --group=0 --sort=name -cf "$OUT/$name.tar" .
    rm -rf "${bundle:?}"
    say "the bundled source is $OUT/$name.tar"
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
    # A build with HDR won't load on GTK older than 4.16, so a second one goes along for those, and
    # install.sh keeps the one that fits. It's the lower floor, so it's what the symbols are read from.
    (cd "$REPO" && cargo build --release --locked -p guesthouse-app --no-default-features)
    cp "$target_folder/release/guesthouse" "$OUT/stage/guesthouse-no-hdr"
    write_symbol_needs "$OUT/stage/guesthouse-no-hdr"
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

# A build that isn't really one would only show up in a guest, much later.
check_display_driver() {
    for name in $DRIVER_BUILT; do
        [ -s "$DISPLAY_DRIVER/$name" ] || die "$DISPLAY_DRIVER has no $name. BUILDING.md says how to build it."
    done
    [ "$(head -c 2 "$DISPLAY_DRIVER/GuesthouseDisplay.dll")" = MZ ] ||
        die "$DISPLAY_DRIVER/GuesthouseDisplay.dll isn't a Windows program."
    ! grep -q '\$ARCH\$\|\$UMDFVERSION\$' "$DISPLAY_DRIVER/GuesthouseDisplay.inf" ||
        die "$DISPLAY_DRIVER/GuesthouseDisplay.inf is the source's, not the build's."
}

# Leaves the built driver with what the app needs to install it, and an id that tells builds apart.
copy_display_driver() {
    target=$OUT/stage/$DRIVER_FOLDER
    [ ! -d "$target" ] || rm -rf "${target:?}"
    if [ -z "$DISPLAY_DRIVER" ]; then
        say "there's no display driver in this build, so its Windows machines stay on the basic display"
        return 0
    fi
    check_display_driver
    mkdir -p "$target"
    for name in $DRIVER_BUILT; do
        cp "$DISPLAY_DRIVER/$name" "$target/"
    done
    for name in $DRIVER_SETUP; do
        cp "$REPO/$DRIVER_SOURCE/setup/$name" "$target/"
    done
    for name in $DRIVER_FILES; do
        cp "$REPO/$DRIVER_SOURCE/$name" "$target/"
    done
    (cd "$target" && cat GuesthouseDisplay.dll GuesthouseDisplay.inf | sha256sum | cut -d ' ' -f 1) >"$target/package.txt"
    say "the display driver is $(cat "$target/package.txt")"
}

release_parts() {
    parts=app
    [ ! -d "$OUT/stage/guest" ] || parts="$parts windows"
    [ ! -d "$OUT/stage/$DRIVER_FOLDER" ] || parts="$parts display"
    [ ! -d "$OUT/stage/linux" ] || parts="$parts linux"
    printf '%s' "$parts"
}

# Packs the stage under a folder of the archive's own name, without leaving a second copy behind.
pack_stage() {
    tar -C "$OUT/stage" --transform "s,^\.,$1,S" --owner=0 --group=0 --sort=name -cJf "$OUT/$1.tar.xz" .
}

stage_payload() {
    version=$(release_version)
    copy_guest_files
    copy_display_driver
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

# Everything the GPL asks for in one file: this repo with the patch and these scripts, QEMU's whole tree
# with its subprojects, and the Rust crates, so that nothing depends on anywhere else staying as it is.
stage_source() {
    [ -f "$OUT/stage/$DRIVER_FOLDER/GuesthouseDisplay.dll" ] ||
        die "a release can't go out without the display driver. Name the build with --display-driver."
    version=$(release_version)
    name=guesthouse-$version-source
    bundle=$OUT/source-bundle
    [ ! -d "$bundle" ] || rm -rf "${bundle:?}"
    mkdir -p "$bundle/qemu-source"
    git -C "$REPO" archive HEAD | tar -C "$bundle" -xf -
    tar -C "$(qemu_tree)" --exclude=.git -cf - . | tar -C "$bundle/qemu-source" -xf -
    # The crates are linked into the programs, and carry their licences with them here.
    (cd "$REPO" && cargo vendor --locked --quiet "$bundle/vendor" >/dev/null)
    tar -C "$bundle" --transform "s,^\.,$name,S" --owner=0 --group=0 --sort=name -cJf "$OUT/$name.tar.xz" .
    rm -rf "${bundle:?}"
    bundled=guesthouse-$version-bundled-source.tar
    [ -f "$OUT/$bundled" ] || bundled=""
    # shellcheck disable=SC2086 # empty, or one name with no spaces
    (cd "$OUT" && sha256sum "guesthouse-$version-$ARCH.tar.xz" "$name.tar.xz" $bundled >SHA256SUMS)
    say "the source is $OUT/$name.tar.xz"
    say "for RELEASE_VERSION, PAYLOAD_SHA256 and SOURCE_SHA256 in install.sh:"
    say "$version"
    head -n 2 "$OUT/SHA256SUMS" | cut -d ' ' -f 1
}

run_stage() {
    case $1 in
        qemu) stage_qemu ;;
        bundled) stage_bundled ;;
        app) stage_app ;;
        payload) stage_payload ;;
        source) stage_source ;;
        check-driver) check_display_driver ;;
        *) die "there's no stage called $1" ;;
    esac
}

main() {
    if [ "${1:-}" = --out ]; then
        OUT=$2
        shift 2
    fi
    if [ "${1:-}" = --display-driver ]; then
        DISPLAY_DRIVER=$2
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
