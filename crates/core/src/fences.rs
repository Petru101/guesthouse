//! Fence pages: counters in shared memory that guest, host and GPU all read.

use guesthouse_backend::{BackendError, MemoryId};

/// Fence pages take the front of the guest's IO space, as one block.
pub const FENCE_AREA: u64 = 16 << 20;
/// Each fence gets a page of its own.
pub const FENCE_PAGE: u64 = 4096;

/// A fence's guest physical address and GPU address.
pub type Fence = (u64, u64);

/// The block of fence pages, mapped at one GPU address in every process. A process could move
/// another's fence, which is the one thing that crosses between address spaces, but they hold
/// only counters.
#[derive(Debug, Clone, Copy)]
pub struct FenceBlock {
    pub memory: MemoryId,
    pub gpu_address: u64,
}

/// Which pages are in use.
#[derive(Debug, Default)]
pub struct FencePool {
    next_page: u64,
    pub block: Option<FenceBlock>,
    free: Vec<Fence>,
    free_paging: Vec<u64>,
}

impl FencePool {
    pub fn new() -> Self {
        Self::default()
    }

    /// A page never used before, as an offset into the fence area.
    pub fn fresh_page(&mut self) -> Result<u64, BackendError> {
        if (self.next_page + 1) * FENCE_PAGE > FENCE_AREA {
            return Err(BackendError::new("out of fence pages"));
        }
        let offset = self.next_page * FENCE_PAGE;
        self.next_page += 1;
        Ok(offset)
    }

    pub fn reuse(&mut self) -> Option<Fence> {
        self.free.pop()
    }

    pub fn give_back(&mut self, fence: Fence) {
        self.free.push(fence);
    }

    pub fn reuse_paging(&mut self) -> Option<u64> {
        self.free_paging.pop()
    }

    pub fn give_back_paging(&mut self, fence: u64) {
        self.free_paging.push(fence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_recently_freed_fence_comes_back_first() {
        let mut pool = FencePool::new();
        pool.give_back((1, 10));
        pool.give_back((2, 20));
        assert_eq!(pool.reuse(), Some((2, 20)));
    }

    #[test]
    fn paging_fences_are_a_separate_pool() {
        let mut pool = FencePool::new();
        pool.give_back_paging(5);
        assert_eq!(pool.reuse(), None);
        assert_eq!(pool.reuse_paging(), Some(5));
    }

    #[test]
    fn the_area_runs_out() {
        let mut pool = FencePool::new();
        for _ in 0..FENCE_AREA / FENCE_PAGE {
            pool.fresh_page().unwrap();
        }
        assert!(pool.fresh_page().is_err());
    }
}
