//! A bridged network's device, from QEMU's setuid helper, the way QEMU itself asks it: the helper
//! gets one end of a socket pair, and sends the open device back through it.

use super::Tap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::Command;

/// The helper's own words for what QEMU asks of it.
const OFFLOADS: &str = "--use-vnet";
/// One byte goes with the descriptor, as a message can't be empty.
const MESSAGE_BYTES: usize = 1;

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

/// Our end closes on exec and the helper's doesn't, as it's the helper's to use.
fn socket_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut ends = [0; 2];
    // SAFETY: the kernel writes two descriptors into an array of two.
    checked(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, ends.as_mut_ptr()) })?;
    // SAFETY: both were just made for us, and nothing else holds them.
    let (ours, theirs) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    // SAFETY: a flag on a descriptor this owns.
    checked(unsafe { libc::fcntl(ours.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
    Ok((ours, theirs))
}

fn receive_descriptor(socket: &OwnedFd) -> io::Result<OwnedFd> {
    // SAFETY: this only asks for a size.
    let space = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
    let mut control = vec![0u8; space];
    let mut byte = [0u8; MESSAGE_BYTES];
    let mut part = libc::iovec { iov_base: byte.as_mut_ptr().cast(), iov_len: byte.len() };
    // SAFETY: a zeroed msghdr is an empty one, and every pointer set below lives across the call.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len();
    // SAFETY: the message and what it points to are ours and valid for the call.
    let read = unsafe { libc::recvmsg(socket.as_raw_fd(), &raw mut message, libc::MSG_CMSG_CLOEXEC) };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the kernel has filled the control buffer and set its length. The header is checked to be
    // a descriptor before one is read from it.
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        if header.is_null()
            || (*header).cmsg_level != libc::SOL_SOCKET
            || (*header).cmsg_type != libc::SCM_RIGHTS
        {
            return Err(io::Error::other("the bridge helper sent no device"));
        }
        let fd = libc::CMSG_DATA(header).cast::<RawFd>().read_unaligned();
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

/// Puts `device` at the number QEMU was told, open across the exec.
fn place(device: OwnedFd, at: RawFd) -> io::Result<()> {
    if device.as_raw_fd() == at {
        // SAFETY: a flag on a descriptor this owns, which is then left open on purpose.
        checked(unsafe { libc::fcntl(at, libc::F_SETFD, 0) })?;
        std::mem::forget(device);
        return Ok(());
    }
    // SAFETY: dup2 makes a copy without the close-on-exec flag at a number of the caller's choosing.
    checked(unsafe { libc::dup2(device.as_raw_fd(), at) })?;
    Ok(())
}

/// Runs the helper and leaves its device open at the tap's number.
pub fn open(tap: &Tap) -> io::Result<()> {
    let (ours, theirs) = socket_pair()?;
    let mut helper = Command::new(&tap.helper)
        .args([OFFLOADS.to_owned(), format!("--fd={}", theirs.as_raw_fd()), format!("--br={}", tap.bridge)])
        .spawn()
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", tap.helper.display())))?;
    drop(theirs);
    let device = receive_descriptor(&ours);
    let done = helper.wait()?;
    if !done.success() {
        return Err(io::Error::other(format!("{} could not join {}", tap.helper.display(), tap.bridge)));
    }
    place(device?, tap.fd)
}
