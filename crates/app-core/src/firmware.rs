//! The firmware files, which come from the distribution. Fedora's are named outright, as a machine's
//! settings file has to match the size of its firmware, and machines made here have those. Anywhere
//! else, QEMU's own descriptors say where a distribution put them.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const FEDORA_FOLDER: &str = "/usr/share/edk2/ovmf";
const DESCRIPTOR_FOLDER: &str = "/usr/share/qemu/firmware";
const SECURE_BOOT: &str = "secure-boot";
const ENROLLED_KEYS: &str = "enrolled-keys";
const NEEDS_SMM: &str = "requires-smm";
const QCOW2: &str = "qcow2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub code: PathBuf,
    /// Goes with `code`.
    pub vars: PathBuf,
    /// The one with TPM drivers.
    pub code_secure: PathBuf,
    /// With Microsoft's keys, so Secure Boot is on. It goes with `code_secure`.
    pub vars_secure: PathBuf,
}

/// QEMU's word for a firmware file's format, which depends on the distribution.
pub fn format_of(file: &Path) -> &'static str {
    if file.extension().is_some_and(|name| name == QCOW2) { QCOW2 } else { "raw" }
}

/// The code and settings files of a descriptor, if it's split firmware for this kind of PC, with the
/// SMM the app's machines run with, and with or without Microsoft's keys as asked.
fn files_of(descriptor: &Value, with_keys: bool) -> Option<(PathBuf, PathBuf)> {
    let has = |list: &Value, word: &str| {
        list.as_array().is_some_and(|words| words.iter().any(|found| found == word))
    };
    let features = &descriptor["features"];
    let suits_this_pc = descriptor["targets"]
        .as_array()?
        .iter()
        .any(|target| target["architecture"] == "x86_64" && target["machines"].to_string().contains("q35"));
    let wanted = has(&descriptor["interface-types"], "uefi")
        && suits_this_pc
        && descriptor["mapping"]["device"] == "flash"
        && has(features, SECURE_BOOT)
        && has(features, NEEDS_SMM)
        && has(features, ENROLLED_KEYS) == with_keys;
    let mapping = &descriptor["mapping"];
    let file = |part: &str| mapping[part]["filename"].as_str().map(PathBuf::from);
    wanted.then(|| Some((file("executable")?, file("nvram-template")?))).flatten()
}

/// What the descriptors in `folder` say. QEMU goes through them in name order, and so does this.
fn from_descriptors(folder: &Path) -> Option<Firmware> {
    let mut names: Vec<PathBuf> = fs::read_dir(folder).ok()?.flatten().map(|entry| entry.path()).collect();
    names.sort();
    let descriptors: Vec<Value> =
        names.iter().filter_map(|path| serde_json::from_slice(&fs::read(path).ok()?).ok()).collect();
    let first = |with_keys| descriptors.iter().find_map(|found| files_of(found, with_keys));
    let (code, vars) = first(false)?;
    let (code_secure, vars_secure) = first(true)?;
    Some(Firmware { code, vars, code_secure, vars_secure })
}

impl Firmware {
    pub fn fedora() -> Self {
        let file = |name: &str| Path::new(FEDORA_FOLDER).join(name);
        Self {
            code: file("OVMF_CODE.secboot.fd"),
            vars: file("OVMF_VARS.fd"),
            code_secure: file("OVMF_CODE_4M.secboot.qcow2"),
            vars_secure: file("OVMF_VARS_4M.secboot.qcow2"),
        }
    }
}

impl Default for Firmware {
    fn default() -> Self {
        let named = Self::fedora();
        if named.code.exists() {
            return named;
        }
        from_descriptors(Path::new(DESCRIPTOR_FOLDER)).unwrap_or(named)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn descriptor(code: &str, vars: &str, features: &str) -> String {
        format!(
            r#"{{"interface-types": ["uefi"],
                "mapping": {{"device": "flash", "mode": "split",
                  "executable": {{"filename": "{code}", "format": "raw"}},
                  "nvram-template": {{"filename": "{vars}", "format": "raw"}}}},
                "targets": [{{"architecture": "x86_64", "machines": ["pc-q35-*"]}}],
                "features": [{features}]}}"#
        )
    }

    #[test]
    fn debians_descriptors_give_its_firmware() {
        let scratch = Scratch::new();
        scratch.write(
            "firmware/40-secure-enrolled.json",
            descriptor(
                "/o/CODE.secboot.fd",
                "/o/VARS.ms.fd",
                r#""enrolled-keys", "requires-smm", "secure-boot""#,
            ),
        );
        scratch.write(
            "firmware/50-secure.json",
            descriptor("/o/CODE.secboot.fd", "/o/VARS.fd", r#""requires-smm", "secure-boot""#),
        );
        scratch.write("firmware/60-plain.json", descriptor("/o/CODE.fd", "/o/PLAIN.fd", ""));
        let found = from_descriptors(&scratch.path("firmware")).unwrap();
        assert_eq!(found.code, Path::new("/o/CODE.secboot.fd"));
        assert_eq!(found.vars, Path::new("/o/VARS.fd"));
        assert_eq!(found.vars_secure, Path::new("/o/VARS.ms.fd"));
    }

    #[test]
    fn with_no_firmware_for_this_pc_there_is_no_answer() {
        let scratch = Scratch::new();
        scratch.write("firmware/10-other.json", descriptor("/o/C", "/o/V", "").replace("x86_64", "aarch64"));
        assert_eq!(from_descriptors(&scratch.path("firmware")), None);
        assert_eq!(from_descriptors(&scratch.path("nowhere")), None);
    }

    #[test]
    fn the_format_is_in_the_name() {
        assert_eq!(format_of(Path::new("/o/CODE.secboot.qcow2")), "qcow2");
        assert_eq!(format_of(Path::new("/o/CODE.secboot.fd")), "raw");
    }
}
