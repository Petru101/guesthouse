//! The adapter query answers that follow from a few facts about the card.
//!
//! Which flags are set is what a real install reported, and only the speed and identity fields follow
//! the card.

const QUERY_UPDATE_STATUS: u32 = 11;
const QUERY_ADAPTER_TYPE: u32 = 15;
const QUERY_DRIVER_CAPS_1_2: u32 = 17;
const QUERY_DRIVER_CAPS_1_3: u32 = 22;
const QUERY_DRIVER_CAPS_2_0: u32 = 24;
const QUERY_UPDATE_STATUS_RENDER: u32 = 55;
const QUERY_UNIQUE_ID: u32 = 60;
pub(super) const QUERY_NODE_SPEED: u32 = 61;
pub(super) const QUERY_ADAPTER_SPEED: u32 = 62;
const QUERY_DRIVER_CAPS_2_9: u32 = 75;
const QUERY_CROSS_ADAPTER: u32 = 76;

const ADAPTER_RENDER: u32 = 1 << 0;
const ADAPTER_DISPLAY: u32 = 1 << 1;
const ADAPTER_POST_DEVICE: u32 = 1 << 3;
const ADAPTER_ACG: u32 = 1 << 8;
const ADAPTER_TIMINGS_FROM_MODE: u32 = 1 << 9;
const ADAPTER_RUNTIME_POWER: u32 = 1 << 13;

const GRAPHICS_PREEMPTION_PIXEL: u32 = 400;
const COMPUTE_PREEMPTION_DISPATCH: u32 = 200;
const CAPS_1_2: u32 = 1 << 0 | 1 << 1 | 1 << 2 | 1 << 3 | 1 << 4 | 1 << 6 | 1 << 7 | 1 << 8;
const CAPS_1_3: u32 = 1 << 4 | 1 << 5;
const CAPS_2_0: u32 = 1 << 0 | 1 << 1 | 1 << 3 | 1 << 4;
const FEATURE_STABLE: u32 = 2;
const HARDWARE_SCHEDULING_ON: u32 = 1 << 2;
const CROSS_ADAPTER_COPY: u32 = 1;
const UNIQUE_ID_SIZE: usize = 80;

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn node_speed(frequency: u64) -> Vec<u8> {
    let mut out = vec![0; 8];
    for value in [frequency, frequency, frequency] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[0; 12 + 4 + 8]);
    out
}

fn adapter_speed(frequency: u64) -> Vec<u8> {
    let mut out = vec![0; 8];
    for value in [frequency, frequency, frequency, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[0; 12 + 4]);
    out
}

fn unique_id(guid: &str) -> Vec<u8> {
    let mut out: Vec<u8> = guid.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.resize(out.len().max(UNIQUE_ID_SIZE), 0);
    out
}

/// Every answer that needs no more than this, by query number.
pub fn fixed_answers(guid: &str, graphics_hertz: u64, memory_hertz: u64) -> Vec<(u32, Vec<u8>)> {
    let adapter_type = ADAPTER_RENDER
        | ADAPTER_DISPLAY
        | ADAPTER_POST_DEVICE
        | ADAPTER_ACG
        | ADAPTER_TIMINGS_FROM_MODE
        | ADAPTER_RUNTIME_POWER;
    vec![
        (QUERY_UPDATE_STATUS, vec![0; 4]),
        (QUERY_ADAPTER_TYPE, words(&[adapter_type])),
        (QUERY_DRIVER_CAPS_1_2, words(&[GRAPHICS_PREEMPTION_PIXEL, COMPUTE_PREEMPTION_DISPATCH, CAPS_1_2])),
        (QUERY_DRIVER_CAPS_1_3, words(&[CAPS_1_3])),
        (QUERY_DRIVER_CAPS_2_0, words(&[CAPS_2_0])),
        (QUERY_UPDATE_STATUS_RENDER, vec![0; 4]),
        (QUERY_UNIQUE_ID, unique_id(guid)),
        (QUERY_NODE_SPEED, node_speed(graphics_hertz)),
        (QUERY_ADAPTER_SPEED, adapter_speed(memory_hertz)),
        (QUERY_DRIVER_CAPS_2_9, words(&[FEATURE_STABLE | HARDWARE_SCHEDULING_ON])),
        (QUERY_CROSS_ADAPTER, words(&[CROSS_ADAPTER_COPY])),
    ]
}
