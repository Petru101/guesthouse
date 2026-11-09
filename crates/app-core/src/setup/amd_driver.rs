//! AMD's Windows driver, for Windows machines on an AMD card: checking AMD's installer, and taking its
//! display driver out.
//!
//! AMD ships more than one package of a release, each for a range of cards, so the installer is judged
//! by whether its INF lists the user's card as well as by its version. The display driver is the
//! folder of the INF under `Packages/Drivers/Display`. The user-mode driver the guest loads sits in a
//! folder inside that, and that folder is the one the guest gets.

use super::amd::write_amd_setup;
use super::driver::Extraction;
use super::inf::{inf_driver_version, latin1};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::vm_args::on_path;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Adrenalin release the AMD daemon was written for, which refuses any other.
pub const AMD_WINDOWS_DRIVER: &str = "32.0.32015.12012";
const DISPLAY_FOLDER: &str = "Packages/Drivers/Display";
const USER_DRIVER: &str = "amdxc64.dll";
const PATH_LINE: &str = "Path = ";
const READ_SECONDS: &str = "120";
/// What `timeout` exits with.
const TIMED_OUT: i32 = 124;

/// What is known of an installer before it is unpacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmdPackage {
    /// The display driver's folder in the archive.
    pub driver_entry: String,
    pub version: String,
}

/// The INF of the display driver among the archive's paths: the one file ending in `.inf` in a
/// folder of its own directly under the display folder.
fn display_inf(paths: &str) -> Option<&str> {
    paths.lines().filter_map(|line| line.strip_prefix(PATH_LINE)).find(|path| {
        let Some(inside) = path.strip_prefix(DISPLAY_FOLDER).and_then(|rest| rest.strip_prefix('/')) else {
            return false;
        };
        let parts: Vec<&str> = inside.split('/').collect();
        parts.len() == 2 && parts[1].to_lowercase().ends_with(".inf")
    })
}

/// Whether the INF has a line for this AMD device, whatever its subsystem and revision.
fn lists_device(inf: &str, device: u32) -> bool {
    let wanted = format!("PCI\\VEN_1002&DEV_{device:04X}");
    inf.to_uppercase().contains(&wanted)
}

/// Says why a package with this INF won't do for the card `device`, if it won't.
pub fn package_problem(inf: &str, device: u32) -> Option<String> {
    let version = match inf_driver_version(inf) {
        Ok(version) => version,
        Err(failure) => return Some(failure.0),
    };
    if version != AMD_WINDOWS_DRIVER {
        return Some(format!(
            "This is AMD's driver {version}. Guesthouse was built for {AMD_WINDOWS_DRIVER}, so get that release \
             from AMD."
        ));
    }
    (!lists_device(inf, device)).then(|| {
        format!(
            "This package of AMD's driver doesn't cover this card, device {device:04X}. AMD makes more than \
             one package of each release; get the one for this card."
        )
    })
}

fn seven_zip(args: &[&str]) -> Result<Vec<u8>> {
    if !on_path("7z") {
        return Err(Failure::new("Reading the installer needs 7z, which is not installed."));
    }
    let result = Command::new("timeout").arg(READ_SECONDS).arg("7z").args(args).output()?;
    if result.status.code() == Some(TIMED_OUT) {
        return Err(Failure::new("The installer took too long to read."));
    }
    if !result.status.success() {
        return Err(Failure::new("This file is not AMD's driver installer."));
    }
    Ok(result.stdout)
}

/// Reads AMD's installer at `path`, and says whether it will do for the card `device`.
pub fn read_amd_installer(path: &str, device: u32) -> Result<AmdPackage> {
    let listing = String::from_utf8_lossy(&seven_zip(&["l", "-ba", "-slt", path])?).into_owned();
    let inf_path =
        display_inf(&listing).ok_or_else(|| Failure::new("This file is not AMD's driver installer."))?;
    let inf = latin1(&seven_zip(&["e", "-so", path, inf_path])?);
    if let Some(problem) = package_problem(&inf, device) {
        return Err(Failure(problem));
    }
    let driver_entry = inf_path.rsplit_once('/').map_or(inf_path, |(folder, _)| folder).to_owned();
    Ok(AmdPackage { driver_entry, version: AMD_WINDOWS_DRIVER.to_owned() })
}

/// The folder inside the display driver that holds the user-mode driver.
fn user_driver_folder(driver: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(driver).ok()?.flatten();
    entries.map(|entry| entry.path()).find(|folder| folder.join(USER_DRIVER).is_file())
}

/// Takes the display driver out of AMD's installer and writes down where it is. `progress` gets the
/// percentage done.
pub fn unpack_amd(
    extraction: &Extraction,
    context: &Context,
    package: &AmdPackage,
    progress: &(impl Fn(u32) + ?Sized),
) -> Result<PathBuf> {
    let target = context.paths.setup_folder().join(format!("amd-driver-{}", package.version));
    let missing = || format!("{USER_DRIVER} is not in AMD's display driver.");
    let check =
        |unpacked: &Path| user_driver_folder(&unpacked.join(&package.driver_entry)).is_none().then(missing);
    extraction.unpack(context, &package.driver_entry, &target, &check, progress)?;
    let folder = user_driver_folder(&target.join(&package.driver_entry)).ok_or_else(|| Failure(missing()))?;
    write_amd_setup(context, &folder, &package.version)?;
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::amd::amd_driver_folder;
    use crate::testing::{Scratch, scratch_context};
    use std::process::Stdio;

    const IGPU: u32 = 0x13C0;

    fn inf(version: &str, devices: &[u32]) -> String {
        let mut text = format!("[Version]\nDriverVer=10/28/2026, {version}\n[ATI.Mfg.NTamd64]\n");
        for device in devices {
            text += &format!("\"%AMD{device:04X}.1%\" = ati2mtag_X, PCI\\VEN_1002&DEV_{device:04X}&REV_C1\n");
        }
        text
    }

    /// An archive laid out like AMD's installer, which 7z can read.
    fn make_installer(scratch: &Scratch, devices: &[u32]) -> String {
        let driver = "source/Packages/Drivers/Display/WT6A_INF";
        scratch.write(&format!("{driver}/u0205499.inf"), inf(AMD_WINDOWS_DRIVER, devices));
        scratch.write(&format!("{driver}/B026716/{USER_DRIVER}"), "user-mode driver");
        scratch.write(&format!("{driver}/B026716/amdkmdag.sys"), "kernel driver");
        scratch.write("source/Packages/Apps/not-needed.msi", "x");
        let path = scratch.path("amd.exe");
        let done = Command::new("7z")
            .args(["a", "-t7z"])
            .arg(&path)
            .arg(".")
            .current_dir(scratch.path("source"))
            .stdout(Stdio::null())
            .status();
        assert!(done.unwrap().success());
        path.display().to_string()
    }

    #[test]
    fn a_package_is_judged_by_its_version_and_by_listing_the_card() {
        assert_eq!(package_problem(&inf(AMD_WINDOWS_DRIVER, &[0x7550, IGPU]), IGPU), None);
        let other_cards = package_problem(&inf(AMD_WINDOWS_DRIVER, &[0x7550]), IGPU).unwrap();
        assert!(other_cards.contains("device 13C0"), "{other_cards}");
        let older = package_problem(&inf("32.0.21001.1", &[IGPU]), IGPU).unwrap();
        assert!(older.contains("32.0.21001.1"), "{older}");
        assert!(package_problem("[Version]\n", IGPU).unwrap().contains("no version"));
    }

    #[test]
    fn the_display_inf_is_the_one_in_a_folder_of_its_own() {
        let listing = "Path = Packages/Drivers/Display/WT6A_INF/B026716/amdxe.inf\n\
                       Path = Packages/Drivers/Display/WT6A_INF/u0205499.inf\n\
                       Path = Packages/Drivers/Audio/x.inf\n";
        assert_eq!(display_inf(listing), Some("Packages/Drivers/Display/WT6A_INF/u0205499.inf"));
        assert_eq!(display_inf("Path = Packages/Drivers/Audio/x.inf\n"), None);
    }

    #[test]
    fn an_installer_for_the_card_is_unpacked_and_written_down() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let installer = make_installer(&scratch, &[IGPU]);
        let package = read_amd_installer(&installer, IGPU).unwrap();
        assert_eq!(package.driver_entry, "Packages/Drivers/Display/WT6A_INF");
        let folder = unpack_amd(&Extraction::new(&installer), &context, &package, &|_| {}).unwrap();
        assert!(folder.join(USER_DRIVER).is_file());
        assert_eq!(amd_driver_folder(&context), Some(folder));
        assert!(!scratch.path("store/setup").join("amd-driver-32.0.32015.12012/Packages/Apps").exists());
    }

    #[test]
    fn an_installer_for_other_cards_is_refused_before_unpacking() {
        let scratch = Scratch::new();
        let installer = make_installer(&scratch, &[0x7550]);
        let refused = read_amd_installer(&installer, IGPU).unwrap_err();
        assert!(refused.0.contains("doesn't cover this card"), "{}", refused.0);
    }

    /// AMD's own installer, named in `GUESTHOUSE_AMD_INSTALLER`, with the card in
    /// `GUESTHOUSE_AMD_DEVICE` as hex. Run by hand, as the installer is a gigabyte and not ours.
    #[test]
    #[ignore = "needs AMD's installer"]
    fn a_real_installer_is_read() {
        let path = std::env::var("GUESTHOUSE_AMD_INSTALLER").unwrap();
        let device = u32::from_str_radix(&std::env::var("GUESTHOUSE_AMD_DEVICE").unwrap(), 16).unwrap();
        match read_amd_installer(&path, device) {
            Ok(package) => println!("covers {device:04X}: {package:?}"),
            Err(failure) => println!("refused {device:04X}: {}", failure.0),
        }
    }

    /// The release is the AMD daemon's, which lives in the backend.
    #[test]
    fn the_release_is_the_one_the_daemon_takes() {
        let path = format!("{}/../backend-amd/src/protocol/guest_driver.rs", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains(&format!("GUEST_RELEASE: &str = \"{AMD_WINDOWS_DRIVER}\"")));
    }
}
