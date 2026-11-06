//! What one running VM owns on this PC, so that several can run side by side.
//!
//! Slot 0, which is also what an unset variable means, has the names a single VM always had. Each
//! other slot adds its number. QEMU and the scripts name things the same way.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The environment variable that names the slot.
pub const VARIABLE: &str = "GUESTHOUSE_SLOT";
/// How many VMs one PC can run.
pub const SLOTS: u8 = 4;

const SHARED_MEMORY: &str = "/dev/shm";
const RUN_FOLDER: &str = "run";
const SHARED_FILE_PREFIX: &str = "gpupv";
const SHARED_FILES: [&str; 3] = ["iomem", "ram", "display"];
const AGENT_PORT: u16 = 18765;
const DEBUG_PORT: u16 = 1234;
const VNC_DISPLAY: u16 = 77;

/// A slot number that cannot be used. It stops the daemon, as guessing could reach another VM's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadSlot(String);

impl fmt::Display for BadSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{VARIABLE} must be a number below {SLOTS}, not {:?}", self.0)
    }
}

impl std::error::Error for BadSlot {}

/// A VM's place on this PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot(u8);

impl Slot {
    /// Reads the slot from the variable's value, which is `None` when unset.
    pub fn parse(value: Option<&str>) -> Result<Self, BadSlot> {
        let Some(text) = value else { return Ok(Self(0)) };
        match text.parse() {
            Ok(number) if number < SLOTS && text.bytes().all(|byte| byte.is_ascii_digit()) => {
                Ok(Self(number))
            }
            _ => Err(BadSlot(text.to_owned())),
        }
    }

    pub fn from_environment() -> Result<Self, BadSlot> {
        Self::parse(std::env::var(VARIABLE).ok().as_deref())
    }

    pub fn number(self) -> u8 {
        self.0
    }

    fn suffix(self) -> String {
        if self.0 == 0 { String::new() } else { format!("-{}", self.0) }
    }

    /// The folder of sockets and switches, inside a project folder.
    pub fn run_folder(self, project: &Path) -> PathBuf {
        project.join(format!("{RUN_FOLDER}{}", self.suffix()))
    }

    /// A file in shared memory, like `ram` or `iomem`.
    pub fn shared_file(self, name: &str) -> PathBuf {
        Path::new(SHARED_MEMORY).join(format!("{SHARED_FILE_PREFIX}-{name}{}", self.suffix()))
    }

    /// Gives back what QEMU left in shared memory. A file some other process still has mapped stays
    /// until that process lets go, as it would for any file.
    pub fn remove_shared_files(self) {
        for name in SHARED_FILES {
            let _ = fs::remove_file(self.shared_file(name));
        }
    }

    pub fn agent_port(self) -> u16 {
        AGENT_PORT + u16::from(self.0)
    }

    pub fn debug_port(self) -> u16 {
        DEBUG_PORT + u16::from(self.0)
    }

    pub fn vnc_display(self) -> u16 {
        VNC_DISPLAY + u16::from(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_variable_is_slot_zero() {
        assert_eq!(Slot::parse(None).unwrap().number(), 0);
    }

    #[test]
    fn slot_zero_has_the_plain_names() {
        let slot = Slot::parse(Some("0")).unwrap();
        assert_eq!(slot.run_folder(Path::new("/project")), Path::new("/project/run"));
        assert_eq!(slot.shared_file("ram"), Path::new("/dev/shm/gpupv-ram"));
    }

    #[test]
    fn other_slots_add_their_number() {
        let slot = Slot::parse(Some("2")).unwrap();
        assert_eq!(slot.run_folder(Path::new("/project")), Path::new("/project/run-2"));
        assert_eq!(slot.shared_file("iomem"), Path::new("/dev/shm/gpupv-iomem-2"));
        assert_eq!(slot.agent_port(), 18767);
        assert_eq!(slot.debug_port(), 1236);
        assert_eq!(slot.vnc_display(), 79);
    }

    #[test]
    fn anything_else_is_refused() {
        for text in ["4", "-1", "+1", "x", "", "1 ", "256"] {
            let error = Slot::parse(Some(text)).unwrap_err();
            assert!(error.to_string().contains("below 4"), "{text}");
        }
    }
}
