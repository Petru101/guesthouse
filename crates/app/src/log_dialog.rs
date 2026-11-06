//! Shows the end of the log, which is where the whole story of a failure is.

use crate::common::in_background;
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::log::read_log;

const WIDTH: i32 = 760;
const HEIGHT: i32 = 520;
const MARGIN: i32 = 12;

pub fn show(window: &Window) {
    let text = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .top_margin(MARGIN)
        .bottom_margin(MARGIN)
        .left_margin(MARGIN)
        .right_margin(MARGIN)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let header = adw::HeaderBar::new();
    let file = window.context().paths.log_file();
    header.set_title_widget(Some(&adw::WindowTitle::new("Log", &file.to_string_lossy())));
    let toolbar = adw::ToolbarView::new();
    toolbar.set_content(Some(&gtk::ScrolledWindow::builder().child(&text).build()));
    toolbar.add_top_bar(&header);
    let dialog = adw::Dialog::builder()
        .title("Log")
        .content_width(WIDTH)
        .content_height(HEIGHT)
        .child(&toolbar)
        .build();
    dialog.present(Some(window.widget()));
    let context = window.context();
    let reading = context.clone();
    let window = window.clone();
    in_background(
        &context,
        move || Ok(read_log(&reading)),
        move |result| match result {
            Ok(log) => {
                let buffer = text.buffer();
                buffer.set_text(if log.is_empty() { "The log is empty." } else { &log });
                text.scroll_to_iter(&mut buffer.end_iter(), 0.0, false, 0.0, 1.0);
            }
            Err(problem) => window.problem(&problem),
        },
    );
}
