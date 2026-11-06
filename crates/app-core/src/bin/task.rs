//! `guesthouse-task ACTION MACHINE [ARGUMENT...]`: one slow job for the app, in the slot named by
//! `GUESTHOUSE_SLOT`. It outlives the app, and writes each step to a small file the app reads.

use guesthouse_app_core::actions::run_task;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::vm_host::LocalHost;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let [action, machine, rest @ ..] = &arguments[..] else {
        eprintln!("usage: guesthouse-task ACTION MACHINE [ARGUMENT...]");
        return ExitCode::from(2);
    };
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    run_task(&context, &LocalHost { context: &context }, action, machine, rest);
    ExitCode::SUCCESS
}
