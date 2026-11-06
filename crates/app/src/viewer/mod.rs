//! The page that shows a guest's screen, with its toolbar, the mouse, fullscreen and the thumbnail.

mod capture;
mod clipboard;
mod dbus_frames;
mod display_attach;
mod display_surface;
mod frames;
mod import;
mod input;
mod monitors;
mod overlay;
mod pointer_lock;
mod ring;
mod screen;
mod socket_frames;

use crate::common::in_background;
use crate::window::{MachineHandle, Window};
use adw::prelude::*;
use clipboard::SharedClipboard;
use gtk::glib;
use guesthouse_app_core::library::save_thumbnail;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::tasks::{State, Status, slot_of};
use monitors::Monitors;
use screen::Screen;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub const RELEASE_KEYS: &str = "Ctrl+Alt+G";
pub const FULLSCREEN_KEYS: &str = "Ctrl+Alt+Return";
pub const CAPTURE_KEYS: &str = "Ctrl+Alt+M";
pub const OVERLAY_KEYS: &str = "Ctrl+Alt+P";
/// Pixels at the top of the screen that bring the header back in fullscreen.
const TOP_EDGE: f64 = 2.0;
const USUAL_HEADER_HEIGHT: i32 = 48;
/// Kept at twice the size it's shown, for screens that scale by two.
const THUMBNAIL_WIDTH: usize = 256;
const THUMBNAIL_EVERY: Duration = Duration::from_secs(30);

struct Parts {
    window: Window,
    machine: MachineHandle,
    page: adw::NavigationPage,
    title: adw::WindowTitle,
    toolbar: adw::ToolbarView,
    fullscreen_button: adw::SplitButton,
    capture_button: gtk::Button,
    screen: RefCell<Option<Screen>>,
    clipboard: RefCell<Option<SharedClipboard>>,
    monitors: RefCell<Option<Rc<Monitors>>>,
    fullscreen_handler: RefCell<Option<glib::SignalHandlerId>>,
    thumbnail_source: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct ScreenPage(Rc<Parts>);

impl ScreenPage {
    pub fn new(window: &Window, machine: MachineHandle) -> Self {
        let title = adw::WindowTitle::new(&machine.borrow().name, "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        header.add_css_class("screen-header");
        let fullscreen_button = adw::SplitButton::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text(format!("Fullscreen ({FULLSCREEN_KEYS})"))
            .build();
        header.pack_end(&fullscreen_button);
        // A button that took the focus from the screen would end the capture it starts.
        let capture_button =
            gtk::Button::builder().icon_name("input-mouse-symbolic").focus_on_click(false).build();
        header.pack_end(&capture_button);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        let page = adw::NavigationPage::builder()
            .title(&machine.borrow().name)
            .tag("screen")
            .child(&toolbar)
            .build();
        let this = Self(Rc::new(Parts {
            window: window.clone(),
            machine,
            page,
            title,
            toolbar,
            fullscreen_button,
            capture_button,
            screen: RefCell::default(),
            clipboard: RefCell::default(),
            monitors: RefCell::default(),
            fullscreen_handler: RefCell::default(),
            thumbnail_source: RefCell::default(),
        }));
        this.make_screen();
        *this.0.monitors.borrow_mut() = Some(Monitors::new(window, &this.0.fullscreen_button));
        this.connect();
        this
    }

    fn run_folder(&self) -> std::path::PathBuf {
        let context = self.0.window.context();
        let slot = slot_of(&context, &self.0.machine.borrow());
        context.run_folder(slot)
    }

    fn make_screen(&self) {
        let weak = Rc::downgrade(&self.0);
        let on_capture = move || {
            if let Some(parts) = weak.upgrade() {
                ScreenPage(parts).on_capture_changed();
            }
        };
        let (gpu_frames, overlay) = {
            let machine = self.0.machine.borrow();
            (machine.has_gpu(), machine.overlay)
        };
        let screen = Screen::new(self.run_folder(), gpu_frames, overlay, on_capture);
        self.0.toolbar.set_content(Some(screen.widget()));
        *self.0.screen.borrow_mut() = Some(screen);
    }

    /// A clone, so nothing stays borrowed while the screen calls back here.
    fn screen(&self) -> Option<Screen> {
        self.0.screen.borrow().clone()
    }

    fn connect(&self) {
        let parts = &self.0;
        let this = self.clone();
        parts.fullscreen_button.connect_clicked(move |_| this.toggle_fullscreen());
        let this = self.clone();
        parts.capture_button.connect_clicked(move |_| this.toggle_capture());
        let this = self.clone();
        let handler = parts.window.widget().connect_fullscreened_notify(move |_| this.on_fullscreened());
        *parts.fullscreen_handler.borrow_mut() = Some(handler);
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |_, _, y| this.on_motion(y));
        parts.toolbar.add_controller(motion);
        let this = self.clone();
        parts.page.connect_shown(move |_| this.on_shown());
        let this = self.clone();
        parts.page.connect_hiding(move |_| this.on_hiding());
        let this = self.clone();
        parts.page.connect_hidden(move |_| this.on_hidden());
    }

    pub fn widget(&self) -> &adw::NavigationPage {
        &self.0.page
    }

    /// How many frames have been shown, for the page checks.
    pub fn frames_shown(&self) -> u64 {
        self.screen().map_or(0, |screen| screen.frames_shown())
    }

    /// Whether the guest is taking input, for the page checks.
    pub fn has_input(&self) -> bool {
        self.screen().is_some_and(|screen| screen.has_input())
    }

    pub fn machine_id(&self) -> String {
        self.0.machine.borrow().id.clone()
    }

    pub fn toggle_fullscreen(&self) {
        let window = self.0.window.widget();
        if window.is_fullscreen() {
            window.unfullscreen();
            return;
        }
        let monitor = self.0.monitors.borrow().as_ref().and_then(|monitors| monitors.chosen());
        match monitor {
            Some(monitor) => window.fullscreen_on_monitor(&monitor),
            None => window.fullscreen(),
        }
        if let Some(screen) = self.screen() {
            screen.focus();
        }
    }

    pub fn toggle_capture(&self) {
        let Some(screen) = self.screen() else { return };
        if screen.capture_asked() {
            screen.stop_capture();
        } else if let Some(problem) = screen.capture_problem() {
            self.0.window.toast(problem, adw::ToastPriority::Normal);
        } else {
            screen.start_capture();
        }
    }

    pub fn toggle_overlay(&self) {
        let wanted = {
            let mut machine = self.0.machine.borrow_mut();
            machine.overlay = !machine.overlay;
            machine.overlay
        };
        if let Some(screen) = self.screen() {
            screen.set_overlay(wanted);
        }
        self.0.window.save_machine(&self.0.machine.borrow());
    }

    fn on_capture_changed(&self) {
        if self.screen().is_some_and(|screen| screen.captured()) {
            // In fullscreen this is all the user sees of the way out, so it waits for no other toast.
            self.0
                .window
                .toast(&format!("Mouse captured. {RELEASE_KEYS} releases it."), adw::ToastPriority::High);
            if self.0.window.widget().is_fullscreen() {
                self.0.toolbar.set_reveal_top_bars(false);
            }
        }
        self.show_hint(None);
    }

    /// The way out: takes nothing from the guest, so it works whatever state that is in.
    pub fn release(&self) {
        self.0.window.widget().unfullscreen();
        if let Some(screen) = self.screen() {
            screen.release();
        }
        self.show_hint(None);
    }

    fn on_fullscreened(&self) {
        let parts = &self.0;
        let fullscreen = parts.window.widget().is_fullscreen();
        parts.toolbar.set_extend_content_to_top_edge(fullscreen);
        parts.toolbar.set_reveal_top_bars(!fullscreen);
        let button = &parts.fullscreen_button;
        button.set_icon_name(if fullscreen { "view-restore-symbolic" } else { "view-fullscreen-symbolic" });
        button.set_tooltip_text(Some(&if fullscreen {
            format!("Leave Fullscreen ({RELEASE_KEYS})")
        } else {
            format!("Fullscreen ({FULLSCREEN_KEYS})")
        }));
        if fullscreen {
            let text = format!("{RELEASE_KEYS} leaves fullscreen. So does the top edge of the screen.");
            parts.window.toast(&text, adw::ToastPriority::Normal);
        }
    }

    fn on_motion(&self, y: f64) {
        let parts = &self.0;
        if parts.window.widget().is_fullscreen() {
            let height = parts.toolbar.top_bar_height();
            let top_bar = if height > 0 { height } else { USUAL_HEADER_HEIGHT };
            if y <= TOP_EDGE {
                parts.toolbar.set_reveal_top_bars(true);
            } else if y > f64::from(top_bar) {
                parts.toolbar.set_reveal_top_bars(false);
            }
        }
    }

    fn on_shown(&self) {
        self.show_hint(None);
        let machine = self.0.machine.borrow().clone();
        if machine.clipboard {
            let context = self.0.window.context();
            let slot = slot_of(&context, &machine);
            let shared = SharedClipboard::new(self.0.window.widget(), context, machine, slot);
            *self.0.clipboard.borrow_mut() = Some(shared);
        }
        let this = self.clone();
        let source = glib::timeout_add_local(THUMBNAIL_EVERY, move || {
            this.save_thumbnail();
            glib::ControlFlow::Continue
        });
        *self.0.thumbnail_source.borrow_mut() = Some(source);
    }

    fn thumbnail(&self) -> Option<Vec<u8>> {
        self.screen()?.thumbnail(THUMBNAIL_WIDTH)
    }

    /// A thumbnail isn't worth interrupting anyone for, so what goes wrong only goes in the log.
    fn save_thumbnail(&self) {
        let Some(png) = self.thumbnail() else { return };
        let context = self.0.window.context();
        let machine = self.0.machine.borrow().clone();
        let working = context.clone();
        let window = self.0.window.clone();
        let id = machine.id.clone();
        in_background(
            &context,
            move || save_thumbnail(&working, &machine, &png),
            move |result| match result {
                Ok(()) => window.show_thumbnail(&id),
                Err(text) => {
                    let _ = write_log(&window.context(), &format!("thumbnail: {text}"));
                }
            },
        );
    }

    /// For the app closing, when a thread might not get to finish.
    pub fn save_thumbnail_now(&self) {
        if let Some(png) = self.thumbnail() {
            let context = self.0.window.context();
            if let Err(failure) = save_thumbnail(&context, &self.0.machine.borrow(), &png) {
                let _ = write_log(&context, &format!("thumbnail: {failure}"));
            }
        }
    }

    /// The frame is still there now, and it's gone once the page is hidden.
    fn on_hiding(&self) {
        if let Some(source) = self.0.thumbnail_source.borrow_mut().take() {
            source.remove();
        }
        self.save_thumbnail();
    }

    fn on_hidden(&self) {
        let parts = &self.0;
        parts.clipboard.borrow_mut().take();
        parts.window.widget().unfullscreen();
        if let Some(handler) = parts.fullscreen_handler.borrow_mut().take() {
            parts.window.widget().disconnect(handler);
        }
        if let Some(monitors) = parts.monitors.borrow_mut().take() {
            monitors.close();
        }
        parts.screen.borrow_mut().take();
        parts.window.screen_closed();
    }

    /// What is going on, under the machine's name.
    pub fn show_hint(&self, status: Option<&Status>) {
        let Some(screen) = self.screen() else { return };
        if status.is_some() {
            screen.follow_machine(self.run_folder(), self.0.machine.borrow().has_gpu());
        }
        let hint = match status {
            Some(status) if status.state == State::Installing => status.step.clone(),
            _ if !screen.has_frames() => "Waiting for the screen".to_owned(),
            _ if !screen.has_input() => "Another viewer has the keyboard and mouse".to_owned(),
            _ if screen.captured() => format!("Mouse captured. {RELEASE_KEYS} releases it."),
            _ if screen.capture_asked() => "Move the pointer into the window to capture it".to_owned(),
            _ if screen.shortcuts_inhibited() => {
                format!("Keys go to the guest. {RELEASE_KEYS} releases them.")
            }
            _ => "Click the screen to send it keys".to_owned(),
        };
        if hint != self.0.title.subtitle() {
            self.0.title.set_subtitle(&hint);
        }
        let problem = screen.capture_problem();
        self.0.capture_button.set_sensitive(problem.is_none());
        let tooltip = format!("Capture Mouse ({CAPTURE_KEYS})");
        self.0.capture_button.set_tooltip_text(Some(problem.unwrap_or(&tooltip)));
    }
}
