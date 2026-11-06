//! Which GTK this PC has, without loading it, as `guesthouse-check` runs before GTK may be here. The
//! library's own file name says, as 4.14.2 is `libgtk-4.so.1.1400.2`.

use crate::linux::driver_host::{libraries_in, loader_listing};
use std::fs;
use std::path::Path;

const GTK_LIBRARY: &str = "libgtk-4.so.1";
const MINORS_ARE_HUNDREDS: u32 = 100;

fn version_in_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix(GTK_LIBRARY)?.strip_prefix('.')?;
    let number: u32 = rest.split('.').next()?.parse().ok()?;
    Some(format!("4.{}", number / MINORS_ARE_HUNDREDS))
}

fn version_of(library: &Path) -> Option<String> {
    let real = fs::canonicalize(library).ok()?;
    version_in_name(real.file_name()?.to_str()?)
}

/// The version of the GTK the loader would use, like `4.14`.
pub fn gtk_version() -> Option<String> {
    let libraries = libraries_in(&loader_listing());
    let library = libraries.iter().find(|path| path.file_name().is_some_and(|name| name == GTK_LIBRARY))?;
    version_of(library)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_gives_the_version() {
        assert_eq!(version_in_name("libgtk-4.so.1.1400.2").as_deref(), Some("4.14"));
        assert_eq!(version_in_name("libgtk-4.so.1.2000.1").as_deref(), Some("4.20"));
    }

    #[test]
    fn a_name_with_no_version_says_nothing() {
        assert_eq!(version_in_name("libgtk-4.so.1"), None);
        assert_eq!(version_in_name("libgtk-3.so.0.2400.1"), None);
    }
}
