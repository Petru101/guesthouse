//! The start where Windows takes the driver package for its card maker's vendor number. The number is part of
//! QEMU's command line, so only a new QEMU can show it, and a user left on Microsoft's would find Edge
//! on the CPU until they thought to stop and start again. So that start stops the VM and starts it
//! once more on the maker's number, and only then is the machine ready.

use super::STOP_WAIT_SECONDS;
use super::Task;
use crate::failure::Result;
use crate::gpu_identity::{IdentityState, PciIdentity, Shown, maker_identity, maker_name};
use crate::log::write_log;
use crate::machine::Machine;
use crate::vm_host::Stopped;

pub(super) fn restarting_vm(machine: &Machine) -> String {
    format!("Restarting the VM once for {}'s vendor ID", maker_name(machine))
}

/// Whether a VM started on `running` has to go and come back before this start is done. Only a move
/// to the maker's number does: going back to Microsoft's comes from a failed start, which ends anyway.
pub(super) fn new_vm_needed(running: PciIdentity, state: &IdentityState, machine: &Machine) -> bool {
    state.shown == Shown::Maker && running != maker_identity(machine)
}

/// Shuts Windows down so the VM can start again with the new number. A Windows that had to be turned
/// off has still taken the package, so the start goes on.
pub(super) fn stop_for_new_identity(task: &mut Task) -> Result<()> {
    task.step(&restarting_vm(&task.machine));
    if task.host.stop(&task.machine, STOP_WAIT_SECONDS)? == Stopped::TurnedOff {
        write_log(task.context, "gpu identity: Windows did not shut down in time, so the VM was turned off")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "identity_switch_tests.rs"]
mod tests;
