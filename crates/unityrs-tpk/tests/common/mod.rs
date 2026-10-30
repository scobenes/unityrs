//! TPKs built in code for the tests that check a bound UnityPy does not
//! have (`PARITY.md` section 19.3), where what is expected is the
//! provider's documented bound, not an outcome of UnityPy's.

#![allow(dead_code)]

use unityrs_core::limits::Limits;
use unityrs_core::provider::ProviderError;
use unityrs_core::{LimitError, UnityVersion};
use unityrs_tpk::{TpkError, TpkProvider};

/// The default [`Limits`] with a tree depth of `depth`.
pub fn limits(depth: u32) -> Limits {
    let mut limits = Limits::default();
    limits.max_tree_depth = depth;
    limits
}

/// A type-tree blob, written as `Tpk.py` reads one (`:232-240`): its
/// versions, classes (an id and its entries, each a version and a release
/// root, flags 192 with the editor root the same), common strings, nodes
/// (sub-node ids) and strings (`s0`, `s1`, ...).
#[derive(Default)]
pub struct Blob {
    pub versions: Vec<u64>,
    pub classes: Vec<(i32, Vec<(u64, u16)>)>,
    pub common: Vec<(u64, u8)>,
    pub indices: Vec<u16>,
    pub nodes: Vec<Vec<u16>>,
    pub strings: usize,
}

/// Where each count of a written blob is.
#[derive(Default)]
pub struct Offsets {
    pub versions: usize,
    pub classes: usize,
    pub common: usize,
    pub indices: usize,
    pub nodes: usize,
    pub strings: usize,
}

pub const V2: u64 = 2 << 48;

impl Blob {
    /// Class 1 at 2.0.0, its release root node 0, over `nodes`.
    pub fn class_1(nodes: Vec<Vec<u16>>) -> Self {
        Blob {
            versions: vec![V2],
            classes: vec![(1, vec![(V2, 0)])],
            nodes,
            strings: 2,
            ..Blob::default()
        }
    }

    pub fn write(&self) -> (Vec<u8>, Offsets) {
        fn count(out: &mut Vec<u8>, n: usize) -> usize {
            let at = out.len();
            out.extend_from_slice(&i32::try_from(n).unwrap().to_le_bytes());
            at
        }
        let mut out = 1_i64.to_le_bytes().to_vec();
        let mut at = Offsets {
            versions: count(&mut out, self.versions.len()),
            ..Offsets::default()
        };
        for v in &self.versions {
            out.extend_from_slice(&v.to_le_bytes());
        }
        at.classes = count(&mut out, self.classes.len());
        for (id, entries) in &self.classes {
            out.extend_from_slice(&id.to_le_bytes());
            count(&mut out, entries.len());
            for (v, root) in entries {
                out.extend_from_slice(&v.to_le_bytes());
                out.extend_from_slice(&[1, 7, 0, 0, 0, 192]);
                out.extend_from_slice(&root.to_le_bytes());
                out.extend_from_slice(&root.to_le_bytes());
            }
        }
        at.common = count(&mut out, self.common.len());
        for (v, n) in &self.common {
            out.extend_from_slice(&v.to_le_bytes());
            out.push(*n);
        }
        at.indices = count(&mut out, self.indices.len());
        for i in &self.indices {
            out.extend_from_slice(&i.to_le_bytes());
        }
        at.nodes = count(&mut out, self.nodes.len());
        for subs in &self.nodes {
            // `<HHihbIH`: s0 s1, size -1, version 1, flags 0, meta 0x4000.
            out.extend_from_slice(&[0, 0, 1, 0]);
            out.extend_from_slice(&(-1_i32).to_le_bytes());
            out.extend_from_slice(&[1, 0, 0]);
            out.extend_from_slice(&0x4000_u32.to_le_bytes());
            out.extend_from_slice(&u16::try_from(subs.len()).unwrap().to_le_bytes());
            for s in subs {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        at.strings = count(&mut out, self.strings);
        for i in 0..self.strings {
            let s = format!("s{i}");
            out.push(u8::try_from(s.len()).unwrap());
            out.extend_from_slice(s.as_bytes());
        }
        (out, at)
    }
}

/// A stored TPK around `payload` (`Tpk.py:146-174`).
pub fn stored(payload: &[u8]) -> Vec<u8> {
    let size = u32::try_from(payload.len()).unwrap().to_le_bytes();
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(payload);
    out
}

pub fn load(blob: &Blob, limits: &Limits) -> Result<TpkProvider, TpkError> {
    TpkProvider::from_bytes(&stored(&blob.write().0), limits)
}

pub fn v2() -> UnityVersion {
    UnityVersion::from_packed(V2)
}

pub fn limit_of(e: &ProviderError) -> LimitError {
    e.limit.unwrap_or_else(|| panic!("not a limit: {e}"))
}

/// The tables of a TPK's blob, as [`TpkError::TooLarge`] names their bounds.
const TABLES: [&str; 7] = [
    "versions",
    "classes",
    "class entries",
    "common-string versions",
    "common-string indices",
    "nodes",
    "strings",
];

/// Whether `e` is the provider's error for the exception `python` that
/// UnityPy 1.25.2's walk raised (`generate_node`, `helpers/Tpk.py:58-88`):
/// G7's porting rule, the same for every case. `NODES[node_id]` or
/// `StringBuffer[...]` past its table raises `IndexError`, which the port
/// gives as an error of no bound naming the table it ran past. Every other
/// exception, and every other error, is no match: a walk error G7 meets that
/// this rule does not list fails G7, so a new pair is a decision, not a
/// pass (round 4: any error passed where UnityPy recorded one).
#[must_use]
pub fn walk_error_is_unitypys(python: &str, e: &ProviderError) -> bool {
    let past_table = [
        "IndexError: a node id past the node buffer",
        "IndexError: a string id past the string buffer",
    ];
    match python {
        "IndexError" => {
            e.provider == TpkProvider::NAME && e.limit.is_none() && past_table.contains(&&*e.detail)
        }
        _ => false,
    }
}

/// Whether `e` is the port's refusal of a TPK on which UnityPy 1.25.2's
/// load (`TpkFile(stream).GetDataBlob()`, `helpers/Tpk.py:145-240`) raised
/// `python`: G7's porting rule for a load, the same for every case. Each
/// exception maps to the errors the port gives where `Tpk.py` raises it; a
/// table's bound ([`TpkError::TooLarge`]) refuses a count before the reads
/// UnityPy runs out on, so it stands for the exception those reads raise.
/// Every other pair is no match. [`TpkError::NotTypeTrees`] stands for
/// whatever a blob of data type 1-5 raises; G7 checks the data type itself
/// (`tests/g7.rs`).
#[must_use]
pub fn load_error_is_unitypys(python: &str, e: &TpkError) -> bool {
    let table = matches!(e, TpkError::TooLarge { what, .. } if TABLES.contains(what));
    match (python, e) {
        // `struct.error`: a read past the payload, or a negative count read
        // as a struct format; `ValueError`: a string or a length read past
        // the payload (a short LZMA payload among them), or a compression or
        // data type that is not a member; `UnicodeDecodeError`: a string's
        // bytes run past the payload and decode short; `IndexError`: a byte
        // read past the payload (`stream.read(1)[0]`: an entry's presence, a
        // common-string version's count); `Exception`: `Tpk.py`'s own raise
        // on the header (the magic, the version, the compressed size).
        // And a data type of 1-5 (`TpkDataType.ToBlob`, `:113-124`): a
        // collection blob's reads (`error`), a file system or JSON blob that
        // `init`'s assertion refuses (`AssertionError`), and `ToBlob`'s own
        // raise for the other two (`Exception`).
        (
            "error" | "ValueError" | "UnicodeDecodeError" | "IndexError" | "Exception",
            TpkError::Malformed(_),
        )
        | ("error" | "AssertionError" | "Exception", TpkError::NotTypeTrees) => true,
        (
            "error" | "ValueError" | "UnicodeDecodeError" | "IndexError",
            TpkError::TooLarge { .. },
        ) => table,
        // python-lz4 and liblzma, by the class the port's message names;
        // `decompress_lzma`'s unpack of the properties (`struct.error`).
        ("LZ4BlockError" | "OverflowError", TpkError::Decompress { codec, detail }) => {
            *codec == "lz4" && detail.split(':').next() == Some(python)
        }
        ("LZMAError", TpkError::Decompress { codec, detail }) => {
            *codec == "lzma" && detail.split(':').next() == Some(python)
        }
        ("error", TpkError::Decompress { codec, detail }) => {
            *codec == "lzma" && detail.starts_with("struct.error: ")
        }
        _ => false,
    }
}

/// Asserts that `got` is UnityPy's load outcome for case `name`: loaded
/// where `python` is `None`, else refused with the error the port gives for
/// that exception ([`load_error_is_unitypys`]).
#[track_caller]
pub fn assert_load_is_unitypys(
    name: &str,
    python: Option<&str>,
    got: &Result<TpkProvider, TpkError>,
) {
    match (python, got) {
        (None, Ok(_)) => {}
        (Some(python), Err(e)) => assert!(
            load_error_is_unitypys(python, e),
            "{name}: UnityPy raised {python}, unityrs {e:?}"
        ),
        (python, got) => panic!("{name}: UnityPy {python:?}, unityrs {got:?}"),
    }
}
