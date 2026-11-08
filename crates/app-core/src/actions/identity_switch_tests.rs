use super::*;
use crate::actions::boot;
use crate::actions::desktop::{BOOT_LINE, SERIAL_FILE};
use crate::actions::identity_restart::RESTART_SCRIPT;
use crate::actions::tests::{machine, plain_task_context};
use crate::agent::GuestAgent;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::failure::Failure;
use crate::gpu_identity::{MICROSOFT_RENDER, read_state, write_state};
use crate::machine::Machine;
use crate::testing::Scratch;
use crate::vm_args::VmPlan;
use crate::vm_host::VmHost;
use crate::vm_host::stand_in::RecordingHost;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DESKTOP_UP: &str = "3\n";
const GPU_UP: &str = "gpu working 1\n";
const NO_GPU: &str = "gpu working 0\n";
const STAGED: &str = "package in: oem7.inf\r\nidentity staged\r\n";
const REFUSED: &str = "stay: Windows' driver store has no copy of vrd.sys.\r\n";
const RESTARTED: &str = "restarting\r\n";
/// What the stand-in host says when asked to stop with the time a shutdown by choice gets.
const STOP_BY_CHOICE: &str = "stop 100";

fn staged() -> IdentityState {
    IdentityState { shown: Shown::Nvidia, restart_due: true, ..IdentityState::default() }
}

#[test]
fn a_start_that_staged_the_package_on_microsofts_number_starts_a_new_vm() {
    assert!(new_vm_needed(MICROSOFT_RENDER, &staged()));
}

#[test]
fn a_vm_already_on_nvidias_number_is_not_started_again() {
    assert!(!new_vm_needed(NVIDIA_RENDER, &staged()));
    let settled = IdentityState { shown: Shown::Nvidia, ..IdentityState::default() };
    assert!(!new_vm_needed(NVIDIA_RENDER, &settled));
}

#[test]
fn a_guest_that_kept_microsofts_number_is_not_started_again() {
    let refused =
        IdentityState { reason: "why".into(), failed_script: "abc".into(), ..IdentityState::default() };
    assert!(!new_vm_needed(MICROSOFT_RENDER, &refused));
    assert!(!new_vm_needed(MICROSOFT_RENDER, &IdentityState::default()));
}

#[test]
fn a_fall_back_to_microsofts_number_is_not_a_reason_for_a_new_vm() {
    let fell_back = IdentityState { reason: "back".into(), ..IdentityState::default() };
    assert!(!new_vm_needed(NVIDIA_RENDER, &fell_back), "that start has failed and ends");
}

/// A guest that answers as scripted, and whose firmware boots again when Windows is asked to restart.
struct Guest {
    scripted: ScriptedAgent,
    serial: PathBuf,
}

impl GuestAgent for Guest {
    fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
        if script == RESTART_SCRIPT {
            let mut serial = OpenOptions::new().append(true).create(true).open(&self.serial).unwrap();
            serial.write_all(BOOT_LINE).unwrap();
        }
        self.scripted.ask(port, token, script, seconds)
    }
}

struct Rig {
    context: Context,
    guest: Arc<Guest>,
    machine: Machine,
}

impl Rig {
    /// A guest on a QEMU that can set the identity, with the identity script in the program folder.
    fn new(scratch: &Scratch, answers: &[&str]) -> Self {
        let mut context = plain_task_context(scratch);
        scratch.write("program/guest/gpu-identity.ps1", "make the package");
        let qemu = scratch.write("qemu", "#!/bin/sh\necho 'channel=<str>'; echo 'vendor-id=<uint16>'\n");
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o700)).unwrap();
        context.paths.qemu = qemu;
        let folder = context.run_folder(None);
        fs::create_dir_all(&folder).unwrap();
        let serial = folder.join(SERIAL_FILE);
        fs::write(&serial, BOOT_LINE).unwrap();
        let guest = Arc::new(Guest { scripted: ScriptedAgent::answering(answers), serial });
        context.agent = guest.clone();
        let machine = machine(&context);
        crate::agent::make_token(&context, &machine).unwrap();
        Self { context, guest, machine }
    }

    fn boot(&self, host: &RecordingHost) -> (Result<()>, String) {
        let mut task = Task::new(&self.context, host, "start", self.machine.clone());
        let result = boot(&mut task);
        (result, task.error)
    }

    fn asked(&self) -> Vec<String> {
        self.guest.scripted.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect()
    }

    fn state(&self) -> IdentityState {
        read_state(&self.context, &self.machine)
    }
}

fn identities(host: &RecordingHost) -> Vec<PciIdentity> {
    host.plans.lock().unwrap().iter().map(|plan| plan.render_identity).collect()
}

#[test]
fn the_start_that_stages_the_package_ends_on_nvidias_number_after_one_new_vm() {
    let scratch = Scratch::new();
    let rig =
        Rig::new(&scratch, &[DESKTOP_UP, GPU_UP, STAGED, DESKTOP_UP, GPU_UP, RESTARTED, DESKTOP_UP, GPU_UP]);
    let host = RecordingHost::default();
    let (result, error) = rig.boot(&host);
    assert_eq!(result, Ok(()));
    assert_eq!(error, "");
    assert_eq!(host.said()[..3], ["start gpu", STOP_BY_CHOICE, "start gpu"]);
    assert_eq!(identities(&host), [MICROSOFT_RENDER, NVIDIA_RENDER]);
    let asked = rig.asked();
    assert!(asked[2].ends_with("make the package"), "the package is made in the first VM");
    assert_eq!(asked[5], RESTART_SCRIPT, "and Windows restarts once in the second");
    assert_eq!(rig.state(), IdentityState { shown: Shown::Nvidia, ..IdentityState::default() });

    let host = RecordingHost::default();
    let before = rig.asked().len();
    *rig.guest.scripted.answers.lock().unwrap() = [DESKTOP_UP, GPU_UP].map(String::from).into();
    assert_eq!(rig.boot(&host).0, Ok(()));
    assert_eq!(host.said()[..1], ["start gpu"], "the next start is one VM");
    assert_eq!(identities(&host), [NVIDIA_RENDER]);
    assert!(!rig.asked()[before..].contains(&RESTART_SCRIPT.to_owned()));
}

#[test]
fn a_start_already_due_its_windows_restart_needs_no_new_vm() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, &[DESKTOP_UP, GPU_UP, RESTARTED, DESKTOP_UP, GPU_UP]);
    write_state(&rig.context, &rig.machine, &staged()).unwrap();
    let host = RecordingHost::default();
    assert_eq!(rig.boot(&host), (Ok(()), String::new()));
    assert_eq!(identities(&host), [NVIDIA_RENDER]);
    assert!(!host.said().contains(&STOP_BY_CHOICE.to_owned()));
    assert_eq!(rig.asked()[2], RESTART_SCRIPT);
}

#[test]
fn a_guest_that_will_not_take_the_package_stays_in_one_vm_and_says_why() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, &[DESKTOP_UP, GPU_UP, REFUSED]);
    let host = RecordingHost::default();
    assert_eq!(rig.boot(&host).0, Ok(()));
    assert_eq!(identities(&host), [MICROSOFT_RENDER]);
    assert!(!host.said().contains(&STOP_BY_CHOICE.to_owned()));
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert!(state.reason.contains("vrd.sys"));
}

#[test]
fn a_new_vm_with_no_gpu_goes_back_to_microsofts_number_and_turns_off() {
    let scratch = Scratch::new();
    let mut answers = vec![DESKTOP_UP, GPU_UP, STAGED, DESKTOP_UP];
    answers.extend([NO_GPU; 10]);
    let rig = Rig::new(&scratch, &answers);
    let host = RecordingHost::default();
    let (result, _) = rig.boot(&host);
    assert!(result.unwrap_err().0.contains("Microsoft's vendor ID again"));
    assert_eq!(host.said(), ["start gpu", STOP_BY_CHOICE, "start gpu", "stop 30"]);
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert!(!state.restart_due);
}

#[test]
fn a_new_vm_that_will_not_start_leaves_nothing_running() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, &[DESKTOP_UP, GPU_UP, STAGED]);
    let host = RecordingHost::default();
    let mut task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    let stand_in = StartsOnce { host: &host, started: Default::default() };
    task.host = &stand_in;
    let result = boot(&mut task);
    assert_eq!(result, Err(Failure::new("no room")));
    assert_eq!(host.said(), ["start gpu", STOP_BY_CHOICE, "start gpu", "release", "stop 30"]);
    assert_eq!(rig.state().shown, Shown::Nvidia, "the package is in, so the next start tries again");
}

/// A host whose second start fails.
struct StartsOnce<'a> {
    host: &'a RecordingHost,
    started: AtomicBool,
}

impl VmHost for StartsOnce<'_> {
    fn start(&self, plan: &VmPlan, machine: &Machine) -> Result<()> {
        self.host.start(plan, machine)?;
        if self.started.swap(true, Ordering::Relaxed) {
            return Err(Failure::new("no room"));
        }
        Ok(())
    }

    fn stop(&self, machine: &Machine, seconds: u64) -> Result<Stopped> {
        self.host.stop(machine, seconds)
    }

    fn release(&self) -> Result<()> {
        self.host.release()
    }
}
