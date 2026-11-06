//! The guest's hardware cursor. Its picture comes as a sealed file, and its numbers can come from a
//! guest, so they're checked before the file is read.

use super::ring::file_size;
use gtk::{gdk, glib};
use guesthouse_frame_socket::CursorShape;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;

const NO_POINTER: &str = "none";
/// A 256 by 256 picture, as large as any cursor Windows has.
const MOST_CURSOR_BYTES: u32 = 256 * 256 * 4;

/// A cursor's picture, ready to show.
#[derive(Clone)]
pub struct CursorPicture {
    pub texture: gdk::Texture,
    pub hot_x: i32,
    pub hot_y: i32,
}

impl CursorPicture {
    /// `None` if the file doesn't hold what the numbers say.
    pub fn read(shape: &CursorShape, file: OwnedFd) -> Option<Self> {
        let holds = file_size(&file).is_some_and(|size| size >= u64::from(shape.size));
        if !shape.fits() || shape.size > MOST_CURSOR_BYTES || !holds {
            return None;
        }
        let mut pixels = vec![0; shape.size as usize];
        File::from(file).read_exact_at(&mut pixels, 0).ok()?;
        let texture = gdk::MemoryTexture::new(
            shape.width as i32,
            shape.height as i32,
            gdk::MemoryFormat::B8g8r8a8,
            &glib::Bytes::from_owned(pixels),
            shape.pitch as usize,
        );
        Some(Self { texture: texture.into(), hot_x: shape.hot_x as i32, hot_y: shape.hot_y as i32 })
    }

    /// The host's pointer, looking like this. It moves with no delay, which a picture drawn in the
    /// window can't.
    pub fn pointer(&self) -> gdk::Cursor {
        gdk::Cursor::from_texture(&self.texture, self.hot_x, self.hot_y, None)
    }
}

/// For a guest that has hidden its cursor.
pub fn no_pointer() -> Option<gdk::Cursor> {
    gdk::Cursor::from_name(NO_POINTER, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_frame_socket::sealed_bytes;

    fn shape(width: u32, size: u32) -> CursorShape {
        CursorShape { shape: 1, width, height: 2, pitch: width * 4, hot_x: 0, hot_y: 1, size }
    }

    #[test]
    fn numbers_that_dont_fit_are_refused_before_the_file_is_read() {
        let file = sealed_bytes("cursor", &[0; 16]).unwrap();
        assert!(CursorPicture::read(&shape(2, 8), file).is_none());
    }

    #[test]
    fn a_huge_cursor_is_refused_before_anything_is_allocated() {
        let file = sealed_bytes("cursor", &[0; 16]).unwrap();
        assert!(CursorPicture::read(&shape(2, u32::MAX), file).is_none());
    }

    #[test]
    fn a_file_shorter_than_its_numbers_is_refused() {
        let file = sealed_bytes("cursor", &[0; 4]).unwrap();
        assert!(CursorPicture::read(&shape(2, 16), file).is_none());
    }
}
