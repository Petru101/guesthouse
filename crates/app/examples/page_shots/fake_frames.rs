//! A pretend producer on the frame socket: a band moving across a ring of files in memory.

use guesthouse_frame_socket::{
    Colour, Descriptor, Format, Frame, FrameListener, Message, Release, Ring, SOCKET_NAME,
};
use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::io::pwrite;
use std::collections::BTreeSet;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const STRIDE: u32 = WIDTH * 4;
const SLOTS: u32 = 3;
const SIZE: u64 = STRIDE as u64 * HEIGHT as u64;
const GENERATION: u32 = 1;
const BAND_WIDTH: u32 = 32;
const STEP: u32 = 4;
const PACE: Duration = Duration::from_millis(33);
const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");

fn draw(file: &OwnedFd, sequence: u64) {
    let band = sequence as u32 * STEP % WIDTH;
    let mut row = Vec::with_capacity(STRIDE as usize);
    for x in 0..WIDTH {
        let lit = (band..band + BAND_WIDTH).contains(&x);
        row.extend_from_slice(if lit { &[0xE0, 0xA0, 0x40, 0] } else { &[0x30, 0x20, 0x20, 0] });
    }
    for y in 0..HEIGHT {
        pwrite(file, &row, u64::from(y * STRIDE)).unwrap();
    }
}

/// Serves one viewer from a thread. `released` counts the slots that come back.
pub fn serve(run_folder: &Path, released: Arc<AtomicU64>) {
    std::fs::create_dir_all(run_folder).unwrap();
    let listener = FrameListener::bind(&run_folder.join(SOCKET_NAME)).unwrap();
    thread::spawn(move || {
        let socket = listener.accept().unwrap();
        assert!(matches!(socket.receive().unwrap().unwrap().message, Message::Hello(_)));
        let slots: Vec<OwnedFd> = (0..SLOTS)
            .map(|_| {
                let file = memfd_create("slot", MemfdFlags::CLOEXEC).unwrap();
                ftruncate(&file, SIZE).unwrap();
                file
            })
            .collect();
        let ring = Ring { generation: GENERATION, slots: SLOTS, size: SIZE };
        let fds: Vec<_> = slots.iter().map(AsFd::as_fd).collect();
        socket.send(&Message::Ring(ring), &fds).unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut free: BTreeSet<u32> = (0..SLOTS).collect();
        let format = Format { fourcc: XRGB8888, modifier: 0 };
        let descriptor = Descriptor {
            width: WIDTH,
            height: HEIGHT,
            stride: STRIDE,
            offset: 0,
            format,
            colour: Colour::SRGB,
        };
        for sequence in 1.. {
            loop {
                match socket.receive() {
                    Ok(Some(packet)) => {
                        if let Message::Release(Release { slot, .. }) = packet.message {
                            free.insert(slot);
                            released.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(None) => break,
                    Err(_) => return,
                }
            }
            if let Some(slot) = free.pop_first() {
                draw(&slots[slot as usize], sequence);
                let frame = Frame { generation: GENERATION, slot, sequence, descriptor };
                if socket.send(&Message::Frame(frame), &[]).is_err() {
                    return;
                }
            }
            thread::sleep(PACE);
        }
    });
}
