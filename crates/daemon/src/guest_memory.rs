//! The guest's memory, as files QEMU shares.

pub use guesthouse_backend::WINDOW_BASE;
use guesthouse_backend::{GuestMemory, MemoryError, PagesView, in_window};
use std::cell::OnceCell;
use std::fs::{File, OpenOptions};
use std::io;
use std::ops::Range;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// With this much RAM, QEMU's machine keeps the first part below 4 GiB, and the rest above it.
const RAM_BELOW_HOLE: u64 = 0x8000_0000;
const HOLE_END: u64 = 1 << 32;
const PAGE_SIZE: u64 = 4096;
const WORD_SIZE: u64 = 8;
const HALF_WORD_SIZE: u64 = 4;

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

    fn window_range(&self) -> Range<u64> {
        WINDOW_BASE..WINDOW_BASE + self.window_size
    }

    /// How far `length` bytes at an address are into the mapping, if all of them are in it.
    fn offset_of(&self, address: u64, length: u64) -> Result<usize, MemoryError> {
        if in_window(&self.window_range(), address, length) {
            Ok((address - WINDOW_BASE) as usize)
        } else {
            Err(MemoryError::Outside(address))
        }
    }

    /// The same for a word, which also has to be aligned to its size.
    fn word_offset(&self, address: u64, size: u64) -> Result<usize, MemoryError> {
        let offset = self.offset_of(address, size)?;
        if address.is_multiple_of(size) { Ok(offset) } else { Err(MemoryError::Outside(address)) }
    }

    /// The word at a guest physical address. Guest and GPU change these while the host looks, so
    /// reads and writes go through an atomic and never through a plain reference.
    fn word(&self, address: u64) -> Result<&AtomicU64, MemoryError> {
        let offset = self.word_offset(address, WORD_SIZE)?;
        // SAFETY: `word_offset` keeps it inside the mapping and aligned, and the mapping outlives &self.
        Ok(unsafe { AtomicU64::from_ptr(self.window.add(offset).cast()) })
    }

    fn ram(&mut self) -> io::Result<&File> {
        match &mut self.ram {
            Some(file) => Ok(file),
            empty => Ok(empty.insert(open_read_write(&self.ram_path)?)),
        }
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        // SAFETY: this is the mapping made in `open`, and no reference outlives self.
        unsafe { libc::munmap(self.window.cast(), self.window_size as usize) };
    }
}

impl GuestMemory for SharedMemory {
    fn window(&self) -> Result<Range<u64>, MemoryError> {
        Ok(self.window_range())
    }

    fn read_u64(&self, address: u64) -> Result<u64, MemoryError> {
        Ok(self.word(address)?.load(Ordering::Acquire))
    }

    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
        self.word(address)?.store(value, Ordering::Release);
        Ok(())
    }

    fn write_u32(&mut self, address: u64, value: u32) -> Result<(), MemoryError> {
        let offset = self.word_offset(address, HALF_WORD_SIZE)?;
        // SAFETY: `word_offset` keeps it inside the mapping and aligned, and the mapping outlives self.
        let word = unsafe { AtomicU32::from_ptr(self.window.add(offset).cast()) };
        word.store(value, Ordering::Release);
        Ok(())
    }

    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize {
        let Ok(offset) = self.offset_of(address, 0) else { return 0 };
        let length = out.len().min(self.window_size as usize - offset);
        // SAFETY: `length` stops at the end of the mapping, and `out` is not part of it.
        unsafe { ptr::copy_nonoverlapping(self.window.add(offset), out.as_mut_ptr(), length) };
        length
    }

    fn zero(&mut self, address: u64, length: u64) -> Result<(), MemoryError> {
        let offset = self.offset_of(address, length)?;
        // SAFETY: `offset_of` keeps the range inside the mapping. Only freed memory gets cleared,
        // which neither guest nor GPU is using.
        unsafe { ptr::write_bytes(self.window.add(offset), 0, length as usize) };
        Ok(())
    }

    fn host_address(&self, address: u64, length: u64) -> Result<u64, MemoryError> {
        Ok(self.window as u64 + self.offset_of(address, length)? as u64)
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
            let run = 1 + pages[index..]
                .windows(2)
                .take_while(|pair| pair[0].checked_add(1) == Some(pair[1]))
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

    /// The memory, opened now if this is the first use. A failed open is tried again next time, as
    /// QEMU may only be late.
    fn memory(&self) -> Result<&SharedMemory, MemoryError> {
        if let Some(opened) = self.opened.get() {
            return Ok(opened);
        }
        let opened = SharedMemory::open(&self.window_path, self.ram_path.clone()).map_err(|error| {
            log::error!("{}: {error}", self.window_path.display());
            MemoryError::Unavailable
        })?;
        Ok(self.opened.get_or_init(|| opened))
    }

    fn memory_mut(&mut self) -> Result<&mut SharedMemory, MemoryError> {
        self.memory()?;
        self.opened.get_mut().ok_or(MemoryError::Unavailable)
    }
}

impl GuestMemory for LazySharedMemory {
    fn window(&self) -> Result<Range<u64>, MemoryError> {
        self.memory()?.window()
    }

    fn read_u64(&self, address: u64) -> Result<u64, MemoryError> {
        self.memory()?.read_u64(address)
    }

    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
        self.memory_mut()?.write_u64(address, value)
    }

    fn write_u32(&mut self, address: u64, value: u32) -> Result<(), MemoryError> {
        self.memory_mut()?.write_u32(address, value)
    }

    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize {
        self.memory().map_or(0, |memory| memory.read_bytes(address, out))
    }

    fn zero(&mut self, address: u64, length: u64) -> Result<(), MemoryError> {
        self.memory_mut()?.zero(address, length)
    }

    fn host_address(&self, address: u64, length: u64) -> Result<u64, MemoryError> {
        self.memory()?.host_address(address, length)
    }

    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView> {
        self.memory_mut().map_err(io::Error::other)?.map_pages(pages)
    }

    fn unmap_pages(&mut self, view: PagesView) {
        if let Ok(memory) = self.memory_mut() {
            memory.unmap_pages(view);
        }
    }
}

/// Maps `run` consecutive guest pages over part of a reservation.
fn map_run(base: *mut libc::c_void, index: usize, run: usize, fd: i32, page: u64) -> io::Result<()> {
    let offset = page.checked_mul(PAGE_SIZE).and_then(ram_offset);
    let offset = offset
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
        assert_eq!(memory.window(), Ok(WINDOW_BASE..WINDOW_BASE + 0x2000));
        memory.write_u64(WINDOW_BASE + 0x1000, 0x1122).unwrap();
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x1000), Ok(0x1122));
        drop(memory);
        assert_eq!(fs::read(&window).unwrap()[0x1000..0x1002], [0x22, 0x11]);
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn a_host_address_is_only_ever_inside_the_mapping() {
        let window = temporary_file("host-address", &[0; 0x2000]);
        let memory = SharedMemory::open(&window, PathBuf::new()).unwrap();
        let base = memory.window as u64;
        assert_eq!(memory.host_address(WINDOW_BASE, 0x2000), Ok(base));
        assert_eq!(memory.host_address(WINDOW_BASE + 0x1000, 0x1000), Ok(base + 0x1000));
        assert!(memory.host_address(WINDOW_BASE + 0x1000, 0x1001).is_err());
        assert!(memory.host_address(WINDOW_BASE - 0x1000, 0x1000).is_err());
        assert!(memory.host_address(u64::MAX, 2).is_err());
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn zero_clears_only_what_it_says() {
        let window = temporary_file("zero", &[0xFF; 0x3000]);
        let mut memory = SharedMemory::open(&window, PathBuf::new()).unwrap();
        memory.zero(WINDOW_BASE + 0x1000, 0x1000).unwrap();
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x1000), Ok(0));
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x2000), Ok(u64::MAX));
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn accesses_past_the_window_are_errors_and_nothing_else() {
        let window = temporary_file("outside", &[0xFF; 0x1000]);
        let mut memory = SharedMemory::open(&window, PathBuf::new()).unwrap();
        for address in [0, WINDOW_BASE - 8, WINDOW_BASE + 0x1000, WINDOW_BASE + 4, u64::MAX - 7] {
            assert_eq!(memory.read_u64(address), Err(MemoryError::Outside(address)));
            assert_eq!(memory.write_u64(address, 0), Err(MemoryError::Outside(address)));
        }
        assert!(memory.write_u32(WINDOW_BASE + 0x1000, 0).is_err());
        assert!(memory.write_u32(WINDOW_BASE + 2, 0).is_err());
        assert!(memory.zero(WINDOW_BASE + 0x800, 0x801).is_err());
        assert!(memory.zero(u64::MAX, 2).is_err());
        assert_eq!(memory.read_u64(WINDOW_BASE), Ok(u64::MAX), "a refused write changes nothing");
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn reads_outside_the_window_find_nothing() {
        let window = temporary_file("short-read", &[7; 0x1000]);
        let memory = SharedMemory::open(&window, PathBuf::new()).unwrap();
        assert_eq!(memory.read_bytes(WINDOW_BASE - 8, &mut [0; 8]), 0);
        assert_eq!(memory.read_bytes(WINDOW_BASE + 0x1000, &mut [0; 8]), 0);
        assert_eq!(memory.read_bytes(WINDOW_BASE + 0xFFC, &mut [0; 8]), 4);
        fs::remove_file(window).unwrap();
    }

    #[test]
    fn a_window_that_is_not_there_yet_is_an_error_and_is_tried_again() {
        let window = env::temp_dir().join(format!("guesthouse-test-{}-late", process::id()));
        let memory = LazySharedMemory::new(window.clone(), PathBuf::new());
        assert_eq!(memory.window(), Err(MemoryError::Unavailable));
        assert_eq!(memory.read_u64(WINDOW_BASE), Err(MemoryError::Unavailable));
        fs::write(&window, [0; 0x1000]).unwrap();
        assert_eq!(memory.window(), Ok(WINDOW_BASE..WINDOW_BASE + 0x1000));
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
        assert!(memory.map_pages(&[u64::MAX - 1, u64::MAX]).is_err());
        fs::remove_file(window).unwrap();
        fs::remove_file(ram).unwrap();
    }
}
