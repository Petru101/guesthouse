//! Capturing the mouse, for games: the compositor locks the pointer, and motion goes to the guest as
//! relative movement.

use super::input::button_name;
use super::screen::Screen;
use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app_core::input::{Guest, MOUSE, TABLET};

impl Screen {
    /// Why the mouse can't be captured now.
    pub fn capture_problem(&self) -> Option<&'static str> {
        if self.0.lock.borrow().is_none() {
            return Some("This desktop cannot lock the pointer");
        }
        if !self.has_input() {
            return Some("The guest is not taking input yet");
        }
        let has_mouse = self.0.guest.borrow().as_ref().is_some_and(Guest::has_mouse);
        (!has_mouse).then_some("This VM was started without a USB mouse")
    }

    pub(super) fn report_capture(&self) {
        (self.0.capture_changed.borrow())();
    }

    /// Asks for the lock. Nothing changes for the guest until the compositor grants it.
    pub fn start_capture(&self) {
        self.0.picture.grab_focus();
        let surface = self.0.root.native().and_then(|native| native.surface());
        if let (Some(lock), Some(surface)) = (self.0.lock.borrow_mut().as_mut(), surface) {
            self.0.capture_asked.set(lock.lock(&surface));
        }
        self.report_capture();
    }

    pub(super) fn enter_capture(&self) {
        self.0.captured.set(true);
        // They went down on the other device.
        self.lift_buttons();
        if let Some(guest) = self.0.guest.borrow_mut().as_mut() {
            guest.point_with(MOUSE);
        }
        // The pointer stays where it was, so it may be over the header, and the guest still wants every
        // click. The window sees them first this way.
        if let Some(root) = self.0.root.root() {
            root.set_cursor_from_name(Some("none"));
            let buttons = gtk::EventControllerLegacy::new();
            buttons.set_propagation_phase(gtk::PropagationPhase::Capture);
            let this = self.clone();
            buttons.connect_event(move |_, event| this.on_capture_event(event));
            let scroll = self.scroll_controller(gtk::PropagationPhase::Capture);
            let controllers: Vec<gtk::EventController> = vec![buttons.upcast(), scroll.upcast()];
            for controller in &controllers {
                root.add_controller(controller.clone());
            }
            *self.0.capture_input.borrow_mut() = controllers;
        }
        self.report_capture();
    }

    /// Not a gesture: one gives up when a second button goes down, as when aiming and firing.
    pub(super) fn on_capture_event(&self, event: &gdk::Event) -> glib::Propagation {
        let Some(button) = event.downcast_ref::<gdk::ButtonEvent>() else {
            return glib::Propagation::Proceed;
        };
        if let Some(name) = button_name(button.button()) {
            self.press(name, event.event_type() == gdk::EventType::ButtonPress);
        }
        glib::Propagation::Stop
    }

    /// Part of the way out, so it waits for nothing: what the guest is told only goes in a queue.
    pub fn stop_capture(&self) {
        if !self.0.capture_asked.replace(false) {
            return;
        }
        if let Some(lock) = self.0.lock.borrow_mut().as_mut() {
            lock.unlock();
        }
        if self.0.captured.replace(false) {
            self.lift_buttons();
            if let Some(guest) = self.0.guest.borrow_mut().as_mut() {
                guest.point_with(TABLET);
            }
            if let Some(root) = self.0.root.root() {
                root.set_cursor(None);
                for controller in self.0.capture_input.borrow_mut().drain(..) {
                    root.remove_controller(&controller);
                }
            }
            // The guest's cursor has moved, so the next position counts.
            self.0.pointer.set(None);
        }
        self.report_capture();
    }

    /// Passes on the pointer's motion. The compositor ends a lock when the window loses focus.
    pub(super) fn follow_capture(&self) {
        if !self.0.capture_asked.get() {
            return;
        }
        let (motion, ended, held) = {
            let mut lock = self.0.lock.borrow_mut();
            let Some(lock) = lock.as_mut() else { return };
            (lock.take_motion(), lock.ended(), lock.held())
        };
        let alive = self.0.guest.borrow().as_ref().is_some_and(Guest::alive);
        if ended || !alive {
            self.stop_capture();
        } else if self.0.captured.get() && motion != (0, 0) {
            if let Some(guest) = self.0.guest.borrow_mut().as_mut() {
                guest.nudge(motion.0, motion.1);
            }
        } else if held && !self.0.captured.get() {
            self.enter_capture();
        }
    }
}
