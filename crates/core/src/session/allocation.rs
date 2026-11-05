//! Handlers for allocations and resources.

use super::{Handled, Session, answer_status};
use crate::deferred::{DeferredAction, FreedAllocation};
use crate::tables::Allocation;
use guesthouse_backend::{AllocationShape, Backend, GuestMemory, MapRequest, SurfaceShape, UnmapRequest};
use guesthouse_messages::Status;
use guesthouse_messages::address::PAGE_SIZE;
use guesthouse_messages::allocation::{
    CACHED_FOR_CPU, CreateAllocation, CreateAllocationAnswer, DestroyAllocation, OpenResource, PRIMARY,
    SetGuestPages, lock_answer, locked_allocation,
};
use guesthouse_messages::standard::StandardRecord;
use guesthouse_messages::wire::align_up;

/// The guest's own number for 32-bit colour.
const FORMAT_32_BIT: u32 = 21;
/// Standard surfaces fill whole units of this size in video memory.
const STANDARD_GRAIN: u64 = 64 << 10;
/// The colour word of a standard surface, which is never a vendor's special case.
const STANDARD_COLOUR: u32 = 0;

fn standard_shape(record: &StandardRecord) -> AllocationShape {
    AllocationShape {
        size: align_up(record.exact_size(), STANDARD_GRAIN),
        exact_size: record.exact_size(),
        surface: Some(SurfaceShape {
            width: record.width,
            height: record.height,
            format: FORMAT_32_BIT,
            pitch: record.pitch,
            tiling: None,
            refresh: record.refresh,
            samples: (1, 0),
            colour: STANDARD_COLOUR,
        }),
        gpu_cached: false,
    }
}

impl<B: Backend, M: GuestMemory> Session<B, M> {
    fn shape_of(&mut self, private: &[u8]) -> AllocationShape {
        match StandardRecord::parse(private) {
            Some(record) => standard_shape(&record),
            None => self.backend.describe_allocation(private),
        }
    }

    /// Guest memory does not count: it is not on the card.
    fn over_video_cap(&self, cap: u64, wanted: u64) -> bool {
        let used: u64 = self
            .allocations
            .values()
            .filter(|allocation| {
                allocation.memory.is_some() && allocation.guest_address.is_none() && allocation.view.is_none()
            })
            .map(|allocation| allocation.size)
            .sum();
        used + wanted > cap
    }

    pub(super) fn create_allocation(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = CreateAllocation::parse(message)?;
        let guest_backed = request.guest_backed();
        CreateAllocationAnswer::begin(request.count, out);
        if let (false, Some(cap)) = (guest_backed, self.config.limits.video_cap) {
            // All of it or none, as nothing has been made yet that would need taking back.
            let wanted = request.entries().map(|entry| self.shape_of(entry.private).size).sum();
            if self.over_video_cap(cap, wanted) {
                CreateAllocationAnswer::fail(Status::NO_VIDEO_MEMORY, out);
                return Ok(());
            }
        }
        let resource = self.handles.take();
        self.resources.insert(resource, Vec::with_capacity(request.count));
        CreateAllocationAnswer::set_resource(resource, out);
        for (index, entry) in request.entries().enumerate() {
            let shape = self.shape_of(entry.private);
            let handle = self.handles.take();
            let mut allocation = Allocation {
                resource,
                gpu_cached: shape.gpu_cached,
                surface: shape.surface,
                ..Allocation::default()
            };
            if guest_backed {
                // The size in the answer is how much memory the guest locks, so it must be exact.
                allocation.size = shape.exact_size;
                let pages = shape.exact_size.div_ceil(PAGE_SIZE) as usize;
                allocation.pending_pages = Some(vec![None; pages]);
            } else {
                let (memory, size) = self.backend.allocate_video_memory(shape.size)?;
                allocation.memory = Some(memory);
                allocation.size = size;
            }
            let flags = (entry.flags & PRIMARY) | CACHED_FOR_CPU;
            CreateAllocationAnswer::set_entry(index, handle, entry.private_size, flags, allocation.size, out);
            if let Some(surface) = shape.surface {
                let fields = [
                    surface.width,
                    surface.height,
                    surface.format,
                    surface.samples.0,
                    surface.samples.1,
                    surface.refresh.0,
                    surface.refresh.1,
                ];
                CreateAllocationAnswer::set_surface(index, fields, out);
            }
            let private_at = out.len();
            out.extend_from_slice(entry.private);
            if StandardRecord::parse(entry.private).is_none() {
                self.backend.finish_allocation_data(&mut out[private_at..], shape.size);
            }
            self.allocations.insert(handle, allocation);
            self.resources.get_mut(&resource).expect("made above").push(handle);
        }
        CreateAllocationAnswer::finish(out);
        Ok(())
    }

    pub(super) fn destroy_allocation(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = DestroyAllocation::parse(message)?;
        let resource = request.resource;
        answer_status(Status::SUCCESS, out);
        if let Some(users) = self.resource_users.get_mut(&resource)
            && *users > 1
        {
            *users -= 1;
            return Ok(());
        }
        self.resource_users.remove(&resource);
        let request = request.with_handles()?;
        // Each process sends this twice: once for the resource and once for its allocations by name.
        // The second must not free what a live resource still holds for someone else.
        let held_elsewhere = |handle: &u32| {
            self.allocations
                .get(handle)
                .is_some_and(|allocation| self.resources.contains_key(&allocation.resource))
        };
        let mut handles: Vec<u32> = request.handles.iter().filter(|handle| !held_elsewhere(handle)).collect();
        handles.extend(self.resources.remove(&resource).unwrap_or_default());
        self.run_deferred();
        for handle in handles {
            let Some(allocation) = self.allocations.remove(&handle) else { continue };
            let Some(memory) = allocation.memory else { continue };
            let freed = FreedAllocation {
                memory,
                mappings: allocation.mappings,
                view: allocation.view,
                shared: allocation.guest_address.map(|address| (address, allocation.size)),
            };
            self.defer(DeferredAction::FreeAllocation(freed));
        }
        Ok(())
    }

    /// Handles mean the same to every process here, so opening returns the existing ones and the
    /// resource counts its users.
    pub(super) fn open_resource(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = OpenResource::parse(message)?;
        let resource = self.shared_objects.get(&request.shared).copied().unwrap_or(request.shared);
        match self.resources.get(&resource).filter(|handles| handles.len() >= request.count) {
            Some(handles) => {
                request.answer(resource, &handles[..request.count], out);
                *self.resource_users.entry(resource).or_insert(1) += 1;
            }
            None => request.refuse(out),
        }
        Ok(())
    }

    /// Gives the guest's CPU access. The first lock moves the allocation from video memory to pages
    /// shared with the guest, at the same GPU addresses.
    pub(super) fn lock_for_cpu(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let handle = locked_allocation(message)?;
        let Some(allocation) = self.allocations.get(&handle) else {
            lock_answer(Status::INVALID_PARAMETER, 0, out);
            return Ok(());
        };
        let Some(memory) = allocation.memory else {
            lock_answer(Status::INVALID_PARAMETER, 0, out);
            return Ok(());
        };
        if let Some(address) = allocation.guest_address {
            lock_answer(Status::SUCCESS, address, out);
            return Ok(());
        }
        let size = allocation.size;
        let (fresh, limit) = (self.fresh_guest_memory(), self.memory.window_end());
        let Some((address, used_before)) = self.guest_ranges.take(size, fresh, limit) else {
            lock_answer(Status::NO_MEMORY, 0, out);
            return Ok(());
        };
        if used_before {
            // Another allocation's data must not reach this one.
            self.memory.zero(address, size);
        }
        let host_address = self.memory.host_address(address);
        let shared = self.backend.adopt_host_pages(host_address, align_up(size, PAGE_SIZE))?;
        let has_mappings = !self.allocations[&handle].mappings.is_empty();
        if self.work_started > 0 && has_mappings {
            let host = &mut super::services!(self);
            self.backend.preserve_contents(host, memory, shared, size)?;
        }
        let own = if has_mappings { Some(self.own_space()?) } else { None };
        let allocation = self.allocations.get_mut(&handle).expect("read above");
        for mapping in &allocation.mappings {
            let unmap = UnmapRequest {
                space: mapping.space,
                memory,
                address: mapping.address,
                length: mapping.length,
            };
            self.backend.unmap(&unmap)?;
            self.backend.map(&MapRequest {
                space: mapping.space,
                memory: shared,
                offset: mapping.first,
                length: mapping.length,
                address: mapping.address,
                small_pages: false,
                // The daemon's own copies carry no cache invalidates, so its mappings stay uncached.
                gpu_cached: Some(mapping.space) != own && allocation.gpu_cached,
            })?;
        }
        allocation.memory = Some(shared);
        allocation.guest_address = Some(address);
        self.backend.release_memory(memory);
        lock_answer(Status::SUCCESS, address, out);
        Ok(())
    }

    /// Long lists come in several messages, and the allocation is ready after the last.
    pub(super) fn set_guest_pages(&mut self, message: &[u8], out: &mut Vec<u8>) -> Handled {
        let request = SetGuestPages::parse(message)?;
        let pending = self.allocations.get_mut(&request.allocation).and_then(|a| a.pending_pages.as_mut());
        let Some(pending) = pending else {
            answer_status(Status::INVALID_PARAMETER, out);
            return Ok(());
        };
        let start = request.first.min(pending.len());
        let end = request.first.saturating_add(request.count).min(pending.len());
        pending.splice(start..end, request.pages()?.iter().map(Some));
        answer_status(Status::SUCCESS, out);
        if pending.iter().any(Option::is_none) {
            return Ok(());
        }
        let pages: Vec<u64> = pending.iter().flatten().copied().collect();
        let allocation = self.allocations.get_mut(&request.allocation).expect("found above");
        allocation.pending_pages = None;
        let view = self.memory.map_pages(&pages).map_err(|error| {
            guesthouse_backend::BackendError::new(format!("mapping guest pages: {error}"))
        })?;
        allocation.view = Some(view);
        allocation.memory = Some(self.backend.adopt_host_pages(view.host_address, view.size)?);
        Ok(())
    }
}
