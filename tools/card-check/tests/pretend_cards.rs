//! The program on the pretend Ada, Ampere and Turing cards: what it finds, what it says, and what never
//! reaches the file. Nothing here goes near a card.

use guesthouse_backend::PciAddress;
use guesthouse_backend_nvidia::profile::{ADA, AMPERE, BLACKWELL, Generation, HOST_615, TURING};
use guesthouse_backend_nvidia::record::card::UUID;
use guesthouse_backend_nvidia::rm::classes::{CONTROL_GET_GPU_ID, DEVICE};
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::fake::{Call, FakeGpu, FakeRm};
use guesthouse_backend_nvidia::rm::fake_card::{
    FAKE_ADA, FAKE_AMPERE, FAKE_BLACKWELL, FAKE_TURING, FakeCard, install_card,
};
use guesthouse_backend_nvidia::rm::{Rm, STATUS_INSUFFICIENT_PERMISSIONS};
use guesthouse_card_check::channels::{Outcome, refusal, run_all, run_on_listed_card};
use guesthouse_card_check::host::{Host, ListedCard};
use guesthouse_card_check::report::{
    Branch, card_lines, channel_lines, heading, host_lines, place_line, progress_lines,
};
use guesthouse_card_check::survey::{Findings, Verdict, survey};
use std::time::Duration;

/// What the pretend cards should come out as, from `other_generations.rs`.
struct Expected {
    card: &'static FakeCard,
    generation: &'static Generation,
    implementation: u32,
    shader_set: u32,
    nodes: usize,
    classes: usize,
    copy_caps: u32,
}

const CARDS: [Expected; 3] = [
    Expected {
        card: &FAKE_ADA,
        generation: &ADA,
        implementation: 0x3,
        shader_set: 0x7005,
        nodes: 15,
        classes: 127,
        copy_caps: 0x1A0,
    },
    Expected {
        card: &FAKE_AMPERE,
        generation: &AMPERE,
        implementation: 0x7,
        shader_set: 0x7002,
        nodes: 13,
        classes: 110,
        copy_caps: 0x1A0,
    },
    Expected {
        card: &FAKE_TURING,
        generation: &TURING,
        implementation: 0x8,
        shader_set: 0x6000,
        nodes: 14,
        classes: 110,
        copy_caps: 0x120,
    },
];

/// The pretend Ada card's classes, with the deferred one it leaves out.
const ADA_CLASSES_WITH_DEFERRED: [u32; 14] = [
    0x902D, 0xA140, 0xC36F, 0xC361, 0xC46F, 0xC461, 0xC56F, 0xC561, 0xC7B5, 0xC997, 0xC9B0, 0xC9B7, 0xC9C0,
    0x5080,
];

fn device_on(card: &FakeCard) -> Device<FakeRm> {
    let mut rm = FakeRm::new();
    install_card(&mut rm, card);
    Device::open(rm, &HOST_615).unwrap()
}

fn findings_on(card: &FakeCard) -> Findings {
    survey(&mut device_on(card)).unwrap()
}

fn host() -> Host {
    Host {
        version: "615.78.08".to_owned(),
        open_module: true,
        cards: vec![
            listed("0000:01:00.0", 0, "NVIDIA GeForce RTX 4080"),
            listed("0000:02:00.0", 1, "NVIDIA GeForce GTX 1660 SUPER"),
        ],
    }
}

/// The whole file, as the program would write it after a channel test.
fn whole_report(card: &FakeCard) -> String {
    let findings = findings_on(card);
    let mut device = device_on(card);
    let generation = findings.generation.unwrap();
    let results =
        run_all(&mut device, generation, &HOST_615, &findings.nodes, Duration::ZERO, &mut |_| Ok(()))
            .unwrap();
    let mut text = heading();
    let host = host();
    text.push_str(&host_lines(&host, Branch::Known { version_tested: true }));
    text.push_str(&place_line(0, &host.cards[0], device.instance));
    text.push_str(&card_lines(0, &findings));
    text.push_str(&channel_lines(&results));
    text
}

#[test]
fn the_pretend_cards_are_read_as_the_tables_say() {
    for expected in &CARDS {
        let findings = findings_on(expected.card);
        assert_eq!(findings.name, expected.card.name);
        assert_eq!(u32::from(findings.device), expected.card.device);
        assert_eq!(findings.architecture, expected.card.architecture);
        assert_eq!(findings.implementation, expected.implementation);
        assert_eq!(findings.sm_version, expected.card.sm_version);
        assert_eq!(findings.shader_set, Some(expected.shader_set));
        assert_eq!(findings.generation, Some(expected.generation));
        assert_eq!(findings.engines, expected.card.engines);
        assert_eq!(findings.graphics_copy, Some(expected.card.graphics_copy));
        assert_eq!(findings.nodes.len(), expected.nodes);
        assert_eq!(findings.classes.len(), expected.classes);
        assert!(!findings.copy_engines.is_empty());
        for copy in &findings.copy_engines {
            assert_eq!(copy.caps, expected.copy_caps, "{:#x}", copy.engine);
        }
    }
}

#[test]
fn ada_and_turing_are_experimental_and_have_never_run() {
    for expected in &CARDS {
        let findings = findings_on(expected.card);
        let wanted = Verdict::Experimental { generation: expected.generation.name, ever_run: false };
        assert_eq!(findings.verdict, wanted);
    }
}

#[test]
fn a_blackwell_card_that_has_not_been_run_is_experimental_and_the_tested_one_is_supported() {
    let findings = findings_on(&FAKE_BLACKWELL);
    assert_eq!(findings.verdict, Verdict::Experimental { generation: BLACKWELL.name, ever_run: true });
    let tested = FakeCard { device: 0x2B85, ..FAKE_BLACKWELL };
    assert_eq!(findings_on(&tested).verdict, Verdict::Supported);
}

#[test]
fn a_card_with_no_profile_or_an_unknown_sm_is_refused_by_name() {
    let unknown = FakeCard { architecture: 0x140, ..FAKE_ADA };
    let findings = findings_on(&unknown);
    assert_eq!(findings.generation, None);
    assert!(findings.nodes.is_empty());
    let Verdict::Refused(reason) = &findings.verdict else { panic!("{:?}", findings.verdict) };
    assert!(reason.contains("0x140"), "{reason}");
    assert!(!findings.classes.is_empty(), "the classes are still listed");

    let odd_sm = FakeCard { sm_version: 0x705, ..FAKE_TURING };
    let findings = findings_on(&odd_sm);
    assert_eq!(findings.shader_set, None);
    let Verdict::Refused(reason) = &findings.verdict else { panic!("{:?}", findings.verdict) };
    assert!(reason.contains("SM 0x705"), "{reason}");
}

#[test]
fn the_report_says_what_was_found_and_what_the_app_would_do() {
    for expected in &CARDS {
        let text = card_lines(0, &findings_on(expected.card));
        assert!(text.contains(&format!("Card 0: {}", expected.card.name)), "{text}");
        assert!(text.contains(&format!("Generation: {}", expected.generation.name)), "{text}");
        assert!(text.contains(&format!("shader set {:#X}", expected.shader_set)), "{text}");
        assert!(text.contains(&format!("Nodes a guest would see: {}", expected.nodes)), "{text}");
        assert!(text.contains(&format!("Classes: {}", expected.classes)), "{text}");
        let deferred = format!("Classes {} uses but the card does not list: 5080", expected.generation.name);
        assert!(text.contains(&deferred), "the pretend cards leave out the deferred class: {text}");
        assert!(text.contains("Verdict: experimental"), "{text}");
        assert!(text.contains("decoder 0"), "{text}");
        assert!(text.contains("encoder 0"), "{text}");
    }
}

#[test]
fn the_report_names_the_classes_a_card_does_not_list() {
    let findings = findings_on(&FakeCard { classes: &[0x902D], ..FAKE_ADA });
    let text = card_lines(0, &findings);
    assert!(
        text.contains("Classes Ada uses but the card does not list: C56F C7B5 C561 C997 C9C0 A140 5080"),
        "{text}"
    );
    let findings = findings_on(&FakeCard { classes: &ADA_CLASSES_WITH_DEFERRED, ..FAKE_ADA });
    assert!(card_lines(0, &findings).contains("Classes Ada uses: all listed by the card"));
}

#[test]
fn the_host_lines_say_the_module_the_branch_and_each_card() {
    let text = host_lines(&host(), Branch::Known { version_tested: false });
    assert!(text.contains("615.78.08, open kernel module"), "{text}");
    assert!(text.contains("point release that has not been run"), "{text}");
    assert!(text.contains("  1: NVIDIA GeForce GTX 1660 SUPER"), "{text}");
    let closed = Host { open_module: false, ..host() };
    assert!(host_lines(&closed, Branch::Unknown).contains("NOT the open kernel module"));
    assert!(host_lines(&closed, Branch::Unknown).contains("no profile for"));
}

#[test]
fn the_channel_test_makes_this_generations_channels_and_no_other() {
    for expected in &CARDS {
        let findings = findings_on(expected.card);
        let mut device = device_on(expected.card);
        let results = run_all(
            &mut device,
            expected.generation,
            &HOST_615,
            &findings.nodes,
            Duration::ZERO,
            &mut |_| Ok(()),
        )
        .unwrap();
        let labels: Vec<&str> = results.iter().map(|result| result.label.as_str()).collect();
        assert_eq!(labels, ["graphics", "copy"], "no video channel where video does not run");
        assert!(results.iter().all(|result| result.outcome == Outcome::NotReleased));
        let made: Vec<u32> = device.rm.allocated().map(|(class, _)| class).collect();
        assert!(made.contains(&expected.generation.channel_class));
        assert!(made.contains(&expected.generation.copy_class));
        assert!(!made.contains(&BLACKWELL.channel_class));
        let text = channel_lines(&results);
        assert!(text.contains("graphics: NOT released"), "{text}");
    }
}

#[test]
fn nothing_that_identifies_the_card_is_ever_asked_for() {
    for expected in &CARDS {
        let mut device = device_on(expected.card);
        let findings = survey(&mut device).unwrap();
        run_all(
            &mut device,
            expected.generation,
            &HOST_615,
            &findings.nodes,
            Duration::ZERO,
            &mut |_| Ok(()),
        )
        .unwrap();
        for call in &device.rm.calls {
            if let Call::Control { command, .. } = call {
                assert_ne!(*command, UUID.0, "the card's UUID is never read");
                assert_ne!(*command, CONTROL_GET_GPU_ID, "the card's number in the machine is never read");
            }
        }
    }
}

/// The file is numbers, names from tables, and the card's model. A path, a user name, a host name or
/// a UUID would be a leak, and each of those has a shape this looks for.
#[test]
fn the_file_holds_no_path_user_name_host_name_or_uuid() {
    let user = std::env::var("USER").unwrap_or_default();
    let host_name = std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_owned();
    for expected in &CARDS {
        let text = whole_report(expected.card);
        assert!(!text.contains('/'), "a path or a date: {text}");
        assert!(!text.contains("GPU-"), "a UUID: {text}");
        for line in text.lines().filter(|line| line.contains("0000:")) {
            let where_it_is =
                line.starts_with("  ") && line.contains(" at ") || line.contains("opened at PCI");
            assert!(where_it_is, "a bus address only says where a card is: {line}");
        }
        if user.len() > 2 {
            assert!(!text.to_lowercase().contains(&user.to_lowercase()), "the user name: {text}");
        }
        if host_name.len() > 2 {
            assert!(!text.to_lowercase().contains(&host_name.to_lowercase()), "the host name: {text}");
        }
        for line in text.lines() {
            let long_hex = line.split(|c: char| !c.is_ascii_hexdigit()).any(|run| run.len() > 8);
            assert!(!long_hex, "a long number nothing in the tables has: {line}");
        }
    }
}

fn listed(address: &str, minor: u32, model: &str) -> ListedCard {
    ListedCard { address: PciAddress::parse(address).unwrap(), minor, model: model.to_owned() }
}

#[test]
fn a_card_whose_number_and_name_do_not_line_up_gets_no_channel() {
    let mut device = device_on(&FAKE_ADA);
    let findings = survey(&mut device).unwrap();
    let calls_before = device.rm.calls.len();
    let other = listed("0000:02:00.0", 1, "NVIDIA GeForce GTX 1660 SUPER");
    let why = refusal(&other, &findings).unwrap();
    assert!(why.contains("don't line up"), "{why}");
    assert!(why.contains("0000:02:00.0"), "{why}");
    let mut told = Vec::new();
    let stopped =
        run_on_listed_card(&mut device, &other, &findings, &HOST_615, Duration::ZERO, &mut |label| {
            told.push(label.to_owned());
            Ok(())
        });
    assert_eq!(stopped, Err(why));
    assert!(told.is_empty(), "no channel was even announced");
    assert_eq!(device.rm.calls.len(), calls_before, "nothing reached the card");
    assert!(!device.rm.allocated().any(|(class, _)| class == ADA.channel_class));

    let same = listed("0000:02:00.0", 1, FAKE_ADA.name);
    assert_eq!(refusal(&same, &findings), None);
    let ran = run_on_listed_card(&mut device, &same, &findings, &HOST_615, Duration::ZERO, &mut |label| {
        told.push(label.to_owned());
        Ok(())
    });
    assert_eq!(ran.unwrap().len(), 2);
    assert_eq!(told, ["graphics", "copy"]);
}

#[test]
fn each_channel_is_announced_before_it_is_tried_and_not_tried_if_that_fails() {
    let mut device = device_on(&FAKE_TURING);
    let findings = survey(&mut device).unwrap();
    let mut file = String::new();
    let mut progress = |label: &str| {
        file = progress_lines(label);
        if label == "graphics" { Err("the disk is full".to_owned()) } else { Ok(()) }
    };
    let stopped = run_all(&mut device, &TURING, &HOST_615, &findings.nodes, Duration::ZERO, &mut progress);
    assert_eq!(stopped, Err("the disk is full".to_owned()));
    assert!(file.contains("trying graphics"), "{file}");
    assert!(file.contains("last line"), "{file}");
    assert!(!device.rm.allocated().any(|(class, _)| class == TURING.channel_class), "never made");
}

/// The tester's PC: the 4080's device file is 0 and the 1660 SUPER's 1, but the RM set the 1660 SUPER
/// up first, so its device instance is 0 and the 4080's 1. Each card is a pretend RM of its own here,
/// as each run of the program opens one device file.
fn tester_rm(card: &FakeCard, opened_minor: u32) -> FakeRm {
    let at = |text: &str| PciAddress::parse(text).unwrap();
    let mut rm = FakeRm::with_gpus(vec![
        FakeGpu { id: 0x0100, address: at("0000:01:00.0"), minor: 0, device_instance: 1, attached: true },
        FakeGpu { id: 0x0200, address: at("0000:02:00.0"), minor: 1, device_instance: 0, attached: true },
    ]);
    install_card(&mut rm, card);
    rm.open_device_file(opened_minor);
    rm
}

#[test]
fn on_two_cards_each_is_opened_by_the_instance_the_rm_gave_its_address() {
    let host = Host {
        cards: vec![listed("0000:01:00.0", 0, FAKE_ADA.name), listed("0000:02:00.0", 1, FAKE_TURING.name)],
        ..host()
    };
    for (number, card, instance) in [(0, &FAKE_ADA, 1), (1, &FAKE_TURING, 0)] {
        let (found, listed) = host.card_numbered(number).unwrap();
        let mut device = Device::open_at(tester_rm(card, listed.minor), &HOST_615, &listed.address).unwrap();
        assert_eq!(device.instance, instance, "card {number}");
        let findings = survey(&mut device).unwrap();
        assert_eq!(refusal(listed, &findings), None, "the name check passes on the right card");
        let line = place_line(found, listed, device.instance);
        assert_eq!(
            line,
            format!(
                "Card {number} opened at PCI {}, device file nvidia{}, RM device instance {instance}\n",
                listed.address, listed.minor
            )
        );
    }
    let by_address = host.card_at(&PciAddress::parse("0000:02:00.0").unwrap()).unwrap();
    assert_eq!(by_address.0, 1);
}

#[test]
fn on_two_cards_the_device_file_number_as_the_instance_is_refused_as_the_tester_saw() {
    let mut rm = tester_rm(&FAKE_TURING, 1);
    let mut params = vec![0; HOST_615.device_params];
    params[..4].copy_from_slice(&1u32.to_le_bytes());
    let client = rm.new_client().unwrap();
    let status = rm.alloc(client, 0xC000_0001, DEVICE, &mut params).unwrap();
    assert_eq!(status, STATUS_INSUFFICIENT_PERMISSIONS, "the 1660 SUPER's file with the 4080's instance");
}

#[test]
fn one_card_is_opened_as_before() {
    let mut rm = FakeRm::new();
    install_card(&mut rm, &FAKE_ADA);
    let only = listed("0000:01:00.0", 0, FAKE_ADA.name);
    let mut device = Device::open_at(rm, &HOST_615, &only.address).unwrap();
    assert_eq!(device.instance, 0);
    let findings = survey(&mut device).unwrap();
    assert_eq!(refusal(&only, &findings), None);
}
