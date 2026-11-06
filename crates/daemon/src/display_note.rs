//! What the daemon says about the display driver's screen, in the run folder. Windows can name a mode
//! and still send nothing, so the app goes by this for whether our display is really up.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

const NOTE_FILE: &str = "display-ring";
const HDR_WORD: &str = "hdr";
const SDR_WORD: &str = "sdr";
const FRAMES_WORD: &str = "frames";
const NO_FRAMES_WORD: &str = "empty";

/// What the display driver's ring holds now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenNote {
    pub width: u32,
    pub height: u32,
    pub hdr: bool,
    /// Who wrote it. A daemon that dies leaves its note behind, so a reader has to see it's alive.
    pub daemon: u32,
    /// A frame has landed on this ring. It's open from the moment the driver registers it.
    pub frames: bool,
}

impl ScreenNote {
    fn text(self) -> String {
        let look = if self.hdr { HDR_WORD } else { SDR_WORD };
        let frames = if self.frames { FRAMES_WORD } else { NO_FRAMES_WORD };
        format!("{}x{} {look} {} {frames}\n", self.width, self.height, self.daemon)
    }

    fn parse(text: &str) -> Option<Self> {
        let mut words = text.split_whitespace();
        let (size, look, daemon, frames) = (words.next()?, words.next()?, words.next()?, words.next()?);
        let (width, height) = size.split_once('x')?;
        let hdr = match look {
            HDR_WORD => true,
            SDR_WORD => false,
            _ => return None,
        };
        let frames = match frames {
            FRAMES_WORD => true,
            NO_FRAMES_WORD => false,
            _ => return None,
        };
        Some(Self {
            width: width.parse().ok()?,
            height: height.parse().ok()?,
            hdr,
            daemon: daemon.parse().ok()?,
            frames,
        })
    }
}

/// Says what's on the ring, or that nothing is. The note goes with the ring.
pub fn write_note(run_folder: &Path, note: Option<ScreenNote>) -> io::Result<()> {
    let path = run_folder.join(NOTE_FILE);
    match note {
        Some(note) => fs::write(path, note.text()),
        None => match fs::remove_file(path) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

pub fn read_note(run_folder: &Path) -> Option<ScreenNote> {
    ScreenNote::parse(&fs::read_to_string(run_folder.join(NOTE_FILE)).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::process;

    #[test]
    fn a_note_can_be_read_and_taken_back() {
        let folder = env::temp_dir().join(format!("guesthouse-display-note-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(read_note(&folder), None);
        write_note(&folder, None).unwrap();
        let note = ScreenNote { width: 3840, height: 2160, hdr: true, daemon: 7, frames: false };
        write_note(&folder, Some(note)).unwrap();
        assert_eq!(read_note(&folder), Some(note));
        let landed = ScreenNote { frames: true, ..note };
        write_note(&folder, Some(landed)).unwrap();
        assert_eq!(read_note(&folder), Some(landed));
        write_note(&folder, None).unwrap();
        assert_eq!(read_note(&folder), None);
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_note_that_makes_no_sense_counts_as_none() {
        for text in ["", "1920x1080", "1920x1080 sdr", "1920 sdr 7", "wide x high sdr 7", "1920x1080 maybe 7"]
        {
            assert_eq!(ScreenNote::parse(text), None, "{text:?}");
        }
    }
}
