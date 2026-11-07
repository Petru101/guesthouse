use super::*;
use crate::actions::desktop::{BOOT_LINE, SERIAL_FILE};
use crate::actions::tests::{machine, plain_task_context};
use crate::agent::GuestAgent;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::gpu_identity::{IdentityState, Shown};
use crate::machine::Machine;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

const DESKTOP_UP: &str = "3\n";
const GPU_UP: &str = "gpu working 1\n";

/// A guest that answers as scripted, and whose firmware boots again when Windows is asked to restart.
struct Guest {
    scripted: ScriptedAgent,
    serial: PathBuf,
    restarts: bool,
}

impl GuestAgent for Guest {
    fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
        if self.restarts && script == RESTART_SCRIPT {
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
    fn new(scratch: &Scratch, state: IdentityState, restarts: bool, answers: &[&str]) -> Self {
        let mut context = plain_task_context(scratch);
        let folder = context.run_folder(None);
        fs::create_dir_all(&folder).unwrap();
        let serial = folder.join(SERIAL_FILE);
        fs::write(&serial, BOOT_LINE).unwrap();
        let guest = Arc::new(Guest { scripted: ScriptedAgent::answering(answers), serial, restarts });
        context.agent = guest.clone();
        let machine = machine(&context);
        crate::agent::make_token(&context, &machine).unwrap();
        write_state(&context, &machine, &state).unwrap();
        Self { context, guest, machine }
    }

    fn start(&self) -> (Result<()>, String) {
        let host = RecordingHost::default();
        let mut task = Task::new(&self.context, &host, "start", self.machine.clone());
        let result = restart_after_switch(&mut task);
        (result, task.error)
    }

    fn asked(&self) -> Vec<String> {
        self.guest.scripted.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect()
    }

    fn state(&self) -> IdentityState {
        read_state(&self.context, &self.machine)
    }
}

fn switching() -> IdentityState {
    IdentityState { shown: Shown::Nvidia, restart_due: true, ..IdentityState::default() }
}

#[test]
fn only_the_first_start_on_nvidias_number_restarts_windows() {
    assert!(switching().restarts_windows());
    let settled = IdentityState { shown: Shown::Nvidia, ..IdentityState::default() };
    assert!(!settled.restarts_windows());
    assert!(!IdentityState::default().restarts_windows());
    let back = IdentityState { shown: Shown::Microsoft, restart_due: true, ..IdentityState::default() };
    assert!(!back.restarts_windows(), "Microsoft's number never needs it");
}

#[test]
fn the_switching_start_restarts_windows_and_waits_for_its_desktop_and_gpu() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &["restarting\r\n", DESKTOP_UP, GPU_UP]);
    let (result, error) = rig.start();
    assert_eq!(result, Ok(()));
    assert_eq!(error, "");
    let asked = rig.asked();
    assert_eq!(asked.len(), 3);
    assert_eq!(asked[0], RESTART_SCRIPT);
    assert!(asked[1].contains("explorer"), "then the desktop is waited for");
    assert!(asked[2].contains("Win32_VideoController"), "and the GPU checked again");
    assert_eq!(rig.state(), IdentityState { shown: Shown::Nvidia, ..IdentityState::default() });

    let (result, _) = rig.start();
    assert_eq!(result, Ok(()));
    assert_eq!(rig.asked().len(), 3, "the next start doesn't restart it again");
}

#[test]
fn a_start_on_nvidias_number_that_has_settled_asks_nothing() {
    let scratch = Scratch::new();
    let settled = IdentityState { shown: Shown::Nvidia, ..IdentityState::default() };
    let rig = Rig::new(&scratch, settled.clone(), true, &[]);
    assert_eq!(rig.start(), (Ok(()), String::new()));
    assert!(rig.asked().is_empty());
    assert_eq!(rig.state(), settled);
}

#[test]
fn a_fall_back_to_microsofts_number_does_not_restart_windows() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &[]);
    let host = RecordingHost::default();
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(rig.state().shown, Shown::Microsoft);
    assert_eq!(rig.start(), (Ok(()), String::new()));
    assert!(rig.asked().is_empty());
}

#[test]
fn a_windows_that_does_not_restart_is_said_and_not_asked_again() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), false, &[]);
    let (result, error) = rig.start();
    assert_eq!(result, Ok(()));
    assert_eq!(error, DID_NOT_RESTART);
    assert_eq!(rig.asked(), [RESTART_SCRIPT]);
    assert!(!rig.state().restart_due);
}

#[test]
fn a_desktop_that_does_not_come_back_fails_the_start() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &["restarting\r\n"]);
    let (result, _) = rig.start();
    assert_eq!(result, Err(Failure::new(DESKTOP_GONE)));
    assert!(!rig.state().restart_due, "the next start goes on without it");
}

#[test]
fn a_gpu_gone_after_the_restart_goes_back_to_microsofts_number() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &["restarting\r\n", DESKTOP_UP]);
    let (result, _) = rig.start();
    assert!(result.unwrap_err().0.contains("without its GPU"));
    assert_eq!(rig.state().shown, Shown::Microsoft);
}
