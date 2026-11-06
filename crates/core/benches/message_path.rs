//! How long a message takes from arrival to reply, with no GPU and no socket involved.
//!
//! This is a plain timing loop rather than a benchmark framework, as it needs only a median and a
//! worst case. Run it with `cargo bench -p guesthouse-core`. The number covers parsing, bookkeeping
//! and building the reply. It does not cover the socket, QEMU or the guest.

use guesthouse_backend::{Backend, GuestMemory, PagesView};
use guesthouse_backend_standin::StandIn;
use guesthouse_core::{Config, MemoryLimits, ReplyTicket, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use std::hint::black_box;
use std::io;
use std::ops::Range;
use std::time::{Duration, Instant};

const TICKET: Option<ReplyTicket> = Some(ReplyTicket { channel: 1, transaction: 1 });
const ROUNDS: usize = 200;
const MESSAGES_PER_ROUND: usize = 10_000;
const WINDOW: u64 = 1 << 30;
const IO_SPACE: u64 = 1 << 40;

/// Plain memory, as a sparse map would dominate the measurement.
struct FlatMemory(Vec<u64>);

impl FlatMemory {
    fn index(address: u64) -> usize {
        ((address - IO_SPACE) / 8) as usize
    }
}

impl GuestMemory for FlatMemory {
    fn window(&self) -> Range<u64> {
        IO_SPACE..IO_SPACE + WINDOW
    }
    fn read_u64(&self, address: u64) -> u64 {
        self.0[Self::index(address)]
    }
    fn write_u64(&mut self, address: u64, value: u64) {
        self.0[Self::index(address)] = value;
    }
    fn write_u32(&mut self, _address: u64, _value: u32) {}
    fn read_bytes(&self, _address: u64, _out: &mut [u8]) -> usize {
        0
    }
    fn zero(&mut self, address: u64, length: u64) {
        self.0[Self::index(address)..Self::index(address + length)].fill(0);
    }
    fn host_address(&self, address: u64, _length: u64) -> Option<u64> {
        Some(address)
    }
    fn map_pages(&mut self, _pages: &[u64]) -> io::Result<PagesView> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn unmap_pages(&mut self, _view: PagesView) {}
}

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 8, 0x4000_2000);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

fn new_session() -> Session<StandIn, FlatMemory> {
    let backend = StandIn::new();
    let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
    let memory = FlatMemory(vec![0; (WINDOW / 8) as usize]);
    let mut session = Session::new(backend, memory, Config::new(limits));
    let mut out = Vec::new();
    let mut request = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut request, HEADER_SIZE, IO_SPACE);
    let _ = session.handle_message(&request, TICKET, &mut out);
    session
}

/// Makes a context and queue, as the guest would before submitting. Returns the queue.
fn queue(session: &mut Session<StandIn, FlatMemory>) -> (u32, u32) {
    let mut out = Vec::new();
    let _ = session.handle_message(&message(Command::CreateContext, 0x38), TICKET, &mut out);
    let context = u32_at(&out, 0x18).unwrap();
    let mut request = message(Command::CreateHardwareQueue, 0x48);
    set_u32(&mut request, 0x38, context);
    let _ = session.handle_message(&request, TICKET, &mut out);
    (context, u32_at(&out, 0x1C).unwrap())
}

fn fence(session: &mut Session<StandIn, FlatMemory>) -> u32 {
    let mut out = Vec::new();
    let mut request = message(Command::CreateSyncObject, 0x30);
    set_u32(&mut request, 0x20, 5);
    let _ = session.handle_message(&request, TICKET, &mut out);
    u32_at(&out, 0).unwrap()
}

/// Times `run` over many rounds, and prints nanoseconds a message.
fn measure(name: &str, mut run: impl FnMut()) {
    let mut rounds: Vec<Duration> = (0..ROUNDS)
        .map(|_| {
            let started = Instant::now();
            (0..MESSAGES_PER_ROUND).for_each(|_| run());
            started.elapsed()
        })
        .collect();
    rounds.sort();
    let per_message = |round: Duration| round.as_nanos() as f64 / MESSAGES_PER_ROUND as f64;
    let (median, worst) = (per_message(rounds[ROUNDS / 2]), per_message(rounds[ROUNDS - 1]));
    println!("{name:<28} median {median:>8.1} ns   slowest round {worst:>8.1} ns");
}

fn main() {
    let mut session = new_session();
    let mut out = Vec::with_capacity(4096);
    let (_, queue_handle) = queue(&mut session);
    let sync = fence(&mut session);

    let mut value = 0;
    let mut submission = message(Command::SubmitToHardwareQueue, 0x50 + 0x100);
    set_u32(&mut submission, 0x18, queue_handle);
    measure("submit (no GPU)", || {
        value += 1;
        set_u64(&mut submission, 0x20, value);
        let _ = session.handle_message(black_box(&submission), TICKET, &mut out);
        black_box(&out);
    });

    let mut signal = message(Command::SignalSyncObject, 0x38 + 4 + 8);
    set_u32(&mut signal, 0x18, 1);
    set_u32(&mut signal, 0x38, sync);
    measure("signal a fence", || {
        value += 1;
        set_u64(&mut signal, 0x3C, value);
        let _ = session.handle_message(black_box(&signal), TICKET, &mut out);
        black_box(&out);
    });

    let mut wait = message(Command::WaitOnCpu, 0x38 + 4 + 8);
    set_u32(&mut wait, 0x1C, 1);
    set_u32(&mut wait, 0x38, sync);
    measure("wait that is already done", || {
        let _ = session.handle_message(black_box(&wait), TICKET, &mut out);
        session.take_notices().for_each(|notice| {
            black_box(notice);
        });
    });

    let flush = message(Command::FlushDevice, HEADER_SIZE);
    measure("plain status answer", || {
        let _ = session.handle_message(black_box(&flush), TICKET, &mut out);
        black_box(&out);
    });
}
