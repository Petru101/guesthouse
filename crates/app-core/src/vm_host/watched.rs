//! Running a helper while keeping an eye on the VM.

use super::HelperOutput;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::processes::qemu_pid;
use crate::vm_args::OUTPUT_FILE;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(super) const SCRIPT_POLL: Duration = Duration::from_millis(500);
pub(super) const LAST_WORDS_BYTES: usize = 300;

pub fn last_words(context: &Context) -> String {
    let path = context.run_folder(None).join(OUTPUT_FILE);
    let text = fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let text = text.trim();
    let start = text.char_indices().rev().nth(LAST_WORDS_BYTES - 1).map_or(0, |found| found.0);
    text[start..].to_owned()
}

/// Runs a program and returns its output, stopping early if the VM goes away under it. Everything it
/// says reaches the log, and is read as it comes, as a full pipe would stop it.
pub fn run_watched(
    context: &Context,
    program: &Path,
    arguments: &[String],
    environment: Vec<(String, String)>,
) -> Result<HelperOutput> {
    let mut child = Command::new(program)
        .args(arguments)
        .envs(environment)
        .current_dir(&context.paths.program)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let lines = Arc::new(Mutex::new(String::new()));
    let readers: Vec<_> = [
        child.stdout.take().map(|stream| keep_lines(stream, lines.clone())),
        child.stderr.take().map(|stream| keep_lines(stream, lines.clone())),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut seen = qemu_pid(context, None).is_some();
    let code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code().unwrap_or(-1);
        }
        let alive = qemu_pid(context, None).is_some();
        if seen && !alive {
            let _ = child.kill();
            let _ = child.wait();
            let words = last_words(context);
            return Err(Failure(format!("The VM stopped unexpectedly. {words}").trim().to_owned()));
        }
        seen = seen || alive;
        context.clock.sleep(SCRIPT_POLL);
    };
    // Something the program left behind may hold the pipe open, and then the readers never end.
    for reader in readers {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
    let text = lines.lock().map(|text| text.clone()).unwrap_or_default();
    Ok(HelperOutput { code, text })
}

fn keep_lines(
    stream: impl std::io::Read + Send + 'static,
    lines: Arc<Mutex<String>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        while matches!(reader.read_until(b'\n', &mut line), Ok(size) if size > 0) {
            let text = String::from_utf8_lossy(&line);
            print!("{text}");
            if let Ok(mut lines) = lines.lock() {
                lines.push_str(&text);
            }
            line.clear();
        }
    })
}
