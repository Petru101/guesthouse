//! A screen's pixels as QEMU's display gives them: a whole frame, rectangles of one, or a file of
//! shared memory that QEMU draws in. Nothing here needs GTK, so a test can check it alone.

use rustix::io::pread;
use std::os::fd::OwnedFd;

const BYTES_PER_PIXEL: usize = 4;
/// Pixman's names for the formats QEMU uses.
pub const PIXMAN_X8R8G8B8: u32 = 0x2002_0888;
pub const PIXMAN_A8R8G8B8: u32 = 0x2002_8888;

pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub pixels: Vec<u8>,
    /// QEMU's shared memory and where the picture starts in it, if it draws there.
    shared: Option<(OwnedFd, u64)>,
}

impl Surface {
    /// `None` for a picture its own data can't hold.
    pub fn new(width: u32, height: u32, stride: u32, format: u32, pixels: Vec<u8>) -> Option<Self> {
        let size = stride as usize * height as usize;
        (pixels.len() >= size).then_some(Self { width, height, stride, format, pixels, shared: None })
    }

    pub fn shared(width: u32, height: u32, stride: u32, format: u32, file: OwnedFd, offset: u64) -> Self {
        let pixels = vec![0; stride as usize * height as usize];
        Self { width, height, stride, format, pixels, shared: Some((file, offset)) }
    }

    /// Copies in a rectangle. One that isn't for this surface is left out, as QEMU sends a whole frame
    /// after any change of surface.
    pub fn write(&mut self, x: i32, y: i32, width: i32, height: i32, stride: u32, data: &[u8]) {
        let fits_x = x >= 0 && width >= 0 && (x + width) as u32 <= self.width;
        let fits_y = y >= 0 && height >= 0 && (y + height) as u32 <= self.height;
        if !fits_x || !fits_y {
            return;
        }
        let row_bytes = width as usize * BYTES_PER_PIXEL;
        let wanted = (height as usize).saturating_sub(1) * stride as usize + row_bytes;
        if data.len() < wanted {
            return;
        }
        for row in 0..height as usize {
            let from = row * stride as usize;
            let to = (y as usize + row) * self.stride as usize + x as usize * BYTES_PER_PIXEL;
            self.pixels[to..to + row_bytes].copy_from_slice(&data[from..from + row_bytes]);
        }
    }

    /// Brings the pixels up to date with shared memory, if this surface has any.
    pub fn refresh(&mut self) {
        if let Some((file, offset)) = &self.shared {
            let mut done = 0;
            while done < self.pixels.len() {
                match pread(file, &mut self.pixels[done..], offset + done as u64) {
                    Ok(read) if read > 0 => done += read,
                    _ => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{MemfdFlags, memfd_create};
    use rustix::io::pwrite;

    fn surface() -> Surface {
        Surface::new(4, 2, 16, PIXMAN_X8R8G8B8, vec![0; 32]).unwrap()
    }

    #[test]
    fn a_frame_too_short_for_its_size_is_refused() {
        assert!(Surface::new(4, 2, 16, PIXMAN_X8R8G8B8, vec![0; 31]).is_none());
    }

    #[test]
    fn a_rectangle_lands_where_it_says() {
        let mut surface = surface();
        surface.write(1, 1, 2, 1, 8, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(surface.pixels[20..28], [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(surface.pixels[..20], [0; 20]);
    }

    #[test]
    fn rows_are_taken_by_their_own_stride() {
        let mut surface = surface();
        let data = [1, 1, 1, 1, 9, 9, 9, 9, 2, 2, 2, 2];
        surface.write(0, 0, 1, 2, 8, &data);
        assert_eq!(surface.pixels[..4], [1; 4]);
        assert_eq!(surface.pixels[16..20], [2; 4]);
    }

    #[test]
    fn a_rectangle_outside_the_surface_changes_nothing() {
        let mut surface = surface();
        for (x, y, width, height) in [(3, 0, 2, 1), (0, 1, 1, 2), (-1, 0, 1, 1), (0, 0, -1, 1)] {
            surface.write(x, y, width, height, 16, &[7; 64]);
        }
        surface.write(0, 0, 4, 2, 16, &[7; 16]);
        assert_eq!(surface.pixels, [0; 32], "short data counts too");
    }

    #[test]
    fn shared_memory_is_read_from_its_offset() {
        let file = memfd_create("screen", MemfdFlags::CLOEXEC).unwrap();
        pwrite(&file, &[5; 40], 0).unwrap();
        let mut surface = Surface::shared(4, 2, 16, PIXMAN_A8R8G8B8, file, 8);
        surface.refresh();
        assert_eq!(surface.pixels, [5; 32]);
    }
}
