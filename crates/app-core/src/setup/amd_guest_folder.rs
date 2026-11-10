//! The folder of AMD's driver that the guest gets. The AMD daemon tells Windows where each driver is
//! by its file name in that one folder, so every file it names has to be in it.
//!
//! AMD's package for RDNA1 and RDNA2 keeps them all in the user-mode driver's folder. The package for
//! RDNA3 and later keeps Vulkan and OpenGL as components of their own, each in a folder beside that
//! one, which AMD's installer puts in place separately. Their files are added to the user-mode
//! driver's folder here, so the guest gets one complete folder from either package.

use crate::failure::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// The components with folders of their own in the newer cards' package: Vulkan, then OpenGL.
const COMPONENTS: [&str; 2] = ["amdvlk", "amdogl"];
/// What a component has only for installing it, which nothing in the guest reads. The older cards'
/// folder has none of these.
const INSTALL_ONLY: [&str; 2] = ["inf", "cat"];
/// Every file the AMD daemon names to Windows: the Direct3D drivers, the kernel driver the adapter is
/// said to have, and the Vulkan and OpenGL drivers. A test holds the daemon to this list.
pub const NAMED_FILES: [&str; 13] = [
    "amdxc64.dll",
    "amdxc32.dll",
    "amdxx64.dll",
    "amdxx32.dll",
    "atidx9loader64.dll",
    "atidx9loader32.dll",
    "amdkmdag.sys",
    "amd-vulkan64.json",
    "amd-vulkan32.json",
    "atig6pxx.dll",
    "atiglpxx.dll",
    "atio6axx.dll",
    "atioglxx.dll",
];
/// Vulkan's manifests name the library the loader opens, beside them.
const VULKAN_MANIFESTS: [&str; 2] = ["amd-vulkan64.json", "amd-vulkan32.json"];
const SAME_FOLDER: &str = ".\\";
/// A copy is written under this name first, so a half-made one is never taken for the file.
const PART: &str = ".part";

/// Adds the files of the components beside `user_driver` to it. A file already there stays, as the
/// user-mode driver's own copy is the one it was built with. Doing it again changes nothing.
pub fn add_components(user_driver: &Path) -> Result<()> {
    let Some(driver) = user_driver.parent() else { return Ok(()) };
    for component in COMPONENTS {
        let Ok(entries) = fs::read_dir(driver.join(component)) else { continue };
        for entry in entries.flatten() {
            let from = entry.path();
            let install_only = from.extension().is_some_and(|extension| {
                INSTALL_ONLY.iter().any(|skipped| extension.eq_ignore_ascii_case(skipped))
            });
            let to = user_driver.join(entry.file_name());
            if from.is_file() && !install_only && !to.exists() {
                link_or_copy(&from, &to)?;
            }
        }
    }
    Ok(())
}

/// A link where it can be, as the files are big and nothing writes to them.
fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    if fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    let part = to.with_file_name(format!("{}{PART}", to.file_name().unwrap_or_default().to_string_lossy()));
    fs::copy(from, &part)?;
    fs::rename(&part, to)?;
    Ok(())
}

/// The libraries a Vulkan manifest in the folder names, for its driver and its layer.
fn manifest_libraries(user_driver: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for manifest in VULKAN_MANIFESTS {
        let Ok(text) = fs::read_to_string(user_driver.join(manifest)) else { continue };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else { continue };
        for part in ["ICD", "layer"] {
            if let Some(path) = manifest[part]["library_path"].as_str() {
                found.push(path.strip_prefix(SAME_FOLDER).unwrap_or(path).to_owned());
            }
        }
    }
    found.dedup();
    found
}

/// The files Windows will be told to load that aren't in the folder.
pub fn missing_files(user_driver: &Path) -> Vec<String> {
    let mut wanted: Vec<String> = NAMED_FILES.iter().map(|name| (*name).to_owned()).collect();
    wanted.extend(manifest_libraries(user_driver));
    wanted.retain(|name| !user_driver.join(name).is_file());
    wanted
}

/// Says what the folder lacks, if it lacks anything.
pub fn folder_problem(user_driver: &Path) -> Option<String> {
    let missing = missing_files(user_driver);
    (!missing.is_empty()).then(|| {
        format!(
            "AMD's driver lacks {}, which Windows is told to load. Unpack AMD's installer for this card again.",
            missing.join(", ")
        )
    })
}

#[cfg(test)]
pub mod stand_in {
    use super::*;
    use crate::testing::Scratch;

    /// A Vulkan manifest whose driver and layer are `library`, beside it, as AMD's are.
    pub fn manifest(library: &str) -> String {
        let path = format!("{SAME_FOLDER}{library}");
        serde_json::json!({"ICD": {"library_path": path}, "layer": {"library_path": path}}).to_string()
    }

    /// The files of each kind, laid out as AMD's packages for 26.10.2 have them: the Direct3D and
    /// kernel drivers and OpenGL's loaders in the user-mode driver's folder, and Vulkan and OpenGL with
    /// their install files and a tool both carry.
    const USER_DRIVER_FILES: [&str; 10] = [
        "amdxc64.dll",
        "amdxc32.dll",
        "amdxx64.dll",
        "amdxx32.dll",
        "atidx9loader64.dll",
        "atidx9loader32.dll",
        "amdkmdag.sys",
        "atig6pxx.dll",
        "atiglpxx.dll",
        "regamdcomp.exe",
    ];
    const VULKAN_FILES: [&str; 6] =
        ["amdvlk64.dll", "amdvlk32.dll", "vulkan64.dll", "amdvlk.inf", "amdvlk.cat", "regamdcomp.exe"];
    const OPENGL_FILES: [&str; 5] =
        ["atio6axx.dll", "atioglxx.dll", "amdogl.inf", "amdogl.cat", "regamdcomp.exe"];

    /// Writes the user-mode driver's folder `folder`, with its components beside it or inside it, in
    /// the display driver's folder `driver` of `scratch`. Each file holds its own path.
    pub fn lay_out_driver(scratch: &Scratch, driver: &str, folder: &str, components_inside: bool) {
        let write = |path: String| {
            scratch.write(&path, &path);
        };
        for name in USER_DRIVER_FILES {
            write(format!("{driver}/{folder}/{name}"));
        }
        let place = |component: &str| {
            if components_inside { format!("{driver}/{folder}") } else { format!("{driver}/{component}") }
        };
        for (component, files) in [("amdvlk", &VULKAN_FILES[..]), ("amdogl", &OPENGL_FILES[..])] {
            for name in files {
                // The older cards' folder has no component install files.
                let skipped = name.ends_with(".inf") || name.ends_with(".cat") || *name == "regamdcomp.exe";
                if !(components_inside && skipped) {
                    write(format!("{}/{name}", place(component)));
                }
            }
        }
        let vulkan = place("amdvlk");
        scratch.write(&format!("{vulkan}/amd-vulkan64.json"), manifest("amdvlk64.dll"));
        scratch.write(&format!("{vulkan}/amd-vulkan32.json"), manifest("amdvlk32.dll"));
        // OpenCL is a component in both packages, and the daemon names none of its files.
        write(format!("{driver}/amdocl/amdocl64.dll"));
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::lay_out_driver;
    use super::*;
    use crate::testing::Scratch;
    use regex::Regex;
    use std::collections::BTreeSet;

    const NEWER: &str = "Display/WT6A_INF";
    const OLDER: &str = "Display2/WT6A_INF";

    fn names(folder: &Path) -> BTreeSet<String> {
        let entries = fs::read_dir(folder).unwrap().flatten();
        entries.map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_newer_cards_folder_lacks_vulkan_and_opengl_until_their_components_are_added() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, NEWER, "B026716", false);
        let folder = scratch.path(&format!("{NEWER}/B026716"));
        let missing = missing_files(&folder);
        for name in ["amd-vulkan64.json", "amd-vulkan32.json", "atio6axx.dll", "atioglxx.dll"] {
            assert!(missing.contains(&name.to_owned()), "{name} in {missing:?}");
        }
        assert!(folder_problem(&folder).unwrap().contains("atio6axx.dll"));
        add_components(&folder).unwrap();
        assert_eq!(missing_files(&folder), Vec::<String>::new());
        assert!(folder.join("amdvlk64.dll").is_file() && folder.join("amdvlk32.dll").is_file());
        assert!(folder.join("vulkan64.dll").is_file(), "all of a component, as the older folder has");
        for left_out in ["amdvlk.inf", "amdvlk.cat", "amdogl.inf", "amdocl64.dll"] {
            assert!(!folder.join(left_out).exists(), "{left_out}");
        }
        let kept = fs::read_to_string(folder.join("regamdcomp.exe")).unwrap();
        assert!(kept.ends_with("B026716/regamdcomp.exe"), "the folder's own copy stays: {kept}");
    }

    #[test]
    fn the_older_cards_folder_is_complete_as_it_comes_and_stays_as_it_is() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, OLDER, "B026715", true);
        let folder = scratch.path(&format!("{OLDER}/B026715"));
        let before = names(&folder);
        assert_eq!(missing_files(&folder), Vec::<String>::new());
        add_components(&folder).unwrap();
        assert_eq!(names(&folder), before);
    }

    #[test]
    fn adding_the_components_again_changes_nothing() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, NEWER, "B026716", false);
        let folder = scratch.path(&format!("{NEWER}/B026716"));
        add_components(&folder).unwrap();
        let once = names(&folder);
        add_components(&folder).unwrap();
        assert_eq!(names(&folder), once);
        assert!(!once.iter().any(|name| name.ends_with(PART)));
    }

    #[test]
    fn a_library_a_vulkan_manifest_names_has_to_be_there() {
        let scratch = Scratch::new();
        lay_out_driver(&scratch, OLDER, "B026715", true);
        let folder = scratch.path(&format!("{OLDER}/B026715"));
        fs::remove_file(folder.join("amdvlk32.dll")).unwrap();
        assert_eq!(missing_files(&folder), ["amdvlk32.dll"]);
    }

    /// The file names in the daemon's answers, from its source: a name the daemon gives that isn't
    /// here would never be checked for.
    #[test]
    fn the_named_files_are_the_ones_the_daemon_names() {
        let file_name = Regex::new(r#""([A-Za-z0-9_-]+\.(?:dll|json|sys))""#).unwrap();
        let mut named = BTreeSet::new();
        for source in ["guest_driver.rs", "api_drivers.rs"] {
            let path = format!("{}/../backend-amd/src/protocol/{source}", env!("CARGO_MANIFEST_DIR"));
            let text = fs::read_to_string(path).unwrap();
            named.extend(file_name.captures_iter(&text).map(|found| found[1].to_owned()));
        }
        let ours: BTreeSet<String> = NAMED_FILES.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(named, ours);
    }
}
