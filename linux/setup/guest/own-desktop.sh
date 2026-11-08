# Sourced by gpu-up.sh and desktop-watch.sh, in a guest someone installed by hand. Its own display
# manager and desktop go on the GPU, through nvidia-drm and the stand-in, as Ubuntu's desktop does. GDM
# is the one set up for it. Anything else, or a GPU screen that never lights, keeps the plain screen.
# shellcheck shell=bash disable=SC2034,SC2154  # sourced after common.sh, which sets root and the rest
desktop_failed=$root/var/lib/nvfwd/desktop-failed
# systemd reads these into every user manager it starts, the login screen's included, and /run goes
# with the boot, so a boot with no GPU gets plain Mesa.
session_settings=$root/run/environment.d/90-nvfwd.conf
# GDM before 49 turns Wayland off when it sees an NVIDIA module without NVIDIA's suspend helpers. Here
# the module is ours, and there's no X driver for it, so those rules are left out of a GPU boot.
gdm_rules=61-gdm.rules
gdm_rules_override=$root/run/udev/rules.d/$gdm_rules

# The unit display-manager.service stands for, such as gdm.service, or nothing.
display_manager() {
    local link
    link=$(readlink "$root/etc/systemd/system/display-manager.service" 2>/dev/null) || return 0
    basename "$link"
}

is_gdm() {
    case $1 in
    gdm.service | gdm3.service) return 0 ;;
    esac
    return 1
}

# Why this boot's desktop stays on the plain screen, or nothing if it can go on the GPU.
# Usage: plain_desktop_reason BUILT_FOR
plain_desktop_reason() {
    local manager
    manager=$(display_manager)
    if [ "$(value_of "$own_install" desktop)" = plain ]; then
        echo "desktop=plain in /etc/nvfwd/own-install.conf keeps it there"
    elif [ -z "$manager" ]; then
        echo "no display manager is enabled"
    elif ! is_gdm "$manager"; then
        echo "the display manager is ${manager%.service}, and only GDM is set up for the GPU so far"
    elif [ "$(cat "$desktop_failed" 2>/dev/null)" = "$1" ]; then
        echo "GDM didn't come up on the GPU on an earlier boot with this driver and kernel. Delete /var/lib/nvfwd/desktop-failed to try again"
    fi
}

# Where the desktop is, in the state file and to the host. Usage: say_desktop gpu|plain [REASON]
say_desktop() {
    sed -i -e '/^desktop=/d' -e '/^desktop_reason=/d' "$state_file"
    echo "desktop=$1" >> "$state_file"
    if [ "$1" = gpu ]; then
        tell_host "nvfwd-desktop gpu"
    else
        echo "desktop_reason=$2" >> "$state_file"
        tell_host "nvfwd-desktop plain $2"
    fi
}

hide_gdm_rules() {
    local folder
    for folder in "$root/usr/lib/udev/rules.d" "$root/lib/udev/rules.d"; do
        if [ -e "$folder/$gdm_rules" ] && [ ! -e "$root/etc/udev/rules.d/$gdm_rules" ]; then
            mkdir -p "$(dirname "$gdm_rules_override")"
            : > "$gdm_rules_override"
            udevadm control --reload
            return
        fi
    done
}

# Ubuntu's session settings, and no glamor for Xwayland. Glamor would load Zink, and NVIDIA's Vulkan
# driver opens DISPLAY as it loads, which is that same Xwayland, so it would wait on itself. Ubuntu's
# own install wraps Xwayland for that instead, which a package manager here would undo.
write_session_settings() {
    mkdir -p "$(dirname "$session_settings")"
    {
        cat "$here/files/gpu-session.env"
        echo XWAYLAND_NO_GLAMOR=1
    } > "$session_settings"
}

# Whether a compositor has lit our card's screen.
gpu_screen_lit() {
    local card enabled
    for card in "$root"/sys/class/drm/card[0-9]; do
        [ "$(cat "$card/device/device" 2>/dev/null)" = "$nvfwd_device" ] || continue
        for enabled in "$card"-*/enabled; do
            [ "$(cat "$enabled" 2>/dev/null)" = enabled ] && return 0
        done
    done
    return 1
}

# Takes the desktop back to the plain screen, remembers that for this driver and kernel, and starts
# the display manager again there. Usage: back_to_plain REASON BUILT_FOR
back_to_plain() {
    local manager name
    mkdir -p "$(dirname "$desktop_failed")"
    echo "$2" > "$desktop_failed"
    rm -f "$session_settings" "$gdm_rules_override"
    ignore_cards ours
    say_desktop plain "$1"
    manager=$(display_manager)
    [ -n "$manager" ] || return 0
    systemctl stop "$manager"
    # The login screen's user manager keeps the settings it started with, so it goes too.
    for name in $(loginctl list-users --no-legend 2>/dev/null | awk '{ print $2 }'); do
        case $name in
        gdm | gdm-* | Debian-gdm) loginctl terminate-user "$name" ;;
        esac
    done
    systemctl start "$manager"
}
