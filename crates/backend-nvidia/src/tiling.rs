//! How the copy engine lays out a tiled surface. Field names and limits are in the open kernel
//! module's header for the copy class, `clc7b5.h`.
//!
//! A surface is built from GOBs of 64 bytes by 8 rows. A block is one GOB wide and `1 << block` GOBs
//! high, and a surface fills whole blocks, so its pitch and height both round up to them.

pub const GOB_WIDTH: u64 = 64;
pub const GOB_HEIGHT: u64 = 8;
/// A block height past this is not a real one: the engine stops at thirty-two GOBs.
pub const BLOCK_LIMIT: u32 = 6;
/// The source's origin is two 16-bit fields of one word.
const ORIGIN_LIMIT: u64 = 1 << 16;

/// The bytes a surface of this pitch, height and block height fills. `None` for a block height the
/// engine doesn't have, and for a size that needs more than 64 bits.
pub fn surface_bytes(pitch: u32, height: u32, block: u32) -> Option<u64> {
    if block >= BLOCK_LIMIT {
        return None;
    }
    let block_rows = u128::from(GOB_HEIGHT << block);
    let gob_width = u128::from(GOB_WIDTH);
    let columns = u128::from(pitch).div_ceil(gob_width);
    let rows = u128::from(height).div_ceil(block_rows);
    u64::try_from(columns * rows * gob_width * block_rows).ok()
}

/// Whether a corner fits the engine's origin fields.
pub fn origin_fits(x: u64, y: u64) -> bool {
    x < ORIGIN_LIMIT && y < ORIGIN_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_fills_whole_blocks_in_both_directions() {
        assert_eq!(surface_bytes(64, 8, 0), Some(512));
        assert_eq!(surface_bytes(65, 8, 0), Some(1024), "the pitch rounds up to a GOB");
        assert_eq!(surface_bytes(64, 9, 0), Some(1024), "the height rounds up to a block");
        assert_eq!(surface_bytes(64, 1, 4), Some(64 * 128));
    }

    #[test]
    fn a_1080p_screen_takes_the_blocks_past_its_last_row() {
        let height_in_blocks = 1080u64.div_ceil(128);
        assert_eq!(surface_bytes(7680, 1080, 4), Some(7680 * height_in_blocks * 128));
    }

    #[test]
    fn nothing_fills_nothing() {
        assert_eq!(surface_bytes(0, 1080, 4), Some(0));
        assert_eq!(surface_bytes(7680, 0, 4), Some(0));
    }

    #[test]
    fn only_the_engines_block_heights_have_a_size() {
        assert!(surface_bytes(64, 8, BLOCK_LIMIT - 1).is_some());
        assert_eq!(surface_bytes(64, 8, BLOCK_LIMIT), None);
        assert_eq!(surface_bytes(64, 8, u32::MAX), None);
    }

    #[test]
    fn a_size_past_64_bits_has_no_answer_and_does_not_wrap() {
        assert_eq!(surface_bytes(u32::MAX, u32::MAX, 5), None, "both round up to exactly 1 << 64");
        assert!(surface_bytes(u32::MAX - 64, u32::MAX - 256, 5).is_some());
    }

    #[test]
    fn a_corner_has_sixteen_bits_each_way() {
        assert!(origin_fits(0xFFFF, 0xFFFF));
        assert!(!origin_fits(0x1_0000, 0));
        assert!(!origin_fits(0, 0x1_0000));
    }
}
