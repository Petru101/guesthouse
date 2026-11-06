//! Any message may arrive in any order, and the daemon serves one VM on one thread, so a handler
//! that panics stops that VM. This sends random messages for every command through
//! `Session::handle_message` and checks that each one comes back.
//!
//! `FUZZ_ROUNDS` runs it for longer, and `FUZZ_SEED` picks where it starts.

use guesthouse_backend::Backend;
use guesthouse_backend_standin::{RandomMessages, SparseMemory, StandIn};
use guesthouse_core::{Config, CopyOrder, MemoryLimits, ReplyTicket, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use std::env;
use std::time::{Duration, Instant};

const TICKET: Option<ReplyTicket> = Some(ReplyTicket { channel: 1, transaction: 7 });
const WINDOW_END: u64 = 1 << 41;
const IO_SPACE: u64 = 1 << 40;
const GUEST_RAM: u64 = 1 << 30;

const DEFAULT_ROUNDS: u64 = 300_000;
const DEFAULT_SEED: u64 = 0x2545_F491_4F6C_DD1D;
/// A fresh session now and then, so a run is not just one session's late state.
const ROUNDS_PER_SESSION: u64 = 4000;
/// Past every deadline a session has, to reach what only happens when something gives up.
const FAR_FUTURE: Duration = Duration::from_secs(3600);

fn setting(name: &str, default: u64) -> u64 {
    env::var(name).ok().and_then(|text| text.parse().ok()).unwrap_or(default)
}

/// A session in one of the states a real one can be in, as `flavour` chooses.
fn fresh_session(flavour: u64) -> Session<StandIn, SparseMemory> {
    let mut backend = StandIn::new();
    backend.run_work = flavour & 1 != 0;
    let held_copies = flavour & 16 != 0;
    // On the message thread a copy waits to be finished, and the stand-in only ends one when told.
    backend.slow_copies = held_copies && flavour & 2 != 0;
    backend.unfinished_limit = (flavour & 4 != 0).then_some(2);
    let cap = (flavour & 8 != 0).then_some(GUEST_RAM);
    let limits = MemoryLimits::new(backend.facts(), cap, GUEST_RAM);
    let mut config = Config::new(limits);
    config.frame_wait = Duration::ZERO;
    config.copy_order = if held_copies { CopyOrder::Held } else { CopyOrder::Blocking };
    let mut session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
    let mut out = Vec::new();
    let mut io = vec![0u8; HEADER_SIZE + 16];
    set_u32(&mut io, 16, Command::SetIoSpace.to_wire());
    set_u64(&mut io, HEADER_SIZE, IO_SPACE);
    let _ = session.handle_message(&io, TICKET, &mut out);
    session
}

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

/// A request for one allocation, with stand-in private data asking for this much.
fn allocation(flags: u32, private: &[u8]) -> Vec<u8> {
    let mut request = message(Command::CreateAllocation, 0x40);
    set_u32(&mut request, 0x28, 1);
    set_u32(&mut request, 0x2C, flags);
    request.extend_from_slice(&[0; 4]);
    request.extend_from_slice(&(private.len() as u32).to_le_bytes());
    request.extend_from_slice(&[0; 4]);
    request.extend_from_slice(private);
    request
}

/// Well-formed messages of the kinds that take most checks to get through.
fn templates() -> Vec<Vec<u8>> {
    let mut surface = vec![0; 17];
    set_u64(&mut surface, 0, 0x10000);
    set_u32(&mut surface, 8, 64);
    set_u32(&mut surface, 12, 64);

    let mut standard = b"GPVS".to_vec();
    standard.resize(32, 0);
    set_u32(&mut standard, 12, 64);
    set_u32(&mut standard, 20, 256);

    let mut map = message(Command::MapGpuAddresses, 0x50);
    set_u64(&mut map, 0x48, 1);
    let mut update = message(Command::UpdateGpuAddresses, 0x38 + 0x40);
    set_u32(&mut update, 0x2C, 1);
    set_u64(&mut update, 0x38 + 8, 0x2000_0000);
    set_u64(&mut update, 0x38 + 16, 0x1000);
    let mut update_copy = update.clone();
    set_u32(&mut update_copy, 0x38, 2);
    set_u64(&mut update_copy, 0x38 + 24, 0x3000_0000);
    let mut pages = message(Command::SetGuestPages, 0x28 + 8);
    set_u32(&mut pages, HEADER_SIZE + 8, 1);

    vec![
        allocation(0, &surface),
        allocation(0, &standard),
        allocation(0x20, &surface),
        map,
        update,
        update_copy,
        pages,
        message(Command::LockForCpu, 0x20),
        message(Command::CreateContext, 0x34),
        message(Command::CreateHardwareQueue, 0x44),
    ]
}

/// Makes two surfaces, the second in shared memory, and a good copy from one to the other.
fn surface_copy(session: &mut Session<StandIn, SparseMemory>) -> Vec<u8> {
    let mut surface = vec![0; 17];
    set_u64(&mut surface, 0, 0x10000);
    set_u32(&mut surface, 8, 64);
    set_u32(&mut surface, 12, 64);
    let mut out = Vec::new();
    let mut handle = || {
        let _ = session.handle_message(&allocation(0, &surface), TICKET, &mut out);
        u32_at(&out, 0x10).unwrap_or(0)
    };
    let (source, target) = (handle(), handle());
    let mut lock = message(Command::LockForCpu, 0x20);
    set_u32(&mut lock, 0x1C, target);
    let _ = session.handle_message(&lock, TICKET, &mut out);
    let mut copy = message(Command::CopyToStaging, 0x50);
    set_u32(&mut copy, 0x20, source);
    set_u32(&mut copy, 0x24, target);
    set_u32(&mut copy, 0x30, 64);
    set_u32(&mut copy, 0x34, 64);
    copy
}

#[test]
fn no_handler_panics_on_a_random_message() {
    let mut messages =
        RandomMessages::new(setting("FUZZ_SEED", DEFAULT_SEED), RandomMessages::all_commands());
    let mut session = fresh_session(0);
    let mut out = Vec::with_capacity(4096);

    for round in 0..setting("FUZZ_ROUNDS", DEFAULT_ROUNDS) {
        if round % ROUNDS_PER_SESSION == 0 {
            session = fresh_session(messages.number());
            let copy = surface_copy(&mut session);
            messages.set_templates(templates().into_iter().chain([copy]).collect());
        }
        let message = messages.message();
        let ticket = if messages.number() & 1 == 0 { TICKET } else { None };
        // The daemon must return for every message, and a panic here fails the test.
        let _ = session.handle_message(&message, ticket, &mut out);
        messages.learn(&out);
        // Polling is part of the loop the daemon runs, so that goes in too.
        let now = Instant::now();
        let _ = session.poll(if messages.number().is_multiple_of(64) { now + FAR_FUTURE } else { now });
        session.backend_mut().copy_done = messages.number().is_multiple_of(4);
    }
}
