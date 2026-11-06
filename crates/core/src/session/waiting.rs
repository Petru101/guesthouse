//! What waits: deferred work, things that need a fence to move, and queue gates.

use super::{Handled, Session, answer_status, services};
use crate::deferred::{Deferred, DeferredAction, FreedAllocation};
use crate::gpu_addresses::{ALLOCATION_ALIGN, AddressUse};
use crate::pending::{
    GATE_TIMEOUT, GateStep, MOST_QUEUE_FAILURES, Pending, QUEUE_TIMEOUT, QueueAction, Signal, Target,
};
use crate::tables::Queue;
use guesthouse_backend::{Backend, GuestMemory, UnmapRequest};
use guesthouse_messages::Status;
use guesthouse_messages::notice::Notice;
use guesthouse_messages::queue::PresentToken;
use guesthouse_messages::sync::{SignalSharedFence, SignalSyncObject, WaitOnCpu, WaitOnGpu};
use std::collections::VecDeque;
use std::thread;
use std::time::{Duration, Instant};

const FRAME_WAIT_STEP: Duration = Duration::from_micros(100);

impl<B: Backend, M: GuestMemory> Session<B, M> {
    /// Does something once the GPU has finished what it has been given so far, or now if it has.
    pub(crate) fn defer(&mut self, action: DeferredAction) {
        let waits: Vec<(u32, u64)> = self
            .queues
            .iter()
            .filter_map(|(&handle, queue)| {
                let value = queue.last_fence?;
                (!queue.failed && self.fence_value(queue.fence) < value).then_some((handle, value))
            })
            .collect();
        if waits.is_empty() {
            self.run_deferred_action(action);
        } else {
            self.deferred.push(Deferred { waits, action });
        }
    }

    /// Finishes whatever has stopped waiting. A queue that has gone or failed waits for nothing.
    pub(crate) fn run_deferred(&mut self) {
        if self.deferred.is_empty() {
            return;
        }
        for mut item in std::mem::take(&mut self.deferred) {
            item.waits.retain(|&(handle, value)| {
                self.queues
                    .get(&handle)
                    .is_some_and(|queue| !queue.failed && self.memory.read_u64(queue.fence) < value)
            });
            if item.waits.is_empty() {
                self.run_deferred_action(item.action);
            } else {
                self.deferred.push(item);
            }
        }
    }

    pub(crate) fn run_deferred_action(&mut self, action: DeferredAction) {
        match action {
            DeferredAction::ReturnFence(fence) => self.fences.give_back(fence),
            DeferredAction::FreeAllocation(freed) => self.free_allocation(freed),
            DeferredAction::CloseSpace(space) => self.close_space(space),
            DeferredAction::GiveBackReserved { address, size } => {
                self.gpu_addresses.give_back(address, size, ALLOCATION_ALIGN, AddressUse::Reserved);
            }
        }
    }

    pub(crate) fn free_allocation(&mut self, freed: FreedAllocation) {
        for mapping in &freed.mappings {
            if !self.closed_spaces.contains(&mapping.space) {
                let unmap = UnmapRequest {
                    space: mapping.space,
                    memory: freed.memory,
                    address: mapping.address,
                    length: mapping.length,
                };
                if let Err(error) = self.backend.unmap(&unmap) {
                    log::warn!("unmapping freed memory: {error}");
                }
            }
            self.give_back_mapping(mapping.address);
        }
        self.backend.release_memory(freed.memory);
        if let Some(view) = freed.view {
            self.memory.unmap_pages(view);
        }
        if let Some((address, size)) = freed.shared {
            self.guest_ranges.release(address, size);
        }
    }

    /// Calls `visit` with each queue behind a handle that may be a queue or a context, if the GPU has
    /// work from it, until `visit` says stop.
    fn visit_live_queues(&self, handle: u32, mut visit: impl FnMut(&Queue) -> bool) -> bool {
        let one = [handle];
        let queues = self.context_queues.get(&handle).map_or(&one[..], Vec::as_slice);
        queues.iter().filter_map(|queue| self.queues.get(queue)).all(|queue| {
            let live = queue.last_fence.is_some() && !queue.failed;
            !live || visit(queue)
        })
    }

    /// Where each queue's progress fence will be once it has done what it has been given.
    pub(crate) fn queue_targets(&self, handles: impl Iterator<Item = u32>) -> Vec<Target> {
        let mut targets = Vec::new();
        for handle in handles {
            self.visit_live_queues(handle, |queue| {
                targets.extend(queue.last_fence.map(|value| (queue.fence, value)));
                true
            });
        }
        targets
    }

    pub(crate) fn targets_reached(&self, targets: &[Target]) -> bool {
        targets.iter().all(|&(fence, value)| self.fence_value(fence) >= value)
    }

    /// Whether no queue behind these handles has work the GPU has not finished.
    fn queues_idle(&self, mut handles: impl Iterator<Item = u32>) -> bool {
        handles.all(|handle| {
            self.visit_live_queues(handle, |queue| {
                queue.last_fence.is_none_or(|value| self.fence_value(queue.fence) >= value)
            })
        })
    }

    fn after_queues(&mut self, handles: impl Iterator<Item = u32> + Clone, action: QueueAction) {
        if self.queues_idle(handles.clone()) {
            self.run_queue_action(action);
        } else {
            let targets = self.queue_targets(handles);
            let deadline = Instant::now() + QUEUE_TIMEOUT;
            self.pending.push(Pending::AfterQueues { targets, deadline, action });
        }
    }

    fn run_queue_action(&mut self, action: QueueAction) {
        match action {
            QueueAction::WakeEvent { event, release } => {
                self.notices.push(Notice::WakeEvent { event, release });
            }
            QueueAction::WriteFences(signals) => {
                signals.into_iter().for_each(|(handle, value)| self.write_fence(handle, value));
            }
            QueueAction::PresentDone(token) => self.notices.push(Notice::PresentDone { token }),
        }
    }

    /// Blocks the message thread, so only briefly, until a frame's queues have finished.
    pub(crate) fn wait_for_frame(&self, targets: &[Target]) {
        let deadline = Instant::now() + self.config.frame_wait;
        while !self.targets_reached(targets) && Instant::now() < deadline {
            thread::sleep(FRAME_WAIT_STEP);
        }
    }

    pub(super) fn signal_sync_object(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = SignalSyncObject::parse(message)?;
        let contexts = request.contexts.iter();
        if let Some(event) = request.cpu_event {
            // The guest passes its reference to the host here, so the event must be released.
            self.after_queues(contexts, QueueAction::WakeEvent { event, release: 1 });
        } else if self.queues_idle(contexts.clone()) {
            match request.values {
                Some(values) => request
                    .objects
                    .iter()
                    .zip(values.iter())
                    .for_each(|(handle, value)| self.write_fence(handle, value)),
                None => request.objects.iter().for_each(|handle| self.write_fence(handle, 0)),
            }
        } else {
            let values = request.values.map(|values| values.iter().collect::<Vec<_>>());
            let value_of = |index: usize| values.as_ref().map_or(0, |values| values[index]);
            let signals = request.objects.iter().enumerate();
            let signals = signals.map(|(index, handle)| (handle, value_of(index))).collect();
            self.after_queues(contexts, QueueAction::WriteFences(signals));
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    pub(super) fn signal_shared_fence(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = SignalSharedFence::parse(message)?;
        let action = QueueAction::WriteFences(vec![(request.handle, request.value)]);
        self.after_queues(std::iter::once(request.context), action);
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// The guest's compositor learns of a new frame when the token comes back, so it must not come
    /// back before the GPU gets here.
    pub(super) fn present_token(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = PresentToken::parse(message)?;
        self.after_queues(std::iter::once(request.context), QueueAction::PresentDone(request.token));
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    fn signals_finished(&self, signals: &[Signal], wake_on_any: bool) -> bool {
        let reached = |&(handle, value): &Signal| self.fence_reached(handle, value);
        if wake_on_any { signals.iter().any(reached) } else { signals.iter().all(reached) }
    }

    pub(super) fn wait_on_cpu(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = WaitOnCpu::parse(message)?;
        let release = u64::from(request.release_event);
        let signals: Vec<Signal> = request.objects.iter().zip(request.values.iter()).collect();
        if self.signals_finished(&signals, request.wake_on_any) {
            self.notices.push(Notice::WakeEvent { event: request.event, release });
        } else {
            let (wake_on_any, event) = (request.wake_on_any, request.event);
            self.pending.push(Pending::CpuWait { signals, wake_on_any, event, release });
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// The queue runs on the guest's own ring, so the only way to hold it is to hold its doorbells.
    pub(super) fn wait_on_gpu(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = WaitOnGpu::parse(message)?;
        let signals: Vec<Signal> = request.objects.iter().zip(request.values.iter()).collect();
        let handle = request.queue_or_context;
        let queues = self.context_queues.get(&handle).cloned().unwrap_or_else(|| vec![handle]);
        let deadline = Instant::now() + GATE_TIMEOUT;
        for queue in queues {
            if let Some(gate) = self.gates.get_mut(&queue) {
                gate.push_back(GateStep::Wait { signals: signals.clone(), deadline });
            } else if !self.signals_finished(&signals, false) {
                let step = GateStep::Wait { signals: signals.clone(), deadline };
                self.gates.insert(queue, VecDeque::from([step]));
                self.pending.push(Pending::Gate { queue });
            }
        }
        answer_status(Status::SUCCESS, out);
        Ok(())
    }

    /// Works through what a queue held back. True once nothing is left.
    fn run_gate(&mut self, queue: u32, now: Instant) -> bool {
        while let Some(step) = self.gates.get_mut(&queue).and_then(VecDeque::pop_front) {
            match step {
                GateStep::Wait { signals, deadline } => {
                    if !self.signals_finished(&signals, false) && now < deadline {
                        let held = GateStep::Wait { signals, deadline };
                        self.gates.get_mut(&queue).expect("just read").push_front(held);
                        return false;
                    }
                }
                GateStep::Ring { doorbell, fence, value } => self.ring(queue, doorbell, fence, value),
            }
        }
        self.gates.remove(&queue);
        true
    }

    /// Lets work run, or leaves it behind the queue's gate if there is one.
    pub(crate) fn ring_or_hold(&mut self, queue: u32, doorbell: u64, fence: u64, value: u64) {
        match self.gates.get_mut(&queue) {
            Some(gate) => gate.push_back(GateStep::Ring { doorbell, fence, value }),
            None => self.ring(queue, doorbell, fence, value),
        }
    }

    fn ring(&mut self, queue: u32, doorbell: u64, fence: u64, value: u64) {
        self.backend.ring_doorbell(queue, doorbell);
        let deadline = Instant::now() + QUEUE_TIMEOUT;
        self.pending.push(Pending::Watch { queue, fence, value, deadline });
    }

    /// Whether queues may still be given work.
    pub(crate) fn work_allowed(&self) -> bool {
        self.queue_failures < MOST_QUEUE_FAILURES
    }

    /// Gives up on a queue the GPU has stopped answering, and frees the guest to carry on.
    fn queue_timed_out(&mut self, handle: u32, fence: u64, value: u64) {
        log::error!("queue {handle:#x} did not reach fence value {value:#x} in time");
        if let Some(queue) = self.queues.get_mut(&handle)
            && !queue.failed
        {
            queue.failed = true;
            self.queue_failures += 1;
            if self.queue_failures == MOST_QUEUE_FAILURES {
                log::error!("too many queues have failed, so no more work goes to the GPU");
            }
        }
        self.memory.write_u64(fence, value);
    }

    /// Checks everything that is waiting. Returns true if anything still is, and the caller should
    /// call again soon. `now` decides deadlines, and tests pass their own.
    pub fn poll(&mut self, now: Instant) -> bool {
        // The two lists swap places, so polling does not allocate.
        let spare = std::mem::take(&mut self.pending_spare);
        let mut items = std::mem::replace(&mut self.pending, spare);
        for item in items.drain(..) {
            if let Some(item) = self.try_finish(item, now) {
                self.pending.push(item);
            }
        }
        self.pending_spare = items;
        !self.pending.is_empty()
    }

    /// Finishes an item if it can. Otherwise hands it back.
    fn try_finish(&mut self, item: Pending, now: Instant) -> Option<Pending> {
        match item {
            Pending::AfterQueues { targets, deadline, action } => {
                if self.targets_reached(&targets) || now > deadline {
                    self.run_queue_action(action);
                    None
                } else {
                    Some(Pending::AfterQueues { targets, deadline, action })
                }
            }
            Pending::CpuWait { signals, wake_on_any, event, release } => {
                if self.signals_finished(&signals, wake_on_any) {
                    self.notices.push(Notice::WakeEvent { event, release });
                    None
                } else {
                    Some(Pending::CpuWait { signals, wake_on_any, event, release })
                }
            }
            Pending::Gate { queue } => (!self.run_gate(queue, now)).then_some(Pending::Gate { queue }),
            Pending::Update { fence, value, space, operations } => {
                if self.fence_reached(fence, value) {
                    self.finish_update(&operations, space, fence, value);
                    None
                } else {
                    Some(Pending::Update { fence, value, space, operations })
                }
            }
            Pending::Watch { queue, fence, value, deadline } => {
                // A queue that has gone may have passed its fence page on to someone else.
                if !self.queues.contains_key(&queue) || self.fence_value(fence) >= value {
                    None
                } else if now > deadline {
                    self.queue_timed_out(queue, fence, value);
                    None
                } else {
                    Some(Pending::Watch { queue, fence, value, deadline })
                }
            }
        }
    }

    /// Hands a submission to the backend. True if the GPU now owns the fence.
    pub(crate) fn start_on_gpu(
        &mut self,
        queue: u32,
        source: guesthouse_backend::WorkSource,
        private: &[u8],
        value: u64,
    ) -> bool {
        let Some(&record) = self.queues.get(&queue) else {
            return false;
        };
        let work = guesthouse_backend::Work {
            source,
            queue,
            space: record.space,
            node: self.context_nodes.get(&record.context).copied().unwrap_or(0),
            private,
            fence: record.fence,
            fence_gpu_address: record.fence_gpu_address,
            fence_value: value,
            may_run: self.work_allowed(),
        };
        let start = self.backend.start_work(&mut services!(self), &work);
        let guesthouse_backend::WorkStart::Started { doorbell } = start else {
            return false;
        };
        self.work_started += 1;
        self.queues.get_mut(&queue).expect("read above").last_fence = Some(value);
        self.ring_or_hold(queue, doorbell, record.fence, value);
        true
    }
}
