//! The first page: the machines, or a way to make the first one.

use super::Window;
use crate::application::APP_ID;
use crate::rows::name_for_readers;
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::updates::VERSION;

const COLUMN_SPACING: i32 = 12;
const COLUMN_MARGIN: i32 = 24;
const SIDE_MARGIN: i32 = 12;
const COLUMN_WIDTH: i32 = 800;

pub struct LibraryPage {
    pub page: adw::NavigationPage,
    list: gtk::ListBox,
    problems: gtk::Label,
    stack: gtk::Stack,
    add: gtk::Button,
    start: gtk::Button,
    update: adw::Banner,
}

fn main_menu() -> gio::Menu {
    let setup_items = gio::Menu::new();
    setup_items.append(Some("Run Setup Again"), Some("win.setup"));
    setup_items.append(Some("Show Log"), Some("win.log"));
    setup_items.append(Some("Check for Updates"), Some("win.check-updates"));
    let about_items = gio::Menu::new();
    about_items.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    about_items.append(Some("About Guesthouse"), Some("win.about"));
    let menu = gio::Menu::new();
    menu.append_section(None, &setup_items);
    menu.append_section(None, &about_items);
    menu
}

impl LibraryPage {
    pub fn new(description: &str) -> Self {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .build();
        list.add_css_class("boxed-list");
        let problems = gtk::Label::builder().wrap(true).xalign(0.0).visible(false).build();
        problems.add_css_class("error");
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(COLUMN_SPACING)
            .margin_top(COLUMN_MARGIN)
            .margin_bottom(COLUMN_MARGIN)
            .margin_start(SIDE_MARGIN)
            .margin_end(SIDE_MARGIN)
            .build();
        column.append(&list);
        column.append(&problems);
        let clamp = adw::Clamp::builder().child(&column).maximum_size(COLUMN_WIDTH).build();
        let scroller = gtk::ScrolledWindow::builder().child(&clamp).build();
        let start = gtk::Button::builder().label("New Machine").halign(gtk::Align::Center).build();
        start.add_css_class("pill");
        start.add_css_class("suggested-action");
        let empty = adw::StatusPage::builder()
            .icon_name(APP_ID)
            .title("No Machines")
            .description(description)
            .child(&start)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&scroller, Some("machines"));
        stack.add_named(&empty, Some("empty"));
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("New Machine").build();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("Guesthouse", VERSION)));
        name_for_readers(&add, "New Machine");
        header.pack_start(&add);
        let menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .primary(true)
            .tooltip_text("Main Menu")
            .menu_model(&main_menu())
            .build();
        name_for_readers(&menu, "Main Menu");
        header.pack_end(&menu);
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&stack));
        toolbar.add_top_bar(&header);
        let update = adw::Banner::builder().button_label("Update").action_name("win.update").build();
        toolbar.add_top_bar(&update);
        let page = adw::NavigationPage::builder().title("Guesthouse").tag("library").child(&toolbar).build();
        Self { page, list, problems, stack, add, start, update }
    }

    pub fn connect_new_machine(&self, window: &Window) {
        for button in [&self.add, &self.start] {
            let window = window.clone();
            button.connect_clicked(move |_| window.show_new_machine());
        }
    }

    pub fn offer_update(&self, version: &str) {
        self.update.set_title(&format!("Guesthouse {version} is out. You have {VERSION}."));
        self.update.set_revealed(true);
    }

    pub fn add(&self, row: &adw::ActionRow) {
        self.list.append(row);
    }

    pub fn remove(&self, row: &adw::ActionRow) {
        self.list.remove(row);
    }

    pub fn show(&self, has_machines: bool, errors: &[String]) {
        self.problems.set_label(&errors.join("\n"));
        self.problems.set_visible(!errors.is_empty());
        let filled = has_machines || !errors.is_empty();
        self.stack.set_visible_child_name(if filled { "machines" } else { "empty" });
    }
}
