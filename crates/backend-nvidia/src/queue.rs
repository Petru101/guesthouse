//! Getting a queue's submissions onto the GPU.

use crate::channel::{AREA_SIZE, Channel, ChannelPlace, Engine, LIST_AREA, ListChannel, ListFull};
use crate::pushbuffer::{ENTRY_SIZE, Header, RingEntry};
use crate::ring_position::has_ring_position_flag;
use crate::rm::Rm;
use crate::{NvidiaBackend, memory_error};
use guesthouse_backend::{HostServices, SpaceId, Work, WorkSource, WorkStart};

// The fields of a submission's private data.
const LIST_OFFSET: usize = 0x0C;
const ENTRIES: usize = 0x14;
const CONTEXT_LIST_ENTRIES: usize = 0x18;
const LIST_ADDRESS: usize = 0x1C;
const RING_END: usize = 0x78;
const RING_ADDRESS: usize = 0x7C;
const SMALLEST_PRIVATE_DATA: usize = 0x84;

/// How much of a guest's ring is read when standing in for the GPU.
const RING_BYTES: usize = 0x10000;
/// The most entries a list is taken to hold. Past this, `unwrap_list` treats it as having wrapped.
const LIST_RING_ENTRIES: usize = 0x800;

/// The guest writes this method to end a copy, which a channel here is refused. The launch method
/// does the same in the same two words.
const COPY_SEMAPHORE_HEADER: u32 = 0x2001_80C1;
const COPY_LAUNCH_HEADER: u32 = 0x2001_80C0;
const COPY_SEMAPHORE_OPERATION_RELEASE: u32 = 1;
const COPY_LAUNCH_RELEASE_ONE_WORD: u32 = 0xC;

/// The host's own semaphore method, and the words of a release up to its payload.
const HOST_SEMAPHORE_METHOD: u32 = 0x5C;
const HOST_SEMAPHORE_WORDS: u32 = 5;

/// What a queue's commands run on.
#[derive(Debug)]
pub(crate) enum QueueChannel {
    /// Making the channel failed once, and it isn't tried twice.
    Unusable,
    Ring {
        channel: Channel,
        ring_address: u64,
    },
    List(ListChannel),
}

impl QueueChannel {
    pub(crate) fn close<R: Rm>(
        self,
        device: &mut crate::rm::device::Device<R>,
        space: &crate::rm::device::AddressSpace,
    ) -> Result<(), crate::rm::RmError> {
        match self {
            Self::Unusable => Ok(()),
            Self::Ring { channel, .. } => channel.close(device, space),
            Self::List(list) => list.channel.close(device, space),
        }
    }
}

/// A queue, as far as the GPU goes.
#[derive(Debug)]
pub(crate) struct LiveQueue {
    pub(crate) space: SpaceId,
    pub(crate) channel: QueueChannel,
    /// Making a channel is tried once.
    channel_tried: bool,
    /// Where the last submission ended.
    position: Option<u32>,
}

impl LiveQueue {
    fn new(space: SpaceId) -> Self {
        Self { space, channel: QueueChannel::Unusable, channel_tried: false, position: None }
    }
}

fn word(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

fn long(data: &[u8], at: usize) -> u64 {
    data.get(at..at + 8).map_or(0, |bytes| u64::from_le_bytes(bytes.try_into().expect("eight bytes")))
}

fn entry_at(ring: &[u8], index: u32) -> RingEntry {
    let at = index as usize * ENTRY_SIZE % ring.len();
    let bytes = ring.get(at..at + ENTRY_SIZE).and_then(|entry| entry.try_into().ok());
    RingEntry::parse(bytes.unwrap_or([0; ENTRY_SIZE]))
}

/// The commands at a GPU address, if all of them can be read.
fn read_words(host: &dyn HostServices, space: SpaceId, address: u64, count: u32) -> Option<Vec<u32>> {
    let mut bytes = vec![0; count as usize * 4];
    let read = host.read_at_gpu_address(space, address, &mut bytes)?;
    (read == bytes.len()).then(|| {
        let (words, _) = bytes.as_chunks::<4>();
        words.iter().map(|word| u32::from_le_bytes(*word)).collect()
    })
}

/// The largest power of two entries that fit a mapping, if a channel can count that far.
fn ring_entries_of(mapping_length: u64) -> Option<u32> {
    let entries = mapping_length / ENTRY_SIZE as u64;
    u32::try_from(1u64 << (u64::BITS - 1 - entries.max(1).leading_zeros())).ok()
}

/// A list longer than its ring has lost its oldest entries, and what follows is not a list at all.
/// Running that is a fault, so this keeps what is left, in order.
fn unwrap_list(listing: Vec<u8>, entries: u32) -> Vec<u8> {
    let Some(lost) = (entries as usize).checked_sub(LIST_RING_ENTRIES).filter(|&lost| lost > 0) else {
        return listing;
    };
    log::warn!("a list of {entries} entries overran its ring");
    let (lost_bytes, ring_bytes) = (lost * ENTRY_SIZE, LIST_RING_ENTRIES * ENTRY_SIZE);
    let newest = listing.get(..lost_bytes).unwrap_or(&[]);
    let oldest = listing.get(lost_bytes..ring_bytes.min(listing.len())).unwrap_or(&[]);
    [oldest, newest].concat()
}

/// Turns a copy queue's semaphore releases into launches, in the guest's own commands. Says whether
/// it could read every entry's commands: an unread one may hold a release a channel here refuses.
fn patch_copy_semaphores(host: &mut dyn HostServices, space: SpaceId, listing: &[u8]) -> bool {
    let (entries, _) = listing.as_chunks::<ENTRY_SIZE>();
    let mut read_them_all = true;
    for entry in entries {
        let entry = RingEntry::parse(*entry);
        if entry.length == 0 {
            continue;
        }
        let mut bytes = vec![0; entry.length as usize * 4];
        if host.read_guest_span(space, entry.address, &mut bytes) != Some(bytes.len()) {
            read_them_all = false;
            continue;
        }
        let (words, _) = bytes.as_chunks::<4>();
        let words: Vec<u32> = words.iter().map(|word| u32::from_le_bytes(*word)).collect();
        let mut index = 0;
        while index < words.len() {
            let header = words[index];
            if header == COPY_SEMAPHORE_HEADER && index + 1 < words.len() {
                if words[index + 1] == COPY_SEMAPHORE_OPERATION_RELEASE {
                    let at = entry.address + index as u64 * 4;
                    host.write_guest_span(space, at, COPY_LAUNCH_HEADER);
                    host.write_guest_span(space, at + 4, COPY_LAUNCH_RELEASE_ONE_WORD);
                } else {
                    log::warn!("a copy semaphore with operation {:#x}", words[index + 1]);
                }
            }
            index += Header::parse(header).stride();
        }
    }
    read_them_all
}

/// Does what a submission's host semaphores would have done, when it did not really run.
///
/// The last one in each submission writes the ring position of its last entry. If that never moves,
/// the ring looks full to the guest's driver, which ends in the device failing.
fn release_host_semaphores(host: &mut dyn HostServices, space: SpaceId, ring: &[u8], entries: u32, end: u32) {
    for index in end.wrapping_sub(entries)..end {
        let entry = entry_at(ring, index);
        if entry.length == 0 {
            continue;
        }
        let Some(words) = read_words(host, space, entry.address, entry.length) else { continue };
        let mut at = 0;
        while at < words.len() {
            let header = Header::parse(words[at]);
            if header.mode == 1
                && header.method == HOST_SEMAPHORE_METHOD
                && header.count >= HOST_SEMAPHORE_WORDS
                && at + 5 < words.len()
            {
                let address = u64::from(words[at + 1]) | u64::from(words[at + 2]) << 32;
                if let Some(span) = host.shared_span(space, address)
                    && let Err(error) = host.write_u32(span.guest_address, words[at + 3])
                {
                    log::warn!("a semaphore can't be released: {error}");
                }
            }
            at += header.stride();
        }
    }
}

impl<R: Rm> NvidiaBackend<R> {
    /// What runs this work's commands, on a list channel.
    fn engine_for(&self, work: &Work<'_>) -> Engine {
        match self.video_engine(work) {
            Some(video) => Engine::Video(video),
            None if self.answers().node_is_graphics(work.node) => Engine::Graphics,
            None => Engine::Copy,
        }
    }

    /// Whether this work's commands are copies, which a channel here needs changed.
    fn runs_copies(&self, work: &Work<'_>) -> bool {
        self.engine_for(work) == Engine::Copy
    }

    fn take_area(&mut self, host: &mut dyn HostServices, size: usize) -> Option<u64> {
        host.take_gpu_addresses(size as u64, crate::CHANNEL_ADDRESS_ALIGN).ok()
    }

    /// The queue's channel on a guest's ring, made the first time. `first` is where this
    /// submission starts, and the entry before it is free for the prelude.
    fn ring_channel(&mut self, host: &mut dyn HostServices, work: &Work<'_>, ring_address: u64, first: u32) {
        let queue = self.queues.entry(work.queue).or_insert_with(|| LiveQueue::new(work.space));
        if std::mem::replace(&mut queue.channel_tried, true) {
            return;
        }
        let Some(span) =
            host.shared_span(work.space, ring_address).filter(|span| span.offset_in_mapping == 0)
        else {
            log::warn!("ring {ring_address:#x} is not at the start of a shared allocation");
            return;
        };
        if first == 0 {
            log::warn!("no free ring entry for the prelude");
            return;
        }
        let Some(area_address) = self.take_area(host, AREA_SIZE) else { return };
        let Some(ring_entries) = ring_entries_of(span.mapping_length) else {
            log::warn!("ring {ring_address:#x} is too long for a channel");
            return;
        };
        let place = ChannelPlace { ring_address, ring_entries, area_address, area_size: AREA_SIZE };
        let Some(space) = self.spaces.get(&work.space) else { return };
        let engine = self.video_engine(work).map_or(Engine::Graphics, Engine::Video);
        let made = Channel::new(&mut self.device, self.generation, self.host_branch, space, place, engine);
        match made {
            Ok(channel) => {
                let prelude_at = span.guest_address + u64::from(first - 1) * ENTRY_SIZE as u64;
                if let Err(error) = host.write_u64(prelude_at, channel.prelude_entry()) {
                    // A channel with no prelude would fault, so it never gets to run.
                    log::warn!("the prelude has nowhere to go: {error}");
                    if let Err(error) = channel.close(&mut self.device, space) {
                        log::warn!("closing a channel: {}", memory_error(error));
                    }
                } else if let Some(queue) = self.queues.get_mut(&work.queue) {
                    queue.channel = QueueChannel::Ring { channel, ring_address };
                }
            }
            Err(error) => log::warn!("a channel failed: {error}"),
        }
    }

    /// Looks at the ring entries between the last submission and this one. They belong to no
    /// submission, but the GPU fetches them anyway, and after the ring wraps they can still hold
    /// entries from the last lap, which would run old commands again.
    fn clear_ring_gap(
        host: &mut dyn HostServices,
        space: SpaceId,
        ring_address: u64,
        size: u32,
        previous: u32,
        start: u32,
        clear: bool,
    ) {
        let Some(span) = host.shared_span(space, ring_address) else { return };
        let gap = (start.wrapping_sub(previous)) % size;
        for index in 0..gap {
            let at = span.guest_address + u64::from(previous.wrapping_add(index) % size) * ENTRY_SIZE as u64;
            let mut entry = [0; ENTRY_SIZE];
            host.read_guest(at, &mut entry);
            if clear
                && entry != [0; ENTRY_SIZE]
                && let Err(error) = host.write_u64(at, 0)
            {
                log::warn!("a ring entry can't be cleared: {error}");
            }
        }
    }

    /// Puts an entry in the last free place ahead of a submission. The guest leaves empty entries
    /// there, and the GPU runs them on its way to the submission, once the doorbell has rung, so this
    /// comes after everything the guest's CPU wrote for it. False if there was no place for it.
    fn invalidate_in_gap(
        host: &mut dyn HostServices,
        space: SpaceId,
        ring_address: u64,
        size: u32,
        previous: u32,
        start: u32,
        entry: u64,
    ) -> bool {
        if start.wrapping_sub(previous).is_multiple_of(size) {
            return false;
        }
        let Some(span) = host.shared_span(space, ring_address) else { return false };
        let last_free = start.wrapping_sub(1) % size;
        let at = span.guest_address + u64::from(last_free) * ENTRY_SIZE as u64;
        host.write_u64(at, entry).is_ok()
    }

    fn start_on_ring(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
        ring_address: u64,
        entries: u32,
        end: u32,
    ) -> Result<u32, WorkStart> {
        let first = end.saturating_sub(entries);
        self.ring_channel(host, work, ring_address, first);
        let queue = self.queues.get_mut(&work.queue).ok_or(WorkStart::Finished)?;
        let QueueChannel::Ring { channel, ring_address: known } = &queue.channel else {
            return Err(WorkStart::Finished);
        };
        if *known != ring_address {
            // The driver can build a bigger ring, and this channel can't follow it.
            log::warn!("queue {:#x} moved to ring {ring_address:#x}, so it stops", work.queue);
            return Err(WorkStart::QueueFailed);
        }
        let mut invalidated = true;
        if let Some(previous) = queue.position {
            let start = end.wrapping_sub(entries);
            let size = channel.ring_entries();
            Self::clear_ring_gap(host, work.space, ring_address, size, previous, start, work.may_run);
            if work.may_run {
                let entry = channel.l2_invalidate_entry();
                invalidated =
                    Self::invalidate_in_gap(host, work.space, ring_address, size, previous, start, entry);
            }
        }
        queue.position = Some(end);
        if !invalidated {
            self.invalidate_and_wait();
        }
        Ok(end)
    }

    fn list_channel(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> bool {
        if self.queues.contains_key(&work.queue) {
            return matches!(self.queues[&work.queue].channel, QueueChannel::List(_));
        }
        let mut queue = LiveQueue::new(work.space);
        if let (Some(area_address), Some(space)) =
            (self.take_area(host, LIST_AREA), self.spaces.get(&work.space))
        {
            let engine = self.engine_for(work);
            let place = ChannelPlace {
                ring_address: ListChannel::ring_address(area_address),
                ring_entries: ListChannel::RING_ENTRIES,
                area_address,
                area_size: LIST_AREA,
            };
            match Channel::new(&mut self.device, self.generation, self.host_branch, space, place, engine) {
                Ok(channel) => queue.channel = QueueChannel::List(ListChannel::new(channel)),
                Err(error) => log::warn!("a channel failed: {error}"),
            }
        }
        let usable = matches!(queue.channel, QueueChannel::List(_));
        self.queues.insert(work.queue, queue);
        usable
    }

    fn start_on_list(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
        listing: &[u8],
    ) -> Result<u32, WorkStart> {
        if !self.list_channel(host, work) {
            return Err(WorkStart::Finished);
        }
        // A fence that can't be read counts as nothing finished, which only holds work back.
        let finished = host.fence_value(work.fence).unwrap_or(0);
        let queue = self.queues.get_mut(&work.queue).ok_or(WorkStart::Finished)?;
        let QueueChannel::List(list) = &mut queue.channel else { return Err(WorkStart::Finished) };
        list.add(listing, work.fence_gpu_address, work.fence_value, finished).map_err(|full| match full {
            ListFull::ForNow => WorkStart::Busy,
            ListFull::TooLong => {
                log::warn!("queue {:#x} sent a list that can't fit", work.queue);
                WorkStart::QueueFailed
            }
        })
    }

    /// Runs the entries a ring position stands for. Whatever can't be read or trusted stops the queue,
    /// and nothing of it reaches a channel.
    fn start_from_ring_position(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
    ) -> Result<u32, WorkStart> {
        let stop = |why: &str| {
            log::warn!("queue {:#x} stops: {why}", work.queue);
            WorkStart::QueueFailed
        };
        let ring = self.guest_rings.get(&work.queue).ok_or_else(|| stop("it named no ring"))?;
        let (count, end) = (word(work.private, ENTRIES), word(work.private, RING_END));
        let listing = ring.entries_before(host, work.space, count, end).map_err(stop)?;
        if self.runs_copies(work) && !patch_copy_semaphores(host, work.space, &listing) {
            return Err(stop("a copy's commands can't be read, so they can't be made safe"));
        }
        self.start_on_list(host, work, &listing)
    }

    /// The list of a submission, where the driver left it. No channel could ever take a list longer
    /// than its ring, so the entry count is judged before any room is made for them.
    fn listing(host: &dyn HostServices, work: &Work<'_>) -> Result<Vec<u8>, WorkStart> {
        let private = work.private;
        let offset = word(private, LIST_OFFSET) as usize;
        let (entries, address) = match work.source {
            WorkSource::HardwareQueue => (word(private, ENTRIES), long(private, LIST_ADDRESS)),
            WorkSource::Context => (word(private, CONTEXT_LIST_ENTRIES), 0),
        };
        if entries > ListChannel::RING_ENTRIES {
            log::warn!("queue {:#x} sent a list of {entries} entries", work.queue);
            return Err(WorkStart::QueueFailed);
        }
        let size = entries as usize * ENTRY_SIZE;
        let listing = if address == 0 {
            let end = offset.saturating_add(size).min(private.len());
            private.get(offset.min(end)..end).unwrap_or(&[]).to_vec()
        } else {
            let mut listing = vec![0; size];
            let read =
                host.read_at_gpu_address(work.space, address, &mut listing).ok_or(WorkStart::Finished)?;
            listing.truncate(read);
            listing
        };
        Ok(if work.source == WorkSource::HardwareQueue { unwrap_list(listing, entries) } else { listing })
    }

    /// Stands in for the GPU when a hardware queue's work did not run.
    fn stand_in(host: &mut dyn HostServices, space: SpaceId, private: &[u8]) {
        if private.len() < SMALLEST_PRIVATE_DATA {
            return;
        }
        let mut ring = vec![0; RING_BYTES];
        let Some(read) = host.read_at_gpu_address(space, long(private, RING_ADDRESS), &mut ring) else {
            return;
        };
        ring.truncate(read);
        let entries = word(private, ENTRIES);
        if !ring.is_empty() && entries <= ListChannel::RING_ENTRIES {
            release_host_semaphores(host, space, &ring, entries, word(private, RING_END));
        }
    }

    pub(crate) fn start_work_on_gpu(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> WorkStart {
        let private = work.private;
        let outcome = self.prepare(host, work);
        let start = match outcome {
            Ok(doorbell) if work.may_run => {
                return WorkStart::Started { doorbell: u64::from(doorbell), work: word(private, ENTRIES) };
            }
            Ok(_) => WorkStart::Finished,
            Err(start) => start,
        };
        if work.source == WorkSource::HardwareQueue && start != WorkStart::Busy {
            Self::stand_in(host, work.space, private);
        }
        start
    }

    fn prepare(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> Result<u32, WorkStart> {
        let private = work.private;
        if private.len() < SMALLEST_PRIVATE_DATA || work.queue_failed {
            return Err(WorkStart::Finished);
        }
        let ring_address = long(private, RING_ADDRESS);
        if ring_address != 0 {
            return self.start_on_ring(
                host,
                work,
                ring_address,
                word(private, ENTRIES),
                word(private, RING_END),
            );
        }
        if has_ring_position_flag(private) {
            return self.start_from_ring_position(host, work);
        }
        let listing = Self::listing(host, work)?;
        if self.runs_copies(work) {
            patch_copy_semaphores(host, work.space, &listing);
        }
        self.start_on_list(host, work, &listing)
    }

    pub(crate) fn ring(&mut self, handle: u32, position: u64) {
        let Some(queue) = self.queues.get(&handle) else { return };
        let position = position as u32;
        match &queue.channel {
            QueueChannel::Ring { channel, .. } => channel.advance(&mut self.device, position),
            QueueChannel::List(list) => list.channel.advance(&mut self.device, position),
            QueueChannel::Unusable => {}
        }
    }

    /// What a list queue has put on its ring, for tests that play the GPU.
    #[cfg(any(test, feature = "fake-rm"))]
    pub fn ring_of_list_queue(&self, handle: u32) -> Option<Vec<RingEntry>> {
        match &self.queues.get(&handle)?.channel {
            QueueChannel::List(list) => Some(list.entries()),
            _ => None,
        }
    }

    pub(crate) fn close(&mut self, handle: u32) {
        self.guest_rings.remove(&handle);
        let Some(queue) = self.queues.remove(&handle) else { return };
        if let Some(space) = self.spaces.get(&queue.space)
            && let Err(error) = queue.channel.close(&mut self.device, space)
        {
            log::warn!("closing queue {handle:#x}: {}", memory_error(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::MemoryError;

    /// Where a ring entry's upper word keeps its length.
    const ONE_WORD_SHIFT: u32 = 10;

    /// Reports every list as present, full of one-word entries, and remembers what was asked.
    #[derive(Default)]
    struct ListHost {
        asked: std::cell::Cell<usize>,
        reads: std::cell::Cell<usize>,
        /// Whether addresses are in shared pages.
        ring: bool,
    }

    impl HostServices for ListHost {
        fn take_gpu_addresses(&mut self, _: u64, _: u64) -> Result<u64, guesthouse_backend::BackendError> {
            Ok(0)
        }
        fn read_at_gpu_address(&self, _: SpaceId, _: u64, out: &mut [u8]) -> Option<usize> {
            self.asked.set(out.len());
            self.reads.set(self.reads.get() + 1);
            let one_word = 1u64 << (u32::BITS + ONE_WORD_SHIFT);
            out.chunks_mut(ENTRY_SIZE)
                .for_each(|entry| entry.copy_from_slice(&one_word.to_le_bytes()[..entry.len()]));
            Some(out.len())
        }
        fn shared_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::SharedSpan> {
            self.ring.then_some(guesthouse_backend::SharedSpan {
                guest_address: 0,
                offset_in_mapping: 0,
                mapping_length: 0,
            })
        }
        fn guest_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::GuestSpan> {
            None
        }
        fn read_guest_span(&self, space: SpaceId, address: u64, out: &mut [u8]) -> Option<usize> {
            self.read_at_gpu_address(space, address, out)
        }
        fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
            true
        }
        fn write_u32(&mut self, _: u64, _: u32) -> Result<(), MemoryError> {
            Ok(())
        }
        fn write_u64(&mut self, _: u64, _: u64) -> Result<(), MemoryError> {
            Ok(())
        }
        fn read_guest(&self, _: u64, _: &mut [u8]) -> usize {
            0
        }
        fn fence_value(&self, _: u64) -> Result<u64, MemoryError> {
            Ok(0)
        }
    }

    fn private_data(entries: u32) -> Vec<u8> {
        let mut private = vec![0; SMALLEST_PRIVATE_DATA];
        private[ENTRIES..ENTRIES + 4].copy_from_slice(&entries.to_le_bytes());
        private[LIST_ADDRESS..LIST_ADDRESS + 8].copy_from_slice(&0x1000u64.to_le_bytes());
        private
    }

    fn work(private: &[u8]) -> Work<'_> {
        Work {
            source: WorkSource::HardwareQueue,
            queue: 1,
            space: SpaceId(1),
            node: 1,
            client_hint: 0,
            private,
            fence: 0,
            fence_gpu_address: 0,
            fence_value: 1,
            may_run: true,
            queue_failed: false,
        }
    }

    #[test]
    fn a_ring_gap_can_start_at_the_last_position_there_is() {
        let mut host = ListHost { ring: true, ..ListHost::default() };
        NvidiaBackend::<crate::rm::fake::FakeRm>::clear_ring_gap(
            &mut host,
            SpaceId(1),
            0,
            8,
            u32::MAX,
            3,
            true,
        );
    }

    #[test]
    fn a_list_as_long_as_the_ring_is_read() {
        let host = ListHost::default();
        let private = private_data(ListChannel::RING_ENTRIES);
        assert!(NvidiaBackend::<crate::rm::fake::FakeRm>::listing(&host, &work(&private)).is_ok());
        assert_eq!(host.asked.get(), ListChannel::RING_ENTRIES as usize * ENTRY_SIZE);
    }

    #[test]
    fn a_longer_list_stops_the_queue_before_any_room_is_made_for_it() {
        let host = ListHost::default();
        let private = private_data(ListChannel::RING_ENTRIES + 1);
        let listing = NvidiaBackend::<crate::rm::fake::FakeRm>::listing(&host, &work(&private));
        assert_eq!(listing, Err(WorkStart::QueueFailed));
        assert_eq!(host.asked.get(), 0);
    }

    #[test]
    fn a_submission_longer_than_any_ring_is_not_walked_when_standing_in() {
        let mut host = ListHost::default();
        let entries = ListChannel::RING_ENTRIES + 1;
        let mut private = private_data(entries);
        private[RING_END..RING_END + 4].copy_from_slice(&entries.to_le_bytes());
        NvidiaBackend::<crate::rm::fake::FakeRm>::stand_in(&mut host, SpaceId(1), &private);
        assert_eq!(host.reads.get(), 1, "the ring itself, and no entry in it");
    }

    #[test]
    fn a_ring_is_the_largest_power_of_two_that_fits() {
        assert_eq!(ring_entries_of(0x10000), Some(0x2000));
        assert_eq!(ring_entries_of(0x10008), Some(0x2000));
        assert_eq!(ring_entries_of(0xFFF8), Some(0x1000));
    }

    #[test]
    fn a_ring_a_channel_could_not_count_is_not_used() {
        assert_eq!(ring_entries_of(1 << 34), Some(1 << 31));
        assert_eq!(ring_entries_of(1 << 35), None);
    }

    #[test]
    fn a_list_within_the_ring_is_left_alone() {
        assert_eq!(unwrap_list(vec![1, 2, 3], 3), [1, 2, 3]);
    }

    #[test]
    fn an_overrun_list_comes_out_oldest_first() {
        let mut listing = vec![0xAA; ENTRY_SIZE];
        listing.extend(vec![0xBB; (LIST_RING_ENTRIES - 1) * ENTRY_SIZE]);
        listing.extend(vec![0xCC; ENTRY_SIZE]);
        let fixed = unwrap_list(listing, LIST_RING_ENTRIES as u32 + 1);
        assert_eq!(fixed.len(), LIST_RING_ENTRIES * ENTRY_SIZE);
        assert_eq!(fixed[0], 0xBB);
        assert_eq!(fixed[fixed.len() - 1], 0xAA);
    }
}
