//! Capturing the mouse, for games: the compositor locks the pointer, and motion goes to the guest as
//! relative movement. One key does both: it captures, and pressed again it lets go.

use super::capture_state::{Capture, Step};
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

    /// The key or the button: starts a capture, or ends the one there is.
    pub fn toggle_capture(&self) {
        let (next, step) = self.0.capture.get().toggled();
        self.0.capture.set(next);
        self.take(step);
    }

    /// Ends a capture at whatever stage it is, as when the focus goes.
    pub fn stop_capture(&self) {
        let (next, step) = self.0.capture.get().dropped();
        self.0.capture.set(next);
        self.take(step);
    }

    /// Passes on the pointer's motion, and follows the lock as the compositor grants or ends it.
    pub(super) fn follow_capture(&self) {
        let state = self.0.capture.get();
        if !state.asked() {
            return;
        }
        let (motion, ended, held) = {
            let mut lock = self.0.lock.borrow_mut();
            let Some(lock) = lock.as_mut() else { return };
            (lock.take_motion(), lock.ended(), lock.held())
        };
        let alive = self.0.guest.borrow().as_ref().is_some_and(Guest::alive);
        let (next, step) = state.followed(held, ended, alive);
        self.0.capture.set(next);
        if step != Step::Nothing {
            self.take(step);
        } else if next.held()
            && motion != (0, 0)
            && let Some(guest) = self.0.guest.borrow_mut().as_mut()
        {
            guest.nudge(motion.0, motion.1);
        }
    }

    fn take(&self, step: Step) {
        match step {
            Step::Nothing => return,
            Step::Ask => self.ask_for_lock(),
            Step::Enter => self.enter_capture(),
            Step::Leave { had_mouse } => self.leave_capture(had_mouse),
        }
        self.report_capture();
    }

    /// Asks for the lock. Nothing changes for the guest until the compositor grants it, and with no
    /// pointer to lock there's no capture to wait for.
    fn ask_for_lock(&self) {
        self.0.picture.grab_focus();
        let surface = self.0.root.native().and_then(|native| native.surface());
        let asked = match (self.0.lock.borrow_mut().as_mut(), surface) {
            (Some(lock), Some(surface)) => lock.lock(&surface),
            _ => false,
        };
        if !asked {
            self.0.capture.set(Capture::Off);
        }
    }

    fn enter_capture(&self) {
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
    fn leave_capture(&self, had_mouse: bool) {
        if let Some(lock) = self.0.lock.borrow_mut().as_mut() {
            lock.unlock();
        }
        if !had_mouse {
            return;
        }
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
}
