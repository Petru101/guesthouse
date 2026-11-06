//! The whole flow, as the app drives it: a machine is started by a real task, runs, and is stopped.
//!
//! This program also stands in for QEMU and the daemon. The task runs copies of it under their names,
//! and a copy only answers as they would, so no VM runs. Set `GUESTHOUSE_TEST_DAEMON` to a build of
//! `guesthouse-daemon` with the `stand-in` feature to go through the real daemon instead.

use guesthouse_app_core::actions::run_task;
use guesthouse_app_core::agent::make_token;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::machine::{Kind, Machine};
use guesthouse_app_core::processes::qemu_pid;
use guesthouse_app_core::tasks::{State, Status, launch, status};
use guesthouse_app_core::text::base64_decode;
use guesthouse_app_core::vm_host::LocalHost;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command};
use std::thread;
use std::time::{Duration, Instant};

const INNER_VARIABLE: &str = "FLOW_TEST_INNER";
/// Long enough for the task's own waits for a desktop.
const MOST_WAIT: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(200);
const PRETEND_LIFE: Duration = Duration::from_secs(120);

fn serve_monitor(mut stream: UnixStream) {
    let _ = stream.write_all(b"{\"QMP\": {}}\n");
    let reader = BufReader::new(stream.try_clone().unwrap());
    for line in reader.lines().map_while(Result::ok) {
        let request: Value = serde_json::from_str(&line).unwrap_or_default();
        let answer = match request["execute"].as_str() {
            Some("query-status") => json!({"return": {"status": "running"}}),
            Some("system_powerdown") => process::exit(0),
            _ => json!({"return": {}}),
        };
        let _ = writeln!(stream, "{answer}");
    }
}

fn listen_on(socket: &str) {
    // QEMU replaces a socket an earlier run left.
    let _ = fs::remove_file(socket);
    let listener = UnixListener::bind(socket).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            thread::spawn(move || serve_monitor(stream));
        }
    });
}

/// What the guest's agent says, as far as a start needs it.
fn agent_answer(script: &str) -> &'static str {
    if script.contains("Stop-Computer") {
        process::exit(0);
    }
    if script.contains("Get-Process") {
        "3\r\n"
    } else if script.contains("Get-Content") {
        "screen 1920x1080\r\n"
    } else {
        "up\r\n"
    }
}

fn serve_agent(port: u16) {
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    thread::spawn(move || {
        for mut stream in listener.incoming().map_while(Result::ok) {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let encoded = line.split_whitespace().nth(1).unwrap_or_default();
            let script = base64_decode(encoded).unwrap_or_default();
            let _ = stream.write_all(agent_answer(&String::from_utf8_lossy(&script)).as_bytes());
        }
    });
}

fn pretend_qemu(arguments: &[String]) {
    for pair in arguments.windows(2) {
        if pair[0] == "-qmp" {
            let path = pair[1].strip_prefix("unix:").unwrap().split(',').next().unwrap();
            listen_on(path);
        }
        if pair[0] == "-nic" {
            let port = pair[1].split("hostfwd=tcp:127.0.0.1:").nth(1).unwrap().split('-').next().unwrap();
            serve_agent(port.parse().unwrap());
        }
    }
    thread::sleep(PRETEND_LIFE);
}

/// Makes the sockets QEMU connects to, as the daemon does once it's ready.
fn pretend_daemon(base: &str) {
    let folder = Path::new(base).join("run");
    let _sockets = ["vgpu.sock", "global.sock"].map(|name| UnixListener::bind(folder.join(name)).unwrap());
    thread::sleep(PRETEND_LIFE);
}

fn wait_for(context: &Context, machine: &Machine, wanted: impl Fn(&Status) -> bool) -> Status {
    let end = Instant::now() + MOST_WAIT;
    loop {
        let found = status(context, machine);
        if wanted(&found) {
            return found;
        }
        assert!(
            Instant::now() < end,
            "it never happened: {found:?}\nlog: {}",
            guesthouse_app_core::log::read_log(context)
        );
        thread::sleep(POLL);
    }
}

/// A task outlives what starts it, so nothing waits for it here but a thread.
fn collect(mut task: Child) {
    thread::spawn(move || task.wait());
}

fn start_and_stop(context: &Context, machine: &Machine) {
    let task = launch(context, "start", machine, &[], None).unwrap();
    collect(task);
    let running = wait_for(context, machine, |found| found.state == State::Running && !found.busy);
    assert_eq!(running.error, "", "{running:?}");
    println!("{} is running in slot {:?}", machine.name, running.slot);
    let task = launch(context, "stop", machine, &[], None).unwrap();
    collect(task);
    let off = wait_for(context, machine, |found| found.state == State::Off && !found.busy);
    assert_eq!(off.error, "", "{off:?}");
    assert_eq!(qemu_pid(context, None), None);
    println!("{} has stopped", machine.name);
}

fn make_machine(context: &Context, id: &str, kind: Kind) -> Machine {
    let folder = context.paths.machines.join(id);
    fs::create_dir_all(&folder).unwrap();
    let (disk, vars) = (folder.join("disk.qcow2"), folder.join("vars.fd"));
    fs::write(&disk, "").unwrap();
    fs::write(&vars, "").unwrap();
    let machine = Machine {
        kind,
        cores: 2,
        ram_gib: 4,
        ..Machine::new(id, id, &disk.to_string_lossy(), &vars.to_string_lossy())
    };
    machine.save(context).unwrap();
    machine
}

fn inner() {
    let mut context = Context::from_process().unwrap();
    context.paths.binaries = env::var_os("FLOW_BINARIES").unwrap().into();
    let plain = make_machine(&context, "plain", Kind::Plain);
    start_and_stop(&context, &plain);
    let windows = make_machine(&context, "windows", Kind::Windows);
    make_token(&context, &windows).unwrap();
    start_and_stop(&context, &windows);
    let daemons = context.paths.shared_file("ram", context.slot);
    assert!(!daemons.exists(), "the shared memory has been given back");
}

/// A port nothing has now. The real one could be held by a real VM.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn copy_program(from: &Path, to: PathBuf) {
    fs::copy(from, to).unwrap();
}

/// Where this test's own tasks read the host's memory figures. Only this program knows it.
const MEMINFO_VARIABLE: &str = "FLOW_MEMINFO";
/// Enough for the test's machine however busy the PC is.
const PLENTY_OF_MEMORY: &str = "MemAvailable: 67108864 kB\n";

fn outer() {
    let this = env::current_exe().unwrap();
    let folder = env::temp_dir().join(format!("gh-flow-{}", process::id()));
    let bin = folder.join("bin");
    let program = folder.join("program");
    for path in
        [&bin, &program.join("guest"), &program.join("capture"), &folder.join("shm"), &folder.join("run")]
    {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(program.join("capture/display.ps1"), "what the session sees").unwrap();
    fs::write(folder.join("meminfo"), PLENTY_OF_MEMORY).unwrap();
    copy_program(&this, bin.join("guesthouse-task"));
    copy_program(&this, bin.join("qemu"));
    let daemon = env::var_os("GUESTHOUSE_TEST_DAEMON").map_or_else(|| this.clone(), PathBuf::from);
    copy_program(&daemon, bin.join("guesthouse-daemon"));
    let done = Command::new(&this)
        .env(INNER_VARIABLE, "1")
        .env("FLOW_BINARIES", &bin)
        .env("HOME", folder.join("home"))
        .env("XDG_RUNTIME_DIR", folder.join("run"))
        .env("GUESTHOUSE_SHARED_MEMORY", folder.join("shm"))
        .env(MEMINFO_VARIABLE, folder.join("meminfo"))
        .env("GUESTHOUSE_PROGRAM", &program)
        .env("AGENT_PORT", free_port().to_string())
        .env("QEMU", bin.join("qemu"))
        .status()
        .unwrap();
    let _ = fs::remove_dir_all(&folder);
    assert!(done.success(), "the flow failed");
    println!("flow ok");
}

/// The real task, with the one difference that it can't be refused for what else the PC is doing.
fn task_with_room(arguments: &[String]) {
    let [action, machine, rest @ ..] = arguments else { panic!("a task needs an action and a machine") };
    let mut context = Context::from_process().unwrap();
    context.meminfo = env::var_os(MEMINFO_VARIABLE).unwrap().into();
    run_task(&context, &LocalHost { context: &context }, action, machine, rest);
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let name = env::args().next().unwrap_or_default();
    if name.ends_with("/qemu") {
        pretend_qemu(&arguments);
    } else if name.ends_with("guesthouse-daemon") {
        pretend_daemon(&arguments[0]);
    } else if name.ends_with("guesthouse-task") {
        task_with_room(&arguments);
    } else if env::var_os(INNER_VARIABLE).is_some() {
        inner();
    } else {
        outer();
    }
}
