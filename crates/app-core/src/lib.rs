//! Everything Guesthouse's desktop app does that needs no display.

pub mod bridge;
pub mod clock;
pub mod failure;
pub mod network;
pub mod paths;
pub mod random;
pub mod slots;
#[doc(hidden)]
pub mod testing;
pub mod text;

pub use failure::{Failure, Result};
