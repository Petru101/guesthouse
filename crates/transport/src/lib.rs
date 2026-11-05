//! The byte stream between QEMU and the daemon.
//!
//! QEMU knows nothing about graphics. For each VMBus channel it opens a stream socket and forwards
//! every guest packet as a frame: a 16-byte header, then the payload. This crate turns that stream
//! back into frames. What a payload means is the business of `guesthouse-messages`.

mod frame;
mod ranges;
mod stream;

pub use frame::{FrameFlags, FrameHeader, FrameKind, HEADER_SIZE, MAX_OUTGOING_PAYLOAD};
pub use ranges::{GpaRange, GpaRanges, split_ranges};
pub use stream::{Frame, FrameStream};

/// A frame or range list that ends before it says it should.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated;

impl std::fmt::Display for Truncated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the data ends before its own length says it should")
    }
}

impl std::error::Error for Truncated {}
