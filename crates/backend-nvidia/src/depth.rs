//! Depth and stencil surfaces. On Turing and Ada the depth unit only takes memory whose page kind
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

/// What a surface's memory should be, from its usage byte and the attribute word its driver asked for.
/// A depth surface that says its packing gets those bits. Anything else, a depth surface that doesn't
/// say or says one the RM has no kind for included, is plain memory, as everything was before.
pub fn memory_kind(usage: u8, attributes: u32) -> MemoryKind {
    if usage & USAGE_DEPTH_STENCIL == 0 {
        return MemoryKind::PLAIN;
    }
    let depth = attributes >> DEPTH_SHIFT & DEPTH_MASK;
    let packing = attributes >> PACKING_SHIFT & PACKING_MASK;
    let float = attributes >> Z_FLOAT_SHIFT & 1 == 1;
    let has_kind = match (depth, float) {
        (DEPTH_UNKNOWN, _) => false,
        (_, false) => FIXED_PACKINGS.contains(&packing),
        (_, true) => FLOAT_PACKINGS.contains(&packing),
    };
    if has_kind {
        MemoryKind(attributes & KIND_BITS)
    } else {
        log::debug!("a depth surface asked for attributes {attributes:#x}, so it gets plain memory");
        MemoryKind::PLAIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RENDER_TARGET: u8 = 0x11;
    const DEPTH_STENCIL: u8 = 0x12;
    const BLOCK_LINEAR: u32 = 2 << 16;
    const COMPRESS_ANY: u32 = 2 << 12;
    const DEPTH_16: u32 = 2;
    const DEPTH_32: u32 = 4;
    const Z_FLOAT: u32 = 1 << Z_FLOAT_SHIFT;

    fn packed(packing: u32) -> u32 {
        packing << PACKING_SHIFT
    }

    #[test]
    fn a_16_bit_depth_surface_keeps_its_packing() {
        let asked = BLOCK_LINEAR | packed(PACKING_Z16) | DEPTH_16;
        assert_eq!(asked, 0x3A_0002);
        assert_eq!(memory_kind(DEPTH_STENCIL, asked), MemoryKind(packed(PACKING_Z16) | DEPTH_16));
    }

    #[test]
    fn a_float_depth_surface_keeps_its_packing() {
        let asked = BLOCK_LINEAR | Z_FLOAT | packed(PACKING_Z32) | DEPTH_32;
        assert_eq!(asked, 0x16_0004);
        assert_eq!(memory_kind(DEPTH_STENCIL, asked), MemoryKind(Z_FLOAT | packed(PACKING_Z32) | DEPTH_32));
    }

    #[test]
    fn a_24_bit_depth_surface_with_stencil_keeps_its_packing() {
        for packing in [PACKING_S8Z24, PACKING_Z24S8, PACKING_X8Z24, PACKING_Z24X8] {
            let asked = BLOCK_LINEAR | packed(packing) | DEPTH_32;
            assert_eq!(memory_kind(DEPTH_STENCIL, asked), MemoryKind(packed(packing) | DEPTH_32));
        }
        let asked = Z_FLOAT | packed(PACKING_Z32_X24S8) | 5;
        assert_eq!(memory_kind(DEPTH_STENCIL, asked), MemoryKind(asked));
    }

    #[test]
    fn a_stencil_surface_is_8_bits_deep() {
        const DEPTH_8: u32 = 1;
        assert_eq!(memory_kind(DEPTH_STENCIL, BLOCK_LINEAR | DEPTH_8), MemoryKind(DEPTH_8));
    }

    #[test]
    fn compression_and_the_rest_are_not_passed_on() {
        let asked = 0x3 << 23 | BLOCK_LINEAR | COMPRESS_ANY | 1 << 10 | packed(PACKING_S8Z24) | DEPTH_32;
        assert_eq!(memory_kind(DEPTH_STENCIL, asked), MemoryKind(packed(PACKING_S8Z24) | DEPTH_32));
    }

    #[test]
    fn a_depth_surface_that_does_not_say_its_packing_is_plain() {
        // Blackwell's driver keeps stencil apart and asks this way.
        assert_eq!(memory_kind(DEPTH_STENCIL, BLOCK_LINEAR), MemoryKind::PLAIN);
    }

    #[test]
    fn a_packing_the_rm_has_no_kind_for_is_plain() {
        assert_eq!(memory_kind(DEPTH_STENCIL, packed(PACKING_Z32) | DEPTH_32), MemoryKind::PLAIN);
        assert_eq!(memory_kind(DEPTH_STENCIL, Z_FLOAT | packed(PACKING_Z16) | DEPTH_16), MemoryKind::PLAIN);
        assert_eq!(memory_kind(DEPTH_STENCIL, packed(6) | DEPTH_32), MemoryKind::PLAIN);
    }

    #[test]
    fn colour_surfaces_are_plain_whatever_they_ask() {
        let asked = BLOCK_LINEAR | COMPRESS_ANY | DEPTH_32;
        assert_eq!(memory_kind(RENDER_TARGET, asked), MemoryKind::PLAIN);
        assert_eq!(memory_kind(0x10, packed(PACKING_Z16) | DEPTH_16), MemoryKind::PLAIN);
        assert_eq!(memory_kind(0, 0), MemoryKind::PLAIN);
    }
}
