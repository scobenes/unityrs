//! Replays `tests/fixtures/d/paths.json`, written by `tools/fixtures/env.py`
//! from UnityPy 1.25.2: files UnityPy reads differently from a path, which
//! its `Environment` opens and reads with the stream reader, than from
//! bytes, which the memory reader reads. Each case is loaded both ways
//! ([`Source::Path`] from a `unityrs_testkit::scratch` folder this test
//! owns, removed however it ends, and [`Source::Bytes`]), and the loads, every object's read with and without
//! `check_read`, its raw data, the `SerializedFile`'s `userInformation` and
//! the case's resource reads must be UnityPy's for that way. UnityPy loaded
//! a case marked `tmpfs` from tmpfs files, whose `seek` takes every position
//! below 2^63, where a smaller file system's raises `OSError`; unityrs reads
//! every path by that rule (`PARITY.md` section 19.3), so it loads such a
//! case from this test's folder.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::stream::{STREAM_READER, StreamRefusal};
use unityrs_core::value::Value;
use unityrs_core::walk::{L2Kind, ParseError, ReadDetail, ReadError};
use unityrs_core::{Bytes, Error, File, Parsed, RawStr, SerializedFile};
use unityrs_testkit::scratch::Scratch;

fn fixture() -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/d/paths.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// `[count, byte]` runs, as `files.py`'s `runs` writes them.
fn unruns(v: &J) -> Vec<u8> {
    let mut out = Vec::new();
    for r in v.as_array().unwrap() {
        let n = usize::try_from(r[0].as_u64().unwrap()).unwrap();
        let b = u8::try_from(r[1].as_u64().unwrap()).unwrap();
        out.extend(std::iter::repeat_n(b, n));
    }
    out
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

const REFUSALS: [StreamRefusal; 5] = [
    StreamRefusal::NegativePosition,
    StreamRefusal::PositionTooLarge,
    StreamRefusal::NegativeRead,
    StreamRefusal::ReadTooLarge,
    StreamRefusal::ReadSizeOverflow,
];

fn refusal_text(r: StreamRefusal) -> String {
    format!("{}: {}", r.exception(), r.message())
}

/// The stream reader's refusal a file-level error carries, as the fixture
/// writes UnityPy's exception.
fn stream_of(e: &Error) -> Option<String> {
    match e {
        Error::Malformed { what, detail } if *what == STREAM_READER => REFUSALS
            .into_iter()
            .find(|r| r.message() == &**detail)
            .map(refusal_text),
        _ => None,
    }
}

fn load_outcome(got: &Result<File, Error>) -> J {
    match got {
        Ok(File::Serialized(_)) => json!("serialized"),
        Ok(File::Bundle(_)) => json!("bundle"),
        Ok(File::Raw(_)) => json!("raw"),
        Ok(File::Web(_)) => json!("web"),
        Err(e) => json!(stream_of(e).unwrap_or_else(|| "error".into())),
    }
}

/// The root dict, its numbers and strings as they are and its bytes as hex
/// (the cases' trees hold nothing else).
fn value_of(p: &Parsed) -> J {
    let Value::Dict(m) = p.value() else {
        panic!("not a dict: {:?}", p.value())
    };
    let mut out = serde_json::Map::new();
    for (name, v) in p.entries(m) {
        let v = match v {
            Value::Int(i) => json!(i),
            Value::Bytes(b) => json!(hex(b)),
            Value::Str(s) => json!(String::from_utf8_lossy(s.as_bytes())),
            other => panic!("{other:?}"),
        };
        out.insert(String::from_utf8_lossy(name).into_owned(), v);
    }
    J::Object(out)
}

fn read_outcome(got: &Result<Parsed, ParseError>) -> J {
    match got {
        Ok(p) => json!({ "value": value_of(p) }),
        Err(ParseError::Read(ReadError {
            detail: ReadDetail::Stream(r),
            ..
        })) => json!({ "stream": refusal_text(*r) }),
        Err(e) => {
            let read = match e {
                ParseError::CheckRead { parsed, .. } if e.l2_kind() == L2Kind::CheckRead => {
                    json!(parsed.bytes_read)
                }
                _ => J::Null,
            };
            json!({ "error": e.l2_kind().as_str(), "bytes_read": read })
        }
    }
}

fn bytes_outcome(got: &Result<Bytes, Error>) -> J {
    match got {
        Ok(b) => json!(hex(b)),
        Err(e) => json!({ "stream": stream_of(e).unwrap_or_else(|| "error".into()) }),
    }
}

fn file_record(sf: &Arc<SerializedFile>) -> J {
    let info = sf.user_information().map_or(&[][..], |s| s.as_bytes());
    let objects: Vec<J> = sf
        .objects()
        .map(|o| {
            json!([
                o.path_id(),
                read_outcome(&o.parse_as_dict(true)),
                read_outcome(&o.parse_as_dict(false)),
                bytes_outcome(&o.raw_data()),
            ])
        })
        .collect();
    json!({
        "user_information": [
            info.len(),
            hex(&info[..info.len().min(8)]),
            hex(&info[info.len().saturating_sub(8)..]),
        ],
        "objects": objects,
    })
}

/// One way of loading a case's files, recorded as the fixture records it.
fn run(case: &J, dir: &Path, by_path: bool) -> J {
    let env = Environment::new(EnvOptions::unitypy());
    let mut loads = Vec::new();
    let mut files = Vec::new();
    let mut first_sf = None;
    for f in case["files"].as_array().unwrap() {
        let name = f[0].as_str().unwrap();
        let source = if by_path {
            Source::Path(dir.join(name))
        } else {
            Source::Bytes {
                name: RawStr::from(name),
                data: Bytes::from(unruns(&f[1])),
            }
        };
        let got = env.load(source);
        loads.push(load_outcome(&got));
        match got {
            Ok(File::Serialized(sf)) => {
                files.push(file_record(&sf));
                first_sf.get_or_insert(sf);
            }
            _ => files.push(J::Null),
        }
    }
    let resources: Vec<J> = case["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let offset: u64 = r[0].as_str().unwrap().parse().unwrap();
            let size: u64 = r[1].as_str().unwrap().parse().unwrap();
            let sf = first_sf.as_ref().unwrap();
            bytes_outcome(&env.resource(sf, &RawStr::from("a.resS"), offset, size))
        })
        .collect();
    json!({ "loads": loads, "files": files, "resources": resources })
}

#[test]
fn a_path_is_read_with_the_stream_readers_rules() {
    let doc = fixture();
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 26, "{}", cases.len());
    for (i, case) in cases.iter().enumerate() {
        let dir = Scratch::new(&format!("env-paths-{i}"));
        for f in case["files"].as_array().unwrap() {
            std::fs::write(dir.join(f[0].as_str().unwrap()), unruns(&f[1])).unwrap();
        }
        let name = case["name"].as_str().unwrap();
        for (how, by_path) in [("bytes", false), ("path", true)] {
            let got = run(case, &dir, by_path);
            assert_eq!(got, case[how], "{name}, loaded from {how}");
        }
        // Every case is one where the two readers differ, but the controls.
        let control = case.get("control").is_some_and(|c| c == true);
        assert_eq!(case["bytes"] != case["path"], !control, "{name}");
    }
}
