//! The frame socket end to end, with this test playing QEMU and the viewer over real sockets, and the
//! stand-in playing the GPU. The buffers here are empty files, so this proves who has which slot and
//! what goes over the socket. It proves nothing about pictures.

use guesthouse_backend::Backend;
use guesthouse_backend_standin::{SparseMemory, StandIn};
use guesthouse_core::{Config, CopyOrder, MemoryLimits, Session};
use guesthouse_daemon::router::{Channel, Router};
use guesthouse_daemon::server::Server;
use guesthouse_frame_socket::{
    Format, Frame, FrameSocket, Hello, Message, Packet, Release, Ring, SOCKET_NAME,
};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at};
use guesthouse_messages::{Command, HEADER_SIZE, Status};
use guesthouse_transport::{FrameFlags, FrameHeader, FrameKind, HEADER_SIZE as FRAME_HEADER_SIZE};
use std::collections::HashSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use std::{env, process};

const IO_SPACE: u64 = 1 << 40;
const WINDOW_END: u64 = IO_SPACE + (1 << 30);
const PROCESS: u32 = 0x4000_2000;
const WIDTH: u32 = 100;
const HEIGHT: u32 = 40;
/// 400 bytes of pixels a row, rounded up to where rows may start.
const PITCH: u32 = 512;
const SURFACE_BYTES: u64 = 1 << 20;
const SLOTS: u32 = 5;
const STEP: Option<Duration> = Some(Duration::from_millis(2));
/// Far more turns of the loop than anything here needs, so a failure ends instead of hanging.
const MOST_STEPS: usize = 500;
const WANTS_COMPLETION: FrameFlags = FrameFlags(1);
const XRGB8888: Format = Format { fourcc: u32::from_le_bytes(*b"XR24"), modifier: 0 };
const SUCCESS: [u8; 4] = Status::SUCCESS.0.to_le_bytes();

static NEXT_FOLDER: AtomicU32 = AtomicU32::new(0);

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size.max(HEADER_SIZE)];
    set_u32(&mut bytes, 8, PROCESS);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

/// Stand-in private data for a surface.
fn surface(width: u32, height: u32) -> Vec<u8> {
    let mut data = SURFACE_BYTES.to_le_bytes().to_vec();
    data.resize(17, 0);
    set_u32(&mut data, 8, width);
    set_u32(&mut data, 12, height);
    data
}

fn create(block: &[u8]) -> Vec<u8> {
    let mut request = message(Command::CreateAllocation, 0x40);
    set_u32(&mut request, 0x28, 1);
    request.extend_from_slice(&[0; 4]);
    request.extend_from_slice(&(block.len() as u32).to_le_bytes());
    request.extend_from_slice(&[0; 4]);
    request.extend_from_slice(block);
    request
}

fn copy(source: u32, target: u32) -> Vec<u8> {
    let mut request = message(Command::CopyToStaging, 0x50);
    set_u32(&mut request, 0x20, source);
    set_u32(&mut request, 0x24, target);
    request
}

/// A daemon with a guest that has a screen, in a run folder of its own.
struct Rig {
    folder: PathBuf,
    server: Server<StandIn, SparseMemory>,
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
        let folder = env::temp_dir().join(format!("guesthouse-viewer-{}-{serial}", process::id()));
        let backend = StandIn::new();
        let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
        let mut config = Config::new(limits);
        config.frame_wait = Duration::ZERO;
        let session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
        let mut server = Server::bind(&folder, Router::new(session)).unwrap();
        let qemu = UnixStream::connect(folder.join(Channel::Vgpu.socket_name())).unwrap();
        server.step(STEP).unwrap();
        qemu.set_nonblocking(true).unwrap();
        let mut rig = Self { folder, server, qemu, image: 0, staging: 0, transaction: 0 };
        let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
        set_u64(&mut io_space, HEADER_SIZE, IO_SPACE);
        set_u64(&mut io_space, HEADER_SIZE + 8, 8 << 30);
        rig.call(&io_space);
        (rig.image, rig.staging) = rig.new_screen(WIDTH, HEIGHT);
        rig
    }

    fn allocate(&mut self, width: u32, height: u32) -> u32 {
        u32_at(&self.call(&create(&surface(width, height))), 0x10).unwrap()
    }

    /// An image and a staging surface, in the shared window as a real screen's is.
    fn new_screen(&mut self, width: u32, height: u32) -> (u32, u32) {
        let (image, staging) = (self.allocate(width, height), self.allocate(width, height));
        let mut lock = message(Command::LockForCpu, 0x20);
        set_u32(&mut lock, 0x1C, staging);
        self.call(&lock);
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

    fn reply(&mut self) -> Vec<u8> {
        (0..MOST_STEPS).find_map(|_| self.try_reply()).expect("the guest got no reply")
    }

    fn call(&mut self, payload: &[u8]) -> Vec<u8> {
        self.post(payload);
        self.reply()
    }

    /// One frame from the guest, which must always get its answer.
    fn present(&mut self) {
        assert_eq!(self.call(&copy(self.image, self.staging)), SUCCESS);
    }

    fn gpu(&mut self) -> &mut StandIn {
        self.server.router_mut().session_mut().backend_mut()
    }

    fn slot_copies(&self) -> usize {
        self.server.router().session().backend().slot_copies.len()
    }

    /// Where the last copy into a slot went.
    fn last_slot_address(&self) -> u64 {
        self.server.router().session().backend().slot_copies.last().unwrap().slot_address
    }

    fn memory_blocks(&self) -> usize {
        self.server.router().session().backend().live_memory_blocks()
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
                viewer.buffers = fds;
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
}

struct TestViewer {
    socket: FrameSocket,
    ring: Ring,
    buffers: Vec<OwnedFd>,
}

impl TestViewer {
    fn hello(&self, formats: Vec<Format>) {
        let hello = Hello { formats, refresh_millihertz: 60_000, peak_nits: 0 };
        self.socket.send(&Message::Hello(hello), &[]).unwrap();
    }

    fn release(&self, frame: &Frame) {
        let release = Release { generation: frame.generation, slot: frame.slot };
        self.socket.send(&Message::Release(release), &[]).unwrap();
    }

    fn is_dropped(&self) -> bool {
        self.socket.receive().is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof)
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
}

#[test]
fn ring_memory_is_made_only_when_someone_asks() {
    let mut rig = Rig::new();
    rig.present();
    rig.present();
    let blocks = rig.memory_blocks();
    assert_eq!(rig.slot_copies(), 0);
    rig.viewer();
    assert_eq!(rig.memory_blocks(), blocks + SLOTS as usize);
}

#[test]
fn each_frame_goes_forward_in_a_slot_of_its_own() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    let first = rig.next_frame(&viewer);
    for _ in 0..3 {
        rig.present();
        let frame = rig.next_frame(&viewer);
        assert!(frame.sequence > first.sequence);
        assert_ne!(frame.slot, first.slot);
        viewer.release(&frame);
    }
}

#[test]
fn a_slot_the_viewer_holds_is_never_written() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    let held = rig.next_frame(&viewer);
    let held_address = rig.last_slot_address();
    for _ in 0..50 {
        rig.present();
        assert_ne!(rig.last_slot_address(), held_address);
        if let Some(frame) = rig.try_frame(&viewer) {
            viewer.release(&frame);
        }
    }
    viewer.release(&held);
    let mut seen = HashSet::new();
    for _ in 0..SLOTS * 2 {
        rig.present();
        viewer.release(&rig.next_frame(&viewer));
        seen.insert(rig.last_slot_address());
    }
    assert!(seen.contains(&held_address), "once released it comes round again");
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
}

#[test]
fn a_viewer_that_leaves_costs_the_guest_nothing_and_gives_its_memory_back() {
    let mut rig = Rig::new();
    rig.present();
    let blocks = rig.memory_blocks();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    let copies = rig.slot_copies();
    drop(viewer);
    rig.step();
    rig.present();
    assert_eq!(rig.memory_blocks(), blocks);
    assert_eq!(rig.slot_copies(), copies, "nothing goes to the ring now");
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
    rig.next_frame(&second);
}

#[test]
fn a_new_viewer_takes_the_place_of_the_old_one() {
    let mut rig = Rig::new();
    rig.present();
    let blocks = rig.memory_blocks();
    let old = rig.viewer();
    let new = rig.viewer();
    assert!(new.ring.generation > old.ring.generation);
    rig.next_frame(&new);
    assert_eq!(rig.memory_blocks(), blocks + SLOTS as usize);
    assert!(old.is_dropped());
}

#[test]
fn a_new_screen_size_brings_a_new_ring_and_a_release_from_the_old_one_is_harmless() {
    let mut rig = Rig::new();
    rig.present();
    let blocks = rig.memory_blocks();
    let viewer = rig.viewer();
    let old_frame = rig.next_frame(&viewer);
    let (image, staging) = rig.new_screen(200, 100);
    let extra_blocks = rig.memory_blocks() - blocks - SLOTS as usize;
    assert_eq!(rig.call(&copy(image, staging)), SUCCESS);
    let viewer = rig.next_ring(viewer);
    assert!(viewer.ring.generation > old_frame.generation);
    let frame = rig.next_frame(&viewer);
    assert_eq!((frame.generation, frame.descriptor.width), (viewer.ring.generation, 200));
    assert_eq!(frame.descriptor.stride, 1024);
    viewer.release(&old_frame);
    assert_eq!(rig.call(&copy(image, staging)), SUCCESS);
    rig.next_frame(&viewer);
    assert_eq!(
        rig.memory_blocks(),
        blocks + SLOTS as usize + extra_blocks,
        "the old ring's memory went back"
    );
}

#[test]
fn a_viewer_with_no_format_in_common_is_turned_away() {
    let mut rig = Rig::new();
    rig.present();
    let blocks = rig.memory_blocks();
    let viewer = rig.connect();
    viewer.hello(vec![Format { fourcc: XRGB8888.fourcc, modifier: 7 }]);
    rig.step();
    rig.present();
    assert_eq!(rig.memory_blocks(), blocks);
    assert!(viewer.is_dropped());
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
        assert!(viewer.is_dropped());
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
    assert_eq!(rig.call(&message(Command::FlushDevice, HEADER_SIZE)), SUCCESS);
}

/// Lets the stand-in end the copy under way.
fn step_the_gpu(rig: &mut Rig) {
    rig.gpu().copy_done = true;
    rig.step();
}

#[test]
fn in_the_held_order_a_frame_waits_for_the_gpu_and_the_guest_waits_for_both_copies() {
    let mut rig = Rig::new();
    rig.present();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    rig.server.router_mut().session_mut().set_copy_order(CopyOrder::Held);
    rig.gpu().slow_copies = true;
    let (image, staging) = (rig.image, rig.staging);
    rig.post(&copy(image, staging));
    assert_eq!(rig.try_reply(), None);
    step_the_gpu(&mut rig);
    assert_eq!(rig.try_reply(), None, "the second copy still reads the guest's image");
    assert!(viewer.socket.receive().unwrap().is_none(), "the GPU hasn't finished");
    assert_eq!(rig.call(&message(Command::FlushDevice, HEADER_SIZE)), SUCCESS, "other messages go on");
    step_the_gpu(&mut rig);
    rig.next_frame(&viewer);
    assert_eq!(rig.reply(), SUCCESS);
}

#[test]
fn a_viewer_leaving_during_a_held_copy_keeps_the_memory_until_the_gpu_is_done() {
    let mut rig = Rig::new();
    rig.present();
    let blocks = rig.memory_blocks();
    let viewer = rig.viewer();
    rig.next_frame(&viewer);
    rig.server.router_mut().session_mut().set_copy_order(CopyOrder::Held);
    rig.gpu().slow_copies = true;
    let (image, staging) = (rig.image, rig.staging);
    rig.post(&copy(image, staging));
    rig.step();
    step_the_gpu(&mut rig);
    drop(viewer);
    rig.step();
    assert_eq!(rig.memory_blocks(), blocks + SLOTS as usize, "a copy into the ring is under way");
    step_the_gpu(&mut rig);
    assert_eq!(rig.reply(), SUCCESS);
    rig.step();
    assert_eq!(rig.memory_blocks(), blocks);
}

#[test]
fn frames_keep_going_when_the_export_is_refused() {
    let mut rig = Rig::new();
    rig.present();
    rig.gpu().refuse_exports = true;
    let viewer = rig.connect();
    viewer.hello(vec![XRGB8888]);
    for _ in 0..10 {
        rig.present();
    }
    rig.step();
    assert!(viewer.socket.receive().unwrap().is_none(), "it's still connected, with nothing to see");
}
