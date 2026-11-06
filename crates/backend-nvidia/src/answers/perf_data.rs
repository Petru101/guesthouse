//! The guest's performance queries, answered from the host's latest readings.
//!
//! The layouts are Microsoft's, in its public `d3dkmthk.h`. These are readings of the whole card,
//! which every VM on it shares. The fan comes as a percentage here and the query wants revolutions,
//! so that field stays empty, and no voltage is available to report.

use super::queries::{QUERY_ADAPTER_SPEED, QUERY_NODE_SPEED};
use crate::sensors::Snapshot;
use guesthouse_backend::{Status, VendorQuery};

const WHAT_THE_GUEST_SENT: usize = 8;

const NODE_FREQUENCY: usize = 0x08;
const NODE_MOST_FREQUENCY: usize = 0x10;
const NODE_MOST_OVERCLOCKED: usize = 0x18;
const NODE_SMALLEST: usize = 0x38;

const ADAPTER_MEMORY_FREQUENCY: usize = 0x08;
const ADAPTER_MOST_MEMORY_FREQUENCY: usize = 0x10;
const ADAPTER_MOST_OVERCLOCKED: usize = 0x18;
const ADAPTER_POWER: usize = 0x34;
const ADAPTER_TEMPERATURE: usize = 0x38;
const ADAPTER_POWERED_ON: usize = 0x3C;
const ADAPTER_SMALLEST: usize = 0x3D;

const HERTZ_PER_MEGAHERTZ: u64 = 1_000_000;
/// Power is in tenths of a percent of what the card may draw, and temperature in tenths of a degree.
const POWER_SCALE: u64 = 1000;
const TEMPERATURE_SCALE: u32 = 10;

fn put_long(answer: &mut [u8], at: usize, value: u64) {
    answer[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn put_word(answer: &mut [u8], at: usize, value: u32) {
    answer[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn hertz(megahertz: Option<u32>) -> Option<u64> {
    megahertz.map(|megahertz| u64::from(megahertz) * HERTZ_PER_MEGAHERTZ)
}

/// A blank answer of the size asked for, holding the node or adapter number the guest sent.
fn blank(query: &VendorQuery<'_>) -> Vec<u8> {
    let mut answer = vec![0; query.size];
    let sent = query.data.len().min(WHAT_THE_GUEST_SENT);
    answer[..sent].copy_from_slice(&query.data[..sent]);
    answer
}

fn node_answer(query: &VendorQuery<'_>, live: &Snapshot) -> Vec<u8> {
    let mut answer = blank(query);
    let frequency = hertz(live.graphics_megahertz).unwrap_or(0);
    let most = hertz(live.graphics_most_megahertz).unwrap_or(frequency);
    put_long(&mut answer, NODE_FREQUENCY, frequency);
    put_long(&mut answer, NODE_MOST_FREQUENCY, most);
    put_long(&mut answer, NODE_MOST_OVERCLOCKED, most);
    answer
}

fn power_share(live: &Snapshot) -> Option<u32> {
    let limit = u64::from(live.power_limit_milliwatts.filter(|&limit| limit != 0)?);
    Some((u64::from(live.power_milliwatts?) * POWER_SCALE / limit) as u32)
}

fn adapter_answer(query: &VendorQuery<'_>, live: &Snapshot) -> Vec<u8> {
    let mut answer = blank(query);
    let frequency = hertz(live.memory_megahertz).unwrap_or(0);
    let most = hertz(live.memory_most_megahertz).unwrap_or(frequency);
    put_long(&mut answer, ADAPTER_MEMORY_FREQUENCY, frequency);
    put_long(&mut answer, ADAPTER_MOST_MEMORY_FREQUENCY, most);
    put_long(&mut answer, ADAPTER_MOST_OVERCLOCKED, most);
    put_word(&mut answer, ADAPTER_POWER, power_share(live).unwrap_or(0));
    let temperature = live.temperature_celsius.unwrap_or(0).saturating_mul(TEMPERATURE_SCALE);
    put_word(&mut answer, ADAPTER_TEMPERATURE, temperature);
    answer[ADAPTER_POWERED_ON] = 1;
    answer
}

/// Answers from `live` if this is one of the two queries. `None` leaves it to the usual answers.
pub fn answer(query: &VendorQuery<'_>, live: &Snapshot, out: &mut Vec<u8>) -> Option<Status> {
    let answer = match query.kind {
        QUERY_NODE_SPEED if query.size >= NODE_SMALLEST => node_answer(query, live),
        QUERY_ADAPTER_SPEED if query.size >= ADAPTER_SMALLEST => adapter_answer(query, live),
        _ => return None,
    };
    out.extend_from_slice(&answer);
    Some(Status::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADAPTER_SIZE: usize = 64;
    const NODE_SIZE: usize = 56;

    fn live() -> Snapshot {
        Snapshot {
            temperature_celsius: Some(40),
            power_milliwatts: Some(60_000),
            power_limit_milliwatts: Some(600_000),
            graphics_megahertz: Some(2000),
            graphics_most_megahertz: Some(3000),
            memory_megahertz: Some(14_000),
            memory_most_megahertz: Some(14_500),
            ..Snapshot::default()
        }
    }

    fn query<'a>(kind: u32, size: usize, data: &'a [u8]) -> VendorQuery<'a> {
        VendorQuery { kind, size, data, process_is_32_bit: false }
    }

    fn long(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    fn word(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_node_answer_says_which_node_and_how_fast_in_hertz() {
        let mut out = Vec::new();
        let asked = query(QUERY_NODE_SPEED, NODE_SIZE, &[3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(answer(&asked, &live(), &mut out), Some(Status::SUCCESS));
        assert_eq!(out.len(), NODE_SIZE);
        assert_eq!(out[0], 3);
        assert_eq!(long(&out, NODE_FREQUENCY), 2_000_000_000);
        assert_eq!(long(&out, NODE_MOST_FREQUENCY), 3_000_000_000);
    }

    #[test]
    fn an_adapter_answer_is_in_the_units_the_layout_says() {
        let mut out = Vec::new();
        answer(&query(QUERY_ADAPTER_SPEED, ADAPTER_SIZE, &[0; 8]), &live(), &mut out);
        assert_eq!(out.len(), ADAPTER_SIZE);
        assert_eq!(long(&out, ADAPTER_MEMORY_FREQUENCY), 14_000_000_000);
        assert_eq!(long(&out, ADAPTER_MOST_MEMORY_FREQUENCY), 14_500_000_000);
        assert_eq!(word(&out, ADAPTER_TEMPERATURE), 400);
        assert_eq!(word(&out, ADAPTER_POWER), 100, "60 W of 600 W is 10.0%");
        assert_eq!(out[ADAPTER_POWERED_ON], 1);
    }

    #[test]
    fn a_power_limit_of_zero_is_no_reading() {
        let snapshot = Snapshot { power_limit_milliwatts: Some(0), ..live() };
        assert_eq!(power_share(&snapshot), None);
    }

    #[test]
    fn a_larger_answer_keeps_the_size_asked_for() {
        let mut out = Vec::new();
        answer(&query(QUERY_NODE_SPEED, 64, &[0; 8]), &live(), &mut out);
        assert_eq!(out.len(), 64);
    }

    #[test]
    fn a_query_too_small_for_the_layout_is_left_alone() {
        let mut out = Vec::new();
        assert_eq!(answer(&query(QUERY_ADAPTER_SPEED, 8, &[]), &live(), &mut out), None);
        assert!(out.is_empty());
    }

    #[test]
    fn other_queries_are_left_alone() {
        assert_eq!(answer(&query(0, 64, &[]), &live(), &mut Vec::new()), None);
    }
}
