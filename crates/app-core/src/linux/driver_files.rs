//! Picks which of the host's files go in a guest's driver image, and where. `driver_host` finds them,
//! so a library added in a later version comes along by itself. `REQUIRED` says what must be there for
//! the guest to work at all.

use super::driver_host::is_driver_library;
use regex::Regex;

const SMI: &str = "nvidia-smi";
const EGL_VENDOR: &str = "10_nvidia.json";
const VULKAN_MANIFEST_START: &str = "nvidia_icd";
const VULKAN_MANIFEST_IN_IMAGE: &str = "json/nvidia_icd.x86_64.json";
/// A 64-bit guest can't load the 32-bit library this one names.
const THIRTY_TWO_BIT_MARK: &str = "i686";
const EGL_PLATFORM_FOLDER: &str = "/egl_external_platform.d/";
/// Where the driver is in the guest, which the Vulkan manifest has to say.
const GUEST_LIBRARIES: &str = "/opt/nv/driver/lib/";
const MANIFEST_LIBRARY: &str = r#"("library_path"\s*:\s*")(?:[^"]*/)?([^"/]+")"#;
const DRIVER: &str = "NVIDIA's driver";
/// The image's name for a file, and what brings it if it's missing. `{v}` is the version.
const REQUIRED: [(&str, &str); 20] = [
    ("lib/libcuda.so.{v}", DRIVER),
    ("lib/libnvidia-ml.so.{v}", DRIVER),
    ("lib/libnvidia-ptxjitcompiler.so.{v}", DRIVER),
    ("lib/libGLX_nvidia.so.{v}", DRIVER),
    ("lib/libEGL_nvidia.so.{v}", DRIVER),
    ("lib/libnvidia-glcore.so.{v}", DRIVER),
    ("lib/libnvidia-eglcore.so.{v}", DRIVER),
    ("lib/libnvidia-glsi.so.{v}", DRIVER),
    ("lib/libnvidia-glvkspirv.so.{v}", DRIVER),
    ("lib/libnvidia-gpucomp.so.{v}", DRIVER),
    ("lib/libnvidia-tls.so.{v}", DRIVER),
    ("lib/libnvidia-allocator.so.{v}", DRIVER),
    ("lib/libnvidia-egl-gbm.so.1", "NVIDIA's egl-gbm"),
    ("lib/libnvidia-egl-wayland.so.1", "NVIDIA's egl-wayland"),
    ("lib/libnvidia-egl-wayland2.so.1", "NVIDIA's egl-wayland2"),
    ("lib/libnvidia-egl-xcb.so.1", "NVIDIA's egl-x11"),
    ("lib/libnvidia-egl-xlib.so.1", "NVIDIA's egl-x11"),
    ("bin/nvidia-smi", DRIVER),
    ("json/10_nvidia.json", DRIVER),
    ("json/nvidia_icd.x86_64.json", DRIVER),
];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Where a file of the driver goes in the image, or `None` if a guest has no use for it.
pub fn place(path: &str) -> Option<String> {
    let name = file_name(path);
    if name == SMI {
        return Some(format!("bin/{SMI}"));
    }
    if path.contains(EGL_PLATFORM_FOLDER) {
        return name.ends_with(".json").then(|| format!("json/egl_external_platform.d/{name}"));
    }
    if name == EGL_VENDOR {
        return Some(format!("json/{EGL_VENDOR}"));
    }
    if name.starts_with(VULKAN_MANIFEST_START) {
        return (!name.contains(THIRTY_TWO_BIT_MARK)).then(|| VULKAN_MANIFEST_IN_IMAGE.to_owned());
    }
    is_driver_library(name).then(|| format!("lib/{name}"))
}

/// Pairs of a file on the host and its path in the image.
pub fn chosen(paths: &[String]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> =
        paths.iter().filter_map(|path| Some((path.clone(), place(path)?))).collect();
    // A name can be in two folders, as on Debian. An image holds it once.
    pairs.sort_by(|one, other| one.1.cmp(&other.1));
    pairs.dedup_by(|one, other| one.1 == other.1);
    pairs
}

/// What the image would lack, as lines a person can act on.
pub fn missing(places: &[String], version: &str) -> Vec<String> {
    let needed = REQUIRED.iter().map(|(name, source)| (name.replace("{v}", version), source));
    needed
        .filter(|(name, _)| !places.contains(name))
        .map(|(name, source)| format!("{name}, from {source}"))
        .collect()
}

/// A host's manifest names its own folder, or none. In the guest the driver is where the image is mounted.
pub fn rewrite_icd(text: &str) -> String {
    let library = Regex::new(MANIFEST_LIBRARY).expect("a valid pattern");
    library.replace(text, format!("${{1}}{GUEST_LIBRARIES}${{2}}")).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libraries_manifests_and_tools_each_find_their_place() {
        assert_eq!(place("/usr/lib64/libcuda.so.615.78.08").as_deref(), Some("lib/libcuda.so.615.78.08"));
        assert_eq!(place("/usr/bin/nvidia-smi").as_deref(), Some("bin/nvidia-smi"));
        let manifest = "/usr/share/egl/egl_external_platform.d/15_nvidia_gbm.json";
        assert_eq!(place(manifest).as_deref(), Some("json/egl_external_platform.d/15_nvidia_gbm.json"));
    }

    #[test]
    fn what_a_guest_has_no_use_for_stays_out() {
        assert_eq!(place("/usr/lib64/nvidia/xorg/libglxserver_nvidia.so"), None);
        assert_eq!(place("/usr/share/vulkan/icd.d/nvidia_icd.i686.json"), None);
        assert_eq!(place("/usr/share/doc/README"), None);
    }

    #[test]
    fn what_is_missing_says_what_brings_it() {
        let all: Vec<String> = REQUIRED.iter().map(|(name, _)| name.replace("{v}", "615.78.08")).collect();
        assert!(missing(&all, "615.78.08").is_empty());
        let lacking = missing(&all[1..], "615.78.08");
        assert_eq!(lacking, ["lib/libcuda.so.615.78.08, from NVIDIA's driver"]);
    }

    #[test]
    fn the_vulkan_manifest_points_at_the_guests_copy() {
        let wanted = r#"{ "library_path": "/opt/nv/driver/lib/libGLX_nvidia.so.0" }"#;
        assert_eq!(rewrite_icd(r#"{ "library_path": "/usr/lib64/libGLX_nvidia.so.0" }"#), wanted);
        assert_eq!(rewrite_icd(r#"{ "library_path": "libGLX_nvidia.so.0" }"#), wanted);
    }
}
