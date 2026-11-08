//! QEMU's jail on this PC's kernel, with no VM. A GPU machine's plan is made as a start would make
//! it, and its words are run with this program standing in for QEMU. In there it tries what a guest
//! that had taken QEMU over would, and says how that went with its exit code. It has no sound, so the
//! launcher opens QEMU's unix sockets and QEMU may make none. Its disk is a qcow2 on a base outside
//! its folder, as a disk made by hand can be.

use guesthouse_app_core::linux::forwarding::sandbox_runs;
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::slots;
use guesthouse_app_core::testing::{Scratch, scratch_context};
use guesthouse_app_core::vm_args::{Boot, QemuJail, VmPlan};
use guesthouse_jail::Support;
use std::env;
use std::fs::{self, OpenOptions};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const PROBE_VARIABLE: &str = "JAIL_PROBE";

fn variable(name: &str) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap()
}

fn status_says(wanted: [&str; 2]) -> bool {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    status.lines().any(|line| line.split_whitespace().eq(wanted))
}

/// What runs as QEMU. Everything it may not reach has to fail, however it fails: in namespaces a
/// path that was never bound isn't there at all.
fn probe() {
    let (own, other) = (variable("PROBE_OWN"), variable("PROBE_OTHER"));
    let checks = [
        ("its own disk", fs::read(own.join("disk.qcow2")).is_ok()),
        ("its disk's base", fs::read(variable("PROBE_BASE")).is_ok()),
        ("not to write the base", OpenOptions::new().write(true).open(variable("PROBE_BASE")).is_err()),
        ("not the base's folder", fs::read_dir(variable("PROBE_BASE").parent().unwrap()).is_err()),
        ("its run folder", fs::write(variable("PROBE_RUN").join("note"), "here").is_ok()),
        ("not its own token", fs::read(own.join("guest.token")).is_err()),
        ("not the other machine's disk", fs::read(other.join("disk.qcow2")).is_err()),
        ("not the other machine's folder", fs::read_dir(&other).is_err()),
        ("not the home folder", fs::read_dir(variable("PROBE_HOME")).is_err()),
        ("its agent port", TcpListener::bind(("127.0.0.1", port("PROBE_AGENT_PORT"))).is_ok()),
        ("not another port", TcpListener::bind(("127.0.0.1", port("PROBE_OTHER_PORT"))).is_err()),
        ("no new unix socket", unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) } == -1),
        ("its own unix sockets", (20..26).all(|fd| fs::read_link(format!("/proc/self/fd/{fd}")).is_ok())),
        ("no ptrace", unsafe { libc::ptrace(libc::PTRACE_ATTACH, libc::getppid(), 0, 0) } == -1),
        ("no new privileges", status_says(["NoNewPrivs:", "1"])),
        ("a filter", status_says(["Seccomp:", "2"])),
    ];
    let failed: Vec<&str> = checks.iter().filter(|(_, held)| !held).map(|(name, _)| *name).collect();
    if !failed.is_empty() {
        eprintln!("not as it should be: {failed:?}");
        process::exit(1);
    }
}

fn port(name: &str) -> u16 {
    env::var(name).unwrap().parse().unwrap()
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn qemu_img(words: &[&str]) {
    assert!(Command::new("qemu-img").args(words).status().unwrap().success(), "qemu-img {words:?}");
}

fn machine(scratch: &Scratch, id: &str) -> Machine {
    let disk = scratch.write(&format!("machines/{id}/disk.qcow2"), "a disk");
    let vars = scratch.write(&format!("machines/{id}/vars.fd"), "");
    scratch.write(&format!("machines/{id}/guest.token"), "a secret");
    Machine::new(id, id, &disk.to_string_lossy(), &vars.to_string_lossy())
}

/// Runs the plan's words with this program as QEMU, in namespaces or not.
fn run_jailed(namespaces: bool) -> bool {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let (own, other) = (machine(&scratch, "own"), machine(&scratch, "other"));
    let base = scratch.path("bases/base.qcow2");
    fs::create_dir_all(base.parent().unwrap()).unwrap();
    qemu_img(&["create", "-q", "-f", "qcow2", base.to_str().unwrap(), "1G"]);
    // Named as qemu-img names it from the overlay's own folder, so QEMU goes through `..` to it.
    qemu_img(&["create", "-q", "-f", "qcow2", "-b", "../../bases/base.qcow2", "-F", "qcow2", &own.disk]);
    // Laid out as a QEMU in its build folder is, so its tree holds nothing of the machines'.
    let qemu = scratch.path("qemu/build/qemu-system-x86_64");
    fs::create_dir_all(qemu.parent().unwrap()).unwrap();
    fs::copy(env::current_exe().unwrap(), &qemu).unwrap();
    let plan = VmPlan::new(&context, &own, Boot::Gpu, slots::number(3), 0);
    fs::create_dir_all(&plan.run_folder).unwrap();
    // The daemon's sockets, which the launcher connects to for a QEMU with no sound.
    let _daemon =
        ["global.sock", "vgpu.sock"].map(|name| UnixListener::bind(plan.run_folder.join(name)).unwrap());
    let jail = QemuJail {
        launcher: env!("CARGO_BIN_EXE_guesthouse-jail").into(),
        namespaces,
        qemu_sandbox: false,
        sound_socket: None,
        backing_files: Vec::new(),
    };
    let jail = plan.with_backing_files(jail).unwrap();
    let granted_base = jail.backing_files[0].clone();
    // A real VM may hold the slot's own port.
    let plan = VmPlan { qemu, qemu_files: None, jail: Some(jail), agent_port: free_port(), ..plan };
    let command = plan.qemu_command();
    let folder_of = |machine: &Machine| Path::new(&machine.disk).parent().unwrap().to_owned();
    let done = Command::new(&command[0])
        .args(&command[1..])
        .env_clear()
        .envs(plan.qemu_environment())
        .env(PROBE_VARIABLE, "1")
        .env("PROBE_OWN", folder_of(&own))
        .env("PROBE_OTHER", folder_of(&other))
        .env("PROBE_BASE", &granted_base)
        .env("PROBE_RUN", &plan.run_folder)
        .env("PROBE_HOME", env::var_os("HOME").unwrap())
        .env("PROBE_AGENT_PORT", plan.agent_port.to_string())
        .env("PROBE_OTHER_PORT", free_port().to_string())
        .status()
        .unwrap();
    done.success()
}

fn main() {
    if env::var_os(PROBE_VARIABLE).is_some() {
        probe();
        return;
    }
    if let Some(refusal) = Support::probe().refusal() {
        println!("skipped: {refusal}");
        return;
    }
    assert!(run_jailed(false), "jailed, without namespaces");
    println!("without namespaces ... ok");
    if sandbox_runs() {
        assert!(run_jailed(true), "jailed, in namespaces");
        println!("in namespaces ... ok");
    } else {
        println!("in namespaces ... skipped, as bubblewrap can't make them here");
    }
}
