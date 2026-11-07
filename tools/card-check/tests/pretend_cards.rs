//! The program on the pretend Ada and Turing cards: what it finds, what it says, and what never
//! reaches the file. Nothing here goes near a card.

use guesthouse_backend_nvidia::profile::{ADA, BLACKWELL, Generation, HOST_615, TURING};
use guesthouse_backend_nvidia::record::card::UUID;
use guesthouse_backend_nvidia::rm::classes::CONTROL_GET_GPU_ID;
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::fake::{Call, FakeRm};
use guesthouse_backend_nvidia::rm::fake_card::{
    FAKE_ADA, FAKE_BLACKWELL, FAKE_TURING, FakeCard, install_card,
};
use guesthouse_card_check::channels::{Outcome, run_all};
use guesthouse_card_check::host::{CardListed, Host};
use guesthouse_card_check::report::{Branch, card_lines, channel_lines, heading, host_lines};
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

const CARDS: [Expected; 2] = [
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
            CardListed { minor: 0, model: "NVIDIA GeForce RTX 4080".to_owned() },
            CardListed { minor: 1, model: "NVIDIA GeForce GTX 1660 SUPER".to_owned() },
        ],
    }
}

/// The whole file, as the program would write it after a channel test.
fn whole_report(card: &FakeCard) -> String {
    let findings = findings_on(card);
    let mut device = device_on(card);
    let generation = findings.generation.unwrap();
    let results = run_all(&mut device, generation, &HOST_615, &findings.nodes, Duration::ZERO).unwrap();
    let mut text = heading();
    text.push_str(&host_lines(&host(), Branch::Known { version_tested: true }));
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
    let unknown = FakeCard { architecture: 0x170, ..FAKE_ADA };
    let findings = findings_on(&unknown);
    assert_eq!(findings.generation, None);
    assert!(findings.nodes.is_empty());
    let Verdict::Refused(reason) = &findings.verdict else { panic!("{:?}", findings.verdict) };
    assert!(reason.contains("0x170"), "{reason}");
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
        let results =
            run_all(&mut device, expected.generation, &HOST_615, &findings.nodes, Duration::ZERO).unwrap();
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
        run_all(&mut device, expected.generation, &HOST_615, &findings.nodes, Duration::ZERO).unwrap();
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
        assert!(!text.contains("0000:"), "a bus address: {text}");
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
