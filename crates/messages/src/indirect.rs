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
const KIND_CURSOR_SHAPE: u16 = 4;
const KIND_CURSOR_MOVE: u16 = 5;

const HEADER_SIZE: usize = 24;
const REGISTER_SIZE: usize = 64;
const FRAME_SIZE: usize = 80;
const REPLY_SIZE: usize = 40;
const CURSOR_PIECE_HEADER: usize = 64;
const CURSOR_MOVE_SIZE: usize = 40;
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
const MODE_REFRESH_AT: usize = 52;
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
// In a piece of a cursor.
const CURSOR_SHAPE_AT: usize = 24;
const CURSOR_WIDTH_AT: usize = 28;
const CURSOR_HEIGHT_AT: usize = 32;
const CURSOR_PITCH_AT: usize = 36;
const CURSOR_HOT_X_AT: usize = 40;
const CURSOR_HOT_Y_AT: usize = 44;
const CURSOR_TOTAL_AT: usize = 48;
const CURSOR_OFFSET_AT: usize = 52;
const CURSOR_BYTES_AT: usize = 56;
// In a cursor's move.
const CURSOR_X_AT: usize = 24;
const CURSOR_Y_AT: usize = 28;
const CURSOR_VISIBLE_AT: usize = 32;
const CURSOR_WHITE_AT: usize = 36;
// In a stamp.
const STAMP_GENERATION_AT: usize = 8;
const STAMP_SLOT_AT: usize = 12;

const NO_SLOT: u32 = u32::MAX;
const HAS_HDR: u32 = 1;
/// More would be a mistake, and each is a descriptor sent to the viewer.
pub const MOST_SLOTS: u32 = 8;

/// A cursor bigger than this on a side is refused. It's held in the host's memory and sent to the viewer.
pub const MOST_CURSOR_SIDE: u32 = 256;
const CURSOR_BYTES_PER_PIXEL: u32 = 4;

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
    /// The rate Windows committed for this mode, 0 from a driver too old to say.
    pub refresh_millihertz: u32,
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

/// One piece of a cursor's picture. A piece that parses is inside its picture, and the picture is small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPiece<'a> {
    pub monitor: u32,
    pub shape: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub total: u32,
    pub offset: u32,
    pub pixels: &'a [u8],
}

/// Where the cursor's picture is, top left, on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorMove {
    pub monitor: u32,
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub sdr_white_nits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    Register(Register),
    Frame(FrameDone),
    Close(RingName),
    CursorShape(CursorPiece<'a>),
    CursorMove(CursorMove),
}

/// What became of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// That ring isn't the one the host has. The driver makes a new one.
    StaleGeneration,
    /// The host can't use this ring. The driver waits for a new mode.
    Refused,
    /// No viewer draws a cursor of its own, so it has to stay in the frames.
    CursorNotWanted,
}

impl Outcome {
    fn wire(self) -> u32 {
        match self {
            Self::Accepted => 0,
            Self::StaleGeneration => 1,
            Self::Refused => 2,
            Self::CursorNotWanted => 3,
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
        refresh_millihertz: u32_at(data, MODE_REFRESH_AT)?,
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

fn cursor_piece_at(data: &[u8], monitor: u32, size: usize) -> Result<CursorPiece<'_>, Malformed> {
    let (width, height) = (u32_at(data, CURSOR_WIDTH_AT)?, u32_at(data, CURSOR_HEIGHT_AT)?);
    let pitch = u32_at(data, CURSOR_PITCH_AT)?;
    let total = u32_at(data, CURSOR_TOTAL_AT)?;
    let offset = u32_at(data, CURSOR_OFFSET_AT)?;
    let bytes = u32_at(data, CURSOR_BYTES_AT)?;
    let side = 1..=MOST_CURSOR_SIDE;
    let shape_makes_sense = side.contains(&width)
        && side.contains(&height)
        && (width * CURSOR_BYTES_PER_PIXEL..=MOST_CURSOR_SIDE * CURSOR_BYTES_PER_PIXEL).contains(&pitch)
        && pitch * height == total;
    let piece_fits = bytes > 0
        && offset.checked_add(bytes).is_some_and(|end| end <= total)
        && bytes as usize + CURSOR_PIECE_HEADER == size;
    if !shape_makes_sense || !piece_fits {
        return Err(Malformed);
    }
    Ok(CursorPiece {
        monitor,
        shape: u32_at(data, CURSOR_SHAPE_AT)?,
        width,
        height,
        pitch,
        hot_x: u32_at(data, CURSOR_HOT_X_AT)?,
        hot_y: u32_at(data, CURSOR_HOT_Y_AT)?,
        total,
        offset,
        pixels: &data[CURSOR_PIECE_HEADER..size],
    })
}

fn cursor_move_at(data: &[u8], monitor: u32) -> Result<CursorMove, Malformed> {
    Ok(CursorMove {
        monitor,
        x: u32_at(data, CURSOR_X_AT)? as i32,
        y: u32_at(data, CURSOR_Y_AT)? as i32,
        visible: u32_at(data, CURSOR_VISIBLE_AT)? != 0,
        sdr_white_nits: u32_at(data, CURSOR_WHITE_AT)?,
    })
}

impl<'a> Request<'a> {
    /// Reads a message, which [`is_ours`] said it was.
    pub fn parse(data: &'a [u8]) -> Result<Self, Malformed> {
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
            (KIND_CURSOR_SHAPE, _) => Ok(Self::CursorShape(cursor_piece_at(data, name.monitor, size)?)),
            (KIND_CURSOR_MOVE, CURSOR_MOVE_SIZE) => Ok(Self::CursorMove(cursor_move_at(data, name.monitor)?)),
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

/// The bytes the driver would send, for tests that play it. Its real source is in C.
pub mod as_the_driver_sends {
    use super::*;
    use crate::wire::set_u32;

    /// The driver sends every message in a buffer of this size, so that any reply fits.
    pub const BUFFER_SIZE: usize = FRAME_SIZE;

    fn head(kind: u16, size: usize, name: RingName) -> Vec<u8> {
        let mut data = vec![0; BUFFER_SIZE];
        set_u64(&mut data, 0, REQUEST_MAGIC);
        data[VERSION_AT..VERSION_AT + 2].copy_from_slice(&VERSION.to_le_bytes());
        data[KIND_AT..KIND_AT + 2].copy_from_slice(&kind.to_le_bytes());
        set_u32(&mut data, SIZE_AT, size as u32);
        set_u32(&mut data, GENERATION_AT, name.generation);
        set_u32(&mut data, MONITOR_AT, name.monitor);
        data
    }

    pub fn register(ring: &Register) -> Vec<u8> {
        let mut data = head(KIND_REGISTER, REGISTER_SIZE, ring.name);
        set_u32(&mut data, SLOTS_AT, ring.slots);
        set_u32(&mut data, WIDTH_AT, ring.width);
        set_u32(&mut data, HEIGHT_AT, ring.height);
        set_u32(&mut data, PITCH_AT, ring.pitch);
        set_u64(&mut data, SLOT_SIZE_AT, ring.slot_size);
        set_u32(&mut data, FOURCC_AT, ring.look.fourcc);
        set_u32(&mut data, MODE_REFRESH_AT, ring.refresh_millihertz);
        let look = ring.look;
        data[CICP_AT..CICP_AT + 4].copy_from_slice(&[look.primaries, look.transfer, look.matrix, 1]);
        data
    }

    pub fn frame(done: &FrameDone) -> Vec<u8> {
        let mut data = head(KIND_FRAME, FRAME_SIZE, done.name);
        set_u32(&mut data, SLOT_AT, done.slot.unwrap_or(NO_SLOT));
        set_u64(&mut data, PRESENT_AT, done.present_time);
        set_u32(&mut data, WINDOWS_FRAME_AT, done.windows_frame);
        set_u32(&mut data, SDR_WHITE_AT, done.sdr_white_nits);
        if let Some(mastering) = done.mastering {
            set_u32(&mut data, FLAGS_AT, HAS_HDR);
            let tail = [
                mastering.max_nits,
                mastering.min,
                mastering.max_content_nits,
                mastering.max_frame_average_nits,
            ];
            let words = mastering.chromaticity.iter().chain(&tail);
            for (index, word) in words.enumerate() {
                let at = HDR_AT + index * size_of::<u16>();
                data[at..at + size_of::<u16>()].copy_from_slice(&word.to_le_bytes());
            }
        }
        data
    }

    pub fn close(name: RingName) -> Vec<u8> {
        head(KIND_CLOSE, HEADER_SIZE, name)
    }

    pub fn cursor_piece(piece: &CursorPiece<'_>) -> Vec<u8> {
        let size = CURSOR_PIECE_HEADER + piece.pixels.len();
        let mut data = head(KIND_CURSOR_SHAPE, size, RingName { generation: 0, monitor: piece.monitor });
        data.resize(size, 0);
        set_u32(&mut data, CURSOR_SHAPE_AT, piece.shape);
        set_u32(&mut data, CURSOR_WIDTH_AT, piece.width);
        set_u32(&mut data, CURSOR_HEIGHT_AT, piece.height);
        set_u32(&mut data, CURSOR_PITCH_AT, piece.pitch);
        set_u32(&mut data, CURSOR_HOT_X_AT, piece.hot_x);
        set_u32(&mut data, CURSOR_HOT_Y_AT, piece.hot_y);
        set_u32(&mut data, CURSOR_TOTAL_AT, piece.total);
        set_u32(&mut data, CURSOR_OFFSET_AT, piece.offset);
        set_u32(&mut data, CURSOR_BYTES_AT, piece.pixels.len() as u32);
        data[CURSOR_PIECE_HEADER..].copy_from_slice(piece.pixels);
        data
    }

    pub fn cursor_move(position: &CursorMove) -> Vec<u8> {
        let name = RingName { generation: 0, monitor: position.monitor };
        let mut data = head(KIND_CURSOR_MOVE, CURSOR_MOVE_SIZE, name);
        set_u32(&mut data, CURSOR_X_AT, position.x as u32);
        set_u32(&mut data, CURSOR_Y_AT, position.y as u32);
        set_u32(&mut data, CURSOR_VISIBLE_AT, position.visible.into());
        set_u32(&mut data, CURSOR_WHITE_AT, position.sdr_white_nits);
        data
    }

    pub fn stamp(stamp: Stamp) -> Vec<u8> {
        let mut bytes = vec![0; STAMP_SIZE];
        set_u64(&mut bytes, 0, STAMP_MAGIC);
        set_u32(&mut bytes, STAMP_GENERATION_AT, stamp.generation);
        set_u32(&mut bytes, STAMP_SLOT_AT, stamp.slot);
        bytes
    }

    /// Reads a reply as the driver does. `None` means there wasn't one.
    pub fn reply(data: &[u8]) -> Option<(u32, Option<u32>)> {
        (u64_at(data, 0).ok()? == REPLY_MAGIC).then_some(())?;
        let next = u32_at(data, NEXT_SLOT_AT).ok()?;
        Some((u32_at(data, STATUS_AT).ok()?, (next != NO_SLOT).then_some(next)))
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
        set_u32(&mut data, MODE_REFRESH_AT, 143_856);
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
        assert_eq!(ring.refresh_millihertz, 143_856);
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
            Request::parse(&data).map(|_| ())
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

    fn piece(pixels: &[u8]) -> CursorPiece<'_> {
        CursorPiece {
            monitor: 1,
            shape: 7,
            width: 4,
            height: 4,
            pitch: 16,
            hot_x: 1,
            hot_y: 2,
            total: 64,
            offset: 0,
            pixels,
        }
    }

    #[test]
    fn a_cursor_piece_is_read_as_the_driver_writes_it() {
        let pixels = [9; 64];
        let bytes = as_the_driver_sends::cursor_piece(&piece(&pixels));
        assert_eq!(Request::parse(&bytes), Ok(Request::CursorShape(piece(&pixels))));
    }

    #[test]
    fn a_cursor_piece_must_lie_in_a_small_picture() {
        let pixels = [9; 16];
        let good = CursorPiece { offset: 48, ..piece(&pixels) };
        assert!(Request::parse(&as_the_driver_sends::cursor_piece(&good)).is_ok());

        let bad = [
            CursorPiece { offset: 49, ..good },
            CursorPiece { offset: u32::MAX, ..good },
            CursorPiece { total: 63, ..good },
            CursorPiece { width: 0, ..good },
            CursorPiece { height: 0, total: 0, ..good },
            CursorPiece { width: MOST_CURSOR_SIDE + 1, pitch: 2048, total: 8192, ..good },
            CursorPiece { height: MOST_CURSOR_SIDE + 1, total: 16 * 257, ..good },
            CursorPiece { pitch: 15, total: 60, ..good },
            CursorPiece { pitch: u32::MAX, ..good },
            CursorPiece { height: u32::MAX, ..good },
        ];
        for piece in bad {
            assert_eq!(
                Request::parse(&as_the_driver_sends::cursor_piece(&piece)),
                Err(Malformed),
                "{piece:?}"
            );
        }
    }

    #[test]
    fn a_cursor_piece_shorter_than_it_says_is_refused() {
        let pixels = [9; 16];
        let bytes = as_the_driver_sends::cursor_piece(&piece(&pixels));
        assert_eq!(Request::parse(&bytes[..bytes.len() - 1]), Err(Malformed));
    }

    #[test]
    fn a_cursor_move_can_be_off_the_screen() {
        let position = CursorMove { monitor: 0, x: -5, y: 40, visible: true, sdr_white_nits: 240 };
        let bytes = as_the_driver_sends::cursor_move(&position);
        assert_eq!(Request::parse(&bytes), Ok(Request::CursorMove(position)));
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
