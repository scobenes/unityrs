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
