//! Reassembling frames from a stream socket.

use crate::{FrameHeader, HEADER_SIZE};
use std::io::{self, Read};

const READ_CHUNK: usize = 64 << 10;

/// A frame borrowed from the stream's buffer.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    pub header: FrameHeader,
    pub payload: &'a [u8],
}

/// Collects bytes from a socket and hands back whole frames without copying them.
///
/// The buffer is reused for the life of the connection, so a steady flow of messages does not
/// allocate. Whatever is left of a partial frame moves to the front before each read.
#[derive(Debug, Default)]
pub struct FrameStream {
    buffer: Vec<u8>,
    start: usize,
    end: usize,
}

impl FrameStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads once from `source`. Returns how many bytes came, and 0 means the peer has gone.
    pub fn fill_from(&mut self, source: &mut impl Read) -> io::Result<usize> {
        self.make_room(READ_CHUNK);
        let count = source.read(&mut self.buffer[self.end..])?;
        self.end += count;
        Ok(count)
    }

    /// Adds bytes that came from somewhere other than a reader.
    pub fn push(&mut self, bytes: &[u8]) {
        self.make_room(bytes.len());
        self.buffer[self.end..self.end + bytes.len()].copy_from_slice(bytes);
        self.end += bytes.len();
    }

    /// The next whole frame, if one has arrived.
    pub fn next_frame(&mut self) -> Option<Frame<'_>> {
        let pending = &self.buffer[self.start..self.end];
        let header = FrameHeader::parse(pending.first_chunk::<HEADER_SIZE>()?);
        let end = HEADER_SIZE + header.payload_len as usize;
        let payload = pending.get(HEADER_SIZE..end)?;
        self.start += end;
        Some(Frame { header, payload })
    }

    /// Moves what is left of a partial frame to the front, and makes sure `wanted` bytes fit after it.
    fn make_room(&mut self, wanted: usize) {
        self.buffer.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.start = 0;
        if self.buffer.len() < self.end + wanted {
            self.buffer.resize(self.end + wanted, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameFlags, FrameKind};

    fn frame(transaction: u64, payload: &[u8]) -> Vec<u8> {
        let header = FrameHeader {
            payload_len: payload.len() as u32,
            kind: FrameKind::Inband,
            flags: FrameFlags(1),
            transaction,
        };
        [&header.encode()[..], payload].concat()
    }

    #[test]
    fn frames_split_at_any_byte_still_arrive_whole() {
        let bytes = [frame(1, b"first"), frame(2, b""), frame(3, b"third")].concat();
        for cut in 0..bytes.len() {
            let mut stream = FrameStream::new();
            let mut seen = Vec::new();
            for part in [&bytes[..cut], &bytes[cut..]] {
                stream.push(part);
                while let Some(frame) = stream.next_frame() {
                    seen.push((frame.header.transaction, frame.payload.to_vec()));
                }
            }
            let expected = [(1, b"first".to_vec()), (2, Vec::new()), (3, b"third".to_vec())];
            assert_eq!(seen, expected, "cut at {cut}");
        }
    }

    #[test]
    fn a_partial_frame_waits() {
        let bytes = frame(9, b"payload");
        let mut stream = FrameStream::new();
        stream.push(&bytes[..bytes.len() - 1]);
        assert!(stream.next_frame().is_none());
        stream.push(&bytes[bytes.len() - 1..]);
        assert_eq!(stream.next_frame().unwrap().payload, b"payload");
    }

    #[test]
    fn fill_reports_a_closed_peer() {
        let mut stream = FrameStream::new();
        let mut empty: &[u8] = &[];
        assert_eq!(stream.fill_from(&mut empty).unwrap(), 0);
        let mut bytes: &[u8] = &frame(4, b"x");
        assert_eq!(stream.fill_from(&mut bytes).unwrap(), 17);
        assert_eq!(stream.next_frame().unwrap().header.transaction, 4);
    }
}
