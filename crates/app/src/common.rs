//! What the windows share: threads that can't fail unseen, and choosing a file.

use adw::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::Result;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::write_log;
use std::path::PathBuf;
use std::sync::Arc;

pub const WENT_WRONG: &str = "Something went wrong. The log has more.";

/// Runs work on a thread, and calls `done` on GTK's thread with what came of it, as text if it
/// failed. A panic counts too, so no thread can die unseen.
pub fn in_background<T: Send + 'static>(
    context: &Arc<Context>,
    work: impl FnOnce() -> Result<T> + Send + 'static,
    done: impl FnOnce(std::result::Result<T, String>) + 'static,
) {
    let context = context.clone();
    glib::spawn_future_local(async move {
        let outcome = match gio::spawn_blocking(work).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(failure)) => Err(failure.0),
            Err(_) => {
                // The panic's own message went to the task's output, and this leaves a mark in the log.
                let _ = write_log(&context, "app: a background thread panicked");
                Err(WENT_WRONG.to_owned())
            }
        };
        done(outcome);
    });
}

/// What a file dialog gave, or `None`. The user changing their mind isn't a failure.
pub fn chosen_path(
    result: std::result::Result<gio::File, glib::Error>,
    failed: &impl Fn(String),
) -> Option<PathBuf> {
    let file = match result {
        Ok(file) => file,
        Err(error) => {
            let gave_up =
                error.matches(gtk::DialogError::Dismissed) || error.matches(gtk::DialogError::Cancelled);
            if !gave_up {
                failed(format!("The file dialog failed: {}", error.message()));
            }
            return None;
        }
    };
    let path = file.path();
    if path.is_none() {
        failed("Choose something on this PC. Guesthouse cannot read that place.".to_owned());
    }
    path
}

/// Calls `chosen` with a path, if the user picks one.
pub fn choose_file(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    filter_name: &str,
    suffix: &str,
    chosen: impl Fn(String) + 'static,
    failed: impl Fn(String) + 'static,
) {
    let wanted = gtk::FileFilter::new();
    wanted.set_name(Some(filter_name));
    wanted.add_suffix(suffix);
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&wanted);
    let dialog = gtk::FileDialog::builder().title(title).filters(&filters).default_filter(&wanted).build();
    dialog.open(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Some(path) = chosen_path(result, &failed) {
            chosen(path.to_string_lossy().into_owned());
        }
    });
}

/// As [`choose_file`], for a folder.
pub fn choose_folder(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    chosen: impl Fn(String) + 'static,
    failed: impl Fn(String) + 'static,
) {
    let dialog = gtk::FileDialog::builder().title(title).build();
    dialog.select_folder(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Some(path) = chosen_path(result, &failed) {
            chosen(path.to_string_lossy().into_owned());
        }
    });
}
