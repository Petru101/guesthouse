//! The NVIDIA backend.
//!
//! This talks to the host's kernel module through the [`rm::Rm`] layer, runs the guest's commands on
//! real channels, and answers what the guest's drivers ask of their own kernel driver.
//!
//! Three things vary, and each has its own place. The [`profile`] module holds a generation's classes
//! and a driver branch's layouts, and cards themselves are read live. See `ARCHITECTURE.md`.

// Parts wait for what uses them. This goes when the backend is joined up, later in this series.
#![allow(dead_code, unused_imports)]

pub mod answers;
pub mod channel;
pub mod escape;
#[cfg(any(test, feature = "fake-rm"))]
pub mod fake_guest;
pub mod inf;
pub mod profile;
pub mod pushbuffer;
pub mod record;
pub mod rm;
pub mod sensors;
pub mod startup;

use answers::adapter::{AdapterAnswers, Inputs};
use answers::nodes::node_list;
use answers::perf_data;
use answers::registry::AdapterRegistry;
use guesthouse_backend::{
    AdapterFacts, AllocationShape, Backend, BackendError, DriverSurvey, GpuRange, HostServices, MapRequest,
    MemoryId, PciCard, Refusal, RegistryName, RegistryValue, Setup, SlotCopy, SpaceId, StagingCopy,
    StagingStart, Status, UnmapRequest, VendorQuery, Work, WorkStart,
};
use inf::{INF_NAME, Inf};
use profile::{
    Generation, GuestBranch, HostBranch, generation_for, guest_branch, guest_channel_classes, host_branch,
};
use record::{CardReader, HostFacts, build, read_host_facts};
use rm::device::{AddressSpace, Device};
use rm::linux::LinuxRm;
use rm::{Handle, Rm, RmError};
use sensors::SnapshotReader;
use std::collections::HashMap;
use std::fs;
use std::os::fd::OwnedFd;
use std::time::Instant;

pub(crate) const CHANNEL_ADDRESS_ALIGN: u64 = 64 << 10;

const NVIDIA_VENDOR: u16 = 0x10DE;

/// How many entries a queue may have unfinished before its next submission waits. This is our own
/// measured number: a long benchmark finished at this limit, and failed at 6,144 and with no limit.
const UNFINISHED_ENTRIES_LIMIT: u64 = 4000;

/// What the guest's runtime says a Vulkan context is for.
const CLIENT_HINT_VULKAN: u32 = 4;

const RECORD_QUEUE_TOKEN: usize = 0x40;
const RECORD_QUEUE_HEAP: usize = 0x5C;
const QUEUE_HEAP_SIZE: u64 = 1 << 32;

pub(crate) fn memory_error(error: RmError) -> BackendError {
    BackendError::new(error.to_string())
}

fn handle_of(memory: MemoryId) -> Handle {
    memory.0 as Handle
}

/// How an RM comes to be, which tests don't use.
pub trait OpenRm: Rm + Sized {
    fn open_rm(version: &str, sizes: &'static HostBranch) -> Result<Self, RmError>;
}

impl OpenRm for LinuxRm {
    fn open_rm(version: &str, sizes: &'static HostBranch) -> Result<Self, RmError> {
        Self::open(version, sizes)
    }
}

#[cfg(any(test, feature = "fake-rm"))]
impl OpenRm for rm::fake::FakeRm {
    fn open_rm(_version: &str, _sizes: &'static HostBranch) -> Result<Self, RmError> {
        Err(RmError::System("a fake RM is built by hand".to_owned()))
    }
}

/// See the crate documentation.
pub struct NvidiaBackend<R> {
    pub(crate) device: Device<R>,
    pub(crate) generation: &'static Generation,
    pub(crate) host_branch: &'static HostBranch,
    guest: &'static GuestBranch,
    /// The class behind each handle the guest's driver uses.
    channel_classes: Vec<(u32, u32)>,
    facts: AdapterFacts,
    answers: AdapterAnswers,
    pub(crate) spaces: HashMap<SpaceId, AddressSpace>,
    next_space: u64,
    /// The host's live readings, if it has any.
    sensors: Option<Box<dyn SnapshotReader>>,
    card_uuid: [u8; 16],
}

impl<R: Rm> NvidiaBackend<R> {
    /// How long a graphics channel group may hold the GPU, for channels made from now on. 0 leaves
    /// the RM's own choice.
    pub fn set_graphics_timeslice(&mut self, micros: u64) {
        self.device.graphics_timeslice_micros = micros;
    }

    /// Reads the card and works out everything the guest will be told. Nothing here changes the card.
    pub fn start(
        mut device: Device<R>,
        host_branch: &'static HostBranch,
        guest: &'static GuestBranch,
        setup: &Setup,
        host: &HostFacts,
        inf_bytes: &[u8],
    ) -> Result<Self, Refusal> {
        let failed = |error: RmError| Refusal(format!("Reading the card failed: {error}"));
        let mut card = CardReader::new(&mut device);
        let architecture = card.read(record::ARCHITECTURE, &[]).map_err(failed)?;
        let architecture = u32::from_le_bytes(architecture[..4].try_into().expect("four bytes"));
        let generation = generation_for(architecture).ok_or_else(|| {
            Refusal(format!("This build has no profile for GPU architecture {architecture:#x}."))
        })?;
        let engines = card.engines().map_err(failed)?;
        let graphics_copy = card.graphics_copy().map_err(failed)?;
        let nodes = node_list(&engines, graphics_copy);
        let record = build(&guest.record, generation, &mut card, host, &nodes)?;
        let node_count = nodes.len() as u32;

        let name = card.text(record::NAME, 4).map_err(failed)?;
        let pci = card.read(record::PCI_INFO, &[]).map_err(failed)?;
        let word = |at: usize| u32::from_le_bytes(pci[at..at + 4].try_into().expect("four bytes"));
        let found =
            PciCard { vendor: word(0) as u16, device: (word(0) >> 16) as u16, revision: word(8) as u8 };
        if let Some(card) = setup.card.filter(|card| card.device != found.device) {
            return Err(Refusal(format!(
                "Setup chose card {:04x} but the first NVIDIA device is {:04x}. Only the first NVIDIA GPU can \
                 be used for now.",
                card.device, found.device
            )));
        }
        let pci_card = setup.card.unwrap_or(found);

        let inf = Inf::parse(inf_bytes);
        let registry = AdapterRegistry::from_inf(&inf, &pci_card)?;
        let layout = &guest.record;
        let dedicated = record[layout.dedicated..layout.dedicated + 8].try_into().expect("eight bytes");
        let guid_seed: String =
            record[layout.uuid..layout.uuid + 16].iter().map(|byte| format!("{byte:02x}")).collect();
        let clock = |at: usize| {
            let bytes = record[at..at + 4].try_into().expect("four bytes");
            u64::from(u32::from_le_bytes(bytes)) * 1000
        };
        let (graphics_hertz, memory_hertz) = (clock(layout.rated_clocks), clock(layout.rated_clocks + 4));
        let card_uuid = record[layout.uuid..layout.uuid + 16].try_into().expect("sixteen bytes");
        let inputs = Inputs { guid_seed, graphics_hertz, memory_hertz, nodes, registry, record };
        Ok(Self {
            device,
            generation,
            host_branch,
            guest,
            channel_classes: guest_channel_classes(guest, generation),
            facts: AdapterFacts {
                card: pci_card,
                name,
                video_memory: u64::from_le_bytes(dedicated),
                node_count,
                guest_driver_version: setup.guest_driver_version.packed(),
            },
            answers: AdapterAnswers::new(inputs, layout),
            spaces: HashMap::new(),
            next_space: 1,
            sensors: None,
            card_uuid,
        })
    }

    /// Where live readings come from. Nothing is read from the card unless this is called.
    pub fn set_sensors(&mut self, sensors: Box<dyn SnapshotReader>) {
        self.sensors = Some(sensors);
    }

    pub fn answers(&self) -> &AdapterAnswers {
        &self.answers
    }

    pub fn device(&self) -> &Device<R> {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut Device<R> {
        &mut self.device
    }

    fn space(spaces: &HashMap<SpaceId, AddressSpace>, space: SpaceId) -> Result<&AddressSpace, BackendError> {
        spaces.get(&space).ok_or_else(|| BackendError::new("this address space is closed"))
    }
}
