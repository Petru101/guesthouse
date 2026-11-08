//! The jail on this PC's kernel. Each case runs in a child of its own, as a jail can't be undone,
//! and the child's exit says whether everything it tried went as it should.

use guesthouse_jail::launch::{Passt, Tap, open_tap, start_passt};
use guesthouse_jail::seccomp::{self, Threads, trap};
use guesthouse_jail::{Grant, Plan, Ports, Support, filters, landlock};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const CASE_VARIABLE: &str = "JAIL_TEST_CASE";
const FOLDER_VARIABLE: &str = "JAIL_TEST_FOLDER";

fn folder() -> PathBuf {
    env::var_os(FOLDER_VARIABLE).unwrap().into()
}

fn refused(result: std::io::Result<impl Sized>) -> bool {
    matches!(result, Err(error) if error.kind() == ErrorKind::PermissionDenied)
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn files_and_ports() {
    let folder = folder();
    let allowed_port = free_port();
    let mut plan = Plan::default();
    plan.grant(Grant::Read, folder.join("mine"));
    plan.network.tcp_bind = Ports::Only(vec![allowed_port]);
    landlock::apply(&plan).unwrap();
    assert_eq!(fs::read_to_string(folder.join("mine/file")).unwrap(), "mine");
    assert!(refused(fs::read(folder.join("theirs/file"))), "another folder");
    assert!(refused(fs::write(folder.join("mine/new"), "")), "a read grant can't make files");
    assert!(TcpListener::bind(("127.0.0.1", allowed_port)).is_ok());
    assert!(refused(TcpListener::bind(("127.0.0.1", free_port_outside()))), "a port not granted");
}

/// Any port but the granted one. Binding to port 0 picks one, which a jail refuses too.
fn free_port_outside() -> u16 {
    40_000 + (process::id() % 1000) as u16
}

fn devices() {
    let mut plan = Plan::default();
    plan.grant(Grant::Device, "/dev/null");
    landlock::apply(&plan).unwrap();
    assert!(fs::write("/dev/null", "gone").is_ok());
    assert!(refused(fs::read("/dev/zero")));
}

fn unix_sockets() {
    let folder = folder();
    let (granted, other) = (folder.join("granted.sock"), folder.join("other.sock"));
    let _listeners = [UnixListener::bind(&granted).unwrap(), UnixListener::bind(&other).unwrap()];
    let mut plan = Plan::default();
    plan.grant(Grant::Socket, &granted);
    landlock::apply(&plan).unwrap();
    assert!(UnixStream::connect(&granted).is_ok());
    if landlock::abi() >= 9 {
        assert!(refused(UnixStream::connect(&other)));
    }
}

fn daemon_calls() {
    trap::report_refusals().unwrap();
    seccomp::install(&filters::daemon(process::id() as i32), Threads::All).unwrap();
    assert!(refused(TcpListener::bind(("127.0.0.1", 0))), "no new sockets");
    let fork = unsafe { libc::fork() };
    assert_eq!(fork, -1, "no new process");
    let traced = unsafe { libc::ptrace(libc::PTRACE_ATTACH, libc::getppid(), 0, 0) };
    assert_eq!((traced, std::io::Error::last_os_error().raw_os_error()), (-1, Some(libc::ENOSYS)));
    let thread = std::thread::spawn(|| 7).join();
    assert_eq!(thread.ok(), Some(7), "threads still start");
    let status = fs::read_to_string("/proc/self/status").unwrap();
    assert!(status.lines().any(|line| line.split_whitespace().eq(["NoNewPrivs:", "1"])));
    assert!(status.lines().any(|line| line.split_whitespace().eq(["Seccomp:", "2"])));
}

fn qemu_calls() {
    seccomp::install(&filters::qemu(false), Threads::This).unwrap();
    let traced = unsafe { libc::ptrace(libc::PTRACE_ATTACH, libc::getppid(), 0, 0) };
    assert_eq!((traced, std::io::Error::last_os_error().raw_os_error()), (-1, Some(libc::EPERM)));
    assert_eq!(unsafe { libc::unshare(libc::CLONE_NEWUSER) }, -1);
    let byte = 0u8;
    assert_eq!(unsafe { libc::ioctl(0, libc::TIOCSTI, &raw const byte) }, -1);
    // QEMU still runs programs and threads.
    assert!(UnixStream::pair().is_ok());
    assert!(Command::new("/usr/bin/true").status().unwrap().success());
}

const HELPER_VARIABLE: &str = "JAIL_TEST_HELPER";
const TAP_AT: i32 = 7;

/// Stands in for QEMU's bridge helper: sends an open file back through the descriptor it's told of.
fn pretend_bridge_helper() {
    let arguments: Vec<String> = env::args().collect();
    assert_eq!(arguments[1], "--use-vnet");
    assert_eq!(arguments[3], "--br=br7");
    let socket: i32 = arguments[2].strip_prefix("--fd=").unwrap().parse().unwrap();
    let device = fs::File::open("/dev/null").unwrap();
    let fd = std::os::fd::AsRawFd::as_raw_fd(&device);
    let space = unsafe { libc::CMSG_SPACE(size_of::<i32>() as u32) } as usize;
    let mut control = vec![0u8; space];
    let mut byte = [0u8; 1];
    let mut part = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: 1 };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<i32>() as u32) as usize;
        libc::CMSG_DATA(header).cast::<i32>().write_unaligned(fd);
        assert_eq!(libc::sendmsg(socket, &raw const message, 0), 1);
    }
}

fn bridged_device() {
    let tap = Tap { helper: env::current_exe().unwrap(), bridge: "br7".to_owned(), fd: TAP_AT };
    // SAFETY: the variable is set before any thread exists.
    unsafe { env::set_var(HELPER_VARIABLE, "1") };
    open_tap(&tap).unwrap();
    let target = fs::read_link(format!("/proc/self/fd/{TAP_AT}")).unwrap();
    assert_eq!(target, Path::new("/dev/null"));
    let flags = unsafe { libc::fcntl(TAP_AT, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0, "it stays open for QEMU");
}

const PASST_VARIABLE: &str = "JAIL_TEST_PASST";
const PASST_AT: i32 = 11;

/// Stands in for passt: checks its words, then leaves a copy of itself in the background that
/// answers one message on the socket it was given, as passt does once it's ready.
fn pretend_passt() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    assert_eq!(arguments[..2], ["--quiet", "--one"]);
    assert_eq!(arguments[2], "--fd");
    let socket: i32 = arguments[3].parse().unwrap();
    // SAFETY: one thread, and the child only reads, writes and exits.
    if unsafe { libc::fork() } == 0 {
        let mut message = [0u8; 4];
        let read = unsafe { libc::read(socket, message.as_mut_ptr().cast(), message.len()) };
        unsafe { libc::write(socket, message.as_ptr().cast(), read as usize) };
        unsafe { libc::_exit(0) };
    }
}

fn passt_connection() {
    let passt = Passt {
        program: env::current_exe().unwrap(),
        fd: PASST_AT,
        words: vec!["--quiet".into(), "--one".into()],
    };
    // SAFETY: the variable is set before any thread exists.
    unsafe { env::set_var(PASST_VARIABLE, "1") };
    start_passt(&passt).unwrap();
    let flags = unsafe { libc::fcntl(PASST_AT, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0, "it stays open for QEMU");
    let sent = unsafe { libc::write(PASST_AT, b"ping".as_ptr().cast(), 4) };
    assert_eq!(sent, 4);
    let mut answer = [0u8; 4];
    let read = unsafe { libc::read(PASST_AT, answer.as_mut_ptr().cast(), 4) };
    assert_eq!((read, &answer), (4, b"ping"), "passt's end is the other end of QEMU's");
}

fn qemu_with_no_new_unix_sockets() {
    seccomp::install(&filters::qemu(true), Threads::This).unwrap();
    assert!(
        refused(UnixStream::connect("/run/user/1000/bus"))
            || unsafe { *libc::__errno_location() } == libc::EPERM
    );
    let made = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    assert_eq!((made, std::io::Error::last_os_error().raw_os_error()), (-1, Some(libc::EPERM)));
    assert!(UnixStream::pair().is_ok(), "a pair reaches nothing");
    assert!(TcpListener::bind(("127.0.0.1", 0)).is_ok(), "the user network stays");
}

const CASES: [(&str, fn()); 8] = [
    ("qemu_with_no_new_unix_sockets", qemu_with_no_new_unix_sockets),
    ("passt_connection", passt_connection),
    ("bridged_device", bridged_device),
    ("files_and_ports", files_and_ports),
    ("devices", devices),
    ("unix_sockets", unix_sockets),
    ("daemon_calls", daemon_calls),
    ("qemu_calls", qemu_calls),
];

fn set_up(folder: &Path) {
    for (name, text) in [("mine", "mine"), ("theirs", "theirs")] {
        fs::create_dir_all(folder.join(name)).unwrap();
        fs::write(folder.join(name).join("file"), text).unwrap();
    }
}

fn main() {
    if env::var_os(HELPER_VARIABLE).is_some() {
        pretend_bridge_helper();
        return;
    }
    if env::var_os(PASST_VARIABLE).is_some() {
        pretend_passt();
        return;
    }
    if let Ok(case) = env::var(CASE_VARIABLE) {
        let (_, run) = CASES.iter().find(|(name, _)| *name == case).unwrap();
        run();
        return;
    }
    if let Some(refusal) = Support::probe().refusal() {
        println!("skipped: {refusal}");
        return;
    }
    let mut failed = Vec::new();
    for (name, _) in CASES {
        let folder = env::temp_dir().join(format!("gh-jail-{}-{name}", process::id()));
        set_up(&folder);
        let done = Command::new(env::current_exe().unwrap())
            .env(CASE_VARIABLE, name)
            .env(FOLDER_VARIABLE, &folder)
            .status()
            .unwrap();
        let _ = fs::remove_dir_all(&folder);
        println!("{name} ... {}", if done.success() { "ok" } else { "FAILED" });
        if !done.success() {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}
