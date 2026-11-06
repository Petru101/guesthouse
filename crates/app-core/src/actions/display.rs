//! Windows' screen: the display driver going in, and the app's settings reaching it. Nothing here can
//! stop a machine. If the driver won't come up, Windows goes on with the basic display and the row says why.

use super::Task;
use super::desktop::{GUEST_FOLDER, GUEST_USER, SessionScript, apply_screen, put_file, run_in_session_text};
use crate::agent::guest_script;
use crate::display::{
    DisplayState, Driver, DriverPackage, PACKAGE_FILES, RunningScreen, SUPPORT_FILES, read_state, ring_in,
    screen_shows_hdr, write_state,
};
use crate::failure::Result;
use crate::log::write_log;
use crate::machine::REFRESH_RATES;
use guesthouse_daemon::display_note::ScreenNote;
use std::time::Duration;

const APPLY_SCRIPT: &str = include_str!("../../data/apply-display.ps1");
const APPLY: SessionScript = SessionScript { name: "apply-display", patience: 45, answer_seconds: 55 };
const REMOVE_SCRIPT: &str = "display-remove.ps1";
const REMOVE_SECONDS: u64 = 60;
const FOLDER_SECONDS: u64 = 10;
/// Signing and installing took about 25 seconds in a guest, and this is what the agent gets.
const INSTALL_SECONDS: u64 = 110;
const TAKE_OVER_SECONDS: u64 = 20;
const CHECK_SECONDS: u64 = 15;
/// From a mode change or a logon to the driver's ring being up.
const RING_WAIT: Duration = Duration::from_secs(40);
const RING_POLL: Duration = Duration::from_secs(1);
/// What the logon task uses if the machine's own rate isn't one the driver has.
const SAFE_HZ: u32 = 60;
const LOGON_TASK: &str = "guesthouse-display";
const HARDWARE_ID: &str = r"Root\GuesthouseDisplay";
/// The name the INF gives the device, which the install prints beside its state.
const HARDWARE_NAME: &str = "RDPUDD Chained DD";
const DEVICE_MARK: &str = "device ";
const WORKING: &str = "working";
const PLACED_MARK: &str = "placed";
const INSTALLED_LINE_START: &str = "OK ";
const INSTALLED_LINE_END: &str = r"ROOT\DISPLAY\";
const UPDATED_MARK: &str = "update True";

const NO_PACKAGE: &str = "This build has no display driver, so Windows stays on its basic display, at about 64 Hz \
                          with no HDR.";
const DID_NOT_INSTALL: &str = "The display driver did not install, so Windows is on its basic display. The log has \
                               what Windows said.";
const NOT_REACHED: &str = "The display driver's files did not reach Windows, so it is on its basic display.";
const NO_FRAMES: &str = "The display driver installed but sent no picture, so it was taken out and Windows is on \
                         its basic display.";
const NOT_UP: &str =
    "The display driver did not come up, so it was taken out and Windows is on its basic display.";
const NO_HDR_SCREEN: &str = "This screen can't show HDR, so Windows stays in SDR.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Device {
    Missing,
    Working,
    Broken,
}

/// What this machine's settings ask of Windows now.
struct Wanted {
    width: u32,
    height: u32,
    hz: u32,
    hdr: bool,
}

impl Wanted {
    fn of(task: &Task) -> Self {
        let machine = &task.machine;
        let (width, height) = machine.screen.split_once('x').unwrap_or_default();
        Self {
            width: width.parse().unwrap_or_default(),
            height: height.parse().unwrap_or_default(),
            hz: machine.refresh_hz,
            hdr: machine.hdr && screen_shows_hdr(task.context),
        }
    }

    fn matches(&self, ring: ScreenNote) -> bool {
        (ring.width, ring.height, ring.hdr) == (self.width, self.height, self.hdr)
    }

    /// What couldn't be done, in words.
    fn shortfall(&self, machine_hdr: bool, screen: RunningScreen) -> String {
        if (screen.width, screen.height, screen.hz) != (self.width, self.height, self.hz) {
            let (width, height, hz) = (self.width, self.height, self.hz);
            return format!(
                "Windows has no mode at {width}x{height} and {hz} Hz, so it's at {}x{} and {} Hz.",
                screen.width, screen.height, screen.hz
            );
        }
        if machine_hdr && !self.hdr { NO_HDR_SCREEN.to_owned() } else { String::new() }
    }
}

fn guest_file(name: &str) -> String {
    format!("{GUEST_FOLDER}\\idd\\{name}")
}

/// Goes by the hardware id, as any other indirect display would have the same kind of device name.
fn device(task: &Task) -> Option<Device> {
    let script = format!(
        "$ours = @(Get-PnpDevice -Class Display -ea 0 | Where-Object {{ \
         (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_HardwareIds -ea 0).Data \
         -contains '{HARDWARE_ID}' }}); \
         if (-not $ours) {{ '{DEVICE_MARK}missing' }} \
         elseif ($ours[0].Status -eq 'OK') {{ '{DEVICE_MARK}{WORKING}' }} else {{ '{DEVICE_MARK}broken' }}"
    );
    let said = task.ask_guest(&script, CHECK_SECONDS);
    match said.split(DEVICE_MARK).nth(1)?.trim() {
        WORKING => Some(Device::Working),
        "missing" => Some(Device::Missing),
        _ => Some(Device::Broken),
    }
}

/// Says whether everything got there.
fn send_package(task: &Task, package: &DriverPackage) -> Result<bool> {
    let folder = guest_file("pkg");
    let made = format!("New-Item -ItemType Directory -Force {folder} | Out-Null; '{PLACED_MARK}'");
    let mut placed = task.ask_guest(&made, FOLDER_SECONDS).contains(PLACED_MARK);
    for name in PACKAGE_FILES {
        placed &= put_file(task, &package.folder.join(name), &format!("{folder}\\{name}"))?;
    }
    for name in SUPPORT_FILES {
        placed &= put_file(task, &package.folder.join(name), &guest_file(name))?;
    }
    Ok(placed)
}

fn went_in(said: &str) -> bool {
    let device_is_fine = |line: &str| {
        line.starts_with(INSTALLED_LINE_START)
            && line.contains(HARDWARE_NAME)
            && line.contains(INSTALLED_LINE_END)
    };
    said.contains(UPDATED_MARK) && said.lines().any(|line| device_is_fine(line.trim()))
}

/// Signs the driver on this machine, installs it, and leaves a task that makes it the display at logon.
/// A lost upload changes nothing here: whatever is in Windows goes on working, and the next boot tries.
fn install(task: &Task, package: &DriverPackage, state: &mut DisplayState, found: Device) -> Result<()> {
    task.step("Installing the display driver");
    if !send_package(task, package)? {
        write_log(task.context, "display driver: the files did not all arrive")?;
        if found != Device::Working {
            state.reason = NOT_REACHED.to_owned();
        }
        return Ok(());
    }
    let folder = guest_file("").trim_end_matches('\\').to_owned();
    let said = task.ask_guest(&format!("& {} {folder}", guest_file("sign-and-install.ps1")), INSTALL_SECONDS);
    write_log(task.context, &format!("display driver: {}", said.trim()))?;
    if !went_in(&said) {
        take_out(task)?;
        state.failed_package.clone_from(&package.id);
        return fail(task, state, DID_NOT_INSTALL, "the install did not finish");
    }
    let wanted = Wanted::of(task);
    let hz = if REFRESH_RATES.contains(&wanted.hz) { wanted.hz } else { SAFE_HZ };
    let (width, height) = (wanted.width, wanted.height);
    let script = format!(
        "& {} {folder} {width} {height} {hz} {GUEST_USER}; Start-ScheduledTask {LOGON_TASK}",
        guest_file("install-take-over.ps1")
    );
    task.ask_guest(&script, TAKE_OVER_SECONDS);
    state.driver = Driver::Installed;
    state.package.clone_from(&package.id);
    state.failed_package.clear();
    state.reason.clear();
    Ok(())
}

fn fail(task: &Task, state: &mut DisplayState, reason: &str, detail: &str) -> Result<()> {
    write_log(task.context, &format!("display driver: {detail}"))?;
    state.driver = Driver::Failed;
    state.package.clear();
    state.reason = reason.to_owned();
    state.running = None;
    Ok(())
}

fn take_out(task: &Task) -> Result<()> {
    task.step("Taking the display driver out");
    let said = task.ask_guest(&guest_script(task.context, REMOVE_SCRIPT)?, REMOVE_SECONDS);
    write_log(task.context, &format!("display driver: {}", said.trim()))?;
    Ok(())
}

/// Sees that the driver is in, if it can be. `again` is someone asking for another try.
fn make_sure_of_driver(task: &Task, state: &mut DisplayState, again: bool) -> Result<()> {
    task.step("Checking the display driver");
    let Some(found) = device(task) else {
        write_log(task.context, "display driver: the agent did not say how the device is")?;
        return Ok(());
    };
    let package = DriverPackage::find(task.context);
    if found == Device::Working {
        state.driver = Driver::Installed;
        state.reason.clear();
    }
    let Some(package) = package else {
        if found != Device::Working {
            state.driver = Driver::Absent;
            state.reason = NO_PACKAGE.to_owned();
        }
        return Ok(());
    };
    let wanted = match found {
        Device::Working => state.package != package.id && state.failed_package != package.id,
        _ => again || state.failed_package != package.id,
    };
    if wanted {
        install(task, &package, state, found)?;
    }
    Ok(())
}

/// `expected` says the driver is in, so Windows gets a moment to show its monitor.
fn tell_windows(task: &Task, wanted: &Wanted, expected: bool) -> Result<Option<RunningScreen>> {
    let Wanted { width, height, hz, hdr } = *wanted;
    let (hdr, expected) = (u8::from(hdr), u8::from(expected));
    let script = format!(
        "$width = {width}\n$height = {height}\n$hz = {hz}\n$hdr = {hdr}\n$expected = {expected}\n{APPLY_SCRIPT}"
    );
    let said = run_in_session_text(task, APPLY, &script)?;
    write_log(task.context, &format!("display: {}", said.trim()))?;
    Ok(RunningScreen::parse(&said))
}

fn ring(task: &Task) -> Option<ScreenNote> {
    ring_in(&task.context.run_folder(None))
}

/// Waits for the frames to say what Windows says, and returns what's there at the end of the wait.
fn wait_for_ring(task: &Task, wanted: &Wanted) -> Option<ScreenNote> {
    let end = task.now() + RING_WAIT;
    while task.now() < end {
        if ring(task).is_some_and(|found| wanted.matches(found)) {
            break;
        }
        task.sleep(RING_POLL);
    }
    ring(task)
}

/// Windows has gone back to the basic display, or never left it.
fn settle_on_basic(task: &mut Task, state: &mut DisplayState, wanted: &Wanted) -> Result<()> {
    apply_screen(task)?;
    state.running = tell_windows(task, wanted, false)?;
    state.note.clear();
    Ok(())
}

fn give_up(task: &mut Task, state: &mut DisplayState, wanted: &Wanted, reason: &str) -> Result<()> {
    take_out(task)?;
    state.failed_package = std::mem::take(&mut state.package);
    fail(task, state, reason, "taken out again, as it never came up")?;
    settle_on_basic(task, state, wanted)
}

fn apply(task: &mut Task, state: &mut DisplayState) -> Result<()> {
    let wanted = Wanted::of(task);
    let Wanted { width, height, hz, .. } = wanted;
    task.step(&format!("Setting the display to {width}x{height} at {hz} Hz"));
    let expected = state.driver == Driver::Installed;
    let Some(screen) = tell_windows(task, &wanted, expected)? else {
        write_log(task.context, "display: Windows did not say what its screen is")?;
        state.running = None;
        return Ok(());
    };
    if !screen.on_driver {
        return if expected {
            give_up(task, state, &wanted, NOT_UP)
        } else {
            settle_on_basic(task, state, &wanted)
        };
    }
    let Some(found) = wait_for_ring(task, &wanted) else {
        return give_up(task, state, &wanted, NO_FRAMES);
    };
    write_log(task.context, &format!("display: frames are {found:?}"))?;
    state.running = Some(screen);
    state.note = wanted.shortfall(task.machine.hdr, screen);
    Ok(())
}

fn save(task: &Task, state: &DisplayState) -> Result<()> {
    write_state(task.context, &task.machine, state)
}

/// What each boot does.
pub(super) fn set_up_display(task: &mut Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    state.running = None;
    save(task, &state)?;
    make_sure_of_driver(task, &mut state, false)?;
    save(task, &state)?;
    apply(task, &mut state)?;
    save(task, &state)
}

/// The settings have changed while the machine is up.
pub(super) fn apply_display(task: &mut Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    apply(task, &mut state)?;
    save(task, &state)
}

/// Someone has asked for the driver, after it failed.
pub(super) fn install_display(task: &mut Task) -> Result<()> {
    let mut state = read_state(task.context, &task.machine);
    make_sure_of_driver(task, &mut state, true)?;
    save(task, &state)?;
    apply(task, &mut state)?;
    save(task, &state)
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod tests;
