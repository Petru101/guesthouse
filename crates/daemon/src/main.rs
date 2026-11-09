//! `guesthouse-daemon [PROJECT_FOLDER]`: serves one VM, named by `GUESTHOUSE_SLOT`.
//!
//! The real backend is behind the `nvidia` or `amd` feature. One binary opens one vendor's card, so
//! a build with both, such as `--all-features`, is the NVIDIA daemon. A build with no backend can
//! only say so. The `stand-in` feature answers messages with nothing
//! behind them, to try the sockets by hand.

use guesthouse_daemon::logger;
use guesthouse_daemon::slot::Slot;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let verbose = env::var_os("GUESTHOUSE_VERBOSE").is_some();
    logger::start(verbose);
    let slot = match Slot::from_environment() {
        Ok(slot) => slot,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let project = env::args_os().nth(1).map_or_else(|| PathBuf::from("."), PathBuf::from);
    match serve(slot, project) {
        Ok(()) => {
            slot.remove_shared_files();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(feature = "stand-in", feature = "nvidia", feature = "amd")))]
fn serve(_slot: Slot, _project: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    Err("This build has no GPU backend in it, so it cannot run a guest.".into())
}

/// Starts the AMD backend and serves. This opens the host's AMD card and no other.
#[cfg(all(feature = "amd", not(feature = "nvidia")))]
fn serve(slot: Slot, project: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use guesthouse_backend::Backend;
    use guesthouse_backend_amd::AmdBackend;
    use guesthouse_core::{Config, MemoryLimits, Session};
    use guesthouse_daemon::amd_setup::read_setup;
    use guesthouse_daemon::environment::{guest_ram, video_cap};
    use guesthouse_daemon::guest_memory::LazySharedMemory;
    use guesthouse_daemon::jail::{Needs, RAM_FILE, WINDOW_FILE, lock_calls, lock_files};
    use guesthouse_daemon::router::Router;
    use guesthouse_daemon::server::Server;
    use guesthouse_daemon::startup::start_backend;

    let setup = read_setup()?;
    let address = setup.card_address.map(|pci| pci.to_string());
    let node = guesthouse_backend_amd::amd_render_node(address.as_deref())
        .ok_or("No AMD graphics card was found at the address in the AMD setup.")?;
    log::info!("AMD card at {}", node.display());
    let run_folder = slot.run_folder(&project);
    let needs = Needs::of_amd_card(slot, &project, &setup.driver_folder, node);
    needs.prepare()?;
    let locked = lock_files(&needs)?;
    let backend: AmdBackend = start_backend(&setup)?;
    let (ram, cap) = (env::var("GUESTHOUSE_RAM").ok(), env::var("GUESTHOUSE_VRAM").ok());
    let limits = MemoryLimits::new(backend.facts(), video_cap(cap.as_deref()), guest_ram(ram.as_deref()));
    let config = Config::new(limits);
    let memory = LazySharedMemory::new(slot.shared_file(WINDOW_FILE), slot.shared_file(RAM_FILE));
    let mut session = Session::new(backend, memory, config);
    session.timings_mut().set_enabled(env::var_os("GUESTHOUSE_TIMING").is_some());
    let mut server = Server::bind(&run_folder, Router::new(session))?;
    lock_calls(locked)?;
    server.run()?;
    Ok(())
}

/// Starts the real backend and serves. This opens the host's GPU.
#[cfg(feature = "nvidia")]
fn serve(slot: Slot, project: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use guesthouse_backend::Backend;
    use guesthouse_backend_nvidia::NvidiaBackend;
    use guesthouse_backend_nvidia::rm::linux::LinuxRm;
    use guesthouse_core::{Config, MemoryLimits, Session};
    use guesthouse_daemon::environment::{guest_ram, video_cap};
    use guesthouse_daemon::guest_memory::LazySharedMemory;
    use guesthouse_daemon::jail::{Needs, RAM_FILE, WINDOW_FILE, lock_calls, lock_files};
    use guesthouse_daemon::router::Router;
    use guesthouse_daemon::server::Server;
    use guesthouse_daemon::setup::read_setup;
    use guesthouse_daemon::startup::start_backend;

    let setup = read_setup()?;
    let run_folder = slot.run_folder(&project);
    let needs = Needs::of_card(slot, &project, &setup.driver_folder);
    needs.prepare()?;
    let locked = lock_files(&needs)?;
    let mut backend: NvidiaBackend<LinuxRm> = start_backend(&setup)?;
    if let Some(micros) = timeslice(env::var("GUESTHOUSE_TIMESLICE_US").ok().as_deref()) {
        backend.set_graphics_timeslice(micros);
    }
    let (ram, cap) = (env::var("GUESTHOUSE_RAM").ok(), env::var("GUESTHOUSE_VRAM").ok());
    let limits = MemoryLimits::new(backend.facts(), video_cap(cap.as_deref()), guest_ram(ram.as_deref()));
    let config = Config::new(limits);
    let memory = LazySharedMemory::new(slot.shared_file(WINDOW_FILE), slot.shared_file(RAM_FILE));
    let mut session = Session::new(backend, memory, config);
    session.timings_mut().set_enabled(env::var_os("GUESTHOUSE_TIMING").is_some());
    if env::var_os("GUESTHOUSE_HELD_COPIES").is_some() {
        session.set_copy_order(guesthouse_core::CopyOrder::Held);
    }
    let mut server = Server::bind(&run_folder, Router::new(session))?;
    lock_calls(locked)?;
    server.run()?;
    Ok(())
}

/// A timeslice in microseconds, if the setting is there and makes sense.
#[cfg(feature = "nvidia")]
fn timeslice(text: Option<&str>) -> Option<u64> {
    let text = text?;
    let micros = text.parse().ok();
    if micros.is_none() {
        log::warn!("GUESTHOUSE_TIMESLICE_US is {text}, which isn't a number of microseconds");
    }
    micros
}

#[cfg(all(feature = "stand-in", not(any(feature = "nvidia", feature = "amd"))))]
fn serve(slot: Slot, project: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use guesthouse_backend::Backend;
    use guesthouse_backend_standin::{SparseMemory, StandIn};
    use guesthouse_core::{Config, MemoryLimits, Session};
    use guesthouse_daemon::environment::{guest_ram, video_cap};
    use guesthouse_daemon::guest_memory::WINDOW_BASE;
    use guesthouse_daemon::jail::{Needs, lock_calls, lock_files};
    use guesthouse_daemon::router::Router;
    use guesthouse_daemon::server::Server;

    /// A window of the usual size, which costs nothing as the stand-in stores only what is written.
    const WINDOW_SIZE: u64 = 8 << 30;

    let run_folder = slot.run_folder(&project);
    let needs = Needs { run_folder: run_folder.clone(), ..Needs::default() };
    needs.prepare()?;
    let locked = lock_files(&needs)?;
    let backend = StandIn::new();
    let (ram, cap) = (env::var("GUESTHOUSE_RAM").ok(), env::var("GUESTHOUSE_VRAM").ok());
    let limits = MemoryLimits::new(backend.facts(), video_cap(cap.as_deref()), guest_ram(ram.as_deref()));
    let memory = SparseMemory::new(WINDOW_BASE + WINDOW_SIZE);
    let router = Router::new(Session::new(backend, memory, Config::new(limits)));
    log::warn!("serving {} with a stand-in GPU: this will not run a guest", run_folder.display());
    let mut server = Server::bind(&run_folder, router)?;
    lock_calls(locked)?;
    server.run()?;
    Ok(())
}
