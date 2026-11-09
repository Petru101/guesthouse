//! The Windows driver a machine's guest is given at install: NVIDIA's from setup, or AMD's for a
//! machine on an AMD card. The guest keeps it in the folder of the host driver store whose name the
//! daemon gives Windows, so the two names here must match the backends'.

use super::amd::{NO_AMD_SETUP, amd_driver_folder};
use crate::context::Context;
use crate::library::NO_DRIVER_FILES;
use crate::machine::Machine;
use crate::settings::driver_folder;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestDriver {
    /// Where the driver's files are on this PC.
    pub folder: PathBuf,
    /// The folder they're in on the install disc, which the guest's setup script looks for.
    pub disc_folder: &'static str,
    /// The folder in the guest's host driver store they go to.
    pub store_folder: &'static str,
}

const NVIDIA_DISC_FOLDER: &str = "nvidia";
const NVIDIA_STORE_FOLDER: &str = "nv_dispi.inf_amd64_gpupv";
const AMD_DISC_FOLDER: &str = "amd";
const AMD_STORE_FOLDER: &str = "amdxc.inf_amd64_gpupv";

/// The driver for this machine's guest, or why there is none yet.
pub fn guest_driver(context: &Context, machine: &Machine) -> Result<GuestDriver, &'static str> {
    if machine.windows_amd_card().is_some() {
        let folder = amd_driver_folder(context).ok_or(NO_AMD_SETUP)?;
        return Ok(GuestDriver { folder, disc_folder: AMD_DISC_FOLDER, store_folder: AMD_STORE_FOLDER });
    }
    let folder = driver_folder(context);
    if !folder.is_dir() {
        return Err(NO_DRIVER_FILES);
    }
    Ok(GuestDriver { folder, disc_folder: NVIDIA_DISC_FOLDER, store_folder: NVIDIA_STORE_FOLDER })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;

    #[test]
    fn a_machine_on_nvidia_gets_setups_driver_and_one_on_amd_gets_amds() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = Machine::new("w", "W", "/d", "/v");
        let on_amd = Machine { amd_gpu: "0000:7c:00.0".into(), ..windows.clone() };
        assert_eq!(guest_driver(&context, &windows), Err(NO_DRIVER_FILES));
        assert_eq!(guest_driver(&context, &on_amd), Err(NO_AMD_SETUP));
        fs::create_dir_all(driver_folder(&context)).unwrap();
        let nvidia = guest_driver(&context, &windows).unwrap();
        assert_eq!((nvidia.disc_folder, nvidia.store_folder), (NVIDIA_DISC_FOLDER, NVIDIA_STORE_FOLDER));
        assert_eq!(guest_driver(&context, &on_amd), Err(NO_AMD_SETUP), "NVIDIA's driver is not AMD's");
    }

    /// The store folders are the names the daemons tell Windows, which the backends hold.
    #[test]
    fn the_store_folders_are_the_ones_the_daemons_name() {
        let backends = [
            (NVIDIA_STORE_FOLDER, "backend-nvidia/src/answers/registry.rs"),
            (AMD_STORE_FOLDER, "backend-amd/src/protocol/guest_driver.rs"),
        ];
        for (store, source) in backends {
            let path = format!("{}/../{source}", env!("CARGO_MANIFEST_DIR"));
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.contains(&format!("FileRepository\\\\{store}\"")), "{store} is not in {source}");
        }
    }
}
