//! What the host says about its driver and cards, from files the kernel module keeps. Nothing here
//! opens a device.

use std::fs;
use std::path::Path;

const VERSION_FILE: &str = "/sys/module/nvidia/version";
const TAINT_FILE: &str = "/sys/module/nvidia/taint";
const CARDS_FOLDER: &str = "/proc/driver/nvidia/gpus";
const INFORMATION_FILE: &str = "information";
/// The kernel's taint letter for a module with a proprietary licence. The open modules are GPL and
/// never set it.
const PROPRIETARY_TAINT: char = 'P';

/// One card, as its information file describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardListed {
    /// Its number in the driver, which is its device file's.
    pub minor: u32,
    pub model: String,
}

/// What the module's files say. Nothing in it identifies the PC or the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub version: String,
    pub open_module: bool,
    /// The cards, by number.
    pub cards: Vec<CardListed>,
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

/// Reads the field after `label:` in an information file.
fn field<'a>(information: &'a str, label: &str) -> Option<&'a str> {
    information.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == label).then(|| value.trim())
    })
}

/// A card from its information file. Only the model and the number are kept: the file also holds
/// the card's UUID, which stays there.
pub fn parse_card(information: &str) -> Option<CardListed> {
    let minor = field(information, "Device Minor")?.parse().ok()?;
    let model = field(information, "Model")?.to_owned();
    Some(CardListed { minor, model })
}

pub fn parse_taint_as_open(taint: &str) -> bool {
    !taint.contains(PROPRIETARY_TAINT)
}

fn read_cards(folder: &Path) -> Vec<CardListed> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let mut cards: Vec<CardListed> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join(INFORMATION_FILE)).ok())
        .filter_map(|information| parse_card(&information))
        .collect();
    cards.sort_by_key(|card| card.minor);
    cards
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
    let cards = read_cards(cards);
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
    fn a_card_is_its_model_and_number_and_nothing_else() {
        let card = parse_card(INFORMATION).unwrap();
        assert_eq!(card, CardListed { minor: 1, model: "NVIDIA GeForce RTX 4080".to_owned() });
        assert!(!format!("{card:?}").contains("GPU-"));
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
}
