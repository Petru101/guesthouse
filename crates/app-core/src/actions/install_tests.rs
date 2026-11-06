use super::super::desktop::DESKTOP_SCRIPT;
use super::super::tests::{FakeClock, machine, plain_task_context, with_vm_running};
use super::super::{Stopped, Task, VmHost, load};
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::machine::Kind;
use crate::monitor::stand_in::RecordingMonitor;
use crate::testing::{Scratch, scratch_context};
use crate::vm_args::VmPlan;
use crate::vm_host::stand_in::RecordingHost;
use std::sync::{Arc, Mutex};

#[test]
fn install_steps_follow_the_serial_log() {
    let line = "\"Windows Boot Manager\"\n";
    let cases = [
        (String::new(), 0),
        (
            "loading Boot0004 \"Windows Boot Manager\"\nstarting Boot0004 \"Windows Boot Manager\"\n"
                .to_owned(),
            1,
        ),
        (line.repeat(4), 2),
        (line.repeat(20), 3),
    ];
    for (text, step) in cases {
        assert_eq!(install_step(text.as_bytes()), INSTALL_STEPS[step]);
    }
}

fn with_finish_script(scratch: &Scratch) {
    scratch.write("program/guest/finish.ps1", "is it finished");
}

#[test]
fn a_disk_that_stops_growing_ends_the_wait_and_keeps_the_screen() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    with_finish_script(&scratch);
    with_vm_running(&context);
    context.clock = Arc::new(FakeClock::default());
    let monitor = Arc::new(RecordingMonitor::default());
    context.monitor = monitor.clone();
    let vm = machine(&context);
    fs::write(&vm.disk, vec![1u8; 1 << 16]).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    let error = wait_for_setup(&mut task).unwrap_err();
    assert!(error.0.contains("Show Screen"), "{error}");
    assert_eq!(monitor.asked()[0].name, "screendump");
}

/// Grows the disk a little each time it's asked, as a slow install does.
struct GrowingGuest {
    disk: String,
    left: Mutex<u32>,
}

impl crate::agent::GuestAgent for GrowingGuest {
    fn ask(&self, _port: u16, _token: &str, _script: &str, _seconds: u64) -> String {
        let mut left = self.left.lock().unwrap();
        *left = left.saturating_sub(1);
        let size = fs::metadata(&self.disk).unwrap().len();
        fs::write(&self.disk, vec![1u8; size as usize + (1 << 16)]).unwrap();
        if *left == 0 { "finished".to_owned() } else { String::new() }
    }
}

#[test]
fn growing_slowly_is_not_stuck() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    with_finish_script(&scratch);
    with_vm_running(&context);
    context.clock = Arc::new(FakeClock::default());
    let vm = machine(&context);
    context.agent = Arc::new(GrowingGuest { disk: vm.disk.clone(), left: Mutex::new(301) });
    crate::agent::make_token(&context, &vm).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    wait_for_setup(&mut task).unwrap();
}

#[test]
fn a_vm_that_goes_away_during_the_install_says_so() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_finish_script(&scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", machine(&context));
    assert!(wait_for_setup(&mut task).unwrap_err().0.contains("stopped during the install"));
}

#[test]
fn the_answer_file_gets_the_hosts_zone() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/guest/autounattend.xml", "<a><TimeZone>UTC</TimeZone></a>");
    assert_eq!(
        answer_file(&context, "Asia/Tokyo").unwrap(),
        "<a><TimeZone>Tokyo Standard Time</TimeZone></a>"
    );
    assert_eq!(answer_file(&context, "Mars/Olympus_Mons").unwrap(), "<a><TimeZone>UTC</TimeZone></a>");
}

fn with_install_files(scratch: &Scratch, context: &Context) {
    for name in INSTALL_FILES {
        scratch.write(&format!("program/guest/{name}"), "<TimeZone>UTC</TimeZone>");
    }
    scratch.write("program/guest/setmode.ps1", "mode");
    let driver = crate::settings::driver_folder(context);
    fs::create_dir_all(driver.join("sub")).unwrap();
    fs::write(driver.join("nv_dispi.inf"), "driver").unwrap();
    fs::write(driver.join("sub/a.dll"), "driver").unwrap();
}

fn on_disc(path: &Path, name: &str) -> Vec<u8> {
    let result = Command::new("isoinfo").args(["-J", "-x", name, "-i"]).arg(path).output().unwrap();
    result.stdout
}

#[test]
fn the_answer_disc_holds_this_machines_token_and_is_private() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_install_files(&scratch, &context);
    let vm = machine(&context);
    make_answer_disc(&context, &vm).unwrap();
    let path = answer_disc_path(&context, &vm);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600, "the disc holds the token");
    let listing = Command::new("isoinfo").args(["-J", "-f", "-i"]).arg(&path).output().unwrap().stdout;
    let listing = String::from_utf8_lossy(&listing);
    for name in [
        "/autounattend.xml",
        "/SetupComplete.cmd",
        "/setup.ps1",
        "/agent.ps1",
        "/setmode.ps1",
        "/token.txt",
        "/nvidia/nv_dispi.inf",
        "/nvidia/sub/a.dll",
    ] {
        assert!(listing.lines().any(|line| line == name), "{name} is not on the disc");
    }
    let token = fs::read_to_string(vm.token_file(&context)).unwrap();
    assert_eq!(String::from_utf8_lossy(&on_disc(&path, "/token.txt")).trim(), token.trim());
    assert!(!vm.folder(&context).join("answer-disc.new").exists(), "the files for the disc are gone");
}

/// Stands in for a guest that installs and shuts down.
#[derive(Default)]
struct InstalledGuest;

impl crate::agent::GuestAgent for InstalledGuest {
    fn ask(&self, _port: u16, _token: &str, script: &str, _seconds: u64) -> String {
        match script {
            "is it finished" => "finished",
            DESKTOP_SCRIPT => "3",
            _ if script.contains("check it") => "all there",
            _ => "",
        }
        .to_owned()
    }
}

fn install_setup(scratch: &Scratch) -> (Context, Machine) {
    let mut context = plain_task_context(scratch);
    context.install_room = Some(0);
    context.agent = Arc::new(InstalledGuest);
    with_install_files(scratch, &context);
    with_finish_script(scratch);
    scratch.write("program/guest/check.ps1", "check it");
    let iso = scratch.write("win.iso", "disc").display().to_string();
    let new = crate::library::NewMachine {
        name: "Steps".into(),
        iso,
        kind: Kind::Windows,
        cores: 2,
        ram_gib: 4,
        disk_gib: 64,
    };
    let vm = crate::library::create(&context, &new).unwrap();
    (context, vm)
}

/// A host whose VM shows up when started, and goes when the guest would shut it down.
struct InstallHost<'a> {
    inner: RecordingHost,
    context: &'a Context,
}

impl VmHost for InstallHost<'_> {
    fn start(&self, plan: &VmPlan, machine: &Machine) -> Result<()> {
        self.inner.start(plan, machine)?;
        with_vm_running(self.context);
        Ok(())
    }

    fn stop(&self, machine: &Machine, seconds: u64) -> Result<Stopped> {
        self.inner.stop(machine, seconds)
    }

    fn release(&self) -> Result<()> {
        self.inner.release()
    }
}

#[test]
fn a_failed_install_stays_one_so_it_can_start_over() {
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    // The guest never finishes, and goes away.
    context.agent = Arc::new(ScriptedAgent::default());
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    let error = install(&mut task).unwrap_err();
    assert!(error.0.contains("stopped during the install"), "{error}");
    assert!(load(&context, &vm.id).unwrap().installing);
}

#[test]
fn the_install_runs_its_steps_in_order_up_to_the_shutdown() {
    let scratch = Scratch::new();
    let (context, vm) = install_setup(&scratch);
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm.clone());
    let error = install(&mut task).unwrap_err();
    assert!(error.0.contains("would not shut down"), "the stand-in VM never goes: {error}");
    assert_eq!(host.inner.said(), ["start plain"], "an install has no GPU yet");
    let plans = host.inner.plans.lock().unwrap();
    let answer = answer_disc_path(&context, &vm);
    assert_eq!(plans[0].boot, Boot::Install { iso: vm.iso.clone(), answer: answer.clone() });
    assert!(answer.exists(), "the VM is still up, so the disc stays for now");
}

#[test]
fn a_machine_outside_its_folder_is_never_installed_over() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let elsewhere = Machine {
        installing: true,
        ..Machine::new("elsewhere", "Elsewhere", "/nowhere/disk", "/nowhere/vars")
    };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", elsewhere);
    assert!(install(&mut task).unwrap_err().0.contains("not in its folder"));
}
