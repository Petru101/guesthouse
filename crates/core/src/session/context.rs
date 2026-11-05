//! Handlers for processes, contexts, queues and work.

use super::{Handled, Session, answer_status, services};
use crate::deferred::DeferredAction;
use crate::tables::Queue;
use guesthouse_backend::{Backend, GuestMemory, SpaceId, StagingCopy, UnmapRequest, WorkSource};
use guesthouse_messages::queue::{
    CopyToStaging, CreateContext, CreateHardwareQueue, SubmitToContext, SubmitToQueue,
};
use guesthouse_messages::sync::first_handle;
use guesthouse_messages::wire::{Malformed, put_u32, u8_at};
use guesthouse_messages::{MessageHeader, Status};

/// Where a new process's flags are, and the flag for a 32-bit one.
const PROCESS_FLAGS: usize = 0x232;
const PROCESS_32_BIT: u8 = 4;

impl<B: Backend, M: GuestMemory> Session<B, M> {
    pub(super) fn create_process(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let handle = self.handles.take();
        if u8_at(message, PROCESS_FLAGS)? & PROCESS_32_BIT != 0 {
            self.processes_32_bit.insert(handle);
        }
        put_u32(out, handle);
        Ok(())
    }

    /// The address space closes at the next chance to run deferred work, not now.
    pub(super) fn destroy_process(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let process = MessageHeader::process_of(message)?;
        self.processes_32_bit.remove(&process);
        if let Some(space) = self.spaces.remove(&process) {
            let action = DeferredAction::CloseSpace(space);
            self.deferred.push(crate::deferred::Deferred { waits: Vec::new(), action });
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// Frees an address space, and anything still running or mapped there.
    pub(crate) fn close_space(&mut self, space: SpaceId) {
        let owned: Vec<u32> =
            self.queues.iter().filter(|(_, queue)| queue.space == space).map(|(&handle, _)| handle).collect();
        owned.into_iter().for_each(|queue| self.close_queue(queue));
        for allocation in self.allocations.values_mut() {
            let Some(memory) = allocation.memory else { continue };
            allocation.mappings.retain(|mapping| {
                if mapping.space != space {
                    return true;
                }
                let unmap = UnmapRequest { space, memory, address: mapping.address, length: mapping.length };
                if let Err(error) = self.backend.unmap(&unmap) {
                    log::warn!("closing an address space: {error}");
                }
                false
            });
        }
        self.closed_spaces.insert(space);
        if let Some(block) = self.fences.block {
            let unmap = UnmapRequest {
                space,
                memory: block.memory,
                address: block.gpu_address,
                length: crate::FENCE_AREA,
            };
            if let Err(error) = self.backend.unmap(&unmap) {
                log::warn!("closing an address space: {error}");
            }
        }
        self.backend.destroy_address_space(space);
    }

    pub(super) fn create_context(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateContext::parse(message)?;
        let handle = self.handles.take();
        self.context_nodes.insert(handle, request.node);
        request.answer(message, handle, out);
        Ok(())
    }

    /// An app that exits destroys its contexts and never names their queues.
    pub(super) fn destroy_context(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let context = first_handle(message)?;
        for queue in self.context_queues.get(&context).cloned().unwrap_or_default() {
            self.close_queue(queue);
        }
        self.context_queues.remove(&context);
        self.context_nodes.remove(&context);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn create_hardware_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateHardwareQueue::parse(message)?;
        request.begin_answer(message, out);
        let queue = self.handles.take();
        let progress = self.handles.take();
        let (fence, fence_gpu_address) = self.take_fence()?;
        let space = self.space_for(MessageHeader::process_of(message)?)?;
        let record = Queue {
            fence,
            fence_gpu_address,
            space,
            context: request.context,
            last_fence: None,
            failed: false,
        };
        self.queues.insert(queue, record);
        self.context_queues.entry(request.context).or_default().push(queue);
        self.queue_progress.insert(progress, fence);
        self.queues_made += 1;
        self.backend.finish_queue_data(&mut out[CreateHardwareQueue::PRIVATE..], self.queues_made)?;
        CreateHardwareQueue::finish_answer(queue, progress, fence, fence_gpu_address, out);
        Ok(())
    }

    pub(super) fn destroy_hardware_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        self.close_queue(first_handle(message)?);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(crate) fn close_queue(&mut self, handle: u32) {
        let closed = self.queues.remove(&handle);
        self.gates.remove(&handle);
        for queues in self.context_queues.values_mut() {
            queues.retain(|&queue| queue != handle);
        }
        self.backend.close_queue(handle);
        if let Some(queue) = closed {
            self.fences.give_back((queue.fence, queue.fence_gpu_address));
        }
    }

    pub(super) fn submit_to_hardware_queue(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        self.run_deferred();
        let request = SubmitToQueue::parse(message)?;
        let started =
            self.start_on_gpu(request.queue, WorkSource::HardwareQueue, request.private, request.fence_value);
        if !started && let Some(queue) = self.queues.get(&request.queue) {
            // Nothing ran, so stand in for the GPU where the guest would notice.
            self.memory.write_u64(queue.fence, request.fence_value);
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// A context that has no queue gets one here, so what waits for it can treat it like any other.
    pub(super) fn submit_to_context(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = SubmitToContext::parse(message)?;
        if self.context_queues.get(&request.context).is_none_or(Vec::is_empty) {
            let handle = self.handles.take();
            let (fence, fence_gpu_address) = self.take_fence()?;
            let space = self.space_for(MessageHeader::process_of(message)?)?;
            let context = request.context;
            let record = Queue { fence, fence_gpu_address, space, context, last_fence: None, failed: false };
            self.queues.insert(handle, record);
            self.context_queues.entry(context).or_default().push(handle);
        }
        let queue = self.context_queues[&request.context][0];
        let record = *self.queues.get(&queue).ok_or(Malformed)?;
        let value = record.last_fence.unwrap_or(0) + 1;
        if !self.start_on_gpu(queue, WorkSource::Context, request.private, value) {
            self.memory.write_u64(record.fence, value);
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// A paravirtual GPU has no screen, so the guest's kernel reads frames from a staging surface. A
    /// surface the host cannot describe cannot be copied.
    pub(super) fn copy_to_staging(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CopyToStaging::parse(message)?;
        let shapes = |handle| self.allocations.get(&handle).and_then(|allocation| allocation.surface);
        let (Some(source), Some(target)) = (shapes(request.source), shapes(request.target)) else {
            answer_status(Status::INVALID_PARAMETER, out);
            return Ok(());
        };
        let target_guest_address = self.allocations[&request.target].guest_address;
        // The kernel's display queue sends no private data, and an app's present always has some.
        let is_screen = target_guest_address.is_some() && request.private_size == 0;
        let targets = self.queue_targets(std::iter::once(request.context));
        let source_address = self.surface_address(request.source)?;
        let target_address = self.surface_address(request.target)?;
        self.wait_for_frame(&targets);
        let rectangle = request.source_rectangle;
        let copy = StagingCopy {
            source,
            target,
            source_address,
            target_address,
            left: rectangle.left,
            top: rectangle.top,
            right: rectangle.right,
            bottom: rectangle.bottom,
            target_left: request.target_left,
            target_top: request.target_top,
            target_guest_address,
            is_screen,
        };
        let status = self.backend.copy_to_staging(&mut services!(self), &copy);
        answer_status(status, out);
        Ok(())
    }
}
