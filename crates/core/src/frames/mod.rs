//! The boundary between what makes the guest's frames and what shows them.

mod picture;
mod slots;
mod staging;

pub use picture::Picture;
pub use slots::{NotTheViewers, Slots};
pub(crate) use staging::StagingFrames;
pub use staging::{StagingFrame, StagingRing, StagingSink};
