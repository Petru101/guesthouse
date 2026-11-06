//! Installing Windows: the answer disc, the wait for setup, and the check of what it made.

use super::desktop::{BOOT_LINE, SERIAL_FILE, count, wait_for_desktop};
use super::{KEY_POLL, Task, plan_for, start_if_there_is_room};
use crate::agent::{QUICK_SECONDS, guest_script, make_token};
use crate::context::Context;
use crate::disk_image::disk_size_gib;
use crate::failure::{Failure, Result};
use crate::iso::iso_maker;
use crate::library::make_disk;
use crate::machine::Machine;
use crate::monitor::Command as Qmp;
use crate::settings::driver_folder;
use crate::start_checks::start_problems;
use crate::vm_args::Boot;
use crate::vm_host::{last_words, send_key};
use crate::zones;
use serde_json::json;
use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(super) const INSTALL_FILES: [&str; 4] =
    ["autounattend.xml", "SetupComplete.cmd", "setup.ps1", "agent.ps1"];
const INSTALL_HOURS: u64 = 2;
/// Setup writes all the time until the desktop, so a disk that stops growing means it's waiting for
/// someone, or stuck. The longest quiet stretch in a normal install is well under this.
const INSTALL_QUIET_MINUTES: u64 = 10;
const BOOT_PROMPT_SECONDS: u64 = 6;
pub const INSTALL_STEPS: [&str; 4] = [
    "Copying Windows to the disk",
    "Windows is setting up its devices",
    "Windows is getting ready",
    "Windows is finishing its setup",
];
/// The firmware names it twice a boot.
const WINDOWS_BOOT_LINE: &[u8] = b"\"Windows Boot Manager\"";
const FIRMWARE_WAIT: Duration = Duration::from_secs(30);
const SETUP_SCRIPT_SECONDS: u64 = 30;
const CHECK_SCRIPT_SECONDS: u64 = 60;
const INSTALL_STOP_SECONDS: u64 = 120;
const SETUP_POLL: Duration = Duration::from_secs(5);
const SHORT_POLL: Duration = Duration::from_millis(200);
/// The agent's file and the time zone both go through these.
const ANSWER_DISC: &str = "answer.iso";
const ANSWER_LABEL: &str = "GPUPV_SETUP";
const SETMODE_FILE: &str = "setmode.ps1";
const TOKEN_ON_DISC: &str = "token.txt";

/// The answer file for a host in this IANA zone. The template says UTC.
pub fn answer_file(context: &Context, zone: &str) -> Result<String> {
    let name = zones::windows_zone(zone);
    Ok(guest_script(context, "autounattend.xml")?.replace("<TimeZone>UTC<", &format!("<TimeZone>{name}<")))
}

pub fn answer_disc_path(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(ANSWER_DISC)
}

/// Everything Windows setup needs from here: the answer file, what it runs, and the driver's files.
/// It holds the agent's token, so it's for this user only, and it goes once the install is done.
pub fn make_answer_disc(context: &Context, machine: &Machine) -> Result<()> {
    let files = machine.folder(context).join("answer-disc.new");
    let _ = fs::remove_dir_all(&files);
    fs::create_dir_all(&files)?;
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700))?;
    let built = fill_answer_disc(context, machine, &files);
    let _ = fs::remove_dir_all(&files);
    built
}

fn fill_answer_disc(context: &Context, machine: &Machine, files: &Path) -> Result<()> {
    for name in INSTALL_FILES {
        fs::copy(context.paths.guest_files().join(name), files.join(name))?;
    }
    fs::copy(context.paths.guest_files().join(SETMODE_FILE), files.join(SETMODE_FILE))?;
    fs::write(
        files.join("autounattend.xml"),
        answer_file(context, &zones::host_zone(zones::HOST_ZONE_LINK))?,
    )?;
    fs::write(files.join(TOKEN_ON_DISC), make_token(context, machine)? + "\n")?;
    let path = answer_disc_path(context, machine);
    // Made private first: the disc holds the token, and genisoimage keeps a file's mode.
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    let result = Command::new(iso_maker())
        .args(["-quiet", "-J", "-joliet-long", "-r", "-V", ANSWER_LABEL, "-graft-points", "-o"])
        .arg(&path)
        .arg(format!("/={}", files.display()))
        .arg(format!("nvidia/={}", driver_folder(context).display()))
        .output()?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        let said = said.trim();
        let tail = said.char_indices().rev().nth(199).map_or(0, |found| found.0);
        return Err(Failure(format!("The install disc could not be made: {}", &said[tail..])));
    }
    Ok(())
}

fn serial_log(task: &Task) -> Vec<u8> {
    fs::read(task.run_file(SERIAL_FILE)).unwrap_or_default()
}

/// A Windows disc only boots if a key is pressed in its first seconds, and starting the same way
/// later would begin the install again. Setup's own screens must not get these keys.
fn pass_boot_prompt(task: &Task) {
    let end = task.now() + FIRMWARE_WAIT;
    while count(&serial_log(task), BOOT_LINE) == 0 && task.now() < end {
        task.sleep(SHORT_POLL);
    }
    let end = task.now() + Duration::from_secs(BOOT_PROMPT_SECONDS);
    while task.now() < end {
        send_key(task.context, "spc");
        task.sleep(KEY_POLL);
    }
}

/// What Windows setup is doing, from how often it has started from the disk.
pub fn install_step(serial: &[u8]) -> &'static str {
    let started = count(serial, WINDOWS_BOOT_LINE);
    INSTALL_STEPS[(started / 2).min(INSTALL_STEPS.len() - 1)]
}

fn disk_written(machine: &Machine) -> u64 {
    fs::metadata(&machine.disk).map_or(0, |found| found.blocks())
}

/// Waits for the agent, and then for the user account, which Windows makes late.
pub fn wait_for_setup(task: &mut Task) -> Result<()> {
    let finish = guest_script(task.context, "finish.ps1")?;
    let quiet = Duration::from_secs(INSTALL_QUIET_MINUTES * 60);
    let end = task.now() + Duration::from_secs(INSTALL_HOURS * 3600);
    let mut step = "";
    let (mut grew_at, mut written) = (task.now(), 0);
    while task.now() < end {
        if task.pid().is_none() {
            let words = last_words(task.context);
            return Err(Failure(format!("The VM stopped during the install. {words}").trim().to_owned()));
        }
        let now_written = disk_written(&task.machine);
        if now_written > written {
            (grew_at, written) = (task.now(), now_written);
        }
        if task.now().saturating_sub(grew_at) > quiet {
            let file = task.run_file("stuck-install.ppm");
            let command = Qmp::new("screendump", json!({"filename": file}));
            task.context.ask_monitor(None, &command);
            return Err(Failure(format!(
                "Windows Setup has not written to the disk for {INSTALL_QUIET_MINUTES} minutes, so it is \
                 stuck or waiting for an answer. The VM is still up: Show Screen has what it says."
            )));
        }
        let current = install_step(&serial_log(task));
        if current != step {
            step = current;
            task.step(step);
        }
        if task.ask_guest(&finish, SETUP_SCRIPT_SECONDS).contains("finished") {
            return Ok(());
        }
        task.sleep(SETUP_POLL);
    }
    Err(Failure(format!(
        "Windows setup did not finish in {INSTALL_HOURS} hours. Show Screen says how far it got."
    )))
}

fn count_files(folder: &Path) -> usize {
    let Ok(entries) = fs::read_dir(folder) else { return 0 };
    entries.flatten().map(|entry| if entry.path().is_dir() { count_files(&entry.path()) } else { 1 }).sum()
}

/// Looks at the finished guest, so that anything missing is found here and not at first boot.
fn check_guest(task: &Task) -> Result<()> {
    task.step("Checking Windows");
    let expected = count_files(&driver_folder(task.context));
    let script = format!("$expected = {expected}\n{}", guest_script(task.context, "check.ps1")?);
    let answer = task.ask_guest(&script, CHECK_SCRIPT_SECONDS);
    if !answer.contains("all there") {
        let said = answer.trim();
        let tail = said.char_indices().rev().nth(299).map_or(0, |found| found.0);
        let said = if said.is_empty() { "the agent went quiet" } else { &said[tail..] };
        return Err(Failure(format!("Windows is missing something: {said}")));
    }
    Ok(())
}

/// Takes a new machine from its ISO to the first boot with its GPU. Each run starts again from
/// nothing, as a half-finished Windows isn't worth keeping.
pub fn install(task: &mut Task) -> Result<()> {
    let context = task.context;
    if !task.machine.owns_files(context) {
        return Err(Failure::new(
            "This machine's disk is not in its folder, so nothing will install over it.",
        ));
    }
    let mut found: Vec<String> = start_problems(context, &task.machine)?;
    found.retain(|problem| !problem.contains("is missing"));
    if !Path::new(&task.machine.iso).exists() {
        found.push(format!("{} is missing.", task.machine.iso));
    }
    if let Some(problem) = found.into_iter().next() {
        return Err(Failure(problem));
    }
    task.step("Preparing the install disc");
    // The answer file counts on it, and all that was here goes anyway.
    task.machine.tpm = true;
    task.machine.save(context)?;
    let size = disk_size_gib(&task.machine.disk)? as u32;
    make_answer_disc(context, &task.machine)?;
    fs::remove_file(&task.machine.disk)?;
    make_disk(context, &task.machine, size)?;
    task.step("Starting the installer");
    let answer = answer_disc_path(context, &task.machine);
    let boot = Boot::Install { iso: task.machine.iso.clone(), answer: answer.clone() };
    let plan = plan_for(context, &task.machine, boot);
    task.host.start(&plan, &task.machine)?;
    pass_boot_prompt(task);
    wait_for_setup(task)?;
    task.step("Waiting for the desktop");
    if !wait_for_desktop(task) {
        return Err(Failure::new("Windows installed, but its desktop never came up."));
    }
    check_guest(task)?;
    task.step("Shutting down Windows");
    task.ask_guest("Stop-Computer -Force", QUICK_SECONDS);
    let end = task.now() + Duration::from_secs(INSTALL_STOP_SECONDS);
    while task.pid().is_some() && task.now() < end {
        task.sleep(KEY_POLL);
    }
    if task.pid().is_some() {
        return Err(Failure::new("Windows is installed but would not shut down. Try Force Off, then Start."));
    }
    fs::remove_file(&answer)?;
    task.machine.installing = false;
    task.machine.iso.clear();
    task.machine.save(context)?;
    if context.second_instance() { Ok(()) } else { start_if_there_is_room(task, "Windows") }
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
