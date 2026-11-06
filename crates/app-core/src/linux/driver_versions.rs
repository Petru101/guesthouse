//! Decides whether the host's NVIDIA files may go to a guest.
//!
//! The guest's user-mode driver talks to the kernel module the host has loaded right now, and the two
//! must be the same version. After an update the files on disk are the new version and the loaded
//! module stays the old one until a reboot, so the loaded module is what everything is compared to.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// The packages that hold what a guest runs. Their versions must all be the loaded one.
pub const DRIVER_PACKAGES: [&str; 3] =
    ["xorg-x11-drv-nvidia-libs", "xorg-x11-drv-nvidia-cuda-libs", "xorg-x11-drv-nvidia-cuda"];
/// Versions a person has run the whole check on. Another point release of a branch runs with a
/// warning, and a branch nobody knows doesn't run, as a new branch can change what the driver asks of
/// the host, on a card the host's own desktop is using.
pub const TESTED_VERSIONS: [&str; 1] = ["615.78.08"];

/// What the host's package database and kernel say.
pub trait DriverHost {
    fn loaded_version(&self) -> Option<String>;
    fn installed_version(&self, package: &str) -> Option<String>;
    /// Every file of these packages.
    fn packaged_files(&self, packages: &[&str]) -> Vec<String>;
}

pub struct RpmHost {
    pub version_file: PathBuf,
}

impl DriverHost for RpmHost {
    fn loaded_version(&self) -> Option<String> {
        let text = fs::read_to_string(&self.version_file).ok()?;
        Some(text.trim().to_owned()).filter(|version| !version.is_empty())
    }

    fn installed_version(&self, package: &str) -> Option<String> {
        let name = format!("{package}.x86_64");
        let result = Command::new("rpm").args(["-q", "--qf", "%{VERSION}", &name]).output().ok()?;
        result.status.success().then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
    }

    /// rpm says a package isn't installed on stdout, and the list of what's required then names it.
    fn packaged_files(&self, packages: &[&str]) -> Vec<String> {
        let names = packages.iter().map(|package| format!("{package}.x86_64"));
        let listing = Command::new("rpm").arg("-ql").args(names).output();
        let text =
            listing.map(|result| String::from_utf8_lossy(&result.stdout).into_owned()).unwrap_or_default();
        text.lines().filter(|line| line.starts_with('/')).map(str::to_owned).collect()
    }
}

pub fn installed_versions(host: &dyn DriverHost) -> BTreeMap<String, Option<String>> {
    DRIVER_PACKAGES.iter().map(|package| ((*package).to_owned(), host.installed_version(package))).collect()
}

pub fn branch(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// Says why a driver's branch can't run here, or `None` if it's known.
pub fn branch_refusal(loaded: &str) -> Option<String> {
    let mut known: Vec<&str> = TESTED_VERSIONS.iter().map(|version| branch(version)).collect();
    known.dedup();
    (!known.contains(&branch(loaded))).then(|| {
        format!(
            "The host's driver {loaded} is from branch {}, and I only know branch {}. Install a driver from \
             that branch.",
            branch(loaded),
            known.join(", ")
        )
    })
}

/// Says in plain words why these files can't be offered, or `None` if they can.
pub fn refusal(loaded: Option<&str>, installed: &BTreeMap<String, Option<String>>) -> Option<String> {
    let Some(loaded) = loaded else {
        return Some(
            "The host has no NVIDIA kernel module loaded, so there is no driver to match.".to_owned(),
        );
    };
    let missing: Vec<&str> =
        installed.iter().filter(|(_, version)| version.is_none()).map(|(name, _)| name.as_str()).collect();
    if !missing.is_empty() {
        return Some(format!("These host packages are not installed: {}.", missing.join(", ")));
    }
    let mut different: Vec<&str> =
        installed.values().flatten().map(String::as_str).filter(|version| *version != loaded).collect();
    different.sort_unstable();
    different.dedup();
    (!different.is_empty()).then(|| {
        format!(
            "The host's NVIDIA files are version {}, but its loaded kernel module is {loaded}. A driver update \
             is probably waiting for a reboot. Reboot the host, then start the VM again.",
            different.join(", ")
        )
    })
}

/// A warning for a known branch at a version nobody has run.
pub fn untested_warning(loaded: &str) -> Option<String> {
    (!TESTED_VERSIONS.contains(&loaded)).then(|| {
        format!(
            "Driver {loaded} is a known branch, but this exact version has not been tested. The modules and \
             tables are built for it, but the allow list may refuse what it needs."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(version: Option<&str>) -> BTreeMap<String, Option<String>> {
        DRIVER_PACKAGES.iter().map(|name| ((*name).to_owned(), version.map(str::to_owned))).collect()
    }

    #[test]
    fn the_tested_version_and_its_branch_are_known() {
        assert_eq!(branch_refusal("615.78.08"), None);
        assert_eq!(branch_refusal("615.90.01"), None);
    }

    #[test]
    fn an_unknown_branch_is_refused_by_name() {
        let said = branch_refusal("620.11").unwrap();
        assert!(said.contains("620") && said.contains("615"));
    }

    #[test]
    fn another_point_release_only_warns() {
        assert_eq!(untested_warning("615.78.08"), None);
        assert!(untested_warning("615.90.01").unwrap().contains("615.90.01"));
    }

    #[test]
    fn matching_files_can_be_offered() {
        assert_eq!(refusal(Some("615.78.08"), &versions(Some("615.78.08"))), None);
    }

    #[test]
    fn no_module_missing_packages_and_a_pending_reboot_each_say_so() {
        assert!(refusal(None, &versions(None)).unwrap().contains("no NVIDIA kernel module"));
        assert!(refusal(Some("615.78.08"), &versions(None)).unwrap().contains("not installed"));
        let said = refusal(Some("615.78.08"), &versions(Some("615.90.01"))).unwrap();
        assert!(said.contains("615.90.01") && said.contains("Reboot"));
    }
}
