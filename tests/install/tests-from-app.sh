# The app updating itself: it runs the script with no terminal, and is itself running from the install.

t_the_app_updates_itself_while_it_runs() {
    two_releases
    start_process_as "$root/1.0/guesthouse"
    no_terminal "$SHELL_UNDER_TEST" "$root/current/install.sh" --update "--from-app=$process"
    stop_process
    check_exit 0
    check_not_said "Guesthouse is running"
    check_not_said "couldn't be removed"
    check_not_log "sudo"
    check_not_log "pkexec"
    [ "$(current_says)" = "guesthouse 2.0" ] || fail "version 2.0 doesn't run"
    check_gone "$root/1.0/guest"
    check_user_files_kept
}

t_the_app_still_waits_for_a_vm() {
    two_releases
    start_process_as "$root/1.0/qemu/build/qemu-copy"
    no_terminal "$SHELL_UNDER_TEST" "$root/current/install.sh" --update --from-app=1
    stop_process
    rm -f "$root/1.0/qemu/build/qemu-copy"
    check_exit 1
    check_said "Guesthouse is running"
    check_old_version_works
}

t_the_app_has_root_asked_for_in_a_window() {
    release 1.0
    HELPER_VERSION=2 release 1.1
    give_everything
    install_version 1.0
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.1)" --from-app=1
    check_exit 0
    check_log "pkexec sh -c { f="
    check_not_log "sudo"
    check_same "$(helper_path)" "$H/.local/lib/guesthouse/current/qemu/build/qemu-bridge-helper" "it's still the old one"
}

t_with_no_pkexec_the_app_is_told_to_use_a_terminal() {
    release 1.0
    HELPER_VERSION=2 release 1.1
    give_everything
    install_version 1.0
    rm "$T/bin/pkexec"
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.1)" --from-app=1
    check_exit 1
    check_said "Run install.sh --update in a terminal"
    [ "$(current_says)" = "guesthouse 1.0" ] || fail "version 1.0 doesn't run"
}

t_a_refused_window_changes_nothing() {
    release 1.0
    HELPER_VERSION=2 release 1.1
    give_everything
    install_version 1.0
    SUDO_FAILS=yes no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.1)" --from-app=1
    check_exit 1
    check_said "Nothing of Guesthouse's was changed"
    [ "$(current_says)" = "guesthouse 1.0" ] || fail "version 1.0 doesn't run"
}

t_from_app_takes_only_a_process_number() {
    release 1.0
    give_everything
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)" --from-app=one
    check_exit 1
    check_said "process number"
    check_gone "$H/.local/lib/guesthouse"
}
