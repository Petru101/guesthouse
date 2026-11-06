# What must never happen, whatever the PC looks like.

t_root_is_refused() {
    rm -f "$T/bin/id"
    printf '#!/bin/sh\necho 0\n' >"$T/bin/id"
    chmod +x "$T/bin/id"
    no_terminal "$SHELL_UNDER_TEST" "$REPO/install.sh"
    check_exit 1
    check_said "don't run this as root"
}

t_a_cut_off_download_does_nothing() {
    release 1.0
    give_everything
    local script size cut before
    script=$(script_for 1.0)
    size=$(wc -c <"$script")
    snapshot >"$T/before"
    # Every byte of the last lines, where the call is, then spread over the rest. Only the final line
    # break can go and leave the whole script.
    for cut in $(seq $((size - 120)) $((size - 2))) $(seq 200 1777 "$size"); do
        head -c "$cut" "$script" >"$T/cut.sh"
        : >"$T/log"
        answering 'yes\n' "$SHELL_UNDER_TEST" "$T/cut.sh"
        [ ! -s "$T/log" ] || fail "cut at $cut reached a stand-in"
        grep -qF 'Checking this PC' "$T/said" && fail "cut at $cut ran main"
    done
    check_home_is "$T/before"
}

t_there_is_nothing_to_install_before_a_release() {
    snapshot >"$T/before"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$REPO/install.sh"
    check_exit 1
    check_said "there's no release yet"
    check_home_is "$T/before"
}

t_another_architecture_is_refused() {
    printf '#!/bin/sh\necho aarch64\n' >"$T/bin/uname.new"
    chmod +x "$T/bin/uname.new"
    mv -f "$T/bin/uname.new" "$T/bin/uname"
    no_terminal "$SHELL_UNDER_TEST" "$REPO/install.sh"
    check_exit 1
    check_said "only runs on x86_64"
}

t_nixos_is_refused() {
    fake_os nixos
    no_terminal "$SHELL_UNDER_TEST" "$REPO/install.sh"
    check_exit 1
    check_said "NixOS isn't supported"
}

t_musl_is_refused() {
    rm -f "$T/bin/ldd"
    printf '#!/bin/sh\necho "musl libc (x86_64)" >&2\n' >"$T/bin/ldd"
    chmod +x "$T/bin/ldd"
    no_terminal "$SHELL_UNDER_TEST" "$REPO/install.sh"
    check_exit 1
    check_said "musl isn't supported"
}

t_a_wrong_download_stops_before_anything_changes() {
    release 1.0
    give_everything
    printf 'tampered' >>"$T/releases/v1.0/guesthouse-1.0-x86_64.tar.xz"
    snapshot >"$T/before"
    install_version 1.0
    check_exit 1
    check_said "isn't the file this script expects"
    check_home_is "$T/before"
}
