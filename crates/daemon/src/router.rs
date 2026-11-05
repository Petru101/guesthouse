//! From frames to the session and back.

use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_core::Session;
use guesthouse_messages::strip_extended_header;
use guesthouse_transport::{Frame, FrameHeader, FrameKind, Truncated, split_ranges};
use std::io;
use std::time::Instant;

/// The two VMBus channels QEMU opens for each VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Global,
    Vgpu,
}

impl Channel {
    pub const ALL: [Self; 2] = [Self::Global, Self::Vgpu];

    pub fn socket_name(self) -> &'static str {
        match self {
            Self::Global => "global.sock",
            Self::Vgpu => "vgpu.sock",
        }
    }
}

/// Where frames for the guest go.
pub trait FrameSink {
    fn send(&mut self, channel: Channel, header: FrameHeader, payload: &[u8]) -> io::Result<()>;
}

/// Runs frames through a session, with buffers that last between messages.
pub struct Router<B, M> {
    session: Session<B, M>,
    reply: Vec<u8>,
    notice: Vec<u8>,
}

const INITIAL_REPLY_CAPACITY: usize = 64 << 10;

impl<B: Backend, M: GuestMemory> Router<B, M> {
    pub fn new(session: Session<B, M>) -> Self {
        Self { session, reply: Vec::with_capacity(INITIAL_REPLY_CAPACITY), notice: Vec::new() }
    }

    pub fn session(&self) -> &Session<B, M> {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session<B, M> {
        &mut self.session
    }

    /// Handles one frame. An error means the frame cannot be read, which only a broken peer causes.
    pub fn handle_frame(
        &mut self,
        channel: Channel,
        frame: Frame<'_>,
        sink: &mut impl FrameSink,
    ) -> Result<(), RouterError> {
        let header = frame.header;
        if header.kind.is_lifecycle_event() {
            log::info!("{channel:?}: {:?}", header.kind);
            return Ok(());
        }
        let message = if header.flags.has_ranges() { split_ranges(frame.payload)?.1 } else { frame.payload };
        if header.kind == FrameKind::Completion {
            return Ok(());
        }
        self.session.handle_message(strip_extended_header(message), &mut self.reply);
        // A handler's notices go out before its reply, the order the guest has always seen.
        self.send_notices(sink)?;
        if header.flags.wants_completion() {
            let completion = FrameHeader::completion(header.transaction, self.reply.len());
            sink.send(channel, completion, &self.reply)?;
        }
        Ok(())
    }

    /// Checks what is waiting on fences. True if something still is.
    pub fn poll(&mut self, now: Instant, sink: &mut impl FrameSink) -> io::Result<bool> {
        let waiting = self.session.poll(now);
        self.send_notices(sink)?;
        Ok(waiting)
    }

    fn send_notices(&mut self, sink: &mut impl FrameSink) -> io::Result<()> {
        for notice in self.session.take_notices() {
            self.notice.clear();
            notice.encode(&mut self.notice);
            sink.send(Channel::Global, FrameHeader::host_message(self.notice.len()), &self.notice)?;
        }
        Ok(())
    }
}

/// Why a frame could not be handled.
#[derive(Debug)]
pub enum RouterError {
    Malformed(Truncated),
    Socket(io::Error),
}

impl From<Truncated> for RouterError {
    fn from(error: Truncated) -> Self {
        Self::Malformed(error)
    }
}

impl From<io::Error> for RouterError {
    fn from(error: io::Error) -> Self {
        Self::Socket(error)
    }
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(error) => error.fmt(f),
            Self::Socket(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RouterError {}
