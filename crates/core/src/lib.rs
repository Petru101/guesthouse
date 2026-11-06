//! What the host keeps track of for one guest, and how it answers each message.
//!
//! Everything here is vendor-neutral. [`Session`] owns the tables, and turns a message into a reply
//! with help from a [`guesthouse_backend::Backend`]. It runs on a single thread: nothing is shared
//! and nothing is locked, and what has to wait for a fence sits in a list that
//! [`Session::poll`] works through.
//!
//! The thread must never wait, because one guest thread waiting for a reply is no reason to stop
//! the others. A message that can't be answered yet gets [`Answer::Held`]: its reply goes out later,
//! through [`Session::take_replies`], while other messages carry on.

mod config;
mod deferred;
mod fences;
mod frames;
mod gpu_addresses;
mod guest_ranges;
mod handles;
mod pending;
mod session;
mod sync_table;
mod tables;
mod timing;

pub use config::{Config, CopyOrder, MemoryLimits, video_budget};
pub use fences::{FENCE_AREA, FENCE_PAGE};
pub use frames::{Delivery, Frame, FrameCounts, FrameRing, FrameSink, NotTheViewers, Picture, Slots};
pub use gpu_addresses::{ALLOCATION_ALIGN, AddressUse, GPU_RANGES, GpuAddresses, LARGE_ALIGN};
pub use guest_ranges::{GUEST_RANGE_GRAIN, GuestRanges};
pub use handles::HandleCounter;
pub use pending::Waiting;
pub use session::{Answer, HeldReply, ReplyTicket, Session, TableSizes};
pub use timing::{HeldFor, Stage, Timings};
