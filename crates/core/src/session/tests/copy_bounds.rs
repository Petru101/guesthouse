//! What a copy may reach, through whole messages. The arithmetic has its own tests beside it.

use super::*;
use crate::CopyOrder;
use guesthouse_messages::wire::set_u32;

const UNSUCCESSFUL: u32 = Status::UNSUCCESSFUL.0;
const LEFT: usize = 0x28;
const TOP: usize = 0x2C;
const RIGHT: usize = 0x30;
const BOTTOM: usize = 0x34;
const TARGET_LEFT: usize = 0x38;
const TARGET_TOP: usize = 0x3C;

/// Stand-in private data for a surface, with the size of its memory stated apart from its shape.
fn shape(memory: u64, width: u32, height: u32, pitch: u32) -> Vec<u8> {
    let mut data = private(memory);
    data.resize(24, 0);
    set_u32(&mut data, 8, width);
    set_u32(&mut data, 12, height);
    set_u32(&mut data, 20, pitch);
    data
}

fn surface(session: &mut TestSession, memory: u64, width: u32, height: u32, pitch: u32) -> u32 {
    allocate(session, shape(memory, width, height, pitch)).1
}

fn copy(source: u32, target: u32, rectangle: [i32; 4], corner: [i32; 2]) -> Vec<u8> {
    let mut request = message(Command::CopyToStaging, 0x50);
    set_u32(&mut request, 0x20, source);
    set_u32(&mut request, 0x24, target);
    for (at, value) in [LEFT, TOP, RIGHT, BOTTOM].into_iter().zip(rectangle) {
        set_u32(&mut request, at, value as u32);
    }
    set_u32(&mut request, TARGET_LEFT, corner[0] as u32);
    set_u32(&mut request, TARGET_TOP, corner[1] as u32);
    request
}

/// Sends a copy, and says what the guest heard and how many copies reached the backend by then.
fn outcome(session: &mut TestSession, request: &[u8]) -> (u32, usize) {
    let status = status_of(&send(session, request));
    (status, session.backend().copies.len())
}

const ROW: u32 = 256 * 4;
const WHOLE: u64 = 256 * 256 * 4;

/// A surface of 256 by 256 with no padding, and as much memory as that takes.
fn plain(session: &mut TestSession) -> u32 {
    surface(session, WHOLE, 256, 256, ROW)
}

#[test]
fn a_copy_of_all_of_a_surface_goes_through() {
    let mut session = session();
    let (source, target) = (plain(&mut session), plain(&mut session));
    let request = copy(source, target, [0, 0, 256, 256], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (SUCCESS, 1));
}

#[test]
fn rows_padded_out_to_a_wider_pitch_go_through() {
    let mut session = session();
    let source = surface(&mut session, 5632 * 768, 1366, 768, 5632);
    let target = surface(&mut session, 5632 * 768, 1366, 768, 5632);
    let request = copy(source, target, [0, 0, 1366, 768], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (SUCCESS, 1));
}

#[test]
fn a_copy_of_part_of_a_surface_goes_through_at_its_far_corner() {
    let mut session = session();
    let (source, target) = (plain(&mut session), plain(&mut session));
    let request = copy(source, target, [128, 128, 256, 256], [128, 128]);
    assert_eq!(outcome(&mut session, &request), (SUCCESS, 1));
}

#[test]
fn a_shape_bigger_than_its_memory_is_refused() {
    let mut session = session();
    let source = surface(&mut session, WHOLE, 2048, 2048, 2048 * 4);
    let target = surface(&mut session, 2048 * 2048 * 4, 2048, 2048, 2048 * 4);
    let request = copy(source, target, [0, 0, 2048, 2048], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

#[test]
fn the_part_of_a_big_shape_that_lies_in_its_memory_goes_through() {
    let mut session = session();
    let source = surface(&mut session, 2048 * 4 * 8, 2048, 2048, 2048 * 4);
    let target = surface(&mut session, 2048 * 2048 * 4, 2048, 2048, 2048 * 4);
    let inside = copy(source, target, [0, 0, 2048, 8], [0, 0]);
    assert_eq!(outcome(&mut session, &inside), (SUCCESS, 1));
    let one_row_more = copy(source, target, [0, 0, 2048, 9], [0, 0]);
    assert_eq!(outcome(&mut session, &one_row_more), (UNSUCCESSFUL, 1));
}

#[test]
fn a_width_of_zero_lets_nothing_past_the_memory() {
    let mut session = session();
    let source = surface(&mut session, WHOLE, 0, 256, ROW);
    let target = surface(&mut session, WHOLE * 2, 512, 256, ROW * 2);
    let inside = copy(source, target, [0, 0, 256, 256], [0, 0]);
    assert_eq!(outcome(&mut session, &inside), (SUCCESS, 1));
    let wider_than_a_row = copy(source, target, [0, 0, 257, 1], [0, 0]);
    assert_eq!(outcome(&mut session, &wider_than_a_row), (UNSUCCESSFUL, 1));
}

#[test]
fn a_height_of_zero_lets_nothing_past_the_memory() {
    let mut session = session();
    let source = surface(&mut session, WHOLE, 256, 0, ROW);
    let target = surface(&mut session, WHOLE * 2, 256, 0, ROW);
    let inside = copy(source, target, [0, 0, 256, 256], [0, 0]);
    assert_eq!(outcome(&mut session, &inside), (SUCCESS, 1));
    let too_tall = copy(source, target, [0, 0, 256, 257], [0, 0]);
    assert_eq!(outcome(&mut session, &too_tall), (UNSUCCESSFUL, 1));
}

#[test]
fn a_target_with_no_stated_size_lets_nothing_past_the_memory() {
    let mut session = session();
    let source = surface(&mut session, WHOLE * 4, 0, 0, ROW);
    let target = surface(&mut session, WHOLE, 0, 0, ROW);
    let too_tall = copy(source, target, [0, 0, 256, 300], [0, 0]);
    assert_eq!(outcome(&mut session, &too_tall), (UNSUCCESSFUL, 0));
}

#[test]
fn a_pitch_smaller_than_the_row_is_refused_on_either_side() {
    let mut session = session();
    let narrow = surface(&mut session, WHOLE, 256, 256, ROW - 4);
    let good = plain(&mut session);
    let reading = copy(narrow, good, [0, 0, 256, 2], [0, 0]);
    assert_eq!(outcome(&mut session, &reading), (UNSUCCESSFUL, 0));
    let writing = copy(good, narrow, [0, 0, 256, 2], [0, 0]);
    assert_eq!(outcome(&mut session, &writing), (UNSUCCESSFUL, 0));
}

#[test]
fn a_row_that_starts_inside_its_pitch_must_end_there_too() {
    let mut session = session();
    let source = surface(&mut session, WHOLE, 0, 0, ROW);
    let target = plain(&mut session);
    let request = copy(source, target, [200, 0, 257, 1], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

#[test]
fn a_target_past_its_end_is_refused() {
    let mut session = session();
    let source = plain(&mut session);
    let target = surface(&mut session, WHOLE - 1, 256, 256, ROW);
    let request = copy(source, target, [0, 0, 256, 256], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

#[test]
fn a_corner_that_pushes_a_copy_past_the_target_is_refused() {
    let mut session = session();
    let (source, target) = (plain(&mut session), surface(&mut session, WHOLE, 0, 0, ROW));
    for corner in [[1, 0], [0, 1], [256, 0], [0, 256], [i32::MAX, 0], [0, i32::MAX], [-1, 0], [0, -1]] {
        let request = copy(source, target, [0, 0, 256, 256], corner);
        assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0), "{corner:?}");
    }
}

#[test]
fn sizes_that_would_wrap_are_refused() {
    let mut session = session();
    let vast = surface(&mut session, WHOLE, 0, 0, u32::MAX);
    let good = plain(&mut session);
    let rectangles = [
        [0, 0x4000_0000, 1, 0x4000_0001],
        [0, i32::MAX - 1, 256, i32::MAX],
        [0x4000_0000, 0, 0x4000_0001, 1],
        [0, 0, i32::MAX, 1],
        [0, 0, 1, i32::MAX],
        [i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX],
    ];
    for rectangle in rectangles {
        let reading = copy(vast, good, rectangle, [0, 0]);
        assert_eq!(outcome(&mut session, &reading), (UNSUCCESSFUL, 0), "{rectangle:?}");
    }
    let writing = copy(good, vast, [0, 0, 1, 2], [0, i32::MAX - 1]);
    assert_eq!(outcome(&mut session, &writing), (UNSUCCESSFUL, 0));
}

#[test]
fn a_refused_copy_maps_nothing() {
    let mut session = session();
    let source = surface(&mut session, 0x1000, 2048, 2048, 2048 * 4);
    let target = plain(&mut session);
    let mappings = session.backend().mapping_count();
    let request = copy(source, target, [0, 0, 2048, 2048], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
    assert_eq!(session.backend().mapping_count(), mappings);
}

#[test]
fn a_refused_copy_is_answered_at_once_in_the_held_order_too() {
    let mut session = session();
    session.set_copy_order(CopyOrder::Held);
    let source = surface(&mut session, 0x1000, 2048, 2048, 2048 * 4);
    let target = plain(&mut session);
    let request = copy(source, target, [0, 0, 2048, 2048], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
    assert!(session.copies.is_empty());
}

#[test]
fn the_next_good_copy_runs_after_a_refusal() {
    let mut session = session();
    let (source, target) = (plain(&mut session), plain(&mut session));
    let refused = copy(source, target, [0, 0, 256, 257], [0, 0]);
    assert_eq!(outcome(&mut session, &refused), (UNSUCCESSFUL, 0));
    let good = copy(source, target, [0, 0, 256, 256], [0, 0]);
    assert_eq!(outcome(&mut session, &good), (SUCCESS, 1));
}

#[test]
fn a_standard_surface_has_room_for_its_whole_rectangle() {
    let mut session = session();
    let image = surface(&mut session, 5632 * 768, 1366, 768, 5632);
    let mut record = b"GPVS".to_vec();
    record.resize(32, 0);
    set_u32(&mut record, 8, 1366);
    set_u32(&mut record, 12, 768);
    set_u32(&mut record, 20, 5632);
    let reply = send(&mut session, &create_message(0, &[record]));
    let staging = u32_at(&reply, 0x10).unwrap();
    let request = copy(image, staging, [0, 0, 1366, 768], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (SUCCESS, 1));
}

/// Stand-in private data for a tiled surface. Blocks there are 8 rows high, doubled `block` times.
fn tiled(memory: u64, width: u32, height: u32, pitch: u32, block: u32) -> Vec<u8> {
    let mut data = shape(memory, width, height, pitch);
    data.resize(28, 0);
    set_u32(&mut data, 24, block + 1);
    data
}

#[test]
fn a_tiled_source_that_fills_its_memory_goes_through() {
    let mut session = session();
    let (_, source) = allocate(&mut session, tiled(ROW as u64 * 256, 256, 250, ROW, 3));
    let target = plain(&mut session);
    let request = copy(source, target, [0, 0, 256, 250], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (SUCCESS, 1));
}

#[test]
fn a_tiled_source_bigger_than_its_memory_is_refused_even_for_one_row() {
    let mut session = session();
    let (_, source) = allocate(&mut session, tiled(ROW as u64 * 250, 256, 250, ROW, 3));
    let target = plain(&mut session);
    let request = copy(source, target, [0, 0, 1, 1], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

#[test]
fn a_block_height_the_engine_lacks_is_refused() {
    let mut session = session();
    let (_, source) = allocate(&mut session, tiled(WHOLE * 64, 256, 256, ROW, 6));
    let target = plain(&mut session);
    let request = copy(source, target, [0, 0, 256, 256], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

#[test]
fn rows_past_a_tiled_sources_height_are_refused_when_it_states_none() {
    let mut session = session();
    let (_, source) = allocate(&mut session, tiled(WHOLE, 256, 0, ROW, 0));
    let target = plain(&mut session);
    let request = copy(source, target, [0, 0, 256, 1], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

#[test]
fn a_row_past_a_tiled_sources_pitch_is_refused() {
    let mut session = session();
    let (_, source) = allocate(&mut session, tiled(WHOLE, 0, 256, ROW, 0));
    let target = plain(&mut session);
    let request = copy(source, target, [1, 0, 257, 1], [0, 0]);
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
}

const ADDRESS_STEP: u64 = 0x1_0000;

/// Whether the next addresses to hand out come straight after the ones `run` started from.
fn takes_no_addresses(session: &mut TestSession, run: impl FnOnce(&mut TestSession)) -> bool {
    let before = session.take_gpu_addresses(ADDRESS_STEP, ADDRESS_STEP).unwrap();
    run(session);
    session.take_gpu_addresses(ADDRESS_STEP, ADDRESS_STEP).unwrap() == before + ADDRESS_STEP
}

#[test]
fn a_surface_with_no_pages_yet_uses_up_no_addresses() {
    let mut session = session();
    let reply = send(&mut session, &create_message(0x20, &[shape(WHOLE, 256, 256, ROW)]));
    let waiting = u32_at(&reply, 0x10).unwrap();
    let target = plain(&mut session);
    let request = copy(waiting, target, [0, 0, 256, 256], [0, 0]);
    session.own_space().unwrap();
    let quiet = takes_no_addresses(&mut session, |session| {
        assert_eq!(outcome(session, &request), (UNSUCCESSFUL, 0));
    });
    assert!(quiet);
}

#[test]
fn a_map_that_fails_gives_its_addresses_back() {
    let mut session = session();
    let (source, target) = (plain(&mut session), plain(&mut session));
    let request = copy(source, target, [0, 0, 256, 256], [0, 0]);
    session.own_space().unwrap();
    let first = session.take_gpu_addresses(WHOLE, ADDRESS_STEP).unwrap();
    session.backend_mut().fail_next_map = true;
    assert_eq!(outcome(&mut session, &request), (UNSUCCESSFUL, 0));
    let mappings = session.allocations[&source].mappings.len();
    assert_eq!(mappings, 0);
    assert_eq!(
        session.take_gpu_addresses(WHOLE, ADDRESS_STEP).unwrap(),
        first + WHOLE,
        "the spare is reused"
    );
}

#[test]
fn every_refusal_is_answered_and_counted_however_many_there_are() {
    let mut session = session();
    let (source, target) = (plain(&mut session), plain(&mut session));
    let refused = copy(source, target, [0, 0, 256, 257], [0, 0]);
    for count in 1..=100 {
        assert_eq!(outcome(&mut session, &refused), (UNSUCCESSFUL, 0));
        assert_eq!(session.copies_refused, count);
    }
}
