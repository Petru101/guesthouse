//! A pretend GPU, for tests only.
//!
//! It keeps the books a real driver would and refuses what a real driver refuses, so that a mistake
//! in the core shows up as a failed call. Its private data format exists only here:
//!
//! | Bytes | Meaning |
//! |-------|---------|
//! | 0..8  | allocation size, or 4096 if the data is shorter |
//! | 8..12 | surface width, or no surface if zero |
//! | 12..16 | surface height |
//! | 16    | nonzero asks for the GPU's cache |

mod memory;
mod random_messages;

use guesthouse_backend::{
    AdapterFacts, AllocationShape, Backend, BackendError, CardLabel, DriverFound, DriverSurvey, GpuRange,
    HostServices, MapRequest, MemoryId, PciCard, Refusal, RegistryName, RegistryValue, Setup, SlotCopy,
    SpaceId, StagingCopy, StagingStart, Status, SurfaceShape, UnmapRequest, VendorQuery, Work, WorkStart,
};
pub use memory::SparseMemory;
pub use random_messages::RandomMessages;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::os::fd::OwnedFd;
use std::time::Instant;

const STAND_IN_VENDOR: u16 = 0xFFFE;
const DEFAULT_SIZE: u64 = 4096;
/// A query only the stand-in's "vendor" answers, to test that the vendor is asked first.
pub const VENDOR_ONLY_QUERY: u32 = 0;
pub const VENDOR_ANSWER: &[u8] = b"stand-in answer";
pub const REGISTRY_NAME: &str = "standinvalue";
pub const DRIVER_STORE: &str = "C:\\stand-in";
const NODE_COUNT: u32 = 8;
/// A node the stand-in has no channel for, to test that a context there is refused.
pub const NODE_WITHOUT_CHANNEL: u32 = 7;
const COLOUR_8_BIT: u32 = 7;
const FORMAT_32_BIT: u32 = 21;
const EXPORTED_FILE: &str = "/dev/null";
const BYTES_PER_PIXEL: u32 = 4;

/// See the crate documentation.
#[derive(Debug)]
pub struct StandIn {
    facts: AdapterFacts,
    next_id: u64,
    live_memory: HashSet<MemoryId>,
    spaces: HashMap<SpaceId, Space>,
    /// Set to make the next video memory allocation fail.
    pub fail_next_allocation: bool,
    /// Whether submissions go to the pretend GPU. If not, they finish at once.
    pub run_work: bool,
    pub doorbells: Vec<(u32, u64)>,
    pub closed_queues: Vec<u32>,
    pub preserved: Vec<(MemoryId, MemoryId, u64)>,
    pub copies: Vec<StagingCopy>,
    pub slot_copies: Vec<SlotCopy>,
    /// Slot copies end in this. A test sets it to see a copy fail.
    pub slot_copy_status: Status,
    /// Set to make every export fail.
    pub refuse_exports: bool,
    /// Addresses taken through [`HostServices`] by the first submission.
    pub channel_address: Option<u64>,
    /// Copies stay under way until a test sets `copy_done`.
    pub slow_copies: bool,
    pub copy_done: bool,
    pub work_per_submission: u32,
    pub unfinished_limit: Option<u64>,
}

#[derive(Debug, Default)]
struct Space {
    ranges: Vec<GpuRange>,
    /// Address to length.
    mappings: BTreeMap<u64, u64>,
}

impl Space {
    fn overlaps(&self, address: u64, length: u64) -> bool {
        let before = self.mappings.range(..address.saturating_add(length)).next_back();
        before.is_some_and(|(&start, &size)| start.saturating_add(size) > address)
    }
}

impl StandIn {
    pub fn new() -> Self {
        let card = PciCard { vendor: STAND_IN_VENDOR, device: 1, revision: 1 };
        let facts = AdapterFacts {
            card,
            name: "Stand-in GPU".into(),
            video_memory: 8 << 30,
            node_count: NODE_COUNT,
            guest_driver_version: 0,
        };
        Self::with_facts(facts)
    }

    pub fn with_facts(facts: AdapterFacts) -> Self {
        Self {
            facts,
            next_id: 1,
            live_memory: HashSet::new(),
            spaces: HashMap::new(),
            fail_next_allocation: false,
            run_work: false,
            doorbells: Vec::new(),
            closed_queues: Vec::new(),
            preserved: Vec::new(),
            copies: Vec::new(),
            slot_copies: Vec::new(),
            slot_copy_status: Status::SUCCESS,
            refuse_exports: false,
            channel_address: None,
            slow_copies: false,
            copy_done: false,
            work_per_submission: 1,
            unfinished_limit: None,
        }
    }

    pub fn live_memory_blocks(&self) -> usize {
        self.live_memory.len()
    }

    pub fn live_address_spaces(&self) -> usize {
        self.spaces.len()
    }

    /// Mappings in every address space.
    pub fn mapping_count(&self) -> usize {
        self.spaces.values().map(|space| space.mappings.len()).sum()
    }

    fn new_memory(&mut self) -> MemoryId {
        let memory = MemoryId(self.next_id);
        self.next_id += 1;
        self.live_memory.insert(memory);
        memory
    }
}

impl Default for StandIn {
    fn default() -> Self {
        Self::new()
    }
}

fn field(private: &[u8], at: usize) -> u32 {
    private.get(at..at + 4).map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

impl Backend for StandIn {
    fn supports(card: &PciCard) -> bool {
        card.vendor == STAND_IN_VENDOR
    }

    fn survey(setup: &Setup) -> Result<DriverSurvey, Refusal> {
        let found = |version: String, branch: &str| DriverFound { version, branch: branch.into() };
        Ok(DriverSurvey {
            host: found("1.0".into(), &setup.guest_branch),
            guest: found(setup.guest_driver_version.to_string(), &setup.guest_branch),
            host_branch_known: true,
            guest_branch_known: true,
            host_version_tested: true,
            mixed_branches_tested: false,
            card: CardLabel::Tested,
        })
    }

    fn open(setup: &Setup, _survey: &DriverSurvey) -> Result<Self, Refusal> {
        let mut stand_in = Self::new();
        stand_in.facts.guest_driver_version = setup.guest_driver_version.packed();
        Ok(stand_in)
    }

    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    fn allocate_video_memory(&mut self, size: u64) -> Result<(MemoryId, u64), BackendError> {
        if std::mem::take(&mut self.fail_next_allocation) {
            return Err(BackendError::new("the stand-in was told to fail"));
        }
        Ok((self.new_memory(), size))
    }

    fn adopt_host_pages(&mut self, _host_address: u64, _size: u64) -> Result<MemoryId, BackendError> {
        Ok(self.new_memory())
    }

    fn release_memory(&mut self, memory: MemoryId) {
        assert!(self.live_memory.remove(&memory), "{memory:?} was freed twice or never made");
    }

    fn preserve_contents(
        &mut self,
        _host: &mut dyn HostServices,
        _own_space: SpaceId,
        from: MemoryId,
        to: MemoryId,
        size: u64,
    ) -> Result<(), BackendError> {
        self.preserved.push((from, to, size));
        Ok(())
    }

    fn create_address_space(&mut self, ranges: &[GpuRange]) -> Result<SpaceId, BackendError> {
        let space = SpaceId(self.next_id);
        self.next_id += 1;
        self.spaces.insert(space, Space { ranges: ranges.to_vec(), mappings: BTreeMap::new() });
        Ok(space)
    }

    fn destroy_address_space(&mut self, space: SpaceId) {
        assert!(self.spaces.remove(&space).is_some(), "{space:?} was destroyed twice");
    }

    fn map(&mut self, request: &MapRequest) -> Result<(), BackendError> {
        assert!(self.live_memory.contains(&request.memory), "mapping memory that does not exist");
        let Some(space) = self.spaces.get_mut(&request.space) else {
            return Err(BackendError::new("this address space is closed"));
        };
        if !space.ranges.iter().any(|range| range.contains(request.address)) {
            return Err(BackendError::new("no range holds this address"));
        }
        if space.overlaps(request.address, request.length) {
            return Err(BackendError::new("these addresses are taken"));
        }
        space.mappings.insert(request.address, request.length);
        Ok(())
    }

    fn unmap(&mut self, unmap: &UnmapRequest) -> Result<(), BackendError> {
        let Some(space) = self.spaces.get_mut(&unmap.space) else {
            return Err(BackendError::new("this address space is closed"));
        };
        match space.mappings.remove(&unmap.address) {
            Some(length) if length == unmap.length => Ok(()),
            _ => Err(BackendError::new("no mapping like this one")),
        }
    }

    fn describe_allocation(&mut self, private: &[u8]) -> AllocationShape {
        let size = private
            .first_chunk::<8>()
            .map_or(DEFAULT_SIZE, |bytes| u64::from_le_bytes(*bytes))
            .max(DEFAULT_SIZE);
        let (width, height) = (field(private, 8), field(private, 12));
        let surface = (width != 0).then_some(SurfaceShape {
            width,
            height,
            format: FORMAT_32_BIT,
            pitch: width.saturating_mul(BYTES_PER_PIXEL),
            tiling: None,
            refresh: (60, 1),
            samples: (1, 0),
            colour: COLOUR_8_BIT,
        });
        AllocationShape {
            size,
            exact_size: size,
            surface,
            gpu_cached: private.get(16).is_some_and(|&byte| byte != 0),
        }
    }

    fn finish_allocation_data(&mut self, private: &mut [u8], size: u64) {
        if let Some(first) = private.first_mut() {
            *first = size as u8 ^ 0xFF;
        }
    }

    fn finish_queue_data(
        &mut self,
        _queue: u32,
        private: &mut [u8],
        serial: u32,
    ) -> Result<(), BackendError> {
        if let Some(first) = private.first_mut() {
            *first = serial as u8;
        }
        Ok(())
    }

    fn escape(&mut self, data: &mut [u8]) {
        data.iter_mut().for_each(|byte| *byte ^= 0xFF);
    }

    fn adapter_answer(&mut self, query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
        (query.kind == VENDOR_ONLY_QUERY).then(|| {
            out.extend_from_slice(VENDOR_ANSWER);
            Status::SUCCESS
        })
    }

    fn registry_value(&mut self, name: &RegistryName) -> Option<RegistryValue> {
        let utf16 = |text: &str| text.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>();
        match name {
            RegistryName::DriverStorePath | RegistryName::DriverImagePath => {
                Some(RegistryValue { value_type: None, bytes: utf16(DRIVER_STORE) })
            }
            RegistryName::Value(name) if name.eq_ignore_ascii_case(REGISTRY_NAME) => {
                Some(RegistryValue { value_type: Some(1), bytes: utf16("value") })
            }
            RegistryName::Value(_) => None,
        }
    }

    fn gpu_clock(&mut self) -> Result<u64, BackendError> {
        Ok(self.next_id * 1000)
    }

    fn start_work(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> WorkStart {
        if !self.run_work || !work.may_run {
            return WorkStart::Finished;
        }
        if self.channel_address.is_none() {
            self.channel_address = host.take_gpu_addresses(0x10000, 0x10000).ok();
        }
        WorkStart::Started { doorbell: work.fence_value, work: self.work_per_submission }
    }

    fn ring_doorbell(&mut self, queue: u32, doorbell: u64) {
        self.doorbells.push((queue, doorbell));
    }

    fn close_queue(&mut self, queue: u32) {
        self.closed_queues.push(queue);
    }

    fn start_staging_copy(&mut self, _host: &mut dyn HostServices, copy: &StagingCopy) -> StagingStart {
        self.copies.push(*copy);
        if self.slow_copies { StagingStart::Running } else { StagingStart::Finished(Status::SUCCESS) }
    }

    fn poll_staging_copy(&mut self, _host: &mut dyn HostServices, _now: Instant) -> Option<Status> {
        std::mem::take(&mut self.copy_done).then_some(Status::SUCCESS)
    }

    fn start_slot_copy(&mut self, _host: &mut dyn HostServices, copy: &SlotCopy) -> StagingStart {
        self.slot_copies.push(*copy);
        if self.slow_copies { StagingStart::Running } else { StagingStart::Finished(self.slot_copy_status) }
    }

    /// Anything that is a file will do, as nothing here has pixels.
    fn export_memory(&mut self, _memory: MemoryId, _size: u64) -> Result<OwnedFd, BackendError> {
        if self.refuse_exports {
            return Err(BackendError::new("the stand-in was told to refuse"));
        }
        let file = File::open(EXPORTED_FILE).map_err(|error| BackendError::new(error.to_string()))?;
        Ok(file.into())
    }

    fn context_allowed(&mut self, node: u32, _client_hint: u32) -> bool {
        node != NODE_WITHOUT_CHANNEL
    }

    fn unfinished_work_limit(&self) -> Option<u64> {
        self.unfinished_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(space: SpaceId, memory: MemoryId, address: u64, length: u64) -> MapRequest {
        MapRequest { space, memory, offset: 0, length, address, small_pages: false, gpu_cached: false }
    }

    #[test]
    fn mapping_refuses_taken_addresses() {
        let mut gpu = StandIn::new();
        let space = gpu.create_address_space(&[GpuRange { start: 0, end: 0x10000 }]).unwrap();
        let (memory, _) = gpu.allocate_video_memory(0x1000).unwrap();
        gpu.map(&map(space, memory, 0x1000, 0x1000)).unwrap();
        assert!(gpu.map(&map(space, memory, 0x1800, 0x1000)).is_err());
        assert!(gpu.map(&map(space, memory, 0x800, 0x1000)).is_err());
        assert!(gpu.map(&map(space, memory, 0x2000, 0x1000)).is_ok());
    }

    #[test]
    fn mapping_refuses_addresses_outside_the_ranges() {
        let mut gpu = StandIn::new();
        let space = gpu.create_address_space(&[GpuRange { start: 0x1000, end: 0x2000 }]).unwrap();
        let (memory, _) = gpu.allocate_video_memory(0x1000).unwrap();
        assert!(gpu.map(&map(space, memory, 0x2000, 0x10)).is_err());
    }

    #[test]
    fn private_data_describes_size_and_surface() {
        let mut private = vec![0; 17];
        private[..8].copy_from_slice(&0x20000u64.to_le_bytes());
        private[8] = 16;
        private[12] = 8;
        let shape = StandIn::new().describe_allocation(&private);
        assert_eq!(shape.size, 0x20000);
        assert_eq!(shape.surface.map(|surface| surface.pitch), Some(64));
        assert_eq!(StandIn::new().describe_allocation(b"short").size, DEFAULT_SIZE);
    }
}
