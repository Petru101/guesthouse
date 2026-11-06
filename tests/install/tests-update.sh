# Two pretend releases, and everything that can go wrong between them.

two_releases() {
    release 1.0 "${1:-windows}"
    release 2.0 "${1:-windows}"
    give_everything
    install_version 1.0
    [ "$status" -eq 0 ] || fail "the first install failed"
    make_main_name 2.0
    root=$H/.local/lib/guesthouse
    mkdir -p "$H/.local/share/guesthouse/machines/win" "$H/.config/guesthouse"
    echo disk >"$H/.local/share/guesthouse/machines/win/disk.qcow2"
    echo '{"mine": 1}' >"$H/.config/guesthouse/app.json"
    cp "$root/manifest" "$T/manifest.1.0"
    (cd "$H" && find .local/share/guesthouse .config -type f -exec sha256sum {} + | sort) >"$T/user-files"
}

check_user_files_kept() {
    (cd "$H" && find .local/share/guesthouse .config -type f -exec sha256sum {} + | sort) >"$T/user-files.now"
    check_same "$T/user-files" "$T/user-files.now" "the machines or settings changed"
}

# The first version is still what runs, all of it, and uninstall still knows all of it.
check_old_version_works() {
    [ "$(readlink "$root/current")" = 1.0 ] || fail "current moved"
    [ "$(current_says)" = "guesthouse 1.0" ] || fail "version 1.0 doesn't run"
    check_same "$T/manifest.1.0" "$root/manifest" "the manifest changed"
    check_gone "$root/2.0"
    [ -z "$(find "$root" -name '.incoming*' -o -name '*.new')" ] || fail "something was left behind"
    check_user_files_kept
}

t_update_moves_to_the_latest_release() {
    two_releases
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 0
    check_said "update Guesthouse from 1.0 to 2.0"
    check_said "changed"
    check_said "Your machines, disks and settings stay"
    [ "$(readlink "$root/current")" = 2.0 ] || fail "current isn't 2.0"
    [ "$(current_says)" = "guesthouse 2.0" ] || fail "version 2.0 doesn't run"
    check_gone "$root/1.0"
    check_there "$root/2.0/guest/setup.ps1"
    check_user_files_kept
    ! grep -q '/1\.0/' "$root/manifest" || fail "the manifest still lists 1.0"
    [ "$(awk -F '\t' '$1 == "version" { print $3 }' "$root/manifest")" = 2.0 ] || fail "the manifest's version"
}

t_running_the_new_script_updates_too() {
    two_releases
    install_version 2.0
    check_exit 0
    [ "$(current_says)" = "guesthouse 2.0" ] || fail "version 2.0 doesn't run"
}

t_uninstall_after_an_update_leaves_only_what_was_yours() {
    release 1.0 windows
    release 2.0 windows
    give_everything
    snapshot >"$T/empty"
    install_version 1.0
    install_version 2.0
    check_exit 0
    answering 'yes\n' "$SHELL_UNDER_TEST" "$H/.local/lib/guesthouse/current/uninstall.sh"
    check_exit 0
    check_home_is "$T/empty"
}

t_up_to_date_is_said_and_nothing_is_downloaded() {
    two_releases
    make_main_name 1.0
    : >"$T/log"
    snapshot >"$T/before"
    answering '' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 0
    check_said "1.0 is the latest release"
    check_not_log "releases/download"
    check_home_is "$T/before"
}

t_going_backwards_is_refused() {
    two_releases
    install_version 2.0
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(script_for 1.0)"
    check_exit 1
    check_said "won't go backwards"
}

t_update_with_nothing_installed_says_so() {
    release 1.0
    make_main_name 1.0
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)" --update
    check_exit 1
    check_said "nothing to update"
}

t_update_waits_for_a_vm() {
    two_releases
    start_process_as "$root/1.0/qemu/build/qemu-copy"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    stop_process
    rm -f "$root/1.0/qemu/build/qemu-copy"
    check_exit 1
    check_said "Guesthouse is running"
    check_old_version_works
}

t_a_bad_hash_leaves_the_old_version() {
    two_releases
    printf 'x' >>"$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_old_version_works
}

t_a_download_that_fails_leaves_the_old_version() {
    two_releases
    rm -f "$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_said "couldn't download"
    check_old_version_works
}

t_a_broken_archive_with_the_right_hash_leaves_the_old_version() {
    two_releases
    head -c 500 "$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz" >"$T/short"
    mv "$T/short" "$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz"
    wanted=$(sha256sum "$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz" | cut -d ' ' -f 1)
    sed -i "s/^PAYLOAD_SHA256=.*/PAYLOAD_SHA256=\"$wanted\"/" "$T/releases/main-install.sh"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_old_version_works
}

# A stand-in for one tool that fails on one path, which is how a full disk or a bad moment looks.
fail_when() {
    local tool=$1 pattern=$2 real
    real=$(readlink -f "$T/bin/$tool")
    rm -f "$T/bin/$tool"
    cat >"$T/bin/$tool" <<EOF
#!/bin/sh
case "\$*" in
    $pattern) exit 1 ;;
esac
exec "$real" "\$@"
EOF
    chmod +x "$T/bin/$tool"
}

t_failing_to_place_the_version_leaves_the_old_one() {
    two_releases
    fail_when mv "*/guesthouse/2.0"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_said "putting everything back"
    check_old_version_works
}

t_failing_on_an_icon_leaves_the_old_one() {
    release 1.0 windows
    release 2.0 windows
    give_everything
    install_version 1.0
    root=$H/.local/lib/guesthouse
    make_main_name 2.0
    # New icons, so this update really has some to place, and the second one fails.
    tree=$T/releases/tree-2.0
    echo '<svg/>' >"$tree/guesthouse-2.0-x86_64/share/icons/hicolor/scalable/apps/net.gpupv.Guesthouse.svg"
    echo '<svg/>' >"$tree/guesthouse-2.0-x86_64/share/icons/hicolor/symbolic/apps/net.gpupv.Guesthouse-symbolic.svg"
    tar -C "$tree" -cJf "$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz" guesthouse-2.0-x86_64
    wanted=$(sha256sum "$T/releases/v2.0/guesthouse-2.0-x86_64.tar.xz" | cut -d ' ' -f 1)
    sed -i "s/^PAYLOAD_SHA256=.*/PAYLOAD_SHA256=\"$wanted\"/" "$T/releases/main-install.sh"
    mkdir -p "$H/.local/share/guesthouse" "$H/.config"
    cp "$root/manifest" "$T/manifest.1.0"
    (cd "$H" && find .local/share/guesthouse .config -type f -exec sha256sum {} + | sort) >"$T/user-files"
    old_icon=$(sha256sum "$H/.local/share/icons/hicolor/scalable/apps/net.gpupv.Guesthouse.svg")
    fail_when mv "*Guesthouse-symbolic.svg*"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_old_version_works
    [ "$(sha256sum "$H/.local/share/icons/hicolor/scalable/apps/net.gpupv.Guesthouse.svg")" = "$old_icon" ] ||
        fail "the old icon wasn't put back"
}

t_failing_at_the_last_step_leaves_the_old_one() {
    two_releases
    fail_when ln "*current.new"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_said "putting everything back"
    check_old_version_works
}

t_being_stopped_midway_leaves_the_old_one() {
    two_releases
    # Stops the run from inside, with the version in place and current not moved yet.
    real=$(readlink -f "$T/bin/ln")
    rm -f "$T/bin/ln"
    printf '#!/bin/sh\nkill -TERM $PPID\nsleep 1\nexec "%s" "$@"\n' "$real" >"$T/bin/ln"
    chmod +x "$T/bin/ln"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    [ "$status" -ne 0 ] || fail "it should have stopped"
    rm -f "$T/bin/ln"
    ln -s "$real" "$T/bin/ln"
    check_old_version_works
}

t_a_kill_with_no_chance_to_clean_up_can_still_be_uninstalled() {
    release 1.0 windows
    release 2.0 windows
    give_everything
    snapshot >"$T/empty"
    install_version 1.0
    root=$H/.local/lib/guesthouse
    make_main_name 2.0
    real=$(readlink -f "$T/bin/ln")
    rm -f "$T/bin/ln"
    printf '#!/bin/sh\nkill -KILL $PPID\nexit 1\n' >"$T/bin/ln"
    chmod +x "$T/bin/ln"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    rm -f "$T/bin/ln"
    ln -s "$real" "$T/bin/ln"
    [ "$(current_says)" = "guesthouse 1.0" ] || fail "version 1.0 doesn't run"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_home_is "$T/empty"
}

t_the_update_after_a_kill_works() {
    two_releases
    real=$(readlink -f "$T/bin/ln")
    rm -f "$T/bin/ln"
    printf '#!/bin/sh\nkill -KILL $PPID\nexit 1\n' >"$T/bin/ln"
    chmod +x "$T/bin/ln"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    rm -f "$T/bin/ln"
    ln -s "$real" "$T/bin/ln"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 0
    [ "$(current_says)" = "guesthouse 2.0" ] || fail "version 2.0 doesn't run"
    check_gone "$root/1.0"
    [ -z "$(find "$root" -name '.incoming*')" ] || fail "staging was left behind"
}

t_a_driver_on_another_branch_is_explained_and_nothing_breaks() {
    two_releases
    export FAKE_GPU_REFUSED="The host driver is on branch 620 and the guest driver on branch 617."
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 0
    check_said "branch 620"
    check_said "host and guest have to match"
    check_said "plain VMs still run"
    check_there "$root/2.0/guest/setup.ps1"
    [ "$(awk -F '\t' '$1 == "mode" { print $3 }' "$root/manifest")" = gpu ] || fail "it stopped being a GPU install"
    check_user_files_kept
}

t_a_plain_install_gains_the_gpu_when_the_host_can_run_it() {
    release 1.0 windows
    give_everything
    export FAKE_GPU_REFUSED="No card."
    install_version 1.0
    root=$H/.local/lib/guesthouse
    check_gone "$root/1.0/guest"
    unset FAKE_GPU_REFUSED
    install_version 1.0
    check_exit 0
    check_there "$root/1.0/guest/setup.ps1"
    [ "$(current_says)" = "guesthouse 1.0" ] || fail "it doesn't run"
}

t_the_update_check_answers_in_one_line() {
    two_releases
    snapshot >"$T/before"
    no_terminal "$SHELL_UNDER_TEST" "$root/current/install.sh" --check-update
    check_exit 10
    [ "$(cat "$T/said")" = "update-available 1.0 2.0" ] || fail "said: $(cat "$T/said")"
    make_main_name 1.0
    no_terminal "$SHELL_UNDER_TEST" "$root/current/install.sh" --check-update
    check_exit 0
    [ "$(cat "$T/said")" = "up-to-date 1.0" ] || fail "said: $(cat "$T/said")"
    rm -f "$T/releases/main-install.sh"
    no_terminal "$SHELL_UNDER_TEST" "$root/current/install.sh" --check-update
    check_exit 1
    [ "$(cat "$T/said")" = "unknown 1.0" ] || fail "said: $(cat "$T/said")"
    check_home_is "$T/before"
}

t_the_update_check_runs_nothing_it_downloads() {
    two_releases
    {
        printf 'touch "%s"\n' "$T/ran"
        cat "$(script_for 2.0)"
    } >"$T/releases/main-install.sh"
    no_terminal "$SHELL_UNDER_TEST" "$root/current/install.sh" --check-update
    check_exit 10
    check_gone "$T/ran"
}

t_nonsense_from_main_is_not_believed() {
    two_releases
    printf 'RELEASE_VERSION="$(reboot)"\nPAYLOAD_SHA256="x"\nSOURCE_SHA256="y"\n' >"$T/releases/main-install.sh"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$root/current/install.sh" --update
    check_exit 1
    check_old_version_works
}
