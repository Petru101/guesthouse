//! The first start on the card maker's vendor number. Windows installs the render device again under its new
//! hardware id while it boots, and until it has started once more, Direct3D 12 can't make a device on
//! the GPU, so Edge and games would find no Direct3D 12. Direct3D 11, Vulkan and CUDA are fine. So that
//! one start restarts Windows by itself before the machine counts as ready.

use super::Task;
use super::desktop::{boots, wait_for_desktop};
use super::gpu_check::check_gpu;
use super::identity::after_failed_gpu;
use crate::failure::{Failure, Result};
use crate::gpu_identity::{maker_name, read_state, write_state};
use crate::log::write_log;
use crate::machine::Machine;
use std::time::Duration;

pub(super) fn restarting(machine: &Machine) -> String {
    format!("Restarting Windows once for {}'s vendor ID", maker_name(machine))
}
pub(super) const RESTART_SCRIPT: &str = "shutdown.exe /r /t 0; 'restarting'";
const RESTART_ANSWER_SECONDS: u64 = 10;
/// Windows took about 20 seconds from being asked to the firmware's next boot line.
const NEW_BOOT_WAIT: Duration = Duration::from_secs(120);
const NEW_BOOT_POLL: Duration = Duration::from_secs(2);
pub(super) const DID_NOT_RESTART: &str =
    "Windows did not restart, so Direct3D 12 may not work until the VM is started again.";
const DESKTOP_GONE: &str = "Windows restarted, but its desktop never came back.";

/// Whether the firmware has started another boot since `before`.
fn wait_for_new_boot(task: &Task, before: usize) -> bool {
    let end = task.now() + NEW_BOOT_WAIT;
    while task.now() < end {
        if boots(task.context, None) > before {
            return true;
        }
        task.sleep(NEW_BOOT_POLL);
    }
    false
}

/// Restarts Windows if this is its first start on its maker's number, and waits for the desktop again.
pub(super) fn restart_after_switch(task: &mut Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    if !state.restarts_windows() {
        return Ok(());
    }
    task.step(&restarting(&task.machine));
    let before = boots(task.context, None);
    task.ask_guest(RESTART_SCRIPT, RESTART_ANSWER_SECONDS);
    let restarted = wait_for_new_boot(task, before);
    // Once is enough either way: by the next start the device's install has long finished.
    state.restart_due = false;
    write_state(task.context, &task.machine, &state)?;
    if !restarted {
        write_log(task.context, "gpu identity: Windows did not restart when asked")?;
        task.error = DID_NOT_RESTART.to_owned();
        return Ok(());
    }
    task.step("Waiting for the desktop");
    if !wait_for_desktop(task) {
        return Err(Failure::new(DESKTOP_GONE));
    }
    check_gpu(task).map_err(|failure| after_failed_gpu(task, failure))
}

#[cfg(test)]
#[path = "identity_restart_tests.rs"]
mod tests;
