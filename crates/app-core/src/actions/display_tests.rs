use super::*;
use crate::actions::tests::{FakeClock, machine, plain_task_context};
use crate::agent::GuestAgent;
use crate::context::Context;
use crate::display::stand_in::{fake_daemon, fake_package};
use crate::display::{SCREEN_HDR_PREF, read_state};
use crate::machine::Machine;
use crate::settings::write_prefs;
use crate::testing::Scratch;
use crate::text::base64_decode;
use crate::vm_host::stand_in::RecordingHost;
use guesthouse_daemon::display_note::write_note;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const DAEMON_PID: u32 = 77;
const INSTALLED: &str = "Added driver packages:  1\nupdate True error 0 reboot False\n\
                         OK CM_PROB_NONE RDPUDD Chained DD ROOT\\DISPLAY\\0000\n";
const REFUSED: &str = "Added driver packages:  0\nupdate False error 5 reboot False\n";
const ON_DRIVER: &str = "screen ours 1920x1080 144 hdr 0\n";
const ON_BASIC: &str = "screen basic 1920x1080 64 hdr 0\n";

/// A guest that answers by what it's asked, so a test can say how Windows behaves.
struct Guest {
    run_folder: PathBuf,
    device: Mutex<&'static str>,
    install: &'static str,
    screen: Mutex<&'static str>,
    /// Whether frames come once the driver has been told to take over.
    sends_frames: AtomicBool,
    /// Files sent to the guest get lost on the way.
    loses_uploads: AtomicBool,
    asked: Mutex<Vec<String>>,
}

impl Guest {
    fn new(context: &Context, device: &'static str, install: &'static str, sends_frames: bool) -> Arc<Self> {
        Arc::new(Self {
            run_folder: context.run_folder(None),
            device: Mutex::new(device),
            install,
            screen: Mutex::new(if device == "working" { ON_DRIVER } else { ON_BASIC }),
            sends_frames: AtomicBool::new(sends_frames),
            loses_uploads: AtomicBool::new(false),
            asked: Mutex::default(),
        })
    }

    fn asked_for(&self, words: &str) -> usize {
        self.asked.lock().unwrap().iter().filter(|script| script.contains(words)).count()
    }

    fn show_frames(&self) {
        let note = ScreenNote { width: 1920, height: 1080, hdr: false, daemon: DAEMON_PID };
        let shown = self.sends_frames.load(Ordering::Relaxed).then_some(note);
        write_note(&self.run_folder, shown).unwrap();
    }
}

impl GuestAgent for Guest {
    fn ask(&self, _port: u16, _token: &str, script: &str, _seconds: u64) -> String {
        self.asked.lock().unwrap().push(script.to_owned());
        if script.contains("idd\\") && self.loses_uploads.load(Ordering::Relaxed) {
            return String::new();
        }
        if script.contains("WriteAllBytes") {
            let sent = script.split('\'').nth(3).and_then(base64_decode).unwrap_or_default();
            self.asked.lock().unwrap().push(String::from_utf8_lossy(&sent).into_owned());
            "file placed".to_owned()
        } else if script.contains("New-Item") {
            "placed".to_owned()
        } else if script.contains("DEVPKEY") {
            format!("device {}", self.device.lock().unwrap())
        } else if script.contains("sign-and-install") {
            if self.install == INSTALLED {
                *self.device.lock().unwrap() = "working";
            }
            self.install.to_owned()
        } else if script.contains("install-take-over") {
            *self.screen.lock().unwrap() = ON_DRIVER;
            self.show_frames();
            String::new()
        } else if script.contains("take it out") {
            *self.device.lock().unwrap() = "missing";
            *self.screen.lock().unwrap() = ON_BASIC;
            "removed".to_owned()
        } else if script.contains("ghapply-display") {
            self.show_frames();
            (*self.screen.lock().unwrap()).to_owned()
        } else if script.contains("ghdisplay") {
            "screen 1920x1080".to_owned()
        } else {
            String::new()
        }
    }
}

/// What a boot does, as the tests run it.
fn boot_display(task: &mut Task) -> Result<()> {
    set_up_display(task);
    Ok(())
}

fn apply_settings(task: &mut Task) -> Result<()> {
    apply_display(task);
    Ok(())
}

fn install_again(task: &mut Task) -> Result<()> {
    install_display(task);
    Ok(())
}

struct Rig {
    scratch: Scratch,
    context: Context,
    machine: Machine,
}

impl Rig {
    fn new() -> Self {
        let scratch = Scratch::new();
        let context = plain_task_context(&scratch);
        scratch.write("program/guest/display.ps1", "what the session sees");
        scratch.write("program/guest/display-remove.ps1", "take it out");
        let machine = machine(&context);
        crate::agent::make_token(&context, &machine).unwrap();
        fs::create_dir_all(context.run_folder(None)).unwrap();
        fake_daemon(&context, DAEMON_PID);
        Self { scratch, context, machine }
    }

    fn with(mut self, guest: impl FnOnce(&Context) -> Arc<Guest>) -> (Self, Arc<Guest>) {
        let guest = guest(&self.context);
        self.context.agent = guest.clone();
        (self, guest)
    }

    fn say_this_build_is_in(&self, id: &str) {
        let state =
            DisplayState { driver: Driver::Installed, package: id.to_owned(), ..DisplayState::default() };
        write_state(&self.context, &self.machine, &state).unwrap();
    }

    fn run(&self, action: fn(&mut Task) -> Result<()>) -> DisplayState {
        let host = RecordingHost::default();
        let mut task = Task::new(&self.context, &host, "start", self.machine.clone());
        action(&mut task).unwrap();
        read_state(&self.context, &self.machine)
    }
}

#[test]
fn a_machine_with_no_driver_gets_it_and_ends_up_on_our_display() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", INSTALLED, true));
    fake_package(&rig.scratch, "build 1");
    let state = rig.run(boot_display);
    assert_eq!(state.driver, Driver::Installed);
    assert_eq!(state.package, "build 1");
    assert_eq!(state.reason, "");
    assert!(state.running.unwrap().on_driver);
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 1);
    assert_eq!(guest.asked_for("C:\\gpupv\\idd\\LICENSE-MS-PL"), 1, "the licences go with it");
    assert_eq!(guest.asked_for("C:\\gpupv\\idd\\LICENSE'"), 1);
    assert_eq!(guest.asked_for("C:\\gpupv\\idd\\pkg\\LICENSE"), 0, "the catalog covers all of pkg");
}

#[test]
fn a_build_with_no_driver_stays_on_the_basic_display_and_says_so() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", INSTALLED, true));
    let state = rig.run(boot_display);
    assert_eq!(state.driver, Driver::Absent);
    assert!(state.reason.contains("no display driver"));
    assert!(!state.running.unwrap().on_driver);
    assert_eq!(guest.asked_for("sign-and-install"), 0);
}

#[test]
fn an_install_that_fails_leaves_the_basic_display_and_the_reason() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", REFUSED, true));
    fake_package(&rig.scratch, "build 1");
    let state = rig.run(boot_display);
    assert_eq!(state.driver, Driver::Failed);
    assert!(state.reason.contains("did not install"));
    assert!(!state.running.unwrap().on_driver);
    assert_eq!(guest.asked_for("take it out"), 1);
}

#[test]
fn a_build_that_failed_isnt_tried_at_every_start_but_can_be_asked_for() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", REFUSED, true));
    fake_package(&rig.scratch, "build 1");
    rig.run(boot_display);
    rig.run(boot_display);
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 1);
    rig.run(install_again);
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 2);
    fake_package(&rig.scratch, "build 2");
    rig.run(boot_display);
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 3, "a new build gets its own try");
}

#[test]
fn a_driver_that_sends_no_picture_is_taken_out_so_the_vm_stays_usable() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", INSTALLED, false));
    fake_package(&rig.scratch, "build 1");
    let state = rig.run(boot_display);
    assert_eq!(state.driver, Driver::Failed);
    assert!(state.reason.contains("sent no picture"));
    assert_eq!(guest.asked_for("take it out"), 1);
    assert!(!state.running.unwrap().on_driver);
}

#[test]
fn a_driver_that_was_in_and_doesnt_come_up_is_said_too() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    *guest.screen.lock().unwrap() = ON_BASIC;
    let state = rig.run(boot_display);
    assert!(state.reason.contains("did not come up"));
    assert_eq!(guest.asked_for("take it out"), 1);
}

#[test]
fn a_working_driver_is_left_alone_and_a_newer_build_goes_on_it() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    fake_package(&rig.scratch, "build 1");
    rig.say_this_build_is_in("build 1");
    rig.run(boot_display);
    assert_eq!(guest.asked_for("sign-and-install"), 0);
    fake_package(&rig.scratch, "build 2");
    assert_eq!(rig.run(boot_display).package, "build 2");
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 1);
}

#[test]
fn hdr_only_reaches_windows_on_a_screen_that_can_show_it() {
    let (mut rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    rig.machine.hdr = true;
    let state = rig.run(apply_settings);
    assert_eq!(guest.asked_for("$hdr = 0"), 1);
    assert_eq!(guest.asked_for("$hdr = 1"), 0);
    assert!(state.note.contains("can't show HDR"), "{}", state.note);
    let shows = json!({SCREEN_HDR_PREF: true});
    write_prefs(&rig.context, shows.as_object().unwrap().clone()).unwrap();
    rig.run(apply_settings);
    assert_eq!(guest.asked_for("$hdr = 1"), 1);
}

#[test]
fn what_windows_really_gave_is_kept_and_the_gap_is_said() {
    let (mut rig, _guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    rig.machine.refresh_hz = 200;
    let state = rig.run(boot_display);
    assert_eq!(state.running.unwrap().hz, 144);
    assert!(state.note.contains("no mode at 1920x1080 and 200 Hz"), "{}", state.note);
}

#[test]
fn a_screen_with_nothing_said_isnt_guessed_at() {
    let clock = Arc::new(FakeClock::default());
    let (mut rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    rig.context.clock = clock;
    *guest.screen.lock().unwrap() = "";
    let state = rig.run(apply_settings);
    assert_eq!(state.running, None);
    assert_eq!(guest.asked_for("take it out"), 0);
}

#[test]
fn a_lost_upload_leaves_a_working_driver_as_it_is_and_the_next_boot_tries_again() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    rig.say_this_build_is_in("build 1");
    fake_package(&rig.scratch, "build 2");
    guest.loses_uploads.store(true, Ordering::Relaxed);
    let state = rig.run(boot_display);
    assert_eq!(
        (state.driver, state.package.as_str(), state.reason.as_str()),
        (Driver::Installed, "build 1", "")
    );
    assert!(state.running.unwrap().on_driver);
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 0);
    guest.loses_uploads.store(false, Ordering::Relaxed);
    assert_eq!(rig.run(boot_display).package, "build 2");
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 1);
}

#[test]
fn a_lost_upload_on_a_new_machine_is_said_and_tried_again() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", INSTALLED, true));
    fake_package(&rig.scratch, "build 1");
    guest.loses_uploads.store(true, Ordering::Relaxed);
    let state = rig.run(boot_display);
    assert!(state.reason.contains("did not reach"), "{}", state.reason);
    assert_eq!(state.package, "");
    guest.loses_uploads.store(false, Ordering::Relaxed);
    assert_eq!(rig.run(boot_display).driver, Driver::Installed);
}

#[test]
fn a_settings_change_that_stalls_never_costs_the_driver() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, false));
    rig.say_this_build_is_in("build 1");
    let state = rig.run(apply_settings);
    assert_eq!(guest.asked_for("take it out"), 0);
    assert_eq!(state.driver, Driver::Installed);
    assert_eq!(state.reason, "");
    assert!(state.note.contains("hasn't confirmed"), "{}", state.note);
    *guest.screen.lock().unwrap() = ON_BASIC;
    let state = rig.run(apply_settings);
    assert_eq!(guest.asked_for("take it out"), 0, "not even if Windows says it's on the basic display");
    assert_eq!(state.driver, Driver::Installed);
}

#[test]
fn a_driver_that_has_shown_frames_only_goes_after_two_blank_boots_in_a_row() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    rig.run(boot_display);
    guest.sends_frames.store(false, Ordering::Relaxed);
    let state = rig.run(boot_display);
    assert_eq!(guest.asked_for("take it out"), 0, "one slow boot can't cost the driver");
    assert_eq!(state.driver, Driver::Installed);
    assert!(state.note.contains("no picture"), "{}", state.note);
    guest.sends_frames.store(true, Ordering::Relaxed);
    rig.run(boot_display);
    guest.sends_frames.store(false, Ordering::Relaxed);
    rig.run(boot_display);
    assert_eq!(guest.asked_for("take it out"), 0, "a good boot starts the count again");
    let state = rig.run(boot_display);
    assert_eq!(guest.asked_for("take it out"), 1, "a machine can't stay black for good");
    assert_eq!(state.driver, Driver::Failed);
}

#[test]
fn a_stopped_gpu_is_no_reason_to_give_up_on_the_driver() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, false));
    guesthouse_daemon::gpu_note::write_note(&rig.context.run_folder(None), true).unwrap();
    for _ in 0..3 {
        assert_eq!(rig.run(boot_display).driver, Driver::Installed);
    }
    assert_eq!(guest.asked_for("take it out"), 0);
}

#[test]
fn take_over_at_install_is_left_to_the_script_that_sets_the_mode() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", INSTALLED, true));
    fake_package(&rig.scratch, "build 1");
    rig.run(boot_display);
    assert_eq!(guest.asked_for("Start-ScheduledTask guesthouse-display"), 0);
}

#[test]
fn a_driver_file_that_cant_be_read_is_no_package_at_all() {
    let (rig, guest) = Rig::new().with(|context| Guest::new(context, "missing", INSTALLED, true));
    fake_package(&rig.scratch, "build 1");
    let file = rig.context.paths.display_driver().join("GuesthouseDisplay.dll");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    let state = rig.run(boot_display);
    assert!(state.reason.contains("no display driver"), "{}", state.reason);
    assert_eq!(guest.asked_for("sign-and-install.ps1 C:"), 0);
}

#[test]
fn a_state_that_cant_be_saved_doesnt_stop_the_machine_and_is_said() {
    let (rig, _guest) = Rig::new().with(|context| Guest::new(context, "working", INSTALLED, true));
    fs::create_dir_all(rig.machine.folder(&rig.context).join("display.json.new")).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    boot_display(&mut task).unwrap();
    assert!(task.error.contains("display"), "{}", task.error);
}
