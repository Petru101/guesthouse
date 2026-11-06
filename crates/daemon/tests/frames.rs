//! The frame socket end to end, with this test playing QEMU and the viewer over real sockets, and
//! the pretend RM playing the card. This proves the bookkeeping and the commands. It proves nothing
//! about a real card, a real dmabuf, or how long anything takes.

use guesthouse_backend::Backend;
use guesthouse_backend_nvidia::NvidiaBackend;
use guesthouse_backend_nvidia::fake_guest::{
    copy_to_staging, create_coloured_surfaces, create_surfaces, fake_backend, lock_surface, message,
    set_io_space, surface_handles,
};
use guesthouse_backend_nvidia::rm::fake::{Call, FakeRm};
use guesthouse_backend_nvidia::rm::fake_engine::FakeEngine;
use guesthouse_backend_standin::SparseMemory;
use guesthouse_core::{Config, CopyOrder, MemoryLimits, Session};
use guesthouse_daemon::router::{Channel, Router};
use guesthouse_daemon::server::Server;
use guesthouse_frame_socket::{
    Colour, Format, Frame, FrameSocket, Hello, Light, Message, Packet, Release, Ring, SOCKET_NAME,
};
use guesthouse_messages::{Command, HEADER_SIZE, Status};
use guesthouse_transport::{FrameFlags, FrameHeader, FrameKind, HEADER_SIZE as FRAME_HEADER_SIZE};
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::{env, process};

const WINDOW: u64 = 1 << 40;
const WINDOW_END: u64 = WINDOW + (1 << 30);
const WIDTH: u32 = 100;
const HEIGHT: u32 = 40;
/// 400 bytes of pixels a row, rounded up to where rows may start.
const PITCH: u32 = 512;
const SLOTS: u32 = 5;
const STEP: Option<Duration> = Some(Duration::from_millis(2));
/// Far more turns of the loop than anything here needs, so a failure ends instead of hanging.
const MOST_STEPS: usize = 500;
/// The guest driver's word for 10 bits a colour.
const COLOUR_10_BIT: u8 = 0x36;
const PAST_THE_COPY_TIMEOUT: Duration = Duration::from_secs(3);
const WANTS_COMPLETION: FrameFlags = FrameFlags(1);
const XRGB8888: Format = Format { fourcc: u32::from_le_bytes(*b"XR24"), modifier: 0 };
const SUCCESS: [u8; 4] = Status::SUCCESS.0.to_le_bytes();

type TestServer = Server<NvidiaBackend<FakeRm>, SparseMemory>;

static NEXT_FOLDER: AtomicU32 = AtomicU32::new(0);

/// A daemon with a guest that has a screen, in a run folder of its own.
struct Rig {
    folder: PathBuf,
    server: TestServer,
    qemu: UnixStream,
    image: u32,
    staging: u32,
    transaction: u64,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

impl Rig {
    fn new() -> Self {
        let serial = NEXT_FOLDER.fetch_add(1, Ordering::Relaxed);
        let folder = env::temp_dir().join(format!("guesthouse-frames-{}-{serial}", process::id()));
        let mut rm = FakeRm::new();
        rm.engine.running = true;
        let backend = fake_backend(rm);
        let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
        let mut config = Config::new(limits);
        config.frame_wait = Duration::ZERO;
        let session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
        let mut server = Server::bind(&folder, Router::new(session)).unwrap();
        let qemu = UnixStream::connect(folder.join(Channel::Vgpu.socket_name())).unwrap();
        server.step(STEP).unwrap();
        qemu.set_nonblocking(true).unwrap();
        let mut rig = Self { folder, server, qemu, image: 0, staging: 0, transaction: 0 };
        rig.call(&set_io_space(WINDOW));
        (rig.image, rig.staging) = rig.new_surfaces(WIDTH, HEIGHT);
        rig
    }

    fn new_surfaces(&mut self, width: u32, height: u32) -> (u32, u32) {
        let (image, staging) = surface_handles(&self.call(&create_surfaces(width, height)));
        self.call(&lock_surface(staging));
        (image, staging)
    }

    fn step(&mut self) {
        self.server.step(STEP).unwrap();
    }

    /// Sends a message the way QEMU does.
    fn post(&mut self, payload: &[u8]) {
        self.transaction += 1;
        let header = FrameHeader {
            payload_len: payload.len() as u32,
            kind: FrameKind::Inband,
            flags: WANTS_COMPLETION,
            transaction: self.transaction,
        };
        self.qemu.write_all(&header.encode()).unwrap();
        self.qemu.write_all(payload).unwrap();
    }

    /// Whatever reply has come, after one turn of the loop.
    fn try_reply(&mut self) -> Option<Vec<u8>> {
        self.step();
        let mut header = [0; FRAME_HEADER_SIZE];
        match self.qemu.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return None,
            Err(error) => panic!("{error}"),
        }
        let mut payload = vec![0; FrameHeader::parse(&header).payload_len as usize];
        self.qemu.read_exact(&mut payload).unwrap();
        Some(payload)
    }

    fn call(&mut self, payload: &[u8]) -> Vec<u8> {
        self.post(payload);
        (0..MOST_STEPS).find_map(|_| self.try_reply()).expect("the guest got no reply")
    }

    /// One frame from the guest, which must always get its answer.
    fn present(&mut self) {
        let copy = copy_to_staging(self.image, self.staging, WIDTH, HEIGHT);
        assert_eq!(self.call(&copy), SUCCESS);
    }

    fn rm(&mut self) -> &mut FakeRm {
        &mut self.server.router_mut().session_mut().backend_mut().device_mut().rm
    }

    fn engine(&mut self) -> &mut FakeEngine {
        &mut self.rm().engine
    }

    fn set_order(&mut self, order: CopyOrder) {
        self.server.router_mut().session_mut().set_copy_order(order);
    }

    /// A viewer that has said hello and has its ring. The screen must have been drawn already.
    fn viewer(&mut self) -> TestViewer {
        let viewer = self.connect();
        viewer.hello(vec![XRGB8888]);
        self.next_ring(viewer)
    }

    fn connect(&mut self) -> TestViewer {
        let socket = FrameSocket::connect(&self.folder.join(SOCKET_NAME)).unwrap();
        socket.set_nonblocking(true).unwrap();
        self.step();
        TestViewer { socket, ring: Ring { generation: 0, slots: 0, size: 0 }, buffers: Vec::new() }
    }

    fn next_ring(&mut self, mut viewer: TestViewer) -> TestViewer {
        for _ in 0..MOST_STEPS {
            self.step();
            if let Some(Packet { message: Message::Ring(ring), fds }) = viewer.socket.receive().unwrap() {
                viewer.ring = ring;
                viewer.buffers = fds.into_iter().map(OwnedFd::into).collect();
                return viewer;
            }
        }
        panic!("no ring came");
    }

    /// The next frame, which may need a few turns of the loop.
    fn next_frame(&mut self, viewer: &TestViewer) -> Frame {
        (0..MOST_STEPS).find_map(|_| self.try_frame(viewer)).expect("no frame came")
    }

    fn try_frame(&mut self, viewer: &TestViewer) -> Option<Frame> {
        self.step();
        match viewer.socket.receive().unwrap()?.message {
            Message::Frame(frame) => Some(frame),
            other => panic!("expected a frame, not {other:?}"),
        }
    }

    fn assert_no_faults(&mut self) {
        assert_eq!(self.engine().faults, Vec::<String>::new());
    }

    fn live_exports(&mut self) -> usize {
        let calls = &self.rm().calls;
        let is_export = |call: &&Call| matches!(call, Call::ExportDmabuf { .. });
        let exported = calls.iter().filter(is_export).filter_map(|call| match call {
            Call::ExportDmabuf { memory, .. } => Some(*memory),
            _ => None,
        });
        let freed = |memory: &u32| {
            calls.iter().any(|call| matches!(call, Call::Free { handle, .. } if handle == memory))
        };
        exported.filter(|memory| !freed(memory)).count()
    }
}

struct TestViewer {
    socket: FrameSocket,
    ring: Ring,
    buffers: Vec<File>,
}

impl TestViewer {
    fn hello(&self, formats: Vec<Format>) {
        let hello = Hello { formats, refresh_millihertz: 60_000, peak_nits: 0 };
        self.socket.send(&Message::Hello(hello), &[]).unwrap();
    }

    fn pixels(&self, frame: &Frame) -> Vec<u8> {
        let size = (frame.descriptor.stride * frame.descriptor.height) as usize;
        let mut pixels = vec![0; size];
        self.buffers[frame.slot as usize].read_exact_at(&mut pixels, frame.descriptor.offset).unwrap();
        pixels
    }

    fn release(&self, frame: &Frame) {
        let release = Release { generation: frame.generation, slot: frame.slot };
        self.socket.send(&Message::Release(release), &[]).unwrap();
    }
}

/// What a viewer should find in a picture the engine's copy numbered `serial` wrote.
fn assert_whole_picture(pixels: &[u8], serial: usize) {
    for (row, bytes) in pixels.chunks(PITCH as usize).enumerate() {
        let (shown, padding) = bytes.split_at(WIDTH as usize * 4);
        let wanted = FakeEngine::stamp(serial, row as u32);
        assert!(shown.iter().all(|byte| *byte == wanted), "row {row} is not all from one copy");
        assert!(padding.iter().all(|byte| *byte == 0), "row {row} ran over its pitch");
    }
}

#[test]
fn hello_is_answered_with_a_ring_for_the_screen_and_its_picture() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    assert_eq!(viewer.ring.slots, SLOTS);
    assert_eq!(viewer.buffers.len(), SLOTS as usize);
    assert!(viewer.ring.size >= u64::from(PITCH * HEIGHT));

    let frame = rig.next_frame(&viewer);
    let descriptor = frame.descriptor;
    assert_eq!((descriptor.width, descriptor.height, descriptor.stride), (WIDTH, HEIGHT, PITCH));
    assert_eq!(descriptor.format, XRGB8888);
    assert!(descriptor.fits(viewer.ring.size));
    let serial = rig.engine().copies.len();
    assert_whole_picture(&viewer.pixels(&frame), serial);
    rig.assert_no_faults();
}

#[test]
fn ring_memory_is_exported_only_when_someone_asks() {
    let mut rig = Rig::new();
    rig.present();
    rig.present();
    assert_eq!(rig.live_exports(), 0);
    rig.viewer();
    assert_eq!(rig.live_exports(), SLOTS as usize);
}

#[test]
fn a_frame_shows_what_the_copy_of_that_frame_wrote() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    let first = rig.next_frame(&viewer);
    for _ in 0..3 {
        rig.present();
        let frame = rig.next_frame(&viewer);
        assert!(frame.sequence > first.sequence);
        assert_ne!(frame.slot, first.slot);
        assert_whole_picture(&viewer.pixels(&frame), rig.engine().copies.len());
        viewer.release(&frame);
    }
    rig.assert_no_faults();
}

#[test]
fn a_slot_the_viewer_holds_is_never_written() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    let held = rig.next_frame(&viewer);
    let picture = viewer.pixels(&held);
    for _ in 0..50 {
        rig.present();
        if let Some(frame) = rig.try_frame(&viewer) {
            viewer.release(&frame);
        }
        assert_eq!(viewer.pixels(&held), picture);
    }
    viewer.release(&held);
    let mut reused = false;
    for _ in 0..SLOTS * 2 {
        rig.present();
        let frame = rig.next_frame(&viewer);
        reused |= frame.slot == held.slot;
        viewer.release(&frame);
    }
    assert!(reused, "once released it comes round again");
}

#[test]
fn a_viewer_that_stops_taking_part_never_holds_the_guest_up() {
    let mut rig = Rig::new();
    rig.present();
    let _viewer = rig.viewer();
    for _ in 0..200 {
        rig.present();
    }
    let session = rig.server.router().session();
    assert_eq!(session.frame_counts().sent, u64::from(SLOTS), "no more than it has slots for");
    assert!(session.frame_counts().skipped > 100);
    rig.assert_no_faults();
}

#[test]
fn a_viewer_that_leaves_costs_the_guest_nothing_and_gives_its_memory_back() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    let copies = rig.engine().copies.len();
    drop(viewer);
    rig.step();
    rig.present();
    assert_eq!(rig.live_exports(), 0);
    assert_eq!(rig.engine().copies.len(), copies + 1, "only the staging copy runs now");
    rig.assert_no_faults();
}

#[test]
fn a_viewer_that_comes_back_gets_a_new_generation_and_the_picture_on_the_screen() {
    let mut rig = Rig::new();
    rig.present();
    let first = rig.viewer();
    let generation = first.ring.generation;
    rig.next_frame(&first);
    drop(first);
    rig.step();
    let second = rig.viewer();
    assert!(second.ring.generation > generation);
    let frame = rig.next_frame(&second);
    assert_whole_picture(&second.pixels(&frame), rig.engine().copies.len());
}

#[test]
fn a_new_viewer_takes_the_place_of_the_old_one() {
    let mut rig = Rig::new();
    rig.present();
    let old = rig.viewer();
    let new = rig.viewer();
    assert!(new.ring.generation > old.ring.generation);
    rig.next_frame(&new);
    assert_eq!(rig.live_exports(), SLOTS as usize);
    assert!(old.socket.receive().map(|_| ()).is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof));
}

#[test]
fn a_new_screen_size_brings_a_new_ring_and_a_release_from_the_old_one_is_harmless() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    let old_frame = rig.next_frame(&viewer);
    let (image, staging) = rig.new_surfaces(200, 100);
    let copy = copy_to_staging(image, staging, 200, 100);
    assert_eq!(rig.call(&copy), SUCCESS);
    let viewer = rig.next_ring(viewer);
    assert!(viewer.ring.generation > old_frame.generation);
    let frame = rig.next_frame(&viewer);
    assert_eq!((frame.generation, frame.descriptor.width), (viewer.ring.generation, 200));
    assert_eq!(frame.descriptor.stride, 1024);
    viewer.release(&old_frame);
    assert_eq!(rig.call(&copy), SUCCESS);
    rig.next_frame(&viewer);
    assert_eq!(rig.live_exports(), SLOTS as usize, "the old ring's memory went back");
    rig.assert_no_faults();
}

#[test]
fn a_viewer_with_no_format_in_common_is_turned_away() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.connect();
    viewer.hello(vec![Format { fourcc: XRGB8888.fourcc, modifier: 7 }]);
    rig.step();
    rig.present();
    assert_eq!(rig.live_exports(), 0);
    assert_eq!(viewer.socket.receive().unwrap_err().kind(), ErrorKind::UnexpectedEof);
}

#[test]
fn a_viewer_talking_nonsense_is_dropped_and_the_guest_never_notices() {
    for nonsense in [
        Message::Release(Release { generation: 1, slot: 0 }),
        Message::Ring(Ring { generation: 1, slots: 1, size: 1 }),
    ] {
        let mut rig = Rig::new();
        rig.present();
        let viewer = rig.connect();
        viewer.socket.send(&nonsense, &[]).unwrap();
        rig.step();
        rig.present();
        assert_eq!(viewer.socket.receive().unwrap_err().kind(), ErrorKind::UnexpectedEof);
    }
}

#[test]
fn a_viewer_sending_as_fast_as_it_can_leaves_the_guest_its_turn() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    let frame = rig.next_frame(&viewer);
    for _ in 0..200 {
        viewer.release(&frame);
    }
    rig.present();
    let reply = rig.call(&message(Command::FlushDevice, HEADER_SIZE));
    assert_eq!(reply, SUCCESS);
}

#[test]
fn in_the_held_order_a_frame_waits_for_the_gpu_and_the_guest_waits_for_both_copies() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    rig.set_order(CopyOrder::Held);
    rig.engine().running = false;
    rig.post(&copy_to_staging(rig.image, rig.staging, WIDTH, HEIGHT));
    for _ in 0..5 {
        assert_eq!(rig.try_reply(), None);
        assert!(viewer.socket.receive().unwrap().is_none(), "the GPU hasn't finished");
    }
    let flush = message(Command::FlushDevice, HEADER_SIZE);
    assert_eq!(rig.call(&flush), SUCCESS, "other messages go on");
    rig.rm().run_engine();
    let frame = rig.next_frame(&viewer);
    assert_whole_picture(&viewer.pixels(&frame), rig.engine().copies.len());
    assert_eq!((0..MOST_STEPS).find_map(|_| rig.try_reply()).unwrap(), SUCCESS);
    rig.assert_no_faults();
}

#[test]
fn a_viewer_leaving_during_a_held_copy_keeps_the_memory_until_the_gpu_is_done() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    rig.set_order(CopyOrder::Held);
    rig.engine().running = false;
    rig.post(&copy_to_staging(rig.image, rig.staging, WIDTH, HEIGHT));
    rig.step();
    rig.rm().run_engine();
    rig.engine().running = false;
    let _ = rig.try_reply();
    drop(viewer);
    rig.step();
    assert_eq!(rig.live_exports(), SLOTS as usize, "a copy into the ring is under way");
    rig.rm().run_engine();
    assert_eq!((0..MOST_STEPS).find_map(|_| rig.try_reply()).unwrap(), SUCCESS);
    rig.step();
    assert_eq!(rig.live_exports(), 0);
    rig.assert_no_faults();
}

#[test]
fn a_dead_copy_channel_costs_a_frame_and_the_guest_still_hears_its_own_copy_went_well() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    let sent = rig.server.router().session().frame_counts().sent;
    rig.set_order(CopyOrder::Held);
    rig.engine().running = false;
    rig.post(&copy_to_staging(rig.image, rig.staging, WIDTH, HEIGHT));
    rig.step();
    rig.rm().run_engine();
    rig.engine().running = false;
    let _ = rig.try_reply();
    let session = rig.server.router_mut().session_mut();
    let _ = session.poll(Instant::now() + PAST_THE_COPY_TIMEOUT);
    let _ = session.poll(Instant::now() + PAST_THE_COPY_TIMEOUT);
    let reply = session.take_replies().next().expect("the guest was answered");
    assert_eq!(reply.bytes, SUCCESS);
    assert_eq!(session.frame_counts().sent, sent);
}

#[test]
fn a_10_bit_picture_is_never_passed_off_as_8_bit() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    let reply = rig.call(&create_coloured_surfaces(WIDTH, HEIGHT, COLOUR_10_BIT));
    let (image, _) = surface_handles(&reply);
    let copies = rig.engine().copies.len();
    assert_eq!(rig.call(&copy_to_staging(image, rig.staging, WIDTH, HEIGHT)), SUCCESS);
    assert_eq!(rig.engine().copies.len(), copies + 1, "the staging copy, and nothing into the ring");
    rig.step();
    assert!(viewer.socket.receive().unwrap().is_none());
    rig.present();
    rig.next_frame(&viewer);
}

#[test]
fn frames_keep_going_when_the_export_is_refused() {
    let mut rig = Rig::new();
    rig.present();
    rig.rm().refuse_exports = true;
    let viewer = rig.connect();
    viewer.hello(vec![XRGB8888]);
    for _ in 0..10 {
        rig.present();
    }
    rig.step();
    assert!(viewer.socket.receive().unwrap().is_none(), "it's still connected, with nothing to see");
}

mod display_driver {
    use super::*;
    use guesthouse_backend_nvidia::fake_guest;
    use guesthouse_messages::indirect::as_the_driver_sends as driver;
    use guesthouse_messages::indirect::{
        CursorMove, CursorPiece, FrameDone, Look, Mastering, Register, RingName, Stamp,
    };
    use guesthouse_messages::wire::{set_u32, u64_at};

    const GENERATION: u32 = 1;
    const NAME: RingName = RingName { generation: GENERATION, monitor: 0 };
    const RING_SLOTS: u32 = 3;
    const ESCAPE_SIZE_AT: usize = HEADER_SIZE + 16;
    const ESCAPE_HEADER: usize = HEADER_SIZE + 24;
    const MAP_ALLOCATION_AT: usize = 0x38;
    const STATUS_OK: u32 = 0;
    const XBGR2101010: Format = Format { fourcc: u32::from_le_bytes(*b"XB30"), modifier: 0 };

    fn escape(data: &[u8]) -> Vec<u8> {
        let mut request = fake_guest::message(Command::DriverEscape, ESCAPE_HEADER);
        set_u32(&mut request, ESCAPE_SIZE_AT, data.len() as u32);
        request.extend_from_slice(data);
        request
    }

    fn ring(look: Look) -> Register {
        Register {
            name: NAME,
            slots: RING_SLOTS,
            width: WIDTH,
            height: HEIGHT,
            pitch: PITCH,
            slot_size: u64::from(PITCH) * u64::from(HEIGHT),
            look,
        }
    }

    fn done(slot: u32) -> FrameDone {
        FrameDone {
            name: NAME,
            slot: Some(slot),
            present_time: 1,
            windows_frame: 1,
            sdr_white_nits: 203,
            mastering: None,
        }
    }

    impl Rig {
        /// Makes the slots as the driver does: buffers it has mapped, each stamped with its slot.
        fn stamped_buffers(&mut self) {
            for slot in 0..RING_SLOTS {
                // Twice as wide, as a slot's rows are longer than a surface's here.
                let (buffer, _) = self.new_surfaces(WIDTH * 2, HEIGHT);
                let mut map = fake_guest::message(Command::MapGpuAddresses, 0x50);
                set_u32(&mut map, MAP_ALLOCATION_AT, buffer);
                let address = u64_at(&self.call(&map), 0).unwrap();
                let memory = self.rm().memory_at(address).expect("it was just mapped");
                let stamp = driver::stamp(Stamp { generation: GENERATION, slot });
                self.rm().memory_starts.insert(memory, stamp);
            }
        }

        fn say(&mut self, data: &[u8]) -> (u32, Option<u32>) {
            driver::reply(&self.call(&escape(data))).expect("a reply")
        }
    }

    fn next_light_and_frame(rig: &mut Rig, viewer: &TestViewer) -> (Option<Light>, Frame) {
        let mut light = None;
        for _ in 0..MOST_STEPS {
            rig.step();
            match viewer.socket.receive().unwrap().map(|packet| packet.message) {
                Some(Message::Light(sent)) => light = Some(sent),
                Some(Message::Frame(frame)) => return (light, frame),
                Some(other) => panic!("expected a frame, not {other:?}"),
                None => {}
            }
        }
        panic!("no frame came");
    }

    #[test]
    fn a_viewer_that_asks_gets_the_cursor_with_its_picture_in_a_sealed_file() {
        let mut rig = Rig::new();
        rig.stamped_buffers();
        let first = rig.say(&driver::register(&ring(Look::SDR))).1.expect("a slot");
        let viewer = rig.connect();
        viewer.hello(vec![XRGB8888]);
        rig.say(&driver::frame(&done(first)));
        let viewer = rig.next_ring(viewer);
        viewer.socket.send(&Message::CursorWanted, &[]).unwrap();
        rig.step();

        let pixels = [5; 64];
        let piece = CursorPiece {
            monitor: 0,
            shape: 1,
            width: 4,
            height: 4,
            pitch: 16,
            hot_x: 0,
            hot_y: 0,
            total: 64,
            offset: 0,
            pixels: &pixels,
        };
        rig.say(&driver::cursor_piece(&piece));
        let position = CursorMove { monitor: 0, x: 3, y: 4, visible: true, sdr_white_nits: 80 };
        rig.say(&driver::cursor_move(&position));

        let (mut shape, mut moved) = (None, None);
        for _ in 0..MOST_STEPS {
            rig.step();
            match viewer.socket.receive().unwrap() {
                Some(Packet { message: Message::CursorShape(sent), fds }) => shape = Some((sent, fds)),
                Some(Packet { message: Message::CursorMove(sent), .. }) => moved = Some(sent),
                _ => {}
            }
            if shape.is_some() && moved.is_some() {
                break;
            }
        }
        let (shape, mut fds) = shape.expect("a shape");
        assert!(shape.fits());
        let mut read = [0; 64];
        File::from(fds.remove(0)).read_exact_at(&mut read, 0).unwrap();
        assert_eq!(read, pixels);
        let moved = moved.expect("a position");
        assert_eq!((moved.x, moved.y), (3, 4));
    }

    #[test]
    fn an_sdr_ring_of_the_guests_buffers_reaches_a_viewer() {
        let mut rig = Rig::new();
        rig.stamped_buffers();
        let (status, first) = rig.say(&driver::register(&ring(Look::SDR)));
        assert_eq!(status, STATUS_OK);
        let first = first.expect("a slot");

        let viewer = rig.connect();
        viewer.hello(vec![XRGB8888]);
        let (status, next) = rig.say(&driver::frame(&done(first)));
        assert_eq!((status, next.is_some()), (STATUS_OK, true));
        let viewer = rig.next_ring(viewer);
        assert_eq!(viewer.buffers.len(), RING_SLOTS as usize);

        let (light, frame) = next_light_and_frame(&mut rig, &viewer);
        assert_eq!(light, None, "SDR has no use for it");
        assert_eq!(frame.slot, first);
        assert_eq!(frame.descriptor.format, XRGB8888);
        assert_eq!(frame.descriptor.stride, PITCH);
        assert_eq!(frame.descriptor.colour, Colour::SRGB);
        assert!(frame.descriptor.fits(viewer.ring.size));
        rig.assert_no_faults();
    }

    #[test]
    fn an_hdr_frame_says_what_it_is_and_brings_its_light() {
        let mut rig = Rig::new();
        rig.stamped_buffers();
        let first = rig.say(&driver::register(&ring(Look::HDR))).1.expect("a slot");

        let viewer = rig.connect();
        viewer.hello(vec![XRGB8888, XBGR2101010]);
        let mastering = Mastering { max_nits: 1000, max_content_nits: 900, ..Mastering::default() };
        rig.say(&driver::frame(&FrameDone { mastering: Some(mastering), ..done(first) }));
        let viewer = rig.next_ring(viewer);

        let (light, frame) = next_light_and_frame(&mut rig, &viewer);
        assert_eq!(frame.descriptor.format, XBGR2101010);
        let colour = frame.descriptor.colour;
        assert_eq!((colour.primaries, colour.transfer, colour.matrix), (9, 16, 0));
        let light = light.expect("an HDR frame's light comes first");
        assert_eq!((light.generation, light.slot), (frame.generation, frame.slot));
        assert_eq!(light.sdr_white_nits, 203);
        assert_eq!(light.mastering.unwrap().max_content_nits, 900);
    }

    #[test]
    fn a_viewer_that_leaves_doesnt_end_the_ring_and_the_next_one_sees_the_last_picture() {
        let mut rig = Rig::new();
        rig.stamped_buffers();
        let first = rig.say(&driver::register(&ring(Look::SDR))).1.expect("a slot");
        let viewer = rig.connect();
        viewer.hello(vec![XRGB8888]);
        rig.say(&driver::frame(&done(first)));
        let viewer = rig.next_ring(viewer);
        let (_, frame) = next_light_and_frame(&mut rig, &viewer);
        drop(viewer);
        rig.step();

        let next = rig.connect();
        next.hello(vec![XRGB8888]);
        let next = rig.next_ring(next);
        assert!(next.ring.generation > frame.generation);
        let (_, again) = next_light_and_frame(&mut rig, &next);
        assert_eq!(again.slot, frame.slot);
    }
}
