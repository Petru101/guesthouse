//! Depth and stencil surfaces. On Turing, Ampere and Ada the depth unit only takes memory whose page kind
//! matches the surface's packing, and faults on anything else. The RM picks that kind when it's asked
//! for depth memory and told the packing, so a depth surface passes on the packing its driver chose.
//! Blackwell's RM gives such memory the plain kind anyway, so nothing changes there.

use guesthouse_backend::MemoryKind;

/// Bit 1 of the usage byte marks a surface the guest's driver will use for depth or stencil.
const USAGE_DEPTH_STENCIL: u8 = 1 << 1;

// The fields of the RM's attribute word that say how a depth surface is packed.
const DEPTH_SHIFT: u32 = 0;
const DEPTH_MASK: u32 = 7;
const Z_FLOAT_SHIFT: u32 = 18;
const PACKING_SHIFT: u32 = 19;
const PACKING_MASK: u32 = 7;
const DEPTH_UNKNOWN: u32 = 0;
const DEPTH_16: u32 = 2;
const DEPTH_32: u32 = 4;
const DEPTH_64: u32 = 5;
const Z_FLOAT: u32 = 1 << Z_FLOAT_SHIFT;

/// The packings the RM has a depth kind for. Any other leaves it with no kind, and it refuses. The
/// first, 8 bits deep, is stencil alone.
const FIXED_PACKINGS: [u32; 5] = [PACKING_Z24S8, PACKING_S8Z24, PACKING_Z24X8, PACKING_X8Z24, PACKING_Z16];
const FLOAT_PACKINGS: [u32; 2] = [PACKING_Z32, PACKING_Z32_X24S8];
const PACKING_Z24S8: u32 = 0;
const PACKING_S8Z24: u32 = 1;
const PACKING_Z32: u32 = 2;
const PACKING_Z24X8: u32 = 3;
const PACKING_X8Z24: u32 = 4;
const PACKING_Z32_X24S8: u32 = 5;
const PACKING_Z16: u32 = 7;

/// The attribute bits the RM chooses a depth kind from. Nothing else is passed on, so the guest's
/// requests for compression or Z culling don't reach it.
const KIND_BITS: u32 = DEPTH_MASK << DEPTH_SHIFT | 1 << Z_FLOAT_SHIFT | PACKING_MASK << PACKING_SHIFT;

const fn packed(packing: u32) -> u32 {
    packing << PACKING_SHIFT
}

// The guest's format byte for each depth format its driver makes, and the packing each one is laid
// out in. A typeless surface a program may bind for depth gets one of these too.
const FORMAT_Z16: u8 = 0x16;
const FORMAT_S8Z24: u8 = 0x17;
const FORMAT_Z32_FLOAT: u8 = 0x38;
const FORMAT_Z32_FLOAT_X24S8: u8 = 0x39;
const FORMAT_PACKINGS: [(u8, u32); 4] = [
    (FORMAT_Z16, packed(PACKING_Z16) | DEPTH_16),
    (FORMAT_S8Z24, packed(PACKING_S8Z24) | DEPTH_32),
    (FORMAT_Z32_FLOAT, Z_FLOAT | packed(PACKING_Z32) | DEPTH_32),
    (FORMAT_Z32_FLOAT_X24S8, Z_FLOAT | packed(PACKING_Z32_X24S8) | DEPTH_64),
];

/// Which surfaces a generation gives a depth kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthKinds {
    /// A surface its usage marks for depth, if it states its packing. Blackwell, whose RM gives
    /// plain memory for these anyway.
    AsStated,
    /// Every depth surface, found by its usage or its format, as the depth unit faults on any left
    /// plain. Small ones come with no usage, and one that states no packing gets its format's.
    /// Turing, Ampere and Ada.
    Always,
}

/// The kind bits of the packing an attribute word states, if the RM has a kind for it.
fn stated(attributes: u32) -> Option<u32> {
    let depth = attributes >> DEPTH_SHIFT & DEPTH_MASK;
    let packing = attributes >> PACKING_SHIFT & PACKING_MASK;
    let float = attributes >> Z_FLOAT_SHIFT & 1 == 1;
    let has_kind = match (depth, float) {
        (DEPTH_UNKNOWN, _) => false,
        (_, false) => FIXED_PACKINGS.contains(&packing),
        (_, true) => FLOAT_PACKINGS.contains(&packing),
    };
    has_kind.then_some(attributes & KIND_BITS)
}

/// What a surface's memory should be, from its usage byte, its format byte and the attribute word its
/// driver asked for. A depth surface that states its packing gets those bits, and where the rule is
/// [`DepthKinds::Always`], one that doesn't gets its format's. Anything else is plain memory, as
/// everything was before.
pub fn memory_kind(rule: DepthKinds, usage: u8, format: u8, attributes: u32) -> MemoryKind {
    let marked = usage & USAGE_DEPTH_STENCIL != 0;
    let format_bits = FORMAT_PACKINGS.iter().find(|known| known.0 == format).map(|known| known.1);
    let is_depth = match rule {
        DepthKinds::AsStated => marked,
        DepthKinds::Always => marked || format_bits.is_some(),
    };
    if !is_depth {
        if rule == DepthKinds::Always {
            // Says which surface it was, should the depth unit fault on one this didn't know.
            log::debug!(
                "a surface (usage {usage:#x}, format {format:#x}, attributes {attributes:#x}) is plain"
            );
        }
        return MemoryKind::PLAIN;
    }
    let chosen = match (stated(attributes), rule) {
        (Some(bits), _) => Some(bits),
        (None, DepthKinds::Always) => format_bits,
        (None, DepthKinds::AsStated) => None,
    };
    let said = format!("usage {usage:#x}, format {format:#x}, attributes {attributes:#x}");
    match chosen {
        Some(bits) => {
            log::debug!("a depth surface ({said}) gets depth memory with {bits:#x}");
            MemoryKind(bits)
        }
        None if rule == DepthKinds::Always => {
            log::warn!("a depth surface ({said}) has no depth kind here, so it gets plain memory");
            MemoryKind::PLAIN
        }
        None => {
            log::debug!("a depth surface ({said}) doesn't say its packing, so it gets plain memory");
            MemoryKind::PLAIN
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DepthKinds::{Always, AsStated};

    const RENDER_TARGET: u8 = 0x11;
    const DEPTH_STENCIL: u8 = 0x12;
    /// What a small depth surface carries, which the driver places itself.
    const NO_USAGE: u8 = 0;
    const BLOCK_LINEAR: u32 = 2 << 16;
    const COMPRESS_ANY: u32 = 2 << 12;
    const FOUR_SAMPLES: u32 = 3 << 4;
    const FORMAT_RGBA8: u8 = 0x20;
    const FORMAT_BGRA8: u8 = 0x07;
    /// The formats typeless surfaces get when nothing may bind them for depth.
    const FORMAT_R24G8_TYPELESS: u8 = 0x64;
    const FORMAT_R32G8X24_TYPELESS: u8 = 0x53;

    /// What Ada's driver asks for each depth format.
    const ADA_D16: u32 = 0x3A_0002;
    const ADA_D24S8: u32 = 0x0A_0004;
    const ADA_D32F: u32 = 0x16_0004;
    const ADA_D32FS8: u32 = 0x2E_0005;

    #[test]
    fn each_depth_format_from_ada_keeps_its_packing() {
        for (format, asked, kind) in [
            (FORMAT_Z16, ADA_D16, 0x38_0002),
            (FORMAT_S8Z24, ADA_D24S8, 0x08_0004),
            (FORMAT_Z32_FLOAT, ADA_D32F, 0x14_0004),
            (FORMAT_Z32_FLOAT_X24S8, ADA_D32FS8, 0x2C_0005),
        ] {
            for rule in [AsStated, Always] {
                assert_eq!(memory_kind(rule, DEPTH_STENCIL, format, asked), MemoryKind(kind), "{format:#x}");
            }
            let multisampled = asked | FOUR_SAMPLES | COMPRESS_ANY;
            assert_eq!(memory_kind(Always, DEPTH_STENCIL, format, multisampled), MemoryKind(kind));
        }
    }

    #[test]
    fn a_small_depth_surface_with_no_usage_still_gets_its_kind() {
        let small = ADA_D24S8 | 5 << 4;
        assert_eq!(memory_kind(Always, NO_USAGE, FORMAT_S8Z24, small), MemoryKind(0x08_0004));
        assert_eq!(memory_kind(Always, NO_USAGE, FORMAT_Z16, ADA_D16), MemoryKind(0x38_0002));
        assert_eq!(memory_kind(AsStated, NO_USAGE, FORMAT_S8Z24, small), MemoryKind::PLAIN);
    }

    #[test]
    fn a_depth_surface_that_does_not_say_its_packing_gets_its_formats() {
        // Blackwell's driver keeps stencil apart and asks this way.
        for (format, kind) in FORMAT_PACKINGS {
            assert_eq!(memory_kind(Always, DEPTH_STENCIL, format, BLOCK_LINEAR), MemoryKind(kind));
            assert_eq!(memory_kind(Always, NO_USAGE, format, BLOCK_LINEAR), MemoryKind(kind));
            assert_eq!(memory_kind(AsStated, DEPTH_STENCIL, format, BLOCK_LINEAR), MemoryKind::PLAIN);
        }
    }

    #[test]
    fn a_stated_packing_wins_over_the_formats() {
        let asked = packed(PACKING_X8Z24) | DEPTH_32;
        assert_eq!(memory_kind(Always, DEPTH_STENCIL, FORMAT_S8Z24, asked), MemoryKind(asked));
    }

    #[test]
    fn a_stencil_surface_is_8_bits_deep() {
        const DEPTH_8: u32 = 1;
        for rule in [AsStated, Always] {
            assert_eq!(memory_kind(rule, DEPTH_STENCIL, 0, BLOCK_LINEAR | DEPTH_8), MemoryKind(DEPTH_8));
        }
    }

    #[test]
    fn compression_and_the_rest_are_not_passed_on() {
        let asked = 0x3 << 23 | BLOCK_LINEAR | COMPRESS_ANY | 1 << 10 | packed(PACKING_S8Z24) | DEPTH_32;
        for rule in [AsStated, Always] {
            let kind = memory_kind(rule, DEPTH_STENCIL, FORMAT_S8Z24, asked);
            assert_eq!(kind, MemoryKind(packed(PACKING_S8Z24) | DEPTH_32));
        }
    }

    #[test]
    fn a_depth_surface_of_an_unknown_format_that_does_not_say_is_plain() {
        for rule in [AsStated, Always] {
            assert_eq!(memory_kind(rule, DEPTH_STENCIL, 0x99, BLOCK_LINEAR), MemoryKind::PLAIN);
        }
    }

    #[test]
    fn a_packing_the_rm_has_no_kind_for_is_not_a_stated_one() {
        let unknown = 0x99;
        let wrong =
            [packed(PACKING_Z32) | DEPTH_32, Z_FLOAT | packed(PACKING_Z16) | DEPTH_16, packed(6) | DEPTH_32];
        for asked in wrong {
            assert_eq!(memory_kind(AsStated, DEPTH_STENCIL, unknown, asked), MemoryKind::PLAIN);
            assert_eq!(memory_kind(Always, DEPTH_STENCIL, unknown, asked), MemoryKind::PLAIN);
        }
        let asked = packed(PACKING_Z32) | DEPTH_32;
        assert_eq!(memory_kind(Always, DEPTH_STENCIL, FORMAT_Z32_FLOAT, asked), MemoryKind(0x14_0004));
    }

    #[test]
    fn colour_surfaces_are_plain_whatever_they_ask() {
        let asked = BLOCK_LINEAR | COMPRESS_ANY | DEPTH_32;
        for rule in [AsStated, Always] {
            assert_eq!(memory_kind(rule, RENDER_TARGET, FORMAT_RGBA8, asked), MemoryKind::PLAIN);
            let z16 = packed(PACKING_Z16) | DEPTH_16;
            assert_eq!(memory_kind(rule, 0x10, FORMAT_BGRA8, z16), MemoryKind::PLAIN);
            assert_eq!(memory_kind(rule, 0, 0, 0), MemoryKind::PLAIN);
        }
    }

    #[test]
    fn a_typeless_surface_nothing_may_bind_for_depth_is_plain() {
        for format in [FORMAT_R24G8_TYPELESS, FORMAT_R32G8X24_TYPELESS] {
            assert_eq!(memory_kind(Always, 0x10, format, BLOCK_LINEAR | DEPTH_32), MemoryKind::PLAIN);
        }
    }
}
