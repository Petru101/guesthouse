//! The daemon's end of the frame socket.
//!
//! Everything here runs from the main loop and the socket never blocks, so a viewer that stops
//! reading, or goes, can't hold up the guest. A viewer is only ever a peer that might go away: any
//! trouble with one drops it and nothing else. The session keeps the books on slots, and this file
//! only moves messages.

use crate::server::OWNER_ONLY;
use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_core::{Delivery, Frame as ReadyFrame, FrameRing, FrameSink, Session};
use guesthouse_frame_socket::{
    Colour, Descriptor, Format, Frame, FrameListener, FrameSocket, Hello, Message, Packet, Ring, SOCKET_NAME,
};
use std::fs::{self, Permissions};
use std::io::{self, ErrorKind};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// DRM's name for 8-bit pixels, blue first in memory, in rows from top to bottom. That's what an
/// SDR Windows desktop is. HDR comes with the indirect display driver.
const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const LINEAR: u64 = 0;
const SHOWN_FORMAT: Format = Format { fourcc: XRGB8888, modifier: LINEAR };
/// How many messages one viewer gets dealt with in a turn, so that one sending as fast as it can
/// leaves room for the guest. The rest are still there next time round.
const MOST_MESSAGES_A_TURN: usize = 64;
const NO_DESCRIPTOR: i32 = -1;

struct Viewer {
    socket: FrameSocket,
    /// It has said hello, so the session knows of it.
    greeted: bool,
    /// The socket had no room, so writability is worth waking for.
    blocked: bool,
}

/// The listening socket, and the viewer if there is one. A new viewer takes the place of an old one.
pub struct ViewerPort {
    listener: FrameListener,
    viewer: Option<Viewer>,
}

fn can_show(hello: &Hello) -> bool {
    hello.formats.contains(&SHOWN_FORMAT)
}

impl ViewerPort {
    /// Listens in `run_folder`, replacing a socket a previous daemon left behind.
    pub fn bind(run_folder: &Path) -> io::Result<Self> {
        let path = run_folder.join(SOCKET_NAME);
        let listener = FrameListener::bind(&path)?;
        fs::set_permissions(path, Permissions::from_mode(OWNER_ONLY))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, viewer: None })
    }

    /// What to wait on, as (the listener, the viewer's descriptor and whether it matters if it can be
    /// written).
    pub fn wait_on(&self) -> (i32, i32, bool) {
        let viewer = self.viewer.as_ref();
        let descriptor = viewer.map_or(NO_DESCRIPTOR, |viewer| viewer.socket.as_fd().as_raw_fd());
        (self.listener.as_fd().as_raw_fd(), descriptor, viewer.is_some_and(|viewer| viewer.blocked))
    }

    /// Takes on whoever is connecting.
    pub fn accept<B: Backend, M: GuestMemory>(&mut self, session: &mut Session<B, M>) {
        let socket = self.listener.accept().and_then(|socket| {
            socket.set_nonblocking(true)?;
            Ok(socket)
        });
        match socket {
            Ok(socket) => {
                self.drop_viewer(session);
                log::info!("a viewer connected");
                self.viewer = Some(Viewer { socket, greeted: false, blocked: false });
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => log::warn!("accepting a viewer: {error}"),
        }
    }

    fn drop_viewer<B: Backend, M: GuestMemory>(&mut self, session: &mut Session<B, M>) {
        if self.viewer.take().is_some_and(|viewer| viewer.greeted) {
            session.viewer_left();
        }
    }

    /// Deals with what the viewer has sent.
    pub fn read<B: Backend, M: GuestMemory>(&mut self, session: &mut Session<B, M>) {
        for _ in 0..MOST_MESSAGES_A_TURN {
            let Some(viewer) = self.viewer.as_mut() else { return };
            match viewer.socket.receive() {
                Ok(None) => return,
                Ok(Some(packet)) => {
                    if !Self::handle(viewer, packet, session) {
                        self.drop_viewer(session);
                    }
                }
                Err(error) => {
                    if error.kind() != ErrorKind::UnexpectedEof {
                        log::warn!("dropping the viewer: {error}");
                    }
                    self.drop_viewer(session);
                }
            }
        }
    }

    /// Says whether this viewer can stay.
    fn handle<B: Backend, M: GuestMemory>(
        viewer: &mut Viewer,
        packet: Packet,
        session: &mut Session<B, M>,
    ) -> bool {
        match packet.message {
            Message::Hello(hello) if !viewer.greeted && can_show(&hello) => {
                log::info!("the viewer offers {} formats", hello.formats.len());
                viewer.greeted = true;
                session.viewer_arrived();
                true
            }
            Message::Hello(_) if !viewer.greeted => {
                log::warn!("the viewer can't show XRGB8888, which is all there is for now");
                false
            }
            Message::Release(release) if viewer.greeted => {
                session.slot_released(release.generation, release.slot);
                true
            }
            other => {
                log::warn!("dropping a viewer that sent {other:?} out of turn");
                false
            }
        }
    }

    /// Sends one message. The session hears of a viewer that has gone from what this returns.
    fn send(&mut self, message: &Message, fds: &[BorrowedFd<'_>]) -> Delivery {
        let Some(viewer) = self.viewer.as_mut() else { return Delivery::Gone };
        viewer.blocked = false;
        match viewer.socket.send(message, fds) {
            Ok(()) => Delivery::Sent,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                viewer.blocked = true;
                Delivery::NotNow
            }
            Err(error) => {
                log::info!("the viewer has gone: {error}");
                self.viewer = None;
                Delivery::Gone
            }
        }
    }
}

impl FrameSink for ViewerPort {
    fn ring_opened(&mut self, ring: &FrameRing, buffers: &[BorrowedFd<'_>]) -> Delivery {
        let ring = Ring { generation: ring.generation, slots: ring.slots, size: ring.size };
        self.send(&Message::Ring(ring), buffers)
    }

    fn frame_ready(&mut self, frame: &ReadyFrame) -> Delivery {
        let picture = frame.picture;
        let descriptor = Descriptor {
            width: picture.width,
            height: picture.height,
            stride: picture.pitch,
            offset: 0,
            format: SHOWN_FORMAT,
            colour: Colour::SRGB,
        };
        let frame =
            Frame { generation: frame.generation, slot: frame.slot, sequence: frame.sequence, descriptor };
        self.send(&Message::Frame(frame), &[])
    }
}
