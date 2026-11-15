//! The fuzz seed command (`examples/fuzz_seeds.rs`) writes every value seed
//! at both of the value targets' memory bounds (`TreeCase::SMALL_MEMORY`
//! clear and set), so a run from the seeds reaches the code that grows a
//! charged buffer at the memory bound as soon as the work bound's; every
//! `texture` and `astc` seed as a `TextureCase`, the `astc` seeds of each of
//! the twelve ASTC LDR formats; and every `meshdecode` seed as a
//! `MeshCase`, plus Shader program blobs as `ShaderCase`.

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
    // One count per target (`TARGETS` sets the array's length): each wrote seeds.
    let counts = seeds::write(&dir).expect("the seeds are written");
    assert_eq!(counts.len(), 21);
    assert!(counts.iter().all(|n| *n > 0), "{counts:?}");
    fsb5_seeds_cover_formats_versions_and_hostile_headers(&dir);
    head_seeds_reach_the_new_readers(&dir);
    texture_seeds_are_cases(&dir);
    astc_seeds_are_cases(&dir);
    crunch_seeds_are_cases(&dir);
    mesh_seeds_are_cases(&dir);
    scene_seeds_are_cases(&dir);
    shader_seeds_are_cases(&dir);
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

/// Valid CRN headers for legacy DXT, Unity DXT (128/129 in the harness),
/// and Unity ETC are retained at every meter;
/// malformed seeds remain canonical cases too.
fn crunch_seeds_are_cases(dir: &Path) {
    let (mut formats, mut meters, mut valid) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in std::fs::read_dir(dir.join("crunch")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        assert!([28, 29, 64, 65, 128, 129].contains(&case.format));
        formats.insert(case.format);
        meters.insert(case.meter);
        if case.data.starts_with(b"Hx") && case.data.len() >= 62 {
            valid.insert((case.format, case.meter));
        }
    }
    assert_eq!(formats, [28, 29, 64, 65, 128, 129].into());
    assert_eq!(meters, (0..4).collect());
    assert_eq!(valid.len(), 24);
}

/// The shader seeds reach the entry-table width and subprogram header
/// boundaries, with both normal and deliberately small memory budgets.
fn shader_seeds_are_cases(dir: &Path) {
    let (mut versions, mut meters, mut programs, mut counts) = (
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    let mut trees = 0;
    let mut script_types = BTreeSet::new();
    let mut complete = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    for entry in std::fs::read_dir(dir.join("shader")).expect("the target's folder") {
        let entry = entry.expect("an entry");
        let bytes = std::fs::read(entry.path()).expect("a seed");
        if entry.file_name().to_string_lossy().contains("-tree-") {
            let case = TreeCase::decode(&bytes);
            assert_eq!(case.encode(), bytes, "a seed that is not a tree case");
            assert_eq!(case.rows[0].ty, b"Shader");
            if entry
                .file_name()
                .to_string_lossy()
                .contains("-tree-complete-")
            {
                complete.insert(entry.file_name().to_string_lossy().into_owned());
                complete_shader_seed_parses(&case);
            }
            for row in &case.rows {
                if row.name == b"m_Script" {
                    script_types.insert(row.ty.clone());
                }
            }
            trees += 1;
            continue;
        }
        let case = unityrs_testkit::fuzz::ShaderCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        versions.insert(case.version);
        meters.insert(case.meter);
        if let Some(count) = case.blob.get(..4) {
            counts.insert(i32::from_le_bytes(count.try_into().expect("four bytes")));
        }
        if case.blob.get(..4) == Some(1i32.to_le_bytes().as_slice()) {
            let offset = if case.version >= (2019, 3) { 16 } else { 12 };
            if let Some(v) = case.blob.get(offset..offset + 4) {
                programs.insert(i32::from_le_bytes(v.try_into().expect("four bytes")));
            }
            if let Some(v) = case.blob.get(offset + 4..offset + 8) {
                kinds.insert(i32::from_le_bytes(v.try_into().expect("four bytes")));
            }
        }
    }
    assert!(versions.is_superset(&[(5, 3), (2019, 2), (2019, 3), (2021, 1)].into()));
    assert_eq!(meters, [0, 1, 16, 64].into());
    assert!(counts.is_superset(&[-1, 0, 1, 2, i32::MAX].into()));
    assert!(programs.is_superset(&[201_509_030, 201_608_170, 201_806_140, 202_012_090].into()));
    assert_eq!(trees, 32);
    assert_eq!(complete.len(), 12);
    assert!(kinds.is_superset(&[1, 23, 24].into()));
    assert_eq!(
        script_types,
        [
            b"TypelessData".to_vec(),
            b"string".to_vec(),
            b"int".to_vec(),
            b"vector".to_vec()
        ]
        .into()
    );
}

fn complete_shader_seed_parses(case: &TreeCase) {
    use unityrs_core::{
        Bytes, RawStr,
        limits::{Meter, MeteredVec},
        stream::Endian,
        typetree::{NodeList, NodeRow, Tree},
        walk::{ReadCtx, ReadOptions, RefTypes, boost},
    };
    let meter = Meter::unlimited();
    let mut rows = MeteredVec::new(&meter);
    for row in &case.rows {
        rows.push(
            NodeRow::new(
                i32::from(row.level),
                RawStr::from(row.ty.clone()),
                RawStr::from(row.name.clone()),
            )
            .with_meta_flag(row.meta_flag),
        )
        .unwrap();
    }
    let options = ReadOptions::default();
    let tree = Tree::compile(
        NodeList::from_rows(rows).unwrap().into_shared().unwrap(),
        &options.limits,
    )
    .unwrap();
    let parsed = boost::read(
        &Bytes::from(case.data.clone()),
        &tree,
        &ReadCtx {
            endian: Endian::Little,
            ref_types: RefTypes::List(&[]),
            options: &options,
            meter: &meter,
        },
    )
    .unwrap();
    assert_eq!(parsed.bytes_read, i64::try_from(case.data.len()).unwrap());
    assert!(parsed.get(&[b"m_ParsedForm"]).unwrap().as_dict().is_some());
    let packed = match parsed.get(&[b"compressedBlob"]).unwrap() {
        unityrs_core::Value::Bytes(b)
        | unityrs_core::Value::Prims(unityrs_core::PrimArray::U8(b)) => b,
        other => panic!("not shader bytes: {other:?}"),
    };
    assert!(!packed.is_empty());
    assert!(parsed.get(&[b"platforms"]).is_some());
    for name in [
        b"offsets".as_slice(),
        b"compressedLengths",
        b"decompressedLengths",
    ] {
        assert!(parsed.get(&[name]).is_some());
    }
}

/// Every `astc` seed is a `TextureCase` as written, of an ASTC format,
/// and all eighteen have seeds of whole blocks.
fn astc_seeds_are_cases(dir: &Path) {
    let mut formats = BTreeSet::new();
    for entry in std::fs::read_dir(dir.join("astc")).expect("the target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        assert!(
            (48..60).contains(&case.format) || (66..72).contains(&case.format),
            "{}",
            case.format
        );
        if case.data.len().is_multiple_of(16) && !case.data.is_empty() {
            formats.insert(case.format);
        }
    }
    assert_eq!(formats, (48..60).chain(66..72).collect());
}

/// Every `texture` seed is a `TextureCase` as written, and the seeds hold
/// each of the five formats and both meters, and a whole 4x4 block, under
/// the memory bound, down each decoder path: every BC7 mode and none (a
/// zero first byte), DXT1 with `c0 > c1` and not, DXT5 with `a0 > a1` and
/// not. (Run from the test above, so the seeds are written once.)
fn texture_seeds_are_cases(dir: &Path) {
    let (mut formats, mut meters) = (BTreeSet::new(), BTreeSet::new());
    let mut tierb = BTreeSet::new();
    let mut odd_yuy2 = false;
    let mut unknown_lengths = BTreeSet::new();
    let (mut pvrtc_atc_count, mut tierb_count) = (0, 0);
    let (mut bc7_modes, mut dxt1, mut dxt5) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in std::fs::read_dir(dir.join("texture")).expect("the target's folder") {
        let path = entry.expect("an entry").path();
        let name = path.file_name().unwrap().to_str().unwrap();
        pvrtc_atc_count += usize::from(name.contains("-pvrtc-atc-"));
        tierb_count += usize::from(name.contains("-tierb-"));
        let bytes = std::fs::read(path).expect("a seed");
        let case = TextureCase::decode(&bytes);
        assert_eq!(case.encode(), bytes, "a seed that is not a case");
        formats.insert(case.format);
        meters.insert(case.meter);
        // ALL[1000 mod 75] is BC5: two 16-byte blocks at 3x5.
        if case.format == 1000 && (case.width, case.height) == (3, 5) {
            unknown_lengths.insert(case.data.len());
        }
        if case.width == 8 && case.height == 8 && case.meter == 0 {
            tierb.insert(case.format);
        }
        odd_yuy2 |= case.format == 21 && case.width % 2 != 0;
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
    assert!(
        tierb.is_superset(
            &[
                9, 15, 16, 17, 18, 19, 20, 21, 22, 23, 62, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                82
            ]
            .into()
        )
    );
    assert!(formats.contains(&6) && odd_yuy2);
    assert!(formats.is_superset(&[24, 66, 67, 68, 69, 70, 71].into()));
    assert_eq!(unknown_lengths, [31, 32, 33].into());
    assert_eq!(pvrtc_atc_count, 576);
    assert_eq!(tierb_count, 1182);
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
    let (mut coefs, mut shifts) = (BTreeSet::new(), BTreeSet::new());
    let mut count = 0;
    let mut hostile_count = false;
    for entry in std::fs::read_dir(dir.join("fsb5")).expect("the fsb5 target's folder") {
        let bytes = std::fs::read(entry.expect("an entry").path()).expect("a seed");
        count += 1;
        if bytes.starts_with(b"FSB5") && bytes.len() >= 28 {
            let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            hostile_count |= word(8) == u32::MAX;
            if word(4) <= 1 && word(8) == 1 && [1, 2, 3, 4, 5, 7, 16].contains(&word(24)) {
                formats.insert((word(4), word(24)));
            }
            if word(4) <= 1 && word(24) == 16 && word(12) == 8 && word(16) == 0 {
                let start = if word(4) == 0 { 72 } else { 68 };
                for frame in bytes[start..].as_chunks::<0x8c>().0 {
                    for &b in &frame[..4] {
                        coefs.extend([b & 15, b >> 4]);
                    }
                    for &b in &frame[4..8] {
                        shifts.extend([b & 15, b >> 4]);
                    }
                }
            }
        }
    }
    assert_eq!(count, 83, "missing ADPCM edge seeds");
    assert!(hostile_count);
    assert_eq!(coefs, (0..16).collect());
    assert_eq!(shifts, (0..16).collect());
    assert_eq!(
        formats,
        (0..=1)
            .flat_map(|v| [1, 2, 3, 4, 5, 7, 16].map(move |m| (v, m)))
            .collect()
    );
}

fn head_seeds_reach_the_new_readers(dir: &Path) {
    let mut versions = BTreeSet::new();
    for target in ["serialized", "bundle"] {
        let mut count = 0;
        for entry in std::fs::read_dir(dir.join(target)).unwrap() {
            let path = entry.unwrap().path();
            if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("-head-")
            {
                continue;
            }
            count += 1;
            let bytes = std::fs::read(path).unwrap();
            if target == "serialized" {
                versions.insert(u32::from_be_bytes(bytes[8..12].try_into().unwrap()));
            } else {
                assert_eq!(u32::from_be_bytes(bytes[8..12].try_into().unwrap()), 9);
            }
        }
        // Bundle seeds include the three explicit-offset empty-block cases.
        assert_eq!(count, if target == "serialized" { 50 } else { 23 });
    }
    assert_eq!(versions, (23..=26).collect());
}

#[test]
fn scene_case_round_trips_and_truncations_are_total() {
    use unityrs_testkit::fuzz::SceneCase;
    let case = SceneCase {
        meter: 64,
        flags: 3,
        profile: br#"{"profile_version":1}"#.to_vec(),
        data: vec![1, 2, 3, 4],
    };
    let bytes = case.encode();
    assert_eq!(SceneCase::decode(&bytes), case);
    for len in 0..bytes.len() {
        let cut = SceneCase::decode(&bytes[..len]);
        assert!(cut.profile.len() <= case.profile.len());
        assert!(cut.data.len() <= case.data.len());
    }
}

fn scene_seeds_are_cases(dir: &Path) {
    use unityrs_testkit::fuzz::SceneCase;
    let mut targets = Vec::new();
    for target in ["scene_collect", "obj_text"] {
        let mut cases = Vec::new();
        for entry in std::fs::read_dir(dir.join(target)).expect("3D target seeds") {
            let entry = entry.unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            let case = SceneCase::decode(&bytes);
            assert_eq!(case.encode(), bytes);
            assert_ne!(case.data, [] as [u8; 0]);
            cases.push((entry.file_name(), case));
        }
        cases.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(cases.len() >= 100);
        let meters: BTreeSet<_> = cases.iter().map(|(_, c)| c.meter).collect();
        assert_eq!(meters, [0, 1, 16, 64].into());
        targets.push(cases);
    }
    assert_eq!(targets[0], targets[1]);
}
