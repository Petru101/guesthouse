//! Surfaces the guest's kernel makes without asking a vendor driver.
//!
//! When the kernel needs one of these, it asks the host to describe it, and later hands the
//! description back when it creates the allocation. A vendor's user-mode driver can read it too,
//! when a program opens the kernel's shared primary, so a vendor whose driver does that gives its
//! own private data here. Otherwise the description is the daemon's own record, which is the only
//! private allocation data the core understands.

use crate::wire::{Malformed, align_up, declared_size, pad_to, put_u32, u32_at};
use crate::{HEADER_SIZE, Status};

const MAGIC: &[u8; 4] = b"GPVS";
const RECORD_SIZE: u32 = 32;
const PITCH_ALIGN: u64 = 256;
const BYTES_PER_PIXEL: u32 = 4;
const FIELD_COUNT: usize = 6;
const DESCRIPTION: usize = 0x28;

const KIND_SHARED_PRIMARY: u32 = 1;
const KIND_SHADOW: u32 = 2;
const KIND_STAGING: u32 = 3;
const KIND_GDI: u32 = 4;

/// The WDK's D3DDDIFMT_X8R8G8B8, the format of a staging surface.
const FORMAT_X8R8G8B8: u32 = 22;

// Offsets in the record.
const RECORD_WIDTH: usize = 8;
const RECORD_HEIGHT: usize = 12;
const RECORD_PITCH: usize = 20;
const RECORD_REFRESH: usize = 24;

// The answer ends in padding that covers these fixed fields and the two blocks of data.
const ANSWER_FIXED_SIZE: u32 = 0x24;
const ANSWER_ALIGN: u64 = 8;

/// A record the core wrote earlier, as read back from the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardRecord {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub refresh: (u32, u32),
}

impl StandardRecord {
    /// `None` when the bytes are not ours, or are too short to be.
    pub fn parse(private: &[u8]) -> Option<Self> {
        if private.first_chunk::<4>() != Some(MAGIC) {
            return None;
        }
        let field = |at| u32_at(private, at).ok();
        Some(Self {
            width: field(RECORD_WIDTH)?,
            height: field(RECORD_HEIGHT)?,
            pitch: field(RECORD_PITCH)?,
            refresh: (field(RECORD_REFRESH).unwrap_or(0), field(RECORD_REFRESH + 4).unwrap_or(0)),
        })
    }

    /// The bytes one row-exact copy of the surface needs. Two 32-bit numbers can't overflow this.
    pub fn exact_size(&self) -> u64 {
        u64::from(self.height) * u64::from(self.pitch)
    }
}

/// What the guest's kernel asks a standard allocation to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardSurface {
    /// The WDK's D3DKMDT_STANDARDALLOCATION_TYPE.
    pub kind: u32,
    pub width: u32,
    pub height: u32,
    /// The guest's own format number. A staging surface names none and is 32-bit colour.
    pub format: u32,
    pub pitch: u32,
    pub refresh: (u32, u32),
    /// The display source a shared primary is for.
    pub source: u32,
}

impl StandardSurface {
    pub fn is_shared_primary(&self) -> bool {
        self.kind == KIND_SHARED_PRIMARY
    }

    /// The daemon's own record of this surface.
    pub fn record(&self) -> StandardRecord {
        StandardRecord { width: self.width, height: self.height, pitch: self.pitch, refresh: self.refresh }
    }
}

/// A request to describe a standard allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescribeStandard {
    fields: [u32; FIELD_COUNT],
    surface: StandardSurface,
    /// How much private data the guest has room for. Zero asks only how much there is.
    allocation_data: u32,
    resource_data: u32,
}

impl DescribeStandard {
    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        let kind = u32_at(message, HEADER_SIZE)?;
        let allocation_data = declared_size(message, HEADER_SIZE + 4)?;
        let resource_data = declared_size(message, HEADER_SIZE + 8)?;
        let mut fields = [0; FIELD_COUNT];
        for (index, field) in fields.iter_mut().enumerate() {
            *field = u32_at(message, DESCRIPTION + index * 4)?;
        }
        let [width, height, ..] = fields;
        let pitch = align_up(u64::from(width) * u64::from(BYTES_PER_PIXEL), PITCH_ALIGN) as u32;
        // Each kind of surface keeps its pitch in a different field of the description.
        match kind {
            KIND_SHADOW => fields[3] = pitch,
            KIND_STAGING => fields[2] = pitch,
            KIND_GDI => fields[5] = pitch,
            _ => {}
        }
        // Only a shared primary's description carries a refresh rate, in fields 3 and 4, and the
        // display source, in field 5. A staging surface has no format field.
        let primary = kind == KIND_SHARED_PRIMARY;
        let refresh = if primary { (fields[3], fields[4]) } else { (0, 0) };
        let format = if kind == KIND_STAGING { FORMAT_X8R8G8B8 } else { fields[2] };
        let source = if primary { fields[5] } else { 0 };
        let surface = StandardSurface { kind, width, height, format, pitch, refresh, source };
        Ok(Self {
            fields,
            surface,
            allocation_data: allocation_data as u32,
            resource_data: resource_data as u32,
        })
    }

    pub fn surface(&self) -> StandardSurface {
        self.surface
    }

    /// Answers with a vendor's private data for the allocation, or with the daemon's own record.
    pub fn answer(&self, vendor_data: Option<&[u8]>, out: &mut Vec<u8>) {
        let surface = &self.surface;
        let size = vendor_data.map_or(RECORD_SIZE, |data| data.len() as u32);
        put_u32(out, Status::SUCCESS.0);
        put_u32(out, size);
        put_u32(out, 0);
        self.fields.iter().for_each(|&field| put_u32(out, field));
        if self.allocation_data >= size {
            match vendor_data {
                Some(data) => out.extend_from_slice(data),
                None => {
                    out.extend_from_slice(MAGIC);
                    let refresh = surface.refresh;
                    for field in [surface.kind, surface.width, surface.height, self.fields[2], surface.pitch]
                    {
                        put_u32(out, field);
                    }
                    put_u32(out, refresh.0);
                    put_u32(out, refresh.1);
                }
            }
        }
        let data = u64::from(self.allocation_data) + u64::from(self.resource_data);
        pad_to(out, align_up(data + u64::from(ANSWER_FIXED_SIZE), ANSWER_ALIGN) as usize);
    }
}

/// Answers a request to describe a standard allocation with the daemon's own record.
pub fn describe(message: &[u8], out: &mut Vec<u8>) -> Result<(), Malformed> {
    DescribeStandard::parse(message)?.answer(None, out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: u32, allocation_data: u32, fields: [u32; 6]) -> Vec<u8> {
        let mut message = vec![0; DESCRIPTION];
        message[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&kind.to_le_bytes());
        message[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&allocation_data.to_le_bytes());
        fields.iter().for_each(|field| message.extend_from_slice(&field.to_le_bytes()));
        message
    }

    #[test]
    fn a_size_query_gets_no_record() {
        let mut out = Vec::new();
        describe(&request(KIND_SHADOW, 0, [100, 50, 0, 0, 0, 0]), &mut out).unwrap();
        assert_eq!(out.len(), 40);
        assert_eq!(u32_at(&out, 4), Ok(RECORD_SIZE));
    }

    #[test]
    fn each_kind_puts_the_pitch_in_its_own_field() {
        for (kind, field) in [(KIND_SHADOW, 3), (KIND_STAGING, 2), (KIND_GDI, 5)] {
            let mut out = Vec::new();
            describe(&request(kind, 32, [100, 50, 21, 0, 0, 0]), &mut out).unwrap();
            assert_eq!(u32_at(&out, 12 + field * 4), Ok(512), "kind {kind}");
        }
    }

    #[test]
    fn the_record_comes_back_readable() {
        let mut out = Vec::new();
        describe(&request(KIND_SHARED_PRIMARY, 32, [1920, 1080, 21, 144, 1, 0]), &mut out).unwrap();
        let record = StandardRecord::parse(&out[36..]).unwrap();
        assert_eq!(record, StandardRecord { width: 1920, height: 1080, pitch: 7680, refresh: (144, 1) });
        assert_eq!(record.exact_size(), 7680 * 1080);
    }

    #[test]
    fn other_data_is_not_a_record() {
        assert_eq!(StandardRecord::parse(b"ABCDxxxxxxxxxxxxxxxxxxxxxxxx"), None);
        assert_eq!(StandardRecord::parse(b"GPVS"), None);
    }
}
