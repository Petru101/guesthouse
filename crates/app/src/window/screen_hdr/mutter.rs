//! Asking Mutter over D-Bus, for a GNOME whose Mutter doesn't offer the colour management protocol.

use super::Window;
use gtk::{gio, glib};
use guesthouse_app_core::display::ScreenHdr;

const BUS_NAME: &str = "org.gnome.Mutter.DisplayConfig";
const OBJECT_PATH: &str = "/org/gnome/Mutter/DisplayConfig";
const CALL_MILLISECONDS: i32 = 2000;
const COLOUR_MODE: &str = "color-mode";
const COLOUR_MODE_BT2100: u32 = 1;
/// Where the monitors are in Mutter's reply, and where each one's properties are.
const MONITORS_AT: usize = 1;
const PROPERTIES_AT: usize = 2;

/// Reads `GetCurrentState`'s reply: `(serial, monitors, logical monitors, properties)`. `None` is a
/// reply of another shape, or one that doesn't say.
fn hdr_in_reply(reply: &glib::Variant) -> Option<bool> {
    if !reply.is_container() || reply.n_children() <= MONITORS_AT {
        return None;
    }
    let monitors = reply.child_value(MONITORS_AT);
    if !monitors.is_container() {
        return None;
    }
    let mut modes = Vec::new();
    for index in 0..monitors.n_children() {
        let monitor = monitors.child_value(index);
        if !monitor.is_container() || monitor.n_children() <= PROPERTIES_AT {
            return None;
        }
        let properties = monitor.child_value(PROPERTIES_AT);
        if !properties.is_type(glib::VariantTy::VARDICT) {
            return None;
        }
        modes.extend(glib::VariantDict::new(Some(&properties)).lookup::<u32>(COLOUR_MODE).ok().flatten());
    }
    (!modes.is_empty()).then(|| modes.contains(&COLOUR_MODE_BT2100))
}

fn answer_from(reply: Option<&glib::Variant>) -> ScreenHdr {
    match reply.and_then(hdr_in_reply) {
        Some(true) => ScreenHdr::Shows,
        Some(false) => ScreenHdr::Cannot,
        None => ScreenHdr::CantTell,
    }
}

/// Asks, and leaves the answer when it comes. Without Mutter nobody can say, and that means SDR.
pub fn ask(window: &Window) {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        window.remember_screen_hdr(ScreenHdr::CantTell);
        return;
    };
    let window = window.clone();
    bus.call(
        Some(BUS_NAME),
        OBJECT_PATH,
        BUS_NAME,
        "GetCurrentState",
        None,
        None,
        gio::DBusCallFlags::NONE,
        CALL_MILLISECONDS,
        gio::Cancellable::NONE,
        move |reply| window.remember_screen_hdr(answer_from(reply.ok().as_ref())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;
    use std::collections::HashMap;

    fn monitor(colour_mode: Option<u32>) -> glib::Variant {
        let mut properties: HashMap<String, glib::Variant> = HashMap::new();
        if let Some(mode) = colour_mode {
            properties.insert(COLOUR_MODE.to_owned(), mode.to_variant());
        }
        (("DP-1", "", "", ""), Vec::<u32>::new(), properties).to_variant()
    }

    fn reply(monitors: &[glib::Variant]) -> glib::Variant {
        let list = glib::Variant::array_from_iter_with_type(monitors[0].type_(), monitors.iter());
        glib::Variant::tuple_from_iter([1u32.to_variant(), list])
    }

    #[test]
    fn one_monitor_in_hdr_is_enough() {
        let found = reply(&[monitor(Some(0)), monitor(Some(COLOUR_MODE_BT2100))]);
        assert_eq!(answer_from(Some(&found)), ScreenHdr::Shows);
    }

    #[test]
    fn monitors_that_all_say_no_mean_no() {
        assert_eq!(answer_from(Some(&reply(&[monitor(Some(0))]))), ScreenHdr::Cannot);
    }

    #[test]
    fn no_answer_is_sdr_and_says_so() {
        assert_eq!(answer_from(None), ScreenHdr::CantTell);
        assert_eq!(answer_from(Some(&reply(&[monitor(None)]))), ScreenHdr::CantTell);
    }

    #[test]
    fn a_reply_of_another_shape_is_not_a_panic() {
        for odd in [
            7u32.to_variant(),
            (1u32,).to_variant(),
            (1u32, 2u32).to_variant(),
            (1u32, vec![1u32]).to_variant(),
        ] {
            assert_eq!(answer_from(Some(&odd)), ScreenHdr::CantTell, "{odd}");
        }
    }
}
