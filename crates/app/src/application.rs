//! The application: one window, which no VM depends on.

use crate::window::Window;
use adw::prelude::*;
use gtk::glib;
use guesthouse_app_core::context::Context;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub const APP_ID: &str = "net.gpupv.Guesthouse";
/// Inside the program folder.
const ICONS: &str = "data/icons";

/// Builds the application around this context, and calls `started` with its window.
pub fn application(context: Arc<Context>, started: impl Fn(&Window) + 'static) -> adw::Application {
    let application = adw::Application::builder().application_id(APP_ID).build();
    let icons = context.paths.helper(ICONS);
    let window: Rc<RefCell<Option<Window>>> = Rc::default();
    application.connect_activate(move |application| {
        let mut slot = window.borrow_mut();
        let window = slot.get_or_insert_with(|| {
            let window = Window::new(application, context.clone());
            window.present();
            window.offer_setup();
            started(&window);
            window
        });
        window.present();
    });
    application.connect_startup(move |_| {
        // So the icon shows before anything has installed it.
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_search_path(&icons);
        }
        gtk::Window::set_default_icon_name(APP_ID);
    });
    application
}

pub fn run() -> glib::ExitCode {
    let context = match Context::from_process() {
        Ok(context) => Arc::new(context),
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::FAILURE;
        }
    };
    application(context, |_| {}).run()
}
