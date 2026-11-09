//! What the launcher opens for the program and leaves at a number the program is told: the
//! bridge helper's device, passt's end of the network, and the unix sockets QEMU would otherwise make
//! itself.

use super::UnixSocket;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// The first descriptor past standard input, output and error.
const FIRST_PAST_STANDARD: libc::c_uint = 3;

pub(super) fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

/// A connected pair. Our end closes on exec and the other doesn't, as it's for the program the
/// launcher runs before the jail, the bridge helper or passt.
pub(super) fn socket_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut ends = [0; 2];
    // SAFETY: the kernel writes two descriptors into an array of two.
    checked(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, ends.as_mut_ptr()) })?;
    // SAFETY: both were just made for us, and nothing else holds them.
    let (ours, theirs) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    // SAFETY: a flag on a descriptor this owns.
    checked(unsafe { libc::fcntl(ours.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
    Ok((ours, theirs))
}

/// Has what `command` starts keep only its standard streams and `kept`. The launcher holds what's
/// meant for the program, the guest's memory among it, and a helper run first has no use for any of
/// that.
pub(super) fn keep_only(command: &mut Command, kept: RawFd) {
    // SAFETY: between fork and exec the closure makes only system calls, which are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            // Close on exec rather than close now, so the pipe the standard library reports a failed
            // exec through still works. A kernel older than 5.11 doesn't know the flag, and the
            // helper then gets what it always got.
            let range = (FIRST_PAST_STANDARD, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC);
            if libc::syscall(libc::SYS_close_range, range.0, range.1, range.2) == 0 {
                checked(libc::fcntl(kept, libc::F_SETFD, 0))?;
            }
            Ok(())
        });
    }
}

/// Puts `opened` at the number the program was told, open across the exec.
pub fn place(opened: OwnedFd, at: RawFd) -> io::Result<()> {
    if opened.as_raw_fd() == at {
        // SAFETY: a flag on a descriptor this owns, which is then left open on purpose.
        checked(unsafe { libc::fcntl(at, libc::F_SETFD, 0) })?;
        std::mem::forget(opened);
        return Ok(());
    }
    // SAFETY: dup2 makes a copy without the close-on-exec flag at a number of the caller's choosing.
    checked(unsafe { libc::dup2(opened.as_raw_fd(), at) })?;
    Ok(())
}

/// Opens a unix socket for the program and leaves it at the number it was told.
pub fn open_unix(socket: &UnixSocket) -> io::Result<()> {
    let with_path =
        |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", socket.path.display()));
    let opened: OwnedFd = if socket.listen {
        match std::fs::remove_file(&socket.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(with_path(error)),
            _ => {}
        }
        UnixListener::bind(&socket.path).map_err(with_path)?.into()
    } else {
        UnixStream::connect(&socket.path).map_err(with_path)?.into()
    };
    place(opened, socket.fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Which of these descriptors a shell started by `command` has open.
    fn open_in_child(command: &mut Command, numbers: [RawFd; 2]) -> Vec<RawFd> {
        let script = format!(
            "for n in {} {}; do if [ -e /proc/$$/fd/$n ]; then echo $n; fi; done",
            numbers[0], numbers[1]
        );
        let listing = command.args(["-c", &script]).stdout(Stdio::piped()).output().unwrap();
        String::from_utf8(listing.stdout).unwrap().split_whitespace().map(|n| n.parse().unwrap()).collect()
    }

    #[test]
    fn a_helper_keeps_its_own_end_and_nothing_meant_for_the_program() {
        let (_ours, theirs) = socket_pair().unwrap();
        let (_other, for_the_program) = socket_pair().unwrap();
        let numbers = [theirs.as_raw_fd(), for_the_program.as_raw_fd()];
        assert_eq!(open_in_child(&mut Command::new("sh"), numbers), numbers, "both, with nothing done");
        let mut helper = Command::new("sh");
        keep_only(&mut helper, theirs.as_raw_fd());
        assert_eq!(open_in_child(&mut helper, numbers), [theirs.as_raw_fd()]);
    }
}
