//! The command buffer for copies the daemon makes on the card itself.
//!
//! One small buffer in host memory, written by the CPU before each copy and run by the DMA engine.
//! It is mapped in the daemon's own space between the core's two address ranges, where the guest
//! can never place anything, so no guest mapping can land on it.

use crate::device::{Device, Mapping};
use crate::memory::{self, CpuAccess, DOMAIN_GTT, GemHandle};
use guesthouse_backend::SpaceId;
use std::io;

/// Where the buffer sits in the space: the start of the gap below the core's upper range.
pub const ADDRESS: u64 = 0x1_0000_0000;
/// Room for any copy's commands, many times over.
const BYTES: usize = 0x1000;

#[derive(Debug)]
pub struct CopyIb {
    handle: GemHandle,
    view: Mapping,
    size: u64,
    /// The space it is mapped in, if it is mapped yet.
    pub mapped_in: Option<SpaceId>,
}

impl CopyIb {
    /// Makes the buffer, sized to the card's mapping alignment.
    pub fn make(device: &Device, alignment: u64) -> io::Result<Self> {
        let size = (BYTES as u64).next_multiple_of(alignment.max(1));
        let handle = memory::create(device, size, alignment, DOMAIN_GTT, CpuAccess::Required)?;
        match device.map_cpu(handle, size as usize) {
            Ok(view) => Ok(Self { handle, view, size, mapped_in: None }),
            Err(error) => {
                memory::close(device, handle);
                Err(error)
            }
        }
    }

    pub fn handle(&self) -> GemHandle {
        self.handle
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Writes `commands` at the start, and returns their length in bytes.
    pub fn write(&mut self, commands: &[u32]) -> io::Result<u32> {
        let bytes = size_of_val(commands);
        if bytes > self.view.size() {
            return Err(io::Error::other("the commands don't fit the buffer"));
        }
        for (index, &word) in commands.iter().enumerate() {
            self.view.write_u32(index * size_of::<u32>(), word);
        }
        u32::try_from(bytes).map_err(io::Error::other)
    }
}
