//! What the launcher opens for the program and leaves at a number the program is told: the
//! bridge helper's device, passt's end of the network, and the unix sockets QEMU would otherwise make
//! itself.

use super::UnixSocket;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

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
