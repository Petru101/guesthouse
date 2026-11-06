//! Any message may arrive in any order, and the daemon serves one VM on one thread, so a handler
//! that panics stops that VM. This sends random messages for every command through
//! `Session::handle_message` and checks that each one comes back.
//!
//! `FUZZ_ROUNDS` runs it for longer, and `FUZZ_SEED` picks where it starts.

use guesthouse_backend::{Backend, StagingCopy, SurfaceShape};
use guesthouse_backend_standin::{RandomMessages, SparseMemory, StandIn, WIDE_COLOUR};
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

/// Stand-in private data with its memory's size stated apart from its shape.
fn shape(memory: u64, width: u32, height: u32, pitch: u32, tiling: u32, colour: u32) -> Vec<u8> {
    let mut data = vec![0; 32];
    set_u64(&mut data, 0, memory);
    set_u32(&mut data, 8, width);
    set_u32(&mut data, 12, height);
    set_u32(&mut data, 20, pitch);
    set_u32(&mut data, 24, tiling);
    set_u32(&mut data, 28, colour);
    data
}

/// Surfaces at each edge of what a copy may do: (memory, width, height, pitch, tiling).
const SHAPES: [(u64, u32, u32, u32, u32); 13] = [
    (0x1_0000, 0, 64, 256, 0),
    (0x1_0000, 64, 0, 256, 0),
    (0x1_0000, 0, 0, 256, 0),
    (0x1_0000, 64, 64, 4, 0),
    (0x1000, 4096, 4096, 16384, 0),
    (0x1000, u32::MAX, u32::MAX, u32::MAX, 0),
    (0x1_0000, 0x4000_0000, 2, 0x4000_0000, 0),
    (0x1_0000, 64, 64, 256, 1),
    (0x1000, 4096, 4096, 16384, 5),
    (0x3C00, 64, 60, 256, 4),
    (0x10_0000, 64, 64, 256, 7),
    (0x10_0000, 64, 64, 256, u32::MAX),
    (0x10_0000, u32::MAX, u32::MAX, u32::MAX, 6),
];

/// Colour words: the usual one, one of wide pixels, and one nobody knows.
const COLOURS: [u32; 3] = [0, WIDE_COLOUR, 0xEE];

/// Rectangles at the same edges, as left, top, right and bottom.
const RECTANGLES: [[i32; 4]; 8] = [
    [0, 0, 64, 64],
    [0, 0, 64, 60],
    [0, 0, 0, 0],
    [0, 0, 0x4000_0000, 1],
    [0, 0, 1, i32::MAX],
    [i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX],
    [0x1_0000, 0x1_0000, 0x1_0001, 0x1_0001],
    [64, 64, 0, 0],
];

/// A copy for each shape and rectangle, in turn as the source and as the target, and
/// with a corner that lands past the end.
fn shape_copies(session: &mut Session<StandIn, SparseMemory>, good_copy: &[u8]) -> Vec<Vec<u8>> {
    let good = u32_at(good_copy, 0x20).unwrap_or(0);
    let mut out = Vec::new();
    let mut copies = Vec::new();
    for ((memory, width, height, pitch, tiling), colour) in
        SHAPES.into_iter().flat_map(|shape| COLOURS.map(|colour| (shape, colour)))
    {
        let private = shape(memory, width, height, pitch, tiling, colour);
        let _ = session.handle_message(&allocation(0, &private), TICKET, &mut out);
        let surface = u32_at(&out, 0x10).unwrap_or(0);
        for rectangle in RECTANGLES {
            for (source, target) in [(surface, good), (good, surface)] {
                let mut copy = good_copy.to_vec();
                set_u32(&mut copy, 0x20, source);
                set_u32(&mut copy, 0x24, target);
                for (index, value) in rectangle.into_iter().enumerate() {
                    set_u32(&mut copy, 0x28 + index * 4, value as u32);
                }
                copies.push(copy.clone());
                set_u32(&mut copy, 0x38, i32::MAX as u32);
                set_u32(&mut copy, 0x3C, 0x1_0000);
                copies.push(copy);
            }
        }
    }
    copies
}

/// The end of a tiled surface, walked to from `clc7b5.h` and not from the code under test: the block
/// holding its last byte, and the end of that block. GOBs are 64 bytes by 8 rows, a block is one GOB
/// wide and `1 << block` high, and blocks run along a row of blocks first. `None` for a block height
/// past `NVC7B5_SET_SRC_BLOCK_SIZE_HEIGHT_THIRTYTWO_GOBS`.
fn end_of_tiled_surface(pitch: u32, height: u32, block: u32) -> Option<u128> {
    if block > 5 {
        return None;
    }
    if pitch == 0 || height == 0 {
        return Some(0);
    }
    let block_rows = 8u128 << block;
    let blocks_across = (u128::from(pitch) - 1) / 64 + 1;
    let last_block_row = (u128::from(height) - 1) / block_rows;
    let last_block = last_block_row * blocks_across + blocks_across - 1;
    Some((last_block + 1) * 64 * block_rows)
}

/// Holds one tiled side of a copy to the header's sum and fields.
fn check_tiled_side(
    session: &Session<StandIn, SparseMemory>,
    copy: &StagingCopy,
    (surface, address): (&SurfaceShape, u64),
    (left, top): (i32, i32),
) {
    let block = surface.tiling.expect("only asked about tiled sides");
    let end = end_of_tiled_surface(surface.pitch, surface.height, block);
    let mapped = session.backend().mapped_length(copy.own_space, address);
    let (Some(end), Some(mapped)) = (end, mapped) else { panic!("not a surface: {copy:?}") };
    assert!(end <= u128::from(mapped), "past its memory: {copy:?}");
    // NVC7B5_SET_SRC_ORIGIN and SET_DST_ORIGIN have 16 bits for each.
    let pixel = i64::from(session.backend().copy_pixel_bytes(&copy.source));
    assert!(i64::from(left) * pixel < 1 << 16 && top < 1 << 16, "past its field: {copy:?}");
}

/// Holds each tiled side of each copy since `from` to the header's sum and fields. Returns how many
/// sources and targets it saw.
fn check_tiled_copies(session: &Session<StandIn, SparseMemory>, from: usize) -> (usize, usize) {
    let (mut sources, mut targets) = (0, 0);
    for copy in &session.backend().copies[from..] {
        if copy.source.tiling.is_some() {
            check_tiled_side(session, copy, (&copy.source, copy.source_address), (copy.left, copy.top));
            sources += 1;
        }
        if copy.target.tiling.is_some() {
            let corner = (copy.target_left, copy.target_top);
            check_tiled_side(session, copy, (&copy.target, copy.target_address), corner);
            targets += 1;
        }
    }
    (sources, targets)
}

#[test]
fn the_tiled_sum_here_is_the_headers() {
    assert_eq!(end_of_tiled_surface(16384, 2160, 4), Some(0x220_0000), "seen on a real 4096 by 2160");
    assert_eq!(end_of_tiled_surface(7680, 1080, 4), Some(7680 * 1152));
    assert_eq!(end_of_tiled_surface(65, 9, 0), Some(4 * 512));
    assert_eq!(end_of_tiled_surface(64, 8, 6), None);
}

#[test]
fn no_shape_of_surface_gets_a_copy_past_its_memory() {
    for flavour in [0, 16] {
        let mut session = fresh_session(flavour);
        let copy = surface_copy(&mut session);
        let mut out = Vec::new();
        for request in shape_copies(&mut session, &copy) {
            let _ = session.handle_message(&request, TICKET, &mut out);
            let _ = session.poll(Instant::now());
        }
        assert_eq!(session.backend().copies_past_their_memory, Vec::<String>::new());
        assert!(!session.backend().copies.is_empty(), "some of these are fine");
        let (sources, targets) = check_tiled_copies(&session, 0);
        assert!(sources > 0 && targets > 0, "some of the fine ones are tiled, on each side");
    }
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
            let shaped = shape_copies(&mut session, &copy);
            messages.set_templates(templates().into_iter().chain([copy]).chain(shaped).collect());
        }
        let copies_so_far = session.backend().copies.len();
        let message = messages.message();
        let ticket = if messages.number() & 1 == 0 { TICKET } else { None };
        // The daemon must return for every message, and a panic here fails the test.
        let _ = session.handle_message(&message, ticket, &mut out);
        messages.learn(&out);
        // Polling is part of the loop the daemon runs, so that goes in too.
        let now = Instant::now();
        let _ = session.poll(if messages.number().is_multiple_of(64) { now + FAR_FUTURE } else { now });
        session.backend_mut().copy_done = messages.number().is_multiple_of(4);
        assert_eq!(session.backend().copies_past_their_memory, Vec::<String>::new());
        check_tiled_copies(&session, copies_so_far);
    }
}
