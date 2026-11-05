//! Log lines to standard error, which the script that starts the daemon sends to a file.

use log::{Level, Log, Metadata, Record};

struct StandardError;

impl Log for StandardError {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{:<5} {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

/// Starts logging. Debug lines cost time on the message path, so they stay off unless asked for.
pub fn start(verbose: bool) {
    let level = if verbose { Level::Debug } else { Level::Info };
    if log::set_logger(&StandardError).is_ok() {
        log::set_max_level(level.to_level_filter());
    }
}
