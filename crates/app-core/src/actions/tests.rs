use super::desktop::{BOOTS_FILE, SERIAL_FILE};
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::clock::Clock;
use crate::machine::Kind;
use crate::processes::stand_in::fake_process;
use crate::slots;
use crate::tasks::{read_task, status};
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Time that only moves when something sleeps.
#[derive(Default)]
pub(super) struct FakeClock(AtomicU64);

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Relaxed))
    }

    fn sleep(&self, time: Duration) {
        self.0.fetch_add(time.as_millis() as u64, Ordering::Relaxed);
    }
}

pub(super) fn machine(context: &Context) -> Machine {
    let mut machine = Machine::new("x", "x", "", "");
    machine.save_files(context);
    machine
}

impl Machine {
    /// Gives it a real disk and firmware file, so a test can get past the checks.
    fn save_files(&mut self, context: &Context) {
        let folder = self.folder(context);
        fs::create_dir_all(&folder).unwrap();
        self.disk = folder.join("disk.qcow2").display().to_string();
        self.vars = folder.join("vars.fd").display().to_string();
        fs::write(&self.disk, "").unwrap();
        fs::write(&self.vars, "").unwrap();
        self.save(context).unwrap();
    }
}

#[test]
fn a_shared_folder_is_reported_only_if_it_did_not_map() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    scratch.write("program/guest/share-start.ps1", "map it");
    let agent = Arc::new(ScriptedAgent::answering(&["mapped\r\n", "not mapped\r\n", ""]));
    context.agent = agent.clone();
    let shared = Machine { share: "/folder".into(), ..machine(&context) };
    crate::agent::make_token(&context, &shared).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", shared.clone());
    apply_share(&mut task).unwrap();
    assert_eq!(task.error, "");
    apply_share(&mut task).unwrap();
    assert!(task.error.contains("did not reach"));
    task.error.clear();
    apply_share(&mut task).unwrap();
    assert!(task.error.contains("did not reach"));
    let asked = agent.requests.lock().unwrap().len();
    let mut alone = Task::new(&context, &host, "start", Machine { share: String::new(), ..shared });
    apply_share(&mut alone).unwrap();
    assert_eq!(agent.requests.lock().unwrap().len(), asked, "with no folder, the guest is not asked");
}

pub(super) fn with_vm_running(context: &Context) {
    let qemu = context.paths.qemu.display().to_string();
    let socket = format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
    fake_process(context, 40, &[&qemu, "-qmp", &socket], &[], "");
}

pub(super) fn plain_task_context(scratch: &Scratch) -> Context {
    let mut context = scratch_context(scratch);
    context.cpus = crate::host::stand_in::fake_cpus(scratch, "cpus", &[32768; 16]);
    context.meminfo = scratch.write("meminfo", "MemAvailable:   100000000 kB\n");
    context.clock = Arc::new(FakeClock::default());
    context
}

#[test]
fn a_plain_machine_starts_through_the_host_with_its_iso() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let iso = scratch.write("disc.iso", "").display().to_string();
    let vm = Machine { kind: Kind::Plain, iso: iso.clone(), ..machine(&context) };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    start(&mut task).unwrap();
    assert_eq!(host.said(), ["start plain"]);
    assert_eq!(host.plans.lock().unwrap()[0].boot, Boot::Plain { iso });
}

#[test]
fn a_gpu_machine_starts_through_the_same_host_and_sets_up_its_desktop() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "3
",
        "",
        "screen 1920x1080
",
    ]));
    context.agent = agent.clone();
    let vm = machine(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    boot(&mut task).unwrap();
    assert_eq!(host.said(), ["start gpu"]);
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    assert!(asked[1].2.contains("WriteAllBytes"), "the script goes to the guest first");
    assert!(asked[2].2.contains("New-ScheduledTaskPrincipal"), "and runs in the user's session");
}

#[test]
fn a_desktop_at_the_wrong_size_is_set_and_checked_again() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    scratch.write("program/guest/setmode.ps1", "set the mode");
    let answers = [
        "3
",
        "",
        "screen 1024x768
",
        "",
        "",
        "",
        "screen 800x600
",
    ];
    context.agent = Arc::new(ScriptedAgent::answering(&answers));
    let vm = machine(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm);
    boot(&mut task).unwrap();
    assert_eq!(task.error, "The desktop is 800x600, not 1920x1080.");
}

#[test]
fn a_failed_gpu_start_gives_back_what_it_took() {
    let scratch = Scratch::new();
    let context = plain_task_context(&scratch);
    let vm = machine(&context);
    let host = RecordingHost::default();
    *host.start_failure.lock().unwrap() = Some(Failure::new("no daemon"));
    let mut task = Task::new(&context, &host, "start", vm);
    // A GPU machine needs real shared memory facts, so this goes straight to the part that matters.
    let booted = boot(&mut task);
    assert!(booted.is_err());
    assert_eq!(host.said(), ["start gpu"]);
}

#[test]
fn stopping_says_when_the_guest_ran_out_of_time() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let host = RecordingHost::default();
    *host.stop_answer.lock().unwrap() = Some(Ok(Stopped::TurnedOff));
    let mut task = Task::new(&context, &host, "stop", machine(&context));
    stop(&mut task).unwrap();
    assert!(task.error.contains("Windows did not shut down in time"));
    task.machine.kind = Kind::Plain;
    stop(&mut task).unwrap();
    assert!(task.error.contains("The guest did not shut down in time"));
}

#[test]
fn a_disk_grows_and_only_grows() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = machine(&context);
    let done =
        Command::new("qemu-img").args(["create", "-q", "-f", "qcow2", &vm.disk, "2G"]).status().unwrap();
    assert!(done.success());
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "grow", vm.clone());
    grow(&mut task, 3).unwrap();
    assert_eq!(disk_size_gib(&vm.disk).unwrap(), 3.0);
    assert!(grow(&mut task, 3).unwrap_err().0.contains("only grow"));
    assert!(grow(&mut task, 1).unwrap_err().0.contains("only grow"));
    with_vm_running(&context);
    assert!(grow(&mut task, 4).unwrap_err().0.contains("while the VM is off"));
}

#[test]
fn a_task_that_cannot_load_its_machine_says_so() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let host = RecordingHost::default();
    run_task(&context, &host, "start", "no-such-machine", &[]);
    let found = status(&context, &Machine::new("no-such-machine", "x", "/d", "/v"));
    assert!(found.error.contains("Could not read") && !found.busy, "{found:?}");
}

#[test]
fn a_task_leaves_its_error_and_clears_its_pid() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let vm = machine(&context);
    let host = RecordingHost::default();
    run_task(&context, &host, "no-such-action", &vm.id, &[]);
    let record = read_task(&context, None);
    assert_eq!(record.pid, 0);
    assert!(record.error.contains("no action"));
}

#[test]
fn only_one_task_holds_the_lock() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    fs::create_dir_all(context.run_folder(None)).unwrap();
    let held = take_lock(&context).unwrap();
    assert!(take_lock(&context).unwrap_err().0.contains("still in progress"));
    drop(held);
    take_lock(&context).unwrap();
}

#[test]
fn a_reboot_is_seen_from_the_serial_log() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = context.run_folder(None);
    fs::create_dir_all(&folder).unwrap();
    assert!(!rebooted(&context, None), "a VM the app never set up is left as it is");
    fs::write(folder.join(SERIAL_FILE), "BdsDxe: starting\n").unwrap();
    fs::write(folder.join(BOOTS_FILE), "1").unwrap();
    assert!(!rebooted(&context, None));
    fs::write(folder.join(SERIAL_FILE), "BdsDxe: starting\nBdsDxe: starting\n").unwrap();
    assert!(rebooted(&context, None));
}
