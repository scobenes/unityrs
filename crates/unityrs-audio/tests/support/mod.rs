use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::Meter,
};

#[allow(dead_code)] // The allocation sweep constructs its own metered budget.
pub fn meter() -> Arc<Meter> {
    Meter::new(16 << 20, Arc::new(Unbounded), Reservation::Input)
}

// Format facts: version 1 base header is 60 bytes; data offsets use 32-byte units.
#[allow(dead_code)] // Shared by integration binaries; oracle replay needs only the meter.
pub fn bank(mode: u32, entries: &[(&[u8], u32, u8, u8)]) -> Vec<u8> {
    let mut headers = Vec::new();
    let mut data = Vec::new();
    for &(payload, frames, channels, rate) in entries {
        data.resize(data.len().next_multiple_of(32), 0);
        let word = u64::from(rate) << 1
            | match channels {
                1 => 0,
                2 => 1,
                6 => 2,
                8 => 3,
                _ => panic!("channels"),
            } << 5
            | (data.len() as u64 / 32) << 7
            | u64::from(frames) << 34;
        headers.extend_from_slice(&word.to_le_bytes());
        data.extend_from_slice(payload);
    }
    let mut out = b"FSB5".to_vec();
    for n in [
        1,
        u32::try_from(entries.len()).unwrap(),
        u32::try_from(headers.len()).unwrap(),
        0,
        u32::try_from(data.len()).unwrap(),
        mode,
    ] {
        out.extend_from_slice(&n.to_le_bytes());
    }
    out.resize(60, 0);
    out.extend(headers);
    out.extend(data);
    out
}
