# What goes, and above all what stays.

install_plain() {
    release 1.0 windows
    give_everything
    snapshot >"$T/empty"
    install_version 1.0
    [ "$status" -eq 0 ] || fail "the install failed"
}

t_it_ends_up_as_it_began() {
    install_plain
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_home_is "$T/empty"
}

t_folders_that_were_there_stay() {
    release 1.0
    give_everything
    mkdir -p "$H/.local/share/applications" "$H/.local/lib"
    echo mine >"$H/.local/share/applications/other.desktop"
    snapshot >"$T/before"
    install_version 1.0
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_home_is "$T/before"
}

t_a_changed_file_stays_and_is_named() {
    install_plain
    launcher=$H/.local/share/applications/net.gpupv.Guesthouse.desktop
    echo "my edit" >>"$launcher"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_there "$launcher"
    check_said "left alone"
}

t_a_file_of_yours_in_its_folder_stays() {
    install_plain
    echo notes >"$H/.local/lib/guesthouse/1.0/my-notes.txt"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_there "$H/.local/lib/guesthouse/1.0/my-notes.txt"
    check_gone "$H/.local/lib/guesthouse/1.0/guesthouse"
}

t_machines_settings_and_logs_stay() {
    install_plain
    mkdir -p "$H/.local/share/guesthouse/machines/win" "$H/.config/guesthouse" "$H/.local/state/guesthouse"
    echo disk >"$H/.local/share/guesthouse/machines/win/disk.qcow2"
    echo '{}' >"$H/.config/guesthouse/app.json"
    echo log >"$H/.local/state/guesthouse/guesthouse.log"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_said "win:"
    check_there "$H/.local/share/guesthouse/machines/win/disk.qcow2"
    check_there "$H/.config/guesthouse/app.json"
    check_there "$H/.local/state/guesthouse/guesthouse.log"
    check_gone "$H/.local/lib/guesthouse"
}

t_a_dry_run_removes_nothing() {
    install_plain
    snapshot >"$T/before"
    no_terminal "$SHELL_UNDER_TEST" "$(uninstaller)" --dry-run
    check_exit 0
    check_home_is "$T/before"
}

t_nothing_installed_is_nothing_to_do() {
    snapshot >"$T/before"
    no_terminal "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_said "Nothing to do"
    check_home_is "$T/before"
}

# A copy of bash under another name, so /proc shows the right program, and it waits on a builtin.
start_process_as() {
    local program=$1
    shift
    WITHOUT_NAMESPACE=yes
    mkdir -p "$(dirname "$program")"
    rm -f "$program"
    cp "$(readlink -f "$T/bin/bash")" "$program"
    mkfifo "$T/hold"
    HOME=${VM_HOME:-$H} "$program" -c 'exec 3<>"$0"; read -r -t 60 line <&3' "$T/hold" "$@" &
    process=$!
}

stop_process() {
    kill "$process" 2>/dev/null
    wait "$process" 2>/dev/null
}

t_a_vm_of_this_install_stops_the_uninstall() {
    install_plain
    start_process_as "$H/.local/lib/guesthouse/1.0/qemu/build/qemu-copy"
    snapshot >"$T/before"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    stop_process
    rm -f "$H/.local/lib/guesthouse/1.0/qemu/build/qemu-copy"
    check_exit 1
    check_said "Guesthouse is running"
    check_there "$H/.local/lib/guesthouse/current"
}

t_a_vm_on_the_systems_qemu_stops_the_uninstall() {
    install_plain
    start_process_as "$T/elsewhere/qemu-system-x86_64" "unix:$T/runtime/guesthouse/run/qmp.sock,server"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    stop_process
    check_exit 1
    check_said "Guesthouse is running"
    check_there "$H/.local/lib/guesthouse/current"
}

t_a_vm_in_another_home_does_not_stop_it() {
    install_plain
    VM_HOME=$T/another-home start_process_as "$T/elsewhere/qemu-system-x86_64" "unix:$T/runtime/guesthouse/run/qmp.sock,server"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    stop_process
    check_exit 0
    check_gone "$H/.local/lib/guesthouse/current"
}

t_a_qemu_of_someone_elses_does_not_stop_it() {
    install_plain
    start_process_as "$T/elsewhere/qemu-system-x86_64" "unix:/somewhere/else/qmp.sock,server"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    stop_process
    check_exit 0
}

make_machine() {
    mkdir -p "$1/machines/win"
    echo disk >"$1/machines/win/disk.qcow2"
}

t_deleting_machines_needs_the_folders_name_typed() {
    install_plain
    make_machine "$H/.local/share/guesthouse"
    snapshot >"$T/before"
    answering 'yes\nyes\n' "$SHELL_UNDER_TEST" "$(uninstaller)" --delete-machines
    check_exit 1
    check_said "nothing was removed"
    check_home_is "$T/before"
}

t_deleting_machines_goes_with_the_right_name() {
    install_plain
    make_machine "$H/.local/share/guesthouse"
    mkdir -p "$H/.local/share/guesthouse/setup"
    answering "yes\n$H/.local/share/guesthouse/machines\n" "$SHELL_UNDER_TEST" "$(uninstaller)" --delete-machines
    check_exit 0
    check_gone "$H/.local/share/guesthouse/machines"
    check_there "$H/.local/share/guesthouse/setup"
}

t_a_store_moved_by_the_pointer_is_the_one_shown() {
    install_plain
    mkdir -p "$H/.config/guesthouse"
    echo "~/big disk" >"$H/.config/guesthouse/store"
    make_machine "$H/big disk"
    answering "yes\n$H/big disk/machines\n" "$SHELL_UNDER_TEST" "$(uninstaller)" --delete-machines
    check_exit 0
    check_gone "$H/big disk/machines"
    check_there "$H/.config/guesthouse/store"
}

t_a_linked_machines_folder_is_not_followed() {
    install_plain
    make_machine "$H/real"
    mkdir -p "$H/.local/share/guesthouse"
    ln -s "$H/real/machines" "$H/.local/share/guesthouse/machines"
    snapshot >"$T/before"
    answering "yes\n$H/.local/share/guesthouse/machines\n" "$SHELL_UNDER_TEST" "$(uninstaller)" --delete-machines
    check_exit 1
    check_said "I won't delete"
    check_home_is "$T/before"
}

t_a_store_pointing_at_home_is_refused() {
    install_plain
    mkdir -p "$H/.config/guesthouse"
    echo "$H" >"$H/.config/guesthouse/store"
    snapshot >"$T/before"
    sandbox env GUESTHOUSE_MACHINES="$H" "$(command -v python3)" -I "$HERE/tty.py" "yes\n$H\n" "$SHELL_UNDER_TEST" "$(uninstaller)" --delete-machines >"$T/said" 2>&1
    status=$?
    check_exit 1
    check_said "I won't delete"
    check_home_is "$T/before"
}

t_paths_outside_home_in_the_manifest_are_ignored() {
    install_plain
    outside=$T/outside.txt
    echo keep >"$outside"
    hash=$(sha256sum "$outside" | cut -d ' ' -f 1)
    printf 'file\t%s\t%s\n' "$hash" "$outside" >>"$H/.local/lib/guesthouse/manifest"
    printf 'file\t%s\t%s\n' "$hash" "$H/../outside.txt" >>"$H/.local/lib/guesthouse/manifest"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_there "$outside"
}

t_a_manifest_with_no_data_home_still_stays_inside_home() {
    install_plain
    outside=$T/outside.txt
    echo keep >"$outside"
    hash=$(sha256sum "$outside" | cut -d ' ' -f 1)
    manifest=$H/.local/lib/guesthouse/manifest
    sed -i '/^data-home/d' "$manifest"
    printf 'file\t%s\t%s\n' "$hash" "$outside" >>"$manifest"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_there "$outside"
}

t_a_manifest_of_another_format_is_refused() {
    install_plain
    sed -i '1s/.*/format\t99\t-/' "$H/.local/lib/guesthouse/manifest"
    snapshot >"$T/before"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 1
    check_home_is "$T/before"
}

t_the_installed_copy_removes_itself_too() {
    install_plain
    answering 'yes\n' "$SHELL_UNDER_TEST" "$H/.local/lib/guesthouse/current/uninstall.sh"
    check_exit 0
    check_home_is "$T/empty"
}

t_an_old_manifest_line_for_a_file_is_harmless() {
    install_plain
    manifest=$H/.local/lib/guesthouse/manifest
    printf 'file\t%s\t%s\n' "0000" "$H/.local/share/applications/net.gpupv.Guesthouse.desktop" >>"$manifest"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_home_is "$T/empty"
}

t_packages_are_listed_and_left() {
    release 1.0
    snapshot >"$T/empty"
    install_version 1.0
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_said "Packages stay"
    check_said "gtk4"
    check_not_log "dnf remove"
    check_home_is "$T/empty"
}

t_the_kvm_group_comes_back_out_as_shown() {
    install_plain
    printf 'group\t-\tkvm\n' >>"$H/.local/lib/guesthouse/manifest"
    answering 'yes\n' "$SHELL_UNDER_TEST" "$(uninstaller)"
    check_exit 0
    check_said "sudo gpasswd -d"
    check_log "gpasswd -d"
}
