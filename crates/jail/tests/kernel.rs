//! The jail on this PC's kernel. Each case runs in a child of its own, as a jail can't be undone,
//! and the child's exit says whether everything it tried went as it should.

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
    assert!(Command::new("/usr/bin/true").status().unwrap().success());
}

const CASES: [(&str, fn()); 5] = [
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
