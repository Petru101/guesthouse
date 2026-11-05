//! What a replay found.

use crate::answers::Provenance;
use std::collections::HashMap;
use std::fmt;

/// The first reply that differed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// Which incoming message, counting from zero.
    pub message_index: usize,
    pub command: String,
    pub transaction: u64,
    pub offset: usize,
    pub reason: String,
    pub expected_bytes: Vec<u8>,
    pub actual_bytes: Vec<u8>,
}

/// Counts for one replay.
#[derive(Debug, Default)]
pub struct Report {
    pub messages: usize,
    /// Replies the core worked out alone, all identical.
    pub worked_identical: usize,
    /// Replies that match and hold vendor fields taken from the recording. The rest of each was
    /// really compared.
    pub partly_recorded_identical: usize,
    /// Replies the recording supplied whole. Matching proves nothing here.
    pub recorded_whole: usize,
    pub different: usize,
    /// Messages wanting a reply that the recording has none for.
    pub no_recorded_reply: usize,
    pub first_difference: Option<Difference>,
    /// How many of each message the host sent by itself, as (recorded, replayed) where they differ.
    pub notices_missing: usize,
    pub notices_extra: usize,
    pub notices_matched: usize,
    pub by_command: HashMap<String, usize>,
}

impl Report {
    pub fn count(&mut self, provenance: Provenance) {
        match provenance {
            Provenance::Worked => self.worked_identical += 1,
            Provenance::PartlyRecorded => self.partly_recorded_identical += 1,
            Provenance::Recorded => self.recorded_whole += 1,
        }
    }

    pub fn passed(&self) -> bool {
        self.different == 0 && self.notices_missing == 0 && self.notices_extra == 0
    }
}

const SHOWN_BYTES: usize = 32;

fn hex_around(bytes: &[u8], offset: usize) -> String {
    let start = offset.saturating_sub(SHOWN_BYTES / 2) & !0xF;
    let shown = &bytes[start.min(bytes.len())..(start + SHOWN_BYTES).min(bytes.len())];
    let text: Vec<String> = shown.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("+{start:#x}: {}", text.join(" "))
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} messages replayed", self.messages)?;
        writeln!(f, "  {} replies identical, worked out by the core alone", self.worked_identical)?;
        writeln!(
            f,
            "  {} replies identical, with vendor fields taken from the recording",
            self.partly_recorded_identical
        )?;
        writeln!(f, "  {} replies supplied whole by the recording: not evidence", self.recorded_whole)?;
        writeln!(f, "  {} replies different", self.different)?;
        writeln!(f, "  {} messages have no recorded reply", self.no_recorded_reply)?;
        writeln!(
            f,
            "Messages the host sends by itself: {} matched, {} missing, {} extra (order is not compared)",
            self.notices_matched, self.notices_missing, self.notices_extra
        )?;
        if let Some(first) = &self.first_difference {
            writeln!(
                f,
                "First difference: message {} ({}), transaction {:#x}",
                first.message_index, first.command, first.transaction
            )?;
            writeln!(f, "  {}", first.reason)?;
            writeln!(f, "  recording {}", hex_around(&first.expected_bytes, first.offset))?;
            writeln!(f, "  replay    {}", hex_around(&first.actual_bytes, first.offset))?;
        }
        Ok(())
    }
}
