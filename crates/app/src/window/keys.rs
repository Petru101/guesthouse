//! The keys that always belong to the host. They're seen before anything reaches the guest and never
//! passed on, and acting on them needs nothing from the guest, QEMU or the daemon.

use super::Window;
use gtk::gdk;
use gtk::glib::Propagation;
use gtk::prelude::*;

const HOST_MODIFIERS: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK.union(gdk::ModifierType::ALT_MASK);

fn is_host_key(key: gdk::Key) -> bool {
    [gdk::Key::m, gdk::Key::p, gdk::Key::Return, gdk::Key::KP_Enter].contains(&key)
}

pub fn add_host_keys(window: &Window) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let handle = window.clone();
    keys.connect_key_pressed(move |_, key, _, state| on_key(&handle, key, state));
    let handle = window.clone();
    keys.connect_key_released(move |_, key, _, _| {
        if is_host_key(key.to_lower()) {
            handle.0.host_key_down.set(false);
        }
    });
    window.widget().add_controller(keys);
}

fn on_key(window: &Window, key: gdk::Key, state: gdk::ModifierType) -> Propagation {
    let page = window.0.screen_page.borrow().clone();
    // Lower case, as Shift or Caps Lock can make it a capital.
    let key = key.to_lower();
    let Some(page) = page.filter(|_| state.contains(HOST_MODIFIERS) && is_host_key(key)) else {
        return Propagation::Proceed;
    };
    // A held key repeats, and must not flip fullscreen back and forth.
    if !window.0.host_key_down.replace(true) {
        match key {
            gdk::Key::m => page.toggle_capture(),
            gdk::Key::p => page.toggle_overlay(),
            _ => page.toggle_fullscreen(),
        }
    }
    Propagation::Stop
}
