//! What a Linux guest says about its GPU, on its serial port. gpu-up.sh writes one line at each boot,
//! `nvfwd-gpu on VERSION` or `nvfwd-gpu off REASON`, and the app shows the newest while the VM runs.
//! The guest writes it, so only that prefix counts, and only plain words of a sane length are shown.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const SERIAL_LOG: &str = "serial.log";
const PREFIX: &str = "nvfwd-gpu ";
/// The line is written early in the boot, and a guest's console can say a lot after it.
const TAIL_BYTES: u64 = 256 * 1024;
const LONGEST_REASON: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestGpu {
    On { version: String },
    Off { reason: String },
}

impl GuestGpu {
    /// The words for the machine's row.
    pub fn text(&self) -> String {
        match self {
            Self::On { version } => format!("The guest has its GPU, on driver {version}."),
            Self::Off { reason } => format!("The guest has no GPU on this boot: {reason}"),
        }
    }
}

/// Keeps what a person would type, so a guest can't send the row control characters or markup.
fn plain(text: &str) -> String {
    let typed = |letter: &char| letter.is_ascii_graphic() || *letter == ' ';
    let kept = text.chars().filter(|letter| typed(letter) && !matches!(letter, '<' | '>' | '&'));
    kept.take(LONGEST_REASON).collect::<String>().trim().to_owned()
}

fn read_line(line: &str) -> Option<GuestGpu> {
    let said = line.trim_end_matches('\r').strip_prefix(PREFIX)?;
    let (word, rest) = said.split_once(' ').unwrap_or((said, ""));
    let rest = plain(rest);
    match word {
        "on" if !rest.is_empty() && !rest.contains(' ') => Some(GuestGpu::On { version: rest }),
        "off" if !rest.is_empty() => Some(GuestGpu::Off { reason: rest }),
        _ => None,
    }
}

/// The newest line in what the serial port has had, if there's one.
pub fn newest(serial: &[u8]) -> Option<GuestGpu> {
    String::from_utf8_lossy(serial).lines().rev().find_map(read_line)
}

/// The newest line in a run folder's serial log. A guest that never wrote one says nothing.
pub fn read(run_folder: &Path) -> Option<GuestGpu> {
    let mut file = File::open(run_folder.join(SERIAL_LOG)).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES))).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut tail).ok()?;
    newest(&tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn the_newest_line_wins() {
        let serial =
            b"[  1.0] booting\nnvfwd-gpu off no nvfwd device.\nrebooting\nnvfwd-gpu on 615.78.08\r\nlogin:";
        assert_eq!(newest(serial), Some(GuestGpu::On { version: "615.78.08".into() }));
        assert_eq!(
            newest(b"nvfwd-gpu on 615.78.08\nnvfwd-gpu off the build failed\n").unwrap().text(),
            "The guest has no GPU on this boot: the build failed"
        );
    }

    #[test]
    fn only_a_line_of_its_own_counts() {
        for serial in [
            &b""[..],
            b"echo nvfwd-gpu on 615.78.08\n",
            b"nvfwd-gpu on\n",
            b"nvfwd-gpu on 615 and more\n",
            b"nvfwd-gpu sideways 615\n",
            b"nvfwd-gpuon 615\n",
        ] {
            assert_eq!(newest(serial), None, "{}", String::from_utf8_lossy(serial));
        }
    }

    #[test]
    fn a_guest_cannot_send_the_row_anything_but_plain_words() {
        let mut serial = b"nvfwd-gpu off <b>bold</b>\x1b[31m red\x07 ".to_vec();
        serial.extend(std::iter::repeat_n(b'x', 1000));
        let Some(GuestGpu::Off { reason }) = newest(&serial) else { panic!("it should be read") };
        assert!(reason.starts_with("bbold/b[31m red x"), "{reason}");
        assert_eq!(reason.len(), LONGEST_REASON);
    }

    #[test]
    fn only_the_end_of_a_long_log_is_read() {
        let scratch = Scratch::new();
        let mut log = b"nvfwd-gpu on 615.78.08\n".to_vec();
        log.extend(std::iter::repeat_n(b'.', TAIL_BYTES as usize));
        std::fs::write(scratch.path("serial.log"), &log).unwrap();
        assert_eq!(read(&scratch.path("")), None);
        log.extend(b"\nnvfwd-gpu off the host gave no image\n");
        std::fs::write(scratch.path("serial.log"), &log).unwrap();
        assert_eq!(read(&scratch.path("")), Some(GuestGpu::Off { reason: "the host gave no image".into() }));
        assert_eq!(read(&scratch.path("nowhere")), None);
    }
}
