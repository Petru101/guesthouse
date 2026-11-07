//! NVIDIA's vendor number going into a machine. A start on Microsoft's number has Windows write the
//! driver package for NVIDIA's, and the next start shows it. Nothing here can stop a machine: a guest
//! that won't take the package keeps Microsoft's number, and the row says why.

use super::Task;
use crate::agent::guest_script;
use crate::failure::{Failure, Result};
use crate::gpu_identity::{IdentityState, NVIDIA_RENDER, Shown, read_state, write_state};
use crate::gpu_support::qemu_sets_identity;
use crate::log::write_log;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const SCRIPT: &str = "gpu-identity.ps1";
/// Making and signing the package took about 20 seconds in a guest, and this is what the agent gets.
const STAGE_SECONDS: u64 = 110;
const STAGED_MARK: &str = "identity staged";
const STAY_MARK: &str = "stay: ";

const QEMU_WITHOUT: &str = "This QEMU can't show Windows NVIDIA's vendor ID, so Windows sees Microsoft's.";
const NO_SCRIPT: &str = "This build has no script for NVIDIA's vendor ID, so Windows sees Microsoft's.";
const DID_NOT_STAGE: &str = "Windows could not make the driver package for NVIDIA's vendor ID, so it keeps \
                             Microsoft's. The log has what Windows said.";
const NEXT_START: &str =
    "Windows sees Microsoft's vendor ID until its next start, when it will see NVIDIA's.";
const FELL_BACK: &str = "The next start shows Windows Microsoft's vendor ID again.";

/// Tells one version of the guest script from another, so a failed one isn't run again.
fn script_id(script: &str) -> String {
    let mut hasher = DefaultHasher::new();
    script.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Has Windows write the package, and records what came of it.
fn stage(task: &Task, state: &mut IdentityState, script: &str) -> Result<()> {
    let (vendor, device) = (NVIDIA_RENDER.vendor, NVIDIA_RENDER.device);
    let sent = format!("$vendor = '{vendor:04X}'\n$device = '{device:04X}'\n{script}");
    let said = task.ask_guest(&sent, STAGE_SECONDS);
    write_log(task.context, &format!("gpu identity: {}", said.trim()))?;
    if said.lines().any(|line| line.trim() == STAGED_MARK) {
        state.shown = Shown::Nvidia;
        state.failed_script.clear();
        state.reason.clear();
        state.note = NEXT_START.to_owned();
        return Ok(());
    }
    state.failed_script = script_id(script);
    let why = said.lines().find_map(|line| line.trim().strip_prefix(STAY_MARK));
    state.reason = match why {
        Some(why) => format!("Windows keeps Microsoft's vendor ID. {why}"),
        None => DID_NOT_STAGE.to_owned(),
    };
    Ok(())
}

fn set_up(task: &Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    state.note.clear();
    if state.shown == Shown::Nvidia {
        return write_state(task.context, &task.machine, &state);
    }
    task.step("Preparing NVIDIA's vendor ID");
    if !qemu_sets_identity(&task.context.paths.qemu) {
        state.reason = QEMU_WITHOUT.to_owned();
        return write_state(task.context, &task.machine, &state);
    }
    let Ok(script) = guest_script(task.context, SCRIPT) else {
        state.reason = NO_SCRIPT.to_owned();
        return write_state(task.context, &task.machine, &state);
    };
    if state.failed_script != script_id(&script) {
        stage(task, &mut state, &script)?;
    }
    write_state(task.context, &task.machine, &state)
}

/// What each boot does, once the GPU and the desktop are up. Whatever goes wrong here goes in the log
/// and on the row, and can't reach the code that turns off a VM whose start has failed.
pub(super) fn set_up_identity(task: &mut Task) {
    if let Err(failure) = set_up(task) {
        let said = format!("NVIDIA's vendor ID could not be prepared: {}", failure.0);
        let _ = write_log(task.context, &said);
        task.error = said;
    }
}

/// Windows started without its GPU. If it was shown NVIDIA's number, that's the first suspect, and
/// the next start goes back to the one that always works.
pub(super) fn after_failed_gpu(task: &Task, failure: Failure) -> Failure {
    let mut state = read_state(task.context, &task.machine);
    if state.shown != Shown::Nvidia {
        return failure;
    }
    state.shown = Shown::Microsoft;
    state.note.clear();
    state.reason = FELL_BACK.to_owned();
    if let Err(problem) = write_state(task.context, &task.machine, &state) {
        let _ = write_log(task.context, &format!("gpu identity: could not fall back: {}", problem.0));
    }
    Failure(format!("{} {FELL_BACK}", failure.0))
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
