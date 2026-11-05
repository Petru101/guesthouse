//! `guesthouse-replay RECORDING`: replays a recording and says whether every reply matched.

use guesthouse_replay::recording::Recording;
use guesthouse_replay::replay::replay;
use std::process::ExitCode;
use std::{env, fs};

const EXIT_DIFFERENT: u8 = 1;
const EXIT_UNUSABLE: u8 = 2;

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: guesthouse-replay RECORDING");
        return ExitCode::from(EXIT_UNUSABLE);
    };
    let outcome = fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| {
        let recording = Recording::parse(&bytes).map_err(|error| error.to_string())?;
        replay(&recording).map_err(|error| error.to_string())
    });
    match outcome {
        Ok(report) => {
            print!("{report}");
            if report.passed() { ExitCode::SUCCESS } else { ExitCode::from(EXIT_DIFFERENT) }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}
