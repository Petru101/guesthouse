//! `guesthouse-share`: QEMU starts this for each connection a guest makes to its shared folder, with
//! the connection on stdin and stdout, and the folder in `SHARE`.

use guesthouse_app_core::share::Share;
use std::env;
use std::io::{self, BufReader};
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(folder) = env::var_os("SHARE") else {
        eprintln!("SHARE must name the folder to share");
        return ExitCode::from(2);
    };
    let reader = BufReader::new(io::stdin().lock());
    match Share::new(folder.as_ref()).and_then(|mut share| share.serve(reader, io::stdout().lock())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
