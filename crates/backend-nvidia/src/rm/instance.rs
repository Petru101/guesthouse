//! Which device instance the RM gave the GPU at a PCI address.
//!
//! The RM numbers devices in the order their GPUs were first set up, which needn't be the order of
//! their device files, and can change from one boot to the next. A device made with another GPU's
//! number is refused when the client hasn't opened that GPU's device file. So the number is looked up
//! each time: the attached GPUs, each one's place on the bus, and the device instance of the one at
//! the address. Opening a GPU's device file attaches it, so this runs after that.
//!
//! Controls and layouts are from `ctrl0000gpu.h`, the same on branches 615 and 620.

use super::classes::{CONTROL_GET_ATTACHED_IDS, CONTROL_GET_GPU_ID_INFO, CONTROL_GET_GPU_PCI_INFO};
use super::{Handle, Rm, RmError};
use guesthouse_backend::PciAddress;

/// `NV0000_CTRL_GPU_MAX_ATTACHED_GPUS`, and `NV0000_CTRL_GPU_INVALID_ID` after the last one.
const MAX_ATTACHED_GPUS: usize = 32;
const NO_GPU: u32 = u32::MAX;
/// `NV0000_CTRL_GPU_GET_PCI_INFO_PARAMS`: gpuId, domain, then 16-bit bus and slot.
const PCI_INFO_SIZE: usize = 12;
const PCI_INFO_DOMAIN: usize = 4;
const PCI_INFO_BUS: usize = 8;
const PCI_INFO_SLOT: usize = 10;
/// `NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS`, eight words, with deviceInstance the third.
const ID_INFO_SIZE: usize = 32;
const ID_INFO_DEVICE_INSTANCE: usize = 8;

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn half(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two bytes"))
}

fn control<R: Rm>(rm: &mut R, client: Handle, command: u32, params: &mut [u8]) -> Result<(), RmError> {
    let status = rm.control(client, command, params)?;
    RmError::check(|| format!("control {command:#x}"), status)
}

/// The RM's ids of the GPUs it has set up.
pub fn attached_gpus<R: Rm>(rm: &mut R, client: Handle) -> Result<Vec<u32>, RmError> {
    let mut params = [0; MAX_ATTACHED_GPUS * 4];
    control(rm, client, CONTROL_GET_ATTACHED_IDS, &mut params)?;
    let ids = params.chunks(4).map(|bytes| word(bytes, 0));
    Ok(ids.take_while(|&id| id != NO_GPU).collect())
}

/// Whether the GPU with this id is the one at the address. The RM gives no function number, and a
/// GPU is always function 0, so an address with another one is never a GPU's.
fn is_at<R: Rm>(rm: &mut R, client: Handle, gpu: u32, address: &PciAddress) -> Result<bool, RmError> {
    let mut params = [0; PCI_INFO_SIZE];
    params[..4].copy_from_slice(&gpu.to_le_bytes());
    control(rm, client, CONTROL_GET_GPU_PCI_INFO, &mut params)?;
    Ok(word(&params, PCI_INFO_DOMAIN) == address.domain
        && half(&params, PCI_INFO_BUS) == u16::from(address.bus)
        && half(&params, PCI_INFO_SLOT) == u16::from(address.device)
        && address.function == 0)
}

fn device_instance_of<R: Rm>(rm: &mut R, client: Handle, gpu: u32) -> Result<u32, RmError> {
    let mut params = [0; ID_INFO_SIZE];
    params[..4].copy_from_slice(&gpu.to_le_bytes());
    control(rm, client, CONTROL_GET_GPU_ID_INFO, &mut params)?;
    Ok(word(&params, ID_INFO_DEVICE_INSTANCE))
}

/// The device instance of the GPU at this address. Its device file must be open already.
pub fn device_instance_at<R: Rm>(rm: &mut R, client: Handle, address: &PciAddress) -> Result<u32, RmError> {
    for gpu in attached_gpus(rm, client)? {
        if is_at(rm, client, gpu, address)? {
            return device_instance_of(rm, client, gpu);
        }
    }
    Err(RmError::System(format!("the driver has no GPU set up at {address}")))
}
