//! Newer releases: a banner on the library page when one is out, a menu item to look now, and the
//! update itself, which ends with the app starting again as the new version.

use super::Window;
use crate::common::in_background;
use crate::rows::{on_yes, question};
use adw::prelude::*;
use guesthouse_app_core::updates::{Found, VERSION, check, update};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// The app's program, in the install's `current` folder, which is the new version once it's updated.
const APP_PROGRAM: &str = "guesthouse";
/// Waits for this process to be gone, so the new one isn't handed to it as a second window, then
/// starts the new one. `$0` is this process and `$1` the program.
const WAIT_THEN_START: &str = r#"while kill -0 "$0" 2>/dev/null; do sleep 0.2; done; exec "$1""#;
const PROGRESS_WIDTH: i32 = 360;
const PROGRESS_MARGIN: i32 = 24;
const PROGRESS_SPACING: i32 = 12;
const SHUT_DOWN_FIRST: &str = "Shut your VMs down first. Guesthouse can't update while one is running.";

/// Asked once as the app opens. It's the only thing the app does on the network by itself, and it
/// only speaks up if there's something new.
pub fn check_at_start(window: &Window) {
    look(window, false);
}

/// From the menu, so it says what it found either way.
pub fn check_now(window: &Window) {
    look(window, true);
}

fn look(window: &Window, asked: bool) {
    let context = window.context();
    let asking = context.clone();
    let window = window.clone();
    in_background(
        &context,
        move || Ok(check(&asking)),
        move |found| {
            let text = match found {
                Ok(Found::Newer(version)) => {
                    window.0.library.offer_update(&version);
                    *window.0.newer.borrow_mut() = Some(version);
                    return;
                }
                Ok(Found::Latest) => format!("Guesthouse {VERSION} is the latest release."),
                Ok(Found::NotInstalled) => {
                    "This copy wasn't installed with install.sh, so it can't update itself.".to_owned()
                }
                Ok(Found::CantTell) | Err(_) => {
                    "Couldn't find out the latest release. Are you online?".to_owned()
                }
            };
            if asked {
                window.toast(&text, adw::ToastPriority::Normal);
            }
        },
    );
}

/// The banner's button.
pub fn ask_to_update(window: &Window) {
    let Some(version) = window.0.newer.borrow().clone() else { return };
    if window.vm_running() {
        window.problem(SHUT_DOWN_FIRST);
        return;
    }
    let body = format!(
        "This downloads Guesthouse {version}, checks it and puts it in place of {VERSION}. Your machines and \
         settings stay as they are. If a part needs root, a window asks for your password. Then Guesthouse \
         starts again."
    );
    let dialog =
        question(&format!("Update to {version}?"), &body, "Update", adw::ResponseAppearance::Suggested);
    let handle = window.clone();
    on_yes(&dialog, move || run_update(&handle, &version));
    dialog.present(Some(window.widget()));
}

fn progress(version: &str) -> adw::Dialog {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(PROGRESS_SPACING)
        .margin_top(PROGRESS_MARGIN)
        .margin_bottom(PROGRESS_MARGIN)
        .margin_start(PROGRESS_MARGIN)
        .margin_end(PROGRESS_MARGIN)
        .build();
    let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
    column.append(&spinner);
    let heading = gtk::Label::new(Some(&format!("Updating to {version}")));
    heading.add_css_class("title-2");
    column.append(&heading);
    let note = gtk::Label::builder()
        .label("This takes a minute or two. Keep Guesthouse open.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    column.append(&note);
    adw::Dialog::builder().child(&column).content_width(PROGRESS_WIDTH).can_close(false).build()
}

fn run_update(window: &Window, version: &str) {
    if window.vm_running() {
        window.problem(SHUT_DOWN_FIRST);
        return;
    }
    let shown = progress(version);
    shown.present(Some(window.widget()));
    let context = window.context();
    let updating = context.clone();
    let window = window.clone();
    in_background(
        &context,
        move || update(&updating, std::process::id()),
        move |result| {
            shown.force_close();
            match result {
                Ok(()) => start_again(&window),
                Err(failure) => window.problem(&failure),
            }
        },
    );
}

/// The files of the version that's running may be gone now, so the new version takes over at once.
fn start_again(window: &Window) {
    let program = window.context().paths.helper(APP_PROGRAM);
    let started = Command::new("sh")
        .arg("-c")
        .arg(WAIT_THEN_START)
        .arg(std::process::id().to_string())
        .arg(&program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    if started.is_err() {
        window
            .problem("Updated, but Guesthouse couldn't start again by itself. Open it again from your apps.");
        return;
    }
    window.widget().close();
    if let Some(application) = window.widget().application() {
        application.quit();
    }
}
