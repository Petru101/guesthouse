use super::*;
use crate::machine::Machine;
use crate::network::Forward;
use crate::slots;
use crate::testing::{Scratch, scratch_context};
use crate::vm_args::{Boot, NetworkPlan};

fn jail(namespaces: bool) -> QemuJail {
    QemuJail {
        launcher: "/p/guesthouse-jail".into(),
        namespaces,
        qemu_sandbox: true,
        sound_socket: Some("/run/user/1000/pipewire-0".into()),
        backing_files: Vec::new(),
    }
}

fn jailed(machine: Machine, namespaces: bool) -> (Scratch, VmPlan) {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::number(2), 0);
    (scratch, VmPlan { jail: Some(jail(namespaces)), qemu: "/usr/bin/qemu-system-x86_64".into(), ..plan })
}

fn windows() -> Machine {
    Machine {
        forwards: vec![Forward::new(Protocol::Tcp, 8080, 80), Forward::new(Protocol::Udp, 9000, 9000)],
        ..Machine::new("win", "Win", "/m/win/disk.qcow2", "/m/win/vars.fd")
    }
}

fn granted(plan: &Plan, grant: Grant) -> Vec<String> {
    plan.paths
        .iter()
        .filter(|(given, _)| *given == grant)
        .map(|(_, path)| path.display().to_string())
        .collect()
}

#[test]
fn it_gets_this_vms_files_and_not_its_folder() {
    let (_scratch, plan) = jailed(windows(), true);
    let rules = plan.qemu_jail_plan(&jail(true));
    let written = granted(&rules, Grant::Write);
    assert!(
        written.contains(&"/m/win/disk.qcow2".to_owned()) && written.contains(&"/m/win/vars.fd".to_owned())
    );
    assert!(
        written.iter().any(|path| path.ends_with("gpupv-ram-2"))
            && written.iter().any(|path| path.ends_with("gpupv-iomem-2"))
    );
    assert!(!rules.paths.iter().any(|(_, path)| path == Path::new("/m/win")), "the folder holds its token");
    assert_eq!(granted(&rules, Grant::Own), [plan.run_folder.display().to_string()]);
    assert_eq!(granted(&rules, Grant::Socket), ["/run/user/1000/pipewire-0"]);
    assert!(granted(&rules, Grant::Device).contains(&"/dev/kvm".to_owned()));
    assert!(
        !granted(&rules, Grant::Device).iter().any(|device| device.contains("nvidia")),
        "the card is the daemon's"
    );
}

#[test]
fn nat_holds_binds_to_the_forwards_and_lets_the_guest_out() {
    let (_scratch, plan) = jailed(windows(), true);
    let network = plan.qemu_jail_plan(&jail(true)).network;
    assert_eq!(network.tcp_bind, Ports::Only(vec![plan.agent_port, 8080]));
    assert_eq!((network.tcp_connect, network.udp_send), (Ports::Any, Ports::Any));
}

#[test]
fn a_restricted_network_reaches_nothing_but_names() {
    let (_scratch, plan) = jailed(Machine { network_mode: Mode::None, ..windows() }, true);
    let network = plan.qemu_jail_plan(&jail(true)).network;
    assert_eq!(network.tcp_connect, Ports::none());
    assert_eq!(network.udp_bind, Ports::Only(vec![0, 9000]));
    assert_eq!(network.udp_send, Ports::Only(vec![DNS_PORT]));
}

#[test]
fn bubblewrap_comes_first_and_binds_each_file_where_it_is() {
    let (_scratch, plan) = jailed(windows(), true);
    let command = plan.qemu_command();
    assert_eq!(command[0], "bwrap");
    let bound = |kind: &str, path: &str| command.windows(3).any(|three| three == [kind, path, path]);
    assert!(bound("--bind-try", "/m/win/disk.qcow2"));
    assert!(bound("--ro-bind", "/p/guesthouse-jail"));
    assert!(bound("--dev-bind-try", "/dev/kvm"));
    assert!(!command.iter().any(|word| word == "--new-session"), "the app stops QEMU by its group");
    assert!(!command.iter().any(|word| word == "--unshare-pid"), "the D-Bus display needs the viewer's pid");
    let launcher = command.iter().position(|word| word == "/p/guesthouse-jail").unwrap();
    let qemu = command.iter().position(|word| word == "/usr/bin/qemu-system-x86_64").unwrap();
    assert!(launcher < qemu);
    assert_eq!(command[qemu - 1], "--");
    assert!(plan.qemu_arguments().ends_with(&["-sandbox".to_owned(), QEMU_SANDBOX.to_owned()]));
}

#[test]
fn without_namespaces_the_launcher_comes_first() {
    let (_scratch, plan) = jailed(windows(), false);
    assert_eq!(plan.qemu_command()[0], "/p/guesthouse-jail");
}

#[test]
fn a_bridged_machine_gets_its_device_from_the_launcher_and_no_namespaces() {
    let (_scratch, plan) = jailed(Machine { network_mode: Mode::Bridged, ..windows() }, true);
    let command = plan.qemu_command();
    assert_eq!(command[0], "/p/guesthouse-jail");
    let tap = command.iter().position(|word| word == "--tap").unwrap();
    assert_eq!(command[tap + 2..tap + 4], ["br0".to_owned(), TAP_FD.to_string()]);
    assert!(
        plan.qemu_arguments()
            .iter()
            .any(|word| word.starts_with("tap,model=e1000e") && word.ends_with(&format!("fd={TAP_FD}")))
    );
}

#[test]
fn passt_keeps_its_own_sandbox() {
    let (_scratch, plan) = jailed(windows(), true);
    let plan = VmPlan { network: NetworkPlan { passt: true, ..plan.network.clone() }, ..plan };
    assert!(plan.qemu_command().iter().any(|word| word == "--own-sandboxes"));
    assert!(!plan.qemu_arguments().iter().any(|word| word == "-sandbox"));
    assert_eq!(plan.qemu_jail_plan(&jail(true)).network.tcp_connect, Ports::Any);
}

#[test]
fn a_jailed_qemu_gets_a_clean_environment() {
    let (_scratch, plan) = jailed(Machine { share: "/home/me/Shared".into(), ..windows() }, true);
    assert!(plan.clears_environment());
    let environment = plan.qemu_environment();
    let names: Vec<&str> = environment.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["PATH", "TMPDIR", "XDG_RUNTIME_DIR", "SHARE"]);
    assert_eq!(
        environment[1].1,
        plan.run_folder.display().to_string(),
        "QEMU's own temporary files stay its own"
    );
    let rules = plan.qemu_jail_plan(&jail(true));
    assert!(granted(&rules, Grant::Own).contains(&"/home/me/Shared".to_owned()));
}

#[test]
fn with_no_sound_the_launcher_opens_every_unix_socket_and_qemu_makes_none() {
    let (_scratch, plan) = jailed(Machine { tpm: true, ..windows() }, false);
    let jail = QemuJail { sound_socket: None, ..jail(false) };
    let plan = VmPlan { jail: Some(jail), sound: None, ..plan };
    let command = plan.qemu_command();
    assert!(command.iter().any(|word| word == "--no-new-unix-sockets"));
    assert_eq!(command.iter().filter(|word| *word == "--unix-listen").count(), 4);
    assert_eq!(command.iter().filter(|word| *word == "--unix-connect").count(), 3);
    let arguments = plan.qemu_arguments();
    assert!(
        !arguments.iter().any(|word| word.starts_with("unix:") || word.contains(",path=")),
        "{arguments:?}"
    );
    assert!(arguments.iter().any(|word| word == "socket,id=gpv,fd=25"));
    assert!(arguments.iter().any(|word| word == "socket,id=tpm,fd=26"));
    let with_sound = VmPlan { sound: Some(crate::vm_args::Sound { sink: None }), ..plan };
    assert!(!with_sound.qemu_command().iter().any(|word| word == "--no-new-unix-sockets"));
}

#[test]
fn a_shared_folder_gets_the_loopback_pair_its_program_is_reached_through() {
    let (_scratch, plan) = jailed(Machine { share: "/home/me/Shared".into(), ..windows() }, true);
    let network = plan.qemu_jail_plan(&jail(true)).network;
    assert_eq!(network.tcp_bind, Ports::Only(vec![plan.agent_port, 8080, EPHEMERAL_PORT]));
    let restricted = Machine { share: "/home/me/Shared".into(), network_mode: Mode::None, ..windows() };
    let (_scratch, plan) = jailed(restricted, true);
    assert_eq!(plan.qemu_jail_plan(&jail(true)).network.tcp_connect, Ports::Any);
}

#[test]
fn a_backing_file_is_only_read_and_bound_read_only_last() {
    let (_scratch, plan) = jailed(Machine { share: "/home/me/Shared".into(), ..windows() }, true);
    let jail = QemuJail { backing_files: vec!["/bases/base.qcow2".into()], ..jail(true) };
    let plan = VmPlan { jail: Some(jail.clone()), ..plan };
    let rules = plan.qemu_jail_plan(&jail);
    let given: Vec<Grant> = rules
        .paths
        .iter()
        .filter(|(_, path)| path == Path::new("/bases/base.qcow2"))
        .map(|(grant, _)| *grant)
        .collect();
    assert_eq!(given, [Grant::Read]);
    assert_eq!(rules.paths.last().unwrap().1, Path::new("/bases/base.qcow2"));
    let command = plan.qemu_command();
    let bound = |kind: &str| {
        command.windows(3).any(|three| three == [kind, "/bases/base.qcow2", "/bases/base.qcow2"])
    };
    assert!(bound("--ro-bind-try") && !bound("--bind-try"));
}

/// A machine whose disk is a qcow2 on a base in `base_folder`, under the scratch folder.
fn on_a_base(scratch: &Scratch, base_folder: &str) -> (Machine, PathBuf) {
    let base = scratch.path(&format!("{base_folder}/base.qcow2"));
    let disk = scratch.path("machines/win/disk.qcow2");
    for file in [&base, &disk] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    }
    let create = |words: &[&str]| {
        assert!(std::process::Command::new("qemu-img").args(words).status().unwrap().success());
    };
    create(&["create", "-q", "-f", "qcow2", base.to_str().unwrap(), "1G"]);
    create(&[
        "create",
        "-q",
        "-f",
        "qcow2",
        "-b",
        base.to_str().unwrap(),
        "-F",
        "qcow2",
        disk.to_str().unwrap(),
    ]);
    let machine = Machine {
        share: scratch.path("Shared").display().to_string(),
        ..Machine::new("win", "Win", disk.to_str().unwrap(), "/m/win/vars.fd")
    };
    (machine, base)
}

#[test]
fn a_start_finds_the_disks_backing_files() {
    let scratch = Scratch::new();
    let (machine, base) = on_a_base(&scratch, "bases");
    let (_plan_scratch, plan) = jailed(machine, true);
    assert_eq!(plan.with_backing_files(jail(true)).unwrap().backing_files, [base]);
}

#[test]
fn a_backing_file_in_the_shared_folder_is_refused() {
    let scratch = Scratch::new();
    let (machine, _) = on_a_base(&scratch, "Shared");
    let (_plan_scratch, plan) = jailed(machine, true);
    let refused = plan.with_backing_files(jail(true)).unwrap_err();
    assert!(refused.0.contains("in the shared folder"), "{}", refused.0);
}
