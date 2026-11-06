//! Looks at this PC. Nothing here changes anything.

use super::cards::{Support, find_cards};
use crate::context::Context;
use crate::host::{GIB, IO_SPACE_GIB, TMPFS_SPARE_GIB, free_gib};
use crate::library::INSTALL_ROOM_GIB;
use crate::linux::driver_versions::{branch_refusal, untested_warning};
use crate::machine::MIN_RAM_GIB;
use crate::usb::sysfs_text;
use crate::vm_args::on_path;
use rustix::fs::{Access, access, statvfs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `nvidia-smi` gives the card's clocks to the record builder.
const TOOLS: [&str; 5] = ["7z", "genisoimage", "qemu-img", "swtpm", "nvidia-smi"];
/// What an Ubuntu guest's install takes, beyond what any machine does.
const UBUNTU_TOOLS: [&str; 6] = ["isoinfo", "genisoimage", "openssl", "ssh-keygen", "setarch", "bwrap"];
const TOOLS_FIX: &str = "On Fedora: sudo dnf install 7zip genisoimage qemu-img swtpm edk2-ovmf, and nvidia-smi from NVIDIA's driver";
const BUILD_FIX: &str = "qemu/README.md in Guesthouse's source says how to build it.";
const QEMU_LIBRARIES_FIX: &str = "On Fedora: sudo dnf install pixman libslirp libfdt libusb1 pipewire-libs";
const CARD_FIX: &str = "GPU-PV needs an RTX 5090, or another consumer RTX 50-series card.";
/// The Linux driver this has been run on. The guest's graphics depend on the exact version.
pub const TESTED_DRIVER: &str = "615.78.08";
/// What setup keeps of the installer, and room to swap it for a new copy.
pub const DRIVER_ROOM_GIB: u64 = 4;
/// What a new Windows machine gets by default.
pub const WANTED_SHM_GIB: u64 = 16;
const QEMU_CHECK_SECONDS: &str = "10";
const MOST_QEMU_WORDS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Ok,
    Warning,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub title: String,
    pub state: CheckState,
    pub text: String,
    pub fix: String,
}

impl Check {
    fn new(title: &str, state: CheckState, text: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { title: title.to_owned(), state, text: text.into(), fix: fix.into() }
    }

    fn ok(title: &str, text: impl Into<String>) -> Self {
        Self::new(title, CheckState::Ok, text, "")
    }
}

fn check_card(context: &Context) -> Check {
    const TITLE: &str = "NVIDIA card";
    let cards = find_cards(context);
    let Some(card) = cards.first() else {
        return Check::new(TITLE, CheckState::Failed, "No NVIDIA card was found.", CARD_FIX);
    };
    match card.support {
        Support::Tested => Check::ok(TITLE, format!("{}. Tested.", card.name)),
        Support::Experimental => Check::new(
            TITLE,
            CheckState::Warning,
            format!("{} ({}). Experimental.", card.name, card.chip),
            "Only the RTX 5090 has been tested, so this card may not work.",
        ),
        Support::Unsupported => {
            Check::new(TITLE, CheckState::Failed, format!("{}. Not supported.", card.name), CARD_FIX)
        }
    }
}

/// Says if the host's driver isn't the one this was tested on.
pub fn driver_warning(context: &Context) -> Option<String> {
    let version = sysfs_text(&context.driver_version_file);
    if version.is_empty() {
        return Some("The NVIDIA driver does not seem to be loaded on this PC.".to_owned());
    }
    (version != TESTED_DRIVER).then(|| {
        format!(
            "This PC has NVIDIA driver {version}, and the VM was tested on {TESTED_DRIVER}. \
             The guest's graphics depend on the exact version, so they may not work."
        )
    })
}

fn check_host_driver(context: &Context) -> Check {
    const TITLE: &str = "Host NVIDIA driver";
    let version = sysfs_text(&context.driver_version_file);
    if version.is_empty() {
        let fix = format!("Install NVIDIA's driver {TESTED_DRIVER} for Linux and start this PC again.");
        return Check::new(TITLE, CheckState::Failed, "The NVIDIA driver is not loaded.", fix);
    }
    if version == TESTED_DRIVER {
        return Check::ok(TITLE, format!("{version}. Tested."));
    }
    Check::new(
        TITLE,
        CheckState::Warning,
        format!("{version}, and this was tested on {TESTED_DRIVER}."),
        format!("The guest's graphics depend on the exact version. Install {TESTED_DRIVER} if they fail."),
    )
}

fn check_kvm(context: &Context) -> Check {
    const TITLE: &str = "Virtualization";
    let kvm = context.kvm.display();
    if !context.kvm.exists() {
        let fix = "Turn on SVM or VT-x in this PC's firmware settings.";
        return Check::new(TITLE, CheckState::Failed, format!("{kvm} does not exist."), fix);
    }
    if access(&context.kvm, Access::READ_OK | Access::WRITE_OK).is_err() {
        let fix = "Run sudo usermod -aG kvm $USER, then log in again.";
        return Check::new(TITLE, CheckState::Failed, format!("This user cannot open {kvm}."), fix);
    }
    Check::ok(TITLE, format!("{kvm} can be opened."))
}

fn check_qemu(context: &Context) -> Check {
    const TITLE: &str = "Patched QEMU";
    let qemu = &context.paths.qemu;
    let shown = qemu.display();
    if access(qemu, Access::EXEC_OK).is_err() {
        return Check::new(TITLE, CheckState::Failed, format!("{shown} is missing."), BUILD_FIX);
    }
    let said = Command::new("timeout")
        .arg(QEMU_CHECK_SECONDS)
        .arg(qemu)
        .arg("-version")
        .stdin(Stdio::null())
        .output();
    let said = match said {
        Ok(said) => said,
        Err(error) => {
            return Check::new(
                TITLE,
                CheckState::Failed,
                format!("{shown} did not run: {error}"),
                QEMU_LIBRARIES_FIX,
            );
        }
    };
    if said.status.success() {
        return Check::ok(TITLE, shown.to_string());
    }
    let error = String::from_utf8_lossy(&said.stderr);
    let error = error.trim();
    let start = error.char_indices().rev().nth(MOST_QEMU_WORDS - 1).map_or(0, |found| found.0);
    let text = if error.is_empty() { format!("{shown} did not run.") } else { error[start..].to_owned() };
    Check::new(TITLE, CheckState::Failed, text, QEMU_LIBRARIES_FIX)
}

fn check_tools(context: &Context, tools: &[&str]) -> Check {
    const TITLE: &str = "Tools";
    let mut missing: Vec<&str> = tools.iter().copied().filter(|tool| !on_path(tool)).collect();
    let firmware = &context.paths.firmware;
    if ![&firmware.vars, &firmware.vars_secure, &firmware.code, &firmware.code_secure]
        .iter()
        .all(|path| path.exists())
    {
        missing.push("OVMF firmware");
    }
    if !missing.is_empty() {
        return Check::new(TITLE, CheckState::Failed, format!("Missing: {}.", missing.join(", ")), TOOLS_FIX);
    }
    Check::ok(TITLE, format!("{} and the OVMF firmware are here.", tools.join(", ")))
}

/// An Ubuntu guest boots whatever this says, so nothing here is a failure: at worst its GPU stays off.
fn check_ubuntu(context: &Context, tools: &[&str]) -> Check {
    const TITLE: &str = "Ubuntu guests";
    let missing: Vec<&str> = tools.iter().copied().filter(|tool| !on_path(tool)).collect();
    if !missing.is_empty() {
        let text = format!("Missing: {}.", missing.join(", "));
        return Check::new(
            TITLE,
            CheckState::Warning,
            text,
            "On Fedora: sudo dnf install genisoimage openssl openssh-clients",
        );
    }
    let version = sysfs_text(&context.driver_version_file);
    let refused = if version.is_empty() {
        Some("The NVIDIA driver is not loaded.".to_owned())
    } else {
        branch_refusal(&version)
    };
    if let Some(reason) = refused {
        return Check::new(TITLE, CheckState::Warning, "They boot, with no GPU.", reason);
    }
    let text = "They install by themselves. The GPU is a preview.";
    match untested_warning(&version) {
        Some(warning) => Check::new(TITLE, CheckState::Warning, text, warning),
        None => Check::ok(TITLE, text),
    }
}

/// Judges shared memory of this size in GiB. It counts the whole size, as a running VM has some in
/// use and gives it back.
fn judge_shared_memory(size: f64) -> Check {
    const TITLE: &str = "Shared memory";
    let room = size as i64 - (IO_SPACE_GIB + TMPFS_SPARE_GIB) as i64;
    let text =
        format!("/dev/shm holds {size:.0} GiB, so a Windows guest can have up to {} GiB.", room.max(0));
    let need = WANTED_SHM_GIB + IO_SPACE_GIB + TMPFS_SPARE_GIB;
    let fix = format!("Make it larger: sudo mount -o remount,size={need}G /dev/shm");
    if room < i64::from(MIN_RAM_GIB) {
        Check::new(TITLE, CheckState::Failed, text, fix)
    } else if room < WANTED_SHM_GIB as i64 {
        Check::new(TITLE, CheckState::Warning, text, fix)
    } else {
        Check::ok(TITLE, text)
    }
}

fn check_shared_memory(context: &Context) -> Check {
    let size = statvfs(&context.paths.shared_memory)
        .map_or(0.0, |disk| (disk.f_blocks * disk.f_frsize) as f64 / GIB as f64);
    judge_shared_memory(size)
}

/// The nearest folder that exists.
pub fn existing_parent(path: &Path) -> PathBuf {
    path.ancestors().find(|folder| folder.exists()).unwrap_or(Path::new("/")).to_owned()
}

/// Judges the disk: `room` is what is free in `place`.
fn judge_disk(room: f64, place: &Path) -> Check {
    const TITLE: &str = "Free disk";
    let need = INSTALL_ROOM_GIB + DRIVER_ROOM_GIB;
    let text = format!("{room:.0} GiB free in {}.", place.display());
    if room < need as f64 {
        let fix = format!("A Windows machine needs {need} GiB. Free some space there.");
        return Check::new(TITLE, CheckState::Failed, text, fix);
    }
    Check::ok(TITLE, text)
}

fn check_disk(context: &Context) -> Check {
    let store = &context.paths.store;
    let place = existing_parent(store);
    if access(&place, Access::WRITE_OK).is_err() {
        let fix = format!("Make {} and give it to this user.", store.display());
        return Check::new(
            "Free disk",
            CheckState::Failed,
            format!("This user cannot write to {}.", place.display()),
            fix,
        );
    }
    judge_disk(free_gib(&place).unwrap_or(0.0), &place)
}

pub fn check_pc(context: &Context) -> Vec<Check> {
    vec![
        check_card(context),
        check_host_driver(context),
        check_kvm(context),
        check_qemu(context),
        check_tools(context, &TOOLS),
        check_ubuntu(context, &UBUNTU_TOOLS),
        check_shared_memory(context),
        check_disk(context),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::cards::stand_in::{NAMES, fake_pci};
    use crate::testing::{Scratch, scratch_context};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_card_check_says_what_is_known() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        let found = check_card(&context);
        assert!(
            found.state == CheckState::Failed
                && found.text.contains("No NVIDIA card")
                && !found.fix.is_empty()
        );
        fake_pci(&scratch, "0000:01:00.0", 0x2BB1, "0x10de", "0x030000");
        let found = check_card(&context);
        assert!(found.state == CheckState::Failed && found.text.contains("Not supported"));
        fake_pci(&scratch, "0000:02:00.0", 0x2C02, "0x10de", "0x030000");
        let found = check_card(&context);
        assert_eq!(found.state, CheckState::Warning);
        assert!(found.text.contains("Experimental") && found.text.contains("GB203") && !found.fix.is_empty());
        fake_pci(&scratch, "0000:03:00.0", 0x2B85, "0x10de", "0x030000");
        assert_eq!(check_card(&context).state, CheckState::Ok);
    }

    #[test]
    fn the_host_driver_is_checked_against_what_was_tested() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.driver_version_file = scratch.path("nothing");
        assert_eq!(check_host_driver(&context).state, CheckState::Failed);
        assert!(driver_warning(&context).unwrap().contains("not seem to be loaded"));
        context.driver_version_file = scratch.write("version", format!("{TESTED_DRIVER}\n"));
        assert_eq!(check_host_driver(&context).state, CheckState::Ok);
        assert_eq!(driver_warning(&context), None);
        fs::write(&context.driver_version_file, "600.1\n").unwrap();
        let found = check_host_driver(&context);
        assert!(
            found.state == CheckState::Warning
                && found.text.contains("600.1")
                && found.fix.contains(TESTED_DRIVER)
        );
        let warning = driver_warning(&context).unwrap();
        assert!(warning.contains("600.1") && warning.contains(TESTED_DRIVER));
    }

    #[test]
    fn kvm_must_be_there_and_open() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.kvm = scratch.path("kvm");
        let found = check_kvm(&context);
        assert!(found.state == CheckState::Failed && found.fix.contains("firmware"));
        fs::write(&context.kvm, "").unwrap();
        fs::set_permissions(&context.kvm, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(check_kvm(&context).state, CheckState::Ok);
        fs::set_permissions(&context.kvm, fs::Permissions::from_mode(0o000)).unwrap();
        let found = check_kvm(&context);
        assert!(
            rustix::process::geteuid().is_root()
                || (found.state == CheckState::Failed && found.fix.contains("usermod"))
        );
    }

    #[test]
    fn qemu_must_run_and_says_why_if_it_cannot() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.qemu = scratch.path("qemu");
        let found = check_qemu(&context);
        assert!(found.state == CheckState::Failed && found.fix.contains("qemu/README.md"));
        let script = "#!/bin/sh\necho 'error while loading shared libraries: libslirp.so.0' >&2\nexit 127\n";
        scratch.program("qemu", script);
        let found = check_qemu(&context);
        assert!(
            found.state == CheckState::Failed
                && found.text.contains("libslirp.so.0")
                && found.fix.contains("dnf install")
        );
        fs::write(&context.paths.qemu, "#!/bin/sh\necho QEMU\n").unwrap();
        assert_eq!(check_qemu(&context).state, CheckState::Ok);
    }

    #[test]
    fn nothing_about_ubuntu_stops_it_booting() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.driver_version_file = scratch.write("version", "615.78.08\n");
        assert_eq!(check_ubuntu(&context, &["sh"]).state, CheckState::Ok);
        fs::write(&context.driver_version_file, "615.90.01\n").unwrap();
        let found = check_ubuntu(&context, &["sh"]);
        assert!(found.state == CheckState::Warning && found.fix.contains("615.90.01"));
        fs::write(&context.driver_version_file, "620.11\n").unwrap();
        let found = check_ubuntu(&context, &["sh"]);
        assert!(
            found.state == CheckState::Warning && found.text.contains("no GPU") && found.fix.contains("620")
        );
        let found = check_ubuntu(&context, &["no-such-tool"]);
        assert!(found.state == CheckState::Warning && found.text.contains("no-such-tool"));
    }

    #[test]
    fn missing_tools_are_named() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.firmware.vars = scratch.path("nothing");
        let found = check_tools(&context, &["no-such-tool", "sh"]);
        assert_eq!(found.state, CheckState::Failed);
        assert!(
            found.text.contains("no-such-tool") && found.text.contains("OVMF") && !found.text.contains("sh,")
        );
    }

    #[test]
    fn shared_memory_is_judged_by_what_a_guest_could_have() {
        let room = IO_SPACE_GIB + TMPFS_SPARE_GIB;
        let min = u64::from(MIN_RAM_GIB);
        for (gib, state) in [
            (room + min - 1, CheckState::Failed),
            (room + min, CheckState::Warning),
            (room + WANTED_SHM_GIB, CheckState::Ok),
        ] {
            let found = judge_shared_memory(gib as f64);
            assert_eq!(found.state, state, "{gib}");
            assert!(state == CheckState::Ok || found.fix.contains("remount"));
        }
    }

    #[test]
    fn the_driver_needs_room_too() {
        let place = Path::new("/");
        assert_eq!(judge_disk(INSTALL_ROOM_GIB as f64, place).state, CheckState::Failed);
        assert_eq!(judge_disk((INSTALL_ROOM_GIB + DRIVER_ROOM_GIB) as f64, place).state, CheckState::Ok);
    }

    #[test]
    fn the_nearest_folder_is_found() {
        let scratch = Scratch::new();
        assert_eq!(existing_parent(&scratch.path("a/b/store")), scratch.root());
    }
}
