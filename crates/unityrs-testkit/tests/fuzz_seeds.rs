//! The fuzz seed command (`examples/fuzz_seeds.rs`) writes every value seed
//! at both of the value targets' memory bounds (`TreeCase::SMALL_MEMORY`
//! clear and set), so a run from the seeds reaches the code that grows a
//! charged buffer at the memory bound as soon as the work bound's; and every
//! `texture` seed as a `TextureCase`.

#[path = "../examples/fuzz_seeds.rs"]
#[allow(dead_code, reason = "the command's `main`, which this test never runs")]
mod seeds;

use std::collections::BTreeSet;
use std::path::Path;

use unityrs_testkit::fuzz::{TextureCase, TreeCase};
use unityrs_testkit::scratch::Scratch;

#[test]
fn every_value_seed_is_written_at_each_memory_bound() {
    let dir = Scratch::new("fuzz-seeds");
    seeds::write(&dir).expect("the seeds are written");
    texture_seeds_are_cases(&dir);
    for target in ["boost_read", "python_read", "write_after_read"] {
        let (mut small, mut large) = (Vec::new(), Vec::new());
        for entry in std::fs::read_dir(dir.join(target)).expect("the target's folder") {
            let mut bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
            let flags = bytes.first_mut().expect("a seed holds its flags");
            let at_small = *flags & TreeCase::SMALL_MEMORY != 0;
            *flags &= !TreeCase::SMALL_MEMORY;
            if at_small {
                small.push(bytes);
            } else {
                large.push(bytes);
            }
        }
        small.sort();
        large.sort();
        assert!(!large.is_empty(), "{target}: no seeds");
        assert!(
            small == large,
            "{target}: {} seeds at the small memory bound, {} at the large, not the same cases",
            small.len(),
            large.len()
        );
    }
}

/// Every `texture` seed is a `TextureCase` as written, and the seeds hold
/// each of the five formats and both meters. (Run from the test above, so
/// the seeds are written once.)
fn texture_seeds_are_cases(dir: &Path) {
    let (mut formats, mut meters) = (BTreeSet::new(), BTreeSet::new());
    for entry in std::fs::read_dir(dir.join("texture")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        formats.insert(case.format);
        meters.insert(case.meter);
    }
    assert!(
        formats.is_superset(&[3, 4, 10, 12, 25].into()),
        "{formats:?}"
    );
    assert_eq!(meters, [0, 1].into());
}
