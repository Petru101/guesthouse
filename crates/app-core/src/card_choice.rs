//! The cards a Windows machine's GPU-PV can run on, as the user is offered them. A machine names its
//! card in `amd_gpu`: an AMD card by PCI address, or empty for the NVIDIA card setup chose. Any card
//! can be picked when the machine is made. After that it can move only to a card of the same maker.

use crate::context::Context;
use crate::linux::amd::{amd_cards, default_linux_gpu};
use crate::machine::Machine;
use crate::setup::cards::{card_in_use, find_cards};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardChoice {
    /// What goes in the machine's `amd_gpu`.
    pub pci: String,
    pub label: String,
}

/// Every card here a new Windows machine could run on, NVIDIA's first.
pub fn windows_card_choices(context: &Context) -> Vec<CardChoice> {
    let mut choices = Vec::new();
    if !find_cards(context).is_empty() {
        let label = card_in_use(context).map_or_else(|| "NVIDIA".to_owned(), |card| card.name);
        choices.push(CardChoice { pci: String::new(), label });
    }
    let amd = amd_cards(context).into_iter();
    choices.extend(
        amd.map(|card| CardChoice { label: format!("{} at {}", card.name, card.pci), pci: card.pci }),
    );
    choices
}

/// The cards an existing machine may move to: those of its own maker. The one it has stays listed even
/// when it's gone from this PC, so the choice still shows it.
pub fn same_maker_choices(context: &Context, machine: &Machine) -> Vec<CardChoice> {
    let mut choices: Vec<CardChoice> = windows_card_choices(context)
        .into_iter()
        .filter(|choice| machine.card_change_problem(&choice.pci).is_none())
        .collect();
    if !choices.iter().any(|choice| choice.pci == machine.amd_gpu) {
        let label = format!("The card at {}, which isn't in this PC now", machine.amd_gpu);
        choices.push(CardChoice { pci: machine.amd_gpu.clone(), label });
    }
    choices
}

/// Which of `choices` a new machine starts on: NVIDIA's card if this PC has one, and else the first AMD.
pub fn default_choice(context: &Context, choices: &[CardChoice]) -> usize {
    let usual = default_linux_gpu(context);
    choices.iter().position(|choice| choice.pci == usual).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::amd::stand_in::fake_render_node;
    use crate::setup::cards::stand_in::{NAMES, fake_pci};
    use crate::testing::{Scratch, scratch_context};

    const IGPU: &str = "0000:7c:00.0";

    fn with_cards(scratch: &Scratch, nvidia: bool) -> Context {
        let mut context = scratch_context(scratch);
        context.pci_devices = scratch.path("pci");
        context.pci_names = scratch.write("pci.ids", NAMES);
        if nvidia {
            fake_pci(scratch, "0000:01:00.0", 0x2B85, "0x10de", "0x030000");
        }
        fake_render_node(scratch, 128, IGPU, "0x1002", 0x13c0);
        context
    }

    #[test]
    fn a_new_machine_is_offered_every_card_and_starts_on_nvidias() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        let choices = windows_card_choices(&context);
        let addresses: Vec<&str> = choices.iter().map(|choice| choice.pci.as_str()).collect();
        assert_eq!(addresses, ["", IGPU]);
        assert!(choices[0].label.contains("RTX 5090"), "{choices:?}");
        assert!(choices[1].label.ends_with("at 0000:7c:00.0"), "{choices:?}");
        assert_eq!(default_choice(&context, &choices), 0);
    }

    #[test]
    fn with_no_nvidia_card_a_new_machine_starts_on_amds() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, false);
        let choices = windows_card_choices(&context);
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[default_choice(&context, &choices)].pci, IGPU);
    }

    #[test]
    fn a_machine_is_offered_only_its_own_makers_cards() {
        let scratch = Scratch::new();
        let context = with_cards(&scratch, true);
        let on_nvidia = Machine::new("w", "W", "/d", "/v");
        let offered = same_maker_choices(&context, &on_nvidia);
        assert_eq!(offered.iter().map(|choice| choice.pci.as_str()).collect::<Vec<_>>(), [""]);
        let on_gone_amd = Machine { amd_gpu: "0000:0a:00.0".into(), ..on_nvidia };
        let offered = same_maker_choices(&context, &on_gone_amd);
        let addresses: Vec<&str> = offered.iter().map(|choice| choice.pci.as_str()).collect();
        assert_eq!(addresses, [IGPU, "0000:0a:00.0"]);
        assert!(offered[1].label.contains("isn't in this PC now"));
    }
}
