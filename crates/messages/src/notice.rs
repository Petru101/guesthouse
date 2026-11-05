//! Messages the host starts by itself, on the global channel.

use crate::wire::{put_u32, put_u64};

const CHANNEL_GLOBAL: u32 = 2;
const COMMAND_WAKE_EVENT: u64 = 0;
const COMMAND_PRESENT_DONE: u64 = 1;

/// What the host tells the guest without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Wakes a guest event. With `release` set, the host held the only reference to it.
    WakeEvent { event: u64, release: u64 },
    /// Returns a present's token, which is how the compositor learns a frame is ready.
    PresentDone { token: [u64; 3] },
}

impl Notice {
    pub fn encode(&self, out: &mut Vec<u8>) {
        put_u64(out, 0);
        put_u32(out, 0);
        put_u32(out, CHANNEL_GLOBAL);
        match *self {
            Self::WakeEvent { event, release } => {
                put_u64(out, COMMAND_WAKE_EVENT);
                put_u64(out, event);
                put_u64(out, 0);
                put_u64(out, release);
            }
            Self::PresentDone { token } => {
                put_u64(out, COMMAND_PRESENT_DONE);
                token.iter().for_each(|&part| put_u64(out, part));
                put_u64(out, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::u64_at;

    #[test]
    fn wake_event_layout() {
        let mut out = Vec::new();
        Notice::WakeEvent { event: 0xAB, release: 1 }.encode(&mut out);
        assert_eq!(out.len(), 48);
        assert_eq!(u64_at(&out, 24), Ok(0xAB));
        assert_eq!(u64_at(&out, 40), Ok(1));
    }

    #[test]
    fn present_done_layout() {
        let mut out = Vec::new();
        Notice::PresentDone { token: [1, 2, 3] }.encode(&mut out);
        assert_eq!(out.len(), 56);
        assert_eq!(u64_at(&out, 16), Ok(1));
        assert_eq!(u64_at(&out, 40), Ok(3));
    }
}
