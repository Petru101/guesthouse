//! Frames from the frame socket. A slot is this side's from its frame until its release, and a release
//! goes when GTK has let go of the texture, which it can do on any thread.

use super::frames::FrameSource;
use super::import::texture;
use super::ring::Buffers;
use gtk::gdk;
use gtk::prelude::*;
use guesthouse_frame_socket::{Format, Frame, FrameSocket, Hello, Message, Packet, Release, SOCKET_NAME};
use std::io;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

const LINEAR: u64 = 0;
/// What a copy can show, so the producer always has something it can send.
const COPYABLE: [u32; 2] = [u32::from_le_bytes(*b"XR24"), u32::from_le_bytes(*b"AR24")];

pub struct SocketFrames {
    socket: FrameSocket,
    display: gdk::Display,
    buffers: Option<Buffers>,
    finished: Sender<Release>,
    releases: Receiver<Release>,
    alive: bool,
}

fn formats(display: &gdk::Display) -> Vec<Format> {
    let dmabuf = display.dmabuf_formats();
    let usable = (0..dmabuf.n_formats()).map(|index| {
        let (fourcc, modifier) = dmabuf.format(index);
        Format { fourcc, modifier }
    });
    let linear = COPYABLE.map(|fourcc| Format { fourcc, modifier: LINEAR });
    let mut all: Vec<Format> = usable.chain(linear).collect();
    all.dedup();
    all
}

impl SocketFrames {
    /// Fails if nothing is offering frames yet.
    pub fn connect(run_folder: &Path, display: &gdk::Display, refresh_millihertz: u32) -> io::Result<Self> {
        let socket = FrameSocket::connect(&run_folder.join(SOCKET_NAME))?;
        let hello = Hello { formats: formats(display), refresh_millihertz, peak_nits: 0 };
        socket.send(&Message::Hello(hello), &[])?;
        socket.set_nonblocking(true)?;
        let (finished, releases) = channel();
        Ok(Self { socket, display: display.clone(), buffers: None, finished, releases, alive: true })
    }

    fn release(&mut self, generation: u32, slot: u32) {
        let message = Message::Release(Release { generation, slot });
        if self.socket.send(&message, &[]).is_err() {
            self.alive = false;
        }
    }

    fn send_releases(&mut self) {
        while let Ok(release) = self.releases.try_recv() {
            self.release(release.generation, release.slot);
        }
    }

    /// Takes in everything waiting, and says which frame is the newest. An older one has been passed
    /// over, so its slot goes straight back.
    fn take_messages(&mut self) -> Option<Frame> {
        let mut newest: Option<Frame> = None;
        loop {
            match self.socket.receive() {
                Ok(None) => return newest,
                Ok(Some(Packet { message: Message::Ring(ring), fds })) => {
                    self.buffers = Buffers::new(&ring, fds);
                    newest = None;
                }
                Ok(Some(Packet { message: Message::Frame(frame), .. })) => {
                    if let Some(older) = newest.replace(frame) {
                        self.release(older.generation, older.slot);
                    }
                }
                Ok(Some(_)) => {}
                Err(_) => {
                    self.alive = false;
                    return newest;
                }
            }
        }
    }
}

impl FrameSource for SocketFrames {
    fn newest(&mut self) -> Option<gdk::Texture> {
        self.send_releases();
        let frame = self.take_messages()?;
        let Some(file) = self.buffers.as_ref().and_then(|buffers| buffers.slot(&frame)) else {
            self.release(frame.generation, frame.slot);
            return None;
        };
        let finished = self.finished.clone();
        let released = Release { generation: frame.generation, slot: frame.slot };
        let on_release = Box::new(move || {
            let _ = finished.send(released);
        });
        texture(&self.display, file, &frame.descriptor, on_release)
    }

    fn alive(&self) -> bool {
        self.alive
    }
}
