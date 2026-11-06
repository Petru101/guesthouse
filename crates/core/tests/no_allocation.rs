//! The busiest messages must not touch the allocator once a session has warmed up.

use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_backend_standin::{SparseMemory, StandIn};
use guesthouse_core::{Config, MemoryLimits, ReplyTicket, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at, u64_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: passes everything to the system allocator and only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's promises are the ones the system allocator needs.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

const TICKET: Option<ReplyTicket> = Some(ReplyTicket { channel: 1, transaction: 1 });
const WARM_UP: u64 = 100;
const ROUNDS: u64 = 1_000;

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

fn allocations_during(mut run: impl FnMut(u64)) -> usize {
    (0..WARM_UP).for_each(&mut run);
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    (WARM_UP..WARM_UP + ROUNDS).for_each(&mut run);
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

struct Prepared {
    session: Session<StandIn, SparseMemory>,
    queue: u32,
    sync: u32,
    fence: u64,
}

fn prepared() -> Prepared {
    let backend = StandIn::new();
    let limits = MemoryLimits::new(backend.facts(), None, 1 << 30);
    let mut session = Session::new(backend, SparseMemory::new(1 << 41), Config::new(limits));
    let mut out = Vec::new();
    let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, 1 << 40);
    let _ = session.handle_message(&io_space, TICKET, &mut out);
    let _ = session.handle_message(&message(Command::CreateContext, 0x38), TICKET, &mut out);
    let mut queue = message(Command::CreateHardwareQueue, 0x48);
    set_u32(&mut queue, 0x38, u32_at(&out, 0x18).unwrap());
    let _ = session.handle_message(&queue, TICKET, &mut out);
    let (queue, fence) = (u32_at(&out, 0x1C).unwrap(), u64_at(&out, 0x28).unwrap());
    let mut sync = message(Command::CreateSyncObject, 0x30);
    set_u32(&mut sync, 0x20, 5);
    let _ = session.handle_message(&sync, TICKET, &mut out);
    Prepared { queue, fence, sync: u32_at(&out, 0).unwrap(), session }
}

// One test, as the counter is shared and tests run side by side.
#[test]
fn the_message_path_does_not_allocate() {
    let Prepared { mut session, queue, sync, fence } = prepared();
    let mut out = Vec::with_capacity(4096);

    let mut submission = message(Command::SubmitToHardwareQueue, 0x60);
    set_u32(&mut submission, 0x18, queue);
    let count = allocations_during(|round| {
        set_u64(&mut submission, 0x20, round);
        let _ = session.handle_message(&submission, TICKET, &mut out);
    });
    assert_eq!(count, 0, "a submission that finishes at once");

    let mut signal = message(Command::SignalSyncObject, 0x38 + 4 + 8);
    set_u32(&mut signal, 0x18, 1);
    set_u32(&mut signal, 0x38, sync);
    let count = allocations_during(|round| {
        set_u64(&mut signal, 0x3C, round);
        let _ = session.handle_message(&signal, TICKET, &mut out);
    });
    assert_eq!(count, 0, "a signal");

    session.backend_mut().run_work = true;
    session.backend_mut().doorbells.reserve(2 * (WARM_UP + ROUNDS) as usize);
    let now = Instant::now();
    let count = allocations_during(|round| {
        set_u64(&mut submission, 0x20, round + 1);
        let _ = session.handle_message(&submission, TICKET, &mut out);
        session.memory_mut().write_u64(fence, round + 1).unwrap();
        let _ = session.poll(now);
    });
    assert_eq!(count, 0, "a submission on the GPU, and watching its fence");
}
