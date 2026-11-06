//! The plain values that cross the line.

use guesthouse_messages::Status;
use std::borrow::Cow;
use std::fmt;

/// A block of GPU-reachable memory, named by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryId(pub u64);

/// A GPU address space, named by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpaceId(pub u64);

/// A range of GPU addresses, end excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuRange {
    pub start: u64,
    pub end: u64,
}

impl GpuRange {
    pub fn contains(&self, address: u64) -> bool {
        (self.start..self.end).contains(&address)
    }
}

/// How a PCI device identifies itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciCard {
    pub vendor: u16,
    pub device: u16,
    pub revision: u8,
}

/// What the guest is told about the card. All of it is found at startup, and none comes from a
/// table of cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFacts {
    pub card: PciCard,
    pub name: String,
    /// The card's dedicated memory in bytes.
    pub video_memory: u64,
    /// How many engine nodes the guest sees.
    pub node_count: u32,
    /// The guest driver's version as setup read it, packed the way the guest wants it.
    pub guest_driver_version: u64,
}

/// A failed backend call on the message path. The text is for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError(pub Cow<'static, str>);

impl BackendError {
    pub fn new(reason: impl Into<Cow<'static, str>>) -> Self {
        Self(reason.into())
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

/// Why a backend will not start, in words for the person running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// A mapping to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapRequest {
    pub space: SpaceId,
    pub memory: MemoryId,
    pub offset: u64,
    pub length: u64,
    pub address: u64,
    /// Use 4 KiB pages, for mappings that may later be cut into pieces.
    pub small_pages: bool,
    /// Let the GPU's cache hold host memory mapped here.
    pub gpu_cached: bool,
}

/// A mapping to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmapRequest {
    pub space: SpaceId,
    pub memory: MemoryId,
    pub address: u64,
    pub length: u64,
}

/// The shape of a surface, for the guest and for copies to the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceShape {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pitch: u32,
    /// How the vendor tiles it, in its own terms, or `None` for rows in order.
    pub tiling: Option<u32>,
    pub refresh: (u32, u32),
    pub samples: (u32, u32),
    /// The vendor's word for the colour format, kept for the copy to the screen.
    pub colour: u32,
}

/// What an allocation's private data asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationShape {
    /// The size to make it in video memory.
    pub size: u64,
    /// The size to report if it lives in the guest's own memory, which the guest locks exactly.
    pub exact_size: u64,
    pub surface: Option<SurfaceShape>,
    /// The vendor's driver asked for the GPU's cache to hold it.
    pub gpu_cached: bool,
}

/// An adapter query offered to the vendor first.
#[derive(Debug, Clone, Copy)]
pub struct VendorQuery<'a> {
    pub kind: u32,
    pub size: usize,
    pub data: &'a [u8],
    /// The asking process is 32-bit, and so needs the 32-bit user-mode driver.
    pub process_is_32_bit: bool,
}

/// A registry value. `value_type` is `None` for the driver paths, which have no type to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValue {
    pub value_type: Option<u32>,
    pub bytes: Vec<u8>,
}

/// Which message brought a submission, as their private data differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkSource {
    HardwareQueue,
    Context,
}

/// A submission, as the core understood it.
#[derive(Debug, Clone, Copy)]
pub struct Work<'a> {
    pub source: WorkSource,
    pub queue: u32,
    pub space: SpaceId,
    /// The engine node of the queue's context.
    pub node: u32,
    /// The vendor driver's private data.
    pub private: &'a [u8],
    pub fence: u64,
    pub fence_gpu_address: u64,
    pub fence_value: u64,
    /// False once too many queues have failed: the backend must not let this run.
    pub may_run: bool,
    /// This queue has stopped, so its work can only be stood in for.
    pub queue_failed: bool,
}

/// What became of a submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStart {
    /// Nothing will reach the GPU, so the core must complete the fence itself.
    Finished,
    /// The GPU now owns the fence, once [`crate::Backend::ring_doorbell`] gets `doorbell`. `work`
    /// says how much was added, in the units of [`crate::Backend::unfinished_work_limit`].
    Started { doorbell: u64, work: u32 },
    /// This queue cannot run any more. Nothing reached the GPU, and nothing should wait for it.
    QueueFailed,
    /// No room right now. Nothing changed, and the submission should come again soon.
    Busy,
}

/// Where a GPU address lies in memory shared with the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedSpan {
    pub guest_address: u64,
    /// Bytes of the mapping before this address.
    pub offset_in_mapping: u64,
    /// The whole mapping's length.
    pub mapping_length: u64,
}

/// What became of a copy to a staging surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagingStart {
    Finished(Status),
    /// Under way. There is one at a time, and [`crate::Backend::poll_staging_copy`] says when it ends.
    Running,
}

/// A frame to copy to a staging surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagingCopy {
    pub source: SurfaceShape,
    pub target: SurfaceShape,
    /// The whole surfaces' addresses in the daemon's own address space.
    pub source_address: u64,
    pub target_address: u64,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub target_left: i32,
    pub target_top: i32,
    /// Where the target lives in the shared window, if it is there.
    pub target_guest_address: Option<u64>,
    /// This frame is the guest's screen, and a viewer may want it.
    pub is_screen: bool,
    /// The daemon's own address space, where the two addresses above are.
    pub own_space: SpaceId,
}
