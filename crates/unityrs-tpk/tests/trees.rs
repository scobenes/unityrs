//! What the provider builds and keeps, however many classes a TPK holds and
//! however many of them an input asks for (`PARITY.md` section 19.3).
//!
//! UnityPy's `NODES_CACHE` keeps one tree per distinct class record
//! (`helpers/Tpk.py:58-62`, `:86-87`; `TpkUnityClass` hashes by value,
//! `:316-330`), and the provider keeps one per release root, the one field of
//! the record `generate_node` reads: every class that names a root shares
//! its tree. UnityPy has no bound on the trees it builds in total; the
//! provider's is [`TpkProvider::MAX_TOTAL_ROWS`], decided at load in the
//! order of the roots' ids, so what a request gets never depends on what was
//! asked before it.

use std::collections::HashSet;
use std::sync::Arc;

use common::{Blob, V2, limit_of, load, v2};
use unityrs_core::limits::Limits;
use unityrs_core::provider::{ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::Tree;
use unityrs_core::{LimitError, LimitKind};
use unityrs_tpk::TpkProvider;

mod common;

/// Nodes `first..first + levels`, each listing the next twice, the last a
/// leaf: the first's tree has `2^levels - 1` rows.
fn doubling(first: u16, levels: u16) -> Vec<Vec<u16>> {
    (0..levels)
        .map(|i| {
            if i + 1 < levels {
                vec![first + i + 1; 2]
            } else {
                vec![]
            }
        })
        .collect()
}

fn ask(provider: &TpkProvider, class_id: i32) -> Result<Option<Arc<Tree>>, ProviderError> {
    let v = v2();
    provider.tree(&TreeRequest {
        class_id,
        version: &v,
        script: None,
    })
}

/// The review's TPK: 65,536 classes (the most a TPK may list), each one
/// entry naming node 0, whose tree is 65,535 rows. UnityPy builds one tree
/// for them all (one record); so does the provider, where each entry's own
/// tree would have been 65,536 x 65,535 rows.
#[test]
fn every_class_that_names_one_release_root_shares_its_tree() {
    let classes = TpkProvider::MAX_RECORDS;
    let blob = Blob {
        versions: vec![V2],
        classes: (0..i32::try_from(classes).unwrap())
            .map(|id| (id, vec![(V2, 0)]))
            .collect(),
        nodes: doubling(0, 16),
        strings: 2,
        ..Blob::default()
    };
    let provider = load(&blob, &Limits::default()).unwrap();
    let first = ask(&provider, 0).unwrap().unwrap();
    assert_eq!(first.list().rows().len(), (1 << 16) - 1);
    let list = provider.node_list(0, &v2()).unwrap().unwrap();
    for id in 0..i32::try_from(classes).unwrap() {
        let tree = ask(&provider, id).unwrap().unwrap();
        assert!(Arc::ptr_eq(&tree, &first), "class {id}");
        let rows = provider.node_list(id, &v2()).unwrap().unwrap();
        assert!(Arc::ptr_eq(&rows, &list), "class {id}");
    }
}

/// 65,520 classes, each with its own release root whose tree is 65,535 rows
/// (it lists a 15-level doubling node twice): 4,293,853,200 rows in
/// all, which UnityPy would build for an input that asks for every class.
/// The provider keeps the trees of the roots in the order of their ids
/// while their rows stay within [`TpkProvider::MAX_TOTAL_ROWS`], and refuses
/// every root after, before it walks it, with a `Count` limit naming the
/// total it would have reached. Asked for every class, it holds no more
/// than the bound; asked for the last class first, it refuses it just the
/// same.
#[test]
fn the_trees_of_every_release_root_together_stop_at_the_total() {
    let shared = 0_u16;
    let mut nodes = doubling(shared, 15);
    let first_root = u16::try_from(nodes.len()).unwrap();
    let roots = TpkProvider::MAX_RECORDS - nodes.len();
    nodes.extend((0..roots).map(|_| vec![shared, shared]));
    let ids = 0..i32::try_from(roots).unwrap();
    let blob = Blob {
        versions: vec![V2],
        classes: ids
            .clone()
            .map(|id| (id, vec![(V2, first_root + u16::try_from(id).unwrap())]))
            .collect(),
        nodes,
        strings: 2,
        ..Blob::default()
    };
    let per_tree = (1_usize << 16) - 1;
    let fit = TpkProvider::MAX_TOTAL_ROWS / per_tree;
    let max = u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap();
    let refused = |kept: usize| LimitError {
        kind: LimitKind::Count,
        value: u64::try_from(kept * per_tree + per_tree).unwrap(),
        max,
    };

    // The last class first: refused, before anything was built.
    let provider = load(&blob, &Limits::default()).unwrap();
    let final_class = ids.end - 1;
    assert_eq!(
        limit_of(&provider.node_list(final_class, &v2()).unwrap_err()),
        refused(fit)
    );
    assert_eq!(
        limit_of(&ask(&provider, final_class).unwrap_err()),
        refused(fit)
    );

    // Every class: the first `fit` roots' trees, then refusals.
    let mut held = HashSet::new();
    let mut rows = 0;
    for id in ids {
        match provider.node_list(id, &v2()) {
            Ok(Some(list)) => {
                assert!(usize::try_from(id).unwrap() < fit, "class {id} built");
                if held.insert(Arc::as_ptr(&list) as usize) {
                    rows += list.rows().len();
                }
            }
            Err(e) => {
                assert!(usize::try_from(id).unwrap() >= fit, "class {id}: {e}");
                assert_eq!(limit_of(&e), refused(fit), "class {id}");
            }
            Ok(None) => panic!("class {id}: no tree"),
        }
    }
    assert_eq!(held.len(), fit);
    assert_eq!(rows, fit * per_tree);
    assert!(rows <= TpkProvider::MAX_TOTAL_ROWS);
}

/// A root whose walk fails takes nothing from the total: after 31 trees of
/// 65,535 rows (2,031,585), a tree past the row bound, a node that lists
/// itself, a chain past the depth bound and an id past the node table are
/// refused for their own reasons, and the last root's tree, which fits only
/// if none of them counted, is built; asked first, so the total decided it
/// at load.
#[test]
fn a_root_whose_walk_fails_takes_nothing_from_the_total() {
    let mut nodes = doubling(0, 15);
    let first = u16::try_from(nodes.len()).unwrap();
    nodes.extend((0..31).map(|_| vec![0, 0]));
    let over_rows = u16::try_from(nodes.len()).unwrap();
    nodes.push(vec![0; 5]);
    let cycle = over_rows + 1;
    nodes.push(vec![cycle]);
    let deep = cycle + 1;
    nodes.extend((0..300).map(|i| vec![deep + i + 1]));
    nodes.push(vec![]);
    let past = u16::try_from(nodes.len()).unwrap();
    nodes.push(vec![u16::MAX]);
    let fits = past + 1;
    nodes.push(vec![0, 0]);
    let roots: Vec<u16> = (first..first + 31)
        .chain([over_rows, cycle, deep, past, fits])
        .collect();
    let blob = Blob {
        versions: vec![V2],
        classes: (0_i32..)
            .zip(&roots)
            .map(|(id, &root)| (id, vec![(V2, root)]))
            .collect(),
        nodes,
        strings: 2,
        ..Blob::default()
    };
    let provider = load(&blob, &Limits::default()).unwrap();
    let list = provider.node_list(35, &v2()).unwrap().unwrap();
    assert_eq!(list.rows().len(), (1 << 16) - 1);
    let kind = |id| limit_of(&provider.node_list(id, &v2()).unwrap_err()).kind;
    assert_eq!(kind(31), LimitKind::Count);
    assert_eq!(kind(32), LimitKind::TreeDepth);
    assert_eq!(kind(33), LimitKind::TreeDepth);
    let e = provider.node_list(34, &v2()).unwrap_err();
    assert_eq!(e.limit, None, "{e}");
}

/// A failure the provider keeps is handed to every later request as a clone
/// that shares its text, so repeated requests allocate nothing: the walk's
/// errors from the node list and the tree alike, and a refusal of the total.
#[test]
fn a_kept_failure_is_shared_by_every_request_not_copied() {
    // Class 0: a 300-deep chain, past a depth of 1; class 1: an id past the
    // node table.
    let mut nodes: Vec<Vec<u16>> = (1..300).map(|i| vec![i]).collect();
    nodes.push(vec![]);
    nodes.push(vec![u16::MAX]);
    let blob = Blob {
        versions: vec![V2],
        classes: vec![(0, vec![(V2, 0)]), (1, vec![(V2, 300)])],
        nodes,
        strings: 2,
        ..Blob::default()
    };
    let provider = load(&blob, &common::limits(1)).unwrap();
    for class in [0, 1] {
        let (a, b) = (
            provider.node_list(class, &v2()).unwrap_err(),
            provider.node_list(class, &v2()).unwrap_err(),
        );
        assert!(
            std::ptr::eq(a.detail.as_ptr(), b.detail.as_ptr()),
            "class {class}: {a}"
        );
        let (a, b) = (
            ask(&provider, class).unwrap_err(),
            ask(&provider, class).unwrap_err(),
        );
        assert!(
            std::ptr::eq(a.detail.as_ptr(), b.detail.as_ptr()),
            "class {class}: {a}"
        );
    }
}
