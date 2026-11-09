//! Copies a finished frame into a staging surface with the CPU.
//!
//! Both surfaces are rows in order here. The source is DWM's surface, made on the card; the target
//! is the guest's staging surface. A frame is a few megabytes, which the CPU moves in well under a
//! frame's time, and it needs no queue of the daemon's own on the card.

use guesthouse_backend::{SlotCopy, StagingCopy, SurfaceShape};

/// The bytes of one pixel of every format this copy takes.
pub const PIXEL_BYTES: u64 = 4;

/// Where a copy reads and writes, as byte ranges of two surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyPlan {
    /// Of the first byte read, from the start of the source.
    pub source_start: u64,
    /// Of the first byte written, from the start of the target.
    pub target_start: u64,
    pub source_pitch: u64,
    pub target_pitch: u64,
    pub row_bytes: u64,
    pub rows: u64,
}

impl CopyPlan {
    /// The plan for a copy, or `None` for a tiled surface or a rectangle past either surface.
    pub fn of(copy: &StagingCopy) -> Option<Self> {
        if copy.source.tiling.is_some() || copy.target.tiling.is_some() {
            return None;
        }
        let width = u64::try_from(copy.right.checked_sub(copy.left)?).ok()?;
        let rows = u64::try_from(copy.bottom.checked_sub(copy.top)?).ok()?;
        let (left, top) = (u64::try_from(copy.left).ok()?, u64::try_from(copy.top).ok()?);
        let (target_left, target_top) =
            (u64::try_from(copy.target_left).ok()?, u64::try_from(copy.target_top).ok()?);
        let fits = |shape: &SurfaceShape, x: u64, y: u64| {
            x + width <= u64::from(shape.width)
                && y + rows <= u64::from(shape.height)
                && u64::from(shape.pitch) >= u64::from(shape.width) * PIXEL_BYTES
        };
        if !fits(&copy.source, left, top) || !fits(&copy.target, target_left, target_top) {
            return None;
        }
        let source_pitch = u64::from(copy.source.pitch);
        let target_pitch = u64::from(copy.target.pitch);
        Some(Self {
            source_start: top * source_pitch + left * PIXEL_BYTES,
            target_start: target_top * target_pitch + target_left * PIXEL_BYTES,
            source_pitch,
            target_pitch,
            row_bytes: width * PIXEL_BYTES,
            rows,
        })
    }

    /// How far into the source the copy reads.
    pub fn source_end(&self) -> u64 {
        self.end(self.source_start, self.source_pitch)
    }

    /// How far into the target the copy writes.
    pub fn target_end(&self) -> u64 {
        self.end(self.target_start, self.target_pitch)
    }

    fn end(&self, start: u64, pitch: u64) -> u64 {
        if self.rows == 0 { start } else { start + (self.rows - 1) * pitch + self.row_bytes }
    }

    /// Moves the rows.
    ///
    /// # Safety
    /// `source` must be readable for [`Self::source_end`] bytes and `target` writable for
    /// [`Self::target_end`] bytes, and the two must not overlap.
    pub unsafe fn run(&self, source: *const u8, target: *mut u8) {
        for row in 0..self.rows {
            let from = (self.source_start + row * self.source_pitch) as usize;
            let to = (self.target_start + row * self.target_pitch) as usize;
            // SAFETY: the caller vouches for both ranges, which the plan keeps inside.
            unsafe {
                std::ptr::copy_nonoverlapping(source.add(from), target.add(to), self.row_bytes as usize)
            };
        }
    }
}

/// A copy of a whole picture into a slot of the frame ring, as a copy to a surface of the slot's
/// shape, so that it goes the way the screen's copies do: by the CPU from rows in order, and by the
/// DMA engine from tiles. `None` for numbers that don't fit an `i32`.
pub fn slot_as_staging(copy: &SlotCopy) -> Option<StagingCopy> {
    let target = SurfaceShape {
        width: copy.width,
        height: copy.height,
        pitch: copy.slot_pitch,
        tiling: None,
        ..copy.source
    };
    Some(StagingCopy {
        source: copy.source,
        target,
        source_address: copy.source_address,
        target_address: copy.slot_address,
        left: 0,
        top: 0,
        right: i32::try_from(copy.width).ok()?,
        bottom: i32::try_from(copy.height).ok()?,
        target_left: 0,
        target_top: 0,
        target_guest_address: None,
        is_screen: false,
        own_space: copy.own_space,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::SpaceId;

    fn shape(width: u32, height: u32, pitch: u32) -> SurfaceShape {
        SurfaceShape {
            width,
            height,
            format: 21,
            pitch,
            tiling: None,
            refresh: (60, 1),
            samples: (1, 0),
            colour: 21,
        }
    }

    fn copy(source: SurfaceShape, target: SurfaceShape, rect: [i32; 4], to: (i32, i32)) -> StagingCopy {
        StagingCopy {
            source,
            target,
            source_address: 0,
            target_address: 0,
            left: rect[0],
            top: rect[1],
            right: rect[2],
            bottom: rect[3],
            target_left: to.0,
            target_top: to.1,
            target_guest_address: None,
            is_screen: true,
            own_space: SpaceId(1),
        }
    }

    #[test]
    fn a_part_of_a_frame_lands_at_its_place_with_each_surface_s_pitch() {
        let (width, height) = (4u32, 3u32);
        let source: Vec<u8> = (0..16 * height).map(|byte| byte as u8).collect();
        let mut target = vec![0u8; 20 * height as usize];
        let request = copy(shape(width, height, 16), shape(width, height, 20), [1, 1, 3, 3], (0, 0));
        let plan = CopyPlan::of(&request).unwrap();
        assert!(plan.source_end() <= source.len() as u64 && plan.target_end() <= target.len() as u64);
        // SAFETY: both ends were checked against the buffers just above.
        unsafe { plan.run(source.as_ptr(), target.as_mut_ptr()) };
        assert_eq!(&target[0..8], &source[20..28]);
        assert_eq!(&target[20..28], &source[36..44]);
        assert!(target[8..20].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_rectangle_past_the_target_or_a_tiled_surface_has_no_plan() {
        let frame = shape(1920, 1080, 7680);
        assert!(CopyPlan::of(&copy(frame, shape(800, 600, 3200), [0, 0, 1920, 1080], (0, 0))).is_none());
        let tiled = SurfaceShape { tiling: Some(9), ..frame };
        assert!(CopyPlan::of(&copy(tiled, frame, [0, 0, 1920, 1080], (0, 0))).is_none());
        assert!(CopyPlan::of(&copy(frame, frame, [10, 0, 5, 1080], (0, 0))).is_none());
    }

    #[test]
    fn a_whole_screen_reads_and_writes_exactly_its_rows() {
        let frame = shape(1920, 1080, 7680);
        let plan = CopyPlan::of(&copy(frame, frame, [0, 0, 1920, 1080], (0, 0))).unwrap();
        assert_eq!((plan.source_end(), plan.target_end()), (7680 * 1080, 7680 * 1080));
    }

    fn slot(source: SurfaceShape, width: u32, height: u32, slot_pitch: u32) -> SlotCopy {
        SlotCopy {
            source,
            source_address: 0x1000_0000,
            slot_address: 0x2000_0000,
            slot_pitch,
            width,
            height,
            own_space: SpaceId(1),
        }
    }

    #[test]
    fn a_slot_copy_of_rows_in_order_moves_the_whole_picture() {
        let staging = slot_as_staging(&slot(shape(1920, 1080, 7680), 1920, 1080, 7936)).unwrap();
        assert_eq!((staging.source_address, staging.target_address), (0x1000_0000, 0x2000_0000));
        let plan = CopyPlan::of(&staging).unwrap();
        assert_eq!(
            (plan.source_pitch, plan.target_pitch, plan.row_bytes, plan.rows),
            (7680, 7936, 7680, 1080)
        );
        assert_eq!(plan.target_end(), 1079 * 7936 + 7680);
    }

    #[test]
    fn a_slot_bigger_than_its_source_is_refused() {
        let staging = slot_as_staging(&slot(shape(1280, 720, 5120), 1920, 1080, 7680)).unwrap();
        assert_eq!(CopyPlan::of(&staging), None);
    }

    #[test]
    fn a_tiled_source_goes_to_the_dma_engine() {
        let tiled = SurfaceShape { tiling: Some(27), ..shape(1920, 1080, 1920) };
        let staging = slot_as_staging(&slot(tiled, 1920, 1080, 7680)).unwrap();
        assert_eq!(CopyPlan::of(&staging), None);
        let plan = crate::sdma_copy::TiledCopy::of(&staging).unwrap();
        assert_eq!(
            (plan.linear_address, plan.linear_pitch, plan.width, plan.height),
            (0x2000_0000, 1920, 1920, 1080)
        );
    }
}
