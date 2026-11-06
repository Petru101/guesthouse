//! What changes with a GPU's architecture. Class numbers are in the open kernel module's headers.

use super::guest::{ChannelObject, GuestBranch};

/// One class bound to a channel, and the size of its parameters.
pub type ChannelClass = (u32, usize);

/// An engine's index in the RM's list of its kind, and the node type the guest's driver knows it by.
pub type IndexedNode = (usize, u32);

/// The node type of each engine that gets a node, by engine. An engine that isn't here gets no node,
/// so the guest doesn't see it and nothing reaches it.
#[derive(Debug, PartialEq, Eq)]
pub struct EngineNodes {
    pub decode: &'static [IndexedNode],
    pub encode: &'static [IndexedNode],
    pub optical_flow: &'static [IndexedNode],
    pub jpeg: &'static [IndexedNode],
}

/// The record's flags for video. All zero, the guest's video runtimes offer no decoders.
#[derive(Debug, PartialEq, Eq)]
pub struct VideoRecordFlags {
    pub words: [u32; 2],
    pub decoder: u8,
}

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
    pub nodes: EngineNodes,
    /// What every copy node's record says. Each bit can go alone, but a guest with none of them
    /// failed its Vulkan test.
    pub copy_node_flags: u32,
    pub video_record_flags: VideoRecordFlags,
}

const BLACKWELL_DMA_COPY_B: u32 = 0xCAB5;
const BLACKWELL_B: u32 = 0xCE97;
const BLACKWELL_COMPUTE_B: u32 = 0xCEC0;
const BLACKWELL_INLINE_TO_MEMORY_A: u32 = 0xCD40;
const FERMI_TWOD_A: u32 = 0x902D;
const DEFERRED_API: u32 = super::super::rm::classes::DEFERRED_API;

/// With the first word's bit clear, Direct3D 11 offers no video decoders at all. Without the second
/// word a decoder can be made, but its first frame fails as out of memory. Either bit of the byte is
/// enough, and with it at zero, making a decoder fails as out of memory.
const BLACKWELL_VIDEO_FLAG: u32 = 1 << 20;
const BLACKWELL_VIDEO_FRAMES: u32 = 0x40_0030;
const BLACKWELL_DECODER_FLAGS: u8 = 0x48;

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
    nodes: EngineNodes {
        decode: &[(0, 0x1A), (3, 0x22)],
        encode: &[(1, 0x14), (2, 0x15), (3, 0x36)],
        optical_flow: &[(0, 0x21)],
        jpeg: &[(3, 0x27)],
    },
    copy_node_flags: 0x1A0,
    video_record_flags: VideoRecordFlags {
        words: [BLACKWELL_VIDEO_FLAG, BLACKWELL_VIDEO_FRAMES],
        decoder: BLACKWELL_DECODER_FLAGS,
    },
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
