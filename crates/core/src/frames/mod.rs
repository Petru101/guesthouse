//! The boundary between what makes the guest's frames and what shows them.
//!
//! A sink hears of rings of buffers and the frames in them, and knows nothing of how a frame came to
//! be. It's offered these from the main loop and never from a message's handler, so a slow viewer
//! can't reach the guest. What a sink can't take now is offered again, and until then it stays here.

mod cursor;
mod indirect;
mod picture;
mod ring;
mod slots;

pub(crate) use cursor::Cursor;
pub use cursor::CursorShape;
pub use indirect::FrameLight;
pub(crate) use indirect::{GuestSlot, IndirectRing, Landing, Registering, ReplyWaiting};
pub use picture::Picture;
pub use ring::FrameCounts;
pub use ring::ViewerFacts;
pub(crate) use ring::{LiveRing, RING_SLOTS, ScreenFrames, SlotInFlight, SlotMemory};
pub use slots::{NotTheViewers, Slots};

use guesthouse_messages::indirect::{CursorMove, Look};
use std::os::fd::BorrowedFd;

/// A set of buffers in video memory that frames take turns in. A new generation means the old
/// buffers are gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRing {
    pub generation: u32,
    pub slots: u32,
    /// The bytes in each slot.
    pub size: u64,
}

/// A finished frame. The copy into its slot has ended on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub generation: u32,
    pub slot: u32,
    pub sequence: u64,
    pub picture: Picture,
    pub look: Look,
    /// What the guest said about this frame's brightness, if it said anything.
    pub light: Option<FrameLight>,
}

/// What became of something offered to a sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Delivery {
    Sent,
    /// There's no room for it now. Nothing changed.
    NotNow,
    /// Whoever was watching has gone.
    Gone,
}

/// See the module documentation.
pub trait FrameSink {
    fn ring_opened(&mut self, ring: &FrameRing, buffers: &[BorrowedFd<'_>]) -> Delivery;

    fn frame_ready(&mut self, frame: &Frame) -> Delivery;

    /// Only for a viewer that has asked to draw the cursor.
    fn cursor_shape(&mut self, _shape: &CursorShape) -> Delivery {
        Delivery::Sent
    }

    /// Only for a viewer that has asked to draw the cursor.
    fn cursor_moved(&mut self, _position: &CursorMove) -> Delivery {
        Delivery::Sent
    }
}
