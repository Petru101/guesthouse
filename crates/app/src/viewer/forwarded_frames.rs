//! Frames from an Ubuntu guest on the GPU. The nvfwd device hands over its buffers once on a socket, and
//! says which is newest in a small shared file. A slot this side is reading has to be marked, as the
//! device and this side work on the same slots at once.
//!
//! This is the other side of the device's own protocol, so its offsets and order of steps are fixed.
//! Each read or write of the shared file is a system call, which orders them as the protocol needs.

use super::frames::FrameSource;
use super::import::texture;
use gtk::gdk;
use guesthouse_app_core::vm_args::FRAMES_PREFIX;
use guesthouse_frame_socket::{Colour, Descriptor, Format};
use rustix::io::{pread, pwrite};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

/// Words in the shared file.
const LATEST: u64 = 0x40;
const HELD: u64 = 0x48;
const CLAIMED: u64 = 0x50;
const READING: u64 = 0x58;
const PRESENT: u64 = 0x80;
/// Slots to hold: the frame on screen, and the two before it, which the GPU may still read.
const KEPT: usize = 3;
const MOST_SLOTS: usize = 16;
/// A set of same-sized buffers, or one of a compositor's framebuffers.
const BUFFERS_MESSAGE: usize = 16;
const SURFACE_MESSAGE: usize = 32;
const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const SLOT_MASK: u32 = 0xFF;

#[derive(Clone)]
struct Buffer {
    file: Arc<OwnedFd>,
    width: u32,
    height: u32,
    stride: u32,
    format: Format,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default())
}

/// What one message from the device says, as the buffers it replaces.
fn take_message(bytes: &[u8], fds: Vec<OwnedFd>, buffers: &mut Vec<Option<Buffer>>, recent: &mut Vec<usize>) {
    if bytes.len() == SURFACE_MESSAGE {
        let Some(fd) = fds.into_iter().next() else { return };
        let slot = (word(bytes, 12) & SLOT_MASK) as usize;
        let modifier = u64::from(word(bytes, 24)) << 32 | u64::from(word(bytes, 20));
        let format = Format { fourcc: word(bytes, 16), modifier };
        let (width, height, stride) = (word(bytes, 0), word(bytes, 4), word(bytes, 8));
        if buffers.len() <= slot {
            buffers.resize(slot + 1, None);
        }
        buffers[slot] = Some(Buffer { file: Arc::new(fd), width, height, stride, format });
    } else if bytes.len() == BUFFERS_MESSAGE {
        let (width, height, stride) = (word(bytes, 0), word(bytes, 4), word(bytes, 8));
        let format = Format { fourcc: DRM_FORMAT_XRGB8888, modifier: 0 };
        let each = |fd| Some(Buffer { file: Arc::new(fd), width, height, stride, format });
        *buffers = fds.into_iter().map(each).collect();
        recent.clear();
    }
}

pub struct ForwardedFrames {
    socket: UnixStream,
    shared: File,
    display: gdk::Display,
    buffers: Vec<Option<Buffer>>,
    recent: Vec<usize>,
    shown: u64,
    alive: bool,
}

impl ForwardedFrames {
    /// Fails until the guest's device is offering frames.
    pub fn connect(run_folder: &Path, display: &gdk::Display) -> io::Result<Self> {
        let shared = OpenOptions::new()
            .read(true)
            .write(true)
            .open(run_folder.join(format!("{FRAMES_PREFIX}.display")))?;
        let socket = UnixStream::connect(run_folder.join(format!("{FRAMES_PREFIX}.sock")))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            shared,
            display: display.clone(),
            buffers: Vec::new(),
            recent: Vec::new(),
            shown: 0,
            alive: true,
        })
    }

    fn read_word(&self, at: u64) -> u64 {
        let mut bytes = [0; 8];
        match pread(&self.shared, &mut bytes, at) {
            Ok(_) => u64::from_le_bytes(bytes),
            Err(_) => 0,
        }
    }

    fn write_word(&self, at: u64, value: &[u8]) {
        let _ = pwrite(&self.shared, value, at);
    }

    /// Takes in all that is waiting: buffers from a message not yet read aren't to be shown.
    fn receive(&mut self) {
        loop {
            let mut bytes = [0; SURFACE_MESSAGE];
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MOST_SLOTS))];
            let mut control = RecvAncillaryBuffer::new(&mut space);
            let mut parts = [IoSliceMut::new(&mut bytes)];
            let received = recvmsg(&self.socket, &mut parts, &mut control, RecvFlags::CMSG_CLOEXEC);
            let mut fds = Vec::new();
            for item in control.drain() {
                if let RecvAncillaryMessage::ScmRights(rights) = item {
                    fds.extend(rights);
                }
            }
            match received.map_err(io::Error::from) {
                Ok(result) if result.bytes > 0 => {
                    take_message(&bytes[..result.bytes], fds, &mut self.buffers, &mut self.recent)
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                _ => {
                    self.alive = false;
                    return;
                }
            }
        }
    }

    fn publish_held(&self) {
        let mask = self.recent.iter().fold(0u32, |mask, slot| mask | 1 << slot);
        self.write_word(HELD, &mask.to_le_bytes());
    }

    /// Takes hold of a slot, and says whether its picture can be shown. Held is written before claimed
    /// is read, and the device does the opposite, so one of the two sees the other.
    fn hold(&mut self, slot: usize, latest: u64) -> bool {
        self.recent.push(slot);
        if self.recent.len() > KEPT {
            self.recent.remove(0);
        }
        self.publish_held();
        if self.read_word(CLAIMED) >> slot & 1 == 1 {
            self.recent.pop();
            self.publish_held();
            return false;
        }
        // A compositor's frame has no slot to hold, so the guest is told which one is being read. If it
        // missed that, a newer frame shows here, and this one is left alone.
        let present = self.read_word(PRESENT + slot as u64 * 8);
        self.write_word(READING, &present.to_le_bytes());
        self.read_word(LATEST) == latest
    }
}

impl FrameSource for ForwardedFrames {
    fn newest(&mut self) -> Option<gdk::Texture> {
        self.receive();
        let latest = self.read_word(LATEST);
        let (sequence, slot) = (latest >> 8, (latest & u64::from(SLOT_MASK)) as usize);
        let buffer = self.buffers.get(slot).cloned().flatten()?;
        if sequence == self.shown || !self.hold(slot, latest) {
            return None;
        }
        self.shown = sequence;
        let descriptor = Descriptor {
            width: buffer.width,
            height: buffer.height,
            stride: buffer.stride,
            offset: 0,
            format: buffer.format,
            colour: Colour::SRGB,
        };
        texture(&self.display, buffer.file, &descriptor, Box::new(|| {}))
    }

    fn alive(&self) -> bool {
        self.alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{MemfdFlags, memfd_create};

    fn file() -> OwnedFd {
        memfd_create("slot", MemfdFlags::CLOEXEC).unwrap()
    }

    fn message(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    fn a_set_of_buffers_replaces_what_was_there() {
        let (mut buffers, mut recent) = (vec![None], vec![0]);
        take_message(&message(&[640, 360, 2560, 0]), vec![file(), file()], &mut buffers, &mut recent);
        assert_eq!(buffers.len(), 2);
        assert!(recent.is_empty());
        let first = buffers[0].as_ref().unwrap();
        assert_eq!((first.width, first.height, first.stride), (640, 360, 2560));
        assert_eq!(first.format.fourcc, DRM_FORMAT_XRGB8888);
    }

    #[test]
    fn a_compositors_framebuffer_takes_its_slot_and_says_its_format() {
        let (mut buffers, mut recent) = (Vec::new(), Vec::new());
        let words = [1920, 1080, 7680, 0x102, 0x3432_5241, 0x10, 0x0300_0000, 0];
        take_message(&message(&words), vec![file()], &mut buffers, &mut recent);
        assert_eq!(buffers.len(), 3);
        let found = buffers[2].as_ref().unwrap();
        assert_eq!(found.format, Format { fourcc: 0x3432_5241, modifier: 0x0300_0000_0000_0010 });
    }

    #[test]
    fn a_message_of_no_known_size_is_left_out() {
        let (mut buffers, mut recent) = (Vec::new(), Vec::new());
        take_message(&message(&[1, 2, 3]), vec![file()], &mut buffers, &mut recent);
        assert!(buffers.is_empty());
    }
}
