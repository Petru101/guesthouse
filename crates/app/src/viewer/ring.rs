//! The buffers a producer has shared, and the checks a frame must pass before they're touched.

use guesthouse_frame_socket::{Descriptor, Frame, Ring};
use rustix::fs::{SealFlags, SeekFrom, fcntl_get_seals, fstatfs, seek};
use rustix::io::pread;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;

/// The filesystem a dmabuf's file is on. A dmabuf can't change size.
const DMA_BUF_MAGIC: i64 = 0x444d_4142;

/// How long a shared file is. A dmabuf's `fstat` says nothing, but seeking to its end works for both
/// kinds, and the file's position isn't used: everything here reads at an offset.
pub fn file_size(file: impl AsFd) -> Option<u64> {
    seek(file, SeekFrom::End(0)).ok()
}

/// Whether a file will stay as long as it is now: a dmabuf, or a file in memory sealed against
/// shrinking. A mapping past a file's end would bring the whole app down.
fn cannot_shrink(file: &OwnedFd) -> bool {
    let is_dmabuf = fstatfs(file).is_ok_and(|filesystem| filesystem.f_type == DMA_BUF_MAGIC);
    is_dmabuf || fcntl_get_seals(file).is_ok_and(|seals| seals.contains(SealFlags::SHRINK))
}

/// One generation of buffers. Textures hold on to their slot's file, so an old ring's last picture
/// can stay on screen after the ring has gone.
pub struct Buffers {
    pub generation: u32,
    size: u64,
    slots: Vec<Arc<OwnedFd>>,
}

impl Buffers {
    /// `None` unless there's a file for each slot, as long as the ring says, and staying that long.
    pub fn new(ring: &Ring, fds: Vec<OwnedFd>) -> Option<Self> {
        let holds =
            |file: &OwnedFd| cannot_shrink(file) && file_size(file).is_some_and(|size| size >= ring.size);
        (fds.len() == ring.slots as usize && fds.iter().all(holds)).then(|| Self {
            generation: ring.generation,
            size: ring.size,
            slots: fds.into_iter().map(Arc::new).collect(),
        })
    }

    /// The slot's file, if this frame is really in it. What a frame says can come from a guest.
    pub fn slot(&self, frame: &Frame) -> Option<Arc<OwnedFd>> {
        let fits = frame.generation == self.generation && frame.descriptor.fits(self.size);
        fits.then(|| self.slots.get(frame.slot as usize).cloned()).flatten()
    }
}

/// Copies a picture out of its slot. This works for a file in memory, and a real dmabuf refuses it.
pub fn read_pixels(file: &OwnedFd, descriptor: &Descriptor) -> io::Result<Vec<u8>> {
    let size = descriptor.stride as usize * descriptor.height as usize;
    let mut pixels = vec![0; size];
    let mut done = 0;
    while done < size {
        let read = pread(file, &mut pixels[done..], descriptor.offset + done as u64)?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        done += read;
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_frame_socket::{Colour, Format};
    use rustix::fs::{MemfdFlags, fcntl_add_seals, memfd_create};
    use rustix::io::pwrite;

    const SIZE: u64 = 4096;

    fn descriptor() -> Descriptor {
        let format = Format { fourcc: 0, modifier: 0 };
        Descriptor { width: 4, height: 2, stride: 16, offset: 8, format, colour: Colour::SRGB }
    }

    fn frame(generation: u32, slot: u32) -> Frame {
        Frame { generation, slot, sequence: 1, descriptor: descriptor() }
    }

    fn unsealed_file() -> OwnedFd {
        let file = memfd_create("slot", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap();
        pwrite(&file, &[7; SIZE as usize], 0).unwrap();
        file
    }

    fn file() -> OwnedFd {
        let file = unsealed_file();
        fcntl_add_seals(&file, SealFlags::SHRINK).unwrap();
        file
    }

    fn buffers() -> Buffers {
        Buffers::new(&Ring { generation: 3, slots: 2, size: SIZE }, vec![file(), file()]).unwrap()
    }

    #[test]
    fn a_ring_needs_a_file_for_each_slot() {
        let ring = Ring { generation: 1, slots: 2, size: SIZE };
        assert!(Buffers::new(&ring, vec![file()]).is_none());
    }

    #[test]
    fn a_ring_larger_than_its_files_is_refused() {
        let ring = Ring { generation: 1, slots: 1, size: 1 << 40 };
        assert!(Buffers::new(&ring, vec![file()]).is_none());
    }

    #[test]
    fn a_file_that_could_shrink_is_refused() {
        let ring = Ring { generation: 1, slots: 1, size: SIZE };
        assert!(Buffers::new(&ring, vec![unsealed_file()]).is_none());
    }

    #[test]
    fn a_good_frame_finds_its_slot() {
        assert!(buffers().slot(&frame(3, 1)).is_some());
    }

    #[test]
    fn a_frame_from_another_ring_or_slot_is_refused() {
        assert!(buffers().slot(&frame(2, 1)).is_none());
        assert!(buffers().slot(&frame(3, 2)).is_none());
    }

    #[test]
    fn a_picture_past_the_end_of_its_slot_is_refused() {
        let mut large = frame(3, 0);
        large.descriptor.height = 1 << 20;
        assert!(buffers().slot(&large).is_none());
    }

    #[test]
    fn pixels_are_read_from_the_pictures_offset() {
        let file = file();
        pwrite(&file, &[9], 8).unwrap();
        let pixels = read_pixels(&file, &descriptor()).unwrap();
        assert_eq!(pixels.len(), 32);
        assert_eq!((pixels[0], pixels[1]), (9, 7));
    }
}
