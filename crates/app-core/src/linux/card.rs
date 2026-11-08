//! The card a Linux guest's device opens: the one setup chose, by its PCI address, or before setup the
//! one it would choose. QEMU's device is told the address and the card's device file, and looks up the
//! RM's numbers for it itself, as the daemon does for Windows. The jail lets QEMU open that card's nodes
//! and no other card's.

use crate::context::Context;
use crate::setup::cards::card_in_use;
use std::fs;
use std::path::Path;

const INFORMATION_FILE: &str = "information";
const MINOR_LABEL: &str = "Device Minor";
const RENDER_PREFIX: &str = "renderD";
/// The PCI device a DRM node belongs to, as a link from its folder in the DRM class.
const DRM_DEVICE_LINK: &str = "device";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardedCard {
    /// Where the card is on the PCI bus, as sysfs names it.
    pub pci: String,
    /// Its device file is `/dev/nvidia` with this on the end.
    pub minor: u32,
    /// Its render node in `/dev/dri`, which the device shows the guest's frames through.
    pub render_node: Option<String>,
}

/// The card setup chose, or before setup the one it would choose, as a Linux machine needs no setup.
/// With its device file and render node, if the driver has it now.
pub fn forwarded_card(context: &Context) -> Option<ForwardedCard> {
    let card = card_in_use(context)?;
    let minor = minor_of(&context.nvidia_cards, &card.pci)?;
    let render_node = render_node_of(&context.drm_class, &card.pci);
    Some(ForwardedCard { pci: card.pci, minor, render_node })
}

/// The device file number in the driver's list of cards, which has a folder for each by address.
fn minor_of(listing: &Path, pci: &str) -> Option<u32> {
    let information = fs::read_to_string(listing.join(pci).join(INFORMATION_FILE)).ok()?;
    information.lines().find_map(|line| {
        let (label, value) = line.split_once(':')?;
        (label.trim() == MINOR_LABEL).then(|| value.trim().parse().ok())?
    })
}

fn render_node_of(drm_class: &Path, pci: &str) -> Option<String> {
    let mut names: Vec<String> = fs::read_dir(drm_class)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(RENDER_PREFIX))
        .collect();
    names.sort();
    names.into_iter().find(|name| {
        let device = fs::read_link(drm_class.join(name).join(DRM_DEVICE_LINK));
        device.is_ok_and(|device| device.file_name().is_some_and(|found| found == pci))
    })
}

#[cfg(test)]
pub mod stand_in {
    use crate::testing::Scratch;
    use std::os::unix::fs::symlink;

    /// A card in the driver's list, and its render node in the DRM class, as this PC lays them out.
    pub fn fake_listed_card(scratch: &Scratch, pci: &str, minor: u32, render: u32) {
        scratch.write(
            &format!("nvidia-gpus/{pci}/information"),
            format!("Model: \t\t NVIDIA Card\nBus Location: \t {pci}\nDevice Minor: \t {minor}\n"),
        );
        let folder = scratch.path(&format!("drm/renderD{render}"));
        std::fs::create_dir_all(&folder).unwrap();
        symlink(format!("../../../{pci}"), folder.join("device")).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::fake_listed_card;
    use super::*;
    use crate::settings::write_setup;
    use crate::setup::cards::stand_in::{NAMES, fake_pci};
    use crate::testing::{Scratch, scratch_context};
    use serde_json::json;

    const DISPLAY: &str = "0x030000";

    fn context_with_two_cards(scratch: &Scratch) -> Context {
        let mut context = scratch_context(scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        context.nvidia_cards = scratch.path("nvidia-gpus");
        context.drm_class = scratch.path("drm");
        // The tester's PC: the 4080 has the first device file, the 1660 SUPER the second.
        fake_pci(scratch, "0000:01:00.0", 0x2704, "0x10de", DISPLAY);
        fake_pci(scratch, "0000:05:00.0", 0x21c4, "0x10de", DISPLAY);
        fake_listed_card(scratch, "0000:01:00.0", 0, 128);
        fake_listed_card(scratch, "0000:05:00.0", 1, 129);
        context
    }

    fn choose(context: &Context, pci: &str) {
        write_setup(context, json!({"card": {"pci": pci}}).as_object().cloned().unwrap()).unwrap();
    }

    #[test]
    fn it_is_the_card_setup_chose_with_its_own_nodes() {
        let scratch = Scratch::new();
        let context = context_with_two_cards(&scratch);
        choose(&context, "0000:05:00.0");
        let card = forwarded_card(&context).unwrap();
        assert_eq!(
            card,
            ForwardedCard { pci: "0000:05:00.0".into(), minor: 1, render_node: Some("renderD129".into()) }
        );
        choose(&context, "0000:01:00.0");
        assert_eq!(forwarded_card(&context).unwrap().minor, 0);
        assert_eq!(forwarded_card(&context).unwrap().render_node.as_deref(), Some("renderD128"));
    }

    #[test]
    fn two_cards_and_no_choice_is_no_card() {
        let scratch = Scratch::new();
        let context = context_with_two_cards(&scratch);
        write_setup(&context, json!({"card": {"name": "old setup"}}).as_object().cloned().unwrap()).unwrap();
        assert_eq!(forwarded_card(&context), None);
    }

    #[test]
    fn a_card_the_driver_does_not_list_is_no_card() {
        let scratch = Scratch::new();
        let mut context = context_with_two_cards(&scratch);
        choose(&context, "0000:05:00.0");
        context.nvidia_cards = scratch.path("nothing-here");
        assert_eq!(forwarded_card(&context), None);
    }

    #[test]
    fn with_no_setup_it_is_the_card_setup_would_choose() {
        let scratch = Scratch::new();
        let context = context_with_two_cards(&scratch);
        // Both are experimental, so setup would take the first by address.
        let card = forwarded_card(&context).unwrap();
        assert_eq!((card.pci.as_str(), card.minor), ("0000:01:00.0", 0));
    }

    #[test]
    fn one_card_from_an_old_setup_is_that_card() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        context.nvidia_cards = scratch.path("nvidia-gpus");
        context.drm_class = scratch.path("drm");
        fake_pci(&scratch, "0000:02:00.0", 0x2b85, "0x10de", DISPLAY);
        fake_listed_card(&scratch, "0000:02:00.0", 0, 129);
        write_setup(&context, json!({"card": {"name": "old setup"}}).as_object().cloned().unwrap()).unwrap();
        let card = forwarded_card(&context).unwrap();
        assert_eq!((card.pci.as_str(), card.minor), ("0000:02:00.0", 0));
    }
}
