//! Copies to the staging surface, which the guest's display reads.
//!
//! A paravirtual GPU has no screen, so the guest's kernel reads frames from a staging surface. A copy
//! must wait for its frame, and the copy itself takes time, and neither may stop the message
//! thread. So a copy that can't finish at once becomes a job here, and its reply is held.
//!
//! The guest thread that sent it waits for the reply, so nothing that thread does can get ahead
//! of the copy. Three things keep the rest in order: jobs run one at a time, in order; the queues
//! it reads are held until it is done, as their next frame may draw into this image; and a message
//! that frees or moves a surface in use is parked until the jobs are done.

use super::{Answer, Handled, HeldReply, ReplyTicket, Session, answer_status, services};
use crate::pending::{GateStep, Pending, Target};
use guesthouse_backend::{Backend, GuestMemory, StagingCopy, StagingStart};
use guesthouse_messages::Status;
use guesthouse_messages::queue::CopyToStaging;
use guesthouse_messages::wire::put_u32;
use std::collections::VecDeque;
use std::time::Instant;

/// A copy that has not finished.
#[derive(Debug)]
pub(crate) struct CopyJob {
    id: u64,
    ticket: Option<ReplyTicket>,
    copy: StagingCopy,
    handles: [u32; 2],
    targets: Vec<Target>,
    deadline: Instant,
    held_since: Instant,
    running: bool,
}

/// A message that has to wait for the copies.
#[derive(Debug)]
pub(crate) struct Parked {
    message: Vec<u8>,
    ticket: Option<ReplyTicket>,
}

impl<B: Backend, M: GuestMemory> Session<B, M> {
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
        let context = std::iter::once(request.context);
        let targets = self.queue_targets(context.clone());
        let source_address = self.surface_address(request.source)?;
        let target_address = self.surface_address(request.target)?;
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
            own_space: self.own_space()?,
        };
        let now = Instant::now();
        let deadline = now + self.config.frame_wait;
        if self.copies.is_empty() && (self.targets_reached(&targets) || now >= deadline) {
            if let StagingStart::Finished(status) = self.begin_copy(&copy) {
                self.end_copy(&copy, status);
                answer_status(status, out);
                return Ok(());
            }
            self.add_job(request.context, [request.source, request.target], copy, targets, deadline, true);
        } else {
            self.add_job(request.context, [request.source, request.target], copy, targets, deadline, false);
        }
        self.hold_reply();
        Ok(())
    }

    fn add_job(
        &mut self,
        context: u32,
        handles: [u32; 2],
        copy: StagingCopy,
        targets: Vec<Target>,
        deadline: Instant,
        running: bool,
    ) {
        let id = self.next_copy;
        self.next_copy += 1;
        for queue in self.live_queues_of(context) {
            let gate = self.gates.entry(queue);
            let gate = gate.or_insert_with(|| {
                self.pending.push(Pending::Gate { queue });
                VecDeque::new()
            });
            gate.push_back(GateStep::Copy { id });
        }
        let ticket = self.ticket;
        let held_since = Instant::now();
        self.copies.push_back(CopyJob { id, ticket, copy, handles, targets, deadline, held_since, running });
    }

    pub(crate) fn copy_is_active(&self, id: u64) -> bool {
        self.copies.iter().any(|job| job.id == id)
    }

    fn begin_copy(&mut self, copy: &StagingCopy) -> StagingStart {
        if let (true, Some(sink), Some(address)) =
            (copy.is_screen, self.frame_sink.as_deref_mut(), copy.target_guest_address)
        {
            self.staging_frames.copy_starting(sink, copy.target, address);
        }
        self.backend.start_staging_copy(&mut services!(self), copy)
    }

    fn end_copy(&mut self, copy: &StagingCopy, status: Status) {
        if let (true, Status::SUCCESS, Some(sink)) = (copy.is_screen, status, self.frame_sink.as_deref_mut())
        {
            self.staging_frames.copy_finished(sink);
        }
    }

    /// Moves the first job along, and the ones after it if that frees them to go.
    pub(super) fn poll_copies(&mut self, now: Instant) {
        while let Some(mut job) = self.copies.pop_front() {
            let status = if job.running {
                self.backend.poll_staging_copy(&mut services!(self), now)
            } else if self.targets_reached(&job.targets) || now >= job.deadline {
                job.running = true;
                match self.begin_copy(&job.copy) {
                    StagingStart::Finished(status) => Some(status),
                    StagingStart::Running => None,
                }
            } else {
                None
            };
            let Some(status) = status else {
                self.copies.push_front(job);
                return;
            };
            self.end_copy(&job.copy, status);
            self.timings.reply_released(job.held_since.elapsed());
            if let Some(ticket) = job.ticket {
                let mut bytes = Vec::with_capacity(4);
                put_u32(&mut bytes, status.0);
                self.replies.push(HeldReply { ticket, bytes });
            }
        }
        self.run_parked();
    }

    /// Whether a running or waiting copy uses this allocation.
    pub(super) fn copy_uses(&self, handle: u32) -> bool {
        self.copies.iter().any(|job| job.handles.contains(&handle))
    }

    /// Sets a message aside until the copies are done. It runs again from the start then.
    pub(super) fn park(&mut self, message: &[u8]) {
        self.parked.push_back(Parked { message: message.to_vec(), ticket: self.ticket });
        self.hold_reply();
    }

    fn run_parked(&mut self) {
        let mut out = Vec::new();
        for parked in std::mem::take(&mut self.parked) {
            let answer = self.handle_message(&parked.message, parked.ticket, &mut out);
            if let (Answer::Now, Some(ticket)) = (answer, parked.ticket) {
                self.replies.push(HeldReply { ticket, bytes: out.clone() });
            }
        }
    }
}
