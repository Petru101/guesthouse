//! What a session is told at startup.

use guesthouse_backend::AdapterFacts;
use std::time::Duration;

const GIB: u64 = 1 << 30;
/// The host's desktop keeps an eighth of the card, and never less than this.
const HOST_SHARE: u64 = 8;
const HOST_MINIMUM_GIB: u64 = 2;

/// How much memory the guest is told about, and may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryLimits {
    /// A hard limit on video memory, or `None` for only what the card has.
    pub video_cap: Option<u64>,
    pub video_budget: u64,
    pub system_budget: u64,
}

impl MemoryLimits {
    pub fn new(facts: &AdapterFacts, video_cap: Option<u64>, guest_ram: u64) -> Self {
        Self {
            video_cap,
            video_budget: video_cap.unwrap_or_else(|| video_budget(facts.video_memory)),
            system_budget: guest_ram,
        }
    }
}

/// What a guest may use of a card with this much dedicated memory, in bytes.
pub fn video_budget(dedicated: u64) -> u64 {
    let card = dedicated.div_ceil(GIB);
    card.saturating_sub((card / HOST_SHARE).max(HOST_MINIMUM_GIB)) * GIB
}

/// How a copy to the staging surface waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOrder {
    /// On the message thread, so nothing else happens until the copy is done. This is slower, and
    /// it is the order the prototype has been proven in.
    Blocking,
    /// With the reply held, while other messages are served.
    Held,
}

/// Settings for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub limits: MemoryLimits,
    /// Tell the guest it may send without waiting. The guest reads this once, as it boots.
    pub async_messages: bool,
    /// The longest a copy to the screen waits for its frame, on the message thread.
    pub frame_wait: Duration,
    pub copy_order: CopyOrder,
}

impl Config {
    /// The usual wait for a frame. It must stay short, as a fence could depend on a later message.
    pub const FRAME_WAIT: Duration = Duration::from_millis(50);

    pub fn new(limits: MemoryLimits) -> Self {
        Self { limits, async_messages: false, frame_wait: Self::FRAME_WAIT, copy_order: CopyOrder::Blocking }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_card_keeps_an_eighth_for_the_host() {
        assert_eq!(video_budget(32 * GIB), 28 * GIB);
    }

    #[test]
    fn a_small_card_keeps_the_minimum() {
        assert_eq!(video_budget(8 * GIB), 6 * GIB);
    }

    #[test]
    fn a_card_slightly_under_a_round_size_counts_as_that_size() {
        assert_eq!(video_budget(32 * GIB - 4096), 28 * GIB);
    }

    #[test]
    fn a_tiny_card_leaves_nothing() {
        assert_eq!(video_budget(GIB), 0);
    }
}
