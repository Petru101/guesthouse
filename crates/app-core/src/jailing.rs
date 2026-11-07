//! Whether a GPU machine starts jailed, and what its row says about that.
//!
//! The jail with no namespaces, Landlock and seccomp, needs nothing an unprivileged user lacks on
//! the distros named in the README, so without it a start is refused, unless the user has allowed
//! that machine to start with no jail. bubblewrap's namespaces go around QEMU as well where the PC
//! allows them, and a PC that doesn't still starts, with a line on the row. Nothing starts unjailed
//! without the row saying so.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::linux::forwarding::sandbox_runs;
use crate::machine::Machine;
use crate::network::Mode;
use crate::vm_args::{QemuJail, VmPlan};
use guesthouse_jail::Support;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const LAUNCHER: &str = "guesthouse-jail";
/// The row's line about the jail, in the run folder. Empty or missing is a whole jail.
const NOTE_FILE: &str = "jail-note";
const CHECK_SECONDS: &str = "10";
/// QEMU only knows `-sandbox` if it was built with seccomp, and it stops at `-version` either way.
const SANDBOX_QUESTION: [&str; 3] = ["-sandbox", "on", "-version"];
const SOUND_SOCKET: &str = "pipewire-0";
const SOUND_FOLDER_VARIABLES: [&str; 2] = ["PIPEWIRE_RUNTIME_DIR", "XDG_RUNTIME_DIR"];
/// Landlock's ABI that can hold pathname sockets, such as the session bus, out of reach.
const SOCKETS_BY_PATH_ABI: u32 = 9;
const ALLOW_HINT: &str = "To start this machine anyway, turn on \"Start without a jail\" in its settings, and it will say so while it runs.";

/// What this PC offers a jail, found at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub support: Support,
    pub namespaces: bool,
    pub qemu_sandbox: bool,
    pub sound_socket: Option<PathBuf>,
}

impl Offer {
    /// A PC with nothing to offer: no Landlock, no seccomp, no namespaces.
    pub fn nothing() -> Self {
        Self {
            support: Support { landlock_abi: 0, seccomp: false },
            namespaces: false,
            qemu_sandbox: false,
            sound_socket: None,
        }
    }

    /// Asks this PC. Takes a moment, as it runs bubblewrap and QEMU once each.
    pub fn here(plan: &VmPlan) -> Self {
        Self {
            support: Support::probe(),
            namespaces: sandbox_runs(),
            qemu_sandbox: has_sandbox(&plan.qemu),
            sound_socket: plan.sound.as_ref().and_then(|_| sound_socket()),
        }
    }
}

fn has_sandbox(qemu: &Path) -> bool {
    let mut question = Command::new("timeout");
    question.arg(CHECK_SECONDS).arg(qemu).args(SANDBOX_QUESTION);
    let quiet = question.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    quiet.status().is_ok_and(|status| status.success())
}

fn sound_socket() -> Option<PathBuf> {
    let folder = SOUND_FOLDER_VARIABLES
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))?;
    Some(Path::new(&folder).join(SOUND_SOCKET)).filter(|socket| socket.exists())
}

/// What a start does about the jail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub qemu: Option<QemuJail>,
    pub daemon_jailed: bool,
    /// The row's line, empty for a whole jail.
    pub note: String,
}

fn without_namespaces(plan: &VmPlan, landlock_abi: u32) -> String {
    let why = if plan.network.mode == Mode::Bridged {
        "a bridged network's helper can't work inside them"
    } else {
        "bubblewrap can't make them on this PC"
    };
    let mut note = format!("Jailed, without namespaces: {why}.");
    // Namespaces hide the session bus and the rest. Without them, only a newer Landlock can, or
    // QEMU getting its sockets from the launcher, which it can't with sound.
    if landlock_abi < SOCKETS_BY_PATH_ABI && plan.sound.is_some() {
        note += " This kernel can't keep QEMU from your desktop's sockets without them.";
    }
    note
}

/// Decides, or refuses the start with the reason and what to do about it.
pub fn choose(context: &Context, machine: &Machine, plan: &VmPlan, offer: &Offer) -> Result<Choice> {
    if let Some(reason) = offer.support.refusal() {
        if !machine.start_without_jail {
            return Err(Failure(format!(
                "The VM did not start, as it can't be jailed: {reason} {ALLOW_HINT}"
            )));
        }
        return Ok(Choice { qemu: None, daemon_jailed: false, note: format!("Not jailed: {reason}") });
    }
    let jail = QemuJail {
        launcher: context.paths.binary(LAUNCHER),
        namespaces: offer.namespaces,
        qemu_sandbox: offer.qemu_sandbox,
        sound_socket: offer.sound_socket.clone(),
    };
    let note = if plan.in_namespaces(&jail) {
        String::new()
    } else {
        without_namespaces(plan, offer.support.landlock_abi)
    };
    Ok(Choice { qemu: Some(jail), daemon_jailed: true, note })
}

pub fn write_note(run_folder: &Path, note: &str) -> std::io::Result<()> {
    fs::write(run_folder.join(NOTE_FILE), note)
}

/// The row's line, if the running machine's jail has one.
pub fn read_note(run_folder: &Path) -> Option<String> {
    let note = fs::read_to_string(run_folder.join(NOTE_FILE)).ok()?;
    Some(note.trim().to_owned()).filter(|note| !note.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_args::Boot;

    fn offer(landlock_abi: u32, namespaces: bool) -> Offer {
        Offer {
            support: Support { landlock_abi, seccomp: true },
            namespaces,
            qemu_sandbox: true,
            sound_socket: None,
        }
    }

    fn choose_for(machine: &Machine, offer: &Offer) -> Result<Choice> {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let plan = VmPlan::new(&context, machine, Boot::Gpu, slots::first(), 0);
        choose(&context, machine, &plan, offer)
    }

    fn windows() -> Machine {
        Machine::new("win", "Win", "/m/disk.qcow2", "/m/vars.fd")
    }

    #[test]
    fn a_whole_jail_has_nothing_to_say() {
        let choice = choose_for(&windows(), &offer(10, true)).unwrap();
        assert!(choice.daemon_jailed && choice.qemu.as_ref().unwrap().namespaces);
        assert_eq!(choice.note, "");
        assert!(choice.qemu.unwrap().launcher.ends_with(LAUNCHER));
    }

    #[test]
    fn no_landlock_refuses_and_says_how_to_start_anyway() {
        let refused = choose_for(&windows(), &offer(0, true)).unwrap_err();
        assert!(
            refused.0.contains("Landlock") && refused.0.contains("Start without a jail"),
            "{}",
            refused.0
        );
        let allowed = Machine { start_without_jail: true, ..windows() };
        let choice = choose_for(&allowed, &offer(0, true)).unwrap();
        assert_eq!((choice.qemu, choice.daemon_jailed), (None, false));
        assert!(choice.note.starts_with("Not jailed: "), "{}", choice.note);
    }

    #[test]
    fn no_namespaces_still_starts_and_says_so() {
        let choice = choose_for(&windows(), &offer(10, false)).unwrap();
        assert!(choice.qemu.is_some());
        assert_eq!(choice.note, "Jailed, without namespaces: bubblewrap can't make them on this PC.");
        let quiet = choose_for(&windows(), &offer(4, false)).unwrap();
        assert!(
            !quiet.note.contains("desktop's sockets"),
            "with no sound QEMU makes no sockets: {}",
            quiet.note
        );
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let plan = VmPlan::new(&context, &windows(), Boot::Gpu, slots::first(), 0);
        let with_sound = VmPlan { sound: Some(crate::vm_args::Sound { sink: None }), ..plan };
        let loud = choose(&context, &windows(), &with_sound, &offer(4, false)).unwrap();
        assert!(loud.note.contains("desktop's sockets"), "{}", loud.note);
    }

    #[test]
    fn a_bridged_machine_says_why_it_has_no_namespaces() {
        let bridged = Machine { network_mode: Mode::Bridged, ..windows() };
        let choice = choose_for(&bridged, &offer(10, true)).unwrap();
        assert!(choice.note.contains("bridged"), "{}", choice.note);
    }

    #[test]
    fn the_note_is_read_back_and_an_empty_one_is_none() {
        let scratch = Scratch::new();
        let folder = scratch.path("run");
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(read_note(&folder), None);
        write_note(&folder, "").unwrap();
        assert_eq!(read_note(&folder), None);
        write_note(&folder, "Not jailed: no Landlock").unwrap();
        assert_eq!(read_note(&folder).as_deref(), Some("Not jailed: no Landlock"));
    }
}
