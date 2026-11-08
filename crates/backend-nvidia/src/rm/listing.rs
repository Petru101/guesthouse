//! The driver's own list of its cards, in `/proc/driver/nvidia/gpus`: one folder a card, named by its
//! PCI address, with an information file that gives the card's device file number.
//!
//! The device file number (the minor) is only the order the driver probed the cards in, and the RM's
//! device instance is something else again, so neither is a card's name. The PCI address is.

use guesthouse_backend::PciAddress;
use std::fs;
use std::path::Path;

const CARDS_FOLDER: &str = "/proc/driver/nvidia/gpus";
const INFORMATION_FILE: &str = "information";

/// One card, as its information file describes it. The file also holds the card's UUID, which
/// isn't kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedCard {
    pub address: PciAddress,
    /// Its device file is `/dev/nvidia` with this on the end.
    pub minor: u32,
    pub model: String,
}

/// Where to reach one card: its device file, and its PCI address for finding the RM's number for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardPlace {
    pub minor: u32,
    /// `None` only when the driver's list couldn't be read, which leaves the guess this project made
    /// before it read the list: the first device file and the RM's first device. That guess only
    /// holds on a PC with one card.
    pub address: Option<PciAddress>,
}

impl CardPlace {
    pub const FIRST_GUESSED: Self = Self { minor: 0, address: None };
}

impl From<&ListedCard> for CardPlace {
    fn from(card: &ListedCard) -> Self {
        Self { minor: card.minor, address: Some(card.address) }
    }
}

/// Reads the field after `label:` in an information file.
fn field<'a>(information: &'a str, label: &str) -> Option<&'a str> {
    information.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == label).then(|| value.trim())
    })
}

pub fn parse_information(information: &str) -> Option<ListedCard> {
    Some(ListedCard {
        address: PciAddress::parse(field(information, "Bus Location")?)?,
        minor: field(information, "Device Minor")?.parse().ok()?,
        model: field(information, "Model")?.to_owned(),
    })
}

/// The cards in a folder laid out as the driver's, by device file number.
pub fn read_listing_from(folder: &Path) -> Vec<ListedCard> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let mut cards: Vec<ListedCard> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join(INFORMATION_FILE)).ok())
        .filter_map(|information| parse_information(&information))
        .collect();
    cards.sort_by_key(|card| card.minor);
    cards
}

/// This PC's cards, by device file number. Empty if the driver isn't loaded or the list can't be read.
pub fn read_listing() -> Vec<ListedCard> {
    read_listing_from(Path::new(CARDS_FOLDER))
}

/// The card at this address, if the driver has one there.
pub fn listed_at(listing: &[ListedCard], address: &PciAddress) -> Option<ListedCard> {
    listing.iter().find(|card| card.address == *address).cloned()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn information(model: &str, address: &str, minor: u32) -> String {
        format!(
            "Model: \t\t {model}\nIRQ:   \t\t 156\nGPU UUID: \t GPU-12345678-aaaa-bbbb-cccc-0123456789ab\n\
             Video BIOS: \t 95.02.3c.00.a1\nBus Type: \t PCIe\nDMA Size: \t 47 bits\nDMA Mask: \t 0x7fffffffffff\n\
             Bus Location: \t {address}\nDevice Minor: \t {minor}\nGPU Firmware: \t 615.78.08\nGPU Excluded:\t No\n"
        )
    }

    #[test]
    fn a_card_is_its_address_number_and_model() {
        let card = parse_information(&information("NVIDIA GeForce RTX 4080", "0000:01:00.0", 1)).unwrap();
        assert_eq!(card.address.to_string(), "0000:01:00.0");
        assert_eq!((card.minor, card.model.as_str()), (1, "NVIDIA GeForce RTX 4080"));
        assert!(!format!("{card:?}").contains("GPU-"));
        assert_eq!(parse_information("Model: x\nDevice Minor: 0\n"), None, "no address, no card");
    }

    #[test]
    fn the_list_is_in_device_file_order_whatever_the_folders_are() {
        let folder = std::env::temp_dir().join(format!("gh-listing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        for (address, minor, model) in [("0000:01:00.0", 1, "B"), ("0000:02:00.0", 0, "A")] {
            fs::create_dir_all(folder.join(address)).unwrap();
            fs::write(folder.join(address).join(INFORMATION_FILE), information(model, address, minor))
                .unwrap();
        }
        let cards = read_listing_from(&folder);
        let order: Vec<(u32, &str)> = cards.iter().map(|card| (card.minor, card.model.as_str())).collect();
        assert_eq!(order, [(0, "A"), (1, "B")]);
        let wanted = PciAddress::parse("0000:01:00.0").unwrap();
        assert_eq!(listed_at(&cards, &wanted).map(|card| card.minor), Some(1));
        assert!(read_listing_from(&folder.join("nothing")).is_empty());
        fs::remove_dir_all(folder).unwrap();
    }
}
