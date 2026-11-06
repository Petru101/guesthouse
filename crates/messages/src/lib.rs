//! The messages a Windows guest's graphics kernel sends the host, and the answers it expects.
//!
//! Requests are read in place: a parsed request borrows the message and copies nothing. Answers are
//! appended to a buffer the caller reuses. Nothing here keeps state or decides anything, and
//! nothing here knows about any GPU vendor.
//!
//! Field positions are offsets from the start of the message, after the optional extended header
//! that [`strip_extended_header`] removes. The guest fixes them, so they cannot be chosen here.

pub mod adapter;
pub mod address;
pub mod allocation;
mod command;
mod header;
pub mod indirect;
pub mod notice;
pub mod queue;
pub mod registry;
pub mod standard;
mod status;
pub mod sync;
pub mod wire;

pub use command::Command;
pub use header::{HEADER_SIZE, MessageHeader, strip_extended_header};
pub use status::Status;
pub use wire::Malformed;
