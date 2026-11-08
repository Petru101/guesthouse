//! What the host says about its driver and cards, from files the kernel module keeps. Nothing here
//! opens a device.

use guesthouse_backend::PciAddress;
pub use guesthouse_backend_nvidia::rm::listing::ListedCard;
use guesthouse_backend_nvidia::rm::listing::read_listing_from;
use std::fs;
use std::path::Path;

const VERSION_FILE: &str = "/sys/module/nvidia/version";
const TAINT_FILE: &str = "/sys/module/nvidia/taint";
const CARDS_FOLDER: &str = "/proc/driver/nvidia/gpus";
/// The kernel's taint letter for a module with a proprietary licence. The open modules are GPL and
/// never set it.
const PROPRIETARY_TAINT: char = 'P';

/// What the module's files say. Nothing in it identifies the PC or the card: a PCI address is a
/// slot, the same on many PCs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub version: String,
    pub open_module: bool,
    /// The cards, in the order of their device files. A card's number here is its place in this list,
    /// which on most PCs is its device file's number too.
    pub cards: Vec<ListedCard>,
}

/// Why the host can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoHost {
    NoDriver,
    NoCard,
}

pub fn no_host_text(why: &NoHost) -> &'static str {
    match why {
        NoHost::NoDriver => "No NVIDIA driver is loaded, so there is nothing to check.",
        NoHost::NoCard => "The NVIDIA driver is loaded, but it has no card. Nothing to check.",
    }
}

/// Whether the card that was opened is the one the driver lists. The listing's model and the card's
/// own name are the same text on a card that lines up.
pub fn same_card(listed_model: &str, opened_name: &str) -> bool {
    listed_model.trim() == opened_name.trim()
}

pub fn parse_taint_as_open(taint: &str) -> bool {
    !taint.contains(PROPRIETARY_TAINT)
}

impl Host {
    /// The card with this number in the list.
    pub fn card_numbered(&self, number: usize) -> Option<(usize, &ListedCard)> {
        self.cards.get(number).map(|card| (number, card))
    }

    /// The card at this address, and its number in the list.
    pub fn card_at(&self, address: &PciAddress) -> Option<(usize, &ListedCard)> {
        self.cards.iter().enumerate().find(|(_, card)| card.address == *address)
    }
}

pub fn read_host() -> Result<Host, NoHost> {
    read_host_from(Path::new(VERSION_FILE), Path::new(TAINT_FILE), Path::new(CARDS_FOLDER))
}

pub fn read_host_from(version: &Path, taint: &Path, cards: &Path) -> Result<Host, NoHost> {
    let version = fs::read_to_string(version).map_err(|_| NoHost::NoDriver)?.trim().to_owned();
    if version.is_empty() {
        return Err(NoHost::NoDriver);
    }
    let open_module = fs::read_to_string(taint).is_ok_and(|taint| parse_taint_as_open(&taint));
    let cards = read_listing_from(cards);
    if cards.is_empty() {
        return Err(NoHost::NoCard);
    }
    Ok(Host { version, open_module, cards })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFORMATION: &str = "Model: \t\t NVIDIA GeForce RTX 4080\nIRQ:   \t\t 156\nGPU UUID: \t GPU-12345678-aaaa-bbbb-cccc-0123456789ab\nVideo BIOS: \t 95.02.3c.00.a1\nBus Type: \t PCIe\nDMA Size: \t 47 bits\nDMA Mask: \t 0x7fffffffffff\nBus Location: \t 0000:01:00.0\nDevice Minor: \t 1\nGPU Firmware: \t 615.78.08\nGPU Excluded:\t No\n";

    #[test]
    fn a_card_lines_up_when_the_listing_and_the_card_say_the_same_name() {
        assert!(same_card("NVIDIA GeForce RTX 4080", "NVIDIA GeForce RTX 4080"));
        assert!(same_card(" NVIDIA GeForce RTX 4080\n", "NVIDIA GeForce RTX 4080"));
        assert!(!same_card("NVIDIA GeForce RTX 4080", "NVIDIA GeForce GTX 1660 SUPER"));
        assert!(!same_card("NVIDIA GeForce RTX 4080", ""));
    }

    #[test]
    fn the_proprietary_module_taints_the_kernel_and_the_open_one_does_not() {
        assert!(parse_taint_as_open("O\n"));
        assert!(parse_taint_as_open("OE\n"));
        assert!(!parse_taint_as_open("POE\n"));
    }

    #[test]
    fn a_missing_driver_and_a_missing_card_are_told_apart() {
        let scratch = std::env::temp_dir().join(format!("card-check-host-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        fs::create_dir_all(scratch.join("gpus")).unwrap();
        let version = scratch.join("version");
        let taint = scratch.join("taint");
        let gpus = scratch.join("gpus");
        assert_eq!(read_host_from(&version, &taint, &gpus), Err(NoHost::NoDriver));
        fs::write(&version, "\n").unwrap();
        assert_eq!(read_host_from(&version, &taint, &gpus), Err(NoHost::NoDriver), "an empty file is none");
        fs::write(&version, "615.78.08\n").unwrap();
        assert_eq!(read_host_from(&version, &taint, &gpus), Err(NoHost::NoCard));
        fs::create_dir_all(gpus.join("0000:01:00.0")).unwrap();
        fs::write(gpus.join("0000:01:00.0").join("information"), INFORMATION).unwrap();
        let host = read_host_from(&version, &taint, &gpus).unwrap();
        assert_eq!(host.version, "615.78.08");
        assert_eq!(host.cards.len(), 1);
        assert!(!host.open_module, "no taint file means the module can't be told open");
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_card_is_found_by_its_number_in_the_list_or_by_its_address() {
        let at = |text: &str| PciAddress::parse(text).unwrap();
        let card = |address: &str, minor: u32, model: &str| ListedCard {
            address: at(address),
            minor,
            model: model.to_owned(),
        };
        let host = Host {
            version: "615.78.08".to_owned(),
            open_module: true,
            cards: vec![
                card("0000:01:00.0", 0, "NVIDIA GeForce RTX 4080"),
                card("0000:02:00.0", 1, "NVIDIA GeForce GTX 1660 SUPER"),
            ],
        };
        assert_eq!(host.card_numbered(1).map(|(_, card)| card.minor), Some(1));
        assert_eq!(host.card_numbered(2), None);
        let (number, found) = host.card_at(&at("0000:02:00.0")).unwrap();
        assert_eq!((number, found.model.as_str()), (1, "NVIDIA GeForce GTX 1660 SUPER"));
        assert_eq!(host.card_at(&at("0000:03:00.0")), None);
    }
}
