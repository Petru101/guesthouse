//! Which card a guest runs on. Each machine keeps its own: the app names an AMD card by its PCI
//! address in [`AMD_CARD_VARIABLE`], and with none the guest is on the NVIDIA card setup chose.

use guesthouse_backend::PciAddress;
use std::env;

pub const AMD_CARD_VARIABLE: &str = "GUESTHOUSE_AMD_CARD";

/// The AMD card the app named, if it named one.
pub fn amd_card() -> Result<Option<PciAddress>, String> {
    amd_card_from(env::var(AMD_CARD_VARIABLE).ok().as_deref())
}

fn amd_card_from(text: Option<&str>) -> Result<Option<PciAddress>, String> {
    match text.map(str::trim) {
        None | Some("") => Ok(None),
        Some(text) => PciAddress::parse(text)
            .map(Some)
            .ok_or_else(|| format!("{AMD_CARD_VARIABLE} is {text}, which is not a PCI address.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_card_named_is_the_nvidia_card() {
        assert_eq!(amd_card_from(None), Ok(None));
        assert_eq!(amd_card_from(Some("")), Ok(None));
    }

    #[test]
    fn a_named_card_is_read_as_its_address() {
        let card = amd_card_from(Some("0000:7c:00.0")).unwrap().unwrap();
        assert_eq!(card.to_string(), "0000:7c:00.0");
    }

    #[test]
    fn a_name_that_is_not_an_address_is_refused() {
        assert!(amd_card_from(Some("renderD128")).unwrap_err().contains("not a PCI address"));
    }
}
