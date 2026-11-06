//! Finds the host's NVIDIA driver without asking any package manager, so it works on any distribution.
//!
//! The loader knows where the libraries are, whichever folder a distribution uses, and the version in
//! a library's own file name says what it is. Data files sit in a few well known places.

use super::driver_versions::DriverHost;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Debian's normal user has no `/sbin` on its path.
const LOADER_TOOLS: [&str; 3] = ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"];
const LOADER_LIST_FLAG: &str = "-p";
/// How the loader marks 64-bit libraries. A guest has no use for 32-bit ones.
const SIXTY_FOUR_BIT: &str = "x86-64";
const ARROW: &str = " => ";
const LIBRARY_MARK: &str = ".so";
const VERSION_MARK: &str = ".so.";

const DRIVER_LIBRARY_PREFIXES: [&str; 9] = [
    "libcuda",
    "libnvcuvid",
    "libnvidia-",
    "libnvoptix",
    "libGLX_nvidia",
    "libEGL_nvidia",
    "libGLESv1_CM_nvidia",
    "libGLESv2_nvidia",
    "libvdpau_nvidia",
];
/// These belong to the host's settings tool, and pull in a desktop a guest may not have.
const LEFT_OUT_LIBRARIES: [&str; 2] = ["libnvidia-gtk", "libnvidia-wayland-client"];

/// The first of these that's there goes in the image. Distributions differ on the Vulkan one.
const VULKAN_MANIFESTS: [&str; 3] = [
    "usr/share/vulkan/icd.d/nvidia_icd.x86_64.json",
    "usr/share/vulkan/icd.d/nvidia_icd.json",
    "etc/vulkan/icd.d/nvidia_icd.json",
];
const EGL_VENDORS: [&str; 2] =
    ["usr/share/glvnd/egl_vendor.d/10_nvidia.json", "etc/glvnd/egl_vendor.d/10_nvidia.json"];
const SMI_PROGRAMS: [&str; 2] = ["usr/bin/nvidia-smi", "bin/nvidia-smi"];
const EGL_PLATFORM_FOLDERS: [&str; 2] =
    ["usr/share/egl/egl_external_platform.d", "etc/egl/egl_external_platform.d"];

fn file_name(path: &Path) -> &str {
    path.file_name().and_then(|name| name.to_str()).unwrap_or_default()
}

pub fn is_driver_library(name: &str) -> bool {
    DRIVER_LIBRARY_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        && !LEFT_OUT_LIBRARIES.iter().any(|prefix| name.starts_with(prefix))
        && name.contains(LIBRARY_MARK)
}

/// The 64-bit libraries in what `ldconfig -p` printed.
pub(crate) fn libraries_in(listing: &str) -> Vec<PathBuf> {
    let lines = listing.lines().filter(|line| line.contains(SIXTY_FOUR_BIT));
    lines.filter_map(|line| line.split_once(ARROW)).map(|(_, path)| PathBuf::from(path.trim())).collect()
}

pub(crate) fn loader_listing() -> String {
    let tools = LOADER_TOOLS.iter().filter_map(|tool| Command::new(tool).arg(LOADER_LIST_FLAG).output().ok());
    let mut answers = tools.filter(|said| said.status.success());
    answers.next().map(|said| String::from_utf8_lossy(&said.stdout).into_owned()).unwrap_or_default()
}

/// A host as the loader and the file system show it. `root` is `/`, or a test's scratch folder.
pub struct LoaderHost {
    pub version_file: PathBuf,
    pub root: PathBuf,
    pub loader_listing: String,
}

impl LoaderHost {
    pub fn of_this_pc(version_file: PathBuf) -> Self {
        Self { version_file, root: "/".into(), loader_listing: loader_listing() }
    }

    fn driver_libraries(&self) -> Vec<PathBuf> {
        let found = libraries_in(&self.loader_listing);
        found.into_iter().filter(|path| is_driver_library(file_name(path))).collect()
    }

    /// Debian's links go through `/etc/alternatives` to a folder the loader doesn't list, so the
    /// folder a link ends in counts too.
    fn library_folders(&self) -> BTreeSet<PathBuf> {
        let mut folders = BTreeSet::new();
        for library in self.driver_libraries() {
            // On a merged system the loader says `/lib64`, the same folder as `/usr/lib64`.
            let listed = library.parent().and_then(|folder| fs::canonicalize(folder).ok());
            let target = fs::canonicalize(&library).ok();
            folders.extend(listed);
            folders.extend(target.and_then(|path| path.parent().map(Path::to_owned)));
        }
        folders
    }

    fn first_of(&self, places: &[&str]) -> Option<String> {
        let found = places.iter().map(|place| self.root.join(place)).find(|path| path.exists());
        found.map(|path| path.display().to_string())
    }
}

/// Plain files, and links to them. Opening a pipe that had a library's name would hang the app.
fn files_in(folder: &Path) -> Vec<PathBuf> {
    let entries = fs::read_dir(folder).into_iter().flatten().flatten();
    entries.map(|entry| entry.path()).filter(|path| path.is_file()).collect()
}

impl DriverHost for LoaderHost {
    fn loaded_version(&self) -> Option<String> {
        let text = fs::read_to_string(&self.version_file).ok()?;
        Some(text.trim().to_owned()).filter(|version| !version.is_empty())
    }

    /// What a library's name ends in, once every link is followed.
    fn library_version(&self, library: &str) -> Option<String> {
        let found = self.driver_libraries().into_iter().find(|path| file_name(path) == library)?;
        let target = fs::canonicalize(found).ok()?;
        let (_, version) = file_name(&target).split_once(VERSION_MARK)?;
        Some(version.to_owned())
    }

    fn driver_files(&self) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for folder in self.library_folders() {
            let libraries = files_in(&folder).into_iter().filter(|path| is_driver_library(file_name(path)));
            found.extend(libraries.map(|path| path.display().to_string()));
        }
        found.extend(self.first_of(&VULKAN_MANIFESTS));
        found.extend(self.first_of(&EGL_VENDORS));
        found.extend(self.first_of(&SMI_PROGRAMS));
        for folder in EGL_PLATFORM_FOLDERS {
            found.extend(files_in(&self.root.join(folder)).iter().map(|path| path.display().to_string()));
        }
        found.sort();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::driver_files::{chosen, missing, rewrite_icd};
    use crate::linux::driver_versions::{installed_versions, refusal};
    use crate::testing::Scratch;
    use std::os::unix::fs::symlink;

    const V: &str = "615.78.08";
    const VERSIONED: [&str; 12] = [
        "libcuda",
        "libnvidia-ml",
        "libnvidia-ptxjitcompiler",
        "libGLX_nvidia",
        "libEGL_nvidia",
        "libnvidia-glcore",
        "libnvidia-eglcore",
        "libnvidia-glsi",
        "libnvidia-glvkspirv",
        "libnvidia-gpucomp",
        "libnvidia-tls",
        "libnvidia-allocator",
    ];
    const OTHERS: [&str; 5] = [
        "libnvidia-egl-gbm.so.1",
        "libnvidia-egl-wayland.so.1",
        "libnvidia-egl-wayland2.so.1",
        "libnvidia-egl-xcb.so.1",
        "libnvidia-egl-xlib.so.1",
    ];
    const SONAMES: [(&str, &str); 3] = [("libcuda", "1"), ("libnvidia-ml", "1"), ("libGLX_nvidia", "0")];

    /// Where one distribution keeps things.
    struct Layout {
        library_folder: &'static str,
        /// Debian keeps some real files here, behind links through `etc/alternatives`.
        real_folder: Option<&'static str>,
        vulkan_manifest: &'static str,
    }

    const FEDORA: Layout = Layout {
        library_folder: "usr/lib64",
        real_folder: None,
        vulkan_manifest: "usr/share/vulkan/icd.d/nvidia_icd.x86_64.json",
    };
    const UBUNTU: Layout = Layout {
        library_folder: "usr/lib/x86_64-linux-gnu",
        real_folder: None,
        vulkan_manifest: "usr/share/vulkan/icd.d/nvidia_icd.json",
    };
    const DEBIAN: Layout = Layout {
        library_folder: "usr/lib/x86_64-linux-gnu",
        real_folder: Some("usr/lib/x86_64-linux-gnu/nvidia/current"),
        vulkan_manifest: "usr/share/vulkan/icd.d/nvidia_icd.json",
    };
    const ARCH: Layout = Layout {
        library_folder: "usr/lib",
        real_folder: None,
        vulkan_manifest: "usr/share/vulkan/icd.d/nvidia_icd.json",
    };
    /// NVIDIA's own installer, on a distribution that doesn't package the driver.
    const RUN_FILE: Layout = Layout {
        library_folder: "usr/lib64",
        real_folder: None,
        vulkan_manifest: "etc/vulkan/icd.d/nvidia_icd.json",
    };

    fn link_through_alternatives(scratch: &Scratch, link: &Path, target: &Path) {
        let name = file_name(link).to_owned();
        let alternative = scratch.path(&format!("etc/alternatives/{name}"));
        fs::create_dir_all(alternative.parent().unwrap()).unwrap();
        symlink(target, &alternative).unwrap();
        symlink(&alternative, link).unwrap();
    }

    fn library(scratch: &Scratch, layout: &Layout, stem: &str, version: &str) {
        let real = layout.real_folder.unwrap_or(layout.library_folder);
        scratch.write(&format!("{real}/{stem}.so.{version}"), "");
    }

    fn sonames(scratch: &Scratch, layout: &Layout, version: &str) -> String {
        let mut listing = String::from("1000 libs found in cache `/etc/ld.so.cache'\n");
        for (stem, number) in SONAMES {
            let real = layout.real_folder.unwrap_or(layout.library_folder);
            let target = scratch.path(&format!("{real}/{stem}.so.{version}"));
            let link = scratch.path(&format!("{}/{stem}.so.{number}", layout.library_folder));
            if layout.real_folder.is_some() {
                link_through_alternatives(scratch, &link, &target);
            } else {
                symlink(format!("{stem}.so.{version}"), &link).unwrap();
            }
            listing += &format!("\t{stem}.so.{number} (libc6,x86-64) => {}\n", link.display());
            // 32-bit copies are listed too, and must be passed over.
            listing += &format!("\t{stem}.so.{number} (libc6) => /usr/lib32/{stem}.so.{number}\n");
        }
        listing
    }

    fn host_on(scratch: &Scratch, layout: &Layout, version: &str) -> LoaderHost {
        for stem in VERSIONED {
            library(scratch, layout, stem, version);
        }
        for name in OTHERS {
            scratch.write(&format!("{}/{name}", layout.library_folder), "");
        }
        scratch.write(&format!("{}/libsomething-else.so.1", layout.library_folder), "");
        scratch.write(&format!("{}/libnvidia-gtk3.so.{version}", layout.library_folder), "");
        scratch.write(layout.vulkan_manifest, r#"{ "library_path": "libGLX_nvidia.so.0" }"#);
        scratch.write("usr/share/glvnd/egl_vendor.d/10_nvidia.json", "{}");
        scratch.write("usr/share/egl/egl_external_platform.d/15_nvidia_gbm.json", "{}");
        scratch.write("usr/bin/nvidia-smi", "");
        LoaderHost {
            version_file: scratch.write("version", format!("{V}\n")),
            root: scratch.path(""),
            loader_listing: sonames(scratch, layout, version),
        }
    }

    fn places(host: &LoaderHost) -> Vec<String> {
        chosen(&host.driver_files()).into_iter().map(|(_, place)| place).collect()
    }

    fn finds_everything(layout: &Layout) {
        let scratch = Scratch::new();
        let host = host_on(&scratch, layout, V);
        assert_eq!(refusal(host.loaded_version().as_deref(), &installed_versions(&host)), None);
        let found = places(&host);
        assert_eq!(missing(&found, V), Vec::<String>::new());
        assert!(!found.iter().any(|place| place.contains("something-else") || place.contains("gtk")));
    }

    #[test]
    fn fedora() {
        finds_everything(&FEDORA);
    }

    #[test]
    fn ubuntu() {
        finds_everything(&UBUNTU);
    }

    #[test]
    fn debian_through_its_alternatives() {
        finds_everything(&DEBIAN);
    }

    #[test]
    fn arch() {
        finds_everything(&ARCH);
    }

    #[test]
    fn nvidias_own_installer() {
        finds_everything(&RUN_FILE);
    }

    #[test]
    fn a_folder_the_loader_names_through_a_link_counts_once() {
        let scratch = Scratch::new();
        let mut host = host_on(&scratch, &FEDORA, V);
        symlink("usr/lib64", scratch.path("lib64")).unwrap();
        let through_link = scratch.path("lib64").display().to_string();
        let real = scratch.path("usr/lib64").display().to_string();
        host.loader_listing = host.loader_listing.replace(&real, &through_link);
        let found = places(&host);
        let unique: BTreeSet<&String> = found.iter().collect();
        assert_eq!(found.len(), unique.len());
        assert_eq!(missing(&found, V), Vec::<String>::new());
    }

    #[test]
    fn only_files_count_whatever_they_are_called() {
        let scratch = Scratch::new();
        let host = host_on(&scratch, &ARCH, V);
        fs::create_dir(scratch.path("usr/lib/libnvidia-folder.so.1")).unwrap();
        let pipe = scratch.path("usr/lib/libnvidia-pipe.so.1");
        assert!(Command::new("mkfifo").arg(&pipe).status().unwrap().success());
        symlink("libnvidia-gone.so.9", scratch.path("usr/lib/libnvidia-dangling.so.1")).unwrap();
        let found = host.driver_files();
        assert!(
            !found.iter().any(|file| ["folder", "pipe", "dangling"].iter().any(|name| file.contains(name)))
        );
        assert!(found.iter().any(|file| file.contains("libcuda")));
    }

    #[test]
    fn a_new_driver_on_disk_is_a_pending_reboot_in_every_layout() {
        for layout in [FEDORA, UBUNTU, DEBIAN, ARCH] {
            let scratch = Scratch::new();
            let host = host_on(&scratch, &layout, "615.90.01");
            let said = refusal(Some(V), &installed_versions(&host)).unwrap();
            assert!(said.contains("615.90.01") && said.contains("Reboot"));
        }
    }

    #[test]
    fn old_files_left_behind_are_not_a_pending_reboot() {
        let scratch = Scratch::new();
        let host = host_on(&scratch, &ARCH, V);
        scratch.write("usr/lib/libcuda.so.610.43.02", "");
        assert_eq!(refusal(Some(V), &installed_versions(&host)), None);
    }

    #[test]
    fn no_driver_at_all_says_the_loader_cant_find_it() {
        let scratch = Scratch::new();
        let host = LoaderHost {
            version_file: scratch.write("version", format!("{V}\n")),
            root: scratch.path(""),
            loader_listing: String::new(),
        };
        assert!(refusal(Some(V), &installed_versions(&host)).unwrap().contains("can't find"));
    }

    #[test]
    fn a_manifest_with_no_folder_still_points_at_the_guests_copy() {
        let scratch = Scratch::new();
        let host = host_on(&scratch, &ARCH, V);
        let manifest = host.driver_files().into_iter().find(|file| file.contains("nvidia_icd")).unwrap();
        let text = rewrite_icd(&fs::read_to_string(manifest).unwrap());
        assert!(text.contains("/opt/nv/driver/lib/libGLX_nvidia.so.0"));
    }
}
