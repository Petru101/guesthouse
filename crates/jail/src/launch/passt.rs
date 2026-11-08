//! passt, started before the jail. It builds a sandbox of its own with namespaces and mounts, which
//! can't be made inside Landlock, so it runs beside QEMU and not under it. It's handed one end of a
//! connected pair and QEMU gets the other, so QEMU needs no socket of its own to reach it, and passt
//! quits when QEMU's end closes.
//!
//! It stays in the foreground, as QEMU's child once the launcher becomes QEMU, so it's in QEMU's
//! process group and goes when that's stopped. The launcher writes its pid file itself: passt's own
//! may not be allowed to write where the run folder is, as on Fedora, where SELinux holds passt to a
//! few places.

use super::Passt;
use super::handover::{place, socket_pair};
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// passt's word for the connected socket it's given, which also has it quit after that one client.
const SOCKET_WORD: &str = "--fd";
const FOREGROUND_WORD: &str = "--foreground";
/// passt checks what it was told before anything else, so a mistake shows by then.
const EARLY_EXIT: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(20);
/// Where the start time is among a process's `stat` fields, after its name.
const START_TIME_FIELD: usize = 19;

/// When a process started, in the kernel's clock ticks since boot, which tells it apart from a later
/// one given the same pid.
pub fn start_time(stat: &str) -> Option<u64> {
    let after_name = stat.rsplit_once(')')?.1;
    after_name.split_whitespace().nth(START_TIME_FIELD)?.parse().ok()
}

/// Starts passt and leaves QEMU's end at the number it was told, and notes its pid and start time.
pub fn start(passt: &Passt) -> io::Result<()> {
    let (ours, theirs) = socket_pair()?;
    let with_name =
        |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", passt.program.display()));
    let mut child = Command::new(&passt.program)
        .args(&passt.words)
        .args([FOREGROUND_WORD, SOCKET_WORD, &theirs.as_raw_fd().to_string()])
        .stdin(Stdio::null())
        .spawn()
        .map_err(with_name)?;
    drop(theirs);
    let stat = fs::read_to_string(format!("/proc/{}/stat", child.id()))?;
    let started = start_time(&stat).ok_or_else(|| io::Error::other("passt's start time can't be read"))?;
    fs::write(&passt.pid_file, format!("{} {started}\n", child.id()))?;
    let end = Instant::now() + EARLY_EXIT;
    while Instant::now() < end {
        if let Some(done) = child.try_wait()? {
            return Err(io::Error::other(format!("{} did not start: {done}", passt.program.display())));
        }
        thread::sleep(POLL);
    }
    place(ours, passt.fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_time_is_found_after_a_name_with_spaces_and_brackets() {
        let stat = "4242 (passt (x) y) S 1 4242 4242 0 -1 4194560 92 0 0 0 0 0 0 0 20 0 1 0 987654 1 2 3";
        assert_eq!(start_time(stat), Some(987654));
        assert_eq!(start_time("4242 (passt) S 1"), None);
    }
}
