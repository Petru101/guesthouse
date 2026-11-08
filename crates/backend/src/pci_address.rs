//! Where a card sits on the PCI bus. Unlike a driver's numbering, this stays the same from one boot
//! to the next while the card stays in its slot, so it's what names a card here.

use std::fmt;

/// A PCI address: domain, bus, device and function, as `0000:01:00.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PciAddress {
    pub domain: u32,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

/// The kernel's limits on the last two parts.
const MAX_DEVICE: u8 = 0x1F;
const MAX_FUNCTION: u8 = 7;

impl PciAddress {
    /// Reads `domain:bus:device.function` in hex. The domain may be any width, as sysfs writes four
    /// digits and `nvidia-smi` eight. A missing domain is domain 0.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (rest, function) = text.rsplit_once('.')?;
        let mut parts = rest.rsplitn(3, ':');
        let device = parts.next()?;
        let bus = parts.next()?;
        let domain = parts.next().unwrap_or("0");
        let hex = |part: &str, most: usize| {
            let fits =
                !part.is_empty() && part.len() <= most && part.bytes().all(|byte| byte.is_ascii_hexdigit());
            fits.then(|| u32::from_str_radix(part, 16).ok()).flatten()
        };
        let address = Self {
            domain: hex(domain, 8)?,
            bus: hex(bus, 2)? as u8,
            device: hex(device, 2)? as u8,
            function: hex(function, 1)? as u8,
        };
        (address.device <= MAX_DEVICE && address.function <= MAX_FUNCTION).then_some(address)
    }
}

/// The form sysfs and `/proc/driver/nvidia` use for a folder's name.
impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04x}:{:02x}:{:02x}.{:x}", self.domain, self.bus, self.device, self.function)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: PciAddress = PciAddress { domain: 0, bus: 1, device: 0, function: 0 };

    #[test]
    fn sysfs_and_nvidia_smi_name_the_same_card() {
        assert_eq!(PciAddress::parse("0000:01:00.0"), Some(FIRST));
        assert_eq!(PciAddress::parse("00000000:01:00.0\n"), Some(FIRST));
        assert_eq!(PciAddress::parse("01:00.0"), Some(FIRST));
        assert_eq!(FIRST.to_string(), "0000:01:00.0");
    }

    #[test]
    fn every_part_is_kept() {
        let address = PciAddress::parse("10002:c1:1f.7").unwrap();
        assert_eq!(address, PciAddress { domain: 0x10002, bus: 0xC1, device: 0x1F, function: 7 });
        assert_eq!(address.to_string(), "10002:c1:1f.7");
    }

    #[test]
    fn what_is_not_an_address_is_refused() {
        for text in ["", "0", "0000:01:00", "0000:01:20.0", "0000:01:00.8", "0000:1x:00.0", "0000:100:00.0"] {
            assert_eq!(PciAddress::parse(text), None, "{text:?}");
        }
    }
}
