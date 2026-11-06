//! Real channels on the GPU, in the three forms this backend uses.
//!
//! A live channel runs a guest's own ring where it lies in guest memory. A list channel has its ring
//! here, for queues that hand over a list with each submission. A copy channel is the daemon's own.
//! All three share [`Channel`]: a small host area holding the channel's control block and a
//! prelude, the commands that bind the classes the guest's commands assume.

mod area;
pub mod copy;

pub use area::Area;
pub use copy::{COPY_AREA_EXTRA, COPY_WINDOW, CopyChannel};

use crate::profile::{Generation, HostBranch};
use crate::pushbuffer::{increasing, ring_entry};
use crate::rm::classes::{
    CHANNEL_GROUP, CONTEXT_SHARE, CONTROL_GET_WORK_SUBMIT_TOKEN, CONTROL_SCHEDULE_CHANNEL_GROUP,
    CONTROL_SET_TIMESLICE, ENGINE_COPY_0, ENGINE_GRAPHICS,
};
use crate::rm::device::{AddressSpace, Device, Mapping};
use crate::rm::{Handle, Rm, RmError};

/// The host area of a channel on a guest's ring.
pub const AREA_SIZE: usize = 0x20000;
pub(crate) const CONTROL_BLOCK: usize = 0x10000;
const PRELUDE: usize = 0x11000;
const WORD_SIZE: u64 = 4;
pub(crate) const GP_PUT: usize = 0x8C;
const GP_GET: usize = 0x88;

const GROUP_PARAM_ERROR_CONTEXT: usize = 0x00;
const GROUP_PARAM_ADDRESS_SPACE: usize = 0x08;
const GROUP_PARAM_ENGINE: usize = 0x0C;
const SHARE_PARAM_ADDRESS_SPACE: usize = 0x00;
const SHARE_PARAM_FLAGS: usize = 0x04;
/// Graphics only works on the synchronous subcontext.
const SHARE_SYNCHRONOUS: u32 = 0;
const SHARE_ASYNCHRONOUS: u32 = 1;
const CHANNEL_PARAM_ERROR_CONTEXT: usize = 0x00;
const CHANNEL_PARAM_BUFFER: usize = 0x04;
pub(crate) const CHANNEL_PARAM_RING: usize = 0x08;
pub(crate) const CHANNEL_PARAM_RING_ENTRIES: usize = 0x10;
const CHANNEL_PARAM_SHARE: usize = 0x18;
pub(crate) const CHANNEL_PARAM_CONTROL_MEMORY: usize = 0x24;
const CHANNEL_PARAM_CONTROL_OFFSET: usize = 0x48;
pub(crate) const CHANNEL_PARAM_ENGINE: usize = 0x88;

/// The subchannel the host's own methods use, with no object bound, as they are the channel's.
pub(crate) const HOST_SUBCHANNEL: u32 = 6;
/// The four words of a memory operation start here, in the open kernel module's channel headers.
const METHOD_MEM_OP_A: u32 = 0x28;
const MEM_OP_WORDS: u32 = 4;
const MEM_OP_OPERATION_SHIFT: u32 = 27;
const MEM_OP_MEMBAR: u32 = 5;
const MEM_OP_L2_SYSMEM_INVALIDATE: u32 = 0xE;

/// A copy engine is reached through this subchannel, also on its own channel.
pub(crate) const COPY_SUBCHANNEL: u32 = 4;
const SET_OBJECT: u32 = 0;
const SCHEDULE_ENABLED: [u8; 3] = [1, 0, 0];

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// What a channel is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Graphics,
    Copy,
}

/// Where a channel's commands and ring are.
#[derive(Debug, Clone, Copy)]
pub struct ChannelPlace {
    pub ring_address: u64,
    pub ring_entries: u32,
    pub area_address: u64,
    pub area_size: usize,
}

/// See the module documentation.
#[derive(Debug)]
pub struct Channel {
    area: Area,
    area_address: u64,
    memory: Handle,
    group: Handle,
    token: u32,
    ring_entries: u32,
    /// The prelude's words that bind classes, which the invalidate follows.
    bind_words: u32,
    prelude_words: u32,
}

/// Drops every line of host memory from the GPU's L2, with a system membar either side.
///
/// Guest pages are host memory, which the guest's CPU writes without the L2 seeing it, and guests
/// map much of it cached. So a line the L2 still holds can be older than what was just written,
/// which shows as a draw with another draw's vertices. It belongs on the GPU, in order, as anything
/// the CPU had to wait for would come out of every submission.
pub(crate) fn l2_invalidate_words() -> Vec<u32> {
    [MEM_OP_MEMBAR, MEM_OP_L2_SYSMEM_INVALIDATE, MEM_OP_MEMBAR]
        .iter()
        .flat_map(|&operation| {
            let header = increasing(HOST_SUBCHANNEL, METHOD_MEM_OP_A, MEM_OP_WORDS);
            [header, 0, 0, 0, operation << MEM_OP_OPERATION_SHIFT]
        })
        .collect()
}

/// After scheduling and before any work, as this can force a channel off the GPU. The channel runs
/// fine on the RM's own slice, so a refusal only costs some speed.
fn set_timeslice<R: Rm>(device: &mut Device<R>, group: Handle) {
    let micros = device.graphics_timeslice_micros;
    if micros != 0
        && let Err(error) = device.control(group, CONTROL_SET_TIMESLICE, &mut micros.to_le_bytes())
    {
        log::warn!("no timeslice of {micros} µs: {error}");
    }
}

impl Channel {
    pub fn new<R: Rm>(
        device: &mut Device<R>,
        generation: &Generation,
        sizes: &HostBranch,
        space: &AddressSpace,
        place: ChannelPlace,
        engine: Engine,
    ) -> Result<Self, RmError> {
        let area = Area::new(place.area_size).map_err(|error| RmError::System(error.to_string()))?;
        let size = place.area_size as u64;
        let memory = device.alloc_host_pages(area.host_address(), size)?;
        device.map(
            space,
            &Mapping { memory, length: size, address: place.area_address, ..Mapping::default() },
        )?;
        map_doorbell_once(device, generation)?;

        let (engine_number, classes) = match engine {
            Engine::Graphics => (ENGINE_GRAPHICS, generation.graphics_classes),
            Engine::Copy => (ENGINE_COPY_0, &[(generation.copy_class, 8)][..]),
        };
        let mut params = vec![0; sizes.channel_group_params];
        put32(&mut params, GROUP_PARAM_ERROR_CONTEXT, memory);
        put32(&mut params, GROUP_PARAM_ADDRESS_SPACE, space.handle);
        put32(&mut params, GROUP_PARAM_ENGINE, engine_number);
        let group = device.alloc(device.device, CHANNEL_GROUP, &mut params)?;

        let mut params = vec![0; sizes.context_share_params];
        put32(&mut params, SHARE_PARAM_ADDRESS_SPACE, space.handle);
        let share_flags = if engine == Engine::Graphics { SHARE_SYNCHRONOUS } else { SHARE_ASYNCHRONOUS };
        put32(&mut params, SHARE_PARAM_FLAGS, share_flags);
        let share = device.alloc(group, CONTEXT_SHARE, &mut params)?;

        let mut params = vec![0; sizes.channel_params];
        put32(&mut params, CHANNEL_PARAM_ERROR_CONTEXT, memory);
        put32(&mut params, CHANNEL_PARAM_BUFFER, memory);
        put64(&mut params, CHANNEL_PARAM_RING, place.ring_address);
        put32(&mut params, CHANNEL_PARAM_RING_ENTRIES, place.ring_entries);
        put32(&mut params, CHANNEL_PARAM_SHARE, share);
        put32(&mut params, CHANNEL_PARAM_CONTROL_MEMORY, memory);
        put64(&mut params, CHANNEL_PARAM_CONTROL_OFFSET, CONTROL_BLOCK as u64);
        put32(&mut params, CHANNEL_PARAM_ENGINE, engine_number);
        let channel = device.alloc(group, generation.channel_class, &mut params)?;

        for &(class, params_size) in classes {
            device.alloc(channel, class, &mut vec![0; params_size])?;
        }
        let mut token = [0; 4];
        device.control(channel, CONTROL_GET_WORK_SUBMIT_TOKEN, &mut token)?;
        let mut schedule = SCHEDULE_ENABLED;
        device.control(group, CONTROL_SCHEDULE_CHANNEL_GROUP, &mut schedule)?;
        if engine == Engine::Graphics {
            set_timeslice(device, group);
        }

        let first = if engine == Engine::Graphics { 0 } else { COPY_SUBCHANNEL };
        let mut words = Vec::with_capacity(classes.len() * 2);
        for (subchannel, &(class, _)) in (first..).zip(classes) {
            words.extend([increasing(subchannel, SET_OBJECT, 1), class]);
        }
        let bind_words = words.len() as u32;
        // Last in the prelude, so that a channel's first submission has it as well.
        words.extend(l2_invalidate_words());
        area.store_words(PRELUDE, &words);
        Ok(Self {
            area,
            area_address: place.area_address,
            memory,
            group,
            token: u32::from_le_bytes(token),
            ring_entries: place.ring_entries,
            bind_words,
            prelude_words: words.len() as u32,
        })
    }

    pub fn area(&self) -> &Area {
        &self.area
    }

    pub fn area_address(&self) -> u64 {
        self.area_address
    }

    pub fn ring_entries(&self) -> u32 {
        self.ring_entries
    }

    pub fn token(&self) -> u32 {
        self.token
    }

    /// How far through the ring the GPU says it has got.
    pub fn gp_get(&self) -> u32 {
        self.area.load(CONTROL_BLOCK + GP_GET)
    }

    /// The ring entry that runs the prelude, in the same form as the guest's entries.
    pub fn prelude_entry(&self) -> u64 {
        ring_entry(self.area_address + PRELUDE as u64, self.prelude_words)
    }

    /// The ring entry that runs the L2 invalidate, to go ahead of a submission.
    pub fn l2_invalidate_entry(&self) -> u64 {
        let address = self.area_address + PRELUDE as u64 + u64::from(self.bind_words) * WORD_SIZE;
        ring_entry(address, self.prelude_words - self.bind_words)
    }

    /// Tells the GPU the ring is filled up to `position`. This is what starts commands.
    pub fn advance<R: Rm>(&self, device: &mut Device<R>, position: u32) {
        // One store. If the GPU read a half-written field it could take the ring as wrapped, and
        // run on past the real end.
        self.area.store(CONTROL_BLOCK + GP_PUT, position % self.ring_entries);
        device.rm.ring_doorbell(self.token);
    }

    /// Frees the channel. A guest's ring stays, because it belongs to the guest.
    pub fn close<R: Rm>(self, device: &mut Device<R>, space: &AddressSpace) -> Result<(), RmError> {
        device.free(self.group)?;
        device.unmap(space, self.memory, self.area_address, self.area.size() as u64)?;
        device.free(self.memory)
    }
}

fn map_doorbell_once<R: Rm>(device: &mut Device<R>, generation: &Generation) -> Result<(), RmError> {
    if device.doorbell_mapped {
        return Ok(());
    }
    let usermode = device.alloc(device.subdevice, generation.usermode_class, &mut [])?;
    device.rm.map_doorbell(device.subdevice, usermode)?;
    device.doorbell_mapped = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{BLACKWELL, HOST_615};
    use crate::rm::fake::{Call, FakeRm};

    const AREA_ADDRESS: u64 = 0x800_0000;

    fn device() -> (Device<FakeRm>, AddressSpace) {
        let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        (device, space)
    }

    fn place() -> ChannelPlace {
        ChannelPlace {
            ring_address: 0x900_0000,
            ring_entries: 1024,
            area_address: AREA_ADDRESS,
            area_size: AREA_SIZE,
        }
    }

    fn channel(device: &mut Device<FakeRm>, space: &AddressSpace, engine: Engine) -> Channel {
        Channel::new(device, &BLACKWELL, &HOST_615, space, place(), engine).unwrap()
    }

    #[test]
    fn a_graphics_channel_binds_the_classes_the_guest_assumes_in_order() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Graphics);
        let words = channel.area().load_words(PRELUDE, 4);
        assert_eq!(words[0], 0x2001_0000);
        assert_eq!(words[1], BLACKWELL.graphics_classes[0].0);
        assert_eq!(words[2], 0x2001_2000);
        let invalidate = 3 * (MEM_OP_WORDS + 1) as u64;
        assert_eq!(channel.prelude_entry() >> 42, 2 * BLACKWELL.graphics_classes.len() as u64 + invalidate);
        assert_eq!(channel.l2_invalidate_entry() >> 42, invalidate);
    }

    fn timeslices_asked_for(device: &Device<FakeRm>) -> Vec<u64> {
        let asked = device.rm.calls.iter().filter_map(|call| match call {
            Call::Control { command: CONTROL_SET_TIMESLICE, params, .. } => {
                Some(u64::from_le_bytes(params.as_slice().try_into().unwrap()))
            }
            _ => None,
        });
        asked.collect()
    }

    #[test]
    fn the_timeslice_can_be_changed() {
        let (mut device, space) = device();
        device.graphics_timeslice_micros = 4000;
        channel(&mut device, &space, Engine::Graphics);
        assert_eq!(timeslices_asked_for(&device), [4000]);
    }

    #[test]
    fn the_timeslice_is_the_rms_unless_someone_asks() {
        let (mut device, space) = device();
        channel(&mut device, &space, Engine::Graphics);
        assert!(timeslices_asked_for(&device).is_empty());
    }

    #[test]
    fn a_copy_channel_keeps_the_rms_timeslice() {
        let (mut device, space) = device();
        channel(&mut device, &space, Engine::Copy);
        assert!(timeslices_asked_for(&device).is_empty());
    }

    #[test]
    fn a_copy_channel_uses_the_copy_subchannel() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Copy);
        assert_eq!(channel.area().load(PRELUDE), 0x2001_8000);
        assert_eq!(channel.area().load(PRELUDE + 4), BLACKWELL.copy_class);
    }

    #[test]
    fn the_doorbell_page_is_mapped_once() {
        let (mut device, space) = device();
        let first = channel(&mut device, &space, Engine::Copy);
        first.close(&mut device, &space).unwrap();
        channel(&mut device, &space, Engine::Copy);
        let maps = device.rm.calls.iter().filter(|call| matches!(call, crate::rm::fake::Call::MapDoorbell));
        assert_eq!(maps.count(), 1);
    }

    #[test]
    fn advancing_stores_the_position_then_rings() {
        let (mut device, space) = device();
        let channel = channel(&mut device, &space, Engine::Graphics);
        channel.advance(&mut device, 1024 + 5);
        assert_eq!(channel.area().load(CONTROL_BLOCK + GP_PUT), 5);
        assert_eq!(device.rm.doorbells(), [channel.token()]);
    }
}
