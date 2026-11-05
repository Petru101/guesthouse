//! The frame header, as QEMU's side of the socket lays it out.

/// Bytes in a frame header.
pub const HEADER_SIZE: usize = 16;

/// The largest payload QEMU accepts from us. Past this it drops everything it has buffered.
pub const MAX_OUTGOING_PAYLOAD: usize = 4 << 20;

const PACKET_INBAND: u16 = 0x6;
const PACKET_GPA_DIRECT: u16 = 0x9;
const PACKET_COMPLETION: u16 = 0xB;
const EVENT_CHANNEL_OPENED: u16 = 0xF001;
const EVENT_CHANNEL_CLOSED: u16 = 0xF002;
const EVENT_RESET: u16 = 0xF003;

/// What a frame carries. The first three are VMBus packet types and the rest are QEMU's own
/// lifecycle events, numbered above the packet range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Inband,
    GpaDirect,
    Completion,
    ChannelOpened,
    ChannelClosed,
    Reset,
    Other(u16),
}

impl FrameKind {
    pub fn from_wire(value: u16) -> Self {
        match value {
            PACKET_INBAND => Self::Inband,
            PACKET_GPA_DIRECT => Self::GpaDirect,
            PACKET_COMPLETION => Self::Completion,
            EVENT_CHANNEL_OPENED => Self::ChannelOpened,
            EVENT_CHANNEL_CLOSED => Self::ChannelClosed,
            EVENT_RESET => Self::Reset,
            other => Self::Other(other),
        }
    }

    pub fn to_wire(self) -> u16 {
        match self {
            Self::Inband => PACKET_INBAND,
            Self::GpaDirect => PACKET_GPA_DIRECT,
            Self::Completion => PACKET_COMPLETION,
            Self::ChannelOpened => EVENT_CHANNEL_OPENED,
            Self::ChannelClosed => EVENT_CHANNEL_CLOSED,
            Self::Reset => EVENT_RESET,
            Self::Other(other) => other,
        }
    }

    pub fn is_lifecycle_event(self) -> bool {
        matches!(self, Self::ChannelOpened | Self::ChannelClosed | Self::Reset)
    }
}

/// The flag bits of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameFlags(pub u16);

impl FrameFlags {
    const WANTS_COMPLETION: u16 = 0x1;
    const HAS_RANGES: u16 = 0x2;

    /// The guest is waiting for an answer with this frame's transaction id.
    pub fn wants_completion(self) -> bool {
        self.0 & Self::WANTS_COMPLETION != 0
    }

    /// The payload starts with a list of guest page ranges. See [`crate::split_ranges`].
    pub fn has_ranges(self) -> bool {
        self.0 & Self::HAS_RANGES != 0
    }
}

/// A frame header. All fields are little-endian on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub payload_len: u32,
    pub kind: FrameKind,
    pub flags: FrameFlags,
    pub transaction: u64,
}

impl FrameHeader {
    pub fn parse(bytes: &[u8; HEADER_SIZE]) -> Self {
        let [l0, l1, l2, l3, k0, k1, f0, f1, t @ ..] = *bytes;
        Self {
            payload_len: u32::from_le_bytes([l0, l1, l2, l3]),
            kind: FrameKind::from_wire(u16::from_le_bytes([k0, k1])),
            flags: FrameFlags(u16::from_le_bytes([f0, f1])),
            transaction: u64::from_le_bytes(t),
        }
    }

    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut bytes = [0; HEADER_SIZE];
        bytes[0..4].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.kind.to_wire().to_le_bytes());
        bytes[6..8].copy_from_slice(&self.flags.0.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.transaction.to_le_bytes());
        bytes
    }

    /// The header of a reply to the transaction `transaction`.
    pub fn completion(transaction: u64, payload_len: usize) -> Self {
        Self {
            payload_len: payload_len as u32,
            kind: FrameKind::Completion,
            flags: FrameFlags::default(),
            transaction,
        }
    }

    /// The header of a message the host starts by itself, which nothing answers.
    pub fn host_message(payload_len: usize) -> Self {
        Self {
            payload_len: payload_len as u32,
            kind: FrameKind::Inband,
            flags: FrameFlags::default(),
            transaction: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_survives_a_round_trip() {
        let header = FrameHeader {
            payload_len: 0x0102_0304,
            kind: FrameKind::GpaDirect,
            flags: FrameFlags(3),
            transaction: 0x1122_3344_5566_7788,
        };
        assert_eq!(FrameHeader::parse(&header.encode()), header);
    }

    #[test]
    fn header_fields_are_little_endian_in_order() {
        let bytes = FrameHeader::completion(0x0A0B, 0x10).encode();
        assert_eq!(bytes, [0x10, 0, 0, 0, 0xB, 0, 0, 0, 0x0B, 0x0A, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn unknown_kinds_keep_their_number() {
        assert_eq!(FrameKind::from_wire(0xF010).to_wire(), 0xF010);
    }

    #[test]
    fn flags_are_independent() {
        assert!(FrameFlags(1).wants_completion() && !FrameFlags(1).has_ranges());
        assert!(FrameFlags(2).has_ranges() && !FrameFlags(2).wants_completion());
    }
}
