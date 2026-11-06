//! What changes with a GPU's architecture. Class numbers are in the open kernel module's headers.

use super::guest::{ChannelObject, GuestBranch};

/// One class bound to a channel, and the size of its parameters.
pub type ChannelClass = (u32, usize);

/// What a generation changes.
#[derive(Debug, PartialEq, Eq)]
pub struct Generation {
    pub name: &'static str,
    /// The architecture numbers the RM reports for this generation.
    pub architectures: &'static [u32],
    pub channel_class: u32,
    pub copy_class: u32,
    pub video_decode_class: u32,
    pub video_encode_class: u32,
    pub usermode_class: u32,
    /// What graphics channels bind, in subchannel order, which the guest's commands assume.
    pub graphics_classes: &'static [ChannelClass],
    /// The class of each object a guest's channel starts with.
    pub channel_objects: &'static [(ChannelObject, u32)],
    /// The shader instruction set the guest's user-mode drivers use, by the SM version the RM reports.
    /// An SM version that isn't here has never been run, so the backend refuses it and doesn't guess.
    pub shader_sets: &'static [(u32, u32)],
}

const BLACKWELL_DMA_COPY_B: u32 = 0xCAB5;
const BLACKWELL_B: u32 = 0xCE97;
const BLACKWELL_COMPUTE_B: u32 = 0xCEC0;
const BLACKWELL_INLINE_TO_MEMORY_A: u32 = 0xCD40;
const FERMI_TWOD_A: u32 = 0x902D;
const DEFERRED_API: u32 = super::super::rm::classes::DEFERRED_API;

/// Blackwell.
pub const BLACKWELL: Generation = Generation {
    name: "Blackwell",
    architectures: &[0x1A0, 0x1B0],
    channel_class: 0xCA6F,
    copy_class: BLACKWELL_DMA_COPY_B,
    video_decode_class: 0xCFB0,
    video_encode_class: 0xCFB7,
    usermode_class: 0xC761,
    graphics_classes: &[
        (BLACKWELL_B, 16),
        (BLACKWELL_COMPUTE_B, 16),
        (BLACKWELL_INLINE_TO_MEMORY_A, 16),
        (FERMI_TWOD_A, 16),
        (BLACKWELL_DMA_COPY_B, 8),
        (DEFERRED_API, 1),
    ],
    channel_objects: &[
        (ChannelObject::InlineToMemory, BLACKWELL_INLINE_TO_MEMORY_A),
        (ChannelObject::Graphics, BLACKWELL_B),
        (ChannelObject::TwoD, FERMI_TWOD_A),
    ],
    shader_sets: &[(0xA04, 0x9004)],
};

const GENERATIONS: [&Generation; 1] = [&BLACKWELL];

/// The profile for an architecture number, if this build has one.
pub fn generation_for(architecture: u32) -> Option<&'static Generation> {
    GENERATIONS.iter().copied().find(|generation| generation.architectures.contains(&architecture))
}

/// The class behind each handle the guest's driver uses, by (handle, class). The guest's branch gives
/// the handles, and the card's generation the classes.
pub fn guest_channel_classes(guest: &GuestBranch, generation: &Generation) -> Vec<(u32, u32)> {
    let class_of = |object| generation.channel_objects.iter().find(|known| known.0 == object);
    let found = guest.channel_handles.iter();
    found.filter_map(|&(handle, object)| class_of(object).map(|&(_, class)| (handle, class))).collect()
}
