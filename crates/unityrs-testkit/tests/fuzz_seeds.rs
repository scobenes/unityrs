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
    fsb5_seeds_cover_formats_versions_and_hostile_headers(&dir);
    image_writer_seeds_cover_the_harness_layout(&dir);
    texture_seeds_are_cases(&dir);
    astc_seeds_are_cases(&dir);
    crunch_seeds_are_cases(&dir);
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

/// Valid CRN headers for both base formats are retained at every meter;
/// malformed seeds remain canonical cases too.
fn crunch_seeds_are_cases(dir: &Path) {
    let (mut formats, mut meters, mut valid) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in std::fs::read_dir(dir.join("crunch")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        assert!([28, 29].contains(&case.format));
        formats.insert(case.format);
        meters.insert(case.meter);
        if case.data.starts_with(b"Hx") && case.data.len() >= 62 {
            valid.insert((case.format, case.meter));
        }
    }
    assert_eq!(formats, [28, 29].into());
    assert_eq!(meters, (0..4).collect());
    assert_eq!(valid.len(), 8);
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
        formats.is_superset(&[3, 4, 10, 12, 25, 30, 31, 32, 33, 35, 36].into()),
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

fn fsb5_seeds_cover_formats_versions_and_hostile_headers(dir: &Path) {
    let mut formats = BTreeSet::new();
    let mut count = 0;
    let mut hostile_count = false;
    for entry in std::fs::read_dir(dir.join("fsb5")).expect("the fsb5 target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        count += 1;
        if bytes.starts_with(b"FSB5") && bytes.len() >= 28 {
            let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            hostile_count |= word(8) == u32::MAX;
            if word(4) <= 1 && word(8) == 1 && (1..=5).contains(&word(24)) {
                formats.insert((word(4), word(24)));
            }
        }
    }
    assert_eq!(count, 54);
    assert!(hostile_count);
    assert_eq!(
        formats,
        (0..=1).flat_map(|v| (1..=5).map(move |m| (v, m))).collect()
    );
}

/// Check the serialized controls, including every format/mode/palette's raw
/// dimensions and pixel-count neighbours. These are consumed without a decoder
/// by the harness, so a shifted header could silently lose a whole writer.
fn image_writer_seeds_cover_the_harness_layout(dir: &Path) {
    let mut sizes = BTreeSet::new();
    let mut pixel_lengths = BTreeSet::new();
    let mut policies = BTreeSet::new();
    let mut budgets = BTreeSet::new();
    let mut filenames = BTreeSet::new();
    let mut headers = BTreeSet::new();
    let mut count = 0;
    for entry in std::fs::read_dir(dir.join("image_writers")).expect("the writer target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a writer seed");
        count += 1;
        if bytes.len() < 13 {
            assert!(bytes.iter().all(|&b| b == 0));
            headers.insert(bytes.len());
            continue;
        }
        let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (width, height) = (word(0), word(4));
        let (mode, format, policy, budget, filename) =
            (bytes[8], bytes[9], bytes[10], bytes[11], bytes[12]);
        assert!(mode < 5 && format < 19);
        assert!(budget < 4 && filename < 4);
        assert!(bytes.len() <= 13 + 768, "seed pixel sources stay small");
        let palette = policy & 4 != 0;
        assert!(!palette || mode == 4);
        assert!([0, 1, 2, 4, 5, 12].contains(&policy));
        let key = (format, mode, palette);
        policies.insert((key, policy % 3));
        budgets.insert((key, budget));
        filenames.insert((key, filename));
        if policy % 3 == 0 {
            sizes.insert((key, width, height));
            if (width, height) == (2, 1) {
                pixel_lengths.insert((key, bytes.len() - 13));
            }
        }
    }
    let expected_sizes = [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (2, 1),
        (3, 2),
        (15, 1),
        (16, 1),
        (17, 1),
        (255, 1),
        (256, 1),
        (65534, 1),
        (65535, 1),
        (65536, 1),
        (i32::MAX as u32, 1),
        (u32::MAX, 1),
        (u32::MAX, u32::MAX),
        (256, 256),
        (1, 65536),
    ];
    for format in 0..19 {
        for (mode, channels) in (0_u8..).zip([1, 2, 3, 4, 1]) {
            for palette in [false, true].into_iter().filter(|p| !p || mode == 4) {
                let key = (format, mode, palette);
                for (width, height) in expected_sizes {
                    assert!(
                        sizes.contains(&(key, width, height)),
                        "{key:?} {width}x{height}"
                    );
                }
                for len in [2 * channels - 1, 2 * channels, 2 * channels + 1] {
                    assert!(
                        pixel_lengths.contains(&(key, len)),
                        "{key:?} pixel count {len}"
                    );
                }
                for policy in 0..3 {
                    assert!(policies.contains(&(key, policy)), "{key:?} policy {policy}");
                }
                for selector in 0..4 {
                    assert!(
                        budgets.contains(&(key, selector)),
                        "{key:?} budget {selector}"
                    );
                    assert!(
                        filenames.contains(&(key, selector)),
                        "{key:?} filename {selector}"
                    );
                }
            }
        }
    }
    assert_eq!(headers, (0..13).collect());
    assert_eq!(count, 2749);
}
