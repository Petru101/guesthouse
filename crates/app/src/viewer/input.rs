//! The keyboard and mouse, from the screen to the guest.

use super::screen::Screen;
use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app_core::input::TABLET_RANGE;

/// GDK adds the X11 offset to the Linux key code.
const X11_KEY_OFFSET: u32 = 8;

pub(super) fn button_name(number: u32) -> Option<&'static str> {
    match number {
        1 => Some("left"),
        2 => Some("middle"),
        3 => Some("right"),
        _ => None,
    }
}

impl Screen {
    pub(super) fn add_input(&self) {
        let picture = &self.0.picture;
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |_, x, y| this.on_motion(x, y));
        picture.add_controller(motion);
        let click = gtk::GestureClick::builder().button(0).build();
        let this = self.clone();
        click.connect_pressed(move |gesture, _, x, y| this.on_button(gesture, x, y, true));
        let this = self.clone();
        click.connect_released(move |gesture, _, x, y| this.on_button(gesture, x, y, false));
        picture.add_controller(click);
        picture.add_controller(self.scroll_controller(gtk::PropagationPhase::Bubble));
        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, _, code, _| {
            this.on_key(code, true);
            glib::Propagation::Stop
        });
        let this = self.clone();
        keys.connect_key_released(move |_, _, code, _| this.on_key(code, false));
        picture.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let this = self.clone();
        focus.connect_enter(move |_| this.on_focus(true));
        let this = self.clone();
        focus.connect_leave(move |_| this.on_focus(false));
        picture.add_controller(focus);
    }

    pub(super) fn scroll_controller(&self, phase: gtk::PropagationPhase) -> gtk::EventControllerScroll {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(phase);
        let this = self.clone();
        scroll.connect_scroll(move |_, _, dy| {
            if let Some(input) = this.0.forwarded.borrow_mut().as_mut() {
                input.scroll(dy as i32);
                return glib::Propagation::Stop;
            }
            if let Some(guest) = this.0.guest.borrow_mut().as_mut() {
                let name = if dy > 0.0 { "wheel-down" } else { "wheel-up" };
                guest.button(name, true);
                guest.button(name, false);
            }
            glib::Propagation::Stop
        });
        scroll
    }

    pub(super) fn on_motion(&self, x: f64, y: f64) {
        if self.0.captured.get() {
            return;
        }
        if let Some(input) = self.0.forwarded.borrow_mut().as_mut() {
            input.move_to(self.in_frame(x, self.frame_width()), self.in_frame(y, self.frame_height()));
            return;
        }
        let picture = &self.0.picture;
        let scaled = |value: f64, size: i32| (value * TABLET_RANGE as f64 / f64::from(size.max(1))) as i64;
        let position = (scaled(x, picture.width()), scaled(y, picture.height()));
        // GTK repeats the last position every time it redraws.
        if self.0.pointer.get() != Some(position)
            && let Some(guest) = self.0.guest.borrow_mut().as_mut()
        {
            self.0.pointer.set(Some(position));
            guest.move_to(position.0, position.1);
        }
    }

    fn frame_width(&self) -> (i32, i32) {
        let size = self.0.latest.borrow().as_ref().map_or(0, |texture| texture.width());
        (size, self.0.picture.width())
    }

    fn frame_height(&self) -> (i32, i32) {
        let size = self.0.latest.borrow().as_ref().map_or(0, |texture| texture.height());
        (size, self.0.picture.height())
    }

    /// From the widget's pixels to the guest's frame's, which an Ubuntu guest's session wants.
    fn in_frame(&self, value: f64, (frame, widget): (i32, i32)) -> f64 {
        value * f64::from(frame) / f64::from(widget.max(1))
    }

    pub(super) fn on_button(&self, gesture: &gtk::GestureClick, x: f64, y: f64, down: bool) {
        if down {
            self.0.picture.grab_focus();
        }
        if let Some(name) = button_name(gesture.current_button()) {
            self.on_motion(x, y);
            self.press(name, down);
        }
    }

    pub(super) fn press(&self, name: &'static str, down: bool) {
        if let Some(input) = self.0.forwarded.borrow_mut().as_mut() {
            input.button(name, down);
            return;
        }
        if let Some(guest) = self.0.guest.borrow_mut().as_mut() {
            guest.button(name, down);
            let mut held = self.0.buttons_down.borrow_mut();
            if down {
                held.insert(name);
            } else {
                held.remove(name);
            }
        }
    }

    pub(super) fn on_key(&self, keycode: u32, down: bool) {
        let code = keycode.saturating_sub(X11_KEY_OFFSET);
        if let Some(input) = self.0.forwarded.borrow_mut().as_mut() {
            input.key(code, down);
            return;
        }
        let mut held = self.0.keys_down.borrow_mut();
        if let Some(guest) = self.0.guest.borrow_mut().as_mut()
            && (down || held.contains(&code))
        {
            guest.key(code, down);
            if down {
                held.insert(code);
            } else {
                held.remove(&code);
            }
        }
    }

    pub(super) fn toplevel(&self) -> Option<gdk::Toplevel> {
        self.0.root.native()?.surface()?.downcast().ok()
    }

    pub(super) fn on_focus(&self, inside: bool) {
        if !inside {
            self.stop_capture();
            self.let_go();
        }
        if let Some(surface) = self.toplevel() {
            if inside {
                surface.inhibit_system_shortcuts(None::<&gdk::Event>);
            } else {
                surface.restore_system_shortcuts();
            }
        }
    }

    pub fn shortcuts_inhibited(&self) -> bool {
        self.toplevel().is_some_and(|surface| surface.is_shortcuts_inhibited())
    }

    /// Hands the desktop its shortcuts back. A click on the screen takes them again.
    pub fn release(&self) {
        if let Some(root) = self.0.root.root() {
            gtk::prelude::RootExt::set_focus(&root, None::<&gtk::Widget>);
        }
        self.on_focus(false);
    }

    /// Lifts what is held down, because the guest won't see those keys come up otherwise.
    pub(super) fn let_go(&self) {
        let mut held = self.0.keys_down.borrow_mut();
        if let Some(guest) = self.0.guest.borrow_mut().as_mut() {
            for code in held.iter() {
                guest.key(*code, false);
            }
        }
        held.clear();
        self.lift_buttons();
    }

    pub(super) fn lift_buttons(&self) {
        let mut held = self.0.buttons_down.borrow_mut();
        if let Some(guest) = self.0.guest.borrow_mut().as_mut() {
            for name in held.iter() {
                guest.button(name, false);
            }
        }
        held.clear();
    }
}
