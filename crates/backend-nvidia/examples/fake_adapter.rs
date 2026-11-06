//! `fake_adapter DRIVER_FOLDER OUTPUT_FOLDER`: writes what the backend works out for the pretend card,
//! to compare with another implementation of the same rules. It never touches a real card.

use guesthouse_backend::{DriverVersion, PciCard, Setup, Status, VendorQuery};
use guesthouse_backend_nvidia::NvidiaBackend;
use guesthouse_backend_nvidia::inf::INF_NAME;
use guesthouse_backend_nvidia::profile::{GUEST_617, HOST_615};
use guesthouse_backend_nvidia::record::read_host_facts_with_clocks;
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::fake::FakeRm;
use guesthouse_backend_nvidia::rm::fake_card::{FAKE_DEVICE, install};
use std::path::PathBuf;
use std::{env, fs};

const RATED_CLOCKS: (u64, u64) = (1_500_000_000, 5_000_000_000);
const NODE_ANSWER_SIZE: usize = 78;
const NODE_QUERY: u32 = 25;
const QUERIES: [(u32, usize); 11] =
    [(11, 4), (15, 4), (17, 12), (22, 4), (24, 4), (55, 4), (60, 80), (61, 56), (62, 64), (75, 4), (76, 4)];
const MOST_NODES: u32 = 40;

fn main() {
    let mut arguments = env::args_os().skip(1).map(PathBuf::from);
    let (folder, output) = (arguments.next().expect("driver folder"), arguments.next().expect("output"));
    let mut rm = FakeRm::new();
    install(&mut rm);
    let device = Device::open(rm, &HOST_615).unwrap();
    let setup = Setup {
        guest_driver_version: DriverVersion::parse("32.0.16.1742").unwrap(),
        guest_branch: "617".into(),
        driver_folder: folder.clone(),
        card: Some(PciCard { vendor: 0x10DE, device: FAKE_DEVICE as u16, revision: 0xA1 }),
    };
    let host = read_host_facts_with_clocks(RATED_CLOCKS).unwrap();
    let inf = fs::read(folder.join(INF_NAME)).unwrap();
    let backend = NvidiaBackend::start(device, &HOST_615, &GUEST_617, &setup, &host, &inf);
    let backend = backend.map_err(|refusal| refusal.0).unwrap();

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
