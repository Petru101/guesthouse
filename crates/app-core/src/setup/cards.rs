//! Which cards are here, and what is known about each. The list is only a label for the user: what a
//! card can do is read from the card.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::sorted_names;
use crate::settings::{CARD_MEMORY_FIELD, write_setup};
use crate::usb::sysfs_text;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const NVIDIA_VENDOR: &str = "0x10de";
const NVIDIA_NAMES_START: &str = "\n10de  ";
const DISPLAY_CLASS: &str = "0x03";
/// The card this has been run on.
const TESTED_CARDS: [u32; 1] = [0x2B85];
/// Consumer 50, 40, 20 and 16 series, from pci.ids, which may be used under the 3-clause BSD licence. The workstation cards on these chips don't count, as nothing
/// says their answers are the same. The 30 series has no profile, so it isn't here.
const CONSUMER_CARDS: [(&str, &[u32]); 15] = [
    ("GB202", &[0x2B85, 0x2B87, 0x2B8C]),
    ("GB203", &[0x2C02, 0x2C05, 0x2C09, 0x2C18, 0x2C19, 0x2C58, 0x2C59]),
    ("GB205", &[0x2F04, 0x2F06, 0x2F18, 0x2F58]),
    ("GB206", &[0x2D04, 0x2D05, 0x2D18, 0x2D19, 0x2D58, 0x2D59]),
    ("GB207", &[0x2D83, 0x2D98, 0x2DD8]),
    ("AD102", &[0x2684, 0x2685, 0x2689]),
    ("AD103", &[0x2702, 0x2703, 0x2704, 0x2705, 0x2709, 0x2717, 0x2757]),
    ("AD104", &[0x2782, 0x2783, 0x2786, 0x2788, 0x27A0, 0x27E0]),
    ("AD106", &[0x2803, 0x2805, 0x2808, 0x2820, 0x2822, 0x2860]),
    ("AD107", &[0x2882, 0x28A0, 0x28A1, 0x28A3, 0x28E0, 0x28E1, 0x28E3]),
    ("TU102", &[0x1E03, 0x1E04, 0x1E07]),
    (
        "TU104",
        &[
            0x1E81, 0x1E82, 0x1E84, 0x1E87, 0x1E89, 0x1E90, 0x1E91, 0x1E93, 0x1EC2, 0x1EC7, 0x1ED0, 0x1ED1,
            0x1ED3,
        ],
    ),
    (
        "TU106",
        &[
            0x1F02, 0x1F03, 0x1F06, 0x1F07, 0x1F08, 0x1F09, 0x1F0A, 0x1F10, 0x1F11, 0x1F12, 0x1F14, 0x1F15,
            0x1F42, 0x1F47, 0x1F50, 0x1F51, 0x1F54, 0x1F55,
        ],
    ),
    ("TU116", &[0x2182, 0x2184, 0x2187, 0x2188, 0x2191, 0x2192, 0x21C4, 0x21D1]),
    ("TU117", &[0x1F82, 0x1F83, 0x1F91, 0x1F92, 0x1F94, 0x1F95, 0x1F96, 0x1F99, 0x1F9D, 0x1FD9, 0x1FDD]),
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

/// The card's memory in MiB, as the driver reports it. `nvidia-smi` says one number a card.
fn parse_card_memory(said: &str) -> Option<u64> {
    said.lines().next()?.trim().parse().ok()
}

fn card_memory_mib() -> Option<u64> {
    let said = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    parse_card_memory(&String::from_utf8_lossy(&said.stdout))
}

/// Writes down the card for the daemon, and how much memory it has for the video memory cap.
pub fn remember_card(context: &Context) -> Result<()> {
    let cards = find_cards(context);
    let card = cards.first().ok_or_else(|| Failure::new("No NVIDIA card was found."))?;
    let memory = card_memory_mib().unwrap_or(0);
    let changes = json!({"card": card, CARD_MEMORY_FIELD: memory});
    write_setup(context, changes.as_object().cloned().unwrap_or_default())
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
                             \t2bb1  GB202GL [RTX PRO 6000]\n\t1b80  Plain name\n\
                             \t2704  AD103 [GeForce RTX 4080]\n\t21c4  TU116 [GeForce GTX 1660 SUPER]\n\
                             \t2204  GA102 [GeForce RTX 3090]\n10df  Next\n";
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
    fn the_cards_memory_is_its_first_line() {
        assert_eq!(parse_card_memory("32607\n"), Some(32607));
        assert_eq!(parse_card_memory("Failed to initialize NVML\n"), None);
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
    fn the_40_and_16_series_are_experimental_and_named_as_pci_ids_has_them() {
        let scratch = Scratch::new();
        let context = context(&scratch);
        fake_pci(&scratch, "0000:01:00.0", 0x2704, NVIDIA, DISPLAY);
        fake_pci(&scratch, "0000:02:00.0", 0x21C4, NVIDIA, DISPLAY);
        let cards = find_cards(&context);
        let found: Vec<(Support, &str, &str)> =
            cards.iter().map(|card| (card.support, card.chip.as_str(), card.name.as_str())).collect();
        assert_eq!(
            found,
            [
                (Support::Experimental, "AD103", "NVIDIA GeForce RTX 4080"),
                (Support::Experimental, "TU116", "NVIDIA GeForce GTX 1660 SUPER"),
            ]
        );
    }

    #[test]
    fn the_30_series_has_no_profile_and_stays_unsupported() {
        assert_eq!(support_of(0x2204), Support::Unsupported);
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
