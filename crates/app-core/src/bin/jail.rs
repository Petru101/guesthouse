//! `guesthouse-jail PLAN -- PROGRAM ARGUMENTS`: runs a program, QEMU, in the jail the plan describes.
//! The words are the app's to write, in `vm_args`, and `guesthouse_jail::launch` says what they mean.

use guesthouse_jail::launch::Launch;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let error = match Launch::parse(env::args_os().skip(1)) {
        Ok(launch) => launch.run().to_string(),
        Err(error) => error,
    };
    eprintln!("guesthouse-jail: {error}");
    ExitCode::FAILURE
}
