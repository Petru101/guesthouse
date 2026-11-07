//! `guesthouse-card-check`: one program for anyone with an NVIDIA card, to find out what Guesthouse
//! would make of it. It reads the card through controls that only read, says what the app knows of
//! it, and then, if asked, tries the channels a VM would use. It writes one file to post.
//!
//! It needs no root, no Rust and no Windows driver. It changes nothing on the system: the objects it
//! makes in the driver go when it ends.

use guesthouse_backend_nvidia::profile::{HostBranch, host_branch, host_branch_of_version};
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::linux::LinuxRm;
use guesthouse_card_check::channels::{self, ChannelResult};
use guesthouse_card_check::host::{self, Host, NoHost, no_host_text};
use guesthouse_card_check::report::{self, Branch};
use guesthouse_card_check::survey;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

const REPORT_NAME: &str = "guesthouse-card-check.txt";
const USAGE: &str = "usage: guesthouse-card-check [--card N] [--read-only]\n\
                     \n\
                     Reads an NVIDIA card and writes guesthouse-card-check.txt. With more than one card,\n\
                     --card N picks one by the number the driver gives it, and without it the program asks.\n\
                     --read-only stops before the channel test.";

const CHANNEL_WARNING: &str = "\
The next step makes a graphics channel and a copy channel on this card, and one on each video engine
where the app runs video. Each writes one value to memory, and then it's freed. This is what a VM does
first. On a card nobody has run this on, the GPU could stop answering, which freezes the desktop on that
card until a reboot. Save your work first.";

const NO_DEVICE_FILES: &str = "\
The driver is loaded but its device files are not there. They are made when something first uses the
card, so running nvidia-smi once usually makes them. Inside a container they are not there at all.";

struct Options {
    card: Option<u32>,
    read_only: bool,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options { card: None, read_only: false };
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--card" => {
                let number = arguments.next().ok_or("--card needs a number")?;
                options.card = Some(number.parse().map_err(|_| format!("{number} is not a card number"))?);
            }
            "--read-only" => options.read_only = true,
            "--help" | "-h" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(options)
}

/// One line typed in answer, or nothing when there's nobody to type one.
fn answer() -> String {
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    line.trim().to_lowercase()
}

fn choose_card(host: &Host, wanted: Option<u32>) -> Result<u32, String> {
    let listed = |minor: u32| host.cards.iter().any(|card| card.minor == minor);
    if let Some(minor) = wanted {
        return if listed(minor) { Ok(minor) } else { Err(format!("There is no card {minor}.")) };
    }
    if host.cards.len() == 1 {
        return Ok(host.cards[0].minor);
    }
    print!("Which card? Type its number: ");
    let _ = io::stdout().flush();
    let typed = answer();
    let minor: u32 = typed.parse().map_err(|_| format!("{typed:?} is not one of the numbers above."))?;
    if listed(minor) { Ok(minor) } else { Err(format!("There is no card {minor}.")) }
}

/// Writes the report where it was run from, or in the temp folder, and says where.
fn save(text: &str) {
    let here = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for folder in [here, env::temp_dir()] {
        let path = folder.join(REPORT_NAME);
        if fs::write(&path, text).is_ok() {
            println!("\nWritten to {}", path.display());
            return;
        }
    }
    println!("\nCould not write {REPORT_NAME} anywhere. The text above is the report.");
}

fn branch_of(host: &Host) -> (Option<&'static HostBranch>, Branch) {
    let profile = host_branch(host_branch_of_version(&host.version));
    let branch = match profile {
        Some(profile) => {
            Branch::Known { version_tested: profile.tested_versions.contains(&host.version.as_str()) }
        }
        None => Branch::Unknown,
    };
    (profile, branch)
}

fn run(options: &Options) -> ExitCode {
    let host = match host::read_host() {
        Ok(host) => host,
        Err(why) => {
            println!("{}", no_host_text(&why));
            if why == NoHost::NoDriver {
                println!("Guesthouse needs NVIDIA's open kernel modules loaded, with a card on them.");
            }
            return ExitCode::FAILURE;
        }
    };
    let (profile, branch) = branch_of(&host);
    let mut text = report::heading();
    text.push('\n');
    text.push_str(&report::host_lines(&host, branch));
    print!("{text}");

    if !host.open_module {
        println!(
            "\nThe loaded driver is not the open kernel module. Guesthouse only works with the open modules, \
             which most distributions call nvidia-open, so this program stops here."
        );
        save(&text);
        return ExitCode::FAILURE;
    }
    let Some(profile) = profile else {
        println!(
            "\nThe host driver {} is on branch {}, which this build has no profile for. The app would refuse \
             to start a VM, and this program stops here, as talking to a branch it doesn't know could hand \
             it the wrong sizes.",
            host.version,
            host_branch_of_version(&host.version)
        );
        save(&text);
        return ExitCode::FAILURE;
    };
    let minor = match choose_card(&host, options.card) {
        Ok(minor) => minor,
        Err(why) => {
            println!("{why}");
            return ExitCode::FAILURE;
        }
    };
    if minor != 0 {
        let note = format!(
            "\nNote: card {minor} is not the first one. This program has only been run on a first card, so \
             a wrong answer below could be the program's rather than the card's.\n"
        );
        print!("{note}");
        text.push_str(&note);
    }

    let rm = match LinuxRm::open_card(&host.version, profile, minor) {
        Ok(rm) => rm,
        Err(error) => {
            println!("\nOpening the driver failed: {error}");
            if error.to_string().contains("No such file") {
                println!("{NO_DEVICE_FILES}");
            }
            save(&text);
            return ExitCode::FAILURE;
        }
    };
    let mut device = match Device::open_card(rm, profile, minor) {
        Ok(device) => device,
        Err(error) => {
            println!("\nOpening card {minor} failed: {error}");
            save(&text);
            return ExitCode::FAILURE;
        }
    };
    let findings = match survey::survey(&mut device) {
        Ok(findings) => findings,
        Err(error) => {
            println!("\nReading the card failed: {error}");
            save(&text);
            return ExitCode::FAILURE;
        }
    };
    let card_text = format!("\n{}", report::card_lines(minor, &findings));
    print!("{card_text}");
    text.push_str(&card_text);
    save(&text);

    if options.read_only {
        return ExitCode::SUCCESS;
    }
    let Some(generation) = findings.generation else {
        println!("\nNo channel test: this build has no classes for a card it has no profile for.");
        return ExitCode::FAILURE;
    };
    println!("\n{CHANNEL_WARNING}");
    print!("Run the channel test? Type y or n: ");
    let _ = io::stdout().flush();
    if answer() != "y" {
        println!("Not run. The file holds the read-only part.");
        return ExitCode::SUCCESS;
    }
    let results = match channels::run_all(&mut device, generation, profile, &findings.nodes, channels::WAIT) {
        Ok(results) => results,
        Err(error) => {
            let failed = format!("\nChannel test: could not make an address space: {error}\n");
            print!("{failed}");
            text.push_str(&failed);
            save(&text);
            return ExitCode::FAILURE;
        }
    };
    let channel_text = format!("\n{}", report::channel_lines(&results));
    print!("{channel_text}");
    text.push_str(&channel_text);
    save(&text);
    let passed = results.iter().take(2).all(ChannelResult::released);
    if passed { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn main() -> ExitCode {
    match parse_options() {
        Ok(options) => run(&options),
        Err(text) => {
            println!("{text}");
            ExitCode::FAILURE
        }
    }
}
