//! What the daemon leaves in the run folder for the app. The app only reads files, and a VM whose GPU
//! work has stopped looks healthy from everywhere else.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

const NOTE_FILE: &str = "gpu-stopped";
const STOPPED: &str = "This VM's GPU has stopped, as too many programs in Windows faulted in a short time. \
                       Programs that use the GPU won't work until Windows restarts, and on the display driver \
                       the picture stays as it is. Shut Down or Force Off is the way out.";

/// Says whether this guest's GPU work has stopped. The note goes when it starts again.
pub fn write_note(run_folder: &Path, stopped: bool) -> io::Result<()> {
    let path = run_folder.join(NOTE_FILE);
    if stopped {
        return fs::write(path, STOPPED);
    }
    match fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The note, in words for the user, if there is one.
pub fn read_note(run_folder: &Path) -> Option<String> {
    fs::read_to_string(run_folder.join(NOTE_FILE)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::process;

    #[test]
    fn a_note_can_be_read_and_taken_back() {
        let folder = env::temp_dir().join(format!("guesthouse-note-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(read_note(&folder), None);
        write_note(&folder, false).unwrap();
        write_note(&folder, true).unwrap();
        assert!(read_note(&folder).unwrap().contains("until Windows restarts"));
        write_note(&folder, false).unwrap();
        assert_eq!(read_note(&folder), None);
        fs::remove_dir_all(&folder).unwrap();
    }
}
