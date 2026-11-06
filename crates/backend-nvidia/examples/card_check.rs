//! `card_check OUTPUT_FOLDER`: opens the real card, reads it, and writes what the guest would be told,
//! to compare with the prototype's answers. Every control it runs only reads. `GUESTHOUSE_DRIVER_FOLDER`
//! and `GUESTHOUSE_DEVICE` say what setup would.

use guesthouse_backend::{
    Backend, DriverVersion, PciCard, Setup, StartDecision, Status, VendorQuery, decide_start,
};
use guesthouse_backend_nvidia::NvidiaBackend;
use guesthouse_backend_nvidia::inf::INF_NAME;
use guesthouse_backend_nvidia::profile::GUEST_617;
use guesthouse_backend_nvidia::rm::linux::LinuxRm;
use guesthouse_backend_nvidia::startup::guest_branch_of;
use std::path::PathBuf;
use std::{env, fs};

const NODE_ANSWER_SIZE: usize = 78;
const NODE_QUERY: u32 = 25;
const QUERIES: [(u32, usize); 11] =
    [(11, 4), (15, 4), (17, 12), (22, 4), (24, 4), (55, 4), (60, 80), (61, 56), (62, 64), (75, 4), (76, 4)];
const MOST_NODES: u32 = 40;
const NVIDIA_VENDOR: u16 = 0x10DE;

fn main() {
    let output = PathBuf::from(env::args_os().nth(1).expect("usage: card_check OUTPUT_FOLDER"));
    let folder = PathBuf::from(env::var_os("GUESTHOUSE_DRIVER_FOLDER").expect("GUESTHOUSE_DRIVER_FOLDER"));
    let device = u16::from_str_radix(&env::var("GUESTHOUSE_DEVICE").expect("GUESTHOUSE_DEVICE"), 16).unwrap();
    let bytes = fs::read(folder.join(INF_NAME)).unwrap();
    let text: String = bytes.iter().map(|&byte| char::from(byte)).collect();
    let version: DriverVersion =
        guesthouse_backend::driver_version_from_inf(&text).expect("a driver version");
    let setup = Setup {
        guest_driver_version: version,
        guest_branch: guest_branch_of(version),
        driver_folder: folder,
        card: Some(PciCard { vendor: NVIDIA_VENDOR, device, revision: 0xA1 }),
    };
    let survey = NvidiaBackend::<LinuxRm>::survey(&setup).map_err(|refusal| refusal.0).unwrap();
    println!("{survey}");
    match decide_start(&survey) {
        StartDecision::Refuse(reason) => panic!("{reason}"),
        StartDecision::RunWithWarning(warning) => println!("{warning}"),
        StartDecision::Run => {}
    }
    let backend = NvidiaBackend::<LinuxRm>::open(&setup, &survey).map_err(|refusal| refusal.0).unwrap();
    println!("{:?}", backend.facts());

    fs::create_dir_all(&output).unwrap();
    let answer = |kind, size, data: &[u8]| {
        let mut out = Vec::new();
        let query = VendorQuery { kind, size, data, process_is_32_bit: false };
        (backend.answers().answer(&query, &mut out) == Some(Status::SUCCESS)).then_some(out)
    };
    fs::write(output.join("record"), answer(0, GUEST_617.record.size, &[]).unwrap()).unwrap();
    for (kind, size) in QUERIES {
        if let Some(bytes) = answer(kind, size, &[]) {
            fs::write(output.join(format!("qai-{kind}")), bytes).unwrap();
        }
    }
    for node in 0..MOST_NODES {
        if let Some(bytes) = answer(NODE_QUERY, NODE_ANSWER_SIZE, &node.to_le_bytes()) {
            fs::write(output.join(format!("node-{node}")), bytes).unwrap();
        }
    }
}
