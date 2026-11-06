# What must never happen, whatever the PC looks like.

t_root_is_refused() {
    rm -f "$T/bin/id"
    printf '#!/bin/sh\necho 0\n' >"$T/bin/id"
    chmod +x "$T/bin/id"
    no_terminal "$SHELL_UNDER_TEST" "$REPO/install.sh"
    check_exit 1
    check_said "don't run this as root"
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
