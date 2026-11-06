//! The steps of a guest's boot, as whole messages through the session and the backend, with the
//! pretend RM. This proves the parts join up. It proves nothing about a real card or a real guest.

#![allow(unused_imports, dead_code)]

use guesthouse_backend::{Backend, DriverVersion, GuestMemory, Setup};
use guesthouse_backend_nvidia::NvidiaBackend;
use guesthouse_backend_nvidia::fake_guest::{
    PROCESS, blank_request, fake_backend, message, start_on_fake_card,
};
use guesthouse_backend_nvidia::profile::GUEST_617;
use guesthouse_backend_nvidia::pushbuffer::{increasing, ring_entry};
use guesthouse_backend_nvidia::rm::classes::{CHANNEL_GROUP, ENGINE_COPY_0};
use guesthouse_backend_nvidia::rm::fake::{Call, FakeRm};
use guesthouse_backend_nvidia::rm::fake_card::{FAKE_GPU_ID, FAKE_NAME};
use guesthouse_backend_nvidia::sensors::{Snapshot, SnapshotReader};
use guesthouse_backend_standin::{RandomMessages, SparseMemory};
use guesthouse_core::{Answer, Config, CopyOrder, MemoryLimits, ReplyTicket, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at, u64_at};
use guesthouse_messages::{Command, HEADER_SIZE, Status};
use std::env;
use std::time::{Duration, Instant};

const WINDOW: u64 = 1 << 40;
const WINDOW_END: u64 = WINDOW + (1 << 30);
const TICKET: Option<ReplyTicket> = Some(ReplyTicket { channel: 1, transaction: 9 });
const SUCCESS: u32 = Status::SUCCESS.0;
const DECODE_NODE: u32 = 1;
/// The decode and the two encode nodes the fake card has.
const VIDEO_NODES: [u32; 3] = [1, 5, 6];
/// Its security, JPEG and VR nodes.
const NODES_NOT_FOR_VULKAN: [u32; 3] = [4, 7, 8];
const VULKAN_CLIENT: u32 = 4;
const COPY_NODE: u32 = 2;

const DEFAULT_FUZZ_ROUNDS: u64 = 30_000;
const DEFAULT_FUZZ_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const FUZZ_ROUNDS_PER_SESSION: u64 = 2000;
const FUZZ_ROUNDS_TO_TIME_OUT: u64 = 64;
/// Past every deadline a session has.
const FAR_FUTURE: Duration = Duration::from_secs(3600);

type Guest = Session<NvidiaBackend<FakeRm>, SparseMemory>;

fn guest() -> Guest {
    let backend = fake_backend(FakeRm::new());
    let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
    let mut config = Config::new(limits);
    config.frame_wait = Duration::ZERO;
    let mut session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
    let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, WINDOW);
    send(&mut session, &io_space);
    session
}

fn send(session: &mut Guest, message: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    assert_eq!(session.handle_message(message, TICKET, &mut out), Answer::Now);
    out
}

/// A shared allocation of `size` bytes with the driver's own kind of private data. Returns its
/// GPU address and where it is in the window.
fn shared_allocation(session: &mut Guest, size: u32) -> (u64, u64) {
    let layout = &GUEST_617.allocation;
    let mut private = blank_request();
    set_u32(&mut private, layout.size, size);
    let mut create = message(Command::CreateAllocation, 0x40);
    set_u32(&mut create, 0x28, 1);
    create.extend_from_slice(&[0; 4]);
    create.extend_from_slice(&(private.len() as u32).to_le_bytes());
    create.extend_from_slice(&[0; 4]);
    create.extend_from_slice(&private);
    let reply = send(session, &create);
    assert_eq!(u32_at(&reply, 0), Ok(SUCCESS));
    let handle = u32_at(&reply, 0x10).unwrap();

    let mut map = message(Command::MapGpuAddresses, 0x50);
    set_u32(&mut map, 0x38, handle);
    let gpu_address = u64_at(&send(session, &map), 0).unwrap();
    let mut lock = message(Command::LockForCpu, 0x20);
    set_u32(&mut lock, 0x1C, handle);
    (gpu_address, u64_at(&send(session, &lock), 8).unwrap())
}

fn context(session: &mut Guest, node: u32) -> u32 {
    let mut request = message(Command::CreateContext, 0x34);
    set_u32(&mut request, 0x20, node);
    u32_at(&send(session, &request), 0x18).unwrap()
}

/// Returns (queue, its fence).
fn queue(session: &mut Guest, context: u32) -> (u32, u64) {
    let mut request = message(Command::CreateHardwareQueue, 0x44 + 0x80);
    set_u32(&mut request, 0x38, context);
    set_u32(&mut request, 0x40, 0x80);
    let reply = send(session, &request);
    assert_ne!(u32_at(&reply, 0x44 + 0x40), Ok(0), "the guest's driver refuses a token of zero");
    (u32_at(&reply, 0x1C).unwrap(), u64_at(&reply, 0x28).unwrap())
}

fn submission(queue: u32, value: u64, private: &[u8]) -> Vec<u8> {
    let mut request = message(Command::SubmitToHardwareQueue, 0x50);
    set_u32(&mut request, 0x18, queue);
    set_u64(&mut request, 0x20, value);
    request.extend_from_slice(private);
    request
}

fn ring_private(entries: u32, end: u32, ring: u64) -> Vec<u8> {
    let mut private = vec![0; 0x84];
    set_u32(&mut private, 0x14, entries);
    set_u32(&mut private, 0x78, end);
    set_u64(&mut private, 0x7C, ring);
    private
}

#[test]
fn the_guest_is_told_what_the_live_card_and_the_setup_say() {
    let mut session = guest();
    let reply = send(&mut session, &message(Command::GetAdapterIdentity, HEADER_SIZE));
    let name: String = String::from_utf16_lossy(
        &reply[24..24 + 2 * FAKE_NAME.len()]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|u| u16::from_le_bytes(*u))
            .collect::<Vec<_>>(),
    );
    assert_eq!(name, FAKE_NAME);
    let mut query = message(Command::QueryAdapter, HEADER_SIZE + 8);
    set_u32(&mut query, HEADER_SIZE, 49);
    set_u32(&mut query, HEADER_SIZE + 4, 8);
    let reply = send(&mut session, &query);
    assert_eq!(u64_at(&reply, 4), Ok(32 << 48 | 16 << 16 | 1742));
}

#[test]
fn the_record_reaches_the_guest_with_the_discovery_word_set() {
    let mut session = guest();
    let size = GUEST_617.record.size;
    let mut query = message(Command::QueryAdapter, HEADER_SIZE + 8);
    set_u32(&mut query, HEADER_SIZE + 4, size as u32);
    let reply = send(&mut session, &query);
    assert_eq!(reply.len(), 4 + size);
    assert_eq!(&reply[4..8], b"ADVN");
    assert_eq!(u32_at(&reply, 4 + GUEST_617.record.gpu_discovery), Ok(1));
}

#[test]
fn the_record_carries_the_live_cards_id() {
    let mut session = guest();
    let size = GUEST_617.record.size;
    let mut query = message(Command::QueryAdapter, HEADER_SIZE + 8);
    set_u32(&mut query, HEADER_SIZE + 4, size as u32);
    let reply = send(&mut session, &query);
    assert_eq!(u32_at(&reply, 4 + GUEST_617.record.gpu_id), Ok(FAKE_GPU_ID));
}

#[test]
fn a_ring_queue_runs_on_the_guests_own_ring() {
    let mut session = guest();
    let (ring, ring_memory) = shared_allocation(&mut session, 0x10000);
    let context = context(&mut session, 0);
    let (queue, _) = queue(&mut session, context);
    let reply = send(&mut session, &submission(queue, 1, &ring_private(1, 2, ring)));
    assert_eq!(reply, SUCCESS.to_le_bytes());

    let prelude = session.memory().read_u64(ring_memory).unwrap();
    assert_ne!(prelude, 0, "the entry before the first holds the prelude");
    assert_eq!(session.backend().device().rm.doorbells().len(), 1);
    let groups =
        session.backend().device().rm.allocated().filter(|(class, _)| *class == CHANNEL_GROUP).count();
    assert_eq!(groups, 1);
    send(&mut session, &submission(queue, 2, &ring_private(1, 3, ring)));
    assert_eq!(session.backend().device().rm.doorbells().len(), 2);
    let groups =
        session.backend().device().rm.allocated().filter(|(class, _)| *class == CHANNEL_GROUP).count();
    assert_eq!(groups, 1, "the channel is made once");

    send(&mut session, &submission(queue, 3, &ring_private(1, 6, ring)));
    assert_eq!(session.memory().read_u64(ring_memory + 3 * 8), Ok(0), "the gap's other entries stay empty");
    assert_ne!(session.memory().read_u64(ring_memory + 4 * 8), Ok(0), "the L2 invalidate goes just ahead");
}

#[test]
fn a_ring_with_no_room_for_the_prelude_falls_back_to_finishing_the_fence() {
    let mut session = guest();
    let (ring, _) = shared_allocation(&mut session, 0x10000);
    let context = context(&mut session, 0);
    let (queue, fence) = queue(&mut session, context);
    send(&mut session, &submission(queue, 5, &ring_private(1, 1, ring)));
    assert!(session.backend().device().rm.doorbells().is_empty());
    assert_eq!(session.memory().read_u64(fence).unwrap(), 5);
}

#[test]
fn work_that_did_not_run_still_writes_its_host_semaphores() {
    let mut session = guest();
    let (ring, ring_memory) = shared_allocation(&mut session, 0x10000);
    let (commands, command_memory) = shared_allocation(&mut session, 0x1000);
    let (target, target_memory) = shared_allocation(&mut session, 0x1000);
    let context = context(&mut session, 0);
    let (queue, _) = queue(&mut session, context);
    let words = [increasing(0, 0x5C, 5), target as u32, (target >> 32) as u32, 0x77, 0, 1];
    for (index, word) in words.iter().enumerate() {
        session.memory_mut().write_u32(command_memory + index as u64 * 4, *word).unwrap();
    }
    session.memory_mut().write_u64(ring_memory, ring_entry(commands, words.len() as u32)).unwrap();
    send(&mut session, &submission(queue, 1, &ring_private(1, 1, ring)));
    assert_eq!(session.memory().read_u64(target_memory).unwrap(), 0x77);
}

#[test]
fn a_list_queue_on_a_copy_node_uses_a_copy_channel_and_fixes_its_semaphores() {
    let mut session = guest();
    let (commands, command_memory) = shared_allocation(&mut session, 0x1000);
    session.memory_mut().write_u32(command_memory, 0x2001_80C1).unwrap();
    session.memory_mut().write_u32(command_memory + 4, 1).unwrap();
    let context = context(&mut session, COPY_NODE);
    let (queue, _) = queue(&mut session, context);
    let mut private = vec![0; 0x84];
    set_u32(&mut private, 0x14, 1);
    set_u32(&mut private, 0x0C, 0x84);
    private.extend_from_slice(&ring_entry(commands, 2).to_le_bytes());
    send(&mut session, &submission(queue, 1, &private));

    let copy_group = session.backend().device().rm.allocated().find(|(class, _)| *class == CHANNEL_GROUP);
    assert_eq!(u32_at(copy_group.unwrap().1, 0x0C), Ok(ENGINE_COPY_0));
    assert_eq!(session.memory().read_u64(command_memory).unwrap() as u32, 0x2001_80C0);
    assert_eq!(session.memory().read_u64(command_memory).unwrap() >> 32, 0xC);
    assert_eq!(session.backend().device().rm.doorbells().len(), 1);
}
