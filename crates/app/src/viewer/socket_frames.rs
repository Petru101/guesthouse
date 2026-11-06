//! Frames from the frame socket. A slot is this side's from its frame until its release, and a release
//! goes when GTK has let go of the texture, which it can do on any thread.

use super::frames::FrameSource;
use super::guest_cursor::{CursorPicture, no_pointer};
use super::import::texture;
use super::ring::Buffers;
use gtk::gdk;
use gtk::prelude::*;
use guesthouse_frame_socket::{
    CursorMove, Format, Frame, FrameSocket, Hello, Light, Message, Packet, Release, SOCKET_NAME,
};
use std::io;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

const LINEAR: u64 = 0;
/// What a copy can show, so the producer always has something it can send.
const COPYABLE: [u32; 2] = [u32::from_le_bytes(*b"XR24"), u32::from_le_bytes(*b"AR24")];
/// What an HDR desktop is. A guest only sends it to a viewer that offers it, and a build with no way to
/// give a frame its colour doesn't.
const HDR_FORMAT: Format = Format { fourcc: u32::from_le_bytes(*b"XB30"), modifier: LINEAR };

pub struct SocketFrames {
    socket: FrameSocket,
    display: gdk::Display,
    buffers: Option<Buffers>,
    finished: Sender<Release>,
    releases: Receiver<Release>,
    alive: bool,
    /// How the newest HDR picture wants to be shown. An SDR picture has none.
    light: Option<Light>,
    /// The guest's hardware cursor, if it has one.
    cursor: Option<CursorPicture>,
    cursor_at: Option<CursorMove>,
    /// The pointer over the screen no longer matches the cursor.
    cursor_changed: bool,
}

/// What this display can show, in the producer's order of preference. The message holds a limited
/// number and a display can offer hundreds, so the formats a guest's desktop uses go first.
fn formats(display: &gdk::Display) -> Vec<Format> {
    let dmabuf = display.dmabuf_formats();
    let offered = (0..dmabuf.n_formats()).map(|index| {
        let (fourcc, modifier) = dmabuf.format(index);
        Format { fourcc, modifier }
    });
    in_order(offered.collect())
}

/// HDR is only offered if the display has it, and it comes early, as the list can be cut short.
fn in_order(offered: Vec<Format>) -> Vec<Format> {
    let hdr = (cfg!(feature = "hdr") && offered.contains(&HDR_FORMAT)).then_some(HDR_FORMAT);
    let (usual, others): (Vec<Format>, Vec<Format>) =
        offered.into_iter().partition(|format| COPYABLE.contains(&format.fourcc));
    let linear = COPYABLE.map(|fourcc| Format { fourcc, modifier: LINEAR });
    let mut all = linear.to_vec();
    all.extend(hdr);
    let rest = usual.into_iter().chain(others);
    all.extend(rest.filter(|format| !linear.contains(format) && *format != HDR_FORMAT));
    all
}

impl SocketFrames {
    /// Fails if nothing is offering frames yet.
    pub fn connect(run_folder: &Path, display: &gdk::Display, refresh_millihertz: u32) -> io::Result<Self> {
        let socket = FrameSocket::connect(&run_folder.join(SOCKET_NAME))?;
        let hello = Hello { formats: formats(display), refresh_millihertz, peak_nits: 0 };
        socket.send(&Message::Hello(hello), &[])?;
        socket.send(&Message::CursorWanted, &[])?;
        socket.set_nonblocking(true)?;
        let (finished, releases) = channel();
        Ok(Self {
            socket,
            display: display.clone(),
            buffers: None,
            finished,
            releases,
            alive: true,
            light: None,
            cursor: None,
            cursor_at: None,
            cursor_changed: false,
        })
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
                Ok(Some(Packet { message: Message::CursorShape(shape), fds })) => {
                    self.cursor = fds.into_iter().next().and_then(|file| CursorPicture::read(&shape, file));
                    self.cursor_changed = true;
                }
                Ok(Some(Packet { message: Message::CursorMove(position), .. })) => {
                    let shown = self.cursor_at.is_some_and(|before| before.visible);
                    self.cursor_changed |= shown != position.visible;
                    self.cursor_at = Some(position);
                }
                Ok(Some(Packet { message: Message::Light(light), .. })) => {
                    self.light = Some(light);
                }
                Ok(Some(Packet { message: Message::Frame(frame), .. })) => {
                    if self.light.is_some_and(|light| light.slot != frame.slot) {
                        self.light = None;
                    }
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

    fn sdr_white_nits(&self) -> Option<u32> {
        self.light.map(|light| light.sdr_white_nits)
    }

    fn drawn_cursor(&self) -> Option<(CursorPicture, CursorMove)> {
        let position = self.cursor_at.filter(|position| position.visible)?;
        Some((self.cursor.clone()?, position))
    }

    fn new_cursor(&mut self) -> Option<gdk::Cursor> {
        if !std::mem::take(&mut self.cursor_changed) {
            return None;
        }
        match (&self.cursor, self.cursor_at) {
            (_, Some(position)) if !position.visible => no_pointer(),
            (Some(cursor), _) => Some(cursor.pointer()),
            (None, _) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(name: &[u8; 4], modifier: u64) -> Format {
        Format { fourcc: u32::from_le_bytes(*name), modifier }
    }

    #[test]
    fn what_every_guest_can_send_comes_first() {
        let found = in_order(vec![format(b"NV12", 0), format(b"XR24", 7)]);
        assert_eq!(found[0], format(b"XR24", LINEAR));
        assert_eq!(found[1], format(b"AR24", LINEAR));
    }

    #[test]
    #[cfg(feature = "hdr")]
    fn hdr_is_offered_only_by_a_display_that_has_it() {
        assert!(!in_order(vec![format(b"XR24", 0)]).contains(&HDR_FORMAT));
        let found = in_order(vec![format(b"NV12", 0), HDR_FORMAT]);
        assert_eq!(found[2], HDR_FORMAT);
        assert_eq!(found.iter().filter(|format| **format == HDR_FORMAT).count(), 1);
    }

    #[test]
    #[cfg(not(feature = "hdr"))]
    fn a_build_with_no_colour_never_offers_hdr() {
        assert!(!in_order(vec![HDR_FORMAT]).contains(&HDR_FORMAT));
    }

    #[test]
    fn hdr_with_a_tiled_layout_is_not_enough() {
        assert!(!in_order(vec![format(b"XB30", 7)]).contains(&HDR_FORMAT));
    }
}
