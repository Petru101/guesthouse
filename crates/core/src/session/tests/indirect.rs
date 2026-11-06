//! The display driver's ring through a session: the stand-in backend, a pretend sink, and this test
//! playing the driver.

use super::*;
use crate::{Delivery, Frame, FrameRing, FrameSink, ViewerFacts};
use guesthouse_messages::indirect::as_the_driver_sends as driver;
use guesthouse_messages::indirect::{FrameDone, Look, Mastering, Register, RingName, Stamp};
use std::os::fd::BorrowedFd;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const PITCH: u32 = 2560;
const SLOT_BYTES: u64 = PITCH as u64 * HEIGHT as u64;
const SLOTS: u32 = 3;
const NAME: RingName = RingName { generation: 1, monitor: 0 };
const OTHER_PROCESS: u32 = PROCESS + 8;
const STATUS_OK: u32 = 0;
const STATUS_STALE: u32 = 1;
const STATUS_REFUSED: u32 = 2;
const ESCAPE_SIZE_AT: usize = HEADER_SIZE + 16;
const ESCAPE_HEADER: usize = HEADER_SIZE + 24;
const HDR_VIEWER: ViewerFacts = ViewerFacts { shows_hdr: true, refresh_millihertz: 240_000, peak_nits: 1000 };

#[derive(Default)]
struct Sink {
    rings: Vec<FrameRing>,
    frames: Vec<Frame>,
}

impl FrameSink for Sink {
    fn ring_opened(&mut self, ring: &FrameRing, buffers: &[BorrowedFd<'_>]) -> Delivery {
        assert_eq!(buffers.len(), ring.slots as usize);
        self.rings.push(*ring);
        Delivery::Sent
    }

    fn frame_ready(&mut self, frame: &Frame) -> Delivery {
        self.frames.push(*frame);
        Delivery::Sent
    }
}

fn escape_from(process: u32, data: &[u8]) -> Vec<u8> {
    let mut request = message(Command::DriverEscape, ESCAPE_HEADER);
    set_u32(&mut request, 8, process);
    set_u32(&mut request, ESCAPE_SIZE_AT, data.len() as u32);
    request.extend_from_slice(data);
    request
}

fn ring(look: Look) -> Register {
    Register {
        name: NAME,
        slots: SLOTS,
        width: WIDTH,
        height: HEIGHT,
        pitch: PITCH,
        slot_size: SLOT_BYTES,
        look,
    }
}

fn done(slot: Option<u32>) -> FrameDone {
    FrameDone { name: NAME, slot, present_time: 5, windows_frame: 1, sdr_white_nits: 240, mastering: None }
}

/// A guest with `SLOTS` stamped buffers, in reverse of their slot order to prove the stamps decide.
/// Returns the session and the buffers, by slot.
fn stamped(generation: u32) -> (TestSession, Vec<u32>) {
    let mut session = session();
    session.space_for(PROCESS).unwrap();
    let mut buffers = Vec::new();
    for index in 0..SLOTS {
        let (_, handle) = allocate(&mut session, private(SLOT_BYTES));
        map(&mut session, handle, 0, 0, 0);
        let memory = session.allocations[&handle].memory.unwrap();
        let slot = SLOTS - 1 - index;
        session.backend.memory_starts.insert(memory, driver::stamp(Stamp { generation, slot }));
        buffers.insert(0, handle);
    }
    (session, buffers)
}

/// Sends a REGISTER and lets it finish. Returns (status, first slot).
fn register(session: &mut TestSession, register: &Register) -> (u32, Option<u32>) {
    let message = escape_from(PROCESS, &driver::register(register));
    let mut out = Vec::new();
    let answer = session.handle_message(&message, Some(ReplyTicket::default()), &mut out);
    if answer == Answer::Now {
        return driver::reply(&out).unwrap();
    }
    let _ = session.poll(Instant::now());
    let replies: Vec<_> = session.take_replies().collect();
    assert_eq!(replies.len(), 1, "the driver's thread is waiting");
    driver::reply(&replies[0].bytes).unwrap()
}

fn say(session: &mut TestSession, process: u32, data: &[u8]) -> (u32, Option<u32>) {
    driver::reply(&send(session, &escape_from(process, data))).expect("a reply")
}

fn frame(session: &mut TestSession, slot: Option<u32>) -> (u32, Option<u32>) {
    say(session, PROCESS, &driver::frame(&done(slot)))
}

fn turn(session: &mut TestSession, sink: &mut Sink) {
    let _ = session.poll(Instant::now());
    session.offer_frames(sink);
}

fn registered() -> (TestSession, u32) {
    let (mut session, _) = stamped(NAME.generation);
    let (status, first) = register(&mut session, &ring(Look::SDR));
    assert_eq!(status, STATUS_OK);
    (session, first.expect("a slot to start with"))
}

#[test]
fn the_stamps_say_which_buffer_is_which_slot() {
    let (mut session, buffers) = stamped(1);
    let (status, first) = register(&mut session, &ring(Look::SDR));
    assert_eq!((status, first.is_some()), (STATUS_OK, true));
    let found: Vec<u32> =
        session.frames.indirect.as_ref().unwrap().slots.iter().map(|slot| slot.allocation).collect();
    assert_eq!(found, buffers);
}

#[test]
fn numbers_that_dont_add_up_are_refused() {
    let good = ring(Look::SDR);
    let bad = [
        Register { pitch: PITCH - 4, ..good },
        Register { pitch: u32::MAX, ..good },
        Register { slot_size: SLOT_BYTES - 1, ..good },
        Register { slot_size: u64::MAX, ..good },
        Register { width: 0, ..good },
        Register { height: 8193, ..good },
        Register { width: u32::MAX, height: u32::MAX, ..good },
    ];
    for ring in bad {
        let (mut session, _) = stamped(1);
        assert_eq!(register(&mut session, &ring), (STATUS_REFUSED, None), "{ring:?}");
        assert!(session.frames.indirect.is_none());
    }
}

#[test]
fn a_ring_with_a_slot_missing_its_stamp_is_refused() {
    let (mut session, buffers) = stamped(1);
    let memory = session.allocations[&buffers[1]].memory.unwrap();
    session.backend.memory_starts.remove(&memory);
    assert_eq!(register(&mut session, &ring(Look::SDR)), (STATUS_REFUSED, None));
}

#[test]
fn an_old_rings_stamps_dont_count() {
    let (mut session, _) = stamped(1);
    let newer = Register { name: RingName { generation: 2, monitor: 0 }, ..ring(Look::SDR) };
    assert_eq!(register(&mut session, &newer), (STATUS_REFUSED, None));
}

#[test]
fn a_surface_of_the_same_size_isnt_taken_for_a_slot() {
    let (mut session, buffers) = stamped(1);
    let (_, decoy) = allocate(&mut session, private(SLOT_BYTES));
    map(&mut session, decoy, 0, 0, 0);
    assert_eq!(register(&mut session, &ring(Look::SDR)).0, STATUS_OK);
    let found: Vec<u32> =
        session.frames.indirect.as_ref().unwrap().slots.iter().map(|slot| slot.allocation).collect();
    assert_eq!(found, buffers);
}

#[test]
fn a_process_with_no_space_gets_no_ring() {
    let mut session = session();
    let data = driver::register(&ring(Look::SDR));
    assert_eq!(say(&mut session, OTHER_PROCESS, &data), (STATUS_REFUSED, None));
}

#[test]
fn nonsense_with_our_magic_goes_back_as_it_came_and_never_reaches_the_vendor() {
    let mut session = session();
    let mut data = driver::close(NAME);
    data[12] = 1;
    let reply = send(&mut session, &escape_from(PROCESS, &data));
    assert_eq!(reply, data);
    assert_eq!(driver::reply(&reply), None, "the driver turns itself off");
}

#[test]
fn a_short_buffer_gets_no_reply_written_past_its_end() {
    let mut session = session();
    let data = driver::close(NAME);
    let short = &data[..24];
    let reply = send(&mut session, &escape_from(PROCESS, short));
    assert_eq!(reply, short);
}

#[test]
fn only_the_slot_handed_out_can_be_written() {
    let (mut session, first) = registered();
    let wrong = (first + 1) % SLOTS;
    assert_eq!(frame(&mut session, Some(wrong)), (STATUS_STALE, None));
    assert!(session.frames.indirect.is_none(), "a driver this confused starts again");
}

#[test]
fn a_slot_past_the_ring_is_just_a_wrong_slot() {
    let (mut session, _) = registered();
    assert_eq!(frame(&mut session, Some(u32::MAX - 1)), (STATUS_STALE, None));
}

#[test]
fn another_process_cant_speak_for_the_ring_or_take_it() {
    let (mut session, first) = registered();
    session.space_for(OTHER_PROCESS).unwrap();
    let frame_data = driver::frame(&done(Some(first)));
    assert_eq!(say(&mut session, OTHER_PROCESS, &frame_data), (STATUS_STALE, None));
    let ring_data = driver::register(&ring(Look::SDR));
    assert_eq!(say(&mut session, OTHER_PROCESS, &ring_data), (STATUS_REFUSED, None));
    say(&mut session, OTHER_PROCESS, &driver::close(NAME));
    assert!(session.frames.indirect.is_some());
}

#[test]
fn a_ring_nobody_has_gets_a_stale_answer() {
    let mut session = session();
    assert_eq!(frame(&mut session, Some(0)).0, STATUS_STALE);
}

#[test]
fn asking_again_names_the_slot_already_held() {
    let (mut session, first) = registered();
    assert_eq!(frame(&mut session, None), (STATUS_OK, Some(first)));
    assert_eq!(frame(&mut session, None), (STATUS_OK, Some(first)));
}

#[test]
fn a_frame_waits_for_the_guests_queues() {
    let (mut session, first) = registered();
    session.backend.run_work = true;
    let context = create_context(&mut session, 0);
    let (queue, _, _) = create_queue(&mut session, context);
    submit(&mut session, queue, 7);
    session.viewer_arrived();
    let mut sink = Sink::default();
    frame(&mut session, Some(first));
    turn(&mut session, &mut sink);
    assert!(sink.frames.is_empty(), "the GPU may still be writing it");

    let fence = session.queues[&queue].fence;
    session.memory.write_u64(fence, 7).unwrap();
    turn(&mut session, &mut sink);
    assert_eq!(sink.frames.len(), 1);
}

#[test]
fn a_viewer_gets_the_ring_and_frames_in_it() {
    let (mut session, first) = registered();
    session.viewer_arrived();
    let mut sink = Sink::default();
    let (status, next) = frame(&mut session, Some(first));
    assert_eq!(status, STATUS_OK);
    assert_ne!(next, Some(first));
    turn(&mut session, &mut sink);
    assert_eq!(sink.rings.len(), 1);
    assert_eq!(sink.rings[0].slots, SLOTS);
    assert_eq!(sink.frames.len(), 1);
    let shown = sink.frames[0];
    assert_eq!((shown.slot, shown.look), (first, Look::SDR));
    assert_eq!(shown.picture.pitch, PITCH);
    assert_eq!(shown.light.unwrap().sdr_white_nits, 240);
}

#[test]
fn frames_arrive_before_anyone_watches_and_the_newest_waits() {
    let (mut session, mut slot) = registered();
    for _ in 0..5 {
        slot = frame(&mut session, Some(slot)).1.expect("nobody is holding a slot");
        let _ = session.poll(Instant::now());
    }
    session.viewer_arrived();
    let mut sink = Sink::default();
    turn(&mut session, &mut sink);
    turn(&mut session, &mut sink);
    assert_eq!(sink.frames.len(), 1, "only the newest");
    assert_eq!(sink.frames[0].sequence, 5);
}

#[test]
fn the_next_viewer_gets_the_same_slots_and_the_last_picture() {
    let (mut session, first) = registered();
    session.viewer_arrived();
    let mut sink = Sink::default();
    frame(&mut session, Some(first));
    turn(&mut session, &mut sink);
    turn(&mut session, &mut sink);
    assert_eq!(sink.frames.len(), 1);

    session.viewer_left();
    assert!(session.frames.indirect.is_some(), "the guest's memory is still there");
    session.viewer_arrived();
    let mut next = Sink::default();
    turn(&mut session, &mut next);
    turn(&mut session, &mut next);
    assert_eq!(next.rings.len(), 1);
    assert!(next.rings[0].generation > sink.rings[0].generation, "its releases can't be confused");
    assert_eq!(next.frames.len(), 1, "a still desktop has nothing else to show");
    assert_eq!(next.frames[0].slot, first);

    session.slot_released(sink.rings[0].generation, first);
    assert!(!session.frames.ring.as_ref().unwrap().slots.has_free() || frame(&mut session, None).1.is_some());
}

#[test]
fn with_every_slot_with_the_viewer_the_driver_skips_until_one_comes_back() {
    let (mut session, first) = registered();
    session.viewer_arrived();
    let mut sink = Sink::default();
    let mut slot = Some(first);
    while let Some(writing) = slot {
        slot = frame(&mut session, Some(writing)).1;
        turn(&mut session, &mut sink);
        turn(&mut session, &mut sink);
    }
    assert_eq!(sink.frames.len(), SLOTS as usize);
    assert_eq!(frame(&mut session, None), (STATUS_OK, None));
    assert_eq!(session.frame_counts().skipped, 2);

    let shown = sink.frames[0];
    session.slot_released(shown.generation, shown.slot);
    assert_eq!(frame(&mut session, None), (STATUS_OK, Some(shown.slot)));
}

#[test]
fn hdr_frames_carry_their_look_and_light() {
    let (mut session, _) = stamped(1);
    let first = register(&mut session, &ring(Look::HDR)).1.unwrap();
    session.viewer_arrived();
    session.viewer_described(HDR_VIEWER);
    let mastering = Mastering { max_nits: 1000, ..Mastering::default() };
    let data = driver::frame(&FrameDone { mastering: Some(mastering), ..done(Some(first)) });
    say(&mut session, PROCESS, &data);
    let mut sink = Sink::default();
    turn(&mut session, &mut sink);
    assert_eq!(sink.frames[0].look, Look::HDR);
    assert_eq!(sink.frames[0].light.unwrap().mastering, Some(mastering));
}

#[test]
fn a_viewer_that_cant_show_hdr_isnt_sent_it() {
    let (mut session, _) = stamped(1);
    let first = register(&mut session, &ring(Look::HDR)).1.unwrap();
    session.viewer_arrived();
    frame(&mut session, Some(first));
    let mut sink = Sink::default();
    turn(&mut session, &mut sink);
    assert!(sink.frames.is_empty());
}

#[test]
fn the_reply_passes_on_what_the_viewer_said_of_its_monitor() {
    let (mut session, first) = registered();
    session.viewer_arrived();
    session.viewer_described(HDR_VIEWER);
    let reply = send(&mut session, &escape_from(PROCESS, &driver::frame(&done(Some(first)))));
    assert_eq!(u32_at(&reply, 32), Ok(240_000));
    assert_eq!(u32_at(&reply, 36), Ok(1000));
}

#[test]
fn closing_ends_the_ring_and_a_new_one_can_follow() {
    let (mut session, _) = registered();
    assert_eq!(say(&mut session, PROCESS, &driver::close(NAME)).0, STATUS_OK);
    assert!(session.frames.indirect.is_none());
    assert!(session.frames.ring.is_none());
    assert_eq!(register(&mut session, &ring(Look::SDR)).0, STATUS_OK);
}

#[test]
fn the_driver_replacing_its_ring_needs_no_close() {
    let (mut session, _) = registered();
    assert_eq!(register(&mut session, &ring(Look::SDR)).0, STATUS_OK);
}

#[test]
fn destroying_a_slot_ends_the_ring() {
    let (mut session, buffers) = stamped(1);
    register(&mut session, &ring(Look::SDR));
    let resource = session.allocations[&buffers[0]].resource;
    destroy(&mut session, resource, &[buffers[0]]);
    assert!(session.frames.indirect.is_none());
    assert_eq!(frame(&mut session, None).0, STATUS_STALE);
}

#[test]
fn a_slot_whose_memory_moved_ends_the_ring() {
    let (mut session, buffers) = stamped(1);
    let first = register(&mut session, &ring(Look::SDR)).1;
    session.allocations.get_mut(&buffers[0]).unwrap().memory = None;
    assert_eq!(frame(&mut session, first).0, STATUS_STALE);
    assert!(session.frames.indirect.is_none());
}

#[test]
fn the_process_going_ends_the_ring() {
    let (mut session, _) = registered();
    assert_eq!(status_of(&send(&mut session, &message(Command::DestroyProcess, HEADER_SIZE))), SUCCESS);
    assert!(session.frames.indirect.is_none());
}

#[test]
fn an_export_that_fails_stops_trying_and_the_guest_carries_on() {
    let (mut session, first) = registered();
    session.backend.refuse_exports = true;
    session.viewer_arrived();
    let mut sink = Sink::default();
    let next = frame(&mut session, Some(first)).1;
    turn(&mut session, &mut sink);
    assert!(sink.rings.is_empty() && sink.frames.is_empty());
    assert!(session.frames.indirect.as_ref().unwrap().export_refused);
    assert_eq!(frame(&mut session, next).0, STATUS_OK);
}

#[test]
fn copies_to_the_old_screen_leave_the_ring_alone() {
    let (mut session, first) = registered();
    session.viewer_arrived();
    let mut sink = Sink::default();
    frame(&mut session, Some(first));
    turn(&mut session, &mut sink);
    let (_, image) = allocate(&mut session, private(0x200000));
    let (_, staging) = allocate(&mut session, private(0x200000));
    lock(&mut session, staging);
    let mut copy = message(Command::CopyToStaging, 0x50);
    set_u32(&mut copy, 0x20, image);
    set_u32(&mut copy, 0x24, staging);
    let mut out = Vec::new();
    let _ = session.handle_message(&copy, Some(ReplyTicket::default()), &mut out);
    let _ = session.poll(Instant::now() + Duration::from_secs(1));
    assert!(session.frames.ring.is_some() && session.frames.indirect.is_some());
    assert!(session.backend.slot_copies.is_empty());
}

#[test]
fn a_ring_that_ends_gives_the_screen_back_to_copies() {
    let (mut session, _) = registered();
    session.viewer_arrived();
    say(&mut session, PROCESS, &driver::close(NAME));
    assert!(session.frames.indirect.is_none());
    assert!(session.frames.ring.is_none(), "the next copy to the screen brings a ring of its own");
}
