//! Reading the start of video memory. The CPU can't reach most of it, so the copy engine brings
//! a page out to host memory the GPU can write.

use crate::rm::{Handle, Rm};
use crate::{NvidiaBackend, memory_error};
use guesthouse_backend::{BackendError, HostServices, SpaceId};
use std::ptr;

const PAGE_SIZE: usize = 0x1000;

/// The page the copies land in, kept for good as these are rare and it's small.
#[derive(Debug)]
pub(crate) struct PeekPage {
    memory: Handle,
    address: *mut u8,
}

impl<R: Rm> NvidiaBackend<R> {
    fn peek_page(&mut self) -> Result<(Handle, *mut u8), BackendError> {
        if let Some(page) = &self.peek_page {
            return Ok((page.memory, page.address));
        }
        let protection = libc::PROT_READ | libc::PROT_WRITE;
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
        // SAFETY: a new anonymous mapping touches nothing that exists.
        let mapped = unsafe { libc::mmap(ptr::null_mut(), PAGE_SIZE, protection, flags, -1, 0) };
        if mapped == libc::MAP_FAILED {
            return Err(BackendError::new("no memory for a page to read video memory through"));
        }
        let address = mapped.cast::<u8>();
        let memory = self.device.alloc_host_pages(address as u64, PAGE_SIZE as u64).map_err(memory_error)?;
        self.peek_page = Some(PeekPage { memory, address });
        Ok((memory, address))
    }

    pub(crate) fn peek(
        &mut self,
        host: &mut dyn HostServices,
        own_space: SpaceId,
        from: Handle,
        out: &mut [u8],
    ) -> Result<(), BackendError> {
        if out.len() > PAGE_SIZE {
            return Err(BackendError::new("that's more than one page"));
        }
        let (page, address) = self.peek_page()?;
        // SAFETY: the page is ours and stays mapped, and the GPU only writes it during the copy below,
        // which `move_contents` waits for. Clearing means a copy that failed reads as nothing, not as
        // the last thing read.
        unsafe { ptr::write_bytes(address, 0, PAGE_SIZE) };
        self.move_contents(host, own_space, from, page, PAGE_SIZE as u64)?;
        for (index, byte) in out.iter_mut().enumerate() {
            // SAFETY: `index` is inside the page. Volatile, as the compiler can't see the GPU's write.
            *byte = unsafe { ptr::read_volatile(address.add(index)) };
        }
        Ok(())
    }
}
