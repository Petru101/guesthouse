//! The five messages and their bytes, which are little-endian. Every packet starts with the magic
//! number, the protocol's version and what kind of message follows.

use std::fmt;

/// Goes up with any change to a message's bytes.
pub const VERSION: u16 = 1;
/// More slots than a viewer can hold on to is a mistake, and each one is a file descriptor.
pub const MOST_SLOTS: u32 = 16;
pub const MOST_FORMATS: usize = 64;
const MAGIC: [u8; 4] = *b"GHFR";
const HELLO: u16 = 1;
const RING: u16 = 2;
const FRAME: u16 = 3;
const RELEASE: u16 = 4;
const LIGHT: u16 = 5;

/// A pixel format and memory layout, as DRM names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub fourcc: u32,
    pub modifier: u64,
}

/// Viewer to producer, first: what it can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub formats: Vec<Format>,
    /// The monitor's refresh rate, in thousandths of a hertz. 0 if it isn't known.
    pub refresh_millihertz: u32,
    /// The monitor's peak brightness. 0 if it isn't known.
    pub peak_nits: u32,
}

/// Producer to viewer: a new set of buffers, one file descriptor for each slot alongside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ring {
    pub generation: u32,
    pub slots: u32,
    /// The bytes in each slot.
    pub size: u64,
}

/// How a picture's numbers turn into colour, in the code points of ITU-T H.273.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

impl Colour {
    /// BT.709 primaries with the sRGB transfer, as an ordinary desktop is.
    pub const SRGB: Self = Self { primaries: 1, transfer: 13, matrix: 0, full_range: true };
}

/// Where a picture is in its slot, and what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
    /// Bytes from the start of the slot.
    pub offset: u64,
    pub format: Format,
    pub colour: Colour,
}

impl Descriptor {
    /// Whether this picture lies inside a slot of this size. These numbers can come from a guest, so
    /// a viewer must ask before it touches a buffer.
    pub fn fits(&self, size: u64) -> bool {
        let rows = u64::from(self.stride).checked_mul(u64::from(self.height));
        let end = rows.and_then(|rows| rows.checked_add(self.offset));
        self.width > 0 && self.height > 0 && end.is_some_and(|end| end <= size)
    }
}

/// Producer to viewer: a finished picture. The slot is the viewer's until its release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub generation: u32,
    pub slot: u32,
    pub sequence: u64,
    pub descriptor: Descriptor,
}

/// How the display a picture was graded on behaved. The numbers are as Windows' HDR10 metadata has them.
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

/// Producer to viewer, just before the [`Frame`] it's about, and only for an HDR picture. A viewer
/// that offered an HDR format knows this kind. Nothing else ever meets it, so the version stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Light {
    pub generation: u32,
    pub slot: u32,
    /// How bright the guest wants SDR white to be.
    pub sdr_white_nits: u32,
    pub mastering: Option<Mastering>,
}

/// Viewer to producer: the slot can be written again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub generation: u32,
    pub slot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Ring(Ring),
    Frame(Frame),
    Release(Release),
    Light(Light),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    NotOurs,
    OtherVersion(u16),
    UnknownKind(u16),
    /// Too short, too long, or holding a number no message can have.
    Malformed,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOurs => f.write_str("this is not a frame message"),
            Self::OtherVersion(version) => {
                write!(f, "the peer speaks version {version}, and this is {VERSION}")
            }
            Self::UnknownKind(kind) => write!(f, "message kind {kind} is unknown"),
            Self::Malformed => f.write_str("the message is malformed"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u16(&mut self, value: u16) {
        self.0.extend(value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.0.extend(value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend(value.to_le_bytes());
    }

    fn format(&mut self, format: Format) {
        self.u32(format.fourcc);
        self.u64(format.modifier);
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const SIZE: usize>(&mut self) -> Result<[u8; SIZE], DecodeError> {
        let (bytes, rest) = self.0.split_first_chunk::<SIZE>().ok_or(DecodeError::Malformed)?;
        self.0 = rest;
        Ok(*bytes)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take()?))
    }

    fn format(&mut self) -> Result<Format, DecodeError> {
        Ok(Format { fourcc: self.u32()?, modifier: self.u64()? })
    }

    fn finished(&self) -> Result<(), DecodeError> {
        if self.0.is_empty() { Ok(()) } else { Err(DecodeError::Malformed) }
    }
}

fn write_frame(writer: &mut Writer, frame: &Frame) {
    let descriptor = frame.descriptor;
    writer.u32(frame.generation);
    writer.u32(frame.slot);
    writer.u64(frame.sequence);
    writer.u32(descriptor.width);
    writer.u32(descriptor.height);
    writer.u32(descriptor.stride);
    writer.u64(descriptor.offset);
    writer.format(descriptor.format);
    writer.u8(descriptor.colour.primaries);
    writer.u8(descriptor.colour.transfer);
    writer.u8(descriptor.colour.matrix);
    writer.u8(descriptor.colour.full_range.into());
}

fn read_frame(reader: &mut Reader) -> Result<Frame, DecodeError> {
    Ok(Frame {
        generation: reader.u32()?,
        slot: reader.u32()?,
        sequence: reader.u64()?,
        descriptor: Descriptor {
            width: reader.u32()?,
            height: reader.u32()?,
            stride: reader.u32()?,
            offset: reader.u64()?,
            format: reader.format()?,
            colour: Colour {
                primaries: reader.u8()?,
                transfer: reader.u8()?,
                matrix: reader.u8()?,
                full_range: reader.u8()? != 0,
            },
        },
    })
}

fn write_light(writer: &mut Writer, light: &Light) {
    writer.u32(light.generation);
    writer.u32(light.slot);
    writer.u32(light.sdr_white_nits);
    writer.u8(light.mastering.is_some().into());
    let mastering = light.mastering.unwrap_or_default();
    mastering.chromaticity.iter().for_each(|&value| writer.u16(value));
    writer.u16(mastering.max_nits);
    writer.u16(mastering.min);
    writer.u16(mastering.max_content_nits);
    writer.u16(mastering.max_frame_average_nits);
}

fn read_light(reader: &mut Reader) -> Result<Light, DecodeError> {
    let (generation, slot, sdr_white_nits) = (reader.u32()?, reader.u32()?, reader.u32()?);
    let present = reader.u8()? != 0;
    let mut mastering = Mastering::default();
    for value in &mut mastering.chromaticity {
        *value = reader.u16()?;
    }
    mastering.max_nits = reader.u16()?;
    mastering.min = reader.u16()?;
    mastering.max_content_nits = reader.u16()?;
    mastering.max_frame_average_nits = reader.u16()?;
    Ok(Light { generation, slot, sdr_white_nits, mastering: present.then_some(mastering) })
}

fn read_hello(reader: &mut Reader) -> Result<Hello, DecodeError> {
    let refresh_millihertz = reader.u32()?;
    let peak_nits = reader.u32()?;
    let count = usize::from(reader.u16()?);
    if count > MOST_FORMATS {
        return Err(DecodeError::Malformed);
    }
    let formats = (0..count).map(|_| reader.format()).collect::<Result<_, _>>()?;
    Ok(Hello { formats, refresh_millihertz, peak_nits })
}

fn read_ring(reader: &mut Reader) -> Result<Ring, DecodeError> {
    let ring = Ring { generation: reader.u32()?, slots: reader.u32()?, size: reader.u64()? };
    if ring.slots == 0 || ring.slots > MOST_SLOTS {
        return Err(DecodeError::Malformed);
    }
    Ok(ring)
}

impl Message {
    fn kind(&self) -> u16 {
        match self {
            Self::Hello(_) => HELLO,
            Self::Ring(_) => RING,
            Self::Frame(_) => FRAME,
            Self::Release(_) => RELEASE,
            Self::Light(_) => LIGHT,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::default();
        writer.0.extend(MAGIC);
        writer.u16(VERSION);
        writer.u16(self.kind());
        match self {
            Self::Hello(hello) => {
                writer.u32(hello.refresh_millihertz);
                writer.u32(hello.peak_nits);
                writer.u16(hello.formats.len().min(MOST_FORMATS) as u16);
                hello.formats.iter().take(MOST_FORMATS).for_each(|format| writer.format(*format));
            }
            Self::Ring(ring) => {
                writer.u32(ring.generation);
                writer.u32(ring.slots);
                writer.u64(ring.size);
            }
            Self::Frame(frame) => write_frame(&mut writer, frame),
            Self::Light(light) => write_light(&mut writer, light),
            Self::Release(release) => {
                writer.u32(release.generation);
                writer.u32(release.slot);
            }
        }
        writer.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader(bytes);
        if reader.take::<4>() != Ok(MAGIC) {
            return Err(DecodeError::NotOurs);
        }
        let version = reader.u16()?;
        if version != VERSION {
            return Err(DecodeError::OtherVersion(version));
        }
        let message = match reader.u16()? {
            HELLO => Self::Hello(read_hello(&mut reader)?),
            RING => Self::Ring(read_ring(&mut reader)?),
            FRAME => Self::Frame(read_frame(&mut reader)?),
            LIGHT => Self::Light(read_light(&mut reader)?),
            RELEASE => Self::Release(Release { generation: reader.u32()?, slot: reader.u32()? }),
            kind => return Err(DecodeError::UnknownKind(kind)),
        };
        reader.finished()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XRGB: Format = Format { fourcc: 0x3432_5258, modifier: 0 };

    fn descriptor() -> Descriptor {
        Descriptor { width: 1920, height: 1080, stride: 7680, offset: 0, format: XRGB, colour: Colour::SRGB }
    }

    fn all() -> [Message; 6] {
        [
            Message::Hello(Hello { formats: vec![XRGB], refresh_millihertz: 144_000, peak_nits: 400 }),
            Message::Ring(Ring { generation: 7, slots: 3, size: 1 << 23 }),
            Message::Frame(Frame { generation: 7, slot: 2, sequence: 9, descriptor: descriptor() }),
            Message::Light(Light { generation: 7, slot: 2, sdr_white_nits: 240, mastering: None }),
            Message::Light(Light {
                generation: 7,
                slot: 2,
                sdr_white_nits: 203,
                mastering: Some(Mastering {
                    chromaticity: [1, 2, 3, 4, 5, 6, 7, 8],
                    max_nits: 1000,
                    min: 1,
                    max_content_nits: 900,
                    max_frame_average_nits: 400,
                }),
            }),
            Message::Release(Release { generation: 7, slot: 2 }),
        ]
    }

    #[test]
    fn every_message_comes_back_as_it_went() {
        for message in all() {
            assert_eq!(Message::decode(&message.encode()), Ok(message));
        }
    }

    #[test]
    fn another_version_is_refused_by_name() {
        for message in all() {
            let mut bytes = message.encode();
            bytes[4] = 9;
            assert_eq!(Message::decode(&bytes), Err(DecodeError::OtherVersion(9)));
        }
    }

    #[test]
    fn what_is_not_ours_or_not_whole_is_refused() {
        assert_eq!(Message::decode(b"nope and more"), Err(DecodeError::NotOurs));
        assert_eq!(Message::decode(b""), Err(DecodeError::NotOurs));
        for message in all() {
            let bytes = message.encode();
            assert_eq!(Message::decode(&bytes[..bytes.len() - 1]), Err(DecodeError::Malformed));
            assert_eq!(Message::decode(&[&bytes[..], &[0]].concat()), Err(DecodeError::Malformed));
        }
    }

    #[test]
    fn a_ring_must_have_a_sane_number_of_slots() {
        for slots in [0, MOST_SLOTS + 1] {
            let bytes = Message::Ring(Ring { generation: 1, slots, size: 1 }).encode();
            assert_eq!(Message::decode(&bytes), Err(DecodeError::Malformed));
        }
    }

    #[test]
    fn a_picture_must_lie_inside_its_slot() {
        let picture = descriptor();
        let exact = 7680 * 1080;
        assert!(picture.fits(exact));
        assert!(!picture.fits(exact - 1));
        assert!(!Descriptor { offset: 1, ..picture }.fits(exact));
        assert!(!Descriptor { width: 0, ..picture }.fits(exact));
        assert!(!Descriptor { offset: u64::MAX, ..picture }.fits(u64::MAX), "sums must not wrap");
        assert!(!Descriptor { stride: u32::MAX, height: u32::MAX, ..picture }.fits(1 << 40));
    }
}
