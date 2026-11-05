//! The fields every message starts with.

use crate::Command;
use crate::wire::{Malformed, u32_at};

/// Where a message's own fields begin. The common fields end sooner, and the guest pads them to
/// an 8-byte boundary.
pub const HEADER_SIZE: usize = 24;

const EXTENDED_HEADER_SIZE: usize = 16;
const PROCESS: usize = 8;
const COMMAND: usize = 16;
const SHORTEST_MESSAGE: usize = 20;

/// The common fields of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// The guest process it came from, as the host named it. Zero for messages from the kernel.
    pub process: u32,
    pub command: Command,
}

impl MessageHeader {
    /// `None` for something too short to name a command, which is answered as unknown.
    pub fn parse(message: &[u8]) -> Option<Self> {
        if message.len() < SHORTEST_MESSAGE {
            return None;
        }
        Some(Self {
            process: Self::process_of(message).ok()?,
            command: Command::from_wire(u32_at(message, COMMAND).ok()?),
        })
    }

    pub fn process_of(message: &[u8]) -> Result<u32, Malformed> {
        u32_at(message, PROCESS)
    }
}

/// Newer guests put a small header before each message, whose first field is its own size.
pub fn strip_extended_header(message: &[u8]) -> &[u8] {
    match u32_at(message, 0) {
        Ok(size) if size as usize == EXTENDED_HEADER_SIZE && message.len() >= EXTENDED_HEADER_SIZE => {
            &message[EXTENDED_HEADER_SIZE..]
        }
        _ => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(process: u32, command: u32) -> Vec<u8> {
        let mut bytes = vec![0; HEADER_SIZE];
        bytes[PROCESS..PROCESS + 4].copy_from_slice(&process.to_le_bytes());
        bytes[COMMAND..COMMAND + 4].copy_from_slice(&command.to_le_bytes());
        bytes
    }

    #[test]
    fn reads_process_and_command() {
        let header = MessageHeader::parse(&message(0x77, 1000)).unwrap();
        assert_eq!(header.process, 0x77);
        assert_eq!(header.command, Command::CreateProcess);
    }

    #[test]
    fn a_short_message_has_no_header() {
        assert_eq!(MessageHeader::parse(&[0; 19]), None);
    }

    #[test]
    fn extended_header_goes_only_when_it_is_one() {
        let mut bytes = vec![0; 16];
        bytes[0] = 16;
        bytes.extend_from_slice(b"rest");
        assert_eq!(strip_extended_header(&bytes), b"rest");
        bytes[0] = 15;
        assert_eq!(strip_extended_header(&bytes).len(), 20);
        assert_eq!(strip_extended_header(&[16, 0, 0, 0]).len(), 4);
    }
}
