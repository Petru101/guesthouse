//! What the launcher opens for the program and leaves at a number the program is told: the
//! bridge helper's device, and the unix sockets QEMU would otherwise make itself.

use super::UnixSocket;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
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
