//! Everything Guesthouse's desktop app does that needs no display.

pub mod bridge;
pub mod clock;
pub mod context;
pub mod daemon;
pub mod failure;
pub mod host;
pub mod library;
pub mod log;
pub mod machine;
pub mod monitor;
pub mod network;
pub mod paths;
pub mod processes;
pub mod random;
pub mod settings;
pub mod slots;
pub mod tasks;
#[doc(hidden)]
pub mod testing;
pub mod text;

pub use failure::{Failure, Result};
