//! No request parser may panic, whatever bytes arrive.
//!
//! This sends random and deliberately short byte strings to every request parser in the crate. It
//! drives only the readers. The answer builders are tested with the handlers.

use guesthouse_messages::adapter::{AdapterQuery, CapsQuery, SetIoSpace, node_of};
use guesthouse_messages::address::{
    MapRequest, ReserveRequest, UpdateOperation, UpdateRequest, freed_address, memory_group,
};
use guesthouse_messages::allocation::{
    CreateAllocation, DestroyAllocation, OpenResource, SetGuestPages, locked_allocation,
};
use guesthouse_messages::queue::{
    CopyToStaging, CreateContext, CreateHardwareQueue, DriverEscape, PresentToken, SubmitToContext,
    SubmitToQueue,
};
use guesthouse_messages::registry::RegistryQuery;
use guesthouse_messages::standard::StandardRecord;
use guesthouse_messages::sync::{
    CreateSyncObject, SignalFence, SignalSharedFence, SignalSyncObject, WaitOnCpu, WaitOnGpu, first_handle,
    opened_sync_object,
};
use guesthouse_messages::wire::{U32List, U64List, clamped, i32_at, u8_at, u32_at, u64_at};
use guesthouse_messages::{MessageHeader, strip_extended_header};

/// A tiny deterministic xorshift, so a failure can be reproduced from the seed printed below.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A message of up to `max` bytes. Interesting lengths near field boundaries are favoured.
    fn message(&mut self, max: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max + 1);
        (0..len).map(|_| self.next() as u8).collect()
    }
}

/// Every parser reachable from a guest message. Each must return, never panic, on any input.
fn run_every_parser(message: &[u8]) {
    // The extended-header strip runs before dispatch, so feed both the raw and stripped bytes on.
    for bytes in [message, strip_extended_header(message)] {
        let _ = MessageHeader::parse(bytes);
        let _ = MessageHeader::process_of(bytes);

        let _ = u8_at(bytes, 0x10);
        let _ = u32_at(bytes, 0x20);
        let _ = i32_at(bytes, 0x1C);
        let _ = u64_at(bytes, 0x28);
        let _ = clamped(bytes, 0x18, 0x40);
        // A count taken straight from the message, the usual shape of a list field.
        let count = u32_at(bytes, 0x20).unwrap_or(0) as usize;
        let _ = U32List::at(bytes, 0x28, count).map(|list| list.iter().count());
        let _ = U64List::at(bytes, 0x28, count).map(|list| list.iter().count());

        let _ = MapRequest::parse(bytes);
        let _ = ReserveRequest::parse(bytes);
        let _ = freed_address(bytes);
        let _ = memory_group(bytes);
        if let Ok(update) = UpdateRequest::parse(bytes) {
            let _ = update.is_complete();
            for operation in update.raw_operations().chunks(UpdateRequest::OPERATION_SIZE) {
                let _ = UpdateOperation::parse(operation);
            }
        }
        let _ = UpdateOperation::parse(bytes);

        if let Ok(create) = CreateAllocation::parse(bytes) {
            let _ = create.guest_backed();
            // Walking the entries reads each private block by a guest-declared size.
            let _ = create.entries().map(|entry| entry.private.len()).sum::<usize>();
        }
        if let Ok(destroy) = DestroyAllocation::parse(bytes) {
            let _ = destroy.with_handles().map(|request| request.handles.iter().count());
        }
        let _ = OpenResource::parse(bytes);
        if let Ok(pages) = SetGuestPages::parse(bytes) {
            let _ = pages.pages().map(|list| list.iter().count());
        }
        let _ = locked_allocation(bytes);

        let _ = CreateContext::parse(bytes);
        let _ = CreateHardwareQueue::parse(bytes);
        let _ = SubmitToQueue::parse(bytes).map(|s| s.private.len());
        let _ = SubmitToContext::parse(bytes).map(|s| s.private.len());
        let _ = CopyToStaging::parse(bytes);
        let _ = PresentToken::parse(bytes);
        let _ = DriverEscape::parse(bytes).map(|e| (e.size, e.data.len()));

        let _ = CreateSyncObject::parse(bytes);
        let _ = SignalFence::parse(bytes);
        let _ = SignalSharedFence::parse(bytes);
        if let Ok(signal) = SignalSyncObject::parse(bytes) {
            let _ = signal.objects.iter().count();
            let _ = signal.contexts.iter().count();
            let _ = signal.values.map(|values| values.iter().count());
        }
        if let Ok(wait) = WaitOnCpu::parse(bytes) {
            let _ = (wait.objects.iter().count(), wait.values.iter().count());
        }
        if let Ok(wait) = WaitOnGpu::parse(bytes) {
            let _ = (wait.objects.iter().count(), wait.values.iter().count());
        }
        let _ = opened_sync_object(bytes);
        let _ = first_handle(bytes);

        let _ = RegistryQuery::parse(bytes).map(|query| query.name);
        let _ = StandardRecord::parse(bytes).map(|record| record.exact_size());

        let _ = AdapterQuery::parse(bytes).map(|query| query.data.len());
        let _ = CapsQuery::parse(bytes);
        let _ = SetIoSpace::parse(bytes);
        let _ = node_of(bytes);
    }
}

#[test]
fn no_parser_panics_on_a_malformed_message() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // Short messages exercise the truncation checks; a spread of lengths covers the field offsets.
    for _ in 0..200_000 {
        let message = rng.message(300);
        run_every_parser(&message);
    }
    // A few all-zero and all-one messages of each length, which random bytes rarely produce.
    for len in 0..300 {
        run_every_parser(&vec![0u8; len]);
        run_every_parser(&vec![0xFFu8; len]);
    }
}
