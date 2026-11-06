//! A frame ring in memory the guest made.
//!
//! The indirect display driver writes its own slots and the host only hands them out, so unlike a
//! ring of copies this one outlives viewers, and the host never frees its memory. It also runs with
//! nobody watching: a still desktop sends no frames, so the newest one has to be here for whoever
//! turns up next. That costs the guest its one pass over each frame.

use guesthouse_backend::{MemoryId, SpaceId};
use guesthouse_messages::indirect::{Mastering, Register, RingName};
use std::collections::VecDeque;
use std::time::Instant;

use crate::pending::Target;
use crate::session::ReplyTicket;

/// A reply that goes out when something has finished, whatever it turns out to say.
#[derive(Debug)]
pub(crate) struct ReplyWaiting {
    pub ticket: ReplyTicket,
    pub bytes: Vec<u8>,
}

/// One of the guest's buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuestSlot {
    pub allocation: u32,
    pub memory: MemoryId,
    pub size: u64,
}

/// What came with a frame, beyond the ring's own look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLight {
    pub sdr_white_nits: u32,
    pub mastering: Option<Mastering>,
}

/// A frame the guest has sent, whose pass may still be running on the GPU.
#[derive(Debug)]
pub(crate) struct Landing {
    pub slot: u32,
    pub targets: Vec<Target>,
    pub deadline: Instant,
    pub light: FrameLight,
}

/// A REGISTER whose stamps can't be read until the guest's queues have written them.
#[derive(Debug)]
pub(crate) struct Registering {
    pub reply: ReplyWaiting,
    pub process: u32,
    pub ring: Register,
    pub space: SpaceId,
    pub targets: Vec<Target>,
    pub deadline: Instant,
}

/// See the module documentation. The slots' states are in the [`LiveRing`](super::LiveRing).
#[derive(Debug)]
pub(crate) struct IndirectRing {
    pub owner: u32,
    pub name: RingName,
    pub slots: Vec<GuestSlot>,
    /// The slot the driver may write now. It keeps it until a FRAME gives it back.
    pub granted: Option<u32>,
    pub landing: VecDeque<Landing>,
    /// What came with the frame in each slot.
    pub light: Vec<Option<FrameLight>>,
    /// The next viewer needs its own descriptors.
    pub needs_export: bool,
    /// An export failed, which isn't worth trying on every turn of the loop.
    pub export_refused: bool,
    /// When it opened, for saying how long its first frame took.
    pub opened_at: Instant,
    /// A frame has really finished on this ring. Opening one proves nothing about the picture.
    pub landed: bool,
}

impl IndirectRing {
    pub fn holds(&self, allocation: u32) -> bool {
        self.slots.iter().any(|slot| slot.allocation == allocation)
    }
}
