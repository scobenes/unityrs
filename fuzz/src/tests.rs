//! The harness's own tests: each value case reaches the memory bound it
//! picks, the `lz4` check takes the block-output bound for the limit it
//! is, at the bound and on each side of it, the `texture` check takes
//! each answer a decode gives under each of its meters, and the
//! `meshdecode` check takes every handler fixture case under each meter.

use std::sync::Arc;

use unityrs_core::error::LimitKind;
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::walk::{ReadCtx, ReadDetail, ReadError, boost, python};
use unityrs_testkit::fuzz::{MeshCase, Row, TextureCase, TreeCase, lz4_case};

use crate::Case;

/// The bound a read stopped at, if a bound stopped it.
fn bound(e: &ReadError) -> Option<LimitKind> {
    match &e.detail {
        ReadDetail::Limit(l) | ReadDetail::Charge(ChargeError::Limit(l)) => Some(l.kind),
        _ => None,
    }
}

/// A list of `2^31 - 1` empty classes: a read reaches a bound, after one
/// value per element, whatever the bounds.
fn many_small_values(flags: u8) -> Vec<u8> {
    TreeCase {
        flags,
        start: 0,
        rows: vec![
            Row::new(0, b"Base", b"Base", 0),
            Row::new(1, b"vector", b"x", 0),
            Row::new(2, b"Array", b"Array", 0),
            Row::new(3, b"int", b"size", 0),
            Row::new(3, b"Empty", b"data", 0),
        ],
        refs: Vec::new(),
        data: i32::MAX.to_le_bytes().to_vec(),
    }
    .encode()
}

/// Where a read of `case` on `meter` stops: UnityPyBoost's reader's, or
/// with `python` the pure-Python one's from position 0.
fn stop(case: &Case, meter: &Arc<Meter>, python: bool) -> Option<LimitKind> {
    crate::with_stack(|| {
        let read = ReadCtx {
            endian: case.endian(),
            ref_types: case.ref_types(),
            options: &case.options,
            meter,
        };
        let r = if python {
            python::read(&case.data, 0, &case.tree, &read)
        } else {
            boost::read(&case.data, &case.tree, &read)
        };
        r.err().as_ref().and_then(bound)
    })
}

#[test]
fn a_value_case_reaches_the_memory_bound_it_picks() {
    for python in [false, true] {
        let large = Case::new(&many_small_values(0)).expect("the tree compiles");
        assert_eq!(
            stop(&large, &large.meter, python),
            Some(LimitKind::DecodeWork),
            "python {python}: at the large memory bound a read stops at the work bound"
        );
        let small =
            Case::new(&many_small_values(TreeCase::SMALL_MEMORY)).expect("the tree compiles");
        assert_eq!(
            stop(&small, &small.meter, python),
            Some(LimitKind::InputMemory),
            "python {python}: at the small memory bound a read stops at the memory bound"
        );
        // `write_after_read`'s second read, on a meter of its own.
        assert_eq!(
            stop(&small, &small.new_meter(), python),
            Some(LimitKind::InputMemory),
            "python {python}: a new meter keeps the case's memory bound"
        );
    }
}

/// An LZ4 block whose output is `len` bytes (25 or more): one literal, a
/// match of offset 1 that repeats it to `len - 5` bytes, and five last
/// literals.
fn block_of(len: usize) -> Vec<u8> {
    let mut extra = len - 6 - 19;
    let mut b = vec![0x1F, b'A', 1, 0];
    while extra >= 255 {
        b.push(255);
        extra -= 255;
    }
    b.push(u8::try_from(extra).expect("under 255"));
    b.extend_from_slice(&[0x50, b'B', b'C', b'D', b'E', b'F']);
    b
}

#[test]
fn the_lz4_check_takes_the_block_output_bound_for_the_limit_it_is() {
    let bound = usize::try_from(crate::WORK).expect("a usize");
    // liblz4 accepts each; unityrs below and at the bound, and refuses past
    // it with `BlockOutput`. A memory bound at the block bound refused the
    // block of exactly the bound (its allocation costs a page more), and a
    // block past it failed the check as a difference from liblz4.
    for len in [bound - 1, bound, bound + 1, 8_415_025] {
        for declared in [len, 16 << 20] {
            let declared = u32::try_from(declared).expect("a u32");
            crate::lz4::check(&lz4_case(declared, &block_of(len)));
        }
    }
}

/// The `texture` check passes an image, a meter's refusal below the charge,
/// and each of the plan's errors as the decode's, under each meter.
#[test]
fn the_texture_check_takes_each_answer_under_each_meter() {
    let case = |format, width, height, meter, data: Vec<u8>| {
        TextureCase {
            format,
            width,
            height,
            platform: 19,
            meter,
            blob: None,
            data,
        }
        .encode()
    };
    for meter in 0..4 {
        // An image (refused under the charge less 1), short data, past the
        // bound, Pillow's raw width cap, a side of 0, empty data.
        crate::texture::check(&case(4, 2, 2, meter, vec![7; 16]));
        crate::texture::check(&case(25, 5, 3, meter, vec![1; 31]));
        crate::texture::check(&case(10, 16385, 16384, meter, vec![1; 3]));
        crate::texture::check(&case(4, 67_108_857, 1, meter, vec![1]));
        crate::texture::check(&case(1000, 0, 4, meter, Vec::new()));
        crate::texture::check(&case(12, 4, 4, meter, Vec::new()));
    }
    crate::texture::check(&[]);
}

/// Every case of `unityrs-meshdecode`'s handler fixture as a `MeshCase`, as
/// the seed command (`unityrs-testkit`'s `examples/fuzz_seeds.rs`) writes
/// them, at meter 0.
fn mesh_fixture_cases() -> Vec<MeshCase> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/unityrs-meshdecode/tests/fixtures/process.json"
    ))
    .expect("the handler fixture");
    let body: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let hex = |v: &serde_json::Value| -> Vec<u8> {
        let h = v.as_str().expect("hex").as_bytes();
        h.chunks(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
            .collect()
    };
    let int = |v: &serde_json::Value| v.as_i64().expect("an int");
    let cases = body["cases"].as_array().expect("cases");
    cases
        .iter()
        .map(|c| {
            let rows = body["trees"][c["tree"].as_str().expect("a tree key")]
                .as_array()
                .expect("rows")
                .iter()
                .map(|r| {
                    Row::new(
                        u8::try_from(int(&r[0])).expect("a level"),
                        r[1].as_str().expect("a type").as_bytes(),
                        r[2].as_str().expect("a name").as_bytes(),
                        i32::try_from(int(&r[3])).expect("a meta flag"),
                    )
                })
                .collect();
            let v: Vec<i64> = c["version"]
                .as_array()
                .expect("a version")
                .iter()
                .map(int)
                .collect();
            let mut flags = 0;
            if c["source"] == "sprite" {
                flags |= MeshCase::SPRITE;
            }
            if c["endian"] == ">" {
                flags |= MeshCase::BIG_ENDIAN;
            }
            if c["reader"].as_bool() == Some(true) && !c["resource"].is_null() {
                flags |= MeshCase::RESOURCES;
            }
            MeshCase {
                flags,
                meter: 0,
                version: [0, 1, 2].map(|i| u16::try_from(v[i]).expect("a u16")),
                version_type: u8::try_from(v[3]).expect("a type"),
                version_number: u8::try_from(v[4]).expect("a number"),
                resource: if c["resource"].is_null() {
                    Vec::new()
                } else {
                    hex(&c["resource"])
                },
                value: TreeCase {
                    rows,
                    data: hex(&c["hex"]),
                    ..TreeCase::default()
                },
            }
        })
        .collect()
}

/// The `meshdecode` check passes every handler fixture case under the
/// case's limits and under meters of 64 bytes to about 4 MiB, and each kind
/// of answer is among them: arrays and triangles, the handler's errors, and
/// the smaller meter's refusals, of `process` and of `get_triangles` alone.
#[test]
fn the_meshdecode_check_takes_every_handler_fixture_case_under_each_meter() {
    use crate::meshdecode::{Outcome, answers};
    let cases = mesh_fixture_cases();
    assert!(cases.len() > 300, "{} cases", cases.len());
    let (mut done, mut errors, mut refused, mut triangles_refused, mut same) = (0, 0, 0, 0, 0);
    for mut case in cases {
        for meter in [0, 1, 4, 16, 64, 255] {
            case.meter = meter;
            let (full, small) = answers(&case.encode()).expect("every fixture value reads");
            match full {
                (Outcome::Done(_), Some(Outcome::Done(_))) => done += 1,
                (Outcome::Error(_), None) | (Outcome::Done(_), Some(Outcome::Error(_))) => {
                    errors += 1
                }
                other => panic!("{other:?} under the case's limits"),
            }
            match small {
                Some((Outcome::Refused, _)) => refused += 1,
                Some((_, Some(Outcome::Refused))) => triangles_refused += 1,
                Some(_) => same += 1,
                None => {}
            }
        }
    }
    assert!(
        done > 0 && errors > 0 && refused > 0 && triangles_refused > 0 && same > 0,
        "done {done}, errors {errors}, refused {refused}, triangles refused {triangles_refused}, same {same}"
    );
    assert!(answers(&[]).is_none());
}
