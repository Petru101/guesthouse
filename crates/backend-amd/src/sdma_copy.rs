//! Copies a tiled surface into a linear one with the card's DMA engine.
//!
//! A guest's OpenGL window draws into a tiled surface and presents it as a copy to a linear one.
//! The CPU can't read a tiled layout in order, but the DMA engine's sub-window copy can: given the
//! surface's swizzle mode it walks the tiles itself. The packet is the open one that Mesa's RADV
//! sends on SDMA 5 and 6 (GFX10 to GFX11), with the engine's default cache policy.

use crate::screen_copy::PIXEL_BYTES;
use guesthouse_backend::{StagingCopy, SurfaceShape};

// The packet's header: opcode, sub-opcode, and the direction bit.
const OPCODE_COPY: u32 = 0x1;
const SUB_OPCODE_TILED_SUB_WINDOW: u32 = 0x5;
const SUB_OPCODE_SHIFT: u32 = 8;
const DETILE: u32 = 1 << 31;
const OPCODE_NOP: u32 = 0x0;
/// The engine fetches its command buffer in blocks of eight dwords.
const IB_ALIGNMENT_DWORDS: usize = 8;

// The tiled surface's info dword.
const SWIZZLE_SHIFT: u32 = 3;
const DIMENSION_SHIFT: u32 = 9;
const RESOURCE_2D: u32 = 1;
/// log2 of [`PIXEL_BYTES`], the element size field.
const ELEMENT_SIZE_LOG2: u32 = PIXEL_BYTES.trailing_zeros();

/// The engine's limit on each side of a copy, from the smallest field it is carried in.
const MAX_SIDE: u32 = 1 << 14;
/// Swizzle modes 1 to 27 are the tiled ones on GFX10 and GFX11. 0 is linear.
const LAST_SWIZZLE_MODE: u32 = 27;

/// The side of a swizzle mode's block, in 4-byte pixels, for a 2D surface. Blocks are square at this
/// pixel size: 256 bytes are 8 by 8, 4 KiB 32 by 32, and 64 KiB 128 by 128. Modes 12 to 15 are the
/// variable-size ones GFX10 and GFX11 don't have.
fn block_side(swizzle_mode: u32) -> Option<u64> {
    match swizzle_mode {
        1..=3 => Some(8),
        4..=7 | 20..=23 => Some(32),
        8..=11 | 16..=19 | 24..=27 => Some(128),
        _ => None,
    }
}

/// The bytes a tiled surface fills: whole blocks across its rows and down its height.
pub fn surface_bytes(pitch_bytes: u32, height: u32, swizzle_mode: u32) -> Option<u64> {
    let side = block_side(swizzle_mode)?;
    let across = (u64::from(pitch_bytes) / PIXEL_BYTES).div_ceil(side);
    let down = u64::from(height).div_ceil(side);
    Some(across * down * side * side * PIXEL_BYTES)
}

/// A copy of a rectangle from a tiled surface to a linear one, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TiledCopy {
    pub tiled_address: u64,
    pub tiled_width: u32,
    pub tiled_height: u32,
    pub swizzle_mode: u32,
    pub linear_address: u64,
    /// The linear surface's row length and height, in pixels.
    pub linear_pitch: u32,
    pub linear_height: u32,
    pub from: (u32, u32),
    pub to: (u32, u32),
    pub width: u32,
    pub height: u32,
}

/// `(x, y)` as the engine takes an offset: x in the low half, y in the high.
fn halves(low: u32, high: u32) -> u32 {
    low | high << 16
}

impl TiledCopy {
    /// The copy for a staging copy from a tiled surface to a linear one, or `None` for any other
    /// pair, or a rectangle past either surface.
    pub fn of(copy: &StagingCopy) -> Option<Self> {
        let swizzle_mode = copy.source.tiling.filter(|mode| (1..=LAST_SWIZZLE_MODE).contains(mode))?;
        if copy.target.tiling.is_some() {
            return None;
        }
        let side = |value: i32| u32::try_from(value).ok().filter(|&value| value < MAX_SIDE);
        let width = side(copy.right.checked_sub(copy.left)?)?;
        let height = side(copy.bottom.checked_sub(copy.top)?)?;
        let from = (side(copy.left)?, side(copy.top)?);
        let to = (side(copy.target_left)?, side(copy.target_top)?);
        let fits = |shape: &SurfaceShape, (x, y): (u32, u32)| {
            shape.width < MAX_SIDE
                && shape.height < MAX_SIDE
                && x + width <= shape.width
                && y + height <= shape.height
        };
        let pitch_bytes = u64::from(copy.target.pitch);
        let pitch_fits =
            pitch_bytes % PIXEL_BYTES == 0 && pitch_bytes >= u64::from(copy.target.width) * PIXEL_BYTES;
        if width == 0 || height == 0 || !fits(&copy.source, from) || !fits(&copy.target, to) || !pitch_fits {
            return None;
        }
        Some(Self {
            tiled_address: copy.source_address,
            tiled_width: copy.source.width,
            tiled_height: copy.source.height,
            swizzle_mode,
            linear_address: copy.target_address,
            linear_pitch: u32::try_from(pitch_bytes / PIXEL_BYTES).ok().filter(|&pitch| pitch <= MAX_SIDE)?,
            linear_height: copy.target.height,
            from,
            to,
            width,
            height,
        })
    }

    /// The command buffer, padded to the engine's alignment.
    pub fn commands(&self) -> Vec<u32> {
        let header = OPCODE_COPY | SUB_OPCODE_TILED_SUB_WINDOW << SUB_OPCODE_SHIFT | DETILE;
        let info = ELEMENT_SIZE_LOG2 | self.swizzle_mode << SWIZZLE_SHIFT | RESOURCE_2D << DIMENSION_SHIFT;
        let linear_slice = self.linear_pitch * self.linear_height;
        let mut commands = vec![
            header,
            self.tiled_address as u32,
            (self.tiled_address >> 32) as u32,
            halves(self.from.0, self.from.1),
            // The tiled z offset, then its width.
            halves(0, self.tiled_width - 1),
            // Its height, then its depth.
            halves(self.tiled_height - 1, 0),
            info,
            self.linear_address as u32,
            (self.linear_address >> 32) as u32,
            halves(self.to.0, self.to.1),
            // The linear z offset, then its pitch.
            halves(0, self.linear_pitch - 1),
            linear_slice - 1,
            halves(self.width - 1, self.height - 1),
            // The depth of the copy, less one.
            0,
        ];
        while commands.len() % IB_ALIGNMENT_DWORDS != 0 {
            commands.push(OPCODE_NOP);
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::SpaceId;

    fn shape(width: u32, height: u32, tiling: Option<u32>) -> SurfaceShape {
        let format = 21;
        SurfaceShape {
            width,
            height,
            format,
            pitch: width * 4,
            tiling,
            refresh: (0, 0),
            samples: (1, 0),
            colour: format,
        }
    }

    fn copy(source: SurfaceShape, target: SurfaceShape) -> StagingCopy {
        StagingCopy {
            source,
            target,
            source_address: 0x1_2345_0000,
            target_address: 0x2_0000_0000,
            left: 0,
            top: 0,
            right: 640,
            bottom: 400,
            target_left: 0,
            target_top: 0,
            target_guest_address: None,
            is_screen: false,
            own_space: SpaceId(1),
        }
    }

    #[test]
    fn a_window_copies_from_its_tiles_into_rows() {
        let planned = TiledCopy::of(&copy(shape(640, 400, Some(27)), shape(640, 400, None))).unwrap();
        let commands = planned.commands();
        assert_eq!(commands.len(), 16);
        assert_eq!(commands[0], 0x8000_0501);
        assert_eq!(commands[1..3], [0x2345_0000, 0x1]);
        assert_eq!(commands[4], 639 << 16);
        assert_eq!(commands[5], 399);
        assert_eq!(commands[6], 2 | 27 << 3 | 1 << 9);
        assert_eq!(commands[7..9], [0, 0x2]);
        assert_eq!(commands[10], 639 << 16);
        assert_eq!(commands[11], 640 * 400 - 1);
        assert_eq!(commands[12], 639 | 399 << 16);
        assert!(commands[14..].iter().all(|&word| word == OPCODE_NOP));
    }

    #[test]
    fn a_tiled_surface_fills_whole_blocks() {
        assert_eq!(surface_bytes(640 * 4, 400, 27), Some(5 * 4 * 0x10000));
        assert_eq!(surface_bytes(64 * 4, 32, 7), Some(2 * 0x1000));
        assert_eq!(surface_bytes(8 * 4, 9, 1), Some(2 * 0x100));
        assert_eq!(surface_bytes(640 * 4, 400, 13), None);
    }

    #[test]
    fn only_a_tiled_source_and_a_linear_target_are_taken() {
        assert!(TiledCopy::of(&copy(shape(640, 400, None), shape(640, 400, None))).is_none());
        assert!(TiledCopy::of(&copy(shape(640, 400, Some(27)), shape(640, 400, Some(27)))).is_none());
        assert!(TiledCopy::of(&copy(shape(640, 400, Some(0)), shape(640, 400, None))).is_none());
    }

    #[test]
    fn a_rectangle_past_either_surface_is_refused() {
        assert!(TiledCopy::of(&copy(shape(320, 400, Some(27)), shape(640, 400, None))).is_none());
        assert!(TiledCopy::of(&copy(shape(640, 400, Some(27)), shape(640, 200, None))).is_none());
    }
}
