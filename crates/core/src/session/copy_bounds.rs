//! Whether a copy stays inside the memory it names.
//!
//! A surface's width, height, pitch and tiling are only what its description says, and they aren't
//! tied to the size of its memory. So every copy on the daemon's channel is measured here in bytes,
//! against the allocation's real size, and these rules don't lean on the description being right.
//! The sums are done in 128 bits, where two of our 64-bit numbers can't wrap.

use crate::frames::Picture;
use guesthouse_backend::{Backend, COPY_PIXEL_BYTES, SurfaceShape};
use guesthouse_messages::queue::Rectangle;
use log::Level;

/// How many refused copies the log gets as warnings. A guest can send these as fast as it likes, and
/// the rest still go to the debug log and are still refused.
const REFUSALS_TO_WARN_OF: u64 = 16;

/// How to log the refusal that makes this many.
pub(super) fn refusal_level(count: u64) -> Level {
    if count <= REFUSALS_TO_WARN_OF { Level::Warn } else { Level::Debug }
}

/// Some rows of a surface, in bytes across and rows down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rows {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub count: u64,
}

impl Rows {
    /// `None` if the rectangle is inside out or starts before the surface does.
    fn of(left: i32, top: i32, width: i64, count: i64, pixel: u32) -> Option<Self> {
        Some(Self {
            x: u64::try_from(left).ok()?.checked_mul(u64::from(pixel))?,
            y: u64::try_from(top).ok()?,
            width: u64::try_from(width).ok()?.checked_mul(u64::from(pixel))?,
            count: u64::try_from(count).ok()?,
        })
    }

    /// The source's side of a copy, in pixels of this many bytes.
    pub fn read_by(rectangle: &Rectangle, pixel: u32) -> Option<Self> {
        let width = i64::from(rectangle.right) - i64::from(rectangle.left);
        let count = i64::from(rectangle.bottom) - i64::from(rectangle.top);
        Self::of(rectangle.left, rectangle.top, width, count, pixel)
    }

    /// The target's side: the same size, from this corner, in the same pixels as the source's.
    pub fn written_by(rectangle: &Rectangle, left: i32, top: i32, pixel: u32) -> Option<Self> {
        let width = i64::from(rectangle.right) - i64::from(rectangle.left);
        let count = i64::from(rectangle.bottom) - i64::from(rectangle.top);
        Self::of(left, top, width, count, pixel)
    }

    /// A whole picture, from the top left corner.
    pub fn whole(width_in_pixels: u32, count: u32) -> Self {
        let width = u64::from(width_in_pixels) * u64::from(COPY_PIXEL_BYTES);
        Self { x: 0, y: 0, width, count: u64::from(count) }
    }

    fn end_of_row(&self) -> u128 {
        u128::from(self.x) + u128::from(self.width)
    }

    fn end_of_rows(&self) -> u128 {
        u128::from(self.y) + u128::from(self.count)
    }

    /// A row must end before the next one starts, and the last row must end inside the memory.
    pub fn fit_linear(&self, pitch: u32, memory: u64) -> bool {
        let pitch = u128::from(pitch);
        let last_row = self.end_of_rows().saturating_sub(1).max(u128::from(self.y));
        self.end_of_row() <= pitch && last_row * pitch + self.end_of_row() <= u128::from(memory)
    }

    /// Whether a width and height in pixels hold these rows. Zero says nothing, as some surfaces
    /// don't state them, and then the bytes alone decide.
    fn inside_pixels(&self, surface: &SurfaceShape, pixel: u32) -> bool {
        let inside = |end: u128, limit: u32| limit == 0 || end <= u128::from(limit);
        inside(self.end_of_row().div_ceil(u128::from(pixel)), surface.width)
            && inside(self.end_of_rows(), surface.height)
    }
}

/// Whether these rows can be read from or written to a surface in this much memory. A tiled surface
/// has to fit whole, as where one row lies in it is the backend's business, and the rows have to lie
/// inside what the copy engine will take to be the surface.
pub(super) fn side_fits<B: Backend>(backend: &B, rows: &Rows, surface: &SurfaceShape, memory: u64) -> bool {
    if surface.tiling.is_none() {
        return rows.fit_linear(surface.pitch, memory);
    }
    rows.end_of_row() <= u128::from(surface.pitch)
        && rows.end_of_rows() <= u128::from(surface.height)
        && backend.tiled_bytes(surface, rows.x, rows.y).is_some_and(|bytes| bytes <= memory)
}

/// Whether a whole picture can be read from a surface in this much memory, and written to a slot.
pub(super) fn picture_fits<B: Backend>(
    backend: &B,
    picture: &Picture,
    (surface, memory): (&SurfaceShape, u64),
    slot_memory: u64,
) -> bool {
    let rows = Rows::whole(picture.width, picture.height);
    side_fits(backend, &rows, surface, memory) && rows.fit_linear(picture.pitch, slot_memory)
}

/// Whether a copy of a rectangle stays inside both surfaces and their memory. Both sides are held to
/// the same rule, whatever their tiling, and `read` and `written` count the same bytes a pixel, as a
/// copy moves bytes and doesn't change them.
pub(super) fn copy_fits<B: Backend>(
    backend: &B,
    pixel: u32,
    read: &Rows,
    (source, source_memory): (&SurfaceShape, u64),
    written: &Rows,
    (target, target_memory): (&SurfaceShape, u64),
) -> bool {
    read.inside_pixels(source, pixel)
        && written.inside_pixels(target, pixel)
        && side_fits(backend, read, source, source_memory)
        && side_fits(backend, written, target, target_memory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend_standin::StandIn;

    fn surface(width: u32, height: u32, pitch: u32) -> SurfaceShape {
        SurfaceShape {
            width,
            height,
            format: 21,
            pitch,
            tiling: None,
            refresh: (60, 1),
            samples: (1, 0),
            colour: 0,
        }
    }

    fn rows(x: u64, y: u64, width: u64, count: u64) -> Rows {
        Rows { x, y, width, count }
    }

    #[test]
    fn only_the_first_refusals_are_warnings() {
        assert_eq!(refusal_level(1), Level::Warn);
        assert_eq!(refusal_level(REFUSALS_TO_WARN_OF), Level::Warn);
        assert_eq!(refusal_level(REFUSALS_TO_WARN_OF + 1), Level::Debug);
    }

    #[test]
    fn the_last_byte_may_touch_the_end_of_the_memory_and_no_more() {
        let rows = rows(0, 0, 4096, 768);
        assert!(rows.fit_linear(4096, 4096 * 768));
        assert!(!rows.fit_linear(4096, 4096 * 768 - 1));
    }

    #[test]
    fn the_last_row_needs_only_its_own_bytes() {
        let tail = 4096 * 767 + 400;
        assert!(rows(0, 0, 400, 768).fit_linear(4096, tail));
        assert!(!rows(0, 0, 401, 768).fit_linear(4096, tail));
    }

    #[test]
    fn a_row_ends_before_its_pitch() {
        assert!(rows(96, 0, 160, 2).fit_linear(256, 1 << 20));
        assert!(!rows(100, 0, 160, 2).fit_linear(256, 1 << 20));
        assert!(!rows(0, 0, 4, 1).fit_linear(0, 1 << 20), "no pitch holds no row");
    }

    #[test]
    fn sums_near_the_top_of_64_bits_do_not_wrap() {
        let top = u64::MAX;
        assert!(!rows(top, top, top, top).fit_linear(u32::MAX, u64::MAX));
        assert!(!rows(0, top, 4, 2).fit_linear(u32::MAX, u64::MAX));
        assert!(!rows(0, 1 << 33, 4, 1 << 33).fit_linear(1 << 31, u64::MAX));
    }

    #[test]
    fn an_empty_rectangle_still_starts_inside() {
        assert!(rows(0, 0, 0, 0).fit_linear(256, 256));
        assert!(!rows(0, 2, 0, 0).fit_linear(256, 256));
        assert!(!rows(512, 0, 0, 1).fit_linear(256, 1 << 20));
    }

    #[test]
    fn a_rectangle_inside_out_or_before_the_surface_has_no_rows() {
        let rectangle = |left, top, right, bottom| Rectangle { left, top, right, bottom };
        assert_eq!(Rows::read_by(&rectangle(10, 0, 5, 5), 4), None);
        assert_eq!(Rows::read_by(&rectangle(0, 10, 5, 5), 4), None);
        assert_eq!(Rows::read_by(&rectangle(-1, 0, 5, 5), 4), None);
        assert_eq!(Rows::read_by(&rectangle(0, -1, 5, 5), 4), None);
        assert_eq!(Rows::written_by(&rectangle(0, 0, 5, 5), -1, 0, 4), None);
        assert_eq!(Rows::written_by(&rectangle(0, 0, 5, 5), 0, -1, 4), None);
    }

    #[test]
    fn the_widest_rectangle_there_is_comes_out_whole() {
        let rectangle = Rectangle { left: 0, top: 0, right: i32::MAX, bottom: i32::MAX };
        let rows = Rows::read_by(&rectangle, 4).unwrap();
        assert_eq!(rows.width, i32::MAX as u64 * 4, "wider than a u32, and not cut down to fit one");
        assert!(!rows.fit_linear(u32::MAX, u64::MAX));
    }

    #[test]
    fn a_surface_must_really_hold_a_picture() {
        let backend = StandIn::new();
        let picture = Picture::sized(1024, 768).unwrap();
        let exact = 4096 * 768;
        let fits =
            |surface: &SurfaceShape, memory| picture_fits(&backend, &picture, (surface, memory), exact);
        assert!(fits(&surface(1024, 768, 4096), exact));
        assert!(!fits(&surface(1024, 768, 4096), exact - 1));
        assert!(!fits(&surface(1024, 768, 4092), exact), "rows would overlap");
        assert!(!fits(&surface(1024, 768, u32::MAX), exact), "one row of this pitch is past the memory");
    }

    #[test]
    fn a_slot_must_really_hold_a_picture() {
        let backend = StandIn::new();
        let picture = Picture::sized(1024, 768).unwrap();
        let exact = 4096 * 768;
        assert!(!picture_fits(&backend, &picture, (&surface(1024, 768, 4096), exact), exact - 1));
    }

    #[test]
    fn a_tiled_surface_must_hold_a_picture_to_the_end_of_its_last_block() {
        let backend = StandIn::new();
        let picture = Picture::sized(1024, 760).unwrap();
        let tiled = SurfaceShape { tiling: Some(4), ..surface(1024, 760, 4096) };
        let (rows, slot) = (4096 * 768, picture.bytes());
        assert!(picture_fits(&backend, &picture, (&tiled, rows), slot));
        assert!(!picture_fits(&backend, &picture, (&tiled, rows - 1), slot));
        assert!(!picture_fits(&backend, &picture, (&tiled, picture.bytes()), slot));
    }

    const PIXEL: u32 = 4;
    const WIDE_PIXEL: u32 = 8;

    fn tiled(width: u32, height: u32, pitch: u32) -> SurfaceShape {
        SurfaceShape { tiling: Some(4), ..surface(width, height, pitch) }
    }

    fn rectangle(left: i32, top: i32, right: i32, bottom: i32) -> Rectangle {
        Rectangle { left, top, right, bottom }
    }

    /// A copy of this rectangle from a surface in plenty of memory, to this corner of a target.
    fn copies_to(
        target: &SurfaceShape,
        memory: u64,
        area: Rectangle,
        corner: (i32, i32),
        pixel: u32,
    ) -> bool {
        let backend = StandIn::new();
        let source = tiled(0, 0x7FFF, 0x7FFF);
        let read = Rows::read_by(&area, pixel).unwrap();
        let written = Rows::written_by(&area, corner.0, corner.1, pixel).unwrap();
        copy_fits(&backend, pixel, &read, (&source, u64::MAX), &written, (target, memory))
    }

    #[test]
    fn a_tiled_target_must_really_hold_its_blocks() {
        let target = tiled(1920, 1080, 7680);
        let blocks = 7680 * 1152;
        let whole = rectangle(0, 0, 1920, 1080);
        assert!(copies_to(&target, blocks, whole, (0, 0), PIXEL));
        assert!(!copies_to(&target, blocks - 1, whole, (0, 0), PIXEL));
        assert!(
            !copies_to(&target, 7680 * 1080, whole, (0, 0), PIXEL),
            "rows alone are short of whole blocks"
        );
    }

    #[test]
    fn a_rectangle_stays_inside_a_tiled_target() {
        let target = tiled(1920, 1080, 7680);
        let plenty = 1 << 30;
        assert!(copies_to(&target, plenty, rectangle(0, 0, 100, 100), (1820, 980), PIXEL));
        assert!(!copies_to(&target, plenty, rectangle(0, 0, 100, 100), (1821, 980), PIXEL));
        assert!(!copies_to(&target, plenty, rectangle(0, 0, 100, 100), (1820, 981), PIXEL));
    }

    #[test]
    fn a_tiled_target_with_no_stated_size_is_held_to_its_pitch_and_height() {
        let target = SurfaceShape { width: 0, ..tiled(0, 1080, 7680) };
        let plenty = 1 << 30;
        assert!(copies_to(&target, plenty, rectangle(0, 0, 1920, 1080), (0, 0), PIXEL));
        assert!(!copies_to(&target, plenty, rectangle(0, 0, 1921, 1080), (0, 0), PIXEL));
        assert!(!copies_to(&target, plenty, rectangle(0, 0, 1920, 1081), (0, 0), PIXEL));
    }

    #[test]
    fn a_tiled_target_past_the_engines_fields_is_refused() {
        let plenty = u64::MAX;
        let bad_block = SurfaceShape { tiling: Some(6), ..tiled(1920, 1080, 7680) };
        assert!(!copies_to(&bad_block, plenty, rectangle(0, 0, 10, 10), (0, 0), PIXEL));
        let wild_block = SurfaceShape { tiling: Some(u32::MAX), ..tiled(1920, 1080, 7680) };
        assert!(!copies_to(&wild_block, plenty, rectangle(0, 0, 10, 10), (0, 0), PIXEL));
        let far = tiled(0, 0x2_0000, 0x10_0000);
        assert!(copies_to(&far, plenty, rectangle(0, 0, 1, 1), (0x3FFF, 0xFFFF), PIXEL));
        assert!(!copies_to(&far, plenty, rectangle(0, 0, 1, 1), (0x4000, 0), PIXEL), "x is in bytes");
        assert!(!copies_to(&far, plenty, rectangle(0, 0, 1, 1), (0, 0x1_0000), PIXEL));
    }

    #[test]
    fn wide_pixels_reach_the_engines_limit_in_half_the_pixels() {
        let far = tiled(0, 8, 0x10_0000);
        assert!(copies_to(&far, u64::MAX, rectangle(0, 0, 1, 1), (0x1FFF, 0), WIDE_PIXEL));
        assert!(!copies_to(&far, u64::MAX, rectangle(0, 0, 1, 1), (0x2000, 0), WIDE_PIXEL));
    }

    #[test]
    fn a_wide_source_cant_overrun_a_narrow_target() {
        // Rows of 8W bytes are wider than a target of pitch 4W.
        let width = 2560;
        let target = surface(width, 1440, width * PIXEL);
        let exact = u64::from(width * PIXEL) * 1440;
        let whole = rectangle(0, 0, width as i32, 1440);
        assert!(copies_to(&target, exact, whole, (0, 0), PIXEL));
        assert!(!copies_to(&target, exact, whole, (0, 0), WIDE_PIXEL));
        let tiled_target = tiled(width, 1440, width * PIXEL);
        assert!(!copies_to(&tiled_target, u64::MAX, whole, (0, 0), WIDE_PIXEL));
    }

    #[test]
    fn a_colour_nobody_knows_is_held_to_four_bytes_a_pixel() {
        let backend = StandIn::new();
        let unknown = SurfaceShape { colour: 0xEE, ..surface(100, 100, 1 << 20) };
        assert_eq!(backend.copy_pixel_bytes(&unknown), PIXEL);
        let wide = SurfaceShape { colour: guesthouse_backend_standin::WIDE_COLOUR, ..unknown };
        assert_eq!(backend.copy_pixel_bytes(&wide), WIDE_PIXEL);
        let area = rectangle(0, 0, 100, 1);
        let read = Rows::read_by(&area, PIXEL).unwrap();
        assert_eq!(read.width, 400, "the copy moves four bytes a pixel too, so this is what it can touch");
    }

    #[test]
    fn a_stated_size_still_limits_a_copy_in_pixels() {
        let inside = rows(0, 0, 4 * 100, 100);
        assert!(inside.inside_pixels(&surface(100, 100, 512), 4));
        assert!(!inside.inside_pixels(&surface(99, 100, 512), 4));
        assert!(!inside.inside_pixels(&surface(100, 99, 512), 4));
    }

    #[test]
    fn a_size_of_zero_leaves_only_the_bytes_to_decide() {
        let rows = rows(0, 0, 400, 100);
        assert!(rows.inside_pixels(&surface(0, 0, 512), 4));
        assert!(rows.fit_linear(512, 512 * 100));
        assert!(!rows.fit_linear(512, 512 * 99));
    }
}
