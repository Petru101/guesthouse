//! Result codes, in the guest's numbering.

/// A result code the guest understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u32);

impl Status {
    pub const SUCCESS: Self = Self(0);
    pub const UNSUCCESSFUL: Self = Self(0xC000_0001);
    pub const INVALID_PARAMETER: Self = Self(0xC000_000D);
    pub const NO_MEMORY: Self = Self(0xC000_0017);
    pub const NOT_SUPPORTED: Self = Self(0xC000_00BB);
    pub const NO_VIDEO_MEMORY: Self = Self(0xC01E_0100);
}
