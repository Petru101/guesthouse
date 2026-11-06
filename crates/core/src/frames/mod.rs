//! The boundary between what makes the guest's frames and what shows them.
//!
//! A sink hears about rings of buffers and the frames in them, and knows nothing of how a frame came
//! to be. Copies to the staging surface are the one producer so far, for boot and install.

mod picture;
mod slots;

use guesthouse_backend::SurfaceShape;
pub use picture::Picture;
pub use slots::{NotTheViewers, Slots};

/// A set of buffers that frames take turns in. A new generation means the old buffers are gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRing {
    pub generation: u32,
    pub slots: u32,
    pub surface: SurfaceShape,
    /// Where the first slot is in the shared window.
    pub guest_address: u64,
}

/// A finished frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub generation: u32,
    pub slot: u32,
    pub sequence: u64,
}

/// See the module documentation.
pub trait FrameSink {
    fn ring_opened(&mut self, ring: &FrameRing);

    fn frame_ready(&mut self, frame: &Frame);

    fn ring_closed(&mut self, generation: u32);

    /// A slot is about to be written over while it may still be on show. Only a producer with a single
    /// slot has to do that.
    fn slot_overwriting(&mut self, _generation: u32, _slot: u32) {}
}

const STAGING_SLOT: u32 = 0;

/// Makes rings and frames out of copies to the staging surface, which is one buffer written in place.
#[derive(Debug, Default)]
pub struct StagingFrames {
    current: Option<FrameRing>,
    generation: u32,
    sequence: u64,
}

impl StagingFrames {
    /// A copy to this surface is about to start.
    pub fn copy_starting(&mut self, sink: &mut dyn FrameSink, surface: SurfaceShape, guest_address: u64) {
        let same = |ring: &FrameRing| ring.surface == surface && ring.guest_address == guest_address;
        if !self.current.as_ref().is_some_and(same) {
            if let Some(old) = self.current.take() {
                sink.ring_closed(old.generation);
            }
            self.generation += 1;
            let ring = FrameRing { generation: self.generation, slots: 1, surface, guest_address };
            sink.ring_opened(&ring);
            self.current = Some(ring);
        }
        sink.slot_overwriting(self.generation, STAGING_SLOT);
    }

    /// The copy is done.
    pub fn copy_finished(&mut self, sink: &mut dyn FrameSink) {
        self.sequence += 1;
        sink.frame_ready(&Frame { generation: self.generation, slot: STAGING_SLOT, sequence: self.sequence });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Log(Vec<String>);

    impl FrameSink for Log {
        fn ring_opened(&mut self, ring: &FrameRing) {
            self.0.push(format!("open {}", ring.generation));
        }
        fn frame_ready(&mut self, frame: &Frame) {
            self.0.push(format!("frame {} {}", frame.generation, frame.sequence));
        }
        fn ring_closed(&mut self, generation: u32) {
            self.0.push(format!("close {generation}"));
        }
        fn slot_overwriting(&mut self, generation: u32, _slot: u32) {
            self.0.push(format!("writing {generation}"));
        }
    }

    fn surface(width: u32) -> SurfaceShape {
        SurfaceShape {
            width,
            height: 480,
            format: 21,
            pitch: width * 4,
            tiling: None,
            refresh: (60, 1),
            samples: (1, 0),
            colour: 0,
        }
    }

    #[test]
    fn the_same_surface_stays_in_one_ring() {
        let (mut frames, mut sink) = (StagingFrames::default(), Log::default());
        for _ in 0..2 {
            frames.copy_starting(&mut sink, surface(640), 0x1000);
            frames.copy_finished(&mut sink);
        }
        assert_eq!(sink.0, ["open 1", "writing 1", "frame 1 1", "writing 1", "frame 1 2"]);
    }

    #[test]
    fn a_mode_change_closes_the_ring_and_opens_another() {
        let (mut frames, mut sink) = (StagingFrames::default(), Log::default());
        frames.copy_starting(&mut sink, surface(640), 0x1000);
        frames.copy_starting(&mut sink, surface(800), 0x1000);
        assert_eq!(sink.0, ["open 1", "writing 1", "close 1", "open 2", "writing 2"]);
    }
}
