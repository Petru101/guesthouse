//! What a Windows guest on an AMD card needs from setup: AMD's Windows driver, unpacked, and its
//! version. The daemon reads them from `amd.json` in the setup folder. The card itself is the
//! machine's own choice, so the file names none.

use crate::context::Context;
use crate::linux::amd::amd_card;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const AMD_SETUP_FILE: &str = "amd.json";
const DRIVER_FOLDER_FIELD: &str = "driver_folder";
pub const NO_AMD_SETUP: &str = "A Windows machine on an AMD card needs AMD's Windows driver unpacked first, and it \
                                hasn't been.";

fn amd_setup_file(context: &Context) -> PathBuf {
    context.paths.setup_folder().join(AMD_SETUP_FILE)
}

/// Where AMD's Windows driver was unpacked, if it was.
pub fn amd_driver_folder(context: &Context) -> Option<PathBuf> {
    let text = fs::read_to_string(amd_setup_file(context)).ok()?;
    let setup: Value = serde_json::from_str(&text).ok()?;
    let folder = PathBuf::from(setup.get(DRIVER_FOLDER_FIELD)?.as_str()?);
    folder.is_dir().then_some(folder)
}

/// Why a Windows machine can't start on the AMD card at `pci`, if it can't.
pub fn amd_start_problems(context: &Context, pci: &str) -> Vec<String> {
    let mut found = Vec::new();
    if amd_driver_folder(context).is_none() {
        found.push(NO_AMD_SETUP.to_owned());
    }
    if amd_card(context, pci).is_none() {
        found.push(format!(
            "The AMD card this machine runs on, at {pci}, isn't in this PC now. Choose another AMD card in \
             its settings."
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::amd::stand_in::fake_render_node;
    use crate::testing::{Scratch, scratch_context};

    const CARD: &str = "0000:7c:00.0";

    #[test]
    fn with_no_driver_and_no_card_both_are_said() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let found = amd_start_problems(&context, CARD);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0], NO_AMD_SETUP);
        assert!(found[1].contains("at 0000:7c:00.0, isn't in this PC now"));
    }

    #[test]
    fn with_the_driver_unpacked_and_the_card_here_it_can_start() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let driver = scratch.path("amd-driver");
        fs::create_dir_all(&driver).unwrap();
        fs::create_dir_all(context.paths.setup_folder()).unwrap();
        let setup = serde_json::json!({ DRIVER_FOLDER_FIELD: driver });
        fs::write(amd_setup_file(&context), setup.to_string()).unwrap();
        fake_render_node(&scratch, 128, CARD, "0x1002", 0x13c0);
        assert_eq!(amd_driver_folder(&context), Some(driver));
        assert_eq!(amd_start_problems(&context, CARD), Vec::<String>::new());
    }
}
