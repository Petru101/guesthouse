//! `guesthouse-check`: what this PC can run, for `install.sh`. It's a program of its own because it has
//! to run from a download, before GTK may be on this PC. It changes nothing, and opens no device.
//!
//! Each line is `check`, the state, the title, what was found and the fix. A `gtk` line has the version
//! of GTK, if there is one, so the script can choose the app that runs on it. The last is `gpu` and
//! `yes` or `no`, all split by tabs. The script prints these words and never runs them.

use guesthouse_app_core::context::Context;
use guesthouse_app_core::setup::checks::{Check, CheckState, check_host};
use guesthouse_app_core::setup::toolkit::gtk_version;
use std::process::ExitCode;

fn state_word(state: CheckState) -> &'static str {
    match state {
        CheckState::Ok => "ok",
        CheckState::Warning => "warning",
        CheckState::Failed => "failed",
    }
}

/// A tab or a line break in a message would split it into fields of its own.
fn field(text: &str) -> String {
    text.replace(['\t', '\n'], " ")
}

fn line(check: &Check) -> String {
    let state = state_word(check.state);
    format!("check\t{state}\t{}\t{}\t{}", field(&check.title), field(&check.text), field(&check.fix))
}

fn main() -> ExitCode {
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let report = check_host(&context);
    for check in &report.checks {
        println!("{}", line(check));
    }
    if let Some(version) = gtk_version() {
        println!("gtk\t{version}");
    }
    println!("gpu\t{}", if report.gpu { "yes" } else { "no" });
    ExitCode::SUCCESS
}
