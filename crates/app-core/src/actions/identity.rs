//! The card maker's vendor number going into a machine. A start on Microsoft's number has Windows
//! write the driver package for the maker's, and then starts the VM again to show it (see identity_switch.rs),
//! restarting Windows once on the way (see identity_restart.rs). Nothing here can stop a machine: a
//! guest that won't take the package keeps Microsoft's number, and the row says why.

use super::Task;
use crate::agent::guest_script;
use crate::failure::{Failure, Result};
use crate::gpu_identity::{IdentityState, Shown, maker_identity, maker_name, read_state, write_state};
use crate::gpu_support::qemu_sets_identity;
use crate::log::write_log;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const SCRIPT: &str = "gpu-identity.ps1";
/// Making and signing the package took about 20 seconds in a guest, and this is what the agent gets.
const STAGE_SECONDS: u64 = 110;
const STAGED_MARK: &str = "identity staged";
const STAY_MARK: &str = "stay: ";

const FELL_BACK: &str = "The next start shows Windows Microsoft's vendor ID again.";

fn qemu_without(maker: &str) -> String {
    format!("This QEMU can't show Windows {maker}'s vendor ID, so Windows sees Microsoft's.")
}

fn no_script(maker: &str) -> String {
    format!("This build has no script for {maker}'s vendor ID, so Windows sees Microsoft's.")
}

fn did_not_stage(maker: &str) -> String {
    format!(
        "Windows could not make the driver package for {maker}'s vendor ID, so it keeps Microsoft's. \
         The log has what Windows said."
    )
}

fn next_start(maker: &str) -> String {
    format!("Windows sees Microsoft's vendor ID until its next start, when it will see {maker}'s.")
}

/// Tells one version of the guest script from another, so a failed one isn't run again.
fn script_id(script: &str) -> String {
    let mut hasher = DefaultHasher::new();
    script.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Has Windows write the package, and records what came of it.
fn stage(task: &Task, state: &mut IdentityState, script: &str) -> Result<()> {
    let identity = maker_identity(&task.machine);
    let maker = maker_name(&task.machine);
    let (vendor, device) = (identity.vendor, identity.device);
    let sent = format!("$vendor = '{vendor:04X}'\n$device = '{device:04X}'\n{script}");
    let said = task.ask_guest(&sent, STAGE_SECONDS);
    write_log(task.context, &format!("gpu identity: {}", said.trim()))?;
    if said.lines().any(|line| line.trim() == STAGED_MARK) {
        state.shown = Shown::Maker;
        state.package_vendor = vendor;
        state.failed_script.clear();
        state.reason.clear();
        state.note = next_start(maker);
        state.restart_due = true;
        return Ok(());
    }
    state.failed_script = script_id(script);
    let why = said.lines().find_map(|line| line.trim().strip_prefix(STAY_MARK));
    state.reason = match why {
        Some(why) => format!("Windows keeps Microsoft's vendor ID. {why}"),
        None => did_not_stage(maker),
    };
    Ok(())
}

fn set_up(task: &Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    state.note.clear();
    if state.shown == Shown::Maker {
        return write_state(task.context, &task.machine, &state);
    }
    let maker = maker_name(&task.machine);
    task.step(&format!("Preparing {maker}'s vendor ID"));
    if !qemu_sets_identity(&task.context.paths.qemu) {
        state.reason = qemu_without(maker);
        return write_state(task.context, &task.machine, &state);
    }
    let Ok(script) = guest_script(task.context, SCRIPT) else {
        state.reason = no_script(maker);
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
        let said = format!("{}'s vendor ID could not be prepared: {}", maker_name(&task.machine), failure.0);
        let _ = write_log(task.context, &said);
        task.error = said;
    }
}

/// Windows started without its GPU. If it was shown its maker's number, that's the first suspect, and
/// the next start goes back to the one that always works.
pub(super) fn after_failed_gpu(task: &Task, failure: Failure) -> Failure {
    let mut state = read_state(task.context, &task.machine);
    if state.shown != Shown::Maker {
        return failure;
    }
    state.shown = Shown::Microsoft;
    state.restart_due = false;
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
