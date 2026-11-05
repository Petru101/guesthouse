//! The safety net for the rewrite: replaying a real session and comparing every reply.
//!
//! A recording holds what the reference daemon received and sent in a real session. This crate feeds
//! the received frames through the same [`guesthouse_daemon::router::Router`] the daemon runs, and
//! compares what comes out with what was sent, byte for byte.
//!
//! No vendor backend exists yet, so [`answers::RecordedAnswers`] plays that part by reading vendor
//! data out of the recording. That makes those bytes match by construction, so a report counts
//! them apart from what the core really worked out. See [`report::Report`].

pub mod answers;
pub mod explain;
pub mod recording;
pub mod replay;
pub mod report;
