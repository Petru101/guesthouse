//! Which cards are here, and what is known about each. The list is only a label for the user: what a
//! card can do is read from the card.

use crate::context::Context;
use crate::machine::sorted_names;
use crate::usb::sysfs_text;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const NVIDIA_VENDOR: &str = "0x10de";
const NVIDIA_NAMES_START: &str = "\n10de  ";
const DISPLAY_CLASS: &str = "0x03";
/// The card this has been run on.
const TESTED_CARDS: [u32; 1] = [0x2B85];
/// Consumer 50-series, from pci.ids. The workstation cards on these chips don't count, as nothing
/// says their answers are the same.
const CONSUMER_CARDS: [(&str, &[u32]); 5] = [
    ("GB202", &[0x2B85, 0x2B87, 0x2B8C]),
    ("GB203", &[0x2C02, 0x2C05, 0x2C09, 0x2C18, 0x2C19, 0x2C58, 0x2C59]),
    ("GB205", &[0x2F04, 0x2F06, 0x2F18, 0x2F58]),
    ("GB206", &[0x2D04, 0x2D05, 0x2D18, 0x2D19, 0x2D58, 0x2D59]),
    ("GB207", &[0x2D83, 0x2D98, 0x2DD8]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    Tested,
    Experimental,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub device: u32,
    pub name: String,
    /// Empty if it isn't a chip this knows.
    pub chip: String,
    pub support: Support,
    pub revision: u32,
}

fn chip_of(device: u32) -> &'static str {
    CONSUMER_CARDS.iter().find(|(_, devices)| devices.contains(&device)).map_or("", |found| found.0)
}

fn support_of(device: u32) -> Support {
    if TESTED_CARDS.contains(&device) {
        Support::Tested
    } else if chip_of(device).is_empty() {
        Support::Unsupported
    } else {
        Support::Experimental
    }
}

/// The card's name in pci.ids, in the brackets if it has them, as those hold the product.
pub fn card_name(device: u32, names: &Path) -> String {
    let fallback = || format!("NVIDIA card {device:04x}");
    let Ok(data) = fs::read(names) else { return fallback() };
    let text = String::from_utf8_lossy(&data);
    let Some(start) = text.find(NVIDIA_NAMES_START) else { return fallback() };
    let vendor = &text[start + 1..];
    let wanted = format!("\t{device:04x}  ");
    let lines = vendor.lines().skip(1).take_while(|line| !line.starts_with('1'));
    let Some(line) = lines.into_iter().find_map(|line| line.strip_prefix(&wanted)) else { return fallback() };
    let bracketed =
        line.find('[').and_then(|start| Some(&line[start + 1..start + line[start..].rfind(']')?]));
    format!("NVIDIA {}", bracketed.unwrap_or(line))
}

fn hex_number(text: &str) -> Option<u32> {
    u32::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

/// NVIDIA's display devices, the best supported first.
pub fn find_cards(context: &Context) -> Vec<Card> {
    let root = &context.pci_devices;
    let mut cards = Vec::new();
    for name in sorted_names(root) {
        let folder = root.join(name);
        let text = |field: &str| sysfs_text(&folder.join(field));
        if text("vendor") == NVIDIA_VENDOR && text("class").starts_with(DISPLAY_CLASS) {
            let Some(device) = hex_number(&text("device")) else { continue };
            cards.push(Card {
                device,
                name: card_name(device, &context.pci_names),
                chip: chip_of(device).to_owned(),
                support: support_of(device),
                revision: hex_number(&text("revision")).unwrap_or(0),
            });
        }
    }
    cards.sort_by_key(|card| card.support);
    cards
}

#[cfg(test)]
pub mod stand_in {
    use crate::testing::Scratch;

    pub fn fake_pci(scratch: &Scratch, name: &str, device: u32, vendor: &str, class: &str) {
        scratch.write(&format!("pci/{name}/vendor"), format!("{vendor}\n"));
        scratch.write(&format!("pci/{name}/class"), format!("{class}\n"));
        scratch.write(&format!("pci/{name}/device"), format!("0x{device:04x}\n"));
    }

    pub const NAMES: &str = "10dd  Other\n\t2c02  Not this one\n10de  NVIDIA Corporation\n\
                             \t2c02  GB203 [GeForce RTX 5080]\n\t2b85  GB202 [GeForce RTX 5090]\n\
                             \t2bb1  GB202GL [RTX PRO 6000]\n\t1b80  Plain name\n10df  Next\n";
}

#[cfg(test)]
mod tests {
    use super::stand_in::{NAMES, fake_pci};
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    const DISPLAY: &str = "0x030000";
    const NVIDIA: &str = "0x10de";

    fn context(scratch: &Scratch) -> Context {
        let mut context = scratch_context(scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        context
    }

    #[test]
    fn workstation_cards_do_not_count() {
        let scratch = Scratch::new();
        let context = context(&scratch);
        fake_pci(&scratch, "0000:00:01.0", 0x2B85, "0x1002", DISPLAY);
        fake_pci(&scratch, "0000:01:00.0", 0x2BB1, NVIDIA, DISPLAY);
        fake_pci(&scratch, "0000:01:00.1", 0x2B85, NVIDIA, "0x040300");
        let cards = find_cards(&context);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].support, Support::Unsupported);
        assert_eq!(cards[0].name, "NVIDIA RTX PRO 6000");
    }

    #[test]
    fn the_best_supported_card_comes_first() {
        let scratch = Scratch::new();
        let context = context(&scratch);
        fake_pci(&scratch, "0000:01:00.0", 0x2BB1, NVIDIA, DISPLAY);
        fake_pci(&scratch, "0000:02:00.0", 0x2C02, NVIDIA, DISPLAY);
        fake_pci(&scratch, "0000:03:00.0", 0x2B85, NVIDIA, DISPLAY);
        let cards = find_cards(&context);
        let found: Vec<(Support, &str)> =
            cards.iter().map(|card| (card.support, card.chip.as_str())).collect();
        assert_eq!(
            found,
            [(Support::Tested, "GB202"), (Support::Experimental, "GB203"), (Support::Unsupported, "")]
        );
        assert_eq!(
            (cards[0].name.as_str(), cards[1].name.as_str()),
            ("NVIDIA GeForce RTX 5090", "NVIDIA GeForce RTX 5080")
        );
    }

    #[test]
    fn a_name_comes_from_pci_ids_or_from_the_number() {
        let scratch = Scratch::new();
        let names = scratch.write("pci.ids", NAMES);
        assert_eq!(card_name(0x1B80, &names), "NVIDIA Plain name");
        assert_eq!(card_name(0x9999, &names), "NVIDIA card 9999");
        assert_eq!(card_name(0x2B85, &scratch.path("nothing")), "NVIDIA card 2b85");
    }

    #[test]
    fn no_folder_is_no_cards() {
        let scratch = Scratch::new();
        assert!(find_cards(&context(&scratch)).is_empty());
    }
}
