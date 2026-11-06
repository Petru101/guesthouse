//! Everything Guesthouse's desktop app does that needs no display.
//!
//! Each machine is one JSON file. Anything slow runs as a task: a process of its own that outlives
//! the app and writes each step to a small file, so the app can close at any moment and read where
//! things are when it opens again. [`context::Context`] holds what the code needs from the PC, and
//! tests swap those parts for stand-ins. How a VM starts is behind [`vm_host::VmHost`].

pub mod agent;
pub mod bridge;
pub mod clock;
pub mod context;
pub mod daemon;
pub mod failure;
pub mod gpu_sample;
pub mod host;
pub mod input;
pub mod iso;
pub mod library;
pub mod log;
pub mod machine;
pub mod monitor;
pub mod network;
pub mod network_settings;
pub mod overlay;
pub mod paths;
pub mod processes;
pub mod random;
pub mod settings;
pub mod share;
pub mod slots;
pub mod start_checks;
pub mod tasks;
#[doc(hidden)]
pub mod testing;
pub mod text;
pub mod usb;
pub mod vm_args;
pub mod vm_host;

pub use failure::{Failure, Result};
