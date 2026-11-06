//! The window's actions, and the About and shortcuts dialogs they lead to.

use super::Window;
use super::machine_row::MachineRow;
use crate::application::APP_ID;
use adw::prelude::*;
use gtk::gio;

/// Keys the guest must get while its screen is up, so these actions are off then.
const HOST_ACCELS: [(&str, &str); 4] = [
    ("new-machine", "<Control>n"),
    ("settings", "<Control>comma"),
    ("quit", "<Control>q"),
    ("shortcuts", "<Control>question"),
];
const SHORTCUTS_WIDTH: i32 = 480;
const SHORTCUTS_HEIGHT: i32 = 520;
const SCREEN_SHORTCUTS: [(&str, &str); 4] = [
    ("Give Back the Keyboard and Leave Fullscreen", "<Control><Alt>g"),
    ("Fullscreen", "<Control><Alt>Return"),
    ("Capture the Mouse", "<Control><Alt>m"),
    ("Performance Overlay", "<Control><Alt>p"),
];

fn accel_of(name: &str) -> &'static str {
    HOST_ACCELS.iter().find(|(action, _)| *action == name).map_or("", |found| found.1)
}

fn add(window: &Window, name: &str, run: impl Fn(&Window) + 'static) {
    let action = gio::SimpleAction::new(name, None);
    let handle = window.clone();
    action.connect_activate(move |_, _| run(&handle));
    window.widget().add_action(&action);
}

pub fn add_actions(window: &Window) {
    add(window, "new-machine", on_new_machine_key);
    add(window, "settings", on_settings_key);
    add(window, "quit", |window| window.widget().close());
    add(window, "shortcuts", show_shortcuts);
    add(window, "setup", Window::show_setup);
    add(window, "log", Window::show_log);
    add(window, "about", show_about);
    if let Some(application) = window.widget().application() {
        for (name, keys) in HOST_ACCELS {
            application.set_accels_for_action(&format!("win.{name}"), &[keys]);
        }
    }
}

/// A disabled action leaves its key alone, so it goes on to whatever has the focus.
pub fn allow_host_keys(window: &Window, allowed: bool) {
    for (name, _) in HOST_ACCELS {
        let action = window.widget().lookup_action(name);
        if let Some(Ok(action)) = action.map(|found| found.downcast::<gio::SimpleAction>()) {
            action.set_enabled(allowed);
        }
    }
}

fn has_dialog(window: &Window) -> bool {
    window.widget().visible_dialog().is_some()
}

fn on_new_machine_key(window: &Window) {
    if !has_dialog(window) {
        window.show_new_machine();
    }
}

/// The row with the focus, which a click gives it too. With one machine there's no doubt.
fn current_row(window: &Window) -> Option<MachineRow> {
    let rows = window.0.rows.borrow();
    let mut widget = gtk::prelude::GtkWindowExt::focus(window.widget());
    while let Some(found) = widget {
        if let Some(row) = rows.values().find(|row| row.widget().upcast_ref::<gtk::Widget>() == &found) {
            return Some(row.clone());
        }
        widget = found.parent();
    }
    (rows.len() == 1).then(|| rows.values().next().cloned()).flatten()
}

fn on_settings_key(window: &Window) {
    if has_dialog(window) || window.0.rows.borrow().is_empty() {
        return;
    }
    match current_row(window) {
        Some(row) => window.show_settings(row.machine()),
        None => window.toast("Click a machine first.", adw::ToastPriority::Normal),
    }
}

fn show_shortcuts(window: &Window) {
    if has_dialog(window) {
        return;
    }
    let library = adw::PreferencesGroup::builder().title("Library").build();
    let items = [
        ("New Machine", "new-machine"),
        ("Settings of the Selected Machine", "settings"),
        ("Keyboard Shortcuts", "shortcuts"),
        ("Quit", "quit"),
    ];
    for (title, action) in items {
        library.add(&shortcut_row(title, accel_of(action)));
    }
    let screen = adw::PreferencesGroup::builder().title("Guest Screen").build();
    for (title, keys) in SCREEN_SHORTCUTS {
        screen.add(&shortcut_row(title, keys));
    }
    let page = adw::PreferencesPage::new();
    page.add(&library);
    page.add(&screen);
    let content = adw::ToolbarView::new();
    content.add_top_bar(&adw::HeaderBar::new());
    content.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(SHORTCUTS_WIDTH)
        .content_height(SHORTCUTS_HEIGHT)
        .child(&content)
        .build();
    dialog.present(Some(window.widget()));
}

fn shortcut_row(title: &str, keys: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&gtk::ShortcutLabel::new(keys));
    row
}

fn show_about(window: &Window) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Guesthouse")
        .application_icon(APP_ID)
        .comments("Runs VMs on this PC. With its GPU part, Windows gets NVIDIA's own driver on the card the host keeps using.")
        .license_type(gtk::License::Gpl20)
        .build();
    dialog.present(Some(window.widget()));
}
