//! A pretend Ada card and a pretend Turing card, held to what `tools/card-report` gives for each chip.
//! This shows the tables and the code around them agree with those values, and nothing about a real
//! card. The engines the pretend cards list are only there to make the nodes come out.

use guesthouse_backend::{Backend, Status, VendorQuery};
use guesthouse_backend_nvidia::NvidiaBackend;
use guesthouse_backend_nvidia::channel::{AREA_SIZE, Channel, ChannelPlace, Engine};
use guesthouse_backend_nvidia::fake_guest::{message, start_on_card};
use guesthouse_backend_nvidia::profile::{
    ADA, GUEST_617, Generation, HOST_615, TURING, generation_for, guest_channel_classes,
};
use guesthouse_backend_nvidia::pushbuffer::{RingEntry, ring_entry};
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::fake::FakeRm;
use guesthouse_backend_nvidia::rm::fake_card::{FAKE_ADA, FAKE_TURING, FakeCard};
use guesthouse_backend_standin::SparseMemory;
use guesthouse_core::{Answer, Config, MemoryLimits, ReplyTicket, Session};
use guesthouse_messages::wire::{set_u32, set_u64, u32_at, u64_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use std::time::Duration;

const LAYOUT: &guesthouse_backend_nvidia::profile::RecordLayout = &GUEST_617.record;
const RECORD_QUERY: u32 = 0;
const NODE_QUERY: u32 = 25;
const NODE_ANSWER_SIZE: usize = 78;
const NODE_ENGINE_FIELD: usize = 4;
const NODE_NAME_FIELD: usize = 8;
const NODE_NAME_BYTES: usize = 64;
const MOST_NODES: u32 = 40;
const GRAPHICS_TABLE_SM: usize = 0x0C * 8 + 4;
const PCI_DEVICE_FIELD: usize = 12;
const IMPLEMENTATION_FIELD: usize = 4;

const WINDOW: u64 = 1 << 40;
const WINDOW_END: u64 = WINDOW + (1 << 30);
const TICKET: Option<ReplyTicket> = Some(ReplyTicket { channel: 1, transaction: 9 });
const AREA_ADDRESS: u64 = 0x800_0000;

// The fields of a submission's private data, with a list inside it.
const LIST_OFFSET_FIELD: usize = 0x0C;
const ENTRIES_FIELD: usize = 0x14;
const LIST_AT: usize = 0x84;

/// What card-report gives for a card.
struct Report {
    device: u16,
    architecture: u32,
    implementation: u32,
    sm_version: u32,
    shader_set: u32,
    classes: usize,
    /// The classes this backend makes, with the generation's own first.
    used_classes: [u32; 7],
    /// The engine nodes Windows lists, in order.
    nodes: &'static [&'static str],
}

const RTX_4080: Report = Report {
    device: 0x2704,
    architecture: 0x190,
    implementation: 0x3,
    sm_version: 0x809,
    shader_set: 0x7005,
    classes: 127,
    used_classes: [0xC997, 0xC9C0, 0xC56F, 0xC7B5, 0xC561, 0xA140, 0x902D],
    nodes: &[
        "3D",
        "overlay",
        "decode",
        "copy",
        "copy",
        "Security",
        "encode",
        "encode",
        "OFA_0",
        "VR",
        "copy",
        "copy",
        "copy",
        "copy",
        "Security_1",
    ],
};

const GTX_1660_SUPER: Report = Report {
    device: 0x21C4,
    architecture: 0x160,
    implementation: 0x8,
    sm_version: 0x703,
    shader_set: 0x6000,
    classes: 110,
    used_classes: [0xC597, 0xC5C0, 0xC46F, 0xC5B5, 0xC461, 0xA140, 0x902D],
    nodes: &[
        "3D",
        "overlay",
        "decode",
        "copy",
        "copy",
        "copy",
        "Security",
        "encode",
        "VR",
        "copy",
        "copy",
        "copy",
        "copy",
        "Security_1",
    ],
};

/// What Windows lists and the guest isn't given: the display's node, and engines with no node here.
const LEFT_OUT: [&str; 4] = ["overlay", "decode", "encode", "OFA_0"];

const CARDS: [(&FakeCard, &Generation, &Report); 2] =
    [(&FAKE_ADA, &ADA, &RTX_4080), (&FAKE_TURING, &TURING, &GTX_1660_SUPER)];

fn started(card: &FakeCard) -> NvidiaBackend<FakeRm> {
    let backend = start_on_card(FakeRm::new(), card, card.device as u16);
    backend.map_err(|refusal| refusal.0).expect("the pretend card passes")
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn answer(backend: &NvidiaBackend<FakeRm>, kind: u32, size: usize, data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let query = VendorQuery { kind, size, data, process_is_32_bit: false };
    (backend.answers().answer(&query, &mut out) == Some(Status::SUCCESS)).then_some(out)
}

fn record(backend: &NvidiaBackend<FakeRm>) -> Vec<u8> {
    answer(backend, RECORD_QUERY, LAYOUT.size, &[]).unwrap()
}

fn classes_in(record: &[u8]) -> Vec<u32> {
    let count = word(record, LAYOUT.classes) as usize;
    (0..count).map(|index| word(record, LAYOUT.classes + 4 + index * 4)).collect()
}

/// A node as the report names it.
fn node_label(answer: &[u8]) -> String {
    let name = &answer[NODE_NAME_FIELD..NODE_NAME_FIELD + NODE_NAME_BYTES];
    let units: Vec<u16> = name.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit)).collect();
    let name = String::from_utf16_lossy(&units);
    match word(answer, NODE_ENGINE_FIELD) {
        1 => "3D".to_owned(),
        2 => "decode".to_owned(),
        3 => "encode".to_owned(),
        6 => "copy".to_owned(),
        _ => name.trim_end_matches('\0').to_owned(),
    }
}

#[test]
fn each_architecture_finds_its_generation() {
    for (_, generation, report) in CARDS {
        assert_eq!(generation_for(report.architecture), Some(generation));
    }
}

#[test]
fn the_guest_is_told_what_the_report_says() {
    for (card, _, report) in CARDS {
        let backend = started(card);
        let record = record(&backend);
        assert_eq!(word(&record, LAYOUT.pci + PCI_DEVICE_FIELD) >> 16, u32::from(report.device));
        assert_eq!(word(&record, LAYOUT.architecture), report.architecture);
        assert_eq!(word(&record, LAYOUT.architecture + IMPLEMENTATION_FIELD), report.implementation);
        assert_eq!(word(&record, LAYOUT.graphics_table + GRAPHICS_TABLE_SM), report.sm_version);
        assert_eq!(word(&record, LAYOUT.shader_set), report.shader_set);
        assert_eq!(backend.facts().card.device, report.device);
    }
}

#[test]
fn the_class_list_fits_and_holds_every_class_the_backend_makes() {
    for (card, generation, report) in CARDS {
        let classes = classes_in(&record(&started(card)));
        assert_eq!(classes.len(), report.classes);
        for class in report.used_classes {
            assert!(classes.contains(&class), "{class:#x}");
        }
        for class in [generation.channel_class, generation.copy_class, generation.usermode_class] {
            assert!(report.used_classes.contains(&class), "{class:#x}");
        }
    }
}

#[test]
fn the_nodes_are_the_reports_in_its_order_less_those_left_out() {
    for (card, _, report) in CARDS {
        let backend = started(card);
        let told: Vec<String> = (0..MOST_NODES)
            .filter_map(|node| answer(&backend, NODE_QUERY, NODE_ANSWER_SIZE, &node.to_le_bytes()))
            .map(|answer| node_label(&answer))
            .collect();
        let wanted: Vec<&str> =
            report.nodes.iter().copied().filter(|node| !LEFT_OUT.contains(node)).collect();
        assert_eq!(told, wanted);
        assert_eq!(backend.facts().node_count as usize, wanted.len());
    }
}

#[test]
fn no_video_node_means_the_record_offers_no_video() {
    for (card, generation, _) in CARDS {
        let record = record(&started(card));
        assert_eq!(generation.nodes.decode.len() + generation.nodes.encode.len(), 0);
        assert_eq!([word(&record, LAYOUT.flags), word(&record, LAYOUT.flags + 4)], [0, 0]);
        assert_eq!(record[LAYOUT.decoder_flags], 0);
    }
}

#[test]
fn the_guests_handles_stand_for_this_generations_classes() {
    for (_, generation, report) in CARDS {
        let classes = guest_channel_classes(&GUEST_617, generation);
        let found: Vec<u32> = classes.iter().map(|&(_, class)| class).collect();
        assert_eq!(found, [report.used_classes[5], report.used_classes[0], report.used_classes[6]]);
    }
}

fn channel(generation: &Generation, engine: Engine) -> (Device<FakeRm>, Vec<u32>) {
    let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
    let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
    let place = ChannelPlace {
        ring_address: 0x900_0000,
        ring_entries: 1024,
        area_address: AREA_ADDRESS,
        area_size: AREA_SIZE,
    };
    let channel = Channel::new(&mut device, generation, &HOST_615, &space, place, engine).unwrap();
    let prelude = RingEntry::parse(channel.prelude_entry().to_le_bytes());
    let start = (prelude.address - AREA_ADDRESS) as usize;
    let words = channel.area().load_words(start, prelude.length as usize);
    (device, words)
}

#[test]
fn a_graphics_channel_makes_and_binds_the_generations_classes_in_order() {
    for (_, generation, _) in CARDS {
        let (device, words) = channel(generation, Engine::Graphics);
        let made: Vec<u32> = device.rm.allocated().map(|(class, _)| class).collect();
        assert!(made.contains(&generation.usermode_class));
        let wanted: Vec<u32> = generation.graphics_classes.iter().map(|class| class.0).collect();
        let at = made.iter().position(|&class| class == generation.channel_class).unwrap();
        assert_eq!(made[at + 1..at + 1 + wanted.len()], wanted);
        let bound: Vec<u32> = words.chunks(2).take(wanted.len()).map(|pair| pair[1]).collect();
        assert_eq!(bound, wanted);
    }
}

#[test]
fn a_copy_channel_makes_the_generations_copy_class() {
    for (_, generation, _) in CARDS {
        let (device, words) = channel(generation, Engine::Copy);
        assert!(device.rm.allocated().any(|(class, _)| class == generation.copy_class));
        assert_eq!(words[1], generation.copy_class);
    }
}

type Guest = Session<NvidiaBackend<FakeRm>, SparseMemory>;

fn send(session: &mut Guest, message: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    assert_eq!(session.handle_message(message, TICKET, &mut out), Answer::Now);
    out
}

fn guest_on(card: &FakeCard) -> Guest {
    let backend = started(card);
    let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
    let mut config = Config::new(limits);
    config.frame_wait = Duration::ZERO;
    let mut session = Session::new(backend, SparseMemory::new(WINDOW_END), config);
    let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, WINDOW);
    send(&mut session, &io_space);
    session
}

#[test]
fn a_guests_first_submission_runs_on_a_channel_of_this_generation() {
    for (card, generation, _) in CARDS {
        let mut session = guest_on(card);
        let mut request = message(Command::CreateContext, 0x34);
        set_u32(&mut request, 0x20, 0);
        let context = u32_at(&send(&mut session, &request), 0x18).unwrap();
        let mut request = message(Command::CreateHardwareQueue, 0x44 + 0x80);
        set_u32(&mut request, 0x38, context);
        set_u32(&mut request, 0x40, 0x80);
        let reply = send(&mut session, &request);
        let queue = u32_at(&reply, 0x1C).unwrap();
        assert_ne!(u64_at(&reply, 0x28), Ok(0));

        let mut private = vec![0; LIST_AT + 8];
        set_u32(&mut private, LIST_OFFSET_FIELD, LIST_AT as u32);
        set_u32(&mut private, ENTRIES_FIELD, 1);
        set_u64(&mut private, LIST_AT, ring_entry(0x1000, 4));
        let mut submission = message(Command::SubmitToHardwareQueue, 0x50);
        set_u32(&mut submission, 0x18, queue);
        set_u64(&mut submission, 0x20, 1);
        submission.extend_from_slice(&private);
        send(&mut session, &submission);

        let rm = &session.backend().device().rm;
        assert!(rm.allocated().any(|(class, _)| class == generation.channel_class));
        assert!(!rm.allocated().any(|(class, _)| class == 0xCA6F), "no Blackwell class on this card");
        assert_eq!(rm.doorbells().len(), 1);
    }
}

#[test]
fn a_card_with_no_profile_is_refused_by_name() {
    for architecture in [0x170, 0x180] {
        let card = FakeCard { architecture, ..FAKE_ADA };
        let Err(refusal) = start_on_card(FakeRm::new(), &card, card.device as u16) else {
            panic!("{architecture:#x} has no profile");
        };
        assert_eq!(refusal.0, format!("This build has no profile for GPU architecture {architecture:#x}."));
    }
}

#[test]
fn a_chip_with_an_sm_version_nobody_has_reported_is_refused() {
    let card = FakeCard { sm_version: 0x705, ..FAKE_TURING };
    let Err(refusal) = start_on_card(FakeRm::new(), &card, card.device as u16) else {
        panic!("0x705 has no shader set");
    };
    assert!(refusal.0.contains("SM 0x705"), "{}", refusal.0);
}

#[test]
fn these_generations_have_not_been_run_with_any_guest_branch() {
    assert!(ADA.run_on_guest_branches.is_empty());
    assert!(TURING.run_on_guest_branches.is_empty());
}

#[test]
fn the_summary_says_what_was_found_and_holds_nothing_that_identifies_the_card() {
    for (card, generation, report) in CARDS {
        let backend = started(card);
        let summary = backend.card_summary();
        assert!(summary.contains(&format!("Generation: {}", generation.name)), "{summary}");
        assert!(summary.contains(&format!("SM version: {:#X}", report.sm_version)), "{summary}");
        assert!(summary.contains(&format!("Shader set: {:#X}", report.shader_set)), "{summary}");
        assert!(summary.contains(&format!("Classes: {}", report.classes)), "{summary}");
        let record = record(&backend);
        let uuid: String =
            record[LAYOUT.uuid..LAYOUT.uuid + 4].iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(!summary.to_lowercase().contains(&uuid), "{summary}");
    }
}
