//! A TPK's trees all name its one string table, and a tree's compile groups
//! each class's children by name (unityrs-core's `Tree::compile`, "Work"):
//! it read again, for every release root, the strings its rows name, so a
//! TPK whose many roots each name two long strings of one length read them
//! once per root (fix pass 13's sweep of review round 13's finding). The
//! provider numbers its strings' names once, at load
//! (`unityrs_core::typetree::tree::NameTable`), and each compile reads none
//! of them; the trees are keyed as the compile keys them from the names'
//! bytes, a name the table holds twice (two entries of one content)
//! included.
//!
//!     scripts/dev cargo test -p unityrs-tpk --test names

use common::{V2, stored, v2};
use unityrs_core::limits::Limits;
use unityrs_core::provider::{TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::{NodeId, Tree};
use unityrs_tpk::TpkProvider;

mod common;

/// The length of each long string.
const L: usize = 100_000;
/// The release roots, one class each.
const ROOTS: u16 = 64;

/// A blob (`Tpk.py:232-240`) of one version, `ROOTS` classes each naming
/// its own root, no common strings, `nodes` (a type and a name string id,
/// and sub-node ids) and `strings`.
fn blob(nodes: &[(u16, u16, Vec<u16>)], roots: &[u16], strings: &[Vec<u8>]) -> Vec<u8> {
    let count = |out: &mut Vec<u8>, n: usize| {
        out.extend_from_slice(&i32::try_from(n).unwrap().to_le_bytes());
    };
    let mut out = 1_i64.to_le_bytes().to_vec();
    count(&mut out, 1);
    out.extend_from_slice(&V2.to_le_bytes());
    count(&mut out, roots.len());
    for (id, root) in (0_i32..).zip(roots) {
        out.extend_from_slice(&id.to_le_bytes());
        count(&mut out, 1);
        out.extend_from_slice(&V2.to_le_bytes());
        out.extend_from_slice(&[1, 7, 0, 0, 0, 192]);
        out.extend_from_slice(&root.to_le_bytes());
        out.extend_from_slice(&root.to_le_bytes());
    }
    count(&mut out, 0);
    count(&mut out, 0);
    count(&mut out, nodes.len());
    for (ty, name, subs) in nodes {
        // `<HHihbIH`: type, name, size -1, version 1, flags 0, meta 0.
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&name.to_le_bytes());
        out.extend_from_slice(&(-1_i32).to_le_bytes());
        out.extend_from_slice(&[1, 0, 0]);
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(&u16::try_from(subs.len()).unwrap().to_le_bytes());
        for s in subs {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    count(&mut out, strings.len());
    for s in strings {
        // A little-endian 7-bit length (`Tpk.py:445-456`).
        let mut n = s.len();
        while n >= 0x80 {
            out.push(u8::try_from(n & 0x7F).unwrap() | 0x80);
            n >>= 7;
        }
        out.push(u8::try_from(n).unwrap());
        out.extend_from_slice(s);
    }
    out
}

/// `ROOTS` roots, each `Base { int a; int a'; int b; int a }`: `a` and `a'`
/// name two entries of one long string, `b` a long string of the same
/// length.
fn provider() -> TpkProvider {
    let strings = vec![
        b"Base".to_vec(),
        b"int".to_vec(),
        vec![b'A'; L],
        vec![b'A'; L],
        vec![b'B'; L],
    ];
    let mut nodes = vec![(1, 2, vec![]), (1, 3, vec![]), (1, 4, vec![])];
    let roots: Vec<u16> = (3..3 + ROOTS).collect();
    for _ in &roots {
        nodes.push((0, 0, vec![0, 1, 2, 0]));
    }
    let tpk = stored(&blob(&nodes, &roots, &strings));
    TpkProvider::from_bytes(&tpk, &Limits::default()).unwrap()
}

/// Each root's compile reads no name: the work its list's meter (the
/// provider's) counts is unchanged by it, where a compile of the same rows
/// with no table reads the three long buffers and compares the two of one
/// content, 4 x 100,000 bytes a root. Both key the children alike: `a'`
/// takes `a`'s key and slot, `b` a slot of its own.
#[test]
fn a_trees_compile_reads_none_of_the_tpks_strings() {
    let provider = provider();
    let v = v2();
    let len = u64::try_from(L).unwrap();
    for class_id in 0..i32::from(ROOTS) {
        let list = provider.node_list(class_id, &v).unwrap().unwrap();
        let meter = list.meter().clone();
        let before = meter.decoded();
        let req = TreeRequest {
            class_id,
            version: &v,
            script: None,
        };
        let tree = provider.tree(&req).unwrap().unwrap();
        assert_eq!(meter.decoded(), before, "class {class_id}: names read");
        let plain = Tree::compile(list, &Limits::default()).unwrap();
        assert_eq!(meter.decoded() - before, 4 * len, "class {class_id}");
        for t in [&tree, &plain] {
            let info = |i: u32| *t.info(NodeId(i)).unwrap();
            let keys: Vec<_> = (1..=4).map(|i| (info(i).key.0, info(i).slot)).collect();
            assert_eq!(keys, [(1, 0), (1, 0), (3, 1), (1, 0)], "class {class_id}");
            assert!(info(0).dup_names);
        }
    }
}

/// A tree's compile costs its own rows, whatever the TPK's string table
/// holds: four classes, each a one-row root naming one of the table's
/// first four strings, over a table of those four strings and over one of
/// 65,536 (the most a TPK holds), are each granted the same bytes on the
/// provider's meter by their compile, and read no name. (Review round 14:
/// with the table's names numbered at load, each compile sized and filled
/// a table of one entry per name the TPK holds: before the fix this test
/// saw 528,656 bytes granted a one-row root's compile over 65,536 strings,
/// against 320 over four.)
#[test]
fn a_one_row_roots_compile_does_not_depend_on_the_string_tables_size() {
    let v = v2();
    let roots: Vec<u16> = (0..4).collect();
    let nodes: Vec<(u16, u16, Vec<u16>)> = roots.iter().map(|r| (*r, *r, vec![])).collect();
    let mut granted = Vec::new();
    for size in [4, usize::from(u16::MAX) + 1] {
        let strings: Vec<Vec<u8>> = (0..size).map(|i| format!("s{i:05}").into_bytes()).collect();
        let tpk = stored(&blob(&nodes, &roots, &strings));
        let provider = TpkProvider::from_bytes(&tpk, &Limits::default()).unwrap();
        let mut each = Vec::new();
        for class_id in 0..4 {
            let list = provider.node_list(class_id, &v).unwrap().unwrap();
            assert_eq!(list.len(), 1);
            let meter = list.meter().clone();
            let (before, work) = (meter.granted(), meter.decoded());
            let req = TreeRequest {
                class_id,
                version: &v,
                script: None,
            };
            provider.tree(&req).unwrap().unwrap();
            assert_eq!(meter.decoded(), work, "class {class_id}: names read");
            each.push(meter.granted() - before);
        }
        granted.push((size, each));
    }
    assert_eq!(
        granted[0].1, granted[1].1,
        "bytes granted a compile: {granted:?}"
    );
}
