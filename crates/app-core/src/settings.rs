//! The small JSON files: what setup left behind, and what the app remembers between runs.

use crate::context::Context;
use crate::failure::Result;
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the driver's files go until setup has found them.
const DRIVER_FOLDER: &str = "setup/driver";
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;
/// The card's memory in MiB, in `setup.json`.
pub const CARD_MEMORY_FIELD: &str = "card_memory_mib";
/// The host keeps an eighth of the card, and at least 2 GiB, for its desktop.
const HOST_SHARE: u64 = 8;
const HOST_MINIMUM_GIB: u64 = 2;

pub type Fields = Map<String, Value>;

/// Replaces the file in one step, so a crash can't leave half of it.
pub fn write_json(path: &Path, fields: &impl Serialize) -> Result<()> {
    let text =
        serde_json::to_string_pretty(fields).map_err(|error| crate::Failure::new(error.to_string()))?;
    fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))?;
    let new = with_suffix(path, ".new");
    fs::write(&new, text + "\n")?;
    fs::rename(&new, path)?;
    Ok(())
}

pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    text.into()
}

/// A file of fields, which is empty when missing or bad. None of what these hold can stop the app.
fn read_fields(path: &Path) -> Fields {
    let text = fs::read_to_string(path).unwrap_or_default();
    match serde_json::from_str(&text) {
        Ok(Value::Object(fields)) => fields,
        _ => Fields::new(),
    }
}

/// What setup left behind, which is empty until it has run. The daemon reads this file too.
pub fn read_setup(context: &Context) -> Fields {
    read_fields(&context.paths.setup_file())
}

pub fn write_setup(context: &Context, changes: Fields) -> Result<()> {
    let mut fields = read_setup(context);
    fields.insert("version".into(), 1.into());
    fields.extend(changes);
    write_json(&context.paths.setup_file(), &fields)
}

pub fn read_prefs(context: &Context) -> Fields {
    read_fields(&context.paths.prefs_file())
}

pub fn write_prefs(context: &Context, changes: Fields) -> Result<()> {
    let mut fields = read_prefs(context);
    fields.extend(changes);
    write_json(&context.paths.prefs_file(), &fields)
}

/// The Windows driver's files, which setup took from the installer.
pub fn driver_folder(context: &Context) -> PathBuf {
    let setup = read_setup(context);
    let folder = setup.get("driver_folder").and_then(Value::as_str).map(PathBuf::from);
    folder.unwrap_or_else(|| context.paths.store.join(DRIVER_FOLDER))
}

/// The card's size in whole GiB, less what the host keeps.
fn video_budget_gib(dedicated: u64) -> u64 {
    let card = dedicated.div_ceil(GIB);
    card.saturating_sub((card / HOST_SHARE).max(HOST_MINIMUM_GIB))
}

/// What the daemon offers with no cap: the host's desktop needs the rest. 0 until setup knows the card.
pub fn max_vram_gib(context: &Context) -> u32 {
    let mib = read_setup(context).get(CARD_MEMORY_FIELD).and_then(Value::as_u64).unwrap_or(0);
    video_budget_gib(mib * MIB) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use serde_json::json;

    fn fields(value: Value) -> Fields {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn what_is_remembered_never_stops_the_app() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert!(read_prefs(&context).is_empty());
        write_prefs(&context, fields(json!({"width": 900, "monitor": "DP-1"}))).unwrap();
        write_prefs(&context, fields(json!({"width": 800}))).unwrap();
        assert_eq!(Value::Object(read_prefs(&context)), json!({"width": 800, "monitor": "DP-1"}));
        for bad in ["[1]", "{"] {
            fs::write(context.paths.prefs_file(), bad).unwrap();
            assert!(read_prefs(&context).is_empty());
        }
    }

    #[test]
    fn the_cap_follows_the_card_setup_found() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(max_vram_gib(&context), 0, "no card known yet");
        write_setup(&context, fields(json!({CARD_MEMORY_FIELD: 16 * 1024}))).unwrap();
        assert_eq!(max_vram_gib(&context), 14);
        write_setup(&context, fields(json!({CARD_MEMORY_FIELD: 32607}))).unwrap();
        assert_eq!(max_vram_gib(&context), 28, "28 GiB on a 32 GiB card");
    }
}
