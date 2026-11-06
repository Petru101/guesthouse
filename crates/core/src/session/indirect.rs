//! The indirect display driver's messages: the second producer of frames, beside copies to the
//! screen. The state is in `frames/indirect.rs`.
//!
//! The driver names no allocations, as its graphics API keeps those from it. So a ring is found by
//! its stamps: the first bytes of each slot say which slot it is. Everything else a message says is
//! checked against what the host handed out, and any process in the guest can send these.

use super::{Handled, HeldReply, Session, services};
use crate::frames::{
    FrameLight, GuestSlot, IndirectRing, Landing, LiveRing, Picture, Registering, ReplyWaiting, Slots,
};
use guesthouse_backend::{Backend, BackendError, GuestMemory, SpaceId};
use guesthouse_messages::indirect::{FrameDone, Outcome, Register, Reply, Request, STAMP_SIZE, Stamp};
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

/// How long a ring's stamps get to reach memory, and a frame's pass to finish. A GPU that takes
/// longer has bigger problems, and the guest's thread is waiting for the first of these.
const QUEUE_WAIT: Duration = Duration::from_secs(2);
/// Memory is handed out in units, so an allocation can be a little larger than the buffer asked for.
const SLOT_SLACK: u64 = 2 << 20;
/// How many allocations of about the right size are worth reading. Each costs a short copy.
const MOST_CANDIDATES: usize = 32;

impl<B: Backend, M: GuestMemory> Session<B, M> {
    fn reply(&self, outcome: Outcome, next_slot: Option<u32>) -> Reply {
        let viewer = self.frames.viewer_facts;
        Reply {
            outcome,
            next_slot,
            refresh_millihertz: viewer.refresh_millihertz,
            peak_nits: viewer.peak_nits,
        }
    }

    /// Answers a message of ours in `out`, which holds it. One that makes no sense goes back as it came.
    pub(super) fn indirect_message(&mut self, process: u32, out: &mut Vec<u8>) -> Handled {
        let Ok(request) = Request::parse(out) else {
            log::warn!("a message from the display driver in process {process:#x} makes no sense");
            return Ok(());
        };
        match request {
            Request::Register(ring) => self.register_ring(process, ring, out),
            Request::Frame(done) => {
                let reply = self.frame_done(process, &done);
                reply.write_over(out);
            }
            Request::Close(name) => {
                if self.frames.owns_ring(process, name) {
                    self.end_indirect_ring("the driver closed it");
                }
                self.reply(Outcome::Accepted, None).write_over(out);
            }
        }
        Ok(())
    }

    fn ring_makes_sense(ring: &Register) -> bool {
        let Some(picture) = Picture::sized(ring.width, ring.height) else { return false };
        picture.pitch == ring.pitch && picture.bytes() == ring.slot_size
    }

    fn register_ring(&mut self, process: u32, ring: Register, out: &mut Vec<u8>) {
        let taken = self.frames.registering.is_some()
            || self
                .frames
                .indirect
                .as_ref()
                .is_some_and(|live| live.owner != process || live.name.monitor != ring.name.monitor);
        let space = self.spaces.get(&process).copied();
        let (Some(ticket), Some(space), false, true) =
            (self.ticket, space, taken, Self::ring_makes_sense(&ring))
        else {
            log::warn!("refused a frame ring from process {process:#x}: {ring:?}, already taken: {taken}");
            self.reply(Outcome::Refused, None).write_over(out);
            return;
        };
        self.end_indirect_ring("the driver made a new one");
        let queues: Vec<u32> = self.queues_in(space);
        let targets = self.queue_targets(queues.into_iter());
        let reply = ReplyWaiting { ticket, bytes: std::mem::take(out) };
        let deadline = Instant::now() + QUEUE_WAIT;
        self.frames.registering = Some(Registering { reply, process, ring, space, targets, deadline });
        self.hold_reply();
    }

    fn queues_in(&self, space: SpaceId) -> Vec<u32> {
        self.queues.iter().filter(|(_, queue)| queue.space == space).map(|(&handle, _)| handle).collect()
    }

    /// Everything of the right size this process can reach, newest first. Other processes' surfaces
    /// it has opened are here too, which is why a size alone can't say what's a slot.
    fn slot_candidates(&self, ring: &Register, space: SpaceId) -> Vec<GuestSlot> {
        let fits = |size: u64| size >= ring.slot_size && size - ring.slot_size <= SLOT_SLACK;
        let mut found: Vec<GuestSlot> = self
            .allocations
            .iter()
            .filter(|(_, allocation)| {
                fits(allocation.size)
                    && allocation.guest_address.is_none()
                    && allocation.view.is_none()
                    && allocation.mappings.iter().any(|mapping| mapping.space == space)
            })
            .filter_map(|(&handle, allocation)| {
                let memory = allocation.memory?;
                Some(GuestSlot { allocation: handle, memory, size: allocation.size })
            })
            .collect();
        found.sort_unstable_by_key(|slot| std::cmp::Reverse(slot.allocation));
        found.truncate(MOST_CANDIDATES);
        found
    }

    /// Finds the buffer behind each slot by its stamp. All of them, or the ring can't work.
    fn find_slots(&mut self, ring: &Register, space: SpaceId) -> Result<Vec<GuestSlot>, BackendError> {
        let own_space = self.own_space()?;
        let mut slots = vec![None; ring.slots as usize];
        for candidate in self.slot_candidates(ring, space) {
            let mut bytes = [0; STAMP_SIZE];
            let host = &mut services!(self);
            self.backend.read_memory_start(host, own_space, candidate.memory, &mut bytes)?;
            let Some(stamp) = Stamp::parse(&bytes) else { continue };
            if stamp.generation == ring.name.generation
                && let Some(place) = slots.get_mut(stamp.slot as usize)
            {
                *place = Some(candidate);
            }
        }
        let found: Option<Vec<GuestSlot>> = slots.into_iter().collect();
        found.ok_or(BackendError::new("the ring's stamps aren't all there"))
    }

    /// The stamps have reached memory, or never will.
    fn finish_registering(&mut self, registering: Registering, reached: bool) {
        let Registering { reply: waiting, process, ring, space, .. } = registering;
        let slots = if reached {
            self.find_slots(&ring, space)
        } else {
            Err(BackendError::new("the guest's queues never reached the stamps"))
        };
        let reply = match slots {
            Ok(slots) => self.open_indirect_ring(process, &ring, slots),
            Err(error) => {
                log::warn!("no ring for the display driver: {error}");
                self.reply(Outcome::Refused, None)
            }
        };
        let ReplyWaiting { ticket, mut bytes } = waiting;
        reply.write_over(&mut bytes);
        self.replies.push(HeldReply { ticket, bytes });
    }

    fn open_indirect_ring(&mut self, process: u32, ring: &Register, slots: Vec<GuestSlot>) -> Reply {
        let Some(picture) = Picture::sized(ring.width, ring.height) else {
            return self.reply(Outcome::Refused, None);
        };
        // Whatever the screen's copies were doing, this is the screen now.
        self.frames.retire_ring();
        let Ok(space) = self.own_space() else { return self.reply(Outcome::Refused, None) };
        self.frames.generation = self.frames.generation.wrapping_add(1);
        let count = slots.len();
        let slot_size = slots.iter().map(|slot| slot.size).min().unwrap_or(ring.slot_size);
        let mut states = Slots::new(ring.slots);
        let granted = states.claim();
        self.frames.ring = Some(LiveRing {
            generation: self.frames.generation,
            picture,
            look: ring.look,
            slot_size,
            space,
            memory: Vec::new(),
            slots: states,
            unsent: None,
        });
        self.frames.indirect = Some(IndirectRing {
            owner: process,
            name: ring.name,
            slots,
            granted,
            landing: VecDeque::new(),
            light: vec![None; count],
            needs_export: true,
            export_refused: false,
        });
        log::info!("the display driver has a ring: {ring:?}");
        self.reply(Outcome::Accepted, granted)
    }

    /// The guest may have moved or dropped a buffer. Its memory is what the viewer holds, so the
    /// ring has to go with it.
    fn ring_memory_is_intact(&self) -> bool {
        let same = |slot: &GuestSlot| {
            self.allocations
                .get(&slot.allocation)
                .is_some_and(|allocation| allocation.memory == Some(slot.memory))
        };
        self.frames.indirect.as_ref().is_some_and(|ring| ring.slots.iter().all(same))
    }

    fn frame_done(&mut self, process: u32, done: &FrameDone) -> Reply {
        if !self.frames.owns_ring(process, done.name) {
            return self.reply(Outcome::StaleGeneration, None);
        }
        if !self.ring_memory_is_intact() {
            self.end_indirect_ring("its memory has changed");
            return self.reply(Outcome::StaleGeneration, None);
        }
        if let Some(slot) = done.slot
            && !self.frame_written(process, slot, done)
        {
            self.end_indirect_ring("the driver wrote a slot it wasn't given");
            return self.reply(Outcome::StaleGeneration, None);
        }
        let next = self.grant_slot();
        self.reply(Outcome::Accepted, next)
    }

    /// Says whether the driver had this slot. The frame is finished once its queues are.
    fn frame_written(&mut self, process: u32, slot: u32, done: &FrameDone) -> bool {
        let Some(space) = self.spaces.get(&process).copied() else { return false };
        let queues = self.queues_in(space);
        let targets = self.queue_targets(queues.into_iter());
        let Some(ring) = self.frames.indirect.as_mut() else { return false };
        if ring.granted != Some(slot) {
            return false;
        }
        ring.granted = None;
        let light = FrameLight { sdr_white_nits: done.sdr_white_nits, mastering: done.mastering };
        ring.landing.push_back(Landing { slot, targets, deadline: Instant::now() + QUEUE_WAIT, light });
        true
    }

    /// The slot the driver holds, or a new one if there's one free.
    fn grant_slot(&mut self) -> Option<u32> {
        let frames = &mut self.frames;
        let (Some(indirect), Some(ring)) = (frames.indirect.as_mut(), frames.ring.as_mut()) else {
            return None;
        };
        if indirect.granted.is_none() {
            indirect.granted = ring.slots.claim();
            frames.counts.skipped += u64::from(indirect.granted.is_none());
        }
        indirect.granted
    }

    /// The ring is over. Its memory is the guest's, and the viewer's descriptors keep what it needs.
    pub(super) fn end_indirect_ring(&mut self, why: &str) {
        if self.frames.indirect.take().is_some() {
            log::info!("the display driver's ring has ended: {why}");
            self.frames.ring = None;
            self.frames.behind = true;
        }
    }

    pub(super) fn allocation_going(&mut self, handle: u32) {
        if self.frames.indirect.as_ref().is_some_and(|ring| ring.holds(handle)) {
            self.end_indirect_ring("the guest destroyed a slot");
        }
    }

    pub(super) fn process_going(&mut self, process: u32) {
        if self.frames.indirect.as_ref().is_some_and(|ring| ring.owner == process) {
            self.end_indirect_ring("its process has gone");
        }
    }

    /// Everything here that waits on the GPU or the main loop.
    pub(super) fn service_indirect(&mut self, now: Instant) {
        self.finish_waiting_register(now);
        self.land_frames(now);
        self.export_for_viewer();
    }

    fn finish_waiting_register(&mut self, now: Instant) {
        let Some(registering) = &self.frames.registering else { return };
        let reached = self.targets_reached(&registering.targets);
        let late = now >= registering.deadline;
        // Reading a stamp needs the copy engine to itself.
        if (reached && self.copies.is_empty()) || late {
            let registering = self.frames.registering.take().expect("looked at above");
            self.finish_registering(registering, reached && self.copies.is_empty());
        }
    }

    fn land_frames(&mut self, now: Instant) {
        loop {
            let Some(ring) = self.frames.indirect.as_ref() else { return };
            let Some(front) = ring.landing.front() else { return };
            let reached = self.targets_reached(&front.targets);
            if !reached && now < front.deadline {
                return;
            }
            let frames = &mut self.frames;
            let (Some(indirect), Some(ring)) = (frames.indirect.as_mut(), frames.ring.as_mut()) else {
                return;
            };
            let Some(landing) = indirect.landing.pop_front() else { return };
            if reached {
                frames.sequence += 1;
                let replaced = ring.slots.finished(landing.slot, frames.sequence);
                indirect.light[landing.slot as usize] = Some(landing.light);
                frames.counts.copied += 1;
                frames.counts.replaced += u64::from(replaced);
            } else {
                ring.slots.abandoned(landing.slot);
                log::warn!("a frame from the display driver never finished");
            }
        }
    }

    /// Gives a viewer its own descriptors for the guest's buffers.
    fn export_for_viewer(&mut self) {
        let wanted = self.frames.viewer
            && self.frames.indirect.as_ref().is_some_and(|ring| ring.needs_export && !ring.export_refused);
        if !wanted {
            return;
        }
        let Some(indirect) = self.frames.indirect.as_ref() else { return };
        let slots = indirect.slots.clone();
        let buffers: Result<Vec<OwnedFd>, BackendError> =
            slots.iter().map(|slot| self.backend.export_memory(slot.memory, slot.size)).collect();
        let frames = &mut self.frames;
        let (Some(indirect), Some(ring)) = (frames.indirect.as_mut(), frames.ring.as_mut()) else { return };
        match buffers {
            Ok(buffers) => {
                ring.unsent = Some(buffers);
                indirect.needs_export = false;
            }
            Err(error) => {
                log::warn!("the display driver's slots can't be exported: {error}");
                indirect.export_refused = true;
            }
        }
    }
}
