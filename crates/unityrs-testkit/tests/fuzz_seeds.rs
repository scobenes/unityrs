//! The fuzz seed command (`examples/fuzz_seeds.rs`) writes every value seed
//! at both of the value targets' memory bounds (`TreeCase::SMALL_MEMORY`
//! clear and set), so a run from the seeds reaches the code that grows a
//! charged buffer at the memory bound as soon as the work bound's; every
//! `texture` and `astc` seed as a `TextureCase`, the `astc` seeds of each of
//! the twelve ASTC LDR formats; and every `meshdecode` seed as a
//! `MeshCase`.

#[path = "../examples/fuzz_seeds.rs"]
#[allow(dead_code, reason = "the command's `main`, which this test never runs")]
mod seeds;

use std::collections::BTreeSet;
use std::path::Path;

use unityrs_testkit::fuzz::{MeshCase, TextureCase, TreeCase};
use unityrs_testkit::scratch::Scratch;

#[test]
fn every_value_seed_is_written_at_each_memory_bound() {
    let dir = Scratch::new("fuzz-seeds");
    seeds::write(&dir).expect("the seeds are written");
    texture_seeds_are_cases(&dir);
    astc_seeds_are_cases(&dir);
    mesh_seeds_are_cases(&dir);
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

/// Every `astc` seed is a `TextureCase` as written, of an ASTC LDR format,
/// and each of the twelve has seeds of whole blocks.
fn astc_seeds_are_cases(dir: &Path) {
    let mut formats = BTreeSet::new();
    for entry in std::fs::read_dir(dir.join("astc")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        assert!((48..60).contains(&case.format), "{}", case.format);
        if case.data.len().is_multiple_of(16) && !case.data.is_empty() {
            formats.insert(case.format);
        }
    }
    assert_eq!(formats, (48..60).collect());
}

/// Every `texture` seed is a `TextureCase` as written, and the seeds hold
/// each of the five formats and both meters, and a whole 4x4 block, under
/// the memory bound, down each decoder path: every BC7 mode and none (a
/// zero first byte), DXT1 with `c0 > c1` and not, DXT5 with `a0 > a1` and
/// not. (Run from the test above, so the seeds are written once.)
fn texture_seeds_are_cases(dir: &Path) {
    let (mut formats, mut meters) = (BTreeSet::new(), BTreeSet::new());
    let (mut bc7_modes, mut dxt1, mut dxt5) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in std::fs::read_dir(dir.join("texture")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        formats.insert(case.format);
        meters.insert(case.meter);
        if (case.width, case.height, case.meter) != (4, 4, 0) {
            continue;
        }
        match (case.format, case.data.as_slice()) {
            (25, [first, ..]) if case.data.len() == 16 => {
                bc7_modes.insert(first.trailing_zeros());
            }
            (10, [c0, c0_hi, c1, c1_hi, ..]) if case.data.len() == 8 => {
                dxt1.insert(u16::from_le_bytes([*c0, *c0_hi]) > u16::from_le_bytes([*c1, *c1_hi]));
            }
            (12, [a0, a1, ..]) if case.data.len() == 16 => {
                dxt5.insert(a0 > a1);
            }
            _ => {}
        }
    }
    assert!(
        formats.is_superset(&[3, 4, 10, 12, 25].into()),
        "{formats:?}"
    );
    assert_eq!(meters, [0, 1].into());
    assert_eq!(bc7_modes, (0..=8).collect(), "BC7 modes");
    assert_eq!(dxt1, [false, true].into(), "DXT1 c0 > c1");
    assert_eq!(dxt5, [false, true].into(), "DXT5 a0 > a1");
}

/// Every `meshdecode` seed is a `MeshCase` as written, and the seeds hold
/// meshes and sprites, both byte orders, a reader and none, versions on each
/// side of the handler's boundaries (below 4, 4, 5, 2017, 2018, 2019 and
/// later), and every meter at each case. (Run from the test above.)
fn mesh_seeds_are_cases(dir: &Path) {
    let (mut flags, mut majors) = (BTreeSet::new(), BTreeSet::new());
    let mut per_meter: [Vec<Vec<u8>>; 4] = Default::default();
    for entry in std::fs::read_dir(dir.join("meshdecode")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let mut case = MeshCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        assert!(!case.value.rows.is_empty() && !case.value.data.is_empty());
        for bit in [MeshCase::SPRITE, MeshCase::BIG_ENDIAN, MeshCase::RESOURCES] {
            flags.insert((bit, case.has(bit)));
        }
        majors.insert(case.version[0]);
        let m = seeds::MESH_METERS
            .iter()
            .position(|m| *m == case.meter)
            .expect("a seed's meter is one of the command's");
        case.meter = 0;
        per_meter[m].push(case.encode());
    }
    assert_eq!(flags.len(), 6, "{flags:?}");
    assert!(
        majors.is_superset(&[3, 4, 5, 2017, 2018, 2019, 6000].into()),
        "{majors:?}"
    );
    for m in &mut per_meter {
        m.sort();
    }
    assert!(per_meter[0].len() > 300);
    assert!(
        per_meter.iter().all(|m| *m == per_meter[0]),
        "each meter at each case"
    );
}
