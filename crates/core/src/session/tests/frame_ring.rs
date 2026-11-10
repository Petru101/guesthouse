//! The frame ring through a session: a screen's copies with the stand-in backend, and a pretend sink.

use super::*;
use crate::{CopyOrder, Delivery, Frame, FrameRing, FrameSink, ViewerFacts};
use std::os::fd::BorrowedFd;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const SURFACE_BYTES: u64 = 2 << 20;
const SLOTS: usize = 5;

/// A sink that says what the test tells it to.
struct Sink {
    answer: Delivery,
    rings: Vec<FrameRing>,
    frames: Vec<Frame>,
}

impl Sink {
    fn new() -> Self {
        Self { answer: Delivery::Sent, rings: Vec::new(), frames: Vec::new() }
    }
}

impl FrameSink for Sink {
    fn ring_opened(&mut self, ring: &FrameRing, buffers: &[BorrowedFd<'_>]) -> Delivery {
        if self.answer == Delivery::Sent {
            assert_eq!(buffers.len(), ring.slots as usize);
            self.rings.push(*ring);
        }
        self.answer
    }

    fn frame_ready(&mut self, frame: &Frame) -> Delivery {
        if self.answer == Delivery::Sent {
            self.frames.push(*frame);
        }
        self.answer
    }
}

fn sized_surface(width: u32, height: u32) -> Vec<u8> {
    let mut data = private(SURFACE_BYTES);
    set_u32(&mut data, 8, width);
    set_u32(&mut data, 12, height);
    data
}

/// Returns (image, staging), the staging surface in the shared window as a real screen's is.
fn screen(session: &mut TestSession, width: u32, height: u32) -> (u32, u32) {
    let (_, image) = allocate_primary(session, sized_surface(width, height));
    let (_, staging) = allocate(session, sized_surface(width, height));
    lock(session, staging);
    (image, staging)
}

fn copy(source: u32, target: u32) -> Vec<u8> {
    let mut request = message(Command::CopyToStaging, 0x50);
    set_u32(&mut request, 0x20, source);
    set_u32(&mut request, 0x24, target);
    request
}

fn present(session: &mut TestSession, (image, staging): (u32, u32)) {
    assert_eq!(status_of(&send(session, &copy(image, staging))), SUCCESS);
}

/// Long enough after the last copy for the ring to catch up.
fn later() -> Instant {
    Instant::now() + Duration::from_secs(1)
}

/// A session with a viewer, a ring, and every copy so far delivered.
fn watched() -> (TestSession, (u32, u32), Sink) {
    let mut session = session();
    let surfaces = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, surfaces);
    session.viewer_arrived();
    let mut sink = Sink::new();
    let _ = session.poll(later());
    session.offer_frames(&mut sink);
    assert_eq!(sink.frames.len(), 1, "a new ring fetches the picture on the screen");
    (session, surfaces, sink)
}

#[test]
fn with_nobody_watching_nothing_is_made_or_copied() {
    let mut session = session();
    let surfaces = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, surfaces);
    let blocks = session.backend().live_memory_blocks();
    present(&mut session, surfaces);
    assert_eq!(session.poll(later()), Waiting::Nothing);
    assert_eq!(session.backend().live_memory_blocks(), blocks);
    assert!(session.backend().slot_copies.is_empty());
}

#[test]
fn a_viewer_gets_a_ring_for_the_screens_size_and_its_picture() {
    let (session, _, sink) = watched();
    let ring = sink.rings[0];
    assert_eq!(ring.slots as usize, SLOTS);
    assert_eq!(ring.size, 2560 * 480 / 0x10000 * 0x10000 + 0x10000);
    let frame = sink.frames[0];
    assert_eq!((frame.generation, frame.sequence), (ring.generation, 1));
    assert_eq!((frame.picture.width, frame.picture.height, frame.picture.pitch), (640, 480, 2560));
    assert_eq!(session.backend().slot_copies.len(), 1);
}

#[test]
fn a_viewer_that_comes_before_the_screen_waits_for_it() {
    let mut session = session();
    session.viewer_arrived();
    let mut sink = Sink::new();
    let _ = session.poll(later());
    session.offer_frames(&mut sink);
    assert!(sink.rings.is_empty());
    let surfaces = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, surfaces);
    assert_eq!(session.poll(Instant::now()), Waiting::Background, "the picture is still to come");
    let _ = session.poll(later());
    session.offer_frames(&mut sink);
    assert_eq!((sink.rings.len(), sink.frames.len()), (1, 1));
}

#[test]
fn each_frame_goes_to_a_slot_nobody_has() {
    let (mut session, surfaces, mut sink) = watched();
    for _ in 0..SLOTS - 1 {
        present(&mut session, surfaces);
        session.offer_frames(&mut sink);
    }
    let mut slots: Vec<u32> = sink.frames.iter().map(|frame| frame.slot).collect();
    slots.sort();
    assert_eq!(slots, [0, 1, 2, 3, 4]);
    let addresses = session.backend().slot_copies.iter().map(|copy| copy.slot_address);
    let distinct: HashSet<u64> = addresses.collect();
    assert_eq!(distinct.len(), SLOTS);
}

#[test]
fn a_frame_with_no_free_slot_is_skipped_and_the_guest_never_knows() {
    let (mut session, surfaces, mut sink) = watched();
    for _ in 0..SLOTS - 1 {
        present(&mut session, surfaces);
        session.offer_frames(&mut sink);
    }
    let copies = session.backend().slot_copies.len();
    for _ in 0..100 {
        present(&mut session, surfaces);
    }
    assert_eq!(session.backend().slot_copies.len(), copies, "every slot is the viewer's");
    assert_eq!(session.frame_counts().skipped, 100);
    assert_eq!(session.poll(later()), Waiting::Nothing, "and the main loop isn't spinning on it");
}

#[test]
fn a_released_slot_brings_the_skipped_picture_once_the_screen_is_still() {
    let (mut session, surfaces, mut sink) = watched();
    for _ in 0..SLOTS {
        present(&mut session, surfaces);
        session.offer_frames(&mut sink);
    }
    let first = sink.frames[0];
    session.slot_released(first.generation, first.slot);
    let _ = session.poll(Instant::now());
    session.offer_frames(&mut sink);
    assert_eq!(sink.frames.len(), SLOTS, "a frame may be on its way, and that's cheaper");
    let _ = session.poll(later());
    session.offer_frames(&mut sink);
    assert_eq!(sink.frames.len(), SLOTS + 1);
    assert_eq!(sink.frames[SLOTS].slot, first.slot);
}

#[test]
fn a_release_of_a_slot_the_viewer_lacks_changes_nothing() {
    let (mut session, surfaces, mut sink) = watched();
    for _ in 0..SLOTS - 1 {
        present(&mut session, surfaces);
        session.offer_frames(&mut sink);
    }
    let generation = sink.rings[0].generation;
    session.slot_released(generation + 1, 0);
    session.slot_released(generation - 1, 0);
    session.slot_released(generation, SLOTS as u32);
    let copies = session.backend().slot_copies.len();
    present(&mut session, surfaces);
    assert_eq!(session.backend().slot_copies.len(), copies, "no slot was freed");
}

#[test]
fn a_frame_that_cannot_leave_is_replaced_by_a_newer_one() {
    let (mut session, surfaces, mut sink) = watched();
    sink.answer = Delivery::NotNow;
    for _ in 0..50 {
        present(&mut session, surfaces);
        session.offer_frames(&mut sink);
    }
    assert_eq!(session.frame_counts().replaced, 49);
    assert_eq!(session.frame_counts().skipped, 0, "replaced slots came back");
    sink.answer = Delivery::Sent;
    session.offer_frames(&mut sink);
    assert_eq!(sink.frames.last().unwrap().sequence, 51, "only the newest ever leaves");
    assert_eq!(sink.frames.len(), 2);
}

#[test]
fn a_ring_that_cannot_leave_yet_holds_its_frames_back() {
    let mut session = session();
    let surfaces = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, surfaces);
    session.viewer_arrived();
    let _ = session.poll(later());
    let mut sink = Sink::new();
    sink.answer = Delivery::NotNow;
    session.offer_frames(&mut sink);
    sink.answer = Delivery::Sent;
    session.offer_frames(&mut sink);
    assert_eq!((sink.rings.len(), sink.frames.len()), (1, 1));
}

#[test]
fn the_guest_hears_nothing_of_a_copy_into_the_ring_that_fails() {
    let (mut session, surfaces, mut sink) = watched();
    session.backend_mut().slot_copy_status = Status::UNSUCCESSFUL;
    present(&mut session, surfaces);
    session.offer_frames(&mut sink);
    assert_eq!(sink.frames.len(), 1, "no new frame");
    session.backend_mut().slot_copy_status = Status::SUCCESS;
    present(&mut session, surfaces);
    session.offer_frames(&mut sink);
    assert_eq!(sink.frames.len(), 2, "and the failed slot is free again");
}

#[test]
fn a_viewer_that_leaves_gives_the_memory_back_and_the_next_gets_its_own() {
    let (mut session, surfaces, sink) = watched();
    let blocks = session.backend().live_memory_blocks();
    let maps = session.backend().mapping_count();
    session.viewer_left();
    assert_eq!(session.backend().live_memory_blocks(), blocks - SLOTS);
    assert_eq!(session.backend().mapping_count(), maps - SLOTS);
    let copies = session.backend().slot_copies.len();
    present(&mut session, surfaces);
    assert_eq!(session.backend().slot_copies.len(), copies, "the guest goes on with nobody watching");

    session.viewer_arrived();
    let mut next = Sink::new();
    let _ = session.poll(later());
    session.offer_frames(&mut next);
    assert!(next.rings[0].generation > sink.rings[0].generation);
    assert_eq!(next.frames.len(), 1);
}

#[test]
fn a_viewer_on_another_gpu_gets_slots_in_memory_here_and_they_go_with_it() {
    let mut session = session();
    let surfaces = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, surfaces);
    session.backend_mut().refuse_exports = true;
    session.viewer_arrived();
    session.viewer_described(ViewerFacts { wants_memory: true, ..ViewerFacts::default() });
    let mut sink = Sink::new();
    let _ = session.poll(later());
    session.offer_frames(&mut sink);
    assert_eq!(sink.rings.len(), 1, "no slot was exported from the card");
    assert_eq!(sink.frames.len(), 1);
    assert_eq!(session.memory().host_buffers.len(), SLOTS);
    let blocks = session.backend().live_memory_blocks();
    session.viewer_left();
    assert!(session.memory().host_buffers.is_empty());
    assert_eq!(session.backend().live_memory_blocks(), blocks - SLOTS);
}

#[test]
fn a_sink_whose_viewer_has_gone_ends_the_ring() {
    let (mut session, surfaces, mut sink) = watched();
    let blocks = session.backend().live_memory_blocks();
    sink.answer = Delivery::Gone;
    present(&mut session, surfaces);
    session.offer_frames(&mut sink);
    assert_eq!(session.backend().live_memory_blocks(), blocks - SLOTS);
}

#[test]
fn a_new_screen_size_makes_a_new_generation_and_old_releases_do_nothing() {
    let (mut session, _, mut sink) = watched();
    let old = sink.frames[0];
    let bigger = screen(&mut session, 800, 600);
    present(&mut session, bigger);
    let _ = session.poll(later());
    session.offer_frames(&mut sink);
    assert_eq!(sink.rings.len(), 2);
    assert!(sink.rings[1].generation > sink.rings[0].generation);
    let frame = *sink.frames.last().unwrap();
    assert_eq!((frame.generation, frame.picture.width), (sink.rings[1].generation, 800));
    session.slot_released(old.generation, frame.slot);
    let free_before = session.frames.ring.as_mut().unwrap().slots.released(frame.slot);
    assert_eq!(free_before, Ok(()), "the old ring's release didn't touch this ring's slot");
}

#[test]
fn a_windowed_program_between_the_desktops_frames_leaves_the_ring_alone() {
    let (mut session, desktop, mut sink) = watched();
    let (_, back_buffer) = allocate(&mut session, sized_surface(800, 600));
    let (_, its_staging) = allocate(&mut session, sized_surface(800, 600));
    lock(&mut session, its_staging);
    for _ in 0..3 {
        present(&mut session, (back_buffer, its_staging));
        present(&mut session, desktop);
        session.offer_frames(&mut sink);
    }
    assert_eq!(sink.rings.len(), 1);
    assert_eq!(sink.frames.len(), 4);
    assert!(sink.frames.iter().all(|frame| frame.picture.width == WIDTH));
}

#[test]
fn another_surface_of_the_same_size_keeps_the_ring() {
    let (mut session, _, mut sink) = watched();
    let other = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, other);
    session.offer_frames(&mut sink);
    assert_eq!((sink.rings.len(), sink.frames.len()), (1, 2));
}

#[test]
fn a_ring_that_cannot_be_made_costs_the_guest_nothing_and_is_not_retried() {
    let mut session = session();
    let surfaces = screen(&mut session, WIDTH, HEIGHT);
    present(&mut session, surfaces);
    let blocks = session.backend().live_memory_blocks();
    session.backend_mut().refuse_exports = true;
    session.viewer_arrived();
    assert_eq!(session.poll(later()), Waiting::Nothing);
    assert_eq!(session.backend().live_memory_blocks(), blocks, "what was made has gone again");
    present(&mut session, surfaces);
    assert!(session.backend().slot_copies.is_empty());
}

#[test]
fn an_image_of_the_screens_size_is_read_directly_and_anything_else_by_the_staging_surface() {
    let (mut session, (image, staging), _) = watched();
    let (_, small) = allocate_primary(&mut session, sized_surface(320, 240));
    present(&mut session, (image, staging));
    present(&mut session, (small, staging));
    let copies = &session.backend().slot_copies;
    let [.., direct, by_staging] = &copies[..] else { panic!("two copies") };
    assert_eq!((direct.source.width, direct.width), (WIDTH, WIDTH));
    let staged = session.allocations[&staging].mappings.iter().map(|mapping| mapping.address);
    assert!(staged.clone().any(|address| address == by_staging.source_address));
    assert!(!staged.clone().any(|address| address == direct.source_address));
}

/// A copy of the first row only, which is all that a short surface lets through.
fn present_one_row(session: &mut TestSession, (image, staging): (u32, u32)) {
    let mut request = copy(image, staging);
    set_u32(&mut request, 0x34, 1);
    assert_eq!(status_of(&send(session, &request)), SUCCESS);
}

fn short_surface() -> Vec<u8> {
    let mut data = sized_surface(WIDTH, HEIGHT);
    data[..8].copy_from_slice(&0x1000u64.to_le_bytes());
    data
}

#[test]
fn a_surface_smaller_than_its_numbers_is_never_read_past_its_end() {
    let mut session = session();
    let (_, image) = allocate_primary(&mut session, short_surface());
    let (_, staging) = allocate(&mut session, short_surface());
    lock(&mut session, staging);
    present_one_row(&mut session, (image, staging));
    session.viewer_arrived();
    let _ = session.poll(later());
    present_one_row(&mut session, (image, staging));
    assert!(session.backend().slot_copies.is_empty());
}

#[test]
fn a_short_image_leaves_the_ring_to_read_the_staging_surface() {
    let mut session = session();
    let (_, image) = allocate_primary(&mut session, short_surface());
    let (_, staging) = allocate(&mut session, sized_surface(WIDTH, HEIGHT));
    lock(&mut session, staging);
    present_one_row(&mut session, (image, staging));
    session.viewer_arrived();
    let _ = session.poll(later());
    present_one_row(&mut session, (image, staging));
    let copies = &session.backend().slot_copies;
    assert!(!copies.is_empty());
    let target_address = session.backend().copies[0].target_address;
    assert!(copies.iter().all(|copy| copy.source_address == target_address));
}

#[test]
fn a_tiled_image_short_of_its_last_block_reaches_neither_the_screen_nor_the_ring() {
    let mut session = session();
    // 470 rows fit the memory, and the last block of 128 rows ends at 512.
    let height = 470;
    let mut tiled = sized_surface(WIDTH, height);
    tiled[..8].copy_from_slice(&(2560 * 500u64).to_le_bytes());
    tiled.resize(28, 0);
    set_u32(&mut tiled, 24, 5);
    let (_, image) = allocate(&mut session, tiled);
    let (_, staging) = allocate(&mut session, sized_surface(WIDTH, height));
    lock(&mut session, staging);
    let refused = status_of(&send(&mut session, &copy(image, staging)));
    assert_eq!(refused, Status::UNSUCCESSFUL.0, "the staging copy needs the whole surface to fit too");
    assert!(session.backend().slot_copies.is_empty());
}

mod held {
    use super::*;

    fn held_session() -> (TestSession, (u32, u32), Sink) {
        let (mut session, surfaces, sink) = watched();
        session.set_copy_order(CopyOrder::Held);
        session.backend_mut().slow_copies = true;
        (session, surfaces, sink)
    }

    fn send_with_ticket(session: &mut TestSession, message: &[u8]) -> (Answer, Vec<u8>) {
        let mut out = Vec::new();
        let answer = session.handle_message(message, Some(ReplyTicket::default()), &mut out);
        (answer, out)
    }

    fn step_the_gpu(session: &mut TestSession) {
        session.backend_mut().copy_done = true;
        let _ = session.poll(Instant::now());
    }

    #[test]
    fn the_guest_waits_for_both_copies_and_a_frame_only_for_the_gpu() {
        let (mut session, (image, staging), mut sink) = held_session();
        let copies = session.backend().slot_copies.len();
        let (answer, _) = send_with_ticket(&mut session, &copy(image, staging));
        assert_eq!(answer, Answer::Held);
        step_the_gpu(&mut session);
        assert_eq!(session.backend().slot_copies.len(), copies + 1, "the second follows the first");
        assert_eq!(session.take_replies().count(), 0, "it still reads the guest's image");
        session.offer_frames(&mut sink);
        assert_eq!(sink.frames.len(), 1, "not finished on the GPU, so not sent");
        step_the_gpu(&mut session);
        assert_eq!(session.take_replies().next().unwrap().bytes, SUCCESS.to_le_bytes());
        session.offer_frames(&mut sink);
        assert_eq!(sink.frames.len(), 2);
    }

    #[test]
    fn other_messages_are_served_while_the_ring_copy_runs() {
        let (mut session, (image, staging), _) = held_session();
        let _ = send_with_ticket(&mut session, &copy(image, staging));
        step_the_gpu(&mut session);
        let reply = send(&mut session, &message(Command::FlushDevice, HEADER_SIZE));
        assert_eq!(status_of(&reply), SUCCESS);
    }

    #[test]
    fn a_ring_being_written_outlives_its_viewer_until_the_gpu_is_done() {
        let (mut session, (image, staging), _) = held_session();
        let blocks = session.backend().live_memory_blocks();
        let _ = send_with_ticket(&mut session, &copy(image, staging));
        step_the_gpu(&mut session);
        session.viewer_left();
        assert_eq!(session.backend().live_memory_blocks(), blocks, "a copy into it is under way");
        step_the_gpu(&mut session);
        let _ = session.poll(Instant::now());
        assert_eq!(session.backend().live_memory_blocks(), blocks - SLOTS);
        assert_eq!(session.take_replies().count(), 1, "and the guest got its reply");
    }

    #[test]
    fn a_failed_staging_copy_starts_no_ring_copy() {
        let (mut session, (image, staging), _) = held_session();
        let copies = session.backend().slot_copies.len();
        session.backend_mut().slow_copies = false;
        let source = session.allocations.get_mut(&image).unwrap();
        source.surface = None;
        let reply = send(&mut session, &copy(image, staging));
        assert_ne!(status_of(&reply), SUCCESS);
        assert_eq!(session.backend().slot_copies.len(), copies);
    }
}
