//! Replays the scaffold's oracle fixtures (`tests/fixtures/scaffold/`, written
//! by `tools/fixtures/scaffold.py` from UnityPy 1.25.2) without Python.

use std::path::Path;

use serde_json::Value as J;
use unityrs_core::env::pptr::{ExternalIndexError, external_index};
use unityrs_core::stream::{ReaderKind, py_align, py_read, py_string_to_null, py_unpack_from};

fn load(name: &str) -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/scaffold")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn big(v: &J) -> i128 {
    v.as_str().unwrap().parse().unwrap()
}

fn small(v: &J) -> i64 {
    v.as_i64().unwrap()
}

fn cases<'a>(doc: &'a J, key: &str) -> &'a Vec<J> {
    doc[key].as_array().unwrap()
}

const DATA_LEN: usize = 10;

#[test]
fn read_is_the_oracle_slice() {
    let doc = load("reader.json");
    let all = cases(&doc, "read");
    assert!(all.len() > 300);
    for c in all {
        let (pos, size) = (big(&c["pos"]), i64::try_from(big(&c["size"])).unwrap());
        let (range, new_pos) = py_read(DATA_LEN, pos, size);
        let n = usize::try_from(small(&c["n"])).unwrap();
        assert_eq!(range.len(), n, "{c}");
        if let Some(start) = c["start"].as_u64() {
            assert_eq!(range.start, usize::try_from(start).unwrap(), "{c}");
        }
        assert_eq!(new_pos, big(&c["new_pos"]), "{c}");
    }
}

#[test]
fn fixed_width_reads_are_unpack_from() {
    let doc = load("reader.json");
    for c in cases(&doc, "unpack_from") {
        let pos = big(&c["pos"]);
        let n = usize::try_from(small(&c["n"])).unwrap();
        let got = py_unpack_from(DATA_LEN, pos, n);
        let want = c["start"].as_u64().map(|s| usize::try_from(s).unwrap());
        assert_eq!(got, want, "{c}");
        // The position moves only when the read succeeds.
        let moved = if got.is_some() {
            pos + i128::try_from(n).unwrap()
        } else {
            pos
        };
        assert_eq!(moved, big(&c["new_pos"]), "{c}");
    }
}

#[test]
fn one_byte_reads_are_unpack_of_read_1() {
    let doc = load("reader.json");
    for c in cases(&doc, "read_u_byte") {
        let pos = big(&c["pos"]);
        let (range, new_pos) = py_read(DATA_LEN, pos, 1);
        // The bytes are 0..10, so a byte's value is its index.
        let got = (range.len() == 1).then_some(range.start);
        let want = c["value"].as_u64().map(|v| usize::try_from(v).unwrap());
        assert_eq!(got, want, "{c}");
        assert_eq!(new_pos, big(&c["new_pos"]), "{c}");
    }
}

#[test]
fn align_is_python_floor_modulo() {
    let doc = load("reader.json");
    for c in cases(&doc, "align") {
        let n = u32::try_from(small(&c["n"])).unwrap();
        assert_eq!(py_align(big(&c["pos"]), n), big(&c["new_pos"]), "{c}");
    }
}

#[test]
fn string_to_null_clamps_the_search() {
    let doc = load("reader.json");
    let datas: Vec<Vec<u8>> = cases(&doc, "string_to_null_data")
        .iter()
        .map(|h| hex(h.as_str().unwrap()))
        .collect();
    for c in cases(&doc, "string_to_null") {
        let data = &datas[usize::try_from(small(&c["data"])).unwrap()];
        let max = usize::try_from(small(&c["max"])).unwrap();
        let got = py_string_to_null(data, big(&c["pos"]), max);
        if c.get("error").is_some() {
            assert_eq!(got, None, "{c}");
        } else {
            let (range, new_pos) = got.unwrap_or_else(|| panic!("raised: {c}"));
            assert_eq!(data[range].to_vec(), hex(c["hex"].as_str().unwrap()), "{c}");
            assert_eq!(new_pos, big(&c["new_pos"]), "{c}");
        }
    }
}

#[test]
fn the_stream_readers_string_to_null_is_the_base_classs_loop() {
    // `EndianBinaryReader.read_string_to_null` (`:136-144`) on a stream of a
    // file `fs.open` opened: every window size the fixture records.
    let doc = load("reader.json");
    let datas: Vec<Vec<u8>> = cases(&doc, "string_to_null_data")
        .iter()
        .map(|h| hex(h.as_str().unwrap()))
        .collect();
    let all = cases(&doc, "stream_string_to_null");
    assert!(all.len() > 100);
    for c in all {
        let data = &datas[usize::try_from(small(&c["data"])).unwrap()];
        let max = usize::try_from(small(&c["max"])).unwrap();
        let got = ReaderKind::Stream.string_to_null(data, big(&c["pos"]), max);
        if c.get("error").is_some() {
            assert_eq!(got, None, "{c}");
        } else {
            let (range, new_pos) = got.unwrap_or_else(|| panic!("raised: {c}"));
            assert_eq!(data[range].to_vec(), hex(c["hex"].as_str().unwrap()), "{c}");
            assert_eq!(new_pos, big(&c["new_pos"]), "{c}");
        }
    }
}

#[test]
fn external_index_is_python_indexing_after_a_ge_guard() {
    let doc = load("pptr_external.json");
    for c in cases(&doc, "cases") {
        let len = usize::try_from(small(&c["len"])).unwrap();
        let file_id = i32::try_from(small(&c["file_id"])).unwrap();
        let got = match external_index(file_id, len) {
            Ok(i) => format!("index:{i}"),
            Err(ExternalIndexError::PastEnd) => "past_end".into(),
            Err(ExternalIndexError::BelowStart) => "below_start".into(),
        };
        assert_eq!(got, c["outcome"].as_str().unwrap(), "{c}");
    }
}

/// Why package C's LZ4 codec checks the sequence stream itself (plan
/// section 3.6): the pinned `lz4_flex` and liblz4 1.9.4 disagree both ways.
/// `lz4_flex` decodes blocks liblz4 refuses (its end-of-block conditions), so
/// a bundle UnityPy fails would load; and liblz4 decodes a match at offset 0
/// (a run of zeros: it writes zeros before copying from the match), which
/// `lz4_flex` refuses. C's codec test replays this fixture and must match
/// every verdict and output.
#[test]
fn lz4_flex_and_liblz4_disagree_both_ways() {
    let doc = load("lz4_blocks.json");
    let (mut checked, mut flex_only, mut liblz4_only) = (0, 0, 0);
    for c in cases(&doc, "cases") {
        let block = hex(c["block"].as_str().unwrap());
        for r in c["declared"].as_array().unwrap() {
            let declared = usize::try_from(r[0].as_u64().unwrap()).unwrap();
            let mut out = vec![0; declared];
            let flex = lz4_flex::block::decompress_into(&block, &mut out);
            match (r[1].as_u64(), flex) {
                (Some(n), Ok(m)) => {
                    let n = usize::try_from(n).unwrap();
                    assert_eq!(m, n, "{c}");
                    assert_eq!(hex(c["output"].as_str().unwrap()), out[..n].to_vec(), "{c}");
                }
                (Some(_), Err(_)) => liblz4_only += 1,
                (None, Ok(_)) => flex_only += 1,
                (None, Err(_)) => {}
            }
            checked += 1;
        }
    }
    assert!(checked > 8000, "{checked}");
    assert!(
        flex_only > 0,
        "lz4_flex now refuses every block liblz4 refuses"
    );
    assert!(
        liblz4_only > 0,
        "lz4_flex now accepts every block liblz4 accepts"
    );
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
