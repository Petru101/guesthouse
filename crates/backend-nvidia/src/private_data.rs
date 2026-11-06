//! The guest driver's private data for an allocation: what it asks for, and what the kernel side
//! would fill in on real hardware.

use crate::profile::AllocationLayout;
use guesthouse_backend::{AllocationShape, SurfaceShape};

const SMALL_PAGE: u64 = 0x1000;
const BIG_PAGE: u64 = 0x10000;
const DEFAULT_SIZE: u64 = 4096;
const GOB_WIDTH: u64 = 64;
const GOB_HEIGHT: u64 = 8;
/// A block height past this is not a real one, so the field holds something else.
const TILING_LIMIT: u32 = 6;

/// Bits 3:2 of the second attribute word say whether the GPU's cache may hold this memory.
const CACHE_SHIFT: u32 = 2;
const CACHE_MASK: u32 = 3;
const CACHE_YES: u32 = 1;

/// Bit 0 of the segments byte asks for video memory.
const SEGMENT_VIDEO: u8 = 1;

// The page size and place the kernel side picks, in the RM's own attribute bits.
const PAGE_SIZE_4K: u32 = 1 << 23;
const PAGE_SIZE_BIG: u32 = 2 << 23;
const PAGE_SIZE_HUGE: u32 = 3 << 23;
const LOCATION_PCI: u32 = 1 << 25;
const ATTRIBUTE_2_HUGE_2M: u32 = 1 << 20;
const HUGE_PAGE: u32 = 2 << 20;
/// Between what has been seen of 4 KiB and big pages.
const BIG_PAGE_FROM: u32 = 0x10_0000;

fn word(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

fn set_word(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn is_ours(layout: &AllocationLayout, data: &[u8]) -> bool {
    data.first_chunk::<4>() == Some(layout.magic)
}

/// Stops at the largest multiple there is, which is far more than can be allocated anyway.
fn round_up(value: u64, grain: u64) -> u64 {
    value.div_ceil(grain).saturating_mul(grain)
}

/// The size field, or what a compute allocation really wants. Those have their page size in the size
/// field, and say the same bigger number in the two others, which means all of the block. Video
/// buffers leave it empty, and say they are one row of so many bytes.
fn stated_size(layout: &AllocationLayout, data: &[u8]) -> u32 {
    let (size, whole) = (word(data, layout.size), word(data, layout.whole_size));
    let (width, total) = (word(data, layout.width), word(data, layout.total_size));
    if size != 0 && whole > size && whole == word(data, layout.asked_size) {
        whole
    } else if size == 0
        && word(data, layout.height) == 1
        && total == width
        && width == word(data, layout.pitch)
    {
        total
    } else {
        size
    }
}

/// The size of an allocation in video memory.
pub fn allocation_size(layout: &AllocationLayout, data: &[u8]) -> u64 {
    if !is_ours(layout, data) || data.len() < layout.size + 4 {
        return DEFAULT_SIZE;
    }
    let mut size = u64::from(stated_size(layout, data));
    let (depth, mips, layers) =
        (word(data, layout.depth), word(data, layout.depth + 4), word(data, layout.depth + 8));
    if size == 0 {
        // A block of a pool has its total in one field and what this request wants in another. The
        // driver takes the answer's size off and asks again.
        let (whole, asked) = (word(data, layout.whole_size), word(data, layout.asked_size));
        let limit = word(data, layout.size_limit);
        size = if asked != 0 && asked <= whole && mips == 0 && limit == 0 {
            u64::from(asked)
        } else {
            match word(data, layout.fallback_size) {
                0 => DEFAULT_SIZE,
                fallback => u64::from(fallback),
            }
        };
    }
    // The size field covers the first mip level only. The total covers them all, but buffers can
    // hold something else there, which is never whole pages.
    let total = u64::from(word(data, layout.total_size));
    if depth > 1 || mips > 1 || layers > 1 || total % SMALL_PAGE == 0 {
        size = size.max(total);
    }
    if data.len() >= layout.rows + 4 {
        // A tiled image fills whole blocks, whatever the size fields hold.
        let tiling = word(data, layout.tiling);
        let columns = u64::from(word(data, layout.columns));
        let rows = u64::from(word(data, layout.rows));
        if columns != 0 && tiling < TILING_LIMIT {
            let blocks = columns.saturating_mul(rows);
            size = size.max(blocks.saturating_mul(GOB_WIDTH * (GOB_HEIGHT << tiling)));
        }
    }
    round_up(size, if size >= BIG_PAGE { BIG_PAGE } else { SMALL_PAGE })
}

/// The size of an allocation in the guest's own memory, with no rounding.
pub fn exact_size(layout: &AllocationLayout, data: &[u8]) -> u64 {
    if is_ours(layout, data) && data.len() >= layout.size + 4 {
        match stated_size(layout, data) {
            0 => DEFAULT_SIZE,
            size => u64::from(size),
        }
    } else {
        DEFAULT_SIZE
    }
}

/// A surface's shape, if the data describes one.
pub fn surface_shape(layout: &AllocationLayout, data: &[u8]) -> Option<SurfaceShape> {
    if !is_ours(layout, data) || data.len() < layout.rows + 4 {
        return None;
    }
    let columns = word(data, layout.columns);
    let pitch = match word(data, layout.pitch) {
        // Some images leave the pitch empty, but a GOB is always the same width.
        0 => columns.wrapping_mul(GOB_WIDTH as u32),
        pitch => pitch,
    };
    Some(SurfaceShape {
        width: word(data, layout.width),
        height: word(data, layout.height),
        format: u32::from(data[layout.format]),
        pitch,
        tiling: (columns != 0).then(|| word(data, layout.tiling)),
        refresh: (word(data, layout.refresh), word(data, layout.refresh + 4)),
        samples: (word(data, layout.samples), word(data, layout.samples + 4)),
        colour: u32::from(data[layout.colour]),
    })
}

fn asks_gpu_cached(layout: &AllocationLayout, data: &[u8]) -> bool {
    is_ours(layout, data)
        && data.len() >= layout.attributes + 4
        && word(data, layout.attributes) >> CACHE_SHIFT & CACHE_MASK == CACHE_YES
}

/// Everything the core needs to know about an allocation.
pub fn shape(layout: &AllocationLayout, data: &[u8]) -> AllocationShape {
    AllocationShape {
        size: allocation_size(layout, data),
        exact_size: exact_size(layout, data),
        surface: surface_shape(layout, data),
        gpu_cached: asks_gpu_cached(layout, data),
    }
}

fn page_attributes(size: u32, in_video_memory: bool) -> (u32, u32) {
    if !in_video_memory {
        (PAGE_SIZE_4K | LOCATION_PCI, 0)
    } else if size >= HUGE_PAGE {
        (PAGE_SIZE_HUGE, ATTRIBUTE_2_HUGE_2M)
    } else if size >= BIG_PAGE_FROM {
        (PAGE_SIZE_BIG, 0)
    } else {
        (PAGE_SIZE_4K, 0)
    }
}

/// Whether this is a whole request, which says how long it is and is long enough to hold the size.
/// Anything shorter is some other kind of data, which keeps what it has.
fn is_whole_request(layout: &AllocationLayout, data: &[u8]) -> bool {
    data.len() >= layout.size + 4 && word(data, layout.own_length) as usize == data.len()
}

/// What the kernel side writes into a request's data. Data that says one mip level and has no size
/// to answer, or has an answer already, stays as it is, and so does anything but a whole request.
fn fill_in(layout: &AllocationLayout, data: &mut [u8]) {
    if !is_whole_request(layout, data) {
        return;
    }
    let size = word(data, layout.size);
    if data[layout.mip_levels] == 1 && (word(data, layout.whole_size) != 0 || size == 0) {
        return;
    }
    let in_video = data[layout.segments] & SEGMENT_VIDEO != 0;
    let (attributes, attributes_2) = page_attributes(size, in_video);
    let (width, height, pitch) =
        (word(data, layout.width), word(data, layout.height), word(data, layout.pitch));
    let attributes = attributes | u32::from(data[layout.depth_bits]);
    let attributes_2 = attributes_2 | word(data, layout.attributes);
    data[layout.mip_levels] = data[layout.mip_levels].max(1);
    data[layout.array_layers] = data[layout.array_layers].max(1);
    set_word(data, layout.answer_width, width);
    set_word(data, layout.answer_height, height);
    set_word(data, layout.answer_pitch, pitch);
    set_word(data, layout.whole_size, size);
    set_word(data, layout.size_limit, size.wrapping_sub(1));
    set_word(data, layout.answer_attributes, attributes);
    set_word(data, layout.answer_attributes_2, attributes_2);
}

/// Fills in the data, then puts the size this host really chose where the guest reads it.
pub fn finish(layout: &AllocationLayout, data: &mut [u8], size: u64) {
    fill_in(layout, data);
    if !is_ours(layout, data) || data.len() < layout.size_limit + 4 {
        return;
    }
    set_word(data, layout.whole_size, size as u32);
    if word(data, layout.size_limit) != 0 {
        set_word(data, layout.size_limit, (size as u32).wrapping_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_guest::blank_request;
    use crate::profile::GUEST_617;

    const LAYOUT: &AllocationLayout = &GUEST_617.allocation;

    fn data(size: u32) -> Vec<u8> {
        let mut bytes = blank_request();
        set_word(&mut bytes, LAYOUT.size, size);
        bytes
    }

    #[test]
    fn a_block_that_asks_for_all_of_itself_is_bigger_than_its_size_field() {
        let mut bytes = data(0x20_0000);
        set_word(&mut bytes, LAYOUT.whole_size, 0x100_0000);
        set_word(&mut bytes, LAYOUT.asked_size, 0x100_0000);
        assert_eq!(allocation_size(LAYOUT, &bytes), 0x100_0000);
        assert_eq!(exact_size(LAYOUT, &bytes), 0x100_0000);
    }

    #[test]
    fn a_buffer_of_one_row_is_as_long_as_the_row() {
        let mut bytes = data(0);
        set_word(&mut bytes, LAYOUT.width, 0x27_D800);
        set_word(&mut bytes, LAYOUT.height, 1);
        set_word(&mut bytes, LAYOUT.pitch, 0x27_D800);
        set_word(&mut bytes, LAYOUT.total_size, 0x27_D800);
        assert_eq!(allocation_size(LAYOUT, &bytes), 0x28_0000);
    }

    #[test]
    fn a_piece_of_a_block_keeps_its_size_field() {
        let mut bytes = data(0x20_0000);
        set_word(&mut bytes, LAYOUT.whole_size, 0x100_0000);
        set_word(&mut bytes, LAYOUT.asked_size, 0x8000);
        assert_eq!(allocation_size(LAYOUT, &bytes), 0x20_0000);
    }

    #[test]
    fn a_shape_too_large_to_count_comes_out_as_a_size_nothing_can_have() {
        let mut bytes = data(0x1000);
        set_word(&mut bytes, LAYOUT.columns, u32::MAX);
        set_word(&mut bytes, LAYOUT.rows, u32::MAX);
        set_word(&mut bytes, LAYOUT.tiling, TILING_LIMIT - 1);
        assert!(allocation_size(LAYOUT, &bytes) > 1 << 60);
    }

    #[test]
    fn other_data_gets_one_page() {
        assert_eq!(allocation_size(LAYOUT, b"something else"), DEFAULT_SIZE);
        assert_eq!(exact_size(LAYOUT, b"short"), DEFAULT_SIZE);
    }

    #[test]
    fn sizes_round_to_small_then_big_pages() {
        assert_eq!(allocation_size(LAYOUT, &data(0x1234)), 0x2000);
        assert_eq!(allocation_size(LAYOUT, &data(0x12345)), 0x20000);
    }

    #[test]
    fn the_exact_size_is_not_rounded() {
        assert_eq!(exact_size(LAYOUT, &data(0x12345)), 0x12345);
    }

    #[test]
    fn mip_levels_use_the_total() {
        let mut bytes = data(0x1000);
        set_word(&mut bytes, LAYOUT.depth + 4, 3);
        set_word(&mut bytes, LAYOUT.total_size, 0x5000);
        assert_eq!(allocation_size(LAYOUT, &bytes), 0x5000);
    }

    #[test]
    fn a_total_that_is_not_whole_pages_is_ignored_on_a_buffer() {
        let mut bytes = data(0x1000);
        set_word(&mut bytes, LAYOUT.total_size, 0x0123_4567);
        assert_eq!(allocation_size(LAYOUT, &bytes), 0x1000);
    }

    #[test]
    fn a_tiled_image_fills_whole_blocks() {
        let mut bytes = data(0x1000);
        set_word(&mut bytes, LAYOUT.total_size, 1);
        set_word(&mut bytes, LAYOUT.tiling, 1);
        set_word(&mut bytes, LAYOUT.columns, 4);
        set_word(&mut bytes, LAYOUT.rows, 2);
        assert_eq!(allocation_size(LAYOUT, &bytes), 4 * 64 * 2 * 16);
    }

    #[test]
    fn a_pool_request_uses_what_it_asked_for() {
        let mut bytes = data(0);
        set_word(&mut bytes, LAYOUT.whole_size, 0x10_0000);
        set_word(&mut bytes, LAYOUT.asked_size, 0x8000);
        assert_eq!(allocation_size(LAYOUT, &bytes), 0x8000);
    }

    #[test]
    fn a_surface_with_no_pitch_gets_one_from_its_columns() {
        let mut bytes = data(0x1000);
        set_word(&mut bytes, LAYOUT.columns, 10);
        set_word(&mut bytes, LAYOUT.tiling, 2);
        let surface = surface_shape(LAYOUT, &bytes).unwrap();
        assert_eq!((surface.pitch, surface.tiling), (640, Some(2)));
    }

    #[test]
    fn a_surface_with_no_columns_is_linear() {
        assert_eq!(surface_shape(LAYOUT, &data(0x1000)).unwrap().tiling, None);
    }

    #[test]
    fn the_cache_bits_are_a_yes_only_at_one() {
        let mut bytes = data(0x1000);
        assert!(!shape(LAYOUT, &bytes).gpu_cached);
        set_word(&mut bytes, LAYOUT.attributes, 1 << CACHE_SHIFT);
        assert!(shape(LAYOUT, &bytes).gpu_cached);
        set_word(&mut bytes, LAYOUT.attributes, 2 << CACHE_SHIFT);
        assert!(!shape(LAYOUT, &bytes).gpu_cached);
    }

    #[test]
    fn a_fresh_request_is_filled_in() {
        let mut bytes = data(0x20_0000);
        bytes[LAYOUT.segments] = SEGMENT_VIDEO;
        finish(LAYOUT, &mut bytes, 0x20_0000);
        assert_eq!(word(&bytes, LAYOUT.whole_size), 0x20_0000);
        assert_eq!(word(&bytes, LAYOUT.size_limit), 0x1F_FFFF);
        assert_eq!(word(&bytes, LAYOUT.answer_attributes), PAGE_SIZE_HUGE);
    }

    #[test]
    fn one_mip_level_with_no_size_stays_unfilled() {
        let mut bytes = data(0);
        bytes[LAYOUT.mip_levels] = 1;
        finish(LAYOUT, &mut bytes, 0x1000);
        assert_eq!(word(&bytes, LAYOUT.answer_attributes), 0);
        assert_eq!(word(&bytes, LAYOUT.whole_size), 0x1000, "the size still goes where the guest reads it");
    }

    #[test]
    fn a_request_that_misstates_its_length_is_not_filled_in() {
        let mut bytes = data(0x1000);
        let wrong = bytes.len() as u32 + 1;
        set_word(&mut bytes, LAYOUT.own_length, wrong);
        finish(LAYOUT, &mut bytes, 0x4000);
        assert_eq!(word(&bytes, LAYOUT.answer_attributes), 0);
    }

    #[test]
    fn a_short_request_that_states_its_length_is_not_filled_in() {
        let mut bytes = data(0x1000);
        bytes.truncate(LAYOUT.size);
        set_word(&mut bytes, LAYOUT.own_length, LAYOUT.size as u32);
        finish(LAYOUT, &mut bytes, 0x4000);
        assert_eq!(word(&bytes, LAYOUT.answer_attributes), 0);
    }

    #[test]
    fn data_of_another_size_only_gets_its_size() {
        let mut bytes = data(0x1000);
        bytes.truncate(0x100);
        finish(LAYOUT, &mut bytes, 0x4000);
        assert_eq!(word(&bytes, LAYOUT.whole_size), 0x4000);
        assert_eq!(word(&bytes, LAYOUT.answer_attributes), 0);
    }
}
