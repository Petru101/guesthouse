//! The socket itself: one message a packet, with file descriptors beside a ring.

use crate::message::{DecodeError, MOST_SLOTS, Message};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::io::Errno;
use rustix::net::{
    self, AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
    SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketFlags, SocketType,
};
use std::fs;
use std::io::{self, ErrorKind, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

/// What the socket is called in a VM's run folder.
pub const SOCKET_NAME: &str = "frames.sock";
/// Larger than any message.
const MOST_PACKET: usize = 2048;
const LISTEN_BACKLOG: i32 = 1;

/// A message, and the file descriptors that came with it.
#[derive(Debug)]
pub struct Packet {
    pub message: Message,
    pub fds: Vec<OwnedFd>,
}

fn invalid(error: DecodeError) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, error)
}

fn new_socket() -> io::Result<OwnedFd> {
    Ok(net::socket_with(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?)
}

/// The producer's end, before a viewer has come.
pub struct FrameListener(OwnedFd);

impl FrameListener {
    /// Replaces a socket left by an earlier run.
    pub fn bind(path: &Path) -> io::Result<Self> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let socket = new_socket()?;
        net::bind(&socket, &SocketAddrUnix::new(path)?)?;
        net::listen(&socket, LISTEN_BACKLOG)?;
        Ok(Self(socket))
    }

    pub fn accept(&self) -> io::Result<FrameSocket> {
        Ok(FrameSocket(net::accept_with(&self.0, SocketFlags::CLOEXEC)?))
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(&self.0, nonblocking)
    }
}

impl AsFd for FrameListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

fn set_nonblocking(fd: impl AsFd, nonblocking: bool) -> io::Result<()> {
    let flags = fcntl_getfl(&fd)?;
    let flags = if nonblocking { flags | OFlags::NONBLOCK } else { flags - OFlags::NONBLOCK };
    Ok(fcntl_setfl(&fd, flags)?)
}

pub struct FrameSocket(OwnedFd);

impl FrameSocket {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let socket = new_socket()?;
        net::connect(&socket, &SocketAddrUnix::new(path)?)?;
        Ok(Self(socket))
    }

    /// Two ends joined together, for a test or a viewer's stand-in.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (one, other) =
            net::socketpair(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?;
        Ok((Self(one), Self(other)))
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(&self.0, nonblocking)
    }

    pub fn send(&self, message: &Message, fds: &[BorrowedFd]) -> io::Result<()> {
        let bytes = message.encode();
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MOST_SLOTS as usize))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        if !fds.is_empty() && !control.push(SendAncillaryMessage::ScmRights(fds)) {
            return Err(ErrorKind::InvalidInput.into());
        }
        net::sendmsg(&self.0, &[IoSlice::new(&bytes)], &mut control, SendFlags::NOSIGNAL)?;
        Ok(())
    }

    /// The next message. `None` if the socket is non-blocking and nothing has come. A peer that has
    /// gone is an `UnexpectedEof` error.
    pub fn receive(&self) -> io::Result<Option<Packet>> {
        let mut bytes = [0; MOST_PACKET];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MOST_SLOTS as usize))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let mut buffers = [IoSliceMut::new(&mut bytes)];
        let received = match net::recvmsg(&self.0, &mut buffers, &mut control, RecvFlags::CMSG_CLOEXEC) {
            Ok(received) => received,
            Err(Errno::AGAIN) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // They're ours now, and go with this packet whatever else is wrong with it.
        let mut fds = Vec::new();
        for item in control.drain() {
            if let RecvAncillaryMessage::ScmRights(rights) = item {
                fds.extend(rights);
            }
        }
        if received.bytes == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        if received.flags.intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC) {
            return Err(ErrorKind::InvalidData.into());
        }
        let message = Message::decode(&bytes[..received.bytes]).map_err(invalid)?;
        Ok(Some(Packet { message, fds }))
    }
}

impl AsFd for FrameSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Release, Ring};
    use rustix::fs::{MemfdFlags, memfd_create};
    use std::fs::File;
    use std::io::{Read, Seek, Write};
    use std::thread;

    #[test]
    fn a_ring_brings_its_buffers() {
        let (producer, viewer) = FrameSocket::pair().unwrap();
        let mut buffer = File::from(memfd_create("slot", MemfdFlags::CLOEXEC).unwrap());
        buffer.write_all(b"pixels").unwrap();
        let ring = Message::Ring(Ring { generation: 1, slots: 1, size: 6 });
        producer.send(&ring, &[buffer.as_fd()]).unwrap();
        let packet = viewer.receive().unwrap().unwrap();
        assert_eq!(packet.message, ring);
        let mut shared = File::from(packet.fds.into_iter().next().unwrap());
        shared.rewind().unwrap();
        let mut text = String::new();
        shared.read_to_string(&mut text).unwrap();
        assert_eq!(text, "pixels", "it is the same memory");
    }

    #[test]
    fn with_nothing_waiting_a_non_blocking_socket_says_so() {
        let (_producer, viewer) = FrameSocket::pair().unwrap();
        viewer.set_nonblocking(true).unwrap();
        assert!(viewer.receive().unwrap().is_none());
    }

    #[test]
    fn a_peer_that_goes_is_an_end_of_file() {
        let (producer, viewer) = FrameSocket::pair().unwrap();
        drop(producer);
        assert_eq!(viewer.receive().unwrap_err().kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn each_message_stays_its_own_packet() {
        let (producer, viewer) = FrameSocket::pair().unwrap();
        for slot in 0..3 {
            producer.send(&Message::Release(Release { generation: 1, slot }), &[]).unwrap();
        }
        for slot in 0..3 {
            assert_eq!(
                viewer.receive().unwrap().unwrap().message,
                Message::Release(Release { generation: 1, slot })
            );
        }
    }

    #[test]
    fn a_viewer_connects_by_path_and_an_old_socket_is_replaced() {
        let folder = std::env::temp_dir().join(format!("frame-socket-test-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(SOCKET_NAME);
        fs::write(&path, "left by an earlier run").unwrap();
        let listener = FrameListener::bind(&path).unwrap();
        let connecting = path.clone();
        let viewer = thread::spawn(move || FrameSocket::connect(&connecting).unwrap());
        let producer = listener.accept().unwrap();
        let message = Message::Release(Release { generation: 2, slot: 0 });
        viewer.join().unwrap().send(&message, &[]).unwrap();
        assert_eq!(producer.receive().unwrap().unwrap().message, message);
        fs::remove_dir_all(folder).unwrap();
    }
}
