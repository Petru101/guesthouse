//! The app's log, which holds the whole story of a failure.

use crate::context::Context;
use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};

/// How much of the end of the log is shown, as it can grow for months.
pub const MOST_LOG_BYTES: u64 = 1 << 18;

/// The time at the start of each line.
pub fn clock_text() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

pub fn open_for_appending(context: &Context) -> io::Result<File> {
    let path = context.paths.log_file();
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

pub fn write_log(context: &Context, text: &str) -> io::Result<()> {
    writeln!(open_for_appending(context)?, "{} {}", clock_text(), text.trim_end())
}

/// The end of the log.
pub fn read_log(context: &Context) -> String {
    let Ok(mut file) = File::open(context.paths.log_file()) else { return String::new() };
    let size = file.metadata().map_or(0, |found| found.len());
    let mut data = Vec::new();
    if file.seek(SeekFrom::Start(size.saturating_sub(MOST_LOG_BYTES))).is_err()
        || file.read_to_end(&mut data).is_err()
    {
        return String::new();
    }
    String::from_utf8_lossy(&data).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn only_the_end_of_a_long_log_is_read() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(read_log(&context), "");
        write_log(&context, "one thing\n").unwrap();
        assert!(read_log(&context).ends_with("one thing\n"));
        let filler = "x".repeat(MOST_LOG_BYTES as usize * 2);
        write!(open_for_appending(&context).unwrap(), "{filler}").unwrap();
        write_log(&context, "last thing").unwrap();
        let text = read_log(&context);
        assert!(text.len() <= MOST_LOG_BYTES as usize && text.ends_with("last thing\n"));
    }
}
