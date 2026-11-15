//! Raw FSB5/AudioClip input: validate and decode under bounded work and memory,
//! compare repeated results, check plan lengths and return every charge.
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::Meter,
};

/// A panic, output differing between calls, output exceeding its plan, or a
/// leaked charge is a fuzz finding. FMOD is never loaded by this target.
pub fn check(input: &[u8]) {
    let meter = Meter::with_decode_work(
        crate::MEMORY,
        crate::WORK,
        Arc::new(Unbounded),
        Reservation::Input,
    );
    if let Ok(plan) = unityrs_audio::plan(input, true, &meter) {
        if let Ok(first) = unityrs_audio::decode(&plan, b"fuzz", &meter) {
            assert_eq!(
                first
                    .as_slice()
                    .iter()
                    .map(|s| s.bytes.len() as u64)
                    .sum::<u64>(),
                plan.output_bytes()
            );
            if let Ok(second) = unityrs_audio::decode(&plan, b"fuzz", &meter) {
                assert_eq!(first.len(), second.len());
                for (a, b) in first.as_slice().iter().zip(second.as_slice()) {
                    assert_eq!(a.name.as_slice(), b.name.as_slice());
                    assert_eq!(a.bytes.as_slice(), b.bytes.as_slice());
                }
            }
        }
    }
    assert_eq!(meter.used(), 0);
    let small = Meter::with_decode_work(256, crate::WORK, Arc::new(Unbounded), Reservation::Input);
    if let Ok(plan) = unityrs_audio::plan(input, false, &small) {
        let _ = unityrs_audio::decode(&plan, b"fuzz", &small);
    }
    assert_eq!(small.used(), 0);
}

#[cfg(test)]
mod tests {
    #[test]
    fn synthetic_seeds_reach_pcm_adpcm_and_malformed_paths() {
        unityrs_testkit::fsb5::seeds(|_, input| {
            super::check(input);
            for end in 0..input.len() {
                super::check(&input[..end]);
            }
            Ok(())
        })
        .expect("synthetic audio seeds");
    }
}
