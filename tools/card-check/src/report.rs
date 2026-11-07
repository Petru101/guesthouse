//! The text this program prints and writes: what was found, in a form that's safe to post. Every
//! line here comes from a named field, and none of them identifies the card or the PC.

use crate::channels::{ChannelResult, Outcome};
use crate::host::Host;
use crate::survey::{Findings, Verdict};
use guesthouse_backend_nvidia::answers::nodes::engine_label;

pub const REPORT_VERSION: u32 = 1;
const CLASSES_PER_LINE: usize = 8;

/// What this build knows of the host driver's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    /// A profile exists, and this exact version has been run or not.
    Known {
        version_tested: bool,
    },
    Unknown,
}

fn line(text: &mut String, words: impl AsRef<str>) {
    text.push_str(words.as_ref());
    text.push('\n');
}

pub fn heading() -> String {
    format!("Guesthouse card check, version {REPORT_VERSION}\n")
}

/// The driver and the cards, before any is opened.
pub fn host_lines(host: &Host, branch: Branch) -> String {
    let mut text = String::new();
    let module = if host.open_module { "open kernel module" } else { "NOT the open kernel module" };
    line(&mut text, format!("Host driver: {}, {module}", host.version));
    let known = match branch {
        Branch::Known { version_tested: true } => "a branch this build knows, at a version that has been run",
        Branch::Known { version_tested: false } => {
            "a branch this build knows, at a point release that has not been run"
        }
        Branch::Unknown => "a branch this build has no profile for",
    };
    line(&mut text, format!("Driver branch: {known}"));
    line(&mut text, format!("Cards: {}", host.cards.len()));
    for card in &host.cards {
        line(&mut text, format!("  {}: {}", card.minor, card.model));
    }
    text
}

fn verdict_lines(findings: &Findings) -> String {
    match &findings.verdict {
        Verdict::Supported => "Verdict: supported. This card has been run.\n".to_owned(),
        Verdict::Experimental { generation, ever_run: true } => {
            format!("Verdict: experimental. {generation} has run a guest, but this card has not.\n")
        }
        Verdict::Experimental { generation, ever_run: false } => format!(
            "Verdict: experimental. {generation} has never run a guest. The decoder and encoder engines \
             listed above are what fills in the node types still unset for it.\n"
        ),
        Verdict::Refused(reason) => format!("Verdict: refused. {reason}\n"),
    }
}

fn class_lines(classes: &[u32]) -> String {
    let mut text = format!("Classes: {}\n", classes.len());
    let names: Vec<String> = classes.iter().map(|class| format!("{class:04X}")).collect();
    for chunk in names.chunks(CLASSES_PER_LINE) {
        line(&mut text, format!("  {}", chunk.join(" ")));
    }
    text
}

/// The classes the backend would make on this generation, and whether the card lists each.
fn generation_class_lines(findings: &Findings) -> String {
    let Some(generation) = findings.generation else { return String::new() };
    let mut wanted = vec![generation.channel_class, generation.copy_class, generation.usermode_class];
    wanted.extend(generation.graphics_classes.iter().map(|class| class.0));
    if generation.nodes.video_runs {
        wanted.extend([generation.video_decode_class, generation.video_encode_class]);
    }
    let mut missing: Vec<u32> = Vec::new();
    for class in wanted {
        if !findings.classes.contains(&class) && !missing.contains(&class) {
            missing.push(class);
        }
    }
    let missing: Vec<String> = missing.iter().map(|class| format!("{class:04X}")).collect();
    if missing.is_empty() {
        format!("Classes {} uses: all listed by the card\n", generation.name)
    } else {
        format!("Classes {} uses but the card does not list: {}\n", generation.name, missing.join(" "))
    }
}

/// What the read-only part found on one card.
pub fn card_lines(minor: u32, findings: &Findings) -> String {
    let mut text = String::new();
    line(&mut text, format!("Card {minor}: {}", findings.name));
    line(&mut text, format!("Device ID: {:#06X}, revision {:#04X}", findings.device, findings.revision));
    line(
        &mut text,
        format!("Architecture: {:#X}, implementation {:#X}", findings.architecture, findings.implementation),
    );
    let generation = findings.generation.map_or("none in this build", |generation| generation.name);
    line(&mut text, format!("Generation: {generation}"));
    let shader_set = findings.shader_set.map_or("none for this SM".to_owned(), |set| format!("{set:#X}"));
    line(&mut text, format!("SM version: {:#X}, shader set {shader_set}", findings.sm_version));
    line(&mut text, format!("Engines from the host: {}", findings.engines.len()));
    for &engine in &findings.engines {
        line(&mut text, format!("  {engine:#04x} {}", engine_label(engine)));
    }
    let graphics_copy =
        findings.graphics_copy.map_or("none named".to_owned(), |engine| format!("{engine:#x}"));
    line(&mut text, format!("Copy engine paired with graphics: {graphics_copy}"));
    line(&mut text, format!("Copy engines: {}", findings.copy_engines.len()));
    for copy in &findings.copy_engines {
        line(
            &mut text,
            format!("  {:#04x}: caps {:#x}, physical engines {:#x}", copy.engine, copy.caps, copy.physical),
        );
    }
    if findings.generation.is_some() {
        line(&mut text, format!("Nodes a guest would see: {}", findings.nodes.len()));
        for (ordinal, node) in findings.nodes.iter().enumerate() {
            let entry = format!(
                "  {ordinal}: type {:#04X}, engine {}, scheduled {}, in flight {}, copy engine {:#x} {}",
                node.kind,
                node.engine,
                u32::from(node.scheduled),
                node.in_flight,
                node.copy_engine,
                node.name
            );
            line(&mut text, entry.trim_end());
        }
    }
    text.push_str(&class_lines(&findings.classes));
    text.push_str(&generation_class_lines(findings));
    text.push_str(&verdict_lines(findings));
    text
}

/// What the channel test did, or why it didn't run.
pub fn channel_lines(results: &[ChannelResult]) -> String {
    let mut text = String::from("Channel test:\n");
    for result in results {
        let outcome = match &result.outcome {
            Outcome::Released(took) => format!("released after {} us", took.as_micros()),
            Outcome::NotReleased => "NOT released".to_owned(),
            Outcome::NoChannel(why) => format!("no channel: {why}"),
        };
        line(&mut text, format!("  {}: {outcome}", result.label));
    }
    text
}
