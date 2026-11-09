//! Buffer objects, their CPU mapping, and their place in the GPU's address space.
//!
//! A buffer object is made with `GEM_CREATE` in video memory or GART-mapped system memory, or
//! adopted from the process's own pages with `GEM_USERPTR`. It reaches the GPU once it is given an
//! address with `GEM_VA`, and it reaches another process with `PRIME_HANDLE_TO_FD`. The flags and
//! offsets are the kernel's uAPI.

use crate::ioctl::{self, call, get32, get64, put32, put64};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

// GEM_CREATE domains and flags.
pub const DOMAIN_GTT: u64 = 0x2;
pub const DOMAIN_VRAM: u64 = 0x4;
const CREATE_CPU_ACCESS_REQUIRED: u64 = 1 << 0;
/// The kernel fills a new buffer with zeros. WDDM promises a guest that new memory is, and AMD's
/// driver counts on it: a command buffer's first packet waits on a word it never wrote.
const CREATE_VRAM_CLEARED: u64 = 1 << 3;

// GEM_USERPTR flags. REGISTER keeps the pages tracked so the GPU sees the mapping the process has,
// and VALIDATE faults them in now. ANONONLY is left off, as the guest's RAM is file-backed.
const USERPTR_READONLY: u32 = 1 << 0;
const USERPTR_VALIDATE: u32 = 1 << 2;
const USERPTR_REGISTER: u32 = 1 << 3;

// GEM_VA operations and page flags.
const VA_OP_MAP: u32 = 1;
const VA_OP_UNMAP: u32 = 2;
pub const VM_PAGE_READABLE: u32 = 1 << 1;
pub const VM_PAGE_WRITEABLE: u32 = 1 << 2;
pub const VM_PAGE_EXECUTABLE: u32 = 1 << 3;
/// The memory type lives in four bits at this shift. The values below are MTYPE_* from the header.
const VM_MTYPE_SHIFT: u32 = 5;

/// How the GPU's caches should treat a mapping, which becomes the MTYPE bits of its page tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// The driver's default for the placement.
    Default,
    /// Non-coherent: the GPU may cache it and need not snoop the CPU.
    NonCoherent,
    /// Cache-coherent: used for host memory the CPU also touches.
    CacheCoherent,
    /// Uncached.
    Uncached,
}

impl MemoryType {
    fn bits(self) -> u32 {
        let value = match self {
            MemoryType::Default => 0,
            MemoryType::NonCoherent => 1,
            MemoryType::CacheCoherent => 3,
            MemoryType::Uncached => 4,
        };
        value << VM_MTYPE_SHIFT
    }
}

// Offsets into the request buffers.
const CREATE_BO_SIZE: usize = 0;
const CREATE_ALIGNMENT: usize = 8;
const CREATE_DOMAINS: usize = 16;
const CREATE_DOMAIN_FLAGS: usize = 24;
const CREATE_OUT_HANDLE: usize = 0;
const CREATE_REQUEST_SIZE: usize = 32;

const MMAP_IN_HANDLE: usize = 0;
const MMAP_OUT_ADDR: usize = 0;
const MMAP_REQUEST_SIZE: usize = 8;

const USERPTR_ADDR: usize = 0;
const USERPTR_SIZE: usize = 8;
const USERPTR_FLAGS: usize = 16;
const USERPTR_HANDLE: usize = 20;
const USERPTR_REQUEST_SIZE: usize = 24;

const VA_HANDLE: usize = 0;
const VA_OPERATION: usize = 8;
const VA_FLAGS: usize = 12;
const VA_ADDRESS: usize = 16;
const VA_OFFSET_IN_BO: usize = 24;
const VA_MAP_SIZE: usize = 32;
const VA_REQUEST_SIZE: usize = 64;

const PRIME_HANDLE: usize = 0;
const PRIME_FLAGS: usize = 4;
const PRIME_FD: usize = 8;
const PRIME_REQUEST_SIZE: usize = 12;

const GEM_CLOSE_REQUEST_SIZE: usize = 8;

/// A GEM handle on an open render node. It is only a number until it is given an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GemHandle(pub u32);

/// How much CPU reach a new buffer needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuAccess {
    Required,
    NotNeeded,
}

/// Makes a buffer object in the given domain. On an APU, visible VRAM is the whole of it, so a
/// CPU-reachable VRAM buffer is ordinary.
pub fn create(
    file: &impl AsRawFd,
    size: u64,
    align: u64,
    domain: u64,
    cpu: CpuAccess,
) -> io::Result<GemHandle> {
    let mut request = [0u8; CREATE_REQUEST_SIZE];
    put64(&mut request, CREATE_BO_SIZE, size);
    put64(&mut request, CREATE_ALIGNMENT, align.max(1));
    put64(&mut request, CREATE_DOMAINS, domain);
    put64(&mut request, CREATE_DOMAIN_FLAGS, create_flags(domain, cpu));
    ioctl::drm_ioctl(file, call::gem_create(), &mut request)?;
    Ok(GemHandle(get32(&request, CREATE_OUT_HANDLE)))
}

fn create_flags(domain: u64, cpu: CpuAccess) -> u64 {
    let cpu_flag =
        if cpu == CpuAccess::Required && domain == DOMAIN_VRAM { CREATE_CPU_ACCESS_REQUIRED } else { 0 };
    cpu_flag | CREATE_VRAM_CLEARED
}

/// Adopts pages already mapped in this process, so the GPU reaches the same memory the CPU does.
/// This is how the guest's RAM is handed to the card.
pub fn adopt_userptr(
    file: &impl AsRawFd,
    host_address: u64,
    size: u64,
    read_only: bool,
) -> io::Result<GemHandle> {
    let mut request = [0u8; USERPTR_REQUEST_SIZE];
    put64(&mut request, USERPTR_ADDR, host_address);
    put64(&mut request, USERPTR_SIZE, size);
    let mut flags = USERPTR_REGISTER | USERPTR_VALIDATE;
    if read_only {
        flags |= USERPTR_READONLY;
    }
    put32(&mut request, USERPTR_FLAGS, flags);
    ioctl::drm_ioctl(file, call::gem_userptr(), &mut request)?;
    Ok(GemHandle(get32(&request, USERPTR_HANDLE)))
}

/// The offset to pass to `mmap` to reach a buffer's bytes from the CPU. Only buffers with CPU access
/// can be mapped.
pub fn mmap_offset(file: &impl AsRawFd, handle: GemHandle) -> io::Result<u64> {
    let mut request = [0u8; MMAP_REQUEST_SIZE];
    put32(&mut request, MMAP_IN_HANDLE, handle.0);
    ioctl::drm_ioctl(file, call::gem_mmap(), &mut request)?;
    Ok(get64(&request, MMAP_OUT_ADDR))
}

/// What a mapping is allowed to do and how its cache behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapFlags {
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub memory_type: MemoryType,
}

impl MapFlags {
    pub fn read_write() -> Self {
        Self { readable: true, writable: true, executable: false, memory_type: MemoryType::Default }
    }

    fn bits(self) -> u32 {
        let mut flags = self.memory_type.bits();
        if self.readable {
            flags |= VM_PAGE_READABLE;
        }
        if self.writable {
            flags |= VM_PAGE_WRITEABLE;
        }
        if self.executable {
            flags |= VM_PAGE_EXECUTABLE;
        }
        flags
    }
}

/// Gives a buffer a GPU address, so a submission can reach it. The address and size must be aligned
/// to the page size the card reported.
pub fn map(
    file: &impl AsRawFd,
    handle: GemHandle,
    address: u64,
    offset: u64,
    size: u64,
    flags: MapFlags,
) -> io::Result<()> {
    gem_va(file, handle, VA_OP_MAP, address, offset, size, flags.bits())
}

/// Takes a buffer's GPU address back.
pub fn unmap(file: &impl AsRawFd, handle: GemHandle, address: u64, size: u64) -> io::Result<()> {
    gem_va(file, handle, VA_OP_UNMAP, address, 0, size, 0)
}

fn gem_va(
    file: &impl AsRawFd,
    handle: GemHandle,
    operation: u32,
    address: u64,
    offset: u64,
    size: u64,
    flags: u32,
) -> io::Result<()> {
    let mut request = [0u8; VA_REQUEST_SIZE];
    put32(&mut request, VA_HANDLE, handle.0);
    put32(&mut request, VA_OPERATION, operation);
    put32(&mut request, VA_FLAGS, flags);
    put64(&mut request, VA_ADDRESS, address);
    put64(&mut request, VA_OFFSET_IN_BO, offset);
    put64(&mut request, VA_MAP_SIZE, size);
    ioctl::drm_ioctl(file, call::gem_va(), &mut request)
}

/// Hands a buffer out as a file another process can read where it lies, for frames that stay in
/// video memory.
pub fn export(file: &impl AsRawFd, handle: GemHandle) -> io::Result<OwnedFd> {
    let mut request = [0u8; PRIME_REQUEST_SIZE];
    put32(&mut request, PRIME_HANDLE, handle.0);
    put32(&mut request, PRIME_FLAGS, (libc::O_CLOEXEC | libc::O_RDWR) as u32);
    ioctl::drm_ioctl(file, call::prime_handle_to_fd(), &mut request)?;
    let fd = get32(&request, PRIME_FD) as i32;
    // SAFETY: the kernel returned a fresh file descriptor the caller now owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Takes in a buffer another open file handed out with [`export`], as a handle on `file`. A buffer
/// already known to `file` comes back under the handle it has there.
pub fn import(file: &impl AsRawFd, buffer: &impl AsRawFd) -> io::Result<GemHandle> {
    let mut request = [0u8; PRIME_REQUEST_SIZE];
    put32(&mut request, PRIME_FD, buffer.as_raw_fd() as u32);
    ioctl::drm_ioctl(file, call::prime_fd_to_handle(), &mut request)?;
    Ok(GemHandle(get32(&request, PRIME_HANDLE)))
}

// GEM_METADATA, a 288-byte request: the handle, the operation, then flags, tiling_info and an opaque
// blob the buffer carries for whoever imports it.
const METADATA_REQUEST_SIZE: usize = 288;
const METADATA_HANDLE: usize = 0;
const METADATA_OP: usize = 4;
const METADATA_TILING_INFO: usize = 16;
const METADATA_OP_SET: u32 = 1;
const METADATA_OP_GET: u32 = 2;

/// Keeps tiling flags on a buffer, so an importer lays it out the way it was drawn.
pub fn set_tiling(file: &impl AsRawFd, handle: GemHandle, tiling_info: u64) -> io::Result<()> {
    let mut request = [0u8; METADATA_REQUEST_SIZE];
    put32(&mut request, METADATA_HANDLE, handle.0);
    put32(&mut request, METADATA_OP, METADATA_OP_SET);
    put64(&mut request, METADATA_TILING_INFO, tiling_info);
    ioctl::drm_ioctl(file, call::gem_metadata(), &mut request)
}

/// The tiling flags a buffer carries.
pub fn tiling(file: &impl AsRawFd, handle: GemHandle) -> io::Result<u64> {
    let mut request = [0u8; METADATA_REQUEST_SIZE];
    put32(&mut request, METADATA_HANDLE, handle.0);
    put32(&mut request, METADATA_OP, METADATA_OP_GET);
    ioctl::drm_ioctl(file, call::gem_metadata(), &mut request)?;
    Ok(get64(&request, METADATA_TILING_INFO))
}

/// Lets a GEM handle go. Any address it held must be taken back first.
pub fn close(file: &impl AsRawFd, handle: GemHandle) {
    let mut request = [0u8; GEM_CLOSE_REQUEST_SIZE];
    put32(&mut request, 0, handle.0);
    let _ = ioctl::drm_ioctl(file, call::gem_close(), &mut request);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_new_buffer_starts_cleared() {
        for domain in [DOMAIN_GTT, DOMAIN_VRAM] {
            for cpu in [CpuAccess::Required, CpuAccess::NotNeeded] {
                assert_ne!(create_flags(domain, cpu) & CREATE_VRAM_CLEARED, 0);
            }
        }
    }

    #[test]
    fn only_video_memory_the_cpu_needs_asks_for_cpu_access() {
        assert_ne!(create_flags(DOMAIN_VRAM, CpuAccess::Required) & CREATE_CPU_ACCESS_REQUIRED, 0);
        assert_eq!(create_flags(DOMAIN_VRAM, CpuAccess::NotNeeded) & CREATE_CPU_ACCESS_REQUIRED, 0);
        assert_eq!(create_flags(DOMAIN_GTT, CpuAccess::Required) & CREATE_CPU_ACCESS_REQUIRED, 0);
    }

    #[test]
    fn an_executable_mapping_says_so_to_the_kernel() {
        let flags = MapFlags { executable: true, ..MapFlags::read_write() };
        assert_ne!(flags.bits() & VM_PAGE_EXECUTABLE, 0);
        assert_eq!(MapFlags::read_write().bits() & VM_PAGE_EXECUTABLE, 0);
    }
}
