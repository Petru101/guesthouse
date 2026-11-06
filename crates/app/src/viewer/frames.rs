//! Where the viewer's pictures come from. The real frame path is being redesigned, so the viewer only
//! knows this interface, and for now a pattern stands in for the guest's screen.
//!
//! There will be two origins. QEMU's D-Bus display has the firmware, an install and any machine with no
//! GPU, and a machine with a GPU has it too until its first frame from the GPU, and again after a reset.

use guesthouse_app_core::vm_args::DISPLAY_SOCKET;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BYTES_PER_PIXEL: usize = 4;
const STAND_IN_SIZE: (usize, usize) = (640, 360);
const STAND_IN_INTERVAL: Duration = Duration::from_millis(33);
const BAND_WIDTH: usize = 32;
const STEP_PIXELS: usize = 4;

/// One picture, as blue, green, red and an unused byte for each pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn stride(&self) -> usize {
        self.width * BYTES_PER_PIXEL
    }

    /// A smaller copy of this width, for a thumbnail. Every pixel is picked, not blended: it only has
    /// to say which machine this is.
    pub fn shrunk(&self, width: usize) -> Self {
        let width = width.min(self.width).max(1);
        let height = (self.height * width / self.width).max(1);
        let mut data = Vec::with_capacity(width * height * BYTES_PER_PIXEL);
        for y in 0..height {
            let row = y * self.height / height * self.stride();
            for x in 0..width {
                let at = row + x * self.width / width * BYTES_PER_PIXEL;
                data.extend_from_slice(&self.data[at..at + BYTES_PER_PIXEL]);
            }
        }
        Self { width, height, data }
    }
}

pub trait FrameSource {
    /// The newest frame if there's a new one. A still screen has none to give.
    fn newest(&mut self) -> Option<Frame>;
}

/// Where a machine's frames can come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameOrigin {
    /// The monitor that gives the viewer QEMU's D-Bus display.
    pub display_socket: PathBuf,
    /// The guest has a GPU, whose frames take over from QEMU's once they come.
    pub gpu_frames: bool,
}

impl FrameOrigin {
    pub fn new(run_folder: &Path, gpu_frames: bool) -> Self {
        Self { display_socket: run_folder.join(DISPLAY_SOCKET), gpu_frames }
    }
}

/// The source for this origin. Only the stand-in exists until the frame path is settled.
pub fn frame_source(_origin: &FrameOrigin) -> Box<dyn FrameSource> {
    Box::new(StandInFrames::new())
}

/// A band moving across a dark screen, so that it's plain whether frames are arriving.
pub struct StandInFrames {
    shown: usize,
    last: Option<Instant>,
}

impl StandInFrames {
    pub fn new() -> Self {
        Self { shown: 0, last: None }
    }

    fn draw(&self) -> Frame {
        let (width, height) = STAND_IN_SIZE;
        let band = self.shown * STEP_PIXELS % width;
        let mut data = Vec::with_capacity(width * height * BYTES_PER_PIXEL);
        for _ in 0..height {
            for x in 0..width {
                let lit = x >= band && x < band + BAND_WIDTH;
                data.extend_from_slice(if lit { &[0xE0, 0xA0, 0x40, 0] } else { &[0x30, 0x20, 0x20, 0] });
            }
        }
        Frame { width, height, data }
    }
}

impl Default for StandInFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSource for StandInFrames {
    fn newest(&mut self) -> Option<Frame> {
        if self.last.is_some_and(|last| last.elapsed() < STAND_IN_INTERVAL) {
            return None;
        }
        self.last = Some(Instant::now());
        self.shown += 1;
        Some(self.draw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_is_found_in_the_machines_run_folder() {
        let origin = FrameOrigin::new(Path::new("/run/x"), false);
        assert_eq!(origin.display_socket, Path::new("/run/x/display.sock"));
    }

    #[test]
    fn a_stand_in_frame_is_whole_and_keeps_to_its_pace() {
        let mut source = StandInFrames::new();
        let frame = source.newest().unwrap();
        assert_eq!(frame.data.len(), frame.stride() * frame.height);
        assert_eq!(source.newest(), None, "the next one isn't due yet");
    }

    #[test]
    fn the_picture_moves_from_frame_to_frame() {
        let mut source = StandInFrames::new();
        let first = source.draw();
        source.shown += 1;
        assert_ne!(first, source.draw());
    }

    #[test]
    fn a_thumbnail_keeps_the_shape_of_the_picture() {
        let frame = StandInFrames::new().draw();
        let small = frame.shrunk(160);
        assert_eq!((small.width, small.height), (160, 90));
        assert_eq!(small.data.len(), small.stride() * small.height);
    }
}
