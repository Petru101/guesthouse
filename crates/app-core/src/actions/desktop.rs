//! The guest's desktop once it's up: the boots seen so far, its size on the basic display and its shared
//! folder.

use super::Task;
use super::display::set_up_display;
use crate::agent::guest_script;
use crate::context::Context;
use crate::failure::Result;
use crate::text::base64_encode;
use guesthouse_daemon::slot::Slot;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// The firmware writes one of these to the serial log each boot.
pub(super) const BOOT_LINE: &[u8] = b"BdsDxe: starting";
pub(super) const BOOTS_FILE: &str = "guesthouse-boots";
pub(super) const SERIAL_FILE: &str = "serial.log";
const SHARE_SCRIPT_SECONDS: u64 = 40;
/// Where the guest keeps what the app puts there, and the account its desktop runs as.
pub(super) const GUEST_FOLDER: &str = "C:\\gpupv";
pub(super) const GUEST_USER: &str = "gpupv";
const UPLOAD_SECONDS: u64 = 60;
const PUT_MARK: &str = "file placed";

/// A script for the user's session: its name, and how many seconds it gets.
#[derive(Clone, Copy)]
pub(super) struct SessionScript {
    pub name: &'static str,
    pub patience: u32,
    pub answer_seconds: u64,
}

const DISPLAY_SCRIPT: SessionScript = SessionScript { name: "display", patience: 25, answer_seconds: 40 };
const SET_MODE_SCRIPT: SessionScript = SessionScript { name: "setmode", patience: 40, answer_seconds: 55 };
const DESKTOP_ATTEMPTS: u32 = 60;
const DESKTOP_POLL: Duration = Duration::from_secs(4);
const DESKTOP_SETTLE: Duration = Duration::from_secs(8);
const DESKTOP_ANSWER_SECONDS: u64 = 6;
/// The processes that mean a user's session is really up.
pub(super) const DESKTOP_SCRIPT: &str =
    "(Get-Process explorer,ctfmon,RuntimeBroker -ea 0 | Select -Unique Name).Count";
const DESKTOP_PROCESSES: u32 = 3;

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

/// Copies a file into the guest, and says whether it got there.
pub(super) fn put_file(task: &Task, local: &Path, guest: &str) -> Result<bool> {
    let data = base64_encode(&fs::read(local)?);
    let script =
        format!("[IO.File]::WriteAllBytes('{guest}', [Convert]::FromBase64String('{data}')); '{PUT_MARK}'");
    Ok(task.ask_guest(&script, UPLOAD_SECONDS).contains(PUT_MARK))
}

/// Copies a file from the program folder into the guest.
pub(super) fn put_guest_file(task: &Task, local: &str, guest: &str) -> Result<()> {
    put_file(task, &task.context.paths.guest_files().join(local), guest)?;
    Ok(())
}

/// Runs a script this app holds as the logged-in user.
pub(super) fn run_in_session_text(task: &Task, script: SessionScript, text: &str) -> Result<String> {
    let data = base64_encode(text.as_bytes());
    let file = format!("{GUEST_FOLDER}\\{}.ps1", script.name);
    task.ask_guest(
        &format!("[IO.File]::WriteAllBytes('{file}', [Convert]::FromBase64String('{data}'))"),
        UPLOAD_SECONDS,
    );
    run_placed_script(task, script)
}

/// Runs one of the app's scripts as the logged-in user, as session 0 has no display, and returns what
/// it wrote. It's run as a scheduled task, which is the agent's way into that session.
fn run_in_session(task: &Task, script: SessionScript) -> Result<String> {
    let file = format!("{GUEST_FOLDER}\\{}.ps1", script.name);
    put_guest_file(task, &format!("{}.ps1", script.name), &file)?;
    run_placed_script(task, script)
}

/// The script is in the guest's folder by now.
fn run_placed_script(task: &Task, script: SessionScript) -> Result<String> {
    let SessionScript { name, patience, answer_seconds } = script;
    let (file, result) = (format!("{GUEST_FOLDER}\\{name}.ps1"), format!("{GUEST_FOLDER}\\cap\\{name}.txt"));
    let arguments = format!("-NoProfile -ExecutionPolicy Bypass -File {file}");
    let run = format!(
        "Remove-Item {result} -ea 0; \
         $a=New-ScheduledTaskAction -Execute powershell.exe -Argument \"{arguments}\"; \
         $p=New-ScheduledTaskPrincipal -UserId {GUEST_USER} -LogonType Interactive; \
         Register-ScheduledTask -TaskName gh{name} -Action $a -Principal $p -Force | Out-Null; \
         Start-ScheduledTask gh{name}; $t=0; \
         while(-not (Test-Path {result}) -and $t -lt {patience}){{Start-Sleep 1;$t++}}; \
         Unregister-ScheduledTask gh{name} -Confirm:$false; Get-Content {result}"
    );
    Ok(task.ask_guest(&run, answer_seconds))
}

/// Asks the logged-in session, because Windows can stretch an old size over the screen.
fn desktop_size(task: &Task) -> Result<Option<String>> {
    let said = run_in_session(task, DISPLAY_SCRIPT)?;
    let rest = said.split("screen ").nth(1).unwrap_or("");
    let size: String = rest.chars().take_while(|letter| letter.is_ascii_digit() || *letter == 'x').collect();
    Ok(size.contains('x').then_some(size))
}

/// For the basic display, whose size comes from the VGA memory.
pub(super) fn apply_screen(task: &mut Task) -> Result<()> {
    let wanted = task.machine.basic_screen().to_owned();
    task.step("Checking the desktop size");
    if desktop_size(task)?.as_deref() == Some(&wanted) {
        return Ok(());
    }
    task.step(&format!("Setting the desktop to {wanted}"));
    run_in_session(task, SET_MODE_SCRIPT)?;
    let size = desktop_size(task)?;
    if size.as_deref() != Some(&wanted) {
        let said = size.as_deref().unwrap_or("an unknown size");
        task.error = format!("The desktop is {said}, not {wanted}.");
    }
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
    set_up_display(task);
    apply_share(task)
}
