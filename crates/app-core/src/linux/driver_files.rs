//! Picks which of the host's files go in a guest's driver image, and where. Everything comes from the
//! package database, not a fixed list, so a library added in a later version comes along by itself.
//! `REQUIRED` says what must be there for the guest to work at all.

/// The EGL platform libraries are Fedora's own packages, not the driver's, but the driver loads them
/// when it starts, and a compositor has all five mapped.
pub const PACKAGES: [&str; 7] = [
    "xorg-x11-drv-nvidia-libs",
    "xorg-x11-drv-nvidia-cuda-libs",
    "xorg-x11-drv-nvidia-cuda",
    "egl-gbm",
    "egl-wayland",
    "egl-wayland2",
    "egl-x11",
];
const LIBRARY_FOLDER: &str = "/usr/lib64/";
const LOOSE_FILES: [(&str, &str); 3] = [
    ("/usr/bin/nvidia-smi", "bin/nvidia-smi"),
    ("/usr/share/glvnd/egl_vendor.d/10_nvidia.json", "json/10_nvidia.json"),
    ("/usr/share/vulkan/icd.d/nvidia_icd.x86_64.json", "json/nvidia_icd.x86_64.json"),
];
const EGL_PLATFORM_FOLDER: &str = "/usr/share/egl/egl_external_platform.d/";
/// Where the driver is in the guest, which the Vulkan manifest has to say.
const GUEST_LIBRARIES: &str = "/opt/nv/driver/lib/";
/// The image's name for a file, and the package to install if it's missing. `{v}` is the version.
const REQUIRED: [(&str, &str); 20] = [
    ("lib/libcuda.so.{v}", "xorg-x11-drv-nvidia-cuda-libs"),
    ("lib/libnvidia-ml.so.{v}", "xorg-x11-drv-nvidia-cuda-libs"),
    ("lib/libnvidia-ptxjitcompiler.so.{v}", "xorg-x11-drv-nvidia-cuda-libs"),
    ("lib/libGLX_nvidia.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libEGL_nvidia.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-glcore.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-eglcore.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-glsi.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-glvkspirv.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-gpucomp.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-tls.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-allocator.so.{v}", "xorg-x11-drv-nvidia-libs"),
    ("lib/libnvidia-egl-gbm.so.1", "egl-gbm"),
    ("lib/libnvidia-egl-wayland.so.1", "egl-wayland"),
    ("lib/libnvidia-egl-wayland2.so.1", "egl-wayland2"),
    ("lib/libnvidia-egl-xcb.so.1", "egl-x11"),
    ("lib/libnvidia-egl-xlib.so.1", "egl-x11"),
    ("bin/nvidia-smi", "xorg-x11-drv-nvidia-cuda"),
    ("json/10_nvidia.json", "xorg-x11-drv-nvidia-libs"),
    ("json/nvidia_icd.x86_64.json", "xorg-x11-drv-nvidia-libs"),
];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Where a packaged file goes in the image, or `None` if a guest has no use for it.
pub fn place(path: &str) -> Option<String> {
    if let Some((_, place)) = LOOSE_FILES.iter().find(|(file, _)| *file == path) {
        return Some((*place).to_owned());
    }
    if path.starts_with(EGL_PLATFORM_FOLDER) && path.ends_with(".json") {
        return Some(format!("json/egl_external_platform.d/{}", file_name(path)));
    }
    let library =
        path.strip_prefix(LIBRARY_FOLDER).filter(|name| name.contains(".so") && !name.contains('/'));
    library.map(|name| format!("lib/{name}"))
}

/// Pairs of a file on the host and its path in the image.
pub fn chosen(paths: &[String]) -> Vec<(String, String)> {
    paths.iter().filter_map(|path| Some((path.clone(), place(path)?))).collect()
}

/// What the image would lack, as lines a person can act on.
pub fn missing(places: &[String], version: &str) -> Vec<String> {
    let needed = REQUIRED.iter().map(|(name, package)| (name.replace("{v}", version), package));
    needed
        .filter(|(name, _)| !places.contains(name))
        .map(|(name, package)| format!("{name}, from the package {package}"))
        .collect()
}

/// Fedora's manifest names `/usr/lib64`. In the guest the driver is where the image is mounted.
pub fn rewrite_icd(text: &str) -> String {
    text.replace(LIBRARY_FOLDER, GUEST_LIBRARIES)
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
        assert_eq!(place("/usr/share/doc/README"), None);
    }

    #[test]
    fn what_is_missing_names_the_package_to_install() {
        let all: Vec<String> = REQUIRED.iter().map(|(name, _)| name.replace("{v}", "615.78.08")).collect();
        assert!(missing(&all, "615.78.08").is_empty());
        let lacking = missing(&all[1..], "615.78.08");
        assert_eq!(lacking, ["lib/libcuda.so.615.78.08, from the package xorg-x11-drv-nvidia-cuda-libs"]);
    }

    #[test]
    fn the_vulkan_manifest_points_at_the_guests_copy() {
        assert_eq!(rewrite_icd("/usr/lib64/libGLX_nvidia.so.0"), "/opt/nv/driver/lib/libGLX_nvidia.so.0");
    }
}
