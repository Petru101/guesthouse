# What it asks of root, on each distro.

expect_command() {
    local id=$1 like=$2 wanted=$3
    release 1.0
    fake_os "$id" "$like"
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)" --dry-run
    check_exit 0
    check_said "$wanted"
}

t_fedora_packages() {
    expect_command fedora "" "sudo sh -c 'dnf install -y gtk4 libadwaita pipewire-libs qemu-img edk2-ovmf'"
}

t_debian_packages() {
    expect_command debian "" "sudo sh -c 'apt-get update && apt-get install -y --no-remove libgtk-4-1 libadwaita-1-0 libpipewire-0.3-0t64 qemu-utils ovmf'"
}

t_ubuntu_packages() {
    expect_command ubuntu debian "apt-get install -y --no-remove libgtk-4-1"
}

t_arch_packages() {
    expect_command arch "" "sudo sh -c 'pacman -S --needed --noconfirm gtk4 libadwaita libpipewire qemu-img edk2-ovmf'"
}

t_a_derivative_uses_its_parents_names() {
    expect_command nobara "rhel fedora" "dnf install -y gtk4"
}

t_opensuse_packages() {
    expect_command opensuse-tumbleweed "opensuse suse" "sudo sh -c 'zypper --non-interactive install libgtk-4-1"
}

t_gpu_machines_bring_their_tools() {
    release 1.0 "windows linux"
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)" --dry-run
    check_said "edk2-ovmf 7zip genisoimage swtpm swtpm-tools openssl openssh'"
}

t_an_unknown_distro_gets_a_list_and_no_commands() {
    release 1.0
    fake_os plan9
    no_terminal "$SHELL_UNDER_TEST" "$(script_for 1.0)" --dry-run
    check_exit 0
    check_said "I don't know your distro's names"
    check_not_said "sudo sh"
}
