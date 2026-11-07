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
    sed -e 's/^RELEASE_VERSION=.*/RELEASE_VERSION=""/' -e 's/^PAYLOAD_SHA256=.*/PAYLOAD_SHA256=""/' \
        -e 's/^SOURCE_SHA256=.*/SOURCE_SHA256=""/' "$REPO/install.sh" >"$T/unreleased.sh"
    snapshot >"$T/before"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$T/unreleased.sh"
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

t_saying_no_changes_nothing() {
    release 1.0
    fake_os fedora
    snapshot >"$T/before"
    answering 'no\n' "$SHELL_UNDER_TEST" "$(script_for 1.0)"
    check_exit 1
    check_not_log sudo
    check_home_is "$T/before"
}

t_with_no_terminal_nothing_is_installed() {
    release 1.0
    give_everything
    snapshot >"$T/before"
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)"
    check_exit 1
    check_said "I need a terminal"
    check_home_is "$T/before"
}

t_a_dry_run_changes_nothing() {
    release 1.0
    snapshot >"$T/before"
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)" --dry-run
    check_exit 0
    check_said "sudo sh -c 'dnf install -y gtk4"
    check_said "That was a dry run"
    check_not_log "sudo "
    check_home_is "$T/before"
}

t_a_file_it_did_not_install_is_not_replaced() {
    release 1.0
    give_everything
    mkdir -p "$H/.local/share/applications"
    echo mine >"$H/.local/share/applications/io.github.xnixy.Guesthouse.desktop"
    snapshot >"$T/before"
    install_version 1.0
    check_exit 1
    check_said "this script didn't put it there"
    check_home_is "$T/before"
}

t_a_toolkit_that_is_too_old_stops_it() {
    release 1.0
    give_everything
    printf 'adw_something_older' >"$T/libraries/libadwaita-1.so.0"
    snapshot >"$T/before"
    install_version 1.0
    check_exit 1
    check_said "libadwaita on this system is too old"
    check_said "adw_new_thing"
    check_not_log "sudo "
    check_home_is "$T/before"
}
