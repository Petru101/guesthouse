//! The session: the tables for one guest, and the dispatch from messages to handlers.

mod adapter;
mod allocation;
mod context;
mod mapping;
mod sync;
#[cfg(test)]
mod tests;
mod waiting;

use crate::config::Config;
use crate::deferred::Deferred;
use crate::fences::FencePool;
use crate::gpu_addresses::{AddressUse, GpuAddresses};
use crate::guest_ranges::GuestRanges;
use crate::handles::HandleCounter;
use crate::pending::{Gate, Pending};
use crate::sync_table::SyncTable;
use crate::tables::{Allocation, Queue, ReservedRange};
use guesthouse_backend::{Backend, BackendError, GuestMemory, HostServices, SpaceId};
use guesthouse_messages::notice::Notice;
use guesthouse_messages::wire::{Malformed, put_u32};
use guesthouse_messages::{Command, MessageHeader, Status};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Why a handler gave up. The guest gets a bare failure status either way.
#[derive(Debug)]
pub(crate) enum Failure {
    Malformed,
    Backend(BackendError),
}

impl From<Malformed> for Failure {
    fn from(_: Malformed) -> Self {
        Self::Malformed
    }
}

impl From<BackendError> for Failure {
    fn from(error: BackendError) -> Self {
        Self::Backend(error)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => Malformed.fmt(f),
            Self::Backend(error) => error.fmt(f),
        }
    }
}

pub(crate) type Handled = Result<(), Failure>;

/// Everything the host knows about one guest. See the crate documentation.
pub struct Session<B, M> {
    pub(crate) backend: B,
    pub(crate) memory: M,
    pub(crate) config: Config,
    pub(crate) started: Instant,

    pub(crate) handles: HandleCounter,
    pub(crate) gpu_addresses: GpuAddresses,
    pub(crate) guest_ranges: GuestRanges,
    pub(crate) fences: FencePool,
    pub(crate) io_space_start: u64,

    pub(crate) processes_32_bit: HashSet<u32>,
    pub(crate) spaces: HashMap<u32, SpaceId>,
    pub(crate) closed_spaces: HashSet<SpaceId>,

    pub(crate) allocations: HashMap<u32, Allocation>,
    pub(crate) resources: HashMap<u32, Vec<u32>>,
    /// Users of a resource, when more than the process that made it.
    pub(crate) resource_users: HashMap<u32, u32>,
    /// Shared handle to the object behind it.
    pub(crate) shared_objects: HashMap<u32, u32>,

    pub(crate) sync_objects: SyncTable,
    pub(crate) paging_queues: HashMap<u32, u64>,
    /// A queue's progress sync object to its fence.
    pub(crate) queue_progress: HashMap<u32, u64>,

    pub(crate) queues: HashMap<u32, Queue>,
    pub(crate) context_queues: HashMap<u32, Vec<u32>>,
    pub(crate) context_nodes: HashMap<u32, u32>,
    pub(crate) gates: HashMap<u32, Gate>,
    pub(crate) queues_made: u32,
    pub(crate) work_started: u64,
    pub(crate) queue_failures: u32,

    /// Where a mapping the host placed sits: address to size and alignment.
    pub(crate) mapped_addresses: HashMap<u64, (u64, u64)>,
    pub(crate) mapped_so_far: u64,
    /// In the order the guest asked, which decides who gets a freed range next.
    pub(crate) reserved_addresses: Vec<ReservedRange>,

    pub(crate) deferred: Vec<Deferred>,
    pub(crate) pending: Vec<Pending>,
    pub(crate) pending_spare: Vec<Pending>,
    pub(crate) notices: Vec<Notice>,
}

impl<B: Backend, M: GuestMemory> Session<B, M> {
    pub fn new(backend: B, memory: M, config: Config) -> Self {
        Self {
            backend,
            memory,
            config,
            started: Instant::now(),
            handles: HandleCounter::new(),
            gpu_addresses: GpuAddresses::new(),
            guest_ranges: GuestRanges::new(),
            fences: FencePool::new(),
            io_space_start: 0,
            processes_32_bit: HashSet::new(),
            spaces: HashMap::new(),
            closed_spaces: HashSet::new(),
            allocations: HashMap::new(),
            resources: HashMap::new(),
            resource_users: HashMap::new(),
            shared_objects: HashMap::new(),
            sync_objects: SyncTable::new(),
            paging_queues: HashMap::new(),
            queue_progress: HashMap::new(),
            queues: HashMap::new(),
            context_queues: HashMap::new(),
            context_nodes: HashMap::new(),
            gates: HashMap::new(),
            queues_made: 0,
            work_started: 0,
            queue_failures: 0,
            mapped_addresses: HashMap::new(),
            mapped_so_far: 0,
            reserved_addresses: Vec::new(),
            deferred: Vec::new(),
            pending: Vec::new(),
            pending_spare: Vec::new(),
            notices: Vec::new(),
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn memory(&self) -> &M {
        &self.memory
    }

    pub fn memory_mut(&mut self) -> &mut M {
        &mut self.memory
    }

    /// Takes GPU addresses as a backend would, for a replay standing in for one.
    pub fn take_gpu_addresses(&mut self, size: u64, align: u64) -> Result<u64, BackendError> {
        self.gpu_addresses.take(size, align, AddressUse::Host)
    }

    /// Answers one message. The reply replaces what is in `out`, which the caller should reuse. A
    /// handler that fails answers with a bare failure status, as the guest must never be left waiting.
    pub fn handle_message(&mut self, message: &[u8], out: &mut Vec<u8>) {
        out.clear();
        let command = MessageHeader::parse(message).map(|header| header.command);
        if let Err(failure) = self.dispatch(command, message, out) {
            log::warn!("{command:?} failed: {failure}");
            out.clear();
            put_u32(out, Status::UNSUCCESSFUL.0);
        }
    }

    /// Messages the host starts by itself, waiting to go out on the global channel.
    pub fn take_notices(&mut self) -> impl Iterator<Item = Notice> + '_ {
        self.notices.drain(..)
    }

    fn dispatch(&mut self, command: Option<Command>, message: &[u8], out: &mut Vec<u8>) -> Handled {
        use Command as C;
        match command {
            Some(C::OpenAdapter) => guesthouse_messages::adapter::open_adapter(message, out)?,
            Some(C::GetAdapterIdentity) => self.adapter_identity(out),
            Some(C::SetIoSpace) => self.set_io_space(message, out)?,
            Some(C::DriverEscape) => self.driver_escape(message, out)?,
            Some(C::QueryAdapter) => self.query_adapter(message, out)?,
            Some(C::QueryDriverCaps) => self.query_driver_caps(message, out)?,
            Some(C::DescribeNode) => self.describe_node(message, out)?,
            Some(C::QueryVideoMemory) => self.query_video_memory(message, out)?,
            Some(C::QueryClockCalibration) => self.query_clock(out)?,
            Some(C::GetRegistryKeys) => answer_no_registry_keys(out),
            Some(C::MarkDeviceFailed) => self.mark_device_failed(message, out)?,

            Some(C::CreateProcess) => self.create_process(message, out)?,
            Some(C::DestroyProcess) => self.destroy_process(message, out)?,
            Some(C::CreateDevice) => {
                let handle = self.handles.take();
                put_u32(out, handle);
            }

            Some(C::CreateAllocation) => self.create_allocation(message, out)?,
            Some(C::DestroyAllocation) => self.destroy_allocation(message, out)?,
            Some(C::DescribeStandardAllocation) => guesthouse_messages::standard::describe(message, out)?,
            Some(C::OpenResource) => self.open_resource(message, out)?,
            Some(C::LockForCpu) => self.lock_for_cpu(message, out)?,
            Some(C::SetGuestPages) => self.set_guest_pages(message, out)?,

            Some(C::MapGpuAddresses) => self.map_gpu_addresses(message, out)?,
            Some(C::ReserveGpuAddresses) => self.reserve_gpu_addresses(message, out)?,
            Some(C::UpdateGpuAddresses) => self.update_gpu_addresses(message, out)?,
            Some(C::MakeResident | C::Evict) => {
                guesthouse_messages::address::two_values_and_status(0, 0, Status::SUCCESS, out)
            }

            Some(C::CreateSyncObject) => self.create_sync_object(message, out)?,
            Some(C::OpenSyncObject) => self.open_sync_object(message, out)?,
            Some(C::DestroySyncObject) => self.destroy_sync_object(message, out)?,
            Some(C::CreateSharedObject) => self.create_shared_object(message, out)?,
            Some(C::DestroySharedObject) => self.destroy_shared_object(message, out)?,
            Some(C::CreatePagingQueue) => self.create_paging_queue(out)?,
            Some(C::DestroyPagingQueue) => self.destroy_paging_queue(message, out)?,
            Some(C::SignalSyncObject) => self.signal_sync_object(message, out)?,
            Some(C::SignalSharedFence) => self.signal_shared_fence(message, out)?,
            Some(C::SignalFence) => self.signal_fence(message, out)?,
            Some(C::WaitOnCpu) => self.wait_on_cpu(message, out)?,
            Some(C::WaitOnGpu) => self.wait_on_gpu(message, out)?,

            Some(C::CreateContext) => self.create_context(message, out)?,
            Some(C::DestroyContext) => self.destroy_context(message, out)?,
            Some(C::CreateHardwareQueue) => self.create_hardware_queue(message, out)?,
            Some(C::DestroyHardwareQueue) => self.destroy_hardware_queue(message, out)?,
            Some(C::SubmitToHardwareQueue) => self.submit_to_hardware_queue(message, out)?,
            Some(C::SubmitToContext) => self.submit_to_context(message, out)?,
            Some(C::SetQueuedLimit) => guesthouse_messages::queue::queued_limit_answer(message, out),
            Some(C::PresentToken) => self.present_token(message, out)?,
            Some(C::CopyToStaging) => self.copy_to_staging(message, out)?,

            Some(C::FreeGpuAddresses) => self.free_gpu_addresses(message, out)?,

            Some(
                C::DestroyDevice
                | C::CloseAdapter
                | C::StopAdapter
                | C::FlushAdapter
                | C::FlushDevice
                | C::FlushHeapTransitions
                | C::UnlockForCpu
                | C::SetContextPriority
                | C::SetFlipFenceValue
                | C::IsSyncObjectSignaled,
            ) => answer_status(Status::SUCCESS, out),

            Some(C::Unknown(_)) | None => answer_status(Status::NOT_SUPPORTED, out),
        }
        Ok(())
    }

    /// The address space of `process`, made the first time it is needed.
    pub(crate) fn space_for(&mut self, process: u32) -> Result<SpaceId, BackendError> {
        if let Some(&space) = self.spaces.get(&process) {
            return Ok(space);
        }
        let space = self.backend.create_address_space(&crate::GPU_RANGES)?;
        let block = self.fence_block()?;
        self.backend.map(&guesthouse_backend::MapRequest {
            space,
            memory: block.memory,
            offset: 0,
            length: crate::FENCE_AREA,
            address: block.gpu_address,
            small_pages: true,
            gpu_cached: false,
        })?;
        self.spaces.insert(process, space);
        Ok(space)
    }

    /// The daemon's own address space, for its copies. Process numbers start above zero.
    pub(crate) fn own_space(&mut self) -> Result<SpaceId, BackendError> {
        self.space_for(0)
    }

    pub(crate) fn fence_value(&self, fence: u64) -> u64 {
        self.memory.read_u64(fence)
    }
}

/// The handler bodies borrow these fields apart, so a backend call can use the rest of the session.
pub(crate) struct Services<'a, M> {
    gpu_addresses: &'a mut GpuAddresses,
    allocations: &'a HashMap<u32, Allocation>,
    memory: &'a M,
}

impl<M: GuestMemory> HostServices for Services<'_, M> {
    fn take_gpu_addresses(&mut self, size: u64, align: u64) -> Result<u64, BackendError> {
        self.gpu_addresses.take(size, align, AddressUse::Host)
    }

    fn read_at_gpu_address(&self, address: u64, out: &mut [u8]) -> bool {
        let Some(guest_address) = self.allocations.values().find_map(|allocation| {
            let base = allocation.guest_address?;
            let mapping = allocation.mappings.iter().find(|mapping| mapping.contains(address))?;
            Some(base + mapping.first + address - mapping.address)
        }) else {
            return false;
        };
        for (index, chunk) in out.chunks_mut(8).enumerate() {
            let word = self.memory.read_u64(guest_address + index as u64 * 8).to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
        true
    }

    fn fence_value(&self, fence: u64) -> u64 {
        self.memory.read_u64(fence)
    }
}

macro_rules! services {
    ($session:expr) => {
        $crate::session::Services::new(&mut $session.gpu_addresses, &$session.allocations, &$session.memory)
    };
}
pub(crate) use services;

impl<'a, M> Services<'a, M> {
    pub(crate) fn new(
        gpu_addresses: &'a mut GpuAddresses,
        allocations: &'a HashMap<u32, Allocation>,
        memory: &'a M,
    ) -> Self {
        Self { gpu_addresses, allocations, memory }
    }
}

pub(crate) fn answer_status(status: Status, out: &mut Vec<u8>) {
    put_u32(out, status.0);
}

/// An entry count, then a status. A bare status here would be read as the count.
fn answer_no_registry_keys(out: &mut Vec<u8>) {
    put_u32(out, 0);
    put_u32(out, Status::SUCCESS.0);
}
