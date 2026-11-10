//! The fuzz seed command (`examples/fuzz_seeds.rs`) writes every value seed
//! at both of the value targets' memory bounds (`TreeCase::SMALL_MEMORY`
//! clear and set), so a run from the seeds reaches the code that grows a
//! charged buffer at the memory bound as soon as the work bound's.

#[path = "../examples/fuzz_seeds.rs"]
#[allow(dead_code, reason = "the command's `main`, which this test never runs")]
mod seeds;

use unityrs_testkit::fuzz::TreeCase;
use unityrs_testkit::scratch::Scratch;

#[test]
fn every_value_seed_is_written_at_each_memory_bound() {
    let dir = Scratch::new("fuzz-seeds");
    seeds::write(&dir).expect("the seeds are written");
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
