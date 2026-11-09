//! Which GPU draws the desktop. On most laptops it's the processor's own graphics, and the NVIDIA
//! card only renders. That GPU can't take a buffer from the card's memory, so the viewer asks for its
//! frames in memory instead, and copies them.

use crate::usb::sysfs_text;
use std::fs;
use std::path::Path;

const NVIDIA_VENDOR: &str = "0x10de";
const CONNECTED: &str = "connected";
/// Where sysfs lists DRM devices and their connectors.
pub const DRM_CLASS: &str = "/sys/class/drm";
/// Set to 1 or 0 to choose, whatever the check says.
pub const MEMORY_FRAMES_VARIABLE: &str = "GUESTHOUSE_MEMORY_FRAMES";

/// Whether a GPU that isn't NVIDIA's has a screen connected, from sysfs's DRM class folder. Its
/// connectors are named after their card, as `card1-eDP-1`.
pub fn screen_on_other_gpu(drm_class: &Path) -> bool {
    let Ok(entries) = fs::read_dir(drm_class) else { return false };
    entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).any(|name| {
        let Some((card, _)) = name.split_once('-').filter(|(card, _)| card.starts_with("card")) else {
            return false;
        };
        let vendor = sysfs_text(&drm_class.join(card).join("device/vendor"));
        let status = sysfs_text(&drm_class.join(&name).join("status"));
        !vendor.is_empty() && vendor != NVIDIA_VENDOR && status == CONNECTED
    })
}

/// What the variable says, if it says 1 or 0.
pub fn chosen(value: Option<&str>) -> Option<bool> {
    match value.map(str::trim) {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// Whether the viewer should ask for its frames in memory. A copy can't show HDR, so then it isn't
/// offered.
pub fn wants_memory_frames(drm_class: &Path) -> bool {
    let variable = std::env::var(MEMORY_FRAMES_VARIABLE).ok();
    chosen(variable.as_deref()).unwrap_or_else(|| screen_on_other_gpu(drm_class))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn card(scratch: &Scratch, card: &str, vendor: &str, connectors: &[(&str, &str)]) {
        let device = scratch.path(&format!("drm/{card}/device"));
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("vendor"), format!("{vendor}\n")).unwrap();
        for (connector, status) in connectors {
            let folder = scratch.path(&format!("drm/{card}-{connector}"));
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("status"), format!("{status}\n")).unwrap();
        }
    }

    #[test]
    fn a_laptops_screen_on_its_own_graphics_is_found() {
        let scratch = Scratch::new();
        card(&scratch, "card0", "0x10de", &[]);
        card(&scratch, "card1", "0x8086", &[("eDP-1", "connected"), ("DP-1", "disconnected")]);
        assert!(screen_on_other_gpu(&scratch.path("drm")));
    }

    #[test]
    fn a_desktop_on_the_nvidia_card_keeps_its_buffers() {
        let scratch = Scratch::new();
        card(&scratch, "card0", "0x10de", &[("DP-1", "connected")]);
        card(&scratch, "card1", "0x1002", &[("HDMI-A-1", "disconnected")]);
        assert!(!screen_on_other_gpu(&scratch.path("drm")));
        assert!(!screen_on_other_gpu(&scratch.path("none")));
    }

    #[test]
    fn the_variable_decides_when_it_says_one_or_zero() {
        assert_eq!(chosen(Some("1")), Some(true));
        assert_eq!(chosen(Some("0")), Some(false));
        assert_eq!(chosen(Some("yes")), None);
        assert_eq!(chosen(None), None);
    }
}
