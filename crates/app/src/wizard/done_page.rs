//! The end of setup, with the way to the first machine.

use super::Wizard;
use crate::application::APP_ID;
use adw::prelude::*;
use std::rc::Rc;

pub fn done_page(wizard: &Rc<Wizard>) -> adw::NavigationPage {
    let button = gtk::Button::builder().label("New Machine").halign(gtk::Align::Center).build();
    button.add_css_class("pill");
    button.add_css_class("suggested-action");
    let closing = wizard.clone();
    button.connect_clicked(move |_| {
        closing.dialog.close();
        closing.window.show_new_machine();
    });
    let status = adw::StatusPage::builder()
        .icon_name(APP_ID)
        .title("Guesthouse Is Ready")
        .description("All that is left is a Windows 11 ISO.")
        .child(&button)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.set_content(Some(&status));
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    adw::NavigationPage::builder().title("Done").tag("done").child(&toolbar).build()
}
