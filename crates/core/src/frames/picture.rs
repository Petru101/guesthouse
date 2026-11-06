//! The shape of a picture in the frame ring, worked out from a surface the guest described.

use guesthouse_backend::{COPY_PIXEL_BYTES, SurfaceShape};
use guesthouse_messages::wire::align_up;

/// What a consumer on the GPU expects of a row's start.
const PITCH_ALIGN: u32 = 256;
/// Past 8K on a side. The size comes from the guest, and each slot of the ring is this big in the
/// host's video memory.
const MOST_SIDE: u32 = 8192;

/// A picture's size, and its pitch in the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub pitch: u32,
}

impl Picture {
    /// The picture for a screen of this shape, if it's a size the ring can hold.
    pub fn of(surface: &SurfaceShape) -> Option<Self> {
        Self::sized(surface.width, surface.height)
    }

    /// The picture of this size, if it's one the ring can hold.
    pub fn sized(width: u32, height: u32) -> Option<Self> {
        if !(1..=MOST_SIDE).contains(&width) || !(1..=MOST_SIDE).contains(&height) {
            return None;
        }
        let row = u64::from(width * COPY_PIXEL_BYTES);
        Some(Self { width, height, pitch: align_up(row, u64::from(PITCH_ALIGN)) as u32 })
    }

    /// How many bytes the rows take.
    pub fn bytes(&self) -> u64 {
        u64::from(self.pitch) * u64::from(self.height)
    }
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

    #[test]
    fn rows_start_on_the_alignment_consumers_expect() {
        let picture = Picture::of(&surface(1366, 768, 0)).unwrap();
        assert_eq!(picture.pitch, 5632);
        assert_eq!(picture.bytes(), 5632 * 768);
    }

    #[test]
    fn a_size_no_screen_has_gets_no_picture() {
        for (width, height) in [(0, 768), (1024, 0), (8193, 768), (1024, 8193), (u32::MAX, u32::MAX)] {
            assert_eq!(Picture::of(&surface(width, height, 0)), None, "{width} by {height}");
        }
        assert!(Picture::of(&surface(7680, 4320, 0)).is_some());
    }
}
