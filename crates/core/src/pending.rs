//! Things that wait on fences, and queue gates.
//!
//! None of this has a thread of its own. The daemon calls `Session::poll` while anything here is
//! waiting, and a condition is simply checked again.

use guesthouse_backend::SpaceId;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long the GPU gets before the host stops waiting for a queue.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a queue may wait behind a fence before going on regardless.
pub const GATE_TIMEOUT: Duration = Duration::from_secs(10);
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
    AfterQueues { targets: Vec<Target>, deadline: Instant, action: QueueAction },
    CpuWait { signals: Vec<Signal>, wake_on_any: bool, event: u64, release: u64 },
    Gate { queue: u32 },
    Update { fence: u32, value: u64, space: SpaceId, operations: Vec<u8> },
    Watch { queue: u32, fence: u64, value: u64, deadline: Instant },
}

/// What a queue held back, in order.
#[derive(Debug)]
pub enum GateStep {
    Wait { signals: Vec<Signal>, deadline: Instant },
    Ring { doorbell: u64, fence: u64, value: u64 },
}

pub type Gate = VecDeque<GateStep>;
