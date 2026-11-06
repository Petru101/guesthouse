//! Handlers for sync objects and fences.

use super::{Handled, Session, answer_status};
use crate::deferred::DeferredAction;
use crate::fences::{FENCE_PAGE, Fence, FenceBlock};
use guesthouse_backend::{Backend, BackendError, GuestMemory};
use guesthouse_messages::Status;
use guesthouse_messages::notice::Notice;
use guesthouse_messages::sync::{
    CreateSyncObject, KIND_CPU_NOTIFICATION, KIND_MONITORED_FENCE, SignalFence, first_handle, open_answer,
    opened_sync_object, paging_queue_answer,
};
use guesthouse_messages::wire::{Malformed, put_u32};

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// All fence pages as one block, made the first time something needs it.
    pub(crate) fn fence_block(&mut self) -> Result<FenceBlock, BackendError> {
        if let Some(block) = self.fences.block {
            return Ok(block);
        }
        let host_address = self.memory.host_address(self.io_space_start);
        let memory = self.backend.adopt_host_pages(host_address, crate::FENCE_AREA)?;
        let gpu_address = self.take_gpu_addresses(crate::FENCE_AREA, crate::ALLOCATION_ALIGN)?;
        let block = FenceBlock { memory, gpu_address };
        self.fences.block = Some(block);
        Ok(block)
    }

    fn fresh_fence_page(&mut self) -> Result<u64, BackendError> {
        Ok(self.io_space_start + self.fences.fresh_page()?)
    }

    /// A zeroed fence page, reusing freed ones.
    pub(crate) fn take_fence(&mut self) -> Result<Fence, BackendError> {
        if let Some(fence) = self.fences.reuse() {
            self.memory.zero(fence.0, FENCE_PAGE);
            return Ok(fence);
        }
        let fence = self.fresh_fence_page()?;
        let block = self.fence_block()?;
        Ok((fence, block.gpu_address + fence - self.io_space_start))
    }

    pub(super) fn create_paging_queue(&mut self, out: &mut Vec<u8>) -> Handled {
        let queue = self.handles.take();
        let sync = self.handles.take();
        let fence = match self.fences.reuse_paging() {
            Some(fence) => {
                self.memory.zero(fence, FENCE_PAGE);
                fence
            }
            None => self.fresh_fence_page()?,
        };
        self.paging_queues.insert(queue, fence);
        paging_queue_answer(queue, sync, fence, out);
        Ok(())
    }

    pub(super) fn destroy_paging_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        if let Some(fence) = self.paging_queues.remove(&first_handle(message)?) {
            self.fences.give_back_paging(fence);
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn create_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateSyncObject::parse(message)?;
        let handle = self.handles.take();
        let monitored = request.kind == KIND_MONITORED_FENCE;
        let (fence, gpu_address) = if monitored { self.take_fence()? } else { (0, 0) };
        let event = if request.kind == KIND_CPU_NOTIFICATION {
            Some(request.initial.ok_or(Malformed)?)
        } else {
            None
        };
        self.sync_objects.insert(handle, fence, gpu_address, event);
        if monitored {
            // Some swapchains start their fences above zero, and wait for that before any signal.
            self.memory.write_u64(fence, request.initial.ok_or(Malformed)?);
        }
        CreateSyncObject::answer(handle, gpu_address, fence, out);
        Ok(())
    }

    pub(super) fn open_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let existing = opened_sync_object(message)?;
        if self.sync_objects.get(existing).is_none() {
            open_answer(0, Status::INVALID_PARAMETER, 0, 0, out);
            return Ok(());
        }
        let handle = self.handles.take();
        let object = self.sync_objects.open(existing, handle).expect("checked just above");
        open_answer(handle, Status::SUCCESS, object.gpu_address, object.fence, out);
        Ok(())
    }

    pub(super) fn destroy_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let closed = self.sync_objects.close(first_handle(message)?);
        if let Some(object) = closed.filter(|object| object.fence != 0) {
            self.defer(DeferredAction::ReturnFence((object.fence, object.gpu_address)));
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn create_shared_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let object = first_handle(message)?;
        let handle = self.handles.take();
        self.shared_objects.insert(handle, object);
        put_u32(out, handle);
        put_u32(out, 0);
        Ok(())
    }

    pub(super) fn destroy_shared_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        self.shared_objects.remove(&first_handle(message)?);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    fn resolve_shared(&self, handle: u32) -> u32 {
        self.shared_objects.get(&handle).copied().unwrap_or(handle)
    }

    /// Raises a fence, or wakes a notification's event. Fences never go backwards.
    pub(crate) fn write_fence(&mut self, handle: u32, value: u64) {
        let Some(&object) = self.sync_objects.get(self.resolve_shared(handle)) else {
            return;
        };
        if let Some(event) = object.event {
            self.notices.push(Notice::WakeEvent { event, release: 0 });
        } else if object.fence != 0 && value > self.fence_value(object.fence) {
            self.memory.write_u64(object.fence, value);
        }
    }

    pub(crate) fn fence_reached(&self, handle: u32, value: u64) -> bool {
        let fence = match self.sync_objects.get(self.resolve_shared(handle)) {
            Some(object) => object.fence,
            None => self.queue_progress.get(&handle).copied().unwrap_or(0),
        };
        fence == 0 || self.fence_value(fence) >= value
    }

    pub(super) fn signal_fence(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let signal = SignalFence::parse(message)?;
        self.write_fence(signal.handle, signal.value);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }
}
