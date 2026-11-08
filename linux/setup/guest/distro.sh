# Sourced by setup-own.sh and build-parts.sh. What a guest installed by hand is, and how to ask its
# package manager for things. One family per package manager: fedora (dnf), debian (apt), arch (pacman)
# and suse (zypper).
# shellcheck shell=bash disable=SC2034,SC2154  # sourced after common.sh, which sets root
known_families='Fedora, Debian, Ubuntu, Arch or openSUSE, or a distribution built on one of them'

# One value from an os-release file, without its quotes.
os_value() {
    sed -n "s/^$2=//p" "$1" | head -n 1 | tr -d "\"'"
}

# Prints the family for an os-release file, from ID and then ID_LIKE. Fails for any other.
distro_family() {
    local word
    for word in $(os_value "$1" ID) $(os_value "$1" ID_LIKE); do
        case $word in
        fedora | rhel | centos) echo fedora; return 0 ;;
        debian | ubuntu) echo debian; return 0 ;;
        arch | archlinux) echo arch; return 0 ;;
        suse | opensuse | opensuse-tumbleweed | opensuse-leap | sles) echo suse; return 0 ;;
        esac
    done
    return 1
}

package_manager() {
    case $1 in
    fedora) echo dnf ;;
    debian) echo apt-get ;;
    arch) echo pacman ;;
    suse) echo zypper ;;
    esac
}

# Where the family keeps 64-bit libraries, which NVIDIA's GBM and GLX pieces are linked into.
libraries_folder() {
    case $1 in
    fedora | suse) echo /usr/lib64 ;;
    arch) echo /usr/lib ;;
    debian) echo /usr/lib/x86_64-linux-gnu ;;
    esac
}

# A Debian or Ubuntu kernel's flavour, such as generic, amd64 or cloud-amd64.
debian_flavour() {
    echo "$1" | sed -E 's/^[0-9.]+(-[0-9]+)?(\+[^-]+)?-//'
}

# The package with the headers for this exact kernel.
headers_package() {
    local family=$1 kernel=$2 base
    case $family in
    fedora) echo "kernel-devel-$kernel" ;;
    debian) echo "linux-headers-$kernel" ;;
    arch)
        # Each of Arch's kernels says which package it came from: linux, linux-lts, linux-zen.
        base=$(cat "$root/usr/lib/modules/$kernel/pkgbase" 2>/dev/null) || base=linux
        echo "$base-headers"
        ;;
    suse) echo "kernel-${kernel##*-}-devel" ;;
    esac
}

# A package that brings the next kernel's headers with the next kernel, if the family needs one.
# Fedora's kernel-devel, Arch's headers and openSUSE's devel packages are updated with the kernel.
headers_follower() {
    case $1 in
    debian) echo "linux-headers-$(debian_flavour "$2")" ;;
    esac
}

# The words to install packages with, with no questions asked.
install_command() {
    case $1 in
    fedora) echo dnf install -y ;;
    debian) echo apt-get install -y --no-install-recommends ;;
    arch) echo pacman -S --needed --noconfirm ;;
    suse) echo zypper --non-interactive install --no-recommends ;;
    esac
}

family_install() {
    local family=$1
    shift
    # shellcheck disable=SC2046  # the command is meant to split into words
    DEBIAN_FRONTEND=noninteractive $(install_command "$family") "$@"
}

# Fresh package lists, where the family needs them before an install. Arch's are left alone: a sync
# without an upgrade is the partial upgrade Arch warns against.
family_refresh() {
    case $1 in
    debian) apt-get update ;;
    suse) zypper --non-interactive refresh ;;
    esac
}

# What the family installs for the build and the checks, one package a word.
family_packages() {
    grep -v '^#' "$2/files/own-packages-$1.txt"
}

# How the family builds for a new kernel as it arrives. Every family also builds at boot.
hook_kind() {
    case $1 in
    fedora) echo kernel-install ;;
    debian) echo kernel-postinst ;;
    arch) echo pacman ;;
    suse) echo none ;;
    esac
}
