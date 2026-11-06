//! A pretend nvfwd device: a small shared file that says which slot is newest, and buffers on a socket.

use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::io::pwrite;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::fs::OpenOptions;
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::thread;
use std::time::Duration;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 240;
const STRIDE: u32 = WIDTH * 4;
const SLOTS: usize = 3;
const LATEST: u64 = 0x40;
const SHARED_SIZE: u64 = 4096;
const PACE: Duration = Duration::from_millis(50);

fn slot() -> OwnedFd {
    let file = memfd_create("slot", MemfdFlags::CLOEXEC).unwrap();
    ftruncate(&file, u64::from(STRIDE * HEIGHT)).unwrap();
    file
}

fn draw(file: &OwnedFd, sequence: u64) {
    let shade = (sequence % 200) as u8 + 40;
    let row: Vec<u8> = (0..WIDTH).flat_map(|_| [shade, 0x30, 0x90, 0]).collect();
    for y in 0..HEIGHT {
        pwrite(file, &row, u64::from(y * STRIDE)).unwrap();
    }
}

/// Serves one viewer from a thread, at `show.sock` and `show.display` in the run folder.
pub fn serve(run_folder: &Path) {
    let shared_path = run_folder.join("show.display");
    let shared =
        OpenOptions::new().read(true).write(true).create(true).truncate(true).open(shared_path).unwrap();
    shared.set_len(SHARED_SIZE).unwrap();
    let socket = run_folder.join("show.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(socket).unwrap();
    thread::spawn(move || {
        let stream = listener.accept().unwrap().0;
        let slots: Vec<OwnedFd> = (0..SLOTS).map(|_| slot()).collect();
        let message: Vec<u8> =
            [WIDTH, HEIGHT, STRIDE, 0].iter().flat_map(|word| word.to_le_bytes()).collect();
        let fds: Vec<_> = slots.iter().map(AsFd::as_fd).collect();
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(SLOTS))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        control.push(SendAncillaryMessage::ScmRights(&fds));
        sendmsg(&stream, &[IoSlice::new(&message)], &mut control, SendFlags::empty()).unwrap();
        let mut sequence = 0u64;
        loop {
            sequence += 1;
            let slot = sequence as usize % SLOTS;
            draw(&slots[slot], sequence);
            shared.write_all_at(&(sequence << 8 | slot as u64).to_le_bytes(), LATEST).unwrap();
            thread::sleep(PACE);
        }
    });
}
