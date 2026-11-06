//! The guest's desktop once it's up: the boots seen so far, its size, its rate and its shared folder.

use super::Task;
use crate::agent::guest_script;
use crate::context::Context;
use crate::failure::Result;
use crate::vm_host::Helper;
use guesthouse_daemon::slot::Slot;
use std::fs;
use std::time::Duration;

/// The firmware writes one of these to the serial log each boot.
pub(super) const BOOT_LINE: &[u8] = b"BdsDxe: starting";
pub(super) const BOOTS_FILE: &str = "guesthouse-boots";
pub(super) const SERIAL_FILE: &str = "serial.log";
const SHARE_SCRIPT_SECONDS: u64 = 40;
const DESKTOP_ATTEMPTS: u32 = 60;
const DESKTOP_POLL: Duration = Duration::from_secs(4);
const DESKTOP_SETTLE: Duration = Duration::from_secs(8);
const DESKTOP_ANSWER_SECONDS: u64 = 6;
/// The processes that mean a user's session is really up.
pub(super) const DESKTOP_SCRIPT: &str =
    "(Get-Process explorer,ctfmon,RuntimeBroker -ea 0 | Select -Unique Name).Count";
const DESKTOP_PROCESSES: u32 = 3;
/// How sethz.sh says this can't be done here, which another try won't change.
const REFRESH_CANNOT: i32 = 3;
const REFRESH_REFUSAL: &str = "cannot set the rate: ";

/// How many times the guest has booted in this VM run.
pub fn boots(context: &Context, slot: Option<Slot>) -> usize {
    let data = fs::read(context.run_folder(slot).join(SERIAL_FILE)).unwrap_or_default();
    count(&data, BOOT_LINE)
}

pub(super) fn count(data: &[u8], wanted: &[u8]) -> usize {
    data.windows(wanted.len()).filter(|part| *part == wanted).count()
}

/// Whether Windows has restarted since the app last set the rate, which only lasts one boot. A VM
/// the app never set up has no record here, and is left as it is.
pub fn rebooted(context: &Context, slot: Option<Slot>) -> bool {
    let noted = fs::read_to_string(context.run_folder(slot).join(BOOTS_FILE)).unwrap_or_default();
    noted.trim().parse::<usize>().is_ok_and(|noted| boots(context, slot) > noted)
}

pub(super) fn note_boot(task: &Task) -> Result<()> {
    fs::write(task.run_file(BOOTS_FILE), boots(task.context, None).to_string())?;
    Ok(())
}

/// Asks the logged-in session, because Windows can stretch an old size over the screen.
fn desktop_size(task: &Task) -> Result<Option<String>> {
    let output = task.host.helper(Helper::DesktopSize)?;
    let rest = output.text.split("screen ").nth(1).unwrap_or("");
    let size: String = rest.chars().take_while(|letter| letter.is_ascii_digit() || *letter == 'x').collect();
    Ok(size.contains('x').then_some(size))
}

fn apply_screen(task: &mut Task) -> Result<()> {
    let wanted = task.machine.screen.clone();
    task.step("Checking the desktop size");
    if desktop_size(task)?.as_deref() == Some(&wanted) {
        return Ok(());
    }
    task.step(&format!("Setting the desktop to {wanted}"));
    task.host.helper(Helper::SetDesktopMode)?;
    let size = desktop_size(task)?;
    if size.as_deref() != Some(&wanted) {
        let said = size.as_deref().unwrap_or("an unknown size");
        task.error = format!("The desktop is {said}, not {wanted}.");
    }
    Ok(())
}

/// Success is the helper's line saying what the rate is now.
pub fn apply_refresh(task: &mut Task) -> Result<()> {
    let hz = task.machine.refresh_hz;
    task.step(&format!("Setting {hz} Hz"));
    for _ in 0..2 {
        let output = task.host.helper(Helper::SetRefresh(hz))?;
        if output.text.contains(&format!("now {hz}/1")) {
            return Ok(());
        }
        if output.code == REFRESH_CANNOT {
            let reason = output.text.lines().rev().find_map(|line| line.split(REFRESH_REFUSAL).nth(1));
            let reason = reason.map_or("The reason is in the log.", str::trim);
            task.error =
                format!("The rate was not set to {hz} Hz: {reason} The VM works, at the rate it had.");
            return Ok(());
        }
    }
    task.error = format!("Could not set {hz} Hz. The guest stays at its old rate until this works.");
    Ok(())
}

pub fn apply_share(task: &mut Task) -> Result<()> {
    if !task.machine.share.is_empty() {
        task.step("Connecting the shared folder");
        let script = guest_script(task.context, "share-start.ps1")?;
        if !task.ask_guest(&script, SHARE_SCRIPT_SECONDS).replace("not mapped", "").contains("mapped") {
            task.error = "The shared folder did not reach the guest.".to_owned();
        }
    }
    Ok(())
}

/// Waits until the guest's desktop is really up: its processes running and a screen in the session.
pub(super) fn wait_for_desktop(task: &Task) -> bool {
    for _ in 0..DESKTOP_ATTEMPTS {
        task.sleep(DESKTOP_POLL);
        let said = task.ask_guest(DESKTOP_SCRIPT, DESKTOP_ANSWER_SECONDS);
        let count = said.lines().find_map(|line| line.trim().parse::<u32>().ok());
        if count.is_some_and(|count| count >= DESKTOP_PROCESSES) {
            task.sleep(DESKTOP_SETTLE);
            return true;
        }
    }
    false
}

pub(super) fn set_up_desktop(task: &mut Task) -> Result<()> {
    note_boot(task)?;
    // The size comes first, as a new mode may well reset the rate.
    apply_screen(task)?;
    apply_refresh(task)?;
    apply_share(task)
}
