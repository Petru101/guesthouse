//! Puts the driver's files in an existing guest's system folders. The driver's own folder reaches the
//! guest at install, but a program like a benchmark's launcher looks in System32 by name.

use super::Task;
use super::desktop::{GUEST_FOLDER, put_guest_file};
use crate::failure::Result;
use crate::log::write_log;
use crate::setup::guest_files::guest_file_list;
use crate::text::base64_encode;

const SCRIPT_NAME: &str = "nv-files.ps1";
const LIST_NAME: &str = "nv-files.txt";
const LOG_NAME: &str = "nv-files.log";
const CHECK_SECONDS: u64 = 20;
const PLACE_SECONDS: u64 = 110;
const MISSING_MARK: &str = "missing ";
const FAILED_MARK: &str = "FAILED";

fn guest_path(name: &str) -> String {
    format!("{GUEST_FOLDER}\\{name}")
}

fn send_list(task: &Task, list: &str) {
    let data = base64_encode(list.as_bytes());
    let path = guest_path(LIST_NAME);
    task.ask_guest(
        &format!("[IO.File]::WriteAllBytes('{path}', [Convert]::FromBase64String('{data}'))"),
        CHECK_SECONDS,
    );
}

/// The guest counts for itself, as only it knows what is in its folders.
fn count_missing(task: &Task) -> Option<u32> {
    let list = guest_path(LIST_NAME);
    let script = format!(
        "$count = 0; foreach ($line in Get-Content -LiteralPath '{list}') {{ \
         $folder, $name, $target = $line -split \"`t\"; \
         if (-not (Test-Path -LiteralPath (Join-Path (Join-Path $env:windir $folder) $target))) {{ $count++ }} }}; \
         \"{MISSING_MARK}$count\""
    );
    let said = task.ask_guest(&script, CHECK_SECONDS);
    said.split(MISSING_MARK).nth(1)?.trim().parse().ok()
}

fn run_placing_script(task: &Task) -> String {
    let (script, list, log) = (guest_path(SCRIPT_NAME), guest_path(LIST_NAME), guest_path(LOG_NAME));
    task.ask_guest(&format!("& {script} {list} | Tee-Object {log}"), PLACE_SECONDS)
}

/// Windows is up by now, so anything that goes wrong here is said and doesn't stop the machine.
pub(super) fn place_driver_files(task: &mut Task) -> Result<()> {
    let list = match guest_file_list(task.context) {
        Ok(list) => list,
        Err(failure) => {
            write_log(task.context, &format!("driver files: {}", failure.0))?;
            return Ok(());
        }
    };
    task.step("Checking the driver's files");
    send_list(task, &list);
    if count_missing(task).is_none_or(|count| count == 0) {
        return Ok(());
    }
    task.step("Placing the driver's files");
    put_guest_file(task, SCRIPT_NAME, &guest_path(SCRIPT_NAME))?;
    let said = run_placing_script(task);
    write_log(task.context, &format!("driver files: {}", said.trim()))?;
    if said.contains(FAILED_MARK) {
        task.error = "Some of the driver's files did not reach Windows. The log says which.".to_owned();
    }
    Ok(())
}
