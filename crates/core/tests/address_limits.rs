//! Address requests whose numbers don't fit together. Each gets a plain refusal, and the session
//! goes on working after it.

use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_backend_standin::{SparseMemory, StandIn};
use guesthouse_core::{Answer, Config, MemoryLimits, ReplyTicket, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at, u64_at};
use guesthouse_messages::{Command, HEADER_SIZE, Status};

const TICKET: Option<ReplyTicket> = Some(ReplyTicket { channel: 1, transaction: 1 });
const PROCESS: u32 = 0x4000_2000;
const UNSUCCESSFUL: [u8; 4] = Status::UNSUCCESSFUL.0.to_le_bytes();
const ADDRESS: u64 = 0x2000_0000;

const OPERATION: usize = 0x38;
const OPERATION_SIZE: usize = 0x40;
const OPERATION_ADDRESS: usize = 0x08;
const OPERATION_SIZE_FIELD: usize = 0x10;
const OPERATION_THIRD: usize = 0x18;
const OPERATION_OFFSET: usize = 0x20;
const KIND_MAP: u32 = 0;
const KIND_UNMAP: u32 = 1;
const KIND_COPY: u32 = 2;

type TestSession = Session<StandIn, SparseMemory>;

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 8, PROCESS);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

fn send(session: &mut TestSession, message: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    assert_eq!(session.handle_message(message, TICKET, &mut out), Answer::Now);
    out
}

fn session() -> TestSession {
    let backend = StandIn::new();
    let limits = MemoryLimits::new(backend.facts(), None, 1 << 30);
    let mut session = Session::new(backend, SparseMemory::new(1 << 41), Config::new(limits));
    let mut io = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io, HEADER_SIZE, 1 << 40);
    send(&mut session, &io);
    session
}

fn update(kind: u32, address: u64, size: u64, third: u64, offset: u64) -> Vec<u8> {
    let mut request = message(Command::UpdateGpuAddresses, OPERATION + OPERATION_SIZE);
    set_u32(&mut request, 0x2C, 1);
    set_u32(&mut request, OPERATION, kind);
    set_u64(&mut request, OPERATION + OPERATION_ADDRESS, address);
    set_u64(&mut request, OPERATION + OPERATION_SIZE_FIELD, size);
    set_u64(&mut request, OPERATION + OPERATION_THIRD, third);
    set_u64(&mut request, OPERATION + OPERATION_OFFSET, offset);
    request
}

fn allocate(session: &mut TestSession) -> u32 {
    let mut request = message(Command::CreateAllocation, 0x40);
    set_u32(&mut request, 0x28, 1);
    request.extend_from_slice(&[0; 12]);
    u32_at(&send(session, &request), 0x10).unwrap()
}

fn map(session: &mut TestSession, allocation: u32, first_page: u64, pages: u64) -> Vec<u8> {
    let mut request = message(Command::MapGpuAddresses, 0x50);
    set_u32(&mut request, 0x38, allocation);
    set_u64(&mut request, 0x40, first_page);
    set_u64(&mut request, 0x48, pages);
    send(session, &request)
}

/// A page of an allocation mapped at [`ADDRESS`], by way of an update.
fn mapped_page(session: &mut TestSession) -> u32 {
    let allocation = allocate(session);
    let reply = send(session, &update(KIND_MAP, ADDRESS, 0x1000, u64::from(allocation), 0));
    assert_eq!(reply, Status::SUCCESS.0.to_le_bytes());
    allocation
}

#[test]
fn a_reservation_too_large_to_round_up_is_refused() {
    let mut request = message(Command::ReserveGpuAddresses, 0x40);
    set_u64(&mut request, 0x38, u64::MAX);
    assert_eq!(send(&mut session(), &request), UNSUCCESSFUL);
}

#[test]
fn a_reservation_past_the_addresses_there_are_is_refused() {
    let mut request = message(Command::ReserveGpuAddresses, 0x40);
    set_u64(&mut request, 0x38, u64::MAX - 0x10_0000);
    assert_eq!(send(&mut session(), &request), UNSUCCESSFUL);
}

#[test]
fn an_unmap_that_runs_past_the_last_address_is_refused() {
    assert_eq!(send(&mut session(), &update(KIND_UNMAP, u64::MAX, 2, 0, 0)), UNSUCCESSFUL);
}

#[test]
fn a_map_that_runs_past_the_last_address_is_refused() {
    let mut session = session();
    let allocation = allocate(&mut session);
    let request = update(KIND_MAP, u64::MAX, 2, u64::from(allocation), 0);
    assert_eq!(send(&mut session, &request), UNSUCCESSFUL);
}

#[test]
fn a_map_whose_offset_runs_past_the_last_byte_is_refused() {
    let mut session = session();
    let allocation = allocate(&mut session);
    let request = update(KIND_MAP, ADDRESS, 0x1000, u64::from(allocation), u64::MAX);
    assert_eq!(send(&mut session, &request), UNSUCCESSFUL);
}

#[test]
fn a_copy_from_or_to_past_the_last_address_is_refused() {
    let mut session = session();
    assert_eq!(send(&mut session, &update(KIND_COPY, u64::MAX, 2, ADDRESS, 0)), UNSUCCESSFUL);
    assert_eq!(send(&mut session, &update(KIND_COPY, ADDRESS, 2, u64::MAX, 0)), UNSUCCESSFUL);
}

#[test]
fn a_copy_over_its_own_source_is_refused() {
    let mut session = session();
    mapped_page(&mut session);
    let request = update(KIND_COPY, ADDRESS, 1 << 40, ADDRESS + 0x1000, 0);
    assert_eq!(send(&mut session, &request), UNSUCCESSFUL);
}

#[test]
fn a_copy_of_nothing_at_all_ends_at_once() {
    let mut session = session();
    let reply = send(&mut session, &update(KIND_COPY, 1 << 45, 1 << 60, 1 << 62, 0));
    assert_eq!(reply, Status::SUCCESS.0.to_le_bytes());
}

#[test]
fn a_copy_finds_what_lies_past_a_gap() {
    let mut session = session();
    mapped_page(&mut session);
    let before = session.backend().mapping_count();
    let reply = send(&mut session, &update(KIND_COPY, 0x1000, ADDRESS, 0x3000_0000, 0));
    assert_eq!(reply, Status::SUCCESS.0.to_le_bytes());
    assert_eq!(session.backend().mapping_count(), before + 1);
}

#[test]
fn pages_of_a_mapping_that_would_overflow_are_refused() {
    let mut session = session();
    let allocation = allocate(&mut session);
    for (first_page, pages) in
        [(u64::MAX, 1), (1, u64::MAX), (1 << 52, 1), (u64::MAX / 0x1000, u64::MAX / 0x1000)]
    {
        let reply = map(&mut session, allocation, first_page, pages);
        assert_eq!(u32_at(&reply, 16), Ok(Status::INVALID_PARAMETER.0), "{first_page:#x} {pages:#x}");
    }
    assert_eq!(u32_at(&map(&mut session, allocation, 0, 1), 16), Ok(Status::SUCCESS.0));
}

#[test]
fn a_mapping_past_the_end_of_gpu_addresses_never_grows_an_allocation() {
    let mut session = session();
    let allocation = allocate(&mut session);
    let reply = map(&mut session, allocation, 1 << 40, 1);
    assert_eq!(u32_at(&reply, 16), Ok(Status::INVALID_PARAMETER.0));
}

#[test]
fn an_allocation_that_could_not_grow_is_only_freed_once() {
    let mut session = session();
    let mut request = message(Command::CreateAllocation, 0x40);
    set_u32(&mut request, 0x28, 1);
    request.extend_from_slice(&[0; 12]);
    let reply = send(&mut session, &request);
    let (resource, allocation) = (u32_at(&reply, 4).unwrap(), u32_at(&reply, 0x10).unwrap());
    session.backend_mut().fail_next_allocation = true;
    assert_eq!(map(&mut session, allocation, 0, 2), UNSUCCESSFUL);
    let mut destroy = message(Command::DestroyAllocation, 0x28);
    set_u32(&mut destroy, 0x1C, resource);
    send(&mut session, &destroy);
    assert_eq!(session.backend().live_memory_blocks(), 1, "only the fence pages are left");
}

#[test]
fn a_fence_at_its_highest_value_stays_there() {
    let mut session = session();
    let mut create = message(Command::CreateSyncObject, 0x30);
    set_u32(&mut create, 0x20, 5);
    set_u64(&mut create, 0x28, u64::MAX);
    let reply = send(&mut session, &create);
    let (handle, fence) = (u32_at(&reply, 0).unwrap(), u64_at(&reply, 16).unwrap());
    let mut request = update(KIND_UNMAP, ADDRESS, 0x1000, 0, 0);
    set_u32(&mut request, 0x28, handle);
    set_u64(&mut request, 0x18, u64::MAX);
    assert_eq!(send(&mut session, &request), Status::SUCCESS.0.to_le_bytes());
    assert_eq!(session.memory().read_u64(fence), Ok(u64::MAX));
}
