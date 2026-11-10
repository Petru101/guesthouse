//! The private data NVIDIA's kernel driver gives a shared primary, the surface the guest's kernel
//! makes for a display source.
//!
//! On the basic display, a fullscreen Direct3D 9 program's device opens the kernel's shared primary,
//! and the guest's user-mode driver reads that allocation's private data as its own kernel driver
//! wrote it. The daemon's short record isn't that, and the device is never made. The kernel driver
//! makes these primaries tiled, and the user-mode driver draws to them tiled whatever the data says,
//! so this says tiled too, with the blocks a screen-sized surface of the guest's own driver gets.

use crate::profile::AllocationLayout;
use crate::tiling::{GOB_HEIGHT, GOB_WIDTH, surface_bytes};
use guesthouse_backend::{AllocationShape, MemoryKind, SurfaceShape};
use guesthouse_messages::standard::StandardSurface;

/// How long the kernel driver's data is.
pub const SIZE: usize = 0x24A;

const VERSION: usize = 0x04;
const VERSION_KERNEL: u32 = 0x1_0004;
/// The second magic, which says the kernel driver wrote the data.
const KERNEL_MAGIC: usize = 0x0C;
const KERNEL_MAGIC_VALUE: &[u8; 4] = b"MKVN";
const FLAGS: usize = 0x10;
const FLAGS_SHARED_PRIMARY: u64 = 0x0100_0000_0000_080A;
/// The usage word, which starts with the usage byte.
const USAGE_PRIMARY: u32 = 0x804;
const PLACEMENT: usize = 0x24;
const PLACEMENT_PRIMARY: u32 = 0x3;
const SOURCE: usize = 0x48;
const ROWS: usize = 0x6A;
const BYTES_PER_PIXEL: usize = 0x6E;
/// Three words the kernel driver sets to one on every surface it describes.
const ONES: [usize; 3] = [0x72, 0x76, 0x7A];
const HEAP: usize = 0x8A;
const HEAP_VALUE: u32 = 0x7810_0000;
/// A word that starts with the bytes per pixel, and says when the surface is tiled.
const PIXEL_ATTRIBUTES: u32 = 4;
const TILED: u32 = 0x2_0000;
const ATTRIBUTES_PRIMARY: u32 = 0x8;
/// How many blocks deep the surface is, after the block sizes and counts across and down.
const BLOCKS_DEEP: usize = 0x13A;
/// The whole size again, after the block counts.
const TILED_SIZE: usize = 0x146;
const KIND: usize = 0x1E6;

/// The guest's colour formats a primary can have, by the WDK's numbers, with the driver's own.
const COLOURS: [(u32, u8); 2] = [(21, 0x07), (22, 0x09)];
const PIXEL_BYTES: u32 = 4;
/// Rows of a tiled surface start on this many bytes.
const PITCH_ALIGN: u32 = 128;
/// The highest block the guest's driver gives a screen: sixteen GOBs.
const SCREEN_BLOCK: u32 = 4;
/// Video memory comes in pages of this size for anything as large as a screen.
const BIG_PAGE: u64 = 0x1_0000;
/// The core's own colour word for a standard surface, which is no special case for a copy.
const STANDARD_COLOUR: u32 = 0;

/// How a surface is tiled: bytes per row, block height, and the bytes it fills.
struct Blocks {
    pitch: u32,
    block: u32,
    bytes: u64,
}

fn blocks(surface: &StandardSurface) -> Option<Blocks> {
    let pitch = surface.width.checked_mul(PIXEL_BYTES)?.checked_next_multiple_of(PITCH_ALIGN)?;
    let gobs_high = u64::from(surface.height).div_ceil(GOB_HEIGHT);
    let block = (0..SCREEN_BLOCK).find(|&block| 1 << block >= gobs_high).unwrap_or(SCREEN_BLOCK);
    let bytes = surface_bytes(pitch, surface.height, block)?;
    Some(Blocks { pitch, block, bytes })
}

fn colour(surface: &StandardSurface) -> Option<u8> {
    let found = COLOURS.iter().find(|(format, _)| *format == surface.format);
    found.map(|&(_, colour)| colour)
}

fn put(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// The data for a shared primary, or `None` for other surfaces and colour formats, which no
/// user-mode driver reads and which keep the daemon's own record.
pub fn data(at: &AllocationLayout, surface: &StandardSurface) -> Option<Vec<u8>> {
    if !surface.is_shared_primary() {
        return None;
    }
    let (colour, tiled) = (colour(surface)?, blocks(surface)?);
    let mut data = vec![0; SIZE];
    data[..4].copy_from_slice(at.magic);
    put(&mut data, VERSION, VERSION_KERNEL);
    put(&mut data, at.own_length, SIZE as u32);
    data[KERNEL_MAGIC..KERNEL_MAGIC + 4].copy_from_slice(KERNEL_MAGIC_VALUE);
    data[FLAGS..FLAGS + 8].copy_from_slice(&FLAGS_SHARED_PRIMARY.to_le_bytes());
    put(&mut data, at.format, USAGE_PRIMARY);
    put(&mut data, PLACEMENT, PLACEMENT_PRIMARY);
    data[at.colour] = colour;
    put(&mut data, at.width, surface.width);
    put(&mut data, at.height, surface.height);
    put(&mut data, at.depth, 1);
    put(&mut data, at.mip_levels, 1);
    put(&mut data, at.array_layers, 1);
    put(&mut data, SOURCE, surface.source);
    put(&mut data, at.refresh, surface.refresh.0);
    put(&mut data, at.refresh + 4, surface.refresh.1);
    put(&mut data, ROWS, surface.height);
    put(&mut data, BYTES_PER_PIXEL, PIXEL_BYTES);
    ONES.iter().for_each(|&one| put(&mut data, one, 1));
    put(&mut data, HEAP, HEAP_VALUE);
    let bytes = u32::try_from(tiled.bytes).ok()?;
    put(&mut data, at.pitch, tiled.pitch);
    put(&mut data, at.total_size, bytes);
    put(&mut data, at.depth_bits, PIXEL_ATTRIBUTES | TILED);
    put(&mut data, at.attributes, ATTRIBUTES_PRIMARY);
    put(&mut data, at.tiling, tiled.block);
    put(&mut data, at.columns, tiled.pitch / GOB_WIDTH as u32);
    let block_rows = (GOB_HEIGHT as u32) << tiled.block;
    put(&mut data, at.rows, surface.height.div_ceil(block_rows));
    put(&mut data, BLOCKS_DEEP, 1);
    put(&mut data, TILED_SIZE, bytes);
    put(&mut data, KIND, surface.kind);
    Some(data)
}

/// The shape of a shared primary [`data`] describes: tiled the same way.
pub fn shape(surface: &StandardSurface) -> Option<AllocationShape> {
    if !surface.is_shared_primary() || colour(surface).is_none() {
        return None;
    }
    let tiled = blocks(surface)?;
    Some(AllocationShape {
        size: tiled.bytes.div_ceil(BIG_PAGE).checked_mul(BIG_PAGE)?,
        exact_size: tiled.bytes,
        surface: Some(SurfaceShape {
            width: surface.width,
            height: surface.height,
            format: surface.format,
            pitch: tiled.pitch,
            tiling: Some(tiled.block),
            refresh: surface.refresh,
            samples: (1, 0),
            colour: STANDARD_COLOUR,
        }),
        gpu_cached: false,
        kind: MemoryKind::PLAIN,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_data;
    use crate::profile::GUEST_617;

    const LAYOUT: &AllocationLayout = &GUEST_617.allocation;

    fn surface(kind: u32, format: u32) -> StandardSurface {
        StandardSurface {
            kind,
            width: 1920,
            height: 1080,
            format,
            pitch: 7680,
            refresh: (0xFFFF_FFFE, 0xFFFF_FFFE),
            source: 0,
        }
    }

    fn word(data: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_shared_primary_reads_as_the_kernel_drivers_own_data() {
        let data = data(LAYOUT, &surface(1, 21)).unwrap();
        assert_eq!(data.len(), SIZE);
        assert_eq!(&data[..4], b"ADVN");
        assert_eq!(&data[KERNEL_MAGIC..KERNEL_MAGIC + 4], b"MKVN");
        assert_eq!(word(&data, LAYOUT.own_length), 0x24A);
        assert_eq!(data[LAYOUT.colour], 0x07);
    }

    /// The guest's own driver gives a 1920 x 1080 screen these blocks.
    #[test]
    fn a_screen_gets_the_blocks_the_guests_driver_gives_one() {
        let data = data(LAYOUT, &surface(1, 22)).unwrap();
        assert_eq!(word(&data, LAYOUT.pitch), 0x1E00);
        assert_eq!(word(&data, LAYOUT.tiling), 4);
        assert_eq!(word(&data, LAYOUT.columns), 0x78);
        assert_eq!(word(&data, LAYOUT.rows), 9);
        assert_eq!(word(&data, LAYOUT.total_size), 0x87_0000);
        assert_eq!(data[LAYOUT.colour], 0x09);
    }

    #[test]
    fn the_data_and_the_shape_agree() {
        let primary = surface(1, 21);
        let read = private_data::surface_shape(LAYOUT, &data(LAYOUT, &primary).unwrap()).unwrap();
        let shape = shape(&primary).unwrap();
        let made = shape.surface.unwrap();
        assert_eq!((read.width, read.height, read.pitch, read.tiling), (1920, 1080, 7680, Some(4)));
        assert_eq!((made.pitch, made.tiling), (read.pitch, read.tiling));
        assert_eq!(shape.exact_size, 0x87_0000);
        assert_eq!(shape.size, 0x87_0000);
    }

    #[test]
    fn other_surfaces_keep_the_daemons_record() {
        for kind in [2, 3, 4] {
            assert_eq!(data(LAYOUT, &surface(kind, 21)), None, "kind {kind}");
            assert_eq!(shape(&surface(kind, 21)), None, "kind {kind}");
        }
        assert_eq!(data(LAYOUT, &surface(1, 113)), None);
    }

    #[test]
    fn a_short_screen_gets_a_shorter_block() {
        let short = StandardSurface { height: 60, ..surface(1, 21) };
        assert_eq!(shape(&short).unwrap().surface.unwrap().tiling, Some(3));
    }
}
