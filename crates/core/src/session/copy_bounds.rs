//! Whether a copy stays inside the memory it names.
//!
//! A surface's width, height, pitch and tiling are only what its description says, and they aren't
//! tied to the size of its memory. So every copy on the daemon's channel is measured here in bytes,
//! against the allocation's real size, and these rules don't lean on the description being right.
//! The sums are done in 128 bits, where two of our 64-bit numbers can't wrap.

use guesthouse_backend::{Backend, COPY_PIXEL_BYTES, SurfaceShape};
use guesthouse_messages::queue::Rectangle;

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
    fn of(left: i32, top: i32, width: i64, count: i64) -> Option<Self> {
        Some(Self {
            x: u64::try_from(left).ok()? * u64::from(COPY_PIXEL_BYTES),
            y: u64::try_from(top).ok()?,
            width: u64::try_from(width).ok()? * u64::from(COPY_PIXEL_BYTES),
            count: u64::try_from(count).ok()?,
        })
    }

    /// The source's side of a copy.
    pub fn read_by(rectangle: &Rectangle) -> Option<Self> {
        let width = i64::from(rectangle.right) - i64::from(rectangle.left);
        let count = i64::from(rectangle.bottom) - i64::from(rectangle.top);
        Self::of(rectangle.left, rectangle.top, width, count)
    }

    /// The target's side: the same size, from this corner.
    pub fn written_by(rectangle: &Rectangle, left: i32, top: i32) -> Option<Self> {
        let width = i64::from(rectangle.right) - i64::from(rectangle.left);
        let count = i64::from(rectangle.bottom) - i64::from(rectangle.top);
        Self::of(left, top, width, count)
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
    fn inside_pixels(&self, surface: &SurfaceShape) -> bool {
        let inside = |end: u128, limit: u32| limit == 0 || end <= u128::from(limit);
        let pixel = u128::from(COPY_PIXEL_BYTES);
        inside(self.end_of_row().div_ceil(pixel), surface.width) && inside(self.end_of_rows(), surface.height)
    }
}

/// Whether these rows can be read from a surface in this much memory. A tiled surface has to fit
/// whole, as where one row lies in it is the backend's business, and the rows have to lie inside
/// what the copy engine will take to be the surface.
pub(super) fn read_fits<B: Backend>(backend: &B, rows: &Rows, surface: &SurfaceShape, memory: u64) -> bool {
    if surface.tiling.is_none() {
        return rows.fit_linear(surface.pitch, memory);
    }
    rows.end_of_row() <= u128::from(surface.pitch)
        && rows.end_of_rows() <= u128::from(surface.height)
        && backend.tiled_bytes(surface, rows.x, rows.y).is_some_and(|bytes| bytes <= memory)
}

/// Whether a copy of a rectangle stays inside both surfaces and their memory. The target's tiling
/// plays no part, as a copy writes it row by row whatever it says.
pub(super) fn copy_fits<B: Backend>(
    backend: &B,
    read: &Rows,
    (source, source_memory): (&SurfaceShape, u64),
    written: &Rows,
    (target, target_memory): (&SurfaceShape, u64),
) -> bool {
    read.inside_pixels(source)
        && written.inside_pixels(target)
        && read_fits(backend, read, source, source_memory)
        && written.fit_linear(target.pitch, target_memory)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(Rows::read_by(&rectangle(10, 0, 5, 5)), None);
        assert_eq!(Rows::read_by(&rectangle(0, 10, 5, 5)), None);
        assert_eq!(Rows::read_by(&rectangle(-1, 0, 5, 5)), None);
        assert_eq!(Rows::read_by(&rectangle(0, -1, 5, 5)), None);
        assert_eq!(Rows::written_by(&rectangle(0, 0, 5, 5), -1, 0), None);
        assert_eq!(Rows::written_by(&rectangle(0, 0, 5, 5), 0, -1), None);
    }

    #[test]
    fn the_widest_rectangle_there_is_comes_out_whole() {
        let rectangle = Rectangle { left: 0, top: 0, right: i32::MAX, bottom: i32::MAX };
        let rows = Rows::read_by(&rectangle).unwrap();
        assert_eq!(rows.width, i32::MAX as u64 * 4, "wider than a u32, and not cut down to fit one");
        assert!(!rows.fit_linear(u32::MAX, u64::MAX));
    }

    #[test]
    fn a_stated_size_still_limits_a_copy_in_pixels() {
        let inside = rows(0, 0, 4 * 100, 100);
        assert!(inside.inside_pixels(&surface(100, 100, 512)));
        assert!(!inside.inside_pixels(&surface(99, 100, 512)));
        assert!(!inside.inside_pixels(&surface(100, 99, 512)));
    }

    #[test]
    fn a_size_of_zero_leaves_only_the_bytes_to_decide() {
        let rows = rows(0, 0, 400, 100);
        assert!(rows.inside_pixels(&surface(0, 0, 512)));
        assert!(rows.fit_linear(512, 512 * 100));
        assert!(!rows.fit_linear(512, 512 * 99));
    }
}
