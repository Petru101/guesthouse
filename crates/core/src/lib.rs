//! What the host keeps track of for one guest, and how it answers each message.
//!
//! Everything here is vendor-neutral. [`Session`] owns the tables, and turns a message into a reply
//! with help from a [`guesthouse_backend::Backend`]. It runs on a single thread: nothing is shared
//! and nothing is locked, and what has to wait for a fence sits in a list that
//! [`Session::poll`] works through.

mod config;
mod deferred;
mod fences;
mod gpu_addresses;
mod guest_ranges;
mod handles;
mod pending;
mod session;
mod sync_table;
mod tables;
mod timing;

pub use config::{Config, MemoryLimits, video_budget};
pub use fences::{FENCE_AREA, FENCE_PAGE};
pub use gpu_addresses::{ALLOCATION_ALIGN, AddressUse, GPU_RANGES, GpuAddresses, LARGE_ALIGN};
pub use guest_ranges::{GUEST_RANGE_GRAIN, GuestRanges};
pub use handles::HandleCounter;
pub use session::Session;
pub use timing::{Stage, Timings};
