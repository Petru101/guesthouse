//! Small widgets the pages have in common.

use adw::prelude::*;
use guesthouse_app_core::setup::checks::CheckState;

/// A row saying how something went, and what to do about it. Commands in it can be copied.
pub fn state_row(title: &str, state: CheckState, text: &str, fix: &str) -> adw::ActionRow {
    let lines: Vec<&str> = [text, fix].into_iter().filter(|line| !line.is_empty()).collect();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(lines.join("\n"))
        .use_markup(false)
        .subtitle_selectable(true)
        .build();
    let (name, style) = match state {
        CheckState::Ok => ("object-select-symbolic", "success"),
        CheckState::Warning => ("dialog-warning-symbolic", "warning"),
        CheckState::Failed => ("dialog-error-symbolic", "error"),
    };
    let icon = gtk::Image::from_icon_name(name);
    icon.add_css_class(style);
    row.add_prefix(&icon);
    row
}

/// A button that sits in a row or a group's header.
pub fn row_button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).valign(gtk::Align::Center).build()
}

/// Gives a widget with no text of its own a name for screen readers.
pub fn name_for_readers(widget: &impl IsA<gtk::Widget>, name: &str) {
    widget.upcast_ref::<gtk::Widget>().update_property(&[gtk::accessible::Property::Label(name)]);
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button =
        gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).build();
    button.add_css_class("flat");
    name_for_readers(&button, tooltip);
    button
}

pub fn number_row(title: &str, value: f64, lowest: f64, highest: f64) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(lowest, highest, 1.0);
    row.set_title(title);
    row.set_value(value);
    // The row's field has the focus, so that's what a screen reader lands on.
    name_for_readers(&row, title);
    if let Ok(field) = row.delegate().and_dynamic_cast::<gtk::Widget>() {
        name_for_readers(&field, title);
    }
    row
}

pub fn choice_row(title: &str, names: &[&str]) -> adw::ComboRow {
    let row = adw::ComboRow::builder().title(title).build();
    row.set_model(Some(&gtk::StringList::new(names)));
    row
}

/// Rows to put inside a dialog.
pub fn boxed_rows(rows: &[&gtk::Widget]) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    for row in rows {
        list.append(*row);
    }
    list
}

/// A question with a Cancel button, which calls `answered` only for a yes.
pub fn question(
    heading: &str,
    body: &str,
    yes: &str,
    appearance: adw::ResponseAppearance,
) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("yes", yes);
    dialog.set_response_appearance("yes", appearance);
    dialog.set_close_response("cancel");
    dialog
}

pub fn on_yes(dialog: &adw::AlertDialog, answered: impl Fn() + 'static) {
    dialog.connect_response(None, move |_, response| {
        if response == "yes" {
            answered();
        }
    });
}

/// Asks before something that can't be undone.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    yes: &str,
    answered: impl Fn() + 'static,
) {
    let dialog = question(heading, body, yes, adw::ResponseAppearance::Destructive);
    on_yes(&dialog, answered);
    dialog.present(Some(parent));
}
