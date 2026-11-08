//! passt, started before the jail. It builds a sandbox of its own with namespaces and mounts, which
//! can't be made inside Landlock, so it runs beside QEMU and not under it. It's handed one end of a
//! connected pair and QEMU gets the other, so QEMU needs no socket of its own to reach it, and passt
//! quits when QEMU's end closes.

use super::Passt;
use super::handover::{place, socket_pair};
use std::io;
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};

/// passt's word for the connected socket it's given, which also has it quit after that one client.
const SOCKET_WORD: &str = "--fd";

/// Starts passt and leaves QEMU's end at the number it was told. passt puts itself in the
/// background once it's ready, so by the time this returns it's there to answer.
pub fn start(passt: &Passt) -> io::Result<()> {
    let (ours, theirs) = socket_pair()?;
    let with_name =
        |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", passt.program.display()));
    let done = Command::new(&passt.program)
        .args(&passt.words)
        .args([SOCKET_WORD, &theirs.as_raw_fd().to_string()])
        .stdin(Stdio::null())
        .status()
        .map_err(with_name)?;
    drop(theirs);
    if !done.success() {
        return Err(io::Error::other(format!("{} did not start: {done}", passt.program.display())));
    }
    place(ours, passt.fd)
}
