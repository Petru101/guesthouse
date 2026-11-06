//! `fake_frames RUN_FOLDER [FRAMES_A_SECOND]`: serves `frames.sock` with a pretend guest on the pretend
//! RM, for trying a viewer or a test client with no card and no VM. The pictures are the pretend
//! engine's stripes, which move from frame to frame.

use guesthouse_backend::Backend;
use guesthouse_backend_nvidia::NvidiaBackend;
use guesthouse_backend_nvidia::fake_guest::{
    copy_to_staging, create_surfaces, fake_backend, lock_surface, set_io_space, surface_handles,
};
use guesthouse_backend_nvidia::rm::fake::FakeRm;
use guesthouse_backend_standin::SparseMemory;
use guesthouse_core::{Config, MemoryLimits, Session};
use guesthouse_daemon::router::Router;
use guesthouse_daemon::server::Server;
use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const WINDOW: u64 = 1 << 40;
const WINDOW_END: u64 = WINDOW + (1 << 30);
const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const GUEST_RAM: u64 = 16 << 30;
const DEFAULT_RATE: u64 = 60;
const STEP: Option<Duration> = Some(Duration::from_millis(1));

type Guest = Session<NvidiaBackend<FakeRm>, SparseMemory>;

fn call(session: &mut Guest, message: &[u8]) -> Vec<u8> {
    let mut reply = Vec::new();
    let _ = session.handle_message(message, None, &mut reply);
    reply
}

fn main() {
    let folder = PathBuf::from(env::args().nth(1).expect("usage: fake_frames RUN_FOLDER [FRAMES_A_SECOND]"));
    let rate = env::args().nth(2).and_then(|text| text.parse().ok()).unwrap_or(DEFAULT_RATE);
    let mut rm = FakeRm::new();
    rm.engine.running = true;
    let backend = fake_backend(rm);
    let limits = MemoryLimits::new(backend.facts(), None, GUEST_RAM);
    let mut config = Config::new(limits);
    config.frame_wait = Duration::ZERO;
    let mut session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
    call(&mut session, &set_io_space(WINDOW));
    let (image, staging) = surface_handles(&call(&mut session, &create_surfaces(WIDTH, HEIGHT)));
    call(&mut session, &lock_surface(staging));
    let present = copy_to_staging(image, staging, WIDTH, HEIGHT);

    let mut server = Server::bind(&folder, Router::new(session)).expect("the run folder can be used");
    let pace = Duration::from_micros(1_000_000 / rate);
    let mut next = Instant::now();
    println!("serving {}", folder.display());
    loop {
        server.step(STEP).expect("the sockets stay up");
        if Instant::now() >= next {
            next += pace;
            call(server.router_mut().session_mut(), &present);
        }
    }
}
