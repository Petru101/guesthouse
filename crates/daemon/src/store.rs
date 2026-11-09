//! Where the app keeps its data store, found the same way the app finds it.

use std::env;
use std::fs;
use std::path::PathBuf;

const STORE_VARIABLE: &str = "GUESTHOUSE_STORE";
const STORE_CONFIG: &str = ".config/guesthouse/store";
const STORE_DEFAULT: &str = ".local/share/guesthouse";

/// `GUESTHOUSE_STORE` if set, then the first line of the app's store setting, then the default.
pub fn store() -> PathBuf {
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
