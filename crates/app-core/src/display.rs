//! What a Windows machine's screen really is, which can be less than what was asked for. Tasks write
//! it down in the machine's folder, as it has to outlive a VM, and the app reads it from there.

use crate::context::Context;
use crate::daemon::is_own_daemon;
use crate::failure::Result;
use crate::machine::Machine;
use crate::settings::{read_prefs, write_json};
use guesthouse_daemon::display_note::{ScreenNote, read_note};
use guesthouse_daemon::slot::Slot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "display.json";
const SCREEN_LINE: &str = "screen ";
const ON_THE_DRIVER: &str = "ours";
/// What the app found out about the host's screen, as only it can. A task has no display.
pub const SCREEN_HDR_PREF: &str = "screen_hdr";

/// The package's id, written by the packaging, which tells one build of the driver from another.
const PACKAGE_ID_FILE: &str = "package.txt";
/// What goes in the folder the driver is signed from, and what goes in `pkg` inside it. The catalog
/// names every file in `pkg`, so nothing else goes there.
pub const PACKAGE_FILES: [&str; 2] = ["GuesthouseDisplay.dll", "GuesthouseDisplay.inf"];
/// The licences go along with the driver, wherever it goes.
pub const SUPPORT_FILES: [&str; 6] = [
    "vdd_settings.xml",
    "sign-and-install.ps1",
    "install-take-over.ps1",
    "take-over-display.ps1",
    "LICENSE",
    "LICENSE-MS-PL",
];

/// How the display driver stands on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    #[default]
    Absent,
    Installed,
    /// It went wrong and was taken out again. A start leaves it until there's another build to try.
    Failed,
}

/// What Windows said it's showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningScreen {
    pub on_driver: bool,
    pub width: u32,
    pub height: u32,
    pub hz: u32,
    pub hdr: bool,
}

impl RunningScreen {
    /// Reads the last line of `apply-display.ps1`: `screen ours 1920x1080 240 hdr 0`.
    pub fn parse(said: &str) -> Option<Self> {
        let line = said.lines().rev().find_map(|line| line.trim().strip_prefix(SCREEN_LINE))?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let [display, size, hz, "hdr", hdr] = words[..] else { return None };
        let (width, height) = size.split_once('x')?;
        Some(Self {
            on_driver: display == ON_THE_DRIVER,
            width: width.parse().ok()?,
            height: height.parse().ok()?,
            hz: hz.parse().ok()?,
            hdr: hdr == "1",
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayState {
    pub driver: Driver,
    /// The build that went into Windows. Empty until one really has.
    pub package: String,
    /// The build that failed, which a start doesn't try again.
    #[serde(default)]
    pub failed_package: String,
    /// Why Windows is on the basic display.
    pub reason: String,
    /// What couldn't be done of what was asked for.
    pub note: String,
    pub running: Option<RunningScreen>,
    /// This build has sent a picture, so it's worth waiting for.
    #[serde(default)]
    pub shown_frames: bool,
    /// Boots in a row with no picture, from a build that has sent one before.
    #[serde(default)]
    pub blank_boots: u32,
}

impl DisplayState {
    /// What the user should hear, if anything. A note is about the screen as it is, so it's only
    /// for a machine that's up.
    pub fn problem(&self, up: bool) -> &str {
        if self.reason.is_empty() && up { &self.note } else { &self.reason }
    }
}

fn state_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(STATE_FILE)
}

/// A file that's missing or bad says nothing is known, and a task will look again.
pub fn read_state(context: &Context, machine: &Machine) -> DisplayState {
    let text = fs::read_to_string(state_file(context, machine)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_state(context: &Context, machine: &Machine, state: &DisplayState) -> Result<()> {
    write_json(&state_file(context, machine), state)
}

/// The settings have changed, so what was said about the old ones no longer holds.
pub fn forget_note(context: &Context, machine: &Machine) -> Result<()> {
    let mut state = read_state(context, machine);
    state.note.clear();
    write_state(context, machine, &state)
}

/// The screen in words, for a machine that's up. The daemon knows the size and the look for certain,
/// as it has the frames, and only Windows knows the rate.
pub fn screen_text(state: &DisplayState, ring: Option<ScreenNote>) -> String {
    let Some(screen) = state.running else { return "display not known yet".to_owned() };
    if !screen.on_driver {
        return format!("{}x{} at {} Hz on the basic display", screen.width, screen.height, screen.hz);
    }
    let Some(ring) = ring else { return "no picture from the display driver".to_owned() };
    let hdr = if ring.hdr { ", HDR" } else { "" };
    format!("{}x{} at {} Hz{hdr}", ring.width, ring.height, screen.hz)
}

/// Whether the host's screen can show HDR. Anything short of a yes keeps Windows in SDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenHdr {
    Shows,
    Cannot,
    CantTell,
}

impl ScreenHdr {
    const WORDS: [(Self, &'static str); 3] =
        [(Self::Shows, "shows"), (Self::Cannot, "cannot"), (Self::CantTell, "cant-tell")];

    pub fn word(self) -> &'static str {
        Self::WORDS.iter().find(|(answer, _)| *answer == self).map_or("", |(_, word)| word)
    }

    /// Why Windows stays in SDR, if it does.
    pub fn sdr_reason(self) -> Option<&'static str> {
        match self {
            Self::Shows => None,
            Self::Cannot => Some("This screen can't show HDR, so Windows stays in SDR."),
            Self::CantTell => {
                Some("Guesthouse can't tell if this desktop's screen shows HDR, so Windows stays in SDR.")
            }
        }
    }
}

/// What the app last found out about this PC's screen.
pub fn screen_hdr(context: &Context) -> ScreenHdr {
    let prefs = read_prefs(context);
    let word = prefs.get(SCREEN_HDR_PREF).and_then(Value::as_str);
    ScreenHdr::WORDS
        .iter()
        .find(|(_, known)| Some(*known) == word)
        .map_or(ScreenHdr::CantTell, |found| found.0)
}

/// What the daemon says is on its ring, if that daemon is still there to say it.
pub fn ring_in(context: &Context, slot: Option<Slot>) -> Option<ScreenNote> {
    let note = read_note(&context.run_folder(slot))?;
    is_own_daemon(context, &note.daemon.to_string(), slot.unwrap_or(context.slot)).then_some(note)
}

fn is_readable(file: &Path) -> bool {
    file.is_file() && fs::File::open(file).is_ok()
}

/// A build of the display driver, as the packaging leaves it in the program folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverPackage {
    pub folder: PathBuf,
    pub id: String,
}

impl DriverPackage {
    /// `None` unless all of it is there. A build from source has none, and Windows keeps the basic display.
    pub fn find(context: &Context) -> Option<Self> {
        let folder = context.paths.display_driver();
        let all_there =
            PACKAGE_FILES.iter().chain(&SUPPORT_FILES).all(|name| is_readable(&folder.join(name)));
        let id = fs::read_to_string(folder.join(PACKAGE_ID_FILE)).ok()?.trim().to_owned();
        (all_there && !id.is_empty()).then_some(Self { folder, id })
    }
}

#[cfg(test)]
pub(crate) mod stand_in {
    use super::*;
    use crate::context::SLOT_VARIABLE;
    use crate::daemon::DAEMON_PROGRAM;
    use crate::processes::stand_in::fake_process;
    use crate::testing::Scratch;

    /// A daemon for the context's own slot.
    pub fn fake_daemon(context: &Context, pid: u32) {
        let base = context.paths.runtime.display().to_string();
        let slot = format!("{SLOT_VARIABLE}={}", context.slot.number());
        fake_process(context, pid, &[DAEMON_PROGRAM, &base], &[&slot], "");
    }

    /// A package in the scratch program folder.
    pub fn fake_package(scratch: &Scratch, id: &str) {
        for name in PACKAGE_FILES.iter().chain(&SUPPORT_FILES) {
            scratch.write(&format!("program/display-driver/{name}"), "part of the driver");
        }
        scratch.write(&format!("program/display-driver/{PACKAGE_ID_FILE}"), id);
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::{fake_daemon, fake_package};
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use guesthouse_daemon::display_note::write_note;

    const ON_DRIVER: RunningScreen =
        RunningScreen { on_driver: true, width: 1920, height: 1080, hz: 240, hdr: false };

    #[test]
    fn windows_answer_is_read_from_its_last_line() {
        assert_eq!(
            RunningScreen::parse("made it primary\r\nscreen ours 1920x1080 240 hdr 0\r\n"),
            Some(ON_DRIVER)
        );
        let basic = RunningScreen::parse("screen basic 1920x1080 64 hdr 0").unwrap();
        assert!(!basic.on_driver);
        assert_eq!(basic.hz, 64);
        assert!(RunningScreen::parse("screen ours 3840x2160 120 hdr 1").unwrap().hdr);
    }

    #[test]
    fn an_answer_that_makes_no_sense_is_none() {
        for said in ["", "screen", "screen ours 1920 240 hdr 0", "screen ours 1920x1080 fast hdr 0"] {
            assert_eq!(RunningScreen::parse(said), None, "{said:?}");
        }
    }

    #[test]
    fn the_size_and_the_look_come_from_the_frames_and_the_rate_from_windows() {
        let state = DisplayState { running: Some(ON_DRIVER), ..DisplayState::default() };
        let ring = ScreenNote { width: 3840, height: 2160, hdr: true, daemon: 7 };
        assert_eq!(screen_text(&state, Some(ring)), "3840x2160 at 240 Hz, HDR");
        assert_eq!(screen_text(&state, None), "no picture from the display driver");
    }

    #[test]
    fn the_basic_display_is_named_and_a_rate_nobody_got_is_never_shown() {
        let running = Some(RunningScreen { on_driver: false, hz: 64, ..ON_DRIVER });
        let state = DisplayState { running, ..DisplayState::default() };
        assert_eq!(screen_text(&state, None), "1920x1080 at 64 Hz on the basic display");
        assert_eq!(screen_text(&DisplayState::default(), None), "display not known yet");
    }

    #[test]
    fn a_note_is_only_believed_while_its_daemon_is_alive() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::create_dir_all(context.run_folder(None)).unwrap();
        let note = ScreenNote { width: 1920, height: 1080, hdr: false, daemon: 77 };
        write_note(&context.run_folder(None), Some(note)).unwrap();
        assert_eq!(ring_in(&context, None), None, "that daemon has gone");
        fake_daemon(&context, 77);
        assert_eq!(ring_in(&context, None), Some(note));
    }

    #[test]
    fn a_reason_comes_before_a_note() {
        let state = DisplayState { reason: "why".into(), note: "also".into(), ..DisplayState::default() };
        assert_eq!(state.problem(true), "why");
        let state = DisplayState { note: "also".into(), ..DisplayState::default() };
        assert_eq!(state.problem(true), "also");
        assert_eq!(state.problem(false), "");
    }

    #[test]
    fn a_bad_file_counts_as_nothing_known() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let machine = Machine::new("x", "x", "", "");
        fs::create_dir_all(machine.folder(&context)).unwrap();
        fs::write(state_file(&context, &machine), "{").unwrap();
        assert_eq!(read_state(&context, &machine), DisplayState::default());
    }

    #[test]
    fn a_package_needs_all_its_files_licences_included() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(DriverPackage::find(&context), None);
        fake_package(&scratch, "build 1");
        assert_eq!(DriverPackage::find(&context).unwrap().id, "build 1");
        fs::remove_file(context.paths.display_driver().join("LICENSE-MS-PL")).unwrap();
        assert_eq!(DriverPackage::find(&context), None);
    }
}
