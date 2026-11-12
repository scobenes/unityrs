//! What the provider builds and keeps, however many classes a TPK holds and
//! however many of them an input asks for (`PARITY.md` section 19.3).
//!
//! UnityPy's `NODES_CACHE` keeps one tree per distinct class record
//! (`helpers/Tpk.py:58-62`, `:86-87`; `TpkUnityClass` hashes by value,
//! `:316-330`), and the provider keeps one per release root, the one field of
//! the record `generate_node` reads: every class that names a root shares
//! its tree. UnityPy has no bound on the trees it builds in total; the
//! provider's is [`TpkProvider::MAX_TOTAL_ROWS`], on the rows its walks
//! build, kept or not, decided at load in the order of the roots' ids, so
//! what a request gets never depends on what was asked before it.

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

/// 65,521 classes, each with its own release root whose tree is 65,535 rows
/// (it lists a 15-level doubling node twice): 4,293,918,735 rows in
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
    assert_eq!(
        (roots, roots * per_tree),
        (65_521, 4_293_918_735),
        "the docs"
    );
    let fit = TpkProvider::MAX_TOTAL_ROWS / per_tree;
    let max = u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap();
    let refused = |kept: usize| {
        LimitError::refused(
            LimitKind::Count,
            u64::try_from(kept * per_tree + per_tree).unwrap(),
            max,
        )
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

/// A root whose walk fails takes from the total the most rows its walk may
/// build before it fails (its tree's rows, at most one past the row bound):
/// after 31 trees of 65,535 rows (2,031,585), a 301-row chain past the depth
/// bound and a root that names an id past the node table (2 rows) are
/// walked and fail for their own reasons; a node that lists itself, whose
/// walk may build 65,537 rows, would take the total past the bound and is
/// refused unwalked; a leaf after it still fits; and a tree of 65,535 rows
/// after that, which would fit if a failing walk counted nothing, is
/// refused. The last is asked first, so the total decided it at load.
#[test]
fn a_root_whose_walk_fails_counts_the_rows_its_walk_may_build() {
    let mut nodes = doubling(0, 15);
    let first = u16::try_from(nodes.len()).unwrap();
    nodes.extend((0..31).map(|_| vec![0, 0]));
    let deep = u16::try_from(nodes.len()).unwrap();
    nodes.extend((0..300).map(|i| vec![deep + i + 1]));
    nodes.push(vec![]);
    let past = u16::try_from(nodes.len()).unwrap();
    nodes.push(vec![u16::MAX]);
    let cycle = past + 1;
    nodes.push(vec![cycle]);
    let leaf = cycle + 1;
    nodes.push(vec![]);
    let tree = leaf + 1;
    nodes.push(vec![0, 0]);
    let roots: Vec<u16> = (first..first + 31)
        .chain([deep, past, cycle, leaf, tree])
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
    let total = |value: u64| {
        LimitError::refused(
            LimitKind::Count,
            value,
            u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap(),
        )
    };
    let before = 31 * 65_535 + 301 + 2;
    assert_eq!(before, 2_031_888);
    assert_eq!(
        limit_of(&provider.node_list(35, &v2()).unwrap_err()),
        total(before + 1 + 65_535)
    );
    assert_eq!(
        limit_of(&provider.node_list(33, &v2()).unwrap_err()),
        total(before + 65_537)
    );
    assert_eq!(
        provider.node_list(34, &v2()).unwrap().unwrap().rows().len(),
        1
    );
    let depth = limit_of(&provider.node_list(31, &v2()).unwrap_err());
    assert_eq!(depth.kind(), LimitKind::TreeDepth);
    let e = provider.node_list(32, &v2()).unwrap_err();
    assert_eq!(e.limit, None, "{e}");
    for id in 0..31 {
        let list = provider.node_list(id, &v2()).unwrap().unwrap();
        assert_eq!(list.rows().len(), 65_535, "class {id}");
    }
}

/// The review's TPK (round 3): nodes 0-16 a doubling chain, whose tree is
/// 131,071 rows, past the row bound, and 65,519 more nodes each listing node
/// 0, each the release root of a class of its own. Every walk fails, each
/// after building up to one past the row bound (65,537 rows): about
/// 4.3 x 10^9 rows if every root were walked, where UnityPy builds 131,072
/// for each. The provider walks the first 31 (at most 2,031,647 rows, within
/// [`TpkProvider::MAX_TOTAL_ROWS`]), each failing past the row bound, and
/// refuses every root after them unwalked with the total's `Count` limit,
/// the last even when it is asked first.
#[test]
fn failing_walks_together_build_no_more_than_the_total() {
    let mut nodes = doubling(0, 17);
    let first = u16::try_from(nodes.len()).unwrap();
    let roots = TpkProvider::MAX_RECORDS - nodes.len();
    nodes.extend((0..roots).map(|_| vec![0]));
    let ids = 0..i32::try_from(roots).unwrap();
    let blob = Blob {
        versions: vec![V2],
        classes: ids
            .clone()
            .map(|id| (id, vec![(V2, first + u16::try_from(id).unwrap())]))
            .collect(),
        nodes,
        strings: 2,
        ..Blob::default()
    };
    let per_walk = TpkProvider::MAX_TREE_ROWS + 1;
    let walked = TpkProvider::MAX_TOTAL_ROWS / per_walk;
    assert_eq!((roots, walked), (65_519, 31), "the docs");
    let total = LimitError::refused(
        LimitKind::Count,
        u64::try_from((walked + 1) * per_walk).unwrap(),
        u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap(),
    );
    let provider = load(&blob, &Limits::default()).unwrap();
    let last = ids.end - 1;
    assert_eq!(
        limit_of(&provider.node_list(last, &v2()).unwrap_err()),
        total
    );
    let row_bound = u64::try_from(TpkProvider::MAX_TREE_ROWS).unwrap();
    let mut own = 0;
    for id in ids {
        let e = limit_of(&provider.node_list(id, &v2()).unwrap_err());
        if usize::try_from(id).unwrap() < walked {
            assert_eq!(
                (e.kind(), e.max()),
                (LimitKind::Count, row_bound),
                "class {id}"
            );
            own += 1;
        } else {
            assert_eq!(e, total, "class {id}");
        }
    }
    assert_eq!(own, walked);
}

/// A class listed twice keeps its last listing (a Python dict), and only
/// the roots of kept listings take from the total. Class 0's first listing
/// names a 65,535-row tree no kept listing names, its second a leaf; 32
/// more classes each name a tree of 65,535 rows, so the kept trees are
/// 2,097,121 rows, within the bound, and the dropped listing's tree would
/// take the last of them past it. Every kept tree is built, the last asked
/// first.
#[test]
fn a_class_listed_twice_takes_only_its_kept_listings_root_from_the_total() {
    let mut nodes = doubling(0, 15);
    let dropped = u16::try_from(nodes.len()).unwrap();
    nodes.push(vec![0, 0]);
    let leaf = dropped + 1;
    nodes.push(vec![]);
    let first = leaf + 1;
    nodes.extend((0..32).map(|_| vec![0, 0]));
    let mut classes = vec![(0, vec![(V2, dropped)])];
    classes.extend((1_u16..=32).map(|id| (i32::from(id), vec![(V2, first + id - 1)])));
    classes.push((0, vec![(V2, leaf)]));
    let blob = Blob {
        versions: vec![V2],
        classes,
        nodes,
        strings: 2,
        ..Blob::default()
    };
    const {
        assert!(32 * 65_535 < TpkProvider::MAX_TOTAL_ROWS);
        assert!(33 * 65_535 + 1 > TpkProvider::MAX_TOTAL_ROWS);
    }
    let provider = load(&blob, &Limits::default()).unwrap();
    assert_eq!(provider.class_count(), 33);
    for id in (1..=32).rev() {
        let list = provider.node_list(id, &v2()).unwrap().unwrap();
        assert_eq!(list.rows().len(), 65_535, "class {id}");
    }
    let list = provider.node_list(0, &v2()).unwrap().unwrap();
    assert_eq!(list.rows().len(), 1);
}

/// Classes 0-31, each the release root of a tree of 65,536 rows (it lists a
/// 15-level doubling node twice and a leaf: `1 + 2 x 32,767 + 1`), 2,097,152
/// rows in all, and with `one_more`, class 32, whose root is a leaf after
/// them, one row more.
fn roots_at_the_total(one_more: bool) -> Blob {
    let mut nodes = doubling(0, 15);
    let leaf = u16::try_from(nodes.len()).unwrap();
    nodes.push(vec![]);
    let first = leaf + 1;
    nodes.extend((0..32).map(|_| vec![0, 0, leaf]));
    let mut roots: Vec<u16> = (first..first + 32).collect();
    if one_more {
        roots.push(u16::try_from(nodes.len()).unwrap());
        nodes.push(vec![]);
    }
    const { assert!(32 * 65_536 == TpkProvider::MAX_TOTAL_ROWS) };
    Blob {
        versions: vec![V2],
        classes: (0_i32..)
            .zip(roots)
            .map(|(id, root)| (id, vec![(V2, root)]))
            .collect(),
        nodes,
        strings: 2,
        ..Blob::default()
    }
}

/// The total is a bound on rows past it, not at it (round 9: refusing at
/// the bound passed every test): release roots whose rows total exactly
/// [`TpkProvider::MAX_TOTAL_ROWS`] all load, the last asked first.
#[test]
fn release_roots_totalling_exactly_the_bound_all_load() {
    let provider = load(&roots_at_the_total(false), &Limits::default()).unwrap();
    for id in (0..32).rev() {
        let list = provider.node_list(id, &v2()).unwrap().unwrap();
        assert_eq!(list.rows().len(), 65_536, "class {id}");
    }
}

/// [`release_roots_totalling_exactly_the_bound_all_load`]'s mirror: one row
/// more, a leaf root after the 32 trees, is refused unwalked with a `Count`
/// limit naming the total it would have reached, 2,097,153, asked first;
/// the root before it still loads.
#[test]
fn one_row_past_the_bound_refuses_the_root_that_passes_it() {
    let provider = load(&roots_at_the_total(true), &Limits::default()).unwrap();
    assert_eq!(
        limit_of(&provider.node_list(32, &v2()).unwrap_err()),
        LimitError::refused(
            LimitKind::Count,
            2_097_153,
            u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap()
        )
    );
    let list = provider.node_list(31, &v2()).unwrap().unwrap();
    assert_eq!(list.rows().len(), 65_536);
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
