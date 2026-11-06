# What ends up where.

t_it_installs_where_the_app_looks() {
    release 1.0 windows
    give_everything
    install_version 1.0
    check_exit 0
    root=$H/.local/lib/guesthouse
    [ "$(readlink "$root/current")" = 1.0 ] || fail "current isn't 1.0"
    check_there "$root/1.0/qemu/build/qemu-system-x86_64"
    check_there "$root/1.0/guest/setup.ps1"
    check_there "$root/manifest"
    check_there "$H/.local/share/applications/net.gpupv.Guesthouse.desktop"
    check_there "$H/.local/share/icons/hicolor/scalable/apps/net.gpupv.Guesthouse.svg"
    check_there "$H/.local/share/icons/hicolor/symbolic/apps/net.gpupv.Guesthouse-symbolic.svg"
    [ -z "$(find "$root" -name '.incoming*')" ] || fail "staging was left behind"
}

t_the_launcher_finds_the_program_with_no_help_from_path() {
    release 1.0
    give_everything
    install_version 1.0
    grep -qxF "Exec=env \"GUESTHOUSE_PROGRAM=$H/.local/lib/guesthouse/current\" \"$H/.local/lib/guesthouse/current/guesthouse\"" \
        "$H/.local/share/applications/net.gpupv.Guesthouse.desktop" || fail "the launcher's Exec is wrong"
}

t_a_card_that_cannot_run_still_installs_plain_vms() {
    release 1.0 "windows linux"
    give_everything
    export FAKE_GPU_REFUSED="No supported card."
    install_version 1.0
    check_exit 0
    check_said "plain VMs only"
    check_said "No supported card."
    check_there "$H/.local/lib/guesthouse/current/qemu/build/qemu-system-x86_64"
    check_gone "$H/.local/lib/guesthouse/1.0/guest"
    check_gone "$H/.local/lib/guesthouse/1.0/linuxguest"
}

t_a_release_with_no_gpu_part_says_so() {
    release 1.0
    give_everything
    install_version 1.0
    check_exit 0
    check_said "no GPU part"
}

t_a_check_that_will_not_run_means_plain_vms() {
    release 1.0 windows
    give_everything
    tree=$T/releases/tree-1.0
    rm -f "$tree/guesthouse-1.0-x86_64/guesthouse-check"
    tar -C "$tree" -cJf "$T/releases/v1.0/guesthouse-1.0-x86_64.tar.xz" guesthouse-1.0-x86_64
    wanted=$(sha256sum "$T/releases/v1.0/guesthouse-1.0-x86_64.tar.xz" | cut -d ' ' -f 1)
    sed -i "s/^PAYLOAD_SHA256=.*/PAYLOAD_SHA256=\"$wanted\"/" "$(script_for 1.0)"
    install_version 1.0
    check_exit 0
    check_gone "$H/.local/lib/guesthouse/1.0/guest"
}

t_a_glibc_too_old_is_said_plainly() {
    release 1.0
    give_everything
    printf '#!/bin/sh\necho glibc 2.20\n' >"$T/bin/getconf.new"
    chmod +x "$T/bin/getconf.new"
    mv -f "$T/bin/getconf.new" "$T/bin/getconf"
    snapshot >"$T/before"
    install_version 1.0
    check_exit 1
    check_said "needs glibc 2.30"
    check_home_is "$T/before"
}


# A source bundle whose build script just packs what make-release made, to prove the path around it.
