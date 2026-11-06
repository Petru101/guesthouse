//! The NVIDIA backend.
//!
//! This talks to the host's kernel module through the [`rm::Rm`] layer, runs the guest's commands on
//! real channels, and answers what the guest's drivers ask of their own kernel driver.
//!
//! Three things vary, and each has its own place. The [`profile`] module holds a generation's classes
//! and a driver branch's layouts, and cards themselves are read live. See `ARCHITECTURE.md`.

// Parts wait for what uses them. This goes when the backend is joined up, later in this series.
#![allow(dead_code, unused_imports)]

pub mod answers;
pub mod channel;
pub mod inf;
pub mod profile;
pub mod pushbuffer;
pub mod rm;
pub mod sensors;
pub mod startup;
