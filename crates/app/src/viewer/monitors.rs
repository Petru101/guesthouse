//! Which monitor fullscreen goes to, from the menu beside its button.

use crate::window::Window;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use guesthouse_app_core::settings::read_prefs;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;

const ACTION: &str = "monitor";

pub struct Monitors {
    window: Window,
    button: adw::SplitButton,
    chosen: Rc<RefCell<String>>,
    handler: RefCell<Option<glib::SignalHandlerId>>,
}

fn all_monitors(display: &gdk::Display) -> Vec<gdk::Monitor> {
    display.monitors().iter::<gdk::Monitor>().flatten().collect()
}

impl Monitors {
    pub fn new(window: &Window, button: &adw::SplitButton) -> Rc<Self> {
        let saved = read_prefs(&window.context()).get("monitor").and_then(Value::as_str).map(str::to_owned);
        let chosen = Rc::new(RefCell::new(saved.unwrap_or_default()));
        let this = Rc::new(Self {
            window: window.clone(),
            button: button.clone(),
            chosen: chosen.clone(),
            handler: RefCell::default(),
        });
        let action = gio::SimpleAction::new_stateful(
            ACTION,
            Some(glib::VariantTy::STRING),
            &chosen.borrow().to_variant(),
        );
        let remembering = window.clone();
        action.connect_change_state(move |action, value| {
            let Some(value) = value else { return };
            action.set_state(value);
            let name = value.get::<String>().unwrap_or_default();
            remembering.remember(json!({"monitor": name}));
            *chosen.borrow_mut() = name;
        });
        window.widget().add_action(&action);
        let listing = Rc::downgrade(&this);
        let display = WidgetExt::display(window.widget());
        let handler = display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(this) = listing.upgrade() {
                this.list();
            }
        });
        *this.handler.borrow_mut() = Some(handler);
        this.list();
        this
    }

    fn list(&self) {
        let menu = gio::Menu::new();
        menu.append(Some("Monitor the Window Is On"), Some("win.monitor::"));
        for monitor in all_monitors(&WidgetExt::display(self.window.widget())) {
            let area = monitor.geometry();
            let connector = monitor.connector().map(|name| name.to_string()).unwrap_or_default();
            let name = monitor.description().map(|name| name.to_string());
            let name = name.unwrap_or_else(|| {
                if connector.is_empty() { "Monitor".to_owned() } else { connector.clone() }
            });
            let item =
                gio::MenuItem::new(Some(&format!("{name} ({}x{})", area.width(), area.height())), None);
            item.set_action_and_target_value(Some("win.monitor"), Some(&connector.to_variant()));
            menu.append_item(&item);
        }
        self.button.set_menu_model(Some(&menu));
    }

    /// The monitor picked in the menu, if it's still plugged in.
    pub fn chosen(&self) -> Option<gdk::Monitor> {
        let chosen = self.chosen.borrow();
        let display = WidgetExt::display(self.window.widget());
        all_monitors(&display)
            .into_iter()
            .find(|monitor| monitor.connector().as_deref() == Some(chosen.as_str()))
    }

    pub fn close(&self) {
        if let Some(handler) = self.handler.borrow_mut().take() {
            WidgetExt::display(self.window.widget()).monitors().disconnect(handler);
        }
        self.window.widget().remove_action(ACTION);
    }
}
