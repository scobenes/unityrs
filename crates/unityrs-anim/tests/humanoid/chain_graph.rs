//! The scene graph's structure below the hips (B4b): what the graph
//! dropped or found inconsistent there refuses, and nothing elsewhere in
//! the scene does.
//!
//! Each diagnostic the scene graph records, and whether it refuses inside
//! the hips' subtree (the hips included):
//!
//! | Diagnostic | Arises from | Below the hips |
//! |---|---|---|
//! | `Unresolved`, a child | an `m_Children` entry that resolves to nothing | refuses: a dropped child, `truncated_rig` |
//! | `Unresolved`, `m_Father` | a kept Transform's own `m_Father` | refuses: with its `ParentMismatch`, `misparented_transform` |
//! | `Unresolved`, a component | a `GameObject`'s component pointer | no: no Transform structure |
//! | `Builtin` | a component's builtin reference | no |
//! | `Malformed`, a child | a child that fails to read | refuses: a dropped child, `truncated_rig` |
//! | `Malformed`, a component | a component that fails to read, kept as `Other` | no |
//! | `Malformed`, a scene root | a top-level Transform whose `m_Father` cannot be read: no node | no node to touch |
//! | `Cycle`, a child | a child already reached: a duplicate entry or a loop | refuses: a dropped child, `truncated_rig` |
//! | `Cycle`, a scene root | a root already reached as someone's child | refuses where it was kept: that child's `ParentMismatch` |
//! | `Depth` | a child past the depth limit | refuses: a dropped child, `truncated_rig` |
//! | `ParentMismatch` | a kept child whose `m_Father` is not its lister | refuses: `misparented_transform` |
//!
//! A Transform whose `m_Father` names a node that does not list it is
//! never reached, records nothing and is no node: no skeleton path
//! resolves to it.
use super::{
    body_model::{field, refused, verdict},
    chain::{chain, read, rig},
};
use unityrs_anim::{BodyModel, RigPaths, humanoid::chain::ChainFeature};
use unityrs_scene::GraphOptions;
use unityrs_testkit::{
    ngui::{Object, V, pptr, st},
    scene3d::{game_object, set, transform},
};

fn items<'o>(objects: &'o mut [Object], id: i64, name: &str) -> &'o mut Vec<V> {
    let object = objects.iter_mut().find(|o| o.path_id == id).unwrap();
    let V::Vector(_, _, items) = field(&mut object.value, &[name]) else {
        panic!("{name}")
    };
    items
}

fn listed(objects: &mut [Object], game_object: i64, component: i64) {
    let pair = st(
        "ComponentPair",
        vec![("component", pptr("Component", (0, component)))],
    );
    items(objects, game_object, "m_Component").push(pair);
}

/// The Spine's Transform (109) gains child Transform 181, which fails to
/// read (its `m_GameObject` is a Transform), and, when `listed`, the
/// Spine's `GameObject` (9) lists it as a component too.
fn broken_child(objects: &mut Vec<Object>, listed_too: bool) {
    objects.push(Object::new(
        181,
        4,
        transform((0, 101), (0, 109), &[], false),
    ));
    items(objects, 109, "m_Children").push(pptr("Transform", (0, 181)));
    if listed_too {
        listed(objects, 9, 181);
    }
}

/// A child the graph dropped below the hips refuses (`truncated_rig`) and
/// leaves the chain out of scope, even when a surviving `GameObject` also
/// lists the dropped Transform as a component (kept as `Other`).
#[test]
fn a_dropped_child_below_the_hips_refuses() {
    for listed_too in [false, true] {
        let edit = |o: &mut Vec<Object>| broken_child(o, listed_too);
        let (found, _) = read(
            &rig(Vec::new()),
            GraphOptions::default(),
            false,
            edit,
            |body, paths, meter| verdict(body.rest_scales(paths, meter)),
        );
        assert_eq!(found, Err(refused(BodyModel::TruncatedRig)), "{listed_too}");
        let found = chain(&rig(Vec::new()), edit).unwrap();
        assert_eq!(found.validated(), None, "{listed_too}");
    }
}

/// A drop outside the hips' subtree leaves the scope alone: a second root
/// in the file whose chain of Transforms runs one deeper than the rig's
/// deepest bone, read to that bone's depth, loses its last child while the
/// rig stays in scope.
#[test]
fn a_drop_under_another_root_keeps_the_scope() {
    let full = rig(Vec::new());
    let depth = |paths: &RigPaths<'_>| {
        let nodes = paths.graph().0.nodes();
        let depth = |mut at: usize| {
            let mut depth = 1;
            while let Some(parent) = nodes[at].parent {
                (at, depth) = (parent, depth + 1);
            }
            depth
        };
        (0..nodes.len()).map(depth).max().unwrap()
    };
    let (deepest, _) = read(
        &full,
        GraphOptions::default(),
        false,
        |_| (),
        |_, paths, _| depth(paths),
    );
    let other = |o: &mut Vec<Object>| {
        for k in 0..=i64::from(deepest) {
            let (go, t) = (5000 + k, 5100 + k);
            let father = if k == 0 { 0 } else { t - 1 };
            let children: &[(i32, i64)] = if k < i64::from(deepest) {
                &[(0, t + 1)]
            } else {
                &[]
            };
            o.push(Object::new(go, 1, game_object(b"Other", &[(0, t)])));
            o.push(Object::new(
                t,
                4,
                transform((0, go), (0, father), children, false),
            ));
        }
    };
    let limited = GraphOptions {
        max_depth: Some(deepest),
    };
    let dropped = |paths: &RigPaths<'_>| {
        let graph = paths.graph().0;
        (0..graph.nodes().len()).any(|k| graph.dropped_child(k))
    };
    for (options, any) in [(GraphOptions::default(), false), (limited, true)] {
        let ((found, lost), _) = read(&full, options, true, other, |body, paths, meter| {
            (body.animator_chain(paths, meter), dropped(paths))
        });
        assert_eq!(lost, any);
        assert_eq!(
            found.unwrap().validated(),
            Some((ChainFeature::Plain, false))
        );
    }
}

/// A Transform below the hips whose `m_Father` disagrees with the node
/// that lists it refuses (`misparented_transform`): the Spine's `m_Father`
/// moved to the Animator node or to a missing object while the hips still
/// list it, and the hips' own moved to the Spine. The graph keeps each
/// under its lister.
#[test]
fn a_parent_mismatch_below_the_hips_refuses() {
    for (node, father) in [(109, 101), (109, 9999), (102, 109)] {
        let edit = |o: &mut Vec<Object>| {
            let object = o.iter_mut().find(|o| o.path_id == node).unwrap();
            set(
                &mut object.value,
                "m_Father",
                pptr("Transform", (0, father)),
            );
        };
        let (found, _) = read(
            &rig(Vec::new()),
            GraphOptions::default(),
            false,
            edit,
            |body, paths, meter| verdict(body.rest_scales(paths, meter)),
        );
        let want = Err(refused(BodyModel::MisparentedTransform));
        assert_eq!(found, want, "{node} {father}");
        assert_eq!(chain(&rig(Vec::new()), edit).unwrap().validated(), None);
    }
}

/// A parent mismatch under another root leaves the scope alone.
#[test]
fn a_parent_mismatch_under_another_root_keeps_the_scope() {
    let other = |o: &mut Vec<Object>| {
        o.push(Object::new(5000, 1, game_object(b"Other", &[(0, 5100)])));
        o.push(Object::new(
            5100,
            4,
            transform((0, 5000), (0, 0), &[(0, 5101)], false),
        ));
        o.push(Object::new(5001, 1, game_object(b"Child", &[(0, 5101)])));
        o.push(Object::new(
            5101,
            4,
            transform((0, 5001), (0, 101), &[], false),
        ));
    };
    let mismatched = |paths: &RigPaths<'_>| {
        let graph = paths.graph().0;
        (0..graph.nodes().len()).any(|k| graph.mismatched_parent(k))
    };
    let ((found, any), _) = read(
        &rig(Vec::new()),
        GraphOptions::default(),
        true,
        other,
        |body, paths, meter| (body.animator_chain(paths, meter), mismatched(paths)),
    );
    assert!(any);
    assert_eq!(
        found.unwrap().validated(),
        Some((ChainFeature::Plain, false))
    );
}

/// Many distinct malformed components on one node drop nothing and cost
/// linear work: the hips' `GameObject` lists `n` distinct `LODGroup`s
/// without `m_LODs`, and the chain stays in scope. The chain reads no
/// diagnostics and no components, and every step of its walk is charged
/// (one per node, the membership search per child), so its charged work
/// is its work: it is the same for every `n`. A meter cannot see work
/// left uncharged, which is why the chain's own work is pinned exactly
/// rather than bounded; the graph's read of the components is linear.
#[test]
#[allow(
    clippy::redundant_closure_for_method_calls,
    reason = "the method path is not general over the borrows' lifetimes"
)]
fn many_malformed_components_cost_linear_work() {
    let work = |n: i64| {
        let edit = |o: &mut Vec<Object>| {
            for k in 0..n {
                o.push(Object::new(6000 + k, 205, st("LODGroup", Vec::new())));
                listed(o, 2, 6000 + k);
            }
        };
        let (found, work) = read(
            &rig(Vec::new()),
            GraphOptions::default(),
            false,
            edit,
            |body, paths, meter| body.animator_chain(paths, meter),
        );
        let found = found.unwrap().validated();
        assert_eq!(found, Some((ChainFeature::Plain, false)), "{n}");
        work
    };
    let ([none, chained], [some, middle], [more, last]) = (work(0), work(1000), work(2000));
    assert_eq!([middle, last], [chained; 2]);
    let (first, second) = (some - none, more - some);
    assert!(
        first > 0 && first.abs_diff(second) <= first / 10,
        "{none} {some} {more}"
    );
}
