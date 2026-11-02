// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ContainerHelper.py, UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! The scenes whose container preload walk races a load another thread
//! makes (review round 14): `tools/fixtures/env.py` records UnityPy's walk
//! (`helpers/ContainerHelper.py:31-49`) with the load made before the walk,
//! before each element of each row's span it looks at, after each row, and
//! after the walk, and the core's walk is stopped at each point it reaches
//! while another thread makes the load. In each, the other file replaces a
//! CAB a scene file registered (`RaceScene`'s docs).

use super::named;
use super::revisit::preload_revisits_after_load;
use crate::env::{Bundle, Obj, RaceScene, Scene, bundle, row, serialized};

/// `c.bundle` with `CAB-A` holding object 4 only, then `f.bundle`, whose
/// `CAB-F` has `externals` and the preload table `preload`, and the rows
/// `assets/<k>.prefab` spanning `spans` in turn.
fn walked(externals: &[&str], preload: Vec<(i32, i64)>, spans: &[(i32, i32)]) -> Scene {
    let rows = spans
        .iter()
        .enumerate()
        .map(|(k, &span)| row(&format!("assets/{k}.prefab"), span, (0, 1)))
        .collect();
    let f = serialized(
        externals,
        &[
            Obj::named(1, "f1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload,
                    rows,
                    ..Bundle::default()
                },
            ),
        ],
    );
    let caller = serialized(&[], &[Obj::named(4, "caller4", &[])]);
    Scene {
        files: vec![
            named("c.bundle", bundle(&[("CAB-A", caller)])),
            named("f.bundle", bundle(&[("CAB-F", f)])),
        ],
        resolver: Vec::new(),
    }
}

/// `d.bundle`, whose `CAB-<cab>` holds `objects` (path id, name).
fn other(cab: &str, objects: &[(i64, &str)]) -> (Vec<u8>, Vec<u8>) {
    let objects: Vec<Obj> = objects
        .iter()
        .map(|&(id, n)| Obj::named(id, n, &[]))
        .collect();
    named(
        "d.bundle",
        bundle(&[(&format!("CAB-{cab}"), serialized(&[], &objects))]),
    )
}

/// A change another thread makes is seen at a later row by an index the
/// walk dereferenced before it: the table is `CAB-A`'s 5 and 6, row 0
/// spans the first, row 1 the second, row 2 the first. With the other
/// file's `CAB-A` (5 and 6) loaded before row 1, UnityPy's row 1 gives 6
/// its path and row 2 gives 5 its own (the walk had kept its row-0 miss of
/// 5 and dereferenced it no more).
#[must_use]
pub fn race_seen_at_a_later_row() -> RaceScene {
    RaceScene {
        scene: walked(
            &["archive:/CAB-A/CAB-A"],
            vec![(1, 5), (1, 6)],
            &[(0, 1), (1, 1), (0, 1)],
        ),
        other: other("A", &[(5, "other5"), (6, "other6")]),
    }
}

/// Another thread's change of a key, then the walk's own (review round 14,
/// area d's probe): the table is `CAB-A`'s 5, `t`'s 1 and `a`'s 9, row 0
/// spans all three, row 1 the first. `t` is found nowhere; `a` is matched
/// by suffix in `env.path` (`environment.py:345-354`), whose `cab-a` (5
/// and 9) takes the key, so UnityPy's row 1 finds its 5 whatever the other
/// file (`CAB-A` with 7) replaced before (the walk lost its index to the
/// other thread's change, and put back nothing at its own).
#[must_use]
pub fn race_before_the_walks_own_load() -> RaceScene {
    let mut scene = walked(
        &["archive:/CAB-A/CAB-A", "t", "a"],
        vec![(1, 5), (2, 1), (3, 9)],
        &[(0, 3), (0, 1)],
    );
    let found = serialized(
        &[],
        &[Obj::named(5, "found5", &[]), Obj::named(9, "found9", &[])],
    );
    scene.resolver = vec![named("cab-a", found)];
    RaceScene {
        scene,
        other: other("A", &[(7, "other7")]),
    }
}

/// Two externals of one name, so their lookups read one key: the table is
/// the first's 5, the second's 5 and the first's 6, in one row. With the
/// other file's `CAB-A` (5 and 6) loaded before the second element,
/// UnityPy's second and third elements find it (the walk's second lookup
/// found the change while the first external's kept miss still held, its
/// flag set only after the change was published).
#[must_use]
pub fn race_two_externals_one_key() -> RaceScene {
    RaceScene {
        scene: walked(
            &["archive:/CAB-A/CAB-A", "archive:/CAB-A/CAB-A"],
            vec![(1, 5), (2, 5), (1, 6)],
            &[(0, 3)],
        ),
        other: other("A", &[(5, "other5"), (6, "other6")]),
    }
}

/// The scene `preload_revisits_after_load` with another copy of `CAB-Q`
/// (5 and 6) loaded at each point (review round 14, area x's probe): which
/// copy row `b` finds depends only on whether the other load comes before
/// or after the walk's own load of `env.path`'s `cab-q`.
#[must_use]
pub fn race_around_the_walks_own_load() -> RaceScene {
    RaceScene {
        scene: preload_revisits_after_load(),
        other: other("Q", &[(5, "c2_5"), (6, "c2_6")]),
    }
}

/// A miss, and a change another thread makes while the walk asks the
/// resolver for the next element: the table is `CAB-A`'s 5 and `t`'s 1,
/// row 0 spans both, row 1 the first. UnityPy's row 1 finds the other
/// file's 5 (the walk dereferenced the first element no more).
#[must_use]
pub fn race_in_a_lookup() -> RaceScene {
    RaceScene {
        scene: walked(
            &["archive:/CAB-A/CAB-A", "t"],
            vec![(1, 5), (2, 1)],
            &[(0, 2), (0, 1)],
        ),
        other: other("A", &[(5, "other5")]),
    }
}

/// Every race scene, by name: what `tools/fixtures/env.py` records.
#[must_use]
pub fn race_scenes() -> Vec<(&'static str, RaceScene)> {
    vec![
        ("race_seen_at_a_later_row", race_seen_at_a_later_row()),
        (
            "race_before_the_walks_own_load",
            race_before_the_walks_own_load(),
        ),
        ("race_two_externals_one_key", race_two_externals_one_key()),
        (
            "race_around_the_walks_own_load",
            race_around_the_walks_own_load(),
        ),
        ("race_in_a_lookup", race_in_a_lookup()),
    ]
}
