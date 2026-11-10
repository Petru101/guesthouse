//! The private data AMD's kernel driver gives a surface the guest's kernel makes itself.
//!
//! On the basic display, a fullscreen program's device opens the kernel's shared primary, and AMD's
//! user-mode driver reads that allocation's private data in its own layout: the header and one
//! surface descriptor of [`super::allocation`]. The kernel driver writes those for a standard
//! allocation, with no wrapper and no second information block, and so does this. Host memory
//! behind these surfaces is rows in order, so the descriptor says linear.

use super::allocation::SURFACE_SIZE;
use super::bytes::{put_dword, put_qword};
use guesthouse_messages::standard::StandardSurface;

/// The header and one surface descriptor.
pub const SIZE: usize = FIRST_SURFACE + SURFACE_SIZE;

// The header.
const CLAIMED_SIZE: usize = 0x00;
const FLAGS: usize = 0x04;
/// The size is the 64-bit one, and the kernel driver made the data.
const FLAGS_STANDARD: u32 = 0x9;
const BUFFER_DESCRIPTOR: usize = 0x08;
const BUFFER_DESCRIPTOR_SIZE: u32 = 0x80;
const VM_FLAGS: usize = 0x10;
/// What the kernel driver asks of a shared primary's placement.
const VM_FLAGS_SHARED_PRIMARY: u32 = 0xA200_0001;
const ALIGNMENT: usize = 0x18;
const SURFACE_ALIGNMENT: u32 = 0x1_0000;
const SIZE_64: usize = 0x68;
const SURFACE_COUNT: usize = 0x88;
const FIRST_SURFACE: usize = 0xC4;

// The surface descriptor, by offset from its start.
const SURFACE_OWN_SIZE: usize = 0x00;
const SURFACE_FLAGS: usize = 0x04;
const SURFACE_SWIZZLE_MODE: usize = 0x08;
const SWIZZLE_LINEAR: u32 = 0;
const SURFACE_DIMENSIONS: usize = 0x0C;
const TWO_DIMENSIONS: u32 = 2;
const SURFACE_FORMAT: usize = 0x10;
const SURFACE_WIDTH: usize = 0x14;
const SURFACE_HEIGHT: usize = 0x18;
/// In bytes.
const SURFACE_PITCH: usize = 0x1C;
/// The rows the pitch covers, which for rows in order are the height.
const SURFACE_PITCH_HEIGHT: usize = 0x20;
const SURFACE_DEPTH: usize = 0x24;
const SURFACE_SOURCE: usize = 0x30;
const SURFACE_REFRESH: usize = 0x34;
const SURFACE_KERNEL_FLAGS: usize = 0x54;
/// Always set in a descriptor the kernel driver writes.
const KERNEL_MADE: u32 = 0x8000_0000;

/// The descriptor's flags for each kind of standard allocation, by the WDK's numbers.
fn surface_flags(kind: u32) -> u32 {
    match kind {
        1 => 0x1,
        2 => 0x4,
        3 => 0x8,
        _ => 0x20,
    }
}

/// The private data for a standard surface.
pub fn data(surface: &StandardSurface) -> Vec<u8> {
    let mut data = vec![0; SIZE];
    let size = u64::from(surface.pitch) * u64::from(surface.height);
    put_dword(&mut data, CLAIMED_SIZE, SIZE as u32);
    put_dword(&mut data, FLAGS, FLAGS_STANDARD);
    put_dword(&mut data, BUFFER_DESCRIPTOR, BUFFER_DESCRIPTOR_SIZE);
    if surface.is_shared_primary() {
        put_dword(&mut data, VM_FLAGS, VM_FLAGS_SHARED_PRIMARY);
    }
    put_dword(&mut data, ALIGNMENT, SURFACE_ALIGNMENT);
    put_qword(&mut data, SIZE_64, size);
    put_dword(&mut data, SURFACE_COUNT, 1);

    let at = |offset: usize| FIRST_SURFACE + offset;
    put_dword(&mut data, at(SURFACE_OWN_SIZE), SURFACE_SIZE as u32);
    put_dword(&mut data, at(SURFACE_FLAGS), surface_flags(surface.kind));
    put_dword(&mut data, at(SURFACE_SWIZZLE_MODE), SWIZZLE_LINEAR);
    put_dword(&mut data, at(SURFACE_DIMENSIONS), TWO_DIMENSIONS);
    put_dword(&mut data, at(SURFACE_FORMAT), surface.format);
    put_dword(&mut data, at(SURFACE_WIDTH), surface.width);
    put_dword(&mut data, at(SURFACE_HEIGHT), surface.height);
    put_dword(&mut data, at(SURFACE_PITCH), surface.pitch);
    put_dword(&mut data, at(SURFACE_PITCH_HEIGHT), surface.height);
    put_qword(&mut data, at(SURFACE_DEPTH), 1);
    put_dword(&mut data, at(SURFACE_SOURCE), surface.source);
    put_dword(&mut data, at(SURFACE_REFRESH), surface.refresh.0);
    put_dword(&mut data, at(SURFACE_REFRESH) + 4, surface.refresh.1);
    put_dword(&mut data, at(SURFACE_KERNEL_FLAGS), KERNEL_MADE);
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::allocation;
    use crate::protocol::bytes::dword;

    /// The second information block's offset in the header, which the user-mode driver follows.
    const SECOND_BLOCK_OFFSET: usize = 0xB0;

    fn primary() -> StandardSurface {
        StandardSurface {
            kind: 1,
            width: 1920,
            height: 1080,
            format: 21,
            pitch: 7680,
            refresh: (144, 1),
            source: 0,
        }
    }

    #[test]
    fn a_shared_primary_reads_as_the_kernel_drivers_own_data() {
        let data = data(&primary());
        assert_eq!(data.len(), 0x1E4);
        let read = allocation::parse(&data).unwrap();
        assert_eq!(read.size, 7680 * 1080);
        assert_eq!(read.surfaces.len(), 1);
        let surface = read.surfaces[0];
        assert_eq!((surface.width, surface.height, surface.pitch, surface.format), (1920, 1080, 7680, 21));
        assert!(!read.has_second_block);
    }

    #[test]
    fn the_user_mode_driver_finds_no_second_block_to_follow() {
        assert_eq!(dword(&data(&primary()), SECOND_BLOCK_OFFSET), Some(0));
    }

    #[test]
    fn a_standard_surface_is_rows_in_order() {
        let shape = allocation::shape(&data(&primary()));
        let surface = shape.surface.unwrap();
        assert_eq!(surface.tiling, None);
        assert_eq!(shape.exact_size, 7680 * 1080);
    }
}
