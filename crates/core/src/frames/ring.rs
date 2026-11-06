//! The frame ring's state: the ring in use, the ones on their way out, and how far behind the
//! screen it is.
//!
//! A ring exists only while a viewer is here, so a guest with nobody watching pays nothing, and each
//! viewer gets buffers of its own. The ring is made and closed from the main loop, never during a
//! copy to the screen. So a frame that finds no ring is skipped, and remembered as being behind.

use super::IndirectRing;
use super::Registering;
use super::{Picture, Slots};
use guesthouse_backend::{MemoryId, SpaceId, SurfaceShape};
use guesthouse_messages::indirect::{Look, RingName};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

/// A viewer can hold three, one more is the newest, and the next frame needs one to go in.
pub(crate) const RING_SLOTS: u32 = 5;
/// How long the screen must have been still before the ring catches up by itself. A guest that's
/// drawing brings the next frame sooner than this, and that's the cheaper way to catch up.
const STILL_FOR: Duration = Duration::from_millis(20);

/// One slot's memory. These are the daemon's, and no message from the guest can name them.
#[derive(Debug)]
pub(crate) struct SlotMemory {
    pub memory: MemoryId,
    /// What the backend really gave, which is what's mapped.
    pub size: u64,
    pub address: u64,
}

/// A ring that exists.
#[derive(Debug)]
pub(crate) struct LiveRing {
    pub generation: u32,
    pub picture: Picture,
    pub look: Look,
    pub slot_size: u64,
    pub space: SpaceId,
    pub memory: Vec<SlotMemory>,
    pub slots: Slots,
    /// The buffers, until the sink has taken them.
    pub unsent: Option<Vec<OwnedFd>>,
}

/// A slot a copy is writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotInFlight {
    pub generation: u32,
    pub slot: u32,
}

/// The surface the guest's screen went to last, which always holds the whole picture.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Screen {
    pub staging: u32,
    pub surface: SurfaceShape,
    pub copied_at: Instant,
}

/// What became of the frames, to show in the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameCounts {
    /// Frames that reached a slot.
    pub copied: u64,
    /// Frames that left for the viewer.
    pub sent: u64,
    /// Frames that found no free slot.
    pub skipped: u64,
    /// Frames replaced by a newer one before they could leave.
    pub replaced: u64,
}

/// What a viewer said it can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewerFacts {
    /// It can show HDR, as well as the SDR every viewer can.
    pub shows_hdr: bool,
    /// 0 if not known.
    pub refresh_millihertz: u32,
    /// 0 if not known.
    pub peak_nits: u32,
}

/// See the module documentation.
#[derive(Debug, Default)]
pub(crate) struct ScreenFrames {
    pub viewer: bool,
    pub viewer_facts: ViewerFacts,
    /// The guest's display driver is the screen. Copies to the old screen are ignored meanwhile.
    pub indirect: Option<IndirectRing>,
    pub registering: Option<Registering>,
    pub ring: Option<LiveRing>,
    /// Rings to close, once nothing is writing them.
    pub retired: Vec<LiveRing>,
    pub screen: Option<Screen>,
    /// The ring doesn't have the newest picture.
    pub behind: bool,
    pub generation: u32,
    pub sequence: u64,
    /// A picture no ring could be made for, which isn't worth trying again.
    pub refused: Option<Picture>,
    /// One complaint for each ring is enough.
    pub complained: bool,
    pub counts: FrameCounts,
}

impl ScreenFrames {
    /// Whether the main loop has anything to do here.
    pub fn has_work(&self) -> bool {
        self.viewer || !self.retired.is_empty() || self.registering.is_some() || self.indirect.is_some()
    }

    /// The guest is waiting, or a frame is on its way out.
    pub fn is_urgent(&self) -> bool {
        self.registering.is_some() || self.indirect.as_ref().is_some_and(|ring| !ring.landing.is_empty())
    }

    pub fn owns_ring(&self, process: u32, name: RingName) -> bool {
        self.indirect.as_ref().is_some_and(|ring| ring.owner == process && ring.name == name)
    }

    /// Whether this viewer can show a look.
    pub fn viewer_can_show(&self, look: Look) -> bool {
        look == Look::SDR || self.viewer_facts.shows_hdr
    }

    /// The guest's screen has a new frame. A new size means the ring in use is the wrong one.
    pub fn screen_copied(&mut self, staging: u32, surface: SurfaceShape, now: Instant) {
        self.screen = Some(Screen { staging, surface, copied_at: now });
        let wrong_size = |ring: &LiveRing| Some(ring.picture) != Picture::of(&surface);
        if self.ring.as_ref().is_some_and(wrong_size) {
            self.retire_ring();
        }
    }

    pub fn retire_ring(&mut self) {
        self.behind = true;
        self.retired.extend(self.ring.take());
    }

    /// The picture a ring should be made for now, if it should.
    pub fn wanted_ring(&self) -> Option<Picture> {
        if !self.viewer || self.ring.is_some() || self.indirect.is_some() {
            return None;
        }
        let picture = Picture::of(&self.screen?.surface)?;
        (self.refused != Some(picture)).then_some(picture)
    }

    fn can_catch_up(&self) -> bool {
        let free = |ring: &LiveRing| ring.slots.has_free();
        self.viewer && self.behind && self.indirect.is_none() && self.ring.as_ref().is_some_and(free)
    }

    /// Whether the ring should fetch the picture for itself, as no frame is coming to bring it.
    pub fn should_catch_up(&self, now: Instant) -> bool {
        let still = |screen: Screen| now.saturating_duration_since(screen.copied_at) >= STILL_FOR;
        self.can_catch_up() && self.screen.is_some_and(still)
    }

    /// Whether the main loop must look again soon, with nothing else to wake it.
    pub fn wants_a_look(&self) -> bool {
        self.wanted_ring().is_some() || self.can_catch_up()
    }

    /// The ring of this generation, whether in use or on its way out.
    pub fn ring_of(&mut self, generation: u32) -> Option<&mut LiveRing> {
        let mut rings = self.ring.iter_mut().chain(&mut self.retired);
        rings.find(|ring| ring.generation == generation)
    }

    pub fn is_current(&self, generation: u32) -> bool {
        self.ring.as_ref().is_some_and(|ring| ring.generation == generation)
    }

    /// Says something once for each ring, so a card that refuses every frame doesn't fill the log.
    pub fn complain(&mut self, text: &str) {
        if !std::mem::replace(&mut self.complained, true) {
            log::warn!("{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(width: u32) -> SurfaceShape {
        SurfaceShape {
            width,
            height: 480,
            format: 21,
            pitch: width * 4,
            tiling: None,
            refresh: (60, 1),
            samples: (1, 0),
            colour: 0,
        }
    }

    fn ring(generation: u32, width: u32) -> LiveRing {
        LiveRing {
            generation,
            picture: Picture::of(&surface(width)).unwrap(),
            look: Look::SDR,
            slot_size: 0x10000,
            space: SpaceId(1),
            memory: Vec::new(),
            slots: Slots::new(RING_SLOTS),
            unsent: None,
        }
    }

    #[test]
    fn nobody_watching_means_no_ring_is_wanted() {
        let mut frames = ScreenFrames::default();
        frames.screen_copied(1, surface(640), Instant::now());
        assert_eq!(frames.wanted_ring(), None);
        assert!(!frames.has_work());
        frames.viewer = true;
        assert!(frames.wanted_ring().is_some());
    }

    #[test]
    fn a_viewer_waits_for_a_screen_to_size_the_ring_by() {
        let frames = ScreenFrames { viewer: true, ..ScreenFrames::default() };
        assert_eq!(frames.wanted_ring(), None);
    }

    #[test]
    fn a_new_size_retires_the_ring_and_the_same_size_keeps_it() {
        let mut frames = ScreenFrames { viewer: true, ..ScreenFrames::default() };
        frames.ring = Some(ring(1, 640));
        frames.screen_copied(1, surface(640), Instant::now());
        assert!(frames.ring.is_some());
        frames.screen_copied(2, surface(640), Instant::now());
        assert!(frames.ring.is_some(), "another surface of the same size is the same screen");
        frames.screen_copied(2, surface(800), Instant::now());
        assert!(frames.ring.is_none());
        assert_eq!(frames.retired.len(), 1);
        assert!(frames.behind);
    }

    #[test]
    fn a_picture_that_failed_is_not_tried_again() {
        let mut frames = ScreenFrames { viewer: true, ..ScreenFrames::default() };
        frames.screen_copied(1, surface(640), Instant::now());
        frames.refused = frames.wanted_ring();
        assert_eq!(frames.wanted_ring(), None);
        frames.screen_copied(1, surface(800), Instant::now());
        assert!(frames.wanted_ring().is_some(), "another size might work");
    }

    #[test]
    fn the_ring_catches_up_only_on_a_still_screen() {
        let mut frames = ScreenFrames { viewer: true, behind: true, ..ScreenFrames::default() };
        let start = Instant::now();
        frames.ring = Some(ring(1, 640));
        frames.screen_copied(1, surface(640), start);
        assert!(!frames.should_catch_up(start));
        assert!(frames.wants_a_look());
        assert!(frames.should_catch_up(start + STILL_FOR));
    }

    #[test]
    fn a_retired_ring_is_still_found_by_its_generation() {
        let mut frames = ScreenFrames { ring: Some(ring(2, 640)), ..ScreenFrames::default() };
        frames.retired.push(ring(1, 800));
        assert!(frames.ring_of(1).is_some());
        assert!(frames.ring_of(2).is_some());
        assert!(frames.ring_of(3).is_none());
        assert!(!frames.is_current(1));
        assert!(frames.is_current(2));
    }
}
