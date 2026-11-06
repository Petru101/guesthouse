//! The guest's memory, as files QEMU shares.

use guesthouse_backend::{GuestMemory, PagesView};
use std::cell::OnceCell;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Where QEMU places the shared window in guest physical memory.
pub const WINDOW_BASE: u64 = 1 << 40;

/// With this much RAM, QEMU's machine keeps the first part below 4 GiB, and the rest above it.
const RAM_BELOW_HOLE: u64 = 0x8000_0000;
const HOLE_END: u64 = 1 << 32;
const PAGE_SIZE: u64 = 4096;

/// Where a guest physical address is in the RAM file. The machine splits RAM around a hole below
/// 4 GiB, which the file does not have.
pub fn ram_offset(address: u64) -> Option<u64> {
    if address < RAM_BELOW_HOLE {
        Some(address)
    } else if address >= HOLE_END {
        Some(address - HOLE_END + RAM_BELOW_HOLE)
    } else {
        None
    }
}

fn open_read_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn check(result: *mut libc::c_void) -> io::Result<*mut libc::c_void> {
    if result == libc::MAP_FAILED { Err(io::Error::last_os_error()) } else { Ok(result) }
}

/// The window mapped here, and the RAM file ready to map from.
pub struct SharedMemory {
    window: *mut u8,
    window_size: u64,
    ram_path: PathBuf,
    ram: Option<File>,
}

impl SharedMemory {
    pub fn open(window_path: &Path, ram_path: PathBuf) -> io::Result<Self> {
        let file = open_read_write(window_path)?;
        let window_size = file.metadata()?.len();
        let flags = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: a fresh mapping at an address the kernel picks disturbs nothing. It lasts until
        // Drop, and the file may close once it exists.
        let window = unsafe {
            libc::mmap(ptr::null_mut(), window_size as usize, flags, libc::MAP_SHARED, file.as_raw_fd(), 0)
        };
        Ok(Self { window: check(window)?.cast(), window_size, ram_path, ram: None })
    }

    /// The word at a guest physical address. Guest and GPU change these while the host looks, so
    /// reads and writes go through an atomic and never through a plain reference.
    fn word(&self, address: u64) -> &AtomicU64 {
        let offset = address - WINDOW_BASE;
        assert!(
            offset + 8 <= self.window_size && offset.is_multiple_of(8),
            "{address:#x} is not a word in the window"
        );
        // SAFETY: the assert keeps it inside the mapping and aligned, and the mapping outlives &self.
        unsafe { AtomicU64::from_ptr(self.window.add(offset as usize).cast()) }
    }

    fn ram(&mut self) -> io::Result<&File> {
        if self.ram.is_none() {
            self.ram = Some(open_read_write(&self.ram_path)?);
        }
        Ok(self.ram.as_ref().expect("opened just above"))
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        // SAFETY: this is the mapping made in `open`, and no reference outlives self.
        unsafe { libc::munmap(self.window.cast(), self.window_size as usize) };
    }
}

impl GuestMemory for SharedMemory {
    fn window_end(&self) -> u64 {
        WINDOW_BASE + self.window_size
    }

    fn read_u64(&self, address: u64) -> u64 {
        self.word(address).load(Ordering::Acquire)
    }

    fn write_u64(&mut self, address: u64, value: u64) {
        self.word(address).store(value, Ordering::Release);
    }

    fn write_u32(&mut self, address: u64, value: u32) {
        let offset = address - WINDOW_BASE;
        assert!(
            offset + 4 <= self.window_size && offset.is_multiple_of(4),
            "{address:#x} is not a word in the window"
        );
        // SAFETY: the assert keeps it inside the mapping and aligned, and the mapping outlives self.
        let word = unsafe { AtomicU32::from_ptr(self.window.add(offset as usize).cast()) };
        word.store(value, Ordering::Release);
    }

    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize {
        let offset = address.saturating_sub(WINDOW_BASE).min(self.window_size);
        let length = out.len().min((self.window_size - offset) as usize);
        // SAFETY: `length` stops at the end of the mapping, and `out` is not part of it.
        unsafe { ptr::copy_nonoverlapping(self.window.add(offset as usize), out.as_mut_ptr(), length) };
        length
    }

    fn zero(&mut self, address: u64, length: u64) {
        let offset = address - WINDOW_BASE;
        assert!(offset + length <= self.window_size, "{address:#x} is not in the window");
        // SAFETY: the assert keeps the range inside the mapping. Only freed memory gets cleared,
        // which neither guest nor GPU is using.
        unsafe { ptr::write_bytes(self.window.add(offset as usize), 0, length as usize) };
    }

    fn host_address(&self, address: u64) -> u64 {
        self.window as u64 + (address - WINDOW_BASE)
    }

    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView> {
        let fd = self.ram()?.as_raw_fd();
        let size = pages.len() as u64 * PAGE_SIZE;
        let reserve = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
        // SAFETY: this only reserves address space that the kernel picks.
        let base =
            check(unsafe { libc::mmap(ptr::null_mut(), size as usize, libc::PROT_NONE, reserve, -1, 0) })?;
        let view = PagesView { host_address: base as u64, size };
        let mut index = 0;
        while index < pages.len() {
            let run = 1 + pages[index + 1..]
                .iter()
                .zip(pages[index]..)
                .take_while(|&(&next, previous)| next == previous + 1)
                .count();
            let result = map_run(base, index, run, fd, pages[index]);
            if let Err(error) = result {
                self.unmap_pages(view);
                return Err(error);
            }
            index += run;
        }
        Ok(view)
    }

    fn unmap_pages(&mut self, view: PagesView) {
        // SAFETY: the view came from `map_pages`, and the caller has stopped using it.
        unsafe { libc::munmap(view.host_address as *mut libc::c_void, view.size as usize) };
    }
}

/// QEMU makes the shared files after this daemon has started, so they open at the first use, which
/// comes with the first message.
pub struct LazySharedMemory {
    window_path: PathBuf,
    ram_path: PathBuf,
    opened: OnceCell<SharedMemory>,
}

impl LazySharedMemory {
    pub fn new(window_path: PathBuf, ram_path: PathBuf) -> Self {
        Self { window_path, ram_path, opened: OnceCell::new() }
    }

    fn memory(&self) -> &SharedMemory {
        self.opened.get_or_init(|| {
            let opened = SharedMemory::open(&self.window_path, self.ram_path.clone());
            opened.unwrap_or_else(|error| panic!("{}: {error}", self.window_path.display()))
        })
    }

    fn memory_mut(&mut self) -> &mut SharedMemory {
        self.memory();
        self.opened.get_mut().expect("opened just above")
    }
}

impl GuestMemory for LazySharedMemory {
    fn window_end(&self) -> u64 {
        self.memory().window_end()
    }

    fn read_u64(&self, address: u64) -> u64 {
        self.memory().read_u64(address)
    }

    fn write_u64(&mut self, address: u64, value: u64) {
        self.memory_mut().write_u64(address, value);
    }

    fn write_u32(&mut self, address: u64, value: u32) {
        self.memory_mut().write_u32(address, value);
    }

    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize {
        self.memory().read_bytes(address, out)
    }

    fn zero(&mut self, address: u64, length: u64) {
        self.memory_mut().zero(address, length);
    }

    fn host_address(&self, address: u64) -> u64 {
        self.memory().host_address(address)
    }

    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView> {
        self.memory_mut().map_pages(pages)
    }

    fn unmap_pages(&mut self, view: PagesView) {
        self.memory_mut().unmap_pages(view);
    }
}

/// Maps `run` consecutive guest pages over part of a reservation.
fn map_run(base: *mut libc::c_void, index: usize, run: usize, fd: i32, page: u64) -> io::Result<()> {
    let offset = ram_offset(page * PAGE_SIZE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("page {page:#x} is not RAM")))?;
    let at = base.wrapping_byte_add(index * PAGE_SIZE as usize);
    let protection = libc::PROT_READ | libc::PROT_WRITE;
    let flags = libc::MAP_SHARED | libc::MAP_FIXED;
    // SAFETY: `at` lies inside the reservation made by `map_pages`, which nothing else uses.
    let done = unsafe { libc::mmap(at, run * PAGE_SIZE as usize, protection, flags, fd, offset as i64) };
    check(done).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::process;

    fn temporary_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = env::temp_dir().join(format!("guesthouse-test-{}-{name}", process::id()));
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn ram_is_split_around_the_hole() {
        assert_eq!(ram_offset(0x1000), Some(0x1000));
        assert_eq!(ram_offset(RAM_BELOW_HOLE), None);
        assert_eq!(ram_offset(HOLE_END), Some(RAM_BELOW_HOLE));
    }

    #[test]
    fn the_window_reads_and_writes_the_file() {
        let window = temporary_file("window", &[0; 0x2000]);
        let mut memory = SharedMemory::open(&window, PathBuf::new()).unwrap();
        assert_eq!(memory.window_end(), WINDOW_BASE + 0x2000);
        memory.write_u64(WINDOW_BASE + 0x1000, 0x1122);
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x1000), 0x1122);
        drop(memory);
        assert_eq!(fs::read(&window).unwrap()[0x1000..0x1002], [0x22, 0x11]);
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn zero_clears_only_what_it_says() {
        let window = temporary_file("zero", &[0xFF; 0x3000]);
        let mut memory = SharedMemory::open(&window, PathBuf::new()).unwrap();
        memory.zero(WINDOW_BASE + 0x1000, 0x1000);
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x1000), 0);
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x2000), u64::MAX);
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn scattered_pages_come_out_side_by_side() {
        let mut ram = vec![0; 4 * PAGE_SIZE as usize];
        (0..4).for_each(|page| ram[page * PAGE_SIZE as usize] = page as u8 + 1);
        let window = temporary_file("pages-window", &[0; 8]);
        let ram = temporary_file("pages-ram", &ram);
        let mut memory = SharedMemory::open(&window, ram.clone()).unwrap();
        let view = memory.map_pages(&[3, 1, 2]).unwrap();
        assert_eq!(view.size, 3 * PAGE_SIZE);
        let bytes = view.host_address as *const u8;
        // SAFETY: the view is mapped for three pages until the unmap below.
        let firsts: Vec<u8> = (0..3).map(|page| unsafe { *bytes.add(page * PAGE_SIZE as usize) }).collect();
        assert_eq!(firsts, [4, 2, 3]);
        memory.unmap_pages(view);
        fs::remove_file(window).unwrap();
        fs::remove_file(ram).unwrap();
    }

    #[test]
    fn a_page_in_the_hole_is_refused() {
        let window = temporary_file("hole-window", &[0; 8]);
        let ram = temporary_file("hole-ram", &[0; 4096]);
        let mut memory = SharedMemory::open(&window, ram.clone()).unwrap();
        assert!(memory.map_pages(&[RAM_BELOW_HOLE / PAGE_SIZE]).is_err());
        fs::remove_file(window).unwrap();
        fs::remove_file(ram).unwrap();
    }
}
