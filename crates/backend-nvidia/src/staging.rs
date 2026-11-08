//! The daemon's own copies: frames to the staging surface and into the frame ring, and an allocation's
//! contents when it moves.

use crate::channel::copy::{RING_ENTRIES, Rows, Tiled, TiledSide};
use crate::channel::{AREA_SIZE, COPY_AREA_EXTRA, COPY_WINDOW, Channel, ChannelPlace, CopyChannel, Engine};
use crate::rm::device::Mapping;
use crate::rm::{Handle, Rm, RmError};
use crate::{CHANNEL_ADDRESS_ALIGN, NvidiaBackend, memory_error};
use guesthouse_backend::{
    BackendError, COPY_PIXEL_BYTES, HostServices, SlotCopy, SpaceId, StagingCopy, StagingStart, Status,
    SurfaceShape,
};
use std::time::Instant;

const SMALL_PAGE: u64 = 0x1000;
/// What a guest's 10-bit surfaces carry here, as a Direct3D 11 `R10G10B10A2` swap chain's do. The ring
/// holds 8-bit pictures, and a plain copy can't narrow one.
const SURFACE_COLOUR_10_BIT: u32 = 0x36;
/// What a surface of 16-bit floats carries, as an HDR desktop's does.
const SURFACE_COLOUR_16_BIT_FLOAT: u32 = 0x29;
const FLOAT_PIXEL_BYTES: u32 = 8;

/// How wide a surface's pixels are. A colour not known here counts as four bytes.
pub(crate) fn pixel_bytes(surface: &SurfaceShape) -> u32 {
    if surface.colour == SURFACE_COLOUR_16_BIT_FLOAT { FLOAT_PIXEL_BYTES } else { COPY_PIXEL_BYTES }
}

/// One side of a copy: where it starts, its pitch, and the tiling the engine has to know about. A
/// linear side starts at the rectangle, and a tiled side starts at the surface.
fn copy_side(
    address: u64,
    surface: &SurfaceShape,
    x: i64,
    y: i64,
    pixel: i64,
) -> Option<(u64, u32, Option<TiledSide>)> {
    let x_bytes = x.checked_mul(pixel)?;
    let Some(block) = surface.tiling else {
        let offset = y.checked_mul(i64::from(surface.pitch))?.checked_add(x_bytes)?;
        return Some((address.checked_add_signed(offset)?, surface.pitch, None));
    };
    let side = TiledSide {
        x: u32::try_from(x_bytes).ok()?,
        y: u32::try_from(y).ok()?,
        surface_width: surface.pitch,
        surface_height: surface.height,
        block,
    };
    Some((address, 0, Some(side)))
}

/// The copy channel, and where it lives.
#[derive(Debug)]
pub struct Copier {
    channel: CopyChannel,
    space: SpaceId,
    area: u64,
    /// True while a frame's copy is under way.
    running: bool,
    /// One retry on a new channel, as a fault in one app can take this channel with it, and every
    /// frame on the screen goes through it.
    retried: bool,
}

impl<R: Rm> NvidiaBackend<R> {
    fn make_copy_channel(&mut self, area: u64, space: SpaceId) -> Result<CopyChannel, RmError> {
        let place = ChannelPlace {
            ring_address: area,
            ring_entries: RING_ENTRIES,
            area_address: area,
            area_size: AREA_SIZE,
        };
        let Some(address_space) = self.spaces.get(&space) else {
            return Err(RmError::System("the daemon's address space is closed".to_owned()));
        };
        let generation = self.generation;
        let sizes = self.host_branch;
        Channel::new(&mut self.device, generation, sizes, address_space, place, Engine::Copy)
            .map(CopyChannel::new)
    }

    /// The copy channel, made the first time something needs it.
    fn copier(&mut self, host: &mut dyn HostServices, space: SpaceId) -> Result<&mut Copier, BackendError> {
        if self.copier.is_none() {
            let size = AREA_SIZE as u64 + COPY_AREA_EXTRA;
            let area = host.take_gpu_addresses(size, CHANNEL_ADDRESS_ALIGN)?;
            let channel = self.make_copy_channel(area, space).map_err(memory_error)?;
            self.copier = Some(Copier { channel, space, area, running: false, retried: false });
        }
        Ok(self.copier.as_mut().expect("made just above"))
    }

    /// The core has checked these numbers against the memory, so `None` here means something is
    /// wrong in that check, and no copy is better than one built from a number cut down to size.
    fn batch_for(channel: &mut CopyChannel, copy: &StagingCopy) -> Option<Vec<u32>> {
        let pixel = i64::from(pixel_bytes(&copy.source));
        let (left, top) = (i64::from(copy.left), i64::from(copy.top));
        let width = u32::try_from((i64::from(copy.right) - left).checked_mul(pixel)?).ok()?;
        let count = u32::try_from(i64::from(copy.bottom) - top).ok()?;
        let (source, source_pitch, source_side) =
            copy_side(copy.source_address, &copy.source, left, top, pixel)?;
        let (target_left, target_top) = (i64::from(copy.target_left), i64::from(copy.target_top));
        let (target, target_pitch, target_side) =
            copy_side(copy.target_address, &copy.target, target_left, target_top, pixel)?;
        let rows = Rows { source, target, source_pitch, target_pitch, width, count };
        if source_side.is_none() && target_side.is_none() {
            return Some(channel.rows(&rows));
        }
        channel.tiled(&Tiled { rows, source: source_side, target: target_side })
    }

    /// The batch last started, for a test on a fake.
    #[cfg(any(test, feature = "fake-rm"))]
    pub fn last_copy_words(&self) -> &[u32] {
        &self.copier.as_ref().expect("a copy has run").channel.last_words
    }

    /// Finishes the copy under way, for a test on a fake.
    #[cfg(any(test, feature = "fake-rm"))]
    pub fn pretend_copy_finished(&self) {
        self.copier.as_ref().expect("a copy is under way").channel.pretend_finished();
    }

    /// Drops host memory from the L2 on the copy channel, for a submission with no free ring entry
    /// ahead of it. It has to finish before the doorbell, so this waits, which the ring's way doesn't.
    /// Until the first frame there's no copy channel, and the submission goes without.
    pub(crate) fn invalidate_and_wait(&mut self) {
        let Some(copier) = self.copier.as_mut() else {
            log::warn!("no copy channel for an L2 invalidate");
            return;
        };
        if copier.running && !copier.channel.wait() {
            log::warn!("no L2 invalidate, as a copy is stuck: {}", copier.channel.state());
            return;
        }
        let words = copier.channel.l2_invalidate();
        copier.channel.start(&mut self.device, words);
        if !copier.channel.wait() {
            log::warn!("an L2 invalidate never finished: {}", copier.channel.state());
        }
    }

    pub(crate) fn start_copy(&mut self, host: &mut dyn HostServices, copy: &StagingCopy) -> StagingStart {
        let copier = match self.copier(host, copy.own_space) {
            Ok(copier) => copier,
            Err(error) => {
                log::warn!("no copy channel: {error}");
                return StagingStart::Finished(Status::UNSUCCESSFUL);
            }
        };
        let Some(words) = Self::batch_for(&mut copier.channel, copy) else {
            log::warn!("a copy's numbers don't fit its batch: {copy:?}");
            return StagingStart::Finished(Status::UNSUCCESSFUL);
        };
        copier.running = true;
        copier.retried = false;
        let channel = &mut self.copier.as_mut().expect("read above").channel;
        channel.start(&mut self.device, words);
        StagingStart::Running
    }

    fn slot_batch_for(channel: &mut CopyChannel, copy: &SlotCopy) -> Option<Vec<u32>> {
        let width = copy.width.checked_mul(COPY_PIXEL_BYTES)?;
        let rows = |source_pitch| Rows {
            source: copy.source_address,
            target: copy.slot_address,
            source_pitch,
            target_pitch: copy.slot_pitch,
            width,
            count: copy.height,
        };
        match copy.source.tiling {
            None => Some(channel.rows(&rows(copy.source.pitch))),
            Some(block) => channel.tiled(&Tiled {
                rows: rows(0),
                source: Some(TiledSide {
                    x: 0,
                    y: 0,
                    surface_width: copy.source.pitch,
                    surface_height: copy.source.height,
                    block,
                }),
                target: None,
            }),
        }
    }

    pub(crate) fn start_frame_copy(&mut self, host: &mut dyn HostServices, copy: &SlotCopy) -> StagingStart {
        if copy.source.colour == SURFACE_COLOUR_10_BIT {
            return StagingStart::Finished(Status::UNSUCCESSFUL);
        }
        let copier = match self.copier(host, copy.own_space) {
            Ok(copier) => copier,
            Err(error) => {
                log::warn!("no copy channel: {error}");
                return StagingStart::Finished(Status::UNSUCCESSFUL);
            }
        };
        let Some(words) = Self::slot_batch_for(&mut copier.channel, copy) else {
            log::warn!("a picture's numbers don't fit its batch: {copy:?}");
            return StagingStart::Finished(Status::UNSUCCESSFUL);
        };
        copier.running = true;
        copier.retried = false;
        let channel = &mut self.copier.as_mut().expect("read above").channel;
        channel.start(&mut self.device, words);
        StagingStart::Running
    }

    /// Replaces a dead channel, and runs its last batch again on the new one.
    fn retry_copy(&mut self) -> Result<(), RmError> {
        let mut copier = self.copier.take().expect("a copy is running");
        let mut words = std::mem::take(&mut copier.channel.last_words);
        if let Some(space) = self.spaces.get(&copier.space)
            && let Err(error) = copier.channel.channel.close(&mut self.device, space)
        {
            log::warn!("closing the old copy channel: {error}");
        }
        let mut channel = self.make_copy_channel(copier.area, copier.space)?;
        channel.renumber(&mut words);
        channel.start(&mut self.device, words);
        self.copier = Some(Copier { channel, retried: true, running: true, ..copier });
        Ok(())
    }

    pub(crate) fn poll_copy(&mut self, _host: &mut dyn HostServices, now: Instant) -> Option<Status> {
        let copier = self.copier.as_mut()?;
        if copier.channel.is_finished() {
            copier.running = false;
            return Some(Status::SUCCESS);
        }
        if !copier.channel.has_timed_out(now) {
            return None;
        }
        log::warn!("a copy timed out: {}", copier.channel.state());
        if copier.retried {
            log::warn!("the copy failed on a new channel too, so the guest gets a failure");
            copier.running = false;
            return Some(Status::UNSUCCESSFUL);
        }
        log::warn!("the copy channel stopped answering, so it is being replaced");
        match self.retry_copy() {
            Ok(()) => {
                let channel = &self.copier.as_ref().expect("made just above").channel;
                log::warn!("the copy runs again on a new channel: {}", channel.state());
                None
            }
            Err(error) => {
                log::warn!("no new copy channel: {error}");
                Some(Status::UNSUCCESSFUL)
            }
        }
    }

    /// Copies what the GPU wrote into an allocation over to its new memory.
    pub(crate) fn move_contents(
        &mut self,
        host: &mut dyn HostServices,
        own_space: SpaceId,
        from: Handle,
        to: Handle,
        size: u64,
    ) -> Result<(), BackendError> {
        if size > COPY_WINDOW {
            log::warn!("{size:#x} bytes is too big to keep");
            return Ok(());
        }
        let area = self.copier(host, own_space)?.area;
        let (window, source) = (area + AREA_SIZE as u64, area + AREA_SIZE as u64 + COPY_WINDOW);
        let length = size.next_multiple_of(SMALL_PAGE);
        let space = self.spaces.get(&own_space).ok_or_else(|| BackendError::new("no address space"))?.clone();
        let map = Mapping { length, small_pages: true, ..Mapping::default() };
        self.device.map(&space, &Mapping { memory: to, address: window, ..map }).map_err(memory_error)?;
        if let Err(error) = self.device.map(&space, &Mapping { memory: from, address: source, ..map }) {
            // The window is one place, so a mapping left in it would stop every later move.
            self.device.unmap(&space, to, window, length).map_err(memory_error)?;
            return Err(memory_error(error));
        }
        let copier = self.copier.as_mut().expect("made above");
        let words = copier.channel.bytes(source, window, size as u32);
        copier.channel.start(&mut self.device, words);
        if !copier.channel.wait() {
            log::warn!("a copy of {size:#x} bytes never finished");
        }
        self.device.unmap(&space, from, source, length).map_err(memory_error)?;
        self.device.unmap(&space, to, window, length).map_err(memory_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::CopyChannel;
    use crate::profile::{BLACKWELL, HOST_615};
    use crate::rm::device::Device;
    use crate::rm::fake::FakeRm;
    use guesthouse_backend::SurfaceShape;

    fn channel() -> CopyChannel {
        let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        let place = ChannelPlace {
            ring_address: 0x800_0000,
            ring_entries: RING_ENTRIES,
            area_address: 0x800_0000,
            area_size: AREA_SIZE,
        };
        let channel = Channel::new(&mut device, &BLACKWELL, &HOST_615, &space, place, Engine::Copy);
        CopyChannel::new(channel.unwrap())
    }

    fn slot_copy(tiling: Option<u32>) -> SlotCopy {
        let source = SurfaceShape {
            width: 100,
            height: 50,
            format: 21,
            pitch: 512,
            tiling,
            refresh: (60, 1),
            samples: (1, 0),
            colour: 0,
        };
        SlotCopy {
            source,
            source_address: 0x1000_0000,
            slot_address: 0x2000_0000,
            slot_pitch: 512,
            width: 100,
            height: 50,
            own_space: SpaceId(1),
        }
    }

    fn shape(tiling: Option<u32>, colour: u32, pitch: u32) -> SurfaceShape {
        SurfaceShape {
            width: 100,
            height: 50,
            format: 17,
            pitch,
            tiling,
            refresh: (60, 1),
            samples: (1, 0),
            colour,
        }
    }

    fn copy_between(source: SurfaceShape, target: SurfaceShape) -> StagingCopy {
        StagingCopy {
            source,
            target,
            source_address: 0x1000_0000,
            target_address: 0x2000_0000,
            left: 10,
            top: 20,
            right: 30,
            bottom: 25,
            target_left: 3,
            target_top: 4,
            target_guest_address: None,
            is_screen: false,
            own_space: SpaceId(1),
        }
    }

    fn batch(source: SurfaceShape, target: SurfaceShape) -> Option<Vec<u32>> {
        NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &copy_between(source, target))
    }

    #[test]
    fn a_rectangle_between_linear_surfaces_starts_where_it_should_on_each_side() {
        let words = batch(shape(None, 7, 512), shape(None, 7, 1024)).unwrap();
        let source = 0x1000_0000 + 20 * 512 + 10 * 4;
        let target = 0x2000_0000 + 4 * 1024 + 3 * 4;
        assert_eq!(words[3..11], [0, source, 0, target, 512, 1024, 80, 5]);
    }

    #[test]
    fn a_rectangle_out_of_a_tiled_surface_lands_where_it_should_in_rows() {
        let words = batch(shape(Some(4), 7, 512), shape(None, 7, 1024)).unwrap();
        assert_eq!(words[3..11], [0, 0x1000_0000, 0, 0x2000_0000 + 4 * 1024 + 3 * 4, 0, 1024, 80, 5]);
        assert_eq!(words[21], 40 | 20 << 16);
    }

    #[test]
    fn a_rectangle_into_a_tiled_surface_keeps_its_tiling() {
        let words = batch(shape(Some(4), 7, 512), shape(Some(4), 7, 512)).unwrap();
        assert_eq!(words[3..11], [0, 0x1000_0000, 0, 0x2000_0000, 0, 0, 80, 5], "whole surfaces, no pitch");
        assert_eq!(words[21], 40 | 20 << 16, "the source's corner");
        assert_eq!(words[28], 12 | 4 << 16, "the target's corner");
    }

    #[test]
    fn rows_can_go_into_a_tiled_surface() {
        let words = batch(shape(None, 7, 512), shape(Some(2), 7, 512)).unwrap();
        assert_eq!(words[3..11], [0, 0x1000_0000 + 20 * 512 + 10 * 4, 0, 0x2000_0000, 512, 0, 80, 5]);
        assert_eq!(words[16], 2 << 4 | 1 << 12);
    }

    #[test]
    fn a_rectangle_of_floats_counts_eight_bytes_a_pixel() {
        let wide = shape(Some(4), SURFACE_COLOUR_16_BIT_FLOAT, 1024);
        let words = batch(wide, wide).unwrap();
        assert_eq!(words[9], 160, "twenty pixels wide");
        assert_eq!(words[21], 80 | 20 << 16);
    }

    #[test]
    fn a_surface_of_floats_has_wide_pixels() {
        // Colour words and rows as a real guest has them, SDR and HDR, at sizes rows pad differently.
        for (colour, width, pitch, bytes) in [
            (0x7, 1920, 7680, 4),
            (0x7, 2560, 10240, 4),
            (0x7, 1366, 5504, 4),
            (0x29, 1920, 15360, 8),
            (0x29, 2560, 20480, 8),
            (0x29, 1366, 10944, 8),
        ] {
            let surface = SurfaceShape { width, ..shape(Some(4), colour, pitch) };
            assert_eq!(pixel_bytes(&surface), bytes);
            assert!(width * bytes <= pitch, "a row fits its pitch");
        }
    }

    #[test]
    fn a_colour_nobody_knows_is_four_bytes_however_long_its_rows() {
        assert_eq!(pixel_bytes(&shape(Some(4), 0xEE, 1 << 20)), 4);
    }

    #[test]
    fn a_number_past_its_field_makes_no_batch_for_a_tiled_target() {
        assert_eq!(batch(shape(None, 7, 512), shape(Some(9), 7, 512)), None);
        let far = StagingCopy {
            target_left: 0x1_0000,
            ..copy_between(shape(None, 7, 512), shape(Some(4), 7, 1 << 20))
        };
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &far), None);
    }

    #[test]
    fn a_linear_picture_goes_row_by_row_at_each_sides_pitch() {
        let copy = SlotCopy { slot_pitch: 768, ..slot_copy(None) };
        let words = NvidiaBackend::<FakeRm>::slot_batch_for(&mut channel(), &copy).unwrap();
        assert_eq!(words[3..11], [0, 0x1000_0000, 0, 0x2000_0000, 512, 768, 400, 50]);
    }

    #[test]
    fn a_tiled_picture_is_read_from_its_corner() {
        let words = NvidiaBackend::<FakeRm>::slot_batch_for(&mut channel(), &slot_copy(Some(3))).unwrap();
        assert_eq!(words[7..11], [0, 512, 400, 50], "the engine works out the source's own pitch");
        assert_eq!(words[17..19], [512, 50]);
        assert_eq!(words[21], 0);
    }

    fn staging_copy(right: i32, bottom: i32, tiling: Option<u32>) -> StagingCopy {
        let surface = slot_copy(tiling).source;
        StagingCopy {
            source: surface,
            target: SurfaceShape { tiling: None, ..surface },
            source_address: 0x1000_0000,
            target_address: 0x2000_0000,
            left: 0,
            top: 0,
            right,
            bottom,
            target_left: 0,
            target_top: 0,
            target_guest_address: None,
            is_screen: false,
            own_space: SpaceId(1),
        }
    }

    #[test]
    fn a_batch_says_what_the_rectangle_says() {
        let words = NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &staging_copy(100, 50, None)).unwrap();
        assert_eq!(words[9..11], [400, 50]);
    }

    #[test]
    fn a_row_too_wide_for_the_engines_field_makes_no_batch() {
        let copy = staging_copy(i32::MAX, 1, None);
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &copy), None);
        let copy = staging_copy(i32::MAX, 1, Some(3));
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &copy), None);
    }

    #[test]
    fn addresses_past_64_bits_make_no_batch() {
        let copy = StagingCopy { target_address: u64::MAX, target_top: 1, ..staging_copy(100, 50, None) };
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &copy), None);
    }

    #[test]
    fn a_tiled_corner_or_block_past_their_fields_makes_no_batch() {
        let wide = StagingCopy { left: 0x4000, right: 0x4001, ..staging_copy(100, 50, Some(3)) };
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &wide), None);
        let low = StagingCopy { top: 0x1_0000, bottom: 0x1_0001, ..staging_copy(100, 50, Some(3)) };
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &low), None);
        assert_eq!(NvidiaBackend::<FakeRm>::batch_for(&mut channel(), &staging_copy(100, 50, Some(6))), None);
        assert_eq!(NvidiaBackend::<FakeRm>::slot_batch_for(&mut channel(), &slot_copy(Some(0x13))), None);
    }

    #[test]
    fn a_picture_too_wide_for_the_engines_field_makes_no_batch() {
        let copy = SlotCopy { width: u32::MAX, ..slot_copy(None) };
        assert_eq!(NvidiaBackend::<FakeRm>::slot_batch_for(&mut channel(), &copy), None);
    }

    const AREA: u64 = 0x800_0000;

    /// Nothing but a place for the copy channel, as a move asks for no more of the host.
    struct AreaOnly;

    impl HostServices for AreaOnly {
        fn take_gpu_addresses(&mut self, _: u64, _: u64) -> Result<u64, BackendError> {
            Ok(AREA)
        }
        fn read_at_gpu_address(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
            None
        }
        fn shared_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::SharedSpan> {
            None
        }
        fn guest_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::GuestSpan> {
            None
        }
        fn read_guest_span(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
            None
        }
        fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
            false
        }
        fn write_u32(&mut self, _: u64, _: u32) -> Result<(), guesthouse_backend::MemoryError> {
            Ok(())
        }
        fn write_u64(&mut self, _: u64, _: u64) -> Result<(), guesthouse_backend::MemoryError> {
            Ok(())
        }
        fn read_guest(&self, _: u64, _: &mut [u8]) -> usize {
            0
        }
        fn fence_value(&self, _: u64) -> Result<u64, guesthouse_backend::MemoryError> {
            Ok(0)
        }
    }

    #[test]
    fn a_move_that_cannot_map_its_source_leaves_no_window_behind() {
        use guesthouse_backend::{Backend, GpuRange};
        let window = AREA + AREA_SIZE as u64;
        let mut rm = FakeRm::new();
        rm.refuse_map_at = Some(window + COPY_WINDOW);
        let mut backend = crate::fake_guest::fake_backend(rm);
        let range = GpuRange { start: 0x400_0000, end: 0x1_0000_0000 };
        let space = backend.create_address_space(&[range]).unwrap();
        let (from, _) = backend.allocate_video_memory(0x1000, guesthouse_backend::MemoryKind::PLAIN).unwrap();
        let (to, _) = backend.allocate_video_memory(0x1000, guesthouse_backend::MemoryKind::PLAIN).unwrap();
        let moved = backend.preserve_contents(&mut AreaOnly, space, from, to, 0x1000);
        assert!(moved.is_err());
        assert_eq!(backend.device().rm.memory_at(window), None);
    }
}
