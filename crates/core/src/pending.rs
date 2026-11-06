//! Things that wait on fences, and queue gates.
//!
//! None of this has a thread of its own. The daemon calls `Session::poll` while anything here is
//! waiting, and a condition is simply checked again.

use crate::session::ReplyTicket;
use guesthouse_backend::SpaceId;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long the GPU gets before the host stops waiting for a queue.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a queue may wait behind a fence before going on regardless.
pub const GATE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a submission's reply may be held for ring room. It is bounded, because what the GPU is
/// waiting for may be in a later submission from the same guest thread.
pub const RING_ROOM_TIMEOUT: Duration = Duration::from_millis(100);
/// One app's queue dying should cost only that app, but one that keeps faulting must not keep
/// hitting the GPU.
pub const MOST_QUEUE_FAILURES: u32 = 8;

/// (fence address, value).
pub type Target = (u64, u64);
/// (sync object handle, value).
pub type Signal = (u32, u64);

/// What to do when some queues finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    WakeEvent { event: u64, release: u64 },
    WriteFences(Vec<Signal>),
    PresentDone([u64; 3]),
}

/// One thing that is waiting.
#[derive(Debug)]
pub enum Pending {
    AfterQueues {
        targets: Vec<Target>,
        deadline: Instant,
        action: QueueAction,
    },
    CpuWait {
        signals: Vec<Signal>,
        wake_on_any: bool,
        event: u64,
        release: u64,
    },
    Gate {
        queue: u32,
    },
    Update {
        fence: u32,
        value: u64,
        space: SpaceId,
        operations: Vec<u8>,
    },
    /// A submission's reply, held until its queue has room.
    RingRoom {
        queue: u32,
        fence: u64,
        ticket: ReplyTicket,
        started: Instant,
    },
    /// A submission the backend had no room for, to try again.
    Retry {
        message: Vec<u8>,
        ticket: Option<ReplyTicket>,
        started: Instant,
    },
    /// Only here to notice a GPU that has stopped, so nobody is in a hurry for it.
    Watch {
        queue: u32,
        fence: u64,
        value: u64,
        rung: Instant,
    },
}

impl Pending {
    fn is_urgent(&self) -> bool {
        !matches!(self, Self::Watch { .. })
    }
}

/// What the caller of `Session::poll` should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// Nothing is waiting, so sleep until a message comes.
    Nothing,
    /// Only the GPU's health is being watched, so look again before long.
    Background,
    /// The guest is waiting on a fence, so look again at once.
    Urgent,
}

impl Waiting {
    pub(crate) fn of<'a>(pending: impl Iterator<Item = &'a Pending>, copies_waiting: bool) -> Self {
        let mut found = if copies_waiting { Self::Urgent } else { Self::Nothing };
        for item in pending {
            if item.is_urgent() {
                return Self::Urgent;
            }
            found = Self::Background;
        }
        found
    }

    pub fn any(self) -> bool {
        self != Self::Nothing
    }
}

/// What a queue held back, in order.
#[derive(Debug)]
pub enum GateStep {
    Wait {
        signals: Vec<Signal>,
        deadline: Instant,
    },
    Ring {
        doorbell: u64,
        fence: u64,
        value: u64,
    },
    /// A copy that reads this queue's last frame, which the next frame must not draw over.
    Copy {
        id: u64,
    },
}

pub type Gate = VecDeque<GateStep>;
