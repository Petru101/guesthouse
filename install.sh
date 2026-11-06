#!/bin/sh
# Installs or updates Guesthouse for the user who runs it, under their own folders.
#
# Read this before you run it. It's meant to be read, and it does nothing until the last line. It checks
# this PC, downloads one release into a folder of its own and verifies it, then shows you everything it
# will do and every command that needs root, and waits for you to say yes.
#
# It never installs or changes a GPU driver, kernel arguments or services, and it never needs root for
# anything but packages. --dry-run shows the plan and stops. Everything it puts on this PC is listed in
# ~/.local/lib/guesthouse/manifest, and uninstall.sh removes that and nothing else.
#
# The checksums below only prove that the download is the release this file names. They can't protect
# you from this file itself: if you don't trust it, read it first.
set -eu

# A release sets these three, last of all, once its files are up. Until the first one they're empty.
RELEASE_VERSION=""
PAYLOAD_SHA256=""
SOURCE_SHA256=""

REPO_URL=https://github.com/xnixy/guesthouse
RELEASE_URL=$REPO_URL/releases/download

APP_ID=net.gpupv.Guesthouse
ARCH=x86_64
MANIFEST_FORMAT=1
QEMU_PROGRAM=qemu/build/qemu-system-x86_64
FIRMWARE_DESCRIPTORS=/usr/share/qemu/firmware
# Room for the download, the unpacked files and the version being replaced.
ROOM_NEEDED_MIB=1024
CURL_SECONDS=20
TAB=$(printf '\t')


STAGING=""
CREATED_DIRS=""
KEEP_CREATED_DIRS=no
ROLLBACK=no
MOVED_ASIDE=""
VERSION_PLACED=no
OLD_LINK=""
LINK_FLIPPED=no
MANIFEST_BEFORE=""
PACKAGES_ADDED=""

say() {
    printf '%s\n' "$*"
}

warn() {
    printf 'install.sh: %s\n' "$*" >&2
}

die() {
    warn "$*"
    exit 1
}

usage() {
    cat <<'EOF'
Usage: install.sh [OPTION]

With nothing installed this installs the release it names, and with an older release installed it
updates to that one.

  --help          Show this
EOF
}

parse_arguments() {
    for argument in "$@"; do
        case $argument in
            --help)
                usage
                exit 0
                ;;
            *)
                usage >&2
                exit 2
                ;;
        esac
    done
}

# Everything goes in the user's own folders, and a root install would put it in root's.
refuse_root() {
    [ "$(id -u)" -ne 0 ] || die "don't run this as root. It asks for root itself, once, if it needs packages."
}

# The launcher's Exec line quotes the path, and these would need escaping there.
check_home() {
    [ -n "${HOME:-}" ] || die "HOME isn't set."
    case $HOME in
        /*) ;;
        *) die "HOME isn't an absolute path: $HOME" ;;
    esac
    case $HOME in
        *[\"\`\$\\%"$TAB"]* | *'
'*) die "your home folder's name has characters a launcher can't hold: $HOME" ;;
    esac
    [ -d "$HOME" ] || die "your home folder isn't there: $HOME"
}

# The app has the first of these written into it, as ~/.local/lib/guesthouse/current. See paths.rs.
set_paths() {
    LIB_ROOT=$HOME/.local/lib/guesthouse
    CURRENT_LINK=$LIB_ROOT/current
    MANIFEST=$LIB_ROOT/manifest
    case ${XDG_DATA_HOME:-} in
        /*) DATA_HOME=$XDG_DATA_HOME ;;
        *) DATA_HOME=$HOME/.local/share ;;
    esac
    LAUNCHER=$DATA_HOME/applications/$APP_ID.desktop
    ICON_FOLDER=$DATA_HOME/icons
    case ${XDG_STATE_HOME:-} in
        /*) STATE_HOME=$XDG_STATE_HOME ;;
        *) STATE_HOME=$HOME/.local/state ;;
    esac
    case ${XDG_RUNTIME_DIR:-} in
        /*) RUN_BASE=$XDG_RUNTIME_DIR/guesthouse ;;
        *) RUN_BASE=$STATE_HOME/guesthouse/runtime ;;
    esac
}

os_release_field() {
    sed -n "s/^$1=//p" /etc/os-release 2>/dev/null | head -n 1 | tr -d "\"'"
}

# A derivative says what it's built on in ID_LIKE, and its packages are named the same way.
find_family() {
    FAMILY=unknown
    for word in $(os_release_field ID) $(os_release_field ID_LIKE); do
        case $word in
            fedora) FAMILY=fedora ;;
            debian | ubuntu) FAMILY=debian ;;
            arch) FAMILY=arch ;;
            opensuse* | suse | sles) FAMILY=suse ;;
            nixos) FAMILY=nixos ;;
            *) continue ;;
        esac
        return
    done
}

# Package tools still run on these, but into an image, and only a reboot brings the change in.
find_immutable() {
    IMMUTABLE=no
    LAYER_COMMAND=""
    if [ -e /run/ostree-booted ]; then
        IMMUTABLE=yes
        LAYER_COMMAND="sudo rpm-ostree install"
    elif command -v transactional-update >/dev/null 2>&1; then
        IMMUTABLE=yes
        LAYER_COMMAND="sudo transactional-update pkg install"
    elif command -v steamos-readonly >/dev/null 2>&1; then
        IMMUTABLE=yes
    fi
}

refuse_unsupported() {
    [ "$(uname -m)" = "$ARCH" ] ||
        die "Guesthouse only runs on $ARCH, and this PC is $(uname -m). KVM can't run these guests anywhere else."
    [ "$FAMILY" != nixos ] && [ ! -e /etc/NIXOS ] ||
        die "NixOS isn't supported: the release's programs expect a normal /lib and /usr, and this script can't declare anything in your configuration."
    if ldd --version 2>&1 | grep -qi musl; then
        die "musl isn't supported: the release is built against glibc, and the app needs it."
    fi
}

how_supported() {
    case $FAMILY in
        fedora) say "this is what I run it on" ;;
        debian | arch) say "should work, tested without a GPU" ;;
        suse) say "best effort, and the package names are untested" ;;
        *) say "I don't know this distro, so packages are up to you" ;;
    esac
}

cpu_has_virtualization() {
    grep -Eq '^flags.*\b(vmx|svm)\b' /proc/cpuinfo 2>/dev/null
}

kvm_usable() {
    [ -r /dev/kvm ] && [ -w /dev/kvm ]
}

# The fix is a group, and only when the group is the reason. Most systems open /dev/kvm to whoever's at
# the seat, so this is rare.
kvm_group_would_help() {
    [ -e /dev/kvm ] && ! kvm_usable || return 1
    grep -q '^kvm:' /etc/group 2>/dev/null || return 1
    case " $(id -Gn) " in
        *" kvm "*) return 1 ;;
    esac
}

report_host() {
    say "Checking this PC. Nothing here needs root or changes anything."
    say "  Distro: $(os_release_field PRETTY_NAME) ($(how_supported))"
    [ "$IMMUTABLE" = no ] || say "  Immutable: yes, so this only puts files in your home folder"
    if cpu_has_virtualization; then
        say "  CPU: has virtualization"
    else
        warn "this CPU doesn't show vmx or svm. Turn on SVM or VT-x in the firmware settings, or no VM will run."
    fi
    if kvm_usable; then
        say "  KVM: /dev/kvm can be opened"
    elif [ -e /dev/kvm ]; then
        warn "you can't open /dev/kvm, so VMs would crawl on the CPU alone."
    else
        warn "/dev/kvm isn't here, so VMs would crawl on the CPU alone."
    fi
}

require_tools() {
    for tool in curl tar xz sha256sum sed awk find mktemp df; do
        command -v "$tool" >/dev/null 2>&1 || die "this needs $tool, and it's not installed."
    done
}

manifest_value() {
    [ -f "$MANIFEST" ] || return 0
    awk -F "$TAB" -v key="$1" '$1 == key { print $3; exit }' "$MANIFEST"
}

fetch() {
    curl --proto '=https' --tlsv1.2 -fL --retry 3 --connect-timeout "$CURL_SECONDS" "$@"
}

newer_than() {
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

ensure_dir() {
    [ ! -d "$1" ] || return 0
    ensure_dir "$(dirname "$1")"
    mkdir "$1"
    CREATED_DIRS="$CREATED_DIRS$1
"
}

remove_created_dirs() {
    printf '%s' "$CREATED_DIRS" | sort -r | while IFS= read -r folder; do
        [ -z "$folder" ] || rmdir "$folder" 2>/dev/null || true
    done
}

# Staging is on the same disk as the install. /tmp is often RAM, can be noexec, and a move from it
# wouldn't be one step. A killed run leaves a folder of this name, and only this script makes those.
make_staging() {
    ensure_dir "$LIB_ROOT"
    for stale in "$LIB_ROOT"/.incoming.*; do
        [ ! -e "$stale" ] || rm -rf -- "${stale:?}"
    done
    STAGING=$(mktemp -d "$LIB_ROOT/.incoming.XXXXXX")
}

check_room() {
    free_kib=$(df -Pk "$LIB_ROOT" | awk 'NR == 2 { print $4 }')
    [ "$free_kib" -ge $((ROOM_NEEDED_MIB * 1024)) ] ||
        die "this needs ${ROOM_NEEDED_MIB} MiB free in your home folder, and there's $((free_kib / 1024)) MiB."
}

download_verified() {
    name=$1
    wanted=$2
    url=$RELEASE_URL/v$RELEASE_VERSION/$name
    say "Downloading $url"
    fetch -# -o "$STAGING/$name" "$url" || die "couldn't download $url"
    found=$(sha256sum "$STAGING/$name" | cut -d ' ' -f 1)
    [ "$found" = "$wanted" ] || die "$name isn't the file this script expects, so I'm stopping.
  expected $wanted
  got      $found"
    say "Verified $name"
}

get_payload() {
    payload=guesthouse-$RELEASE_VERSION-$ARCH.tar.xz
    download_verified "$payload" "$PAYLOAD_SHA256"
    tar -xJf "$STAGING/$payload" -C "$STAGING" --no-same-owner || die "$payload wouldn't unpack."
    TREE=$STAGING/guesthouse-$RELEASE_VERSION-$ARCH
    [ -f "$TREE/RELEASE" ] || die "$payload has no RELEASE file, so it's not a Guesthouse release."
    [ "$(release_field version)" = "$RELEASE_VERSION" ] ||
        die "$payload says it's version $(release_field version), not $RELEASE_VERSION."
}

release_field() {
    sed -n "s/^$1=//p" "$TREE/RELEASE" | head -n 1
}

# Without this, the failure would be a loader error the first time someone opens the app.
check_glibc() {
    needed=$(release_field glibc)
    [ -n "$needed" ] || return 0
    have=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{ print $2 }')
    if newer_than "$needed" "${have:-0}"; then
        die "this release needs glibc $needed, and this system has ${have:-an unknown one}. A newer distro release would do, or --from-source if you have the tools to build it."
    fi
}

# The release's own program does the looking, so what a GPU needs is a backend's business and not this
# script's. It needs nothing but glibc, and it's only ever asked questions.
ask_the_release() {
    HOST_REPORT=$STAGING/host-report
    GPU_ANSWER=no
    if [ -x "$TREE/guesthouse-check" ] &&
        GUESTHOUSE_PROGRAM=$TREE "$TREE/guesthouse-check" >"$HOST_REPORT" 2>/dev/null; then
        GPU_ANSWER=$(awk -F "$TAB" '$1 == "gpu" { print $2 }' "$HOST_REPORT")
    else
        : >"$HOST_REPORT"
    fi
}

print_host_report() {
    awk -F "$TAB" '$1 == "check" {
        print "  " $3 ": " $4
        if ($2 != "ok" && $5 != "") print "    " $5
    }' "$HOST_REPORT"
}

on_exit() {
    status=$?
    trap - EXIT
    [ "$ROLLBACK" = no ] || [ "$status" -eq 0 ] || roll_back
    [ -z "$STAGING" ] || rm -rf -- "${STAGING:?}"
    [ "$KEEP_CREATED_DIRS" = yes ] || remove_created_dirs
    exit "$status"
}

# Which release this run is about, and whether there's anything to do.
choose_release() {
    installed=$(manifest_value version)
    [ -n "$RELEASE_VERSION" ] || die "there's no release yet, so there's nothing to install."
    if [ -n "$installed" ] && newer_than "$installed" "$RELEASE_VERSION"; then
        die "you have $installed, which is newer than $RELEASE_VERSION. I won't go backwards."
    fi
    # A plain install of this version goes on, as the GPU may be able to run now.
    if [ "$installed" = "$RELEASE_VERSION" ] && [ "$(manifest_value mode)" = gpu ]; then
        say "Guesthouse $installed is the latest release."
        exit 0
    fi
}

main() {
    parse_arguments "$@"
    refuse_root
    check_home
    set_paths
    find_family
    find_immutable
    refuse_unsupported
    report_host
    require_tools
    choose_release

    trap on_exit EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    make_staging
    check_room
    get_payload
    check_glibc
    ask_the_release
    print_host_report
    say "Nothing installs yet, so that's all for now."
}

# In braces, so that a download cut off anywhere in this line, even right after `main`, is a syntax error.
{ main "$@"; }
