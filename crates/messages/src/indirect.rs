//! What our own indirect display driver in the guest says to the daemon.
//!
//! These ride in a driver escape's private data, and the reply goes back in the same bytes. Nothing
//! here came from a vendor: this protocol is ours, and its C side is in the guest driver's source.
//! Every number comes from the guest, so a message only parses if it describes a ring that makes
//! sense. A message that doesn't is simply not answered, which the driver takes as no host.

use crate::wire::{Malformed, set_u32, set_u64, u8_at, u32_at, u64_at};

const REQUEST_MAGIC: u64 = u64::from_le_bytes(*b"GHOUSEDP");
const REPLY_MAGIC: u64 = u64::from_le_bytes(*b"GHOUSERP");
const STAMP_MAGIC: u64 = u64::from_le_bytes(*b"GHOUSEST");
const VERSION: u16 = 1;

const KIND_REGISTER: u16 = 1;
const KIND_FRAME: u16 = 2;
const KIND_CLOSE: u16 = 3;

const HEADER_SIZE: usize = 24;
const REGISTER_SIZE: usize = 64;
const FRAME_SIZE: usize = 80;
const REPLY_SIZE: usize = 40;
/// The first bytes of a slot, until the first frame writes over them.
pub const STAMP_SIZE: usize = 16;

// In the header.
const VERSION_AT: usize = 8;
const KIND_AT: usize = 10;
const SIZE_AT: usize = 12;
const GENERATION_AT: usize = 16;
const MONITOR_AT: usize = 20;
// In a REGISTER.
const SLOTS_AT: usize = 24;
const WIDTH_AT: usize = 28;
const HEIGHT_AT: usize = 32;
const PITCH_AT: usize = 36;
const SLOT_SIZE_AT: usize = 40;
const FOURCC_AT: usize = 48;
const CICP_AT: usize = 56;
// In a FRAME.
const SLOT_AT: usize = 24;
const FLAGS_AT: usize = 28;
const SEQUENCE_AT: usize = 32;
const PRESENT_AT: usize = 40;
const WINDOWS_FRAME_AT: usize = 48;
const SDR_WHITE_AT: usize = 52;
const HDR_AT: usize = 56;
// In a reply.
const STATUS_AT: usize = 24;
const NEXT_SLOT_AT: usize = 28;
const REFRESH_AT: usize = 32;
const PEAK_AT: usize = 36;
// In a stamp.
const STAMP_GENERATION_AT: usize = 8;
const STAMP_SLOT_AT: usize = 12;

const NO_SLOT: u32 = u32::MAX;
const HAS_HDR: u32 = 1;
/// More would be a mistake, and each is a descriptor sent to the viewer.
pub const MOST_SLOTS: u32 = 8;

const CICP_BT709: u8 = 1;
const CICP_BT2020: u8 = 9;
const CICP_SRGB: u8 = 13;
const CICP_PQ: u8 = 16;
const CICP_IDENTITY: u8 = 0;

/// How a ring's pixels are to be read: a DRM format and H.273's code points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub fourcc: u32,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

impl Look {
    /// 8-bit BT.709, blue first in memory, as an ordinary desktop is.
    pub const SDR: Self = Self {
        fourcc: u32::from_le_bytes(*b"XR24"),
        primaries: CICP_BT709,
        transfer: CICP_SRGB,
        matrix: CICP_IDENTITY,
        full_range: true,
    };
    /// 10-bit BT.2020 with the PQ transfer, red in the low bits.
    pub const HDR: Self = Self {
        fourcc: u32::from_le_bytes(*b"XB30"),
        primaries: CICP_BT2020,
        transfer: CICP_PQ,
        matrix: CICP_IDENTITY,
        full_range: true,
    };
}

/// Mastering data with a frame. The numbers go to the viewer as they came, and nothing here is
/// worked out from them, so no value can do harm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mastering {
    /// Red, green, blue and white, x then y, in units of 0.00002.
    pub chromaticity: [u16; 8],
    pub max_nits: u16,
    /// In units of 0.0001 nits.
    pub min: u16,
    pub max_content_nits: u16,
    pub max_frame_average_nits: u16,
}

/// Says which ring and which screen a message is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingName {
    pub generation: u32,
    pub monitor: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Register {
    pub name: RingName,
    pub slots: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub slot_size: u64,
    pub look: Look,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDone {
    pub name: RingName,
    /// `None` means nothing was written, and the driver wants somewhere to write.
    pub slot: Option<u32>,
    pub present_time: u64,
    pub windows_frame: u32,
    pub sdr_white_nits: u32,
    pub mastering: Option<Mastering>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Register(Register),
    Frame(FrameDone),
    Close(RingName),
}

/// What became of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// That ring isn't the one the host has. The driver makes a new one.
    StaleGeneration,
    /// The host can't use this ring. The driver waits for a new mode.
    Refused,
}

impl Outcome {
    fn wire(self) -> u32 {
        match self {
            Self::Accepted => 0,
            Self::StaleGeneration => 1,
            Self::Refused => 2,
        }
    }
}

/// An answer. The monitor's facts are 0 when not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply {
    pub outcome: Outcome,
    pub next_slot: Option<u32>,
    pub refresh_millihertz: u32,
    pub peak_nits: u32,
}

/// A slot's stamp, as read from its memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub generation: u32,
    pub slot: u32,
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, Malformed> {
    Ok(u16::from(u8_at(bytes, at)?) | u16::from(u8_at(bytes, at + 1)?) << 8)
}

/// Whether an escape's private data is ours. A vendor's data starts differently.
pub fn is_ours(data: &[u8]) -> bool {
    u64_at(data, 0) == Ok(REQUEST_MAGIC)
}

fn look_at(data: &[u8]) -> Result<Look, Malformed> {
    let look = Look {
        fourcc: u32_at(data, FOURCC_AT)?,
        primaries: u8_at(data, CICP_AT)?,
        transfer: u8_at(data, CICP_AT + 1)?,
        matrix: u8_at(data, CICP_AT + 2)?,
        full_range: u8_at(data, CICP_AT + 3)? != 0,
    };
    // Only what the viewer has been told how to show. Anything else is a driver gone wrong.
    if look == Look::SDR || look == Look::HDR { Ok(look) } else { Err(Malformed) }
}

fn register_at(data: &[u8], name: RingName) -> Result<Register, Malformed> {
    let slots = u32_at(data, SLOTS_AT)?;
    if !(1..=MOST_SLOTS).contains(&slots) {
        return Err(Malformed);
    }
    Ok(Register {
        name,
        slots,
        width: u32_at(data, WIDTH_AT)?,
        height: u32_at(data, HEIGHT_AT)?,
        pitch: u32_at(data, PITCH_AT)?,
        slot_size: u64_at(data, SLOT_SIZE_AT)?,
        look: look_at(data)?,
    })
}

fn mastering_at(data: &[u8]) -> Result<Mastering, Malformed> {
    let word = |index: usize| u16_at(data, HDR_AT + index * size_of::<u16>());
    let mut chromaticity = [0; 8];
    for (index, value) in chromaticity.iter_mut().enumerate() {
        *value = word(index)?;
    }
    Ok(Mastering {
        chromaticity,
        max_nits: word(8)?,
        min: word(9)?,
        max_content_nits: word(10)?,
        max_frame_average_nits: word(11)?,
    })
}

fn frame_at(data: &[u8], name: RingName) -> Result<FrameDone, Malformed> {
    let slot = u32_at(data, SLOT_AT)?;
    let has_hdr = u32_at(data, FLAGS_AT)? & HAS_HDR != 0;
    // The sequence is the driver's count, and the host keeps its own, so it's only checked for being there.
    u64_at(data, SEQUENCE_AT)?;
    Ok(FrameDone {
        name,
        slot: (slot != NO_SLOT).then_some(slot),
        present_time: u64_at(data, PRESENT_AT)?,
        windows_frame: u32_at(data, WINDOWS_FRAME_AT)?,
        sdr_white_nits: u32_at(data, SDR_WHITE_AT)?,
        mastering: if has_hdr { Some(mastering_at(data)?) } else { None },
    })
}

impl Request {
    /// Reads a message, which [`is_ours`] said it was.
    pub fn parse(data: &[u8]) -> Result<Self, Malformed> {
        if !is_ours(data) || u16_at(data, VERSION_AT)? != VERSION {
            return Err(Malformed);
        }
        let size = u32_at(data, SIZE_AT)? as usize;
        let name = RingName { generation: u32_at(data, GENERATION_AT)?, monitor: u32_at(data, MONITOR_AT)? };
        // The buffer must hold the reply too, which is why it can't be shorter than one.
        if data.len() < size.max(REPLY_SIZE) {
            return Err(Malformed);
        }
        match (u16_at(data, KIND_AT)?, size) {
            (KIND_REGISTER, REGISTER_SIZE) => Ok(Self::Register(register_at(data, name)?)),
            (KIND_FRAME, FRAME_SIZE) => Ok(Self::Frame(frame_at(data, name)?)),
            (KIND_CLOSE, HEADER_SIZE) => Ok(Self::Close(name)),
            _ => Err(Malformed),
        }
    }
}

impl Reply {
    /// Writes this over a request, keeping its kind, generation and monitor. Everything past the reply
    /// stays as the guest sent it, as it's only the guest's own bytes going back.
    pub fn write_over(&self, data: &mut [u8]) {
        set_u64(data, 0, REPLY_MAGIC);
        set_u32(data, SIZE_AT, REPLY_SIZE as u32);
        set_u32(data, STATUS_AT, self.outcome.wire());
        set_u32(data, NEXT_SLOT_AT, self.next_slot.unwrap_or(NO_SLOT));
        set_u32(data, REFRESH_AT, self.refresh_millihertz);
        set_u32(data, PEAK_AT, self.peak_nits);
    }
}

impl Stamp {
    /// Reads a slot's first bytes. `None` means this memory isn't a slot.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        (u64_at(bytes, 0).ok()? == STAMP_MAGIC).then_some(Self {
            generation: u32_at(bytes, STAMP_GENERATION_AT).ok()?,
            slot: u32_at(bytes, STAMP_SLOT_AT).ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::u32_at;

    const BUFFER: usize = FRAME_SIZE;

    fn head(kind: u16, size: usize) -> Vec<u8> {
        let mut data = vec![0; BUFFER];
        set_u64(&mut data, 0, REQUEST_MAGIC);
        data[VERSION_AT..VERSION_AT + 2].copy_from_slice(&VERSION.to_le_bytes());
        data[KIND_AT..KIND_AT + 2].copy_from_slice(&kind.to_le_bytes());
        set_u32(&mut data, SIZE_AT, size as u32);
        set_u32(&mut data, GENERATION_AT, 3);
        set_u32(&mut data, MONITOR_AT, 1);
        data
    }

    fn register() -> Vec<u8> {
        let mut data = head(KIND_REGISTER, REGISTER_SIZE);
        set_u32(&mut data, SLOTS_AT, 6);
        set_u32(&mut data, WIDTH_AT, 1920);
        set_u32(&mut data, HEIGHT_AT, 1080);
        set_u32(&mut data, PITCH_AT, 7680);
        set_u64(&mut data, SLOT_SIZE_AT, 7680 * 1080);
        set_u32(&mut data, FOURCC_AT, Look::HDR.fourcc);
        data[CICP_AT..CICP_AT + 4].copy_from_slice(&[CICP_BT2020, CICP_PQ, CICP_IDENTITY, 1]);
        data
    }

    fn frame() -> Vec<u8> {
        let mut data = head(KIND_FRAME, FRAME_SIZE);
        set_u32(&mut data, SLOT_AT, 2);
        set_u64(&mut data, PRESENT_AT, 99);
        set_u32(&mut data, WINDOWS_FRAME_AT, 7);
        set_u32(&mut data, SDR_WHITE_AT, 240);
        data
    }

    #[test]
    fn a_register_is_read_as_the_driver_writes_it() {
        let Ok(Request::Register(ring)) = Request::parse(&register()) else { panic!("it should parse") };
        assert_eq!(ring.name, RingName { generation: 3, monitor: 1 });
        assert_eq!((ring.slots, ring.width, ring.height, ring.pitch), (6, 1920, 1080, 7680));
        assert_eq!(ring.slot_size, 7680 * 1080);
        assert_eq!(ring.look, Look::HDR);
    }

    #[test]
    fn a_frame_is_read_as_the_driver_writes_it() {
        let Ok(Request::Frame(done)) = Request::parse(&frame()) else { panic!("it should parse") };
        assert_eq!(done.slot, Some(2));
        assert_eq!((done.present_time, done.windows_frame, done.sdr_white_nits), (99, 7, 240));
        assert_eq!(done.mastering, None);
    }

    #[test]
    fn no_slot_means_the_driver_is_asking_for_one() {
        let mut data = frame();
        set_u32(&mut data, SLOT_AT, NO_SLOT);
        let Ok(Request::Frame(done)) = Request::parse(&data) else { panic!("it should parse") };
        assert_eq!(done.slot, None);
    }

    #[test]
    fn mastering_data_comes_through_in_its_order() {
        let mut data = frame();
        set_u32(&mut data, FLAGS_AT, HAS_HDR);
        for index in 0..12 {
            data[HDR_AT + index * 2] = index as u8 + 1;
        }
        let Ok(Request::Frame(done)) = Request::parse(&data) else { panic!("it should parse") };
        let mastering = done.mastering.unwrap();
        assert_eq!(mastering.chromaticity, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(mastering.max_nits, 9);
        assert_eq!(mastering.max_frame_average_nits, 12);
    }

    #[test]
    fn a_close_is_only_a_header() {
        let data = head(KIND_CLOSE, HEADER_SIZE);
        assert_eq!(Request::parse(&data), Ok(Request::Close(RingName { generation: 3, monitor: 1 })));
    }

    #[test]
    fn what_a_vendor_sends_is_not_ours() {
        assert!(!is_ours(&[0; 64]));
        assert!(!is_ours(&[]));
        assert!(!is_ours(&REPLY_MAGIC.to_le_bytes()));
        assert!(is_ours(&register()));
    }

    #[test]
    fn nothing_short_or_odd_parses() {
        let good = register();
        for length in 0..REPLY_SIZE {
            assert_eq!(Request::parse(&good[..length]), Err(Malformed), "{length} bytes");
        }
        assert_eq!(Request::parse(&good[..REGISTER_SIZE - 1]), Err(Malformed));

        let changed = |at: usize, value: u32| {
            let mut data = register();
            set_u32(&mut data, at, value);
            Request::parse(&data)
        };
        assert_eq!(changed(SIZE_AT, u32::MAX), Err(Malformed), "a size past the buffer");
        assert_eq!(changed(SIZE_AT, 0), Err(Malformed), "a size that isn't this kind's");
        assert_eq!(changed(SLOTS_AT, 0), Err(Malformed));
        assert_eq!(changed(SLOTS_AT, MOST_SLOTS + 1), Err(Malformed));
        assert_eq!(changed(FOURCC_AT, Look::SDR.fourcc), Err(Malformed), "an SDR format with HDR colour");
        assert_eq!(changed(FOURCC_AT, u32::MAX), Err(Malformed));

        let mut other_version = register();
        other_version[VERSION_AT] = 2;
        assert_eq!(Request::parse(&other_version), Err(Malformed));

        let mut other_kind = register();
        other_kind[KIND_AT] = 9;
        assert_eq!(Request::parse(&other_kind), Err(Malformed));
    }

    #[test]
    fn any_bytes_after_the_magic_never_panic() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..20_000 {
            let mut data = vec![0; BUFFER];
            for byte in &mut data {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            set_u64(&mut data, 0, REQUEST_MAGIC);
            let _ = Request::parse(&data);
        }
    }

    #[test]
    fn a_reply_lands_where_the_driver_reads_it() {
        let mut data = frame();
        let reply = Reply {
            outcome: Outcome::Accepted,
            next_slot: Some(4),
            refresh_millihertz: 240_000,
            peak_nits: 1000,
        };
        reply.write_over(&mut data);
        assert_eq!(u64_at(&data, 0), Ok(REPLY_MAGIC));
        assert_eq!(u32_at(&data, SIZE_AT), Ok(REPLY_SIZE as u32));
        assert_eq!(u32_at(&data, GENERATION_AT), Ok(3), "it names the ring it's about");
        assert_eq!(u32_at(&data, STATUS_AT), Ok(0));
        assert_eq!(u32_at(&data, NEXT_SLOT_AT), Ok(4));
        assert_eq!(u32_at(&data, REFRESH_AT), Ok(240_000));
        assert_eq!(u32_at(&data, PEAK_AT), Ok(1000));

        Reply { outcome: Outcome::Refused, next_slot: None, refresh_millihertz: 0, peak_nits: 0 }
            .write_over(&mut data);
        assert_eq!(u32_at(&data, STATUS_AT), Ok(2));
        assert_eq!(u32_at(&data, NEXT_SLOT_AT), Ok(NO_SLOT));
    }

    #[test]
    fn a_stamp_is_only_a_stamp_with_its_magic() {
        let mut bytes = [0; STAMP_SIZE];
        assert_eq!(Stamp::parse(&bytes), None);
        set_u64(&mut bytes, 0, STAMP_MAGIC);
        set_u32(&mut bytes, STAMP_GENERATION_AT, 3);
        set_u32(&mut bytes, STAMP_SLOT_AT, 5);
        assert_eq!(Stamp::parse(&bytes), Some(Stamp { generation: 3, slot: 5 }));
        assert_eq!(Stamp::parse(&bytes[..STAMP_SIZE - 1]), None);
    }
}
