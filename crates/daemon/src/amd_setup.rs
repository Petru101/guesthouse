//! What an AMD guest was set up with. The app's `setup.json` stays the NVIDIA card's record, which
//! this daemon does not read: a Windows guest on the AMD card is described by `setup/amd.json` in
//! the same store.

use guesthouse_backend::{DriverVersion, PciAddress, Setup};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;

const STORE_VARIABLE: &str = "GUESTHOUSE_STORE";
const STORE_CONFIG: &str = ".config/guesthouse/store";
const STORE_DEFAULT: &str = ".local/share/guesthouse";
const AMD_FILE: &str = "setup/amd.json";

fn store() -> PathBuf {
    let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    if let Some(folder) = env::var_os(STORE_VARIABLE).filter(|folder| !folder.is_empty()) {
        return folder.into();
    }
    let configured = fs::read_to_string(home.join(STORE_CONFIG)).ok();
    let first_line = configured.as_deref().and_then(|text| text.lines().next()).map(str::trim);
    match first_line.filter(|line| !line.is_empty()) {
        Some(line) => line.strip_prefix("~/").map_or_else(|| line.into(), |rest| home.join(rest)),
        None => home.join(STORE_DEFAULT),
    }
}

fn parse_setup(text: &str) -> Result<Setup, String> {
    let setup: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let folder = setup["driver_folder"].as_str().ok_or("The AMD setup names no driver folder.")?;
    let driver_folder = PathBuf::from(folder);
    if !driver_folder.is_dir() {
        return Err(format!("The AMD driver folder {} is not there.", driver_folder.display()));
    }
    let version = setup["driver_version"].as_str().ok_or("The AMD setup has no guest driver version.")?;
    let guest_driver_version = DriverVersion::parse(version)
        .ok_or_else(|| format!("The AMD guest driver version {version} is not four numbers."))?;
    let card_address = match setup["pci"].as_str() {
        Some(text) => Some(
            PciAddress::parse(text)
                .ok_or_else(|| format!("The AMD card address {text} is not a PCI address."))?,
        ),
        None => None,
    };
    Ok(Setup {
        guest_branch: guest_driver_version.to_string(),
        guest_driver_version,
        driver_folder,
        card: None,
        card_address,
    })
}

/// Reads the AMD guest file from the store.
pub fn read_setup() -> Result<Setup, String> {
    let path = store().join(AMD_FILE);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("AMD setup has not been written ({}: {error}).", path.display()))?;
    parse_setup(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    #[test]
    fn the_version_and_the_card_address_come_from_the_file() {
        let folder = env::temp_dir().join(format!("guesthouse-amd-setup-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        let text = format!(
            "{{\"driver_folder\": {:?}, \"driver_version\": \"32.0.32015.12012\", \"pci\": \"0000:7c:00.0\"}}",
            folder.to_str().unwrap()
        );
        let setup = parse_setup(&text).unwrap();
        assert_eq!(setup.guest_driver_version.to_string(), "32.0.32015.12012");
        assert_eq!(setup.card_address.map(|address| address.to_string()).as_deref(), Some("0000:7c:00.0"));
        assert_eq!(setup.card, None);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_version_that_is_not_four_numbers_is_refused() {
        let folder = env::temp_dir().join(format!("guesthouse-amd-setup-bad-{}", process::id()));
        fs::create_dir_all(&folder).unwrap();
        let text =
            format!("{{\"driver_folder\": {:?}, \"driver_version\": \"617.42\"}}", folder.to_str().unwrap());
        assert!(parse_setup(&text).unwrap_err().contains("four numbers"));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_missing_driver_folder_is_refused() {
        let text = "{\"driver_folder\": \"/no/such/amd-driver\", \"driver_version\": \"32.0.32015.12012\"}";
        assert!(parse_setup(text).unwrap_err().contains("not there"));
    }
}
