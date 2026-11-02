//! `stream::Reader` against the scaffold's oracle fixture
//! (`tests/fixtures/scaffold/reader.json`): every method at positions from
//! below `-len` to past the end, as UnityPy 1.25.2's memoryview reader, and
//! as its stream reader over a file `fs.open` opened (the `stream_` cases),
//! up to 2^63 on a tmpfs file (the `stream_tmpfs_` cases, at positions a
//! file system with a smaller largest file refuses with `OSError`).

use std::path::Path;

use serde_json::Value as J;
use unityrs_core::Error;
use unityrs_core::limits::Meter;
use unityrs_core::stream::{Endian, Reader, ReaderKind, STREAM_READER, StreamRefusal};

fn load() -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scaffold/reader.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn big(v: &J) -> i128 {
    v.as_str().unwrap().parse().unwrap()
}

fn cases<'a>(doc: &'a J, key: &str) -> &'a Vec<J> {
    doc[key].as_array().unwrap()
}

/// The fixture's data: ten bytes whose values are their indexes.
const DATA: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

fn at(data: &[u8], pos: i128) -> Reader<'_> {
    let mut r = Reader::new(data, Endian::Little);
    r.set_position(pos).unwrap();
    r
}

#[test]
fn read_is_the_python_slice() {
    let doc = load();
    for c in cases(&doc, "read") {
        let size = i64::try_from(big(&c["size"])).unwrap();
        let mut r = at(&DATA, big(&c["pos"]));
        let got = r.read(size).unwrap();
        assert_eq!(
            got.len(),
            usize::try_from(c["n"].as_u64().unwrap()).unwrap(),
            "{c}"
        );
        if let Some(start) = c["start"].as_u64() {
            let start = usize::try_from(start).unwrap();
            assert_eq!(got, &DATA[start..start + got.len()], "{c}");
        }
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
        // `read_bytes` asks for the whole slice and moves the same way.
        if let Ok(n) = usize::try_from(size) {
            let mut r = at(&DATA, big(&c["pos"]));
            let whole = r.read_bytes(n);
            assert_eq!(whole.is_ok(), got.len() == n, "{c}");
            assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
        }
    }
}

#[test]
fn fixed_reads_move_only_on_success() {
    let doc = load();
    for c in cases(&doc, "unpack_from") {
        let n = c["n"].as_u64().unwrap();
        let pos = big(&c["pos"]);
        let start = c["start"].as_u64().map(|s| usize::try_from(s).unwrap());
        let mut r = at(&DATA, pos);
        let (ok, value): (bool, u64) = match n {
            2 => r.read_u16().map_or((false, 0), |v| (true, u64::from(v))),
            4 => r.read_u32().map_or((false, 0), |v| (true, u64::from(v))),
            8 => r.read_u64().map_or((false, 0), |v| (true, v)),
            _ => continue,
        };
        assert_eq!(ok, start.is_some(), "{c}");
        if let Some(s) = start {
            let mut b = [0u8; 8];
            b[..usize::try_from(n).unwrap()]
                .copy_from_slice(&DATA[s..s + usize::try_from(n).unwrap()]);
            assert_eq!(value, u64::from_le_bytes(b), "{c}");
        } else {
            assert!(matches!(at(&DATA, pos).read_i32(), Err(Error::OutOfBounds { .. })) || n != 4);
        }
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
    }
}

#[test]
fn one_byte_reads_move_even_when_they_fail() {
    let doc = load();
    for c in cases(&doc, "read_u_byte") {
        let pos = big(&c["pos"]);
        let want = c["value"].as_u64().map(|v| u8::try_from(v).unwrap());
        for read in [
            |r: &mut Reader<'_>| r.read_u8().ok(),
            |r: &mut Reader<'_>| r.read_i8().ok().map(|v| v.to_le_bytes()[0]),
            |r: &mut Reader<'_>| r.read_bool().ok().map(u8::from),
        ] {
            let mut r = at(&DATA, pos);
            let got = read(&mut r);
            assert_eq!(got.is_some(), want.is_some(), "{c}");
            assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
        }
        assert_eq!(at(&DATA, pos).read_u8().ok(), want, "{c}");
    }
}

#[test]
fn align_is_python_floor_modulo() {
    let doc = load();
    for c in cases(&doc, "align") {
        let mut r = at(&DATA, big(&c["pos"]));
        r.align(u32::try_from(c["n"].as_u64().unwrap()).unwrap())
            .unwrap();
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn read_string_to_null_uses_the_full_window() {
    // The fixture records windows of 1-20 bytes; the reader's is 32,767, so
    // every case with a window at least the data's length applies as is.
    let doc = load();
    let datas: Vec<Vec<u8>> = cases(&doc, "string_to_null_data")
        .iter()
        .map(|h| hex(h.as_str().unwrap()))
        .collect();
    let mut checked = 0;
    for c in cases(&doc, "string_to_null") {
        let data = &datas[usize::try_from(c["data"].as_u64().unwrap()).unwrap()];
        let pos = big(&c["pos"]);
        let max = usize::try_from(c["max"].as_u64().unwrap()).unwrap();
        // With a window reaching past the end the outcome no longer depends
        // on its size (`pos + max >= len` either way).
        if i128::try_from(max).unwrap() + pos < i128::try_from(data.len()).unwrap() + 1 {
            continue;
        }
        let mut r = at(data, pos);
        let got = r.read_string_to_null();
        if c.get("error").is_some() {
            assert!(matches!(got, Err(Error::OutOfBounds { .. })), "{c}");
        } else {
            assert_eq!(got.unwrap(), hex(c["hex"].as_str().unwrap()), "{c}");
            assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
        }
        checked += 1;
    }
    assert!(checked > 20, "{checked}");
}

#[test]
fn read_string_to_null_both_sides_of_the_window_end() {
    // No NUL in the window: at `pos + 32_767 >= len` it raises; below it, it
    // returns the 32,767 bytes from `pos` and moves past them.
    let data = vec![b'a'; 32_767 + 10];
    assert!(
        at(&data, 10).read_string_to_null().is_err(),
        "pos + window == len"
    );
    assert!(at(&data, 11).read_string_to_null().is_err(), "past it");
    let mut r = at(&data, 9);
    assert_eq!(r.read_string_to_null().unwrap().len(), 32_767);
    assert_eq!(r.position(), 9 + 32_767);
    // A NUL just inside the window ends the string there.
    let mut data = data;
    data[9 + 32_766] = 0;
    let mut r = at(&data, 10);
    assert_eq!(r.read_string_to_null().unwrap().len(), 32_765);
    assert_eq!(r.position(), 9 + 32_767);
}

#[test]
fn big_endian_reads_swap() {
    let mut r = Reader::new(&DATA, Endian::Big);
    assert_eq!(r.read_u32().unwrap(), 0x0001_0203);
    assert_eq!(r.read_i16().unwrap(), 0x0405);
    assert!(r.read_i64().is_err(), "two bytes short");
    assert_eq!(r.position(), 6);
    assert_eq!(r.read_array::<2>().unwrap(), [6, 7]);
}

// ---------------------------------------------------------------- the stream reader

/// A stream reader (a file loaded from a path) at `pos`, which the
/// fixture's stream cases only set where `seek` takes it.
fn stream_at(data: &[u8], pos: i128) -> Reader<'_> {
    let mut r = Reader::with_kind(data, Endian::Little, ReaderKind::Stream);
    r.set_position(pos).unwrap();
    r
}

/// The text a [`StreamRefusal`] stands for, as the fixture records
/// UnityPy's exception (`ValueError: <message>`).
fn refusal(e: &Error) -> String {
    match e {
        Error::Malformed { what, detail } if *what == STREAM_READER => {
            let class = [
                StreamRefusal::NegativePosition,
                StreamRefusal::PositionTooLarge,
                StreamRefusal::NegativeRead,
                StreamRefusal::ReadTooLarge,
                StreamRefusal::ReadSizeOverflow,
            ]
            .into_iter()
            .find(|r| r.message() == &**detail)
            .map_or("?", StreamRefusal::exception);
            format!("{class}: {detail}")
        }
        other => format!("{other:?}"),
    }
}

/// The stream cases of `key` on a file of the scratch folder and on a
/// tmpfs file (`stream_tmpfs_`), the latter at least `tmpfs` of them.
fn stream_cases<'a>(doc: &'a J, key: &str, tmpfs: usize) -> impl Iterator<Item = &'a J> {
    let far = cases(doc, &format!("stream_tmpfs_{key}"));
    assert!(far.len() >= tmpfs, "{key}: {}", far.len());
    cases(doc, &format!("stream_{key}")).iter().chain(far)
}

#[test]
fn the_stream_reader_refuses_a_position_seek_cannot_take() {
    let doc = load();
    let all: Vec<_> = stream_cases(&doc, "set_position", 6).collect();
    assert!(all.len() >= 16);
    for c in all {
        let mut r = stream_at(&DATA, 3);
        match (r.set_position(big(&c["pos"])), c.get("error")) {
            (Ok(()), None) => {}
            (Err(e), Some(want)) => assert_eq!(refusal(&e), want.as_str().unwrap(), "{c}"),
            (got, _) => panic!("{got:?}: {c}"),
        }
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
    }
}

#[test]
fn the_stream_reader_reads_what_exists_and_stops_at_the_end() {
    let doc = load();
    let all: Vec<_> = stream_cases(&doc, "read", 40).collect();
    assert!(all.len() > 240);
    let mut machine = 0;
    for c in all {
        let size = i64::try_from(big(&c["size"])).unwrap();
        let mut r = stream_at(&DATA, big(&c["pos"]));
        let want = c
            .get("error")
            .filter(|e| !e.as_str().unwrap().starts_with("OSError"));
        if want.is_none() && c.get("error").is_some() {
            // The machine's refusal (`PARITY.md` section 19.3): at 2^63 - 1
            // the buffered file's fill would end past it, which Linux refuses
            // (`EINVAL`). unityrs reads on, here past the end: nothing.
            assert_eq!(r.read(size).map(<[u8]>::len).ok(), Some(0), "{c}");
            assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
            machine += 1;
            continue;
        }
        match (r.read(size), want) {
            (Ok(got), None) => {
                assert_eq!(
                    got.len(),
                    usize::try_from(c["n"].as_u64().unwrap()).unwrap(),
                    "{c}"
                );
                if let Some(start) = c["start"].as_u64() {
                    let start = usize::try_from(start).unwrap();
                    assert_eq!(got, &DATA[start..start + got.len()], "{c}");
                }
            }
            (Err(e), Some(want)) => assert_eq!(refusal(&e), want.as_str().unwrap(), "{c}"),
            (got, _) => panic!("{got:?}: {c}"),
        }
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
    }
    assert!(machine > 0, "no read the machine refuses is recorded");
}

#[test]
fn the_stream_readers_fixed_reads_move_by_what_they_read() {
    let doc = load();
    for c in cases(&doc, "stream_unpack") {
        let pos = big(&c["pos"]);
        let start = c["start"].as_u64();
        let mut r = stream_at(&DATA, pos);
        // The fixture records the low byte, the first one read.
        let got = match c["n"].as_u64().unwrap() {
            2 => r.read_u16().ok().map(|v| u64::from(v & 0xFF)),
            4 => r.read_u32().ok().map(|v| u64::from(v & 0xFF)),
            8 => r.read_u64().ok().map(|v| v & 0xFF),
            n => panic!("{n}"),
        };
        assert_eq!(got, start, "{c}");
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
    }
    for c in cases(&doc, "stream_read_u_byte") {
        let want = c["value"].as_u64().map(|v| u8::try_from(v).unwrap());
        for read in [
            |r: &mut Reader<'_>| r.read_u8().ok(),
            |r: &mut Reader<'_>| r.read_i8().ok().map(|v| v.to_le_bytes()[0]),
            |r: &mut Reader<'_>| r.read_bool().ok().map(|_| 0),
        ] {
            let mut r = stream_at(&DATA, big(&c["pos"]));
            assert_eq!(read(&mut r).is_some(), want.is_some(), "{c}");
            assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
        }
        assert_eq!(stream_at(&DATA, big(&c["pos"])).read_u8().ok(), want, "{c}");
    }
}

#[test]
fn the_stream_reader_aligns_as_the_memory_reader() {
    // The memory reader's arithmetic, assigned through the `Position`
    // setter: from within `n - 1` of 2^63 - 1 its `seek` refuses the result,
    // and the position stays (the tmpfs cases).
    let doc = load();
    let mut refused = 0;
    for c in stream_cases(&doc, "align", 30) {
        let mut r = stream_at(&DATA, big(&c["pos"]));
        let got = r.align(u32::try_from(c["n"].as_u64().unwrap()).unwrap());
        match (got, c.get("error")) {
            (Ok(()), None) => {}
            (Err(e), Some(want)) => {
                assert_eq!(refusal(&e), want.as_str().unwrap(), "{c}");
                refused += 1;
            }
            (got, _) => panic!("{got:?}: {c}"),
        }
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
    }
    assert!(refused >= 10, "{refused}");
}

#[test]
fn the_stream_reader_drops_the_last_byte_of_an_unterminated_string() {
    // As `read_string_to_null_uses_the_full_window`: the reader's window is
    // 32,767 bytes, so the fixture's 32,767-byte cases apply as they are.
    let doc = load();
    let datas: Vec<Vec<u8>> = cases(&doc, "string_to_null_data")
        .iter()
        .map(|h| hex(h.as_str().unwrap()))
        .collect();
    let mut checked = 0;
    for c in cases(&doc, "stream_string_to_null") {
        if c["max"].as_u64() != Some(32_767) {
            continue;
        }
        let data = &datas[usize::try_from(c["data"].as_u64().unwrap()).unwrap()];
        let mut r = stream_at(data, big(&c["pos"]));
        match (r.read_string_to_null(), c.get("error")) {
            (Ok(got), None) => assert_eq!(got, hex(c["hex"].as_str().unwrap()), "{c}"),
            (Err(Error::OutOfBounds { .. }), Some(_)) => {}
            (got, _) => panic!("{got:?}: {c}"),
        }
        assert_eq!(r.position(), big(&c["new_pos"]), "{c}");
        checked += 1;
    }
    assert!(checked > 30, "{checked}");
    // Past the window the stream reader returns one byte fewer than the
    // memory reader, both at the same position (UnityPy 1.25.2, a path's
    // stream: `test_rows_paths.py` a_path_reads_strings_with_the_stream_rule).
    let data = vec![b'B'; 40_000];
    let mut s = stream_at(&data, 0);
    assert_eq!(s.read_string_to_null().unwrap().len(), 32_766);
    assert_eq!(s.position(), 32_767);
    let mut m = at(&data, 0);
    assert_eq!(m.read_string_to_null().unwrap().len(), 32_767);
    assert_eq!(m.position(), 32_767);
}

#[test]
fn read_i32_array_reads_as_each_reader_reads_it() {
    let doc = load();
    let datas: Vec<Vec<u8>> = cases(&doc, "int_array_data")
        .iter()
        .map(|h| hex(h.as_str().unwrap()))
        .collect();
    let meter = Meter::unlimited();
    for (key, kind) in [
        ("int_array", ReaderKind::Memory),
        ("stream_int_array", ReaderKind::Stream),
    ] {
        let all = cases(&doc, key);
        assert!(all.len() > 40, "{key}");
        for c in all {
            let data = &datas[usize::try_from(c["data"].as_u64().unwrap()).unwrap()];
            let mut r = Reader::with_kind(data, Endian::Little, kind);
            r.set_position(big(&c["pos"])).unwrap();
            let got = r.read_i32_array(&meter);
            match (&got, c.get("values")) {
                (Ok(v), Some(want)) => {
                    let want: Vec<i32> = want
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| i32::try_from(x.as_i64().unwrap()).unwrap())
                        .collect();
                    assert_eq!(v.as_slice(), want.as_slice(), "{key} {c}");
                }
                (Err(Error::Malformed { .. }), None) => {
                    assert!(
                        c["error"].as_str().unwrap().contains("bad char"),
                        "{key} {c}"
                    );
                }
                // Every other `struct.error`: the bytes do not hold it.
                (Err(Error::OutOfBounds { .. }), None) => {
                    assert!(
                        !c["error"].as_str().unwrap().contains("bad char"),
                        "{key} {c}"
                    );
                }
                _ => panic!("{got:?}: {key} {c}"),
            }
            assert_eq!(r.position(), big(&c["new_pos"]), "{key} {c}");
        }
    }
}
