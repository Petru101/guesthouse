//! First-time setup, a page at a time. All the work is in app-core, and runs on threads.

mod check_page;
mod done_page;
mod driver_page;

use crate::window::Window;
use adw::prelude::*;
use std::rc::Rc;

/// The last page isn't a step.
const STEPS: u32 = 2;
const SIZE: i32 = 640;

/// What every step has: a title with its number, a Next button, and rows to fill.
pub struct Step {
    page: adw::NavigationPage,
    next: gtk::Button,
    content: adw::PreferencesPage,
}

impl Step {
    fn new(title: &str, tag: &str, number: u32) -> Self {
        let next = gtk::Button::builder().label("Next").sensitive(false).build();
        next.add_css_class("suggested-action");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(title, &format!("Step {number} of {STEPS}"))));
        header.pack_end(&next);
        let content = adw::PreferencesPage::new();
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&content));
        toolbar.add_top_bar(&header);
        let page = adw::NavigationPage::builder().title(title).tag(tag).child(&toolbar).build();
        Self { page, next, content }
    }
}

pub struct Wizard {
    window: Window,
    dialog: adw::Dialog,
    pages: adw::NavigationView,
}

pub fn show(window: &Window) {
    let pages = adw::NavigationView::new();
    let dialog = adw::Dialog::builder()
        .title("Set Up Guesthouse")
        .content_width(SIZE)
        .content_height(SIZE)
        .child(&pages)
        .build();
    let wizard = Rc::new(Wizard { window: window.clone(), dialog, pages });
    let driver = driver_page::DriverPage::new(&wizard);
    let check = check_page::CheckPage::new(&wizard, &driver);
    wizard.pages.add(check.page());
    let stopping = driver.clone();
    wizard.dialog.connect_closed(move |_| stopping.stop());
    wizard.dialog.present(Some(window.widget()));
}
