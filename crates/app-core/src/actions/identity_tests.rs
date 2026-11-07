use super::*;
use crate::actions::tests::{machine, plain_task_context};
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::gpu_identity::{MICROSOFT_RENDER, shown_identity};
use crate::machine::Machine;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

const STAGED: &str = "signing: Valid\r\npackage in: oem7.inf\r\nidentity staged\r\n";
const REFUSED: &str = "stay: Windows' driver store has no copy of vrd.sys.\r\n";
const SCRIPT_TEXT: &str = "make the package";

struct Rig {
    context: Context,
    agent: Arc<ScriptedAgent>,
    machine: Machine,
}

impl Rig {
    /// A guest answering as given, on a QEMU that can set the identity unless told otherwise.
    fn new(scratch: &Scratch, qemu_can: bool, answers: &[&str]) -> Self {
        let mut context = plain_task_context(scratch);
        scratch.write("program/guest/gpu-identity.ps1", SCRIPT_TEXT);
        let listing =
            if qemu_can { "echo 'channel=<str>'; echo 'vendor-id=<uint16>'" } else { "echo 'channel=<str>'" };
        let qemu = scratch.write("qemu", format!("#!/bin/sh\n{listing}\n"));
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o700)).unwrap();
        context.paths.qemu = qemu;
        let agent = Arc::new(ScriptedAgent::answering(answers));
        context.agent = agent.clone();
        let machine = machine(&context);
        crate::agent::make_token(&context, &machine).unwrap();
        Self { context, agent, machine }
    }

    fn boot(&self) -> String {
        let host = RecordingHost::default();
        let mut task = Task::new(&self.context, &host, "start", self.machine.clone());
        set_up_identity(&mut task);
        task.error
    }

    fn asked(&self) -> Vec<String> {
        self.agent.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect()
    }

    fn state(&self) -> IdentityState {
        read_state(&self.context, &self.machine)
    }
}

#[test]
fn a_guest_that_takes_the_package_is_shown_nvidias_number_from_the_next_start() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    assert_eq!(shown_identity(&rig.context, &rig.machine), MICROSOFT_RENDER);
    assert_eq!(rig.boot(), "");
    let asked = rig.asked();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].starts_with("$vendor = '10DE'\n$device = '2BFE'\n"), "the script gets the numbers");
    assert!(asked[0].ends_with(SCRIPT_TEXT));
    let state = rig.state();
    assert_eq!(state.shown, Shown::Nvidia);
    assert_eq!(state.reason, "");
    assert_eq!(state.note, NEXT_START);
    assert_eq!(shown_identity(&rig.context, &rig.machine), NVIDIA_RENDER);
}

#[test]
fn a_guest_that_keeps_microsofts_number_says_why_and_is_not_asked_again() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[REFUSED, STAGED]);
    assert_eq!(rig.boot(), "");
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(
        state.reason,
        "Windows keeps Microsoft's vendor ID. Windows' driver store has no copy of vrd.sys."
    );
    assert_eq!(state.note, "");
    assert!(!state.failed_script.is_empty());
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 1, "the same script isn't tried again");
    assert_eq!(rig.state(), state);
}

#[test]
fn a_guest_that_says_nothing_useful_keeps_microsofts_number_too() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &["pnputil said something else"]);
    assert_eq!(rig.boot(), "");
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(state.reason, DID_NOT_STAGE);
}

#[test]
fn a_qemu_without_the_properties_keeps_microsofts_number_without_asking() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, false, &[STAGED]);
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 0);
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(state.reason, QEMU_WITHOUT);
}

#[test]
fn a_machine_on_nvidias_number_is_left_alone_and_its_note_goes() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    let state =
        IdentityState { shown: Shown::Nvidia, note: NEXT_START.to_owned(), ..IdentityState::default() };
    write_state(&rig.context, &rig.machine, &state).unwrap();
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 0);
    assert_eq!(rig.state(), IdentityState { shown: Shown::Nvidia, ..IdentityState::default() });
}

#[test]
fn a_failed_gpu_start_on_nvidias_number_goes_back_to_microsofts() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[]);
    let host = RecordingHost::default();
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    let same = after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(same.0, "no GPU", "a machine on Microsoft's number has nothing to go back to");
    let state = IdentityState { shown: Shown::Nvidia, ..IdentityState::default() };
    write_state(&rig.context, &rig.machine, &state).unwrap();
    let said = after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(said.0, format!("no GPU {FELL_BACK}"));
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(state.reason, FELL_BACK);
}

#[test]
fn a_build_without_the_script_keeps_microsofts_number_and_says_so() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    fs::remove_file(rig.context.paths.guest_files().join("gpu-identity.ps1")).unwrap();
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 0);
    assert_eq!(rig.state().reason, NO_SCRIPT);
}
