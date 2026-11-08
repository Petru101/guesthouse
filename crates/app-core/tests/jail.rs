//! QEMU's jail on this PC's kernel, with no VM. A GPU machine's plan is made as a start would make
//! it, and its words are run with this program standing in for QEMU. In there it tries what a guest
//! that had taken QEMU over would, and says how that went with its exit code. It has no sound, so the
//! launcher opens QEMU's unix sockets and QEMU may make none. Its disk is a qcow2 on a base outside
//! its folder, as a disk made by hand can be. On passt, the launcher starts the real passt and the
//! stand-in asks it for the gateway's address through the socket it was handed. Bridged, this program
//! also stands in for the bridge helper, and hands back an open file for the device. With the
//! forwarded GPU, it's an Ubuntu guest's QEMU, which makes its own frames socket.

use guesthouse_app_core::linux::forwarding::sandbox_runs;
use guesthouse_app_core::machine::{Kind, Machine};
use guesthouse_app_core::network::Mode;
use guesthouse_app_core::slots;
use guesthouse_app_core::testing::{Scratch, scratch_context};
use guesthouse_app_core::vm_args::{
    Boot, NetworkPlan, PASST_FD, PASST_PID_FILE, QemuJail, TAP_FD, UbuntuBoot, VmPlan, passt_installed,
};
use guesthouse_jail::Support;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::fd::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant};

const PROBE_VARIABLE: &str = "JAIL_PROBE";
const HELPER_VARIABLE: &str = "JAIL_PROBE_HELPER";
/// What the pretend bridge helper hands back for the device.
const PRETEND_DEVICE: &str = "/dev/null";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Nat,
    Passt,
    Bridged,
    ForwardedGpu,
}

/// The kernel's flag for the old address layout, which `setarch -L` sets.
const ADDR_COMPAT_LAYOUT: u32 = 0x0200000;

fn personality() -> u32 {
    let text = fs::read_to_string("/proc/self/personality").unwrap_or_default();
    u32::from_str_radix(text.trim(), 16).unwrap_or(0)
}

/// Stands in for QEMU's bridge helper: sends an open file back through the descriptor it's told of.
fn pretend_bridge_helper() {
    let socket: i32 = env::args().find_map(|word| word.strip_prefix("--fd=")?.parse().ok()).unwrap();
    let device = File::open(PRETEND_DEVICE).unwrap();
    let fd = std::os::fd::AsRawFd::as_raw_fd(&device);
    let space = unsafe { libc::CMSG_SPACE(size_of::<i32>() as u32) } as usize;
    let mut control = vec![0u8; space];
    let mut byte = [0u8; 1];
    let mut part = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    // SAFETY: a zeroed msghdr is an empty one, and every pointer set here lives across the call.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space;
    // SAFETY: the control buffer has room for one descriptor's header, filled in before the send.
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<i32>() as u32) as usize;
        libc::CMSG_DATA(header).cast::<i32>().write_unaligned(fd);
        assert_eq!(libc::sendmsg(socket, &raw const message, 0), 1);
    }
}

fn variable(name: &str) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap()
}

fn status_says(wanted: [&str; 2]) -> bool {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    status.lines().any(|line| line.split_whitespace().eq(wanted))
}

/// The guest's and the gateway's addresses on passt, as the machine's passt settings give them.
const GUEST_ADDRESS: [u8; 4] = [10, 0, 3, 15];
const GATEWAY: [u8; 4] = [10, 0, 3, 2];
const GUEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const ARP: [u8; 2] = [0x08, 0x06];
const ARP_REPLY: u8 = 2;
const PASST_ANSWER: Duration = Duration::from_secs(5);

/// One frame on passt's socket, which carries each with its length first.
fn send_frame(socket: &mut File, frame: &[u8]) -> bool {
    let length = (frame.len() as u32).to_be_bytes();
    socket.write_all(&length).is_ok() && socket.write_all(frame).is_ok()
}

fn read_frame(socket: &mut File) -> Option<Vec<u8>> {
    let mut length = [0u8; 4];
    socket.read_exact(&mut length).ok()?;
    let mut frame = vec![0u8; u32::from_be_bytes(length) as usize];
    socket.read_exact(&mut frame).ok()?;
    Some(frame)
}

/// Asks who has the gateway's address, as a guest's first packet would, and waits for passt's answer.
fn passt_answers() -> bool {
    // SAFETY: the launcher left this socket open for QEMU, and nothing else here holds it.
    let mut socket = unsafe { File::from_raw_fd(PASST_FD) };
    let mut request = vec![0xff; 6];
    request.extend(GUEST_MAC);
    request.extend(ARP);
    request.extend([0, 1, 0x08, 0, 6, 4, 0, 1]);
    request.extend(GUEST_MAC);
    request.extend(GUEST_ADDRESS);
    request.extend([0; 6]);
    request.extend(GATEWAY);
    if !send_frame(&mut socket, &request) {
        return false;
    }
    let (sender, answer) = std::sync::mpsc::channel();
    thread::spawn(move || {
        while let Some(frame) = read_frame(&mut socket) {
            let is_reply = frame.len() >= 42 && frame[12..14] == ARP && frame[21] == ARP_REPLY;
            if is_reply && frame[28..32] == GATEWAY {
                let _ = sender.send(true);
                return;
            }
        }
    });
    answer.recv_timeout(PASST_ANSWER).unwrap_or(false)
}

/// What runs as QEMU. Everything it may not reach has to fail, however it fails: in namespaces a
/// path that was never bound isn't there at all.
fn probe() {
    let (own, other) = (variable("PROBE_OWN"), variable("PROBE_OTHER"));
    let forwarded = env::var_os("PROBE_FORWARDED").is_some();
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
        (
            "no new unix socket",
            forwarded || unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) } == -1,
        ),
        (
            "its own unix sockets",
            forwarded || (20..26).all(|fd| fs::read_link(format!("/proc/self/fd/{fd}")).is_ok()),
        ),
        ("not another device", OpenOptions::new().read(true).write(true).open("/dev/fuse").is_err()),
        ("no ptrace", unsafe { libc::ptrace(libc::PTRACE_ATTACH, libc::getppid(), 0, 0) } == -1),
        ("no new privileges", status_says(["NoNewPrivs:", "1"])),
        ("a filter", status_says(["Seccomp:", "2"])),
    ];
    let mut checks = checks.to_vec();
    if env::var_os("PROBE_PASST").is_some() {
        checks.push(("passt through its socket", passt_answers()));
    }
    if forwarded {
        let frames = UnixListener::bind(variable("PROBE_RUN").join("show.sock"));
        checks.push(("its frames socket", frames.is_ok()));
        checks.push(("not the session bus", UnixStream::connect(variable("PROBE_BUS")).is_err()));
        checks.push(("the driver image", fs::read(variable("PROBE_IMAGE")).is_ok()));
        checks.push(("the old address layout", personality() & ADDR_COMPAT_LAYOUT != 0));
    }
    if env::var_os("PROBE_BRIDGED").is_some() {
        let device = fs::read_link(format!("/proc/self/fd/{TAP_FD}"));
        checks.push(("the bridge's device", device.is_ok_and(|path| path == Path::new(PRETEND_DEVICE))));
    }
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

fn pid_in(file: &Path) -> Option<u32> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// passt quits once its one client, QEMU, has gone.
fn ends_with_qemu(pid: u32) -> bool {
    let end = Instant::now() + PASST_ANSWER;
    while Path::new(&format!("/proc/{pid}")).exists() {
        if Instant::now() > end {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Runs the plan's words with this program as QEMU, in namespaces or not.
fn run_jailed(namespaces: bool, on: Case) -> bool {
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
    let forwarded = on == Case::ForwardedGpu;
    let (own, boot) = if forwarded {
        let gpu = UbuntuBoot::WithGpu {
            image: scratch.write("store/linux-driver/driver.iso", "the driver"),
            allow_list: scratch.write("program/linux/allow.txt", ""),
            sensor_daemon: scratch.write("qemu/build/nvfwd-sensord", ""),
        };
        (Machine { kind: Kind::Linux, vars: String::new(), ..own }, Boot::Ubuntu(gpu))
    } else {
        (own, Boot::Gpu)
    };
    let plan = VmPlan::new(&context, &own, boot, slots::number(3), 0);
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
    let (passt, bridged) = (on == Case::Passt, on == Case::Bridged);
    let mode = if bridged { Mode::Bridged } else { Mode::Nat };
    let network = NetworkPlan { passt, mode, bridge: "br7".into(), ..plan.network.clone() };
    let plan = VmPlan { qemu, qemu_files: None, jail: Some(jail), agent_port: free_port(), network, ..plan };
    let mut command = plan.qemu_command();
    if let Some(at) = command.iter().position(|word| word == "--tap") {
        command[at + 1] = env::current_exe().unwrap().display().to_string();
    }
    let folder_of = |machine: &Machine| Path::new(&machine.disk).parent().unwrap().to_owned();
    let mut run = Command::new(&command[0]);
    run.args(&command[1..]).env_clear().envs(plan.qemu_environment());
    if passt {
        run.env("PROBE_PASST", "1");
    }
    if bridged {
        run.env("PROBE_BRIDGED", "1").env(HELPER_VARIABLE, "1");
    }
    if forwarded {
        let bus = Path::new(&env::var_os("XDG_RUNTIME_DIR").unwrap_or_default()).join("bus");
        run.env("PROBE_FORWARDED", "1")
            .env("PROBE_BUS", bus)
            .env("PROBE_IMAGE", scratch.path("store/linux-driver/driver.iso"));
    }
    let done = run
        .env(PROBE_VARIABLE, "1")
        .env("PROBE_OWN", folder_of(&own))
        .env("PROBE_OTHER", folder_of(&other))
        .env("PROBE_BASE", &granted_base)
        .env("PROBE_RUN", &plan.run_folder)
        .env("PROBE_HOME", env::var_os("HOME").unwrap())
        .env(
            "PROBE_AGENT_PORT",
            if forwarded { env::var("SSH_PORT").unwrap() } else { plan.agent_port.to_string() },
        )
        .env("PROBE_OTHER_PORT", free_port().to_string())
        .status()
        .unwrap();
    if passt {
        let Some(pid) = pid_in(&plan.run_folder.join(PASST_PID_FILE)) else {
            eprintln!("passt left no pid file");
            return false;
        };
        if !ends_with_qemu(pid) {
            eprintln!("passt {pid} outlived QEMU");
            return false;
        }
    }
    done.success()
}

fn main() {
    if env::var_os(HELPER_VARIABLE).is_some() && env::args().any(|word| word.starts_with("--fd=")) {
        pretend_bridge_helper();
        return;
    }
    if env::var_os(PROBE_VARIABLE).is_some() {
        probe();
        return;
    }
    if let Some(refusal) = Support::probe().refusal() {
        println!("skipped: {refusal}");
        return;
    }
    // SAFETY: no thread exists yet. Ubuntu's SSH port, which a real VM may hold.
    unsafe { env::set_var("SSH_PORT", free_port().to_string()) };
    let mut cases = vec![Case::Nat, Case::Bridged, Case::ForwardedGpu];
    if passt_installed() {
        cases.push(Case::Passt);
    } else {
        println!("on passt ... skipped, as passt isn't installed");
    }
    for on in cases {
        assert!(run_jailed(false, on), "jailed, without namespaces, {on:?}");
        println!("without namespaces, {on:?} ... ok");
        if sandbox_runs() {
            assert!(run_jailed(true, on), "jailed, in namespaces, {on:?}");
            println!("in namespaces, {on:?} ... ok");
        } else {
            println!("in namespaces, {on:?} ... skipped, as bubblewrap can't make them here");
        }
    }
}
