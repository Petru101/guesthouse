//! From frames to the session and back.

use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_core::{Answer, ReplyTicket, Session, Stage, Waiting};
use guesthouse_messages::{Status, strip_extended_header};
use guesthouse_transport::{Frame, FrameHeader, FrameKind, MAX_OUTGOING_PAYLOAD, Truncated, split_ranges};
use std::io;
use std::time::Instant;

/// The two VMBus channels QEMU opens for each VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Global,
    Vgpu,
}

impl Channel {
    pub const ALL: [Self; 2] = [Self::Global, Self::Vgpu];

    fn from_number(number: u8) -> Self {
        if number == Self::Global as u8 { Self::Global } else { Self::Vgpu }
    }

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
            if header.kind == FrameKind::Reset {
                self.session.guest_restarted();
            }
            return Ok(());
        }
        let message = if header.flags.has_ranges() { split_ranges(frame.payload)?.1 } else { frame.payload };
        if header.kind == FrameKind::Completion {
            return Ok(());
        }
        self.session.timings_mut().mark(Stage::Received);
        let wants_reply = header.flags.wants_completion();
        let ticket = ReplyTicket { channel: channel as u8, transaction: header.transaction };
        let ticket = wants_reply.then_some(ticket);
        let answer = self.session.handle_message(strip_extended_header(message), ticket, &mut self.reply);
        // A handler's notices go out before its reply, the order the guest has always seen.
        self.send_waiting(sink)?;
        if wants_reply && answer == Answer::Now {
            self.limit_reply();
            let completion = FrameHeader::completion(header.transaction, self.reply.len());
            sink.send(channel, completion, &self.reply)?;
        }
        self.session.timings_mut().mark(Stage::Replied);
        Ok(())
    }

    /// QEMU drops all it has buffered at a frame past its limit, which would strand every guest
    /// thread waiting on this channel. A bare failure gets through.
    fn limit_reply(&mut self) {
        if self.reply.len() > MAX_OUTGOING_PAYLOAD {
            log::warn!("a reply of {} bytes is too long to send", self.reply.len());
            self.reply.clear();
            self.reply.extend_from_slice(&Status::UNSUCCESSFUL.0.to_le_bytes());
        }
    }

    /// Checks what is waiting on fences, and says how soon to look again.
    pub fn poll(&mut self, now: Instant, sink: &mut impl FrameSink) -> io::Result<Waiting> {
        let waiting = self.session.poll(now);
        self.send_waiting(sink)?;
        Ok(waiting)
    }

    /// Sends what the session has ready that no message asked for just now.
    fn send_waiting(&mut self, sink: &mut impl FrameSink) -> io::Result<()> {
        for held in self.session.take_replies() {
            let completion = FrameHeader::completion(held.ticket.transaction, held.bytes.len());
            sink.send(Channel::from_number(held.ticket.channel), completion, &held.bytes)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::Backend;
    use guesthouse_backend_standin::{SparseMemory, StandIn};
    use guesthouse_core::{Config, MemoryLimits};
    use guesthouse_messages::Command;
    use guesthouse_messages::wire::set_u32;
    use guesthouse_transport::{FrameFlags, HEADER_SIZE};

    const NEEDS_COMPLETION: u16 = 1;
    /// Where a context's message ends. The guest reads the whole message back as the answer.
    const CONTEXT_SIZE: usize = 0x34;

    #[derive(Default)]
    struct Sent(Vec<Vec<u8>>);

    impl FrameSink for Sent {
        fn send(&mut self, _: Channel, _: FrameHeader, payload: &[u8]) -> io::Result<()> {
            self.0.push(payload.to_vec());
            Ok(())
        }
    }

    fn reply_to(message: &[u8]) -> Vec<u8> {
        let backend = StandIn::new();
        let limits = MemoryLimits::new(backend.facts(), None, 1 << 30);
        let session = Session::new(backend, SparseMemory::new(1 << 41), Config::new(limits));
        let mut router = Router::new(session);
        let header = FrameHeader {
            payload_len: message.len() as u32,
            kind: FrameKind::Inband,
            flags: FrameFlags(NEEDS_COMPLETION),
            transaction: 1,
        };
        let mut sent = Sent::default();
        router.handle_frame(Channel::Vgpu, Frame { header, payload: message }, &mut sent).unwrap();
        sent.0.pop().unwrap()
    }

    fn context_message(size: usize) -> Vec<u8> {
        let mut message = vec![0; size];
        set_u32(&mut message, 16, Command::CreateContext.to_wire());
        message
    }

    #[test]
    fn a_reply_qemu_would_take_goes_out_whole() {
        assert_eq!(reply_to(&context_message(MAX_OUTGOING_PAYLOAD)).len(), MAX_OUTGOING_PAYLOAD);
    }

    #[test]
    fn a_reply_qemu_would_not_take_becomes_a_failure() {
        let reply = reply_to(&context_message(MAX_OUTGOING_PAYLOAD + HEADER_SIZE));
        assert_eq!(reply, Status::UNSUCCESSFUL.0.to_le_bytes());
    }

    #[test]
    fn a_short_context_is_not_a_reply_too_long() {
        assert_eq!(reply_to(&context_message(CONTEXT_SIZE)).len(), 0x38);
    }
}
