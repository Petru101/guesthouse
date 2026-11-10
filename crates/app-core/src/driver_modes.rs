//! The modes our display driver gives Windows, read from its `vdd_settings.xml` the way the driver
//! reads it, and the same settings with a rate added that the driver doesn't ship with. The driver
//! makes its list once, as it starts, and takes a new one live by `SETMODES`.

use std::collections::BTreeSet;

/// The settings file, in the driver's package and in the guest.
pub const SETTINGS_FILE: &str = "vdd_settings.xml";
/// The driver's command that replaces its modes and shows Windows the monitor again with them.
const SET_MODES: &str = "SETMODES";
/// Windows dropped the monitor when its list had about 100 modes, so the driver keeps only this many.
pub const MOST_MODES: usize = 96;
/// The longest command the driver's control device reads whole.
pub const MOST_COMMAND_LETTERS: usize = 2047;
const GLOBAL_END: &str = "</global>";
const RATE_TAG: &str = "g_refresh_rate";
const COMMENT_START: &str = "<!--";
const COMMENT_END: &str = "-->";

/// A mode as the driver lists it: width, height and rate.
pub type Mode = (u32, u32, u32);

/// The driver's modes, in its order: each size with its own rate, then every size at each global rate.
/// The first is the one the monitor calls its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriverModes(Vec<Mode>);

impl DriverModes {
    /// What the driver makes of these settings. Like the driver, it goes by each element's text in
    /// order. A rate here is a whole number, which is all the shipped settings have.
    pub fn read(settings: &str) -> Self {
        let (mut width, mut height) = (None, None);
        let mut modes = Vec::new();
        let mut sizes = BTreeSet::new();
        let mut global_rates = Vec::new();
        for (name, value) in elements(settings) {
            let number = value.trim().parse::<u32>().ok();
            match name {
                "width" => width = number,
                "height" => {
                    height = number;
                    sizes.extend(width.zip(height));
                }
                "refresh_rate" => {
                    let size = width.zip(height);
                    modes.extend(size.zip(number).map(|((width, height), hz)| (width, height, hz)));
                }
                RATE_TAG => global_rates.extend(number),
                _ => {}
            }
        }
        for hz in global_rates {
            modes.extend(sizes.iter().map(|(width, height)| (*width, *height, hz)));
        }
        Self(modes)
    }

    pub fn has(&self, mode: Mode) -> bool {
        self.0.contains(&mode)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The driver's command to take these modes, as they are.
    pub fn command(&self) -> String {
        let modes: Vec<String> =
            self.0.iter().map(|(width, height, hz)| format!("{width}x{height}x{hz}")).collect();
        format!("{SET_MODES} {}", modes.join(","))
    }
}

/// Each element's name and its text, in order, with comments left out.
fn elements(settings: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut rest = settings;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        if let Some(comment) = rest.strip_prefix(COMMENT_START) {
            rest = comment.split_once(COMMENT_END).map_or("", |(_, after)| after);
            continue;
        }
        let Some((tag, after)) = rest[1..].split_once('>') else { break };
        rest = after;
        if tag.starts_with(['/', '?', '!']) || tag.ends_with('/') {
            continue;
        }
        let name = tag.split_whitespace().next().unwrap_or_default();
        let text = &after[..after.find('<').unwrap_or(after.len())];
        if !text.trim().is_empty() {
            found.push((name, text));
        }
    }
    found
}

/// The settings with `hz` among the global rates, so the driver offers it at every size. Settings
/// that have it already come back as they are.
pub fn with_rate(settings: &str, hz: u32) -> String {
    let has_it =
        elements(settings).iter().any(|(name, value)| *name == RATE_TAG && value.trim() == hz.to_string());
    match settings.rfind(GLOBAL_END) {
        Some(at) if !has_it => {
            format!("{}\t<{RATE_TAG}>{hz}</{RATE_TAG}>\n\t{}", &settings[..at], &settings[at..])
        }
        _ => settings.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{MAX_REFRESH_HZ, MIN_REFRESH_HZ, REFRESH_RATES, SCREENS};

    /// The settings the driver ships with.
    const SHIPPED: &str = include_str!("../../../drivers/windows-display/vdd_settings.xml");

    fn size(screen: &str) -> (u32, u32) {
        let (width, height) = screen.split_once('x').unwrap();
        (width.parse().unwrap(), height.parse().unwrap())
    }

    #[test]
    fn the_shipped_settings_give_each_size_its_own_rate_then_every_global_rate() {
        let modes = DriverModes::read(SHIPPED);
        assert_eq!(modes.0[0], (800, 600, 30), "the first size's own rate comes first");
        assert_eq!(modes.0[5], (800, 600, 60), "then the global rates, smallest size first");
        assert_eq!(modes.0[6], (1366, 768, 60));
        assert_eq!(modes.len(), 5 + 5 * REFRESH_RATES.len());
        for screen in SCREENS {
            let (width, height) = size(screen);
            for hz in REFRESH_RATES {
                assert!(modes.has((width, height, hz)), "{screen} at {hz}");
            }
            assert!(modes.has((width, height, MIN_REFRESH_HZ)), "{screen} at {MIN_REFRESH_HZ}");
        }
        assert!(!modes.has((1920, 1080, 180)));
    }

    #[test]
    fn a_rate_the_driver_lacks_is_added_at_every_size_and_only_once() {
        let settings = with_rate(SHIPPED, 180);
        let modes = DriverModes::read(&settings);
        for screen in SCREENS {
            let (width, height) = size(screen);
            assert!(modes.has((width, height, 180)), "{screen}");
        }
        assert_eq!(modes.0[0], (800, 600, 30), "the monitor's own mode stays as it was");
        assert_eq!(with_rate(&settings, 180), settings);
        assert_eq!(with_rate(SHIPPED, 144), SHIPPED, "a rate it ships with changes nothing");
        assert!(settings.contains(
            "<g_refresh_rate>240</g_refresh_rate>\n\t\t<g_refresh_rate>180</g_refresh_rate>\n\t</global>"
        ));
    }

    #[test]
    fn any_rate_the_app_takes_stays_within_what_the_driver_keeps() {
        for hz in MIN_REFRESH_HZ..=MAX_REFRESH_HZ {
            let modes = DriverModes::read(&with_rate(SHIPPED, hz));
            assert!(modes.has((1920, 1080, hz)), "{hz}");
            assert!(modes.len() <= MOST_MODES, "{hz} makes {}", modes.len());
            assert!(modes.command().len() <= MOST_COMMAND_LETTERS, "{hz}");
        }
    }

    #[test]
    fn the_command_lists_every_mode_in_order() {
        let modes = DriverModes::read(
            "<r><width>800</width><height>600</height><refresh_rate>30</refresh_rate></r>\
             <!-- <g_refresh_rate>999</g_refresh_rate> --><global><g_refresh_rate>60</g_refresh_rate></global>",
        );
        assert_eq!(modes.command(), "SETMODES 800x600x30,800x600x60");
    }

    #[test]
    fn a_value_the_driver_cant_read_is_skipped() {
        let modes =
            DriverModes::read("<width>800</width><height>600</height><g_refresh_rate>fast</g_refresh_rate>");
        assert!(modes.is_empty());
    }
}
