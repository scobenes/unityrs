// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ContainerHelper.py, UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! The scenes where a container preload's row reaches a table index an
//! earlier row dereferenced, after a load of the walk's own changed what
//! that index's external resolves to (review round 13): UnityPy
//! dereferences every row's whole span (`helpers/ContainerHelper.py:40-47`),
//! so the later row's dereference finds the file the load put under the
//! key.

use super::named;
use crate::env::{Bundle, Obj, Scene, bundle, row, serialized};

/// The review's case. `CAB-B`'s table is `CAB-Q`'s 6, then `q`'s 5; row
/// `a` spans both, row `b` the first alone. At row `a`'s first index
/// `cab-q` is the caller's copy, which has no object 6 (`KeyError`,
/// skipped); its second, `q`, is matched by suffix in `env.path`
/// (`environment.py:345-354`), whose `cab-q` replaces the caller's under
/// that key and gets row `a`'s path for its 5. UnityPy's row `b`
/// dereferences the first index again and finds the new copy, whose 6 gets
/// row `b`'s path.
#[must_use]
pub fn preload_revisits_after_load() -> Scene {
    let b = serialized(
        &["archive:/CAB-Q/CAB-Q", "q"],
        &[
            Obj::named(1, "b1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 6), (2, 5)],
                    rows: vec![
                        row("assets/a.prefab", (0, 2), (0, 1)),
                        row("assets/b.prefab", (0, 1), (0, 1)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let caller = serialized(&[], &[Obj::named(5, "caller5", &[])]);
    let found = serialized(
        &[],
        &[Obj::named(5, "found5", &[]), Obj::named(6, "found6", &[])],
    );
    Scene {
        files: vec![
            named("b.bundle", bundle(&[("CAB-B", b)])),
            named("c.bundle", bundle(&[("CAB-Q", caller)])),
        ],
        // Only `cab-q`: the walk's `q` matches it by suffix.
        resolver: vec![named("cab-q", found)],
    }
}

/// A load in a row puts back an index of the same row that an earlier row
/// dereferenced, ahead of the load, and one behind it. `CAB-B`'s table is
/// `CAB-Q`'s 6, `q`'s 5 and `CAB-Q`'s 7; row `a` spans the last alone, row
/// `b` all three, row `c` the first alone. Row `a` finds 7 in the caller's
/// `CAB-Q`; row `b` misses 6 there, then `q` loads `env.path`'s `cab-q` by
/// suffix (`environment.py:345-354`), which takes the key, so row `b`'s
/// third index finds the new copy's 7, and row `c`'s first its 6.
#[must_use]
pub fn preload_revisits_ahead() -> Scene {
    let b = serialized(
        &["archive:/CAB-Q/CAB-Q", "q"],
        &[
            Obj::named(1, "b1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 6), (2, 5), (1, 7)],
                    rows: vec![
                        row("assets/a.prefab", (2, 1), (0, 1)),
                        row("assets/b.prefab", (0, 3), (0, 1)),
                        row("assets/c.prefab", (0, 1), (0, 1)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let caller = serialized(&[], &[Obj::named(7, "caller7", &[])]);
    let found = serialized(
        &[],
        &[
            Obj::named(5, "found5", &[]),
            Obj::named(6, "found6", &[]),
            Obj::named(7, "found7", &[]),
        ],
    );
    Scene {
        files: vec![
            named("b.bundle", bundle(&[("CAB-B", b)])),
            named("c.bundle", bundle(&[("CAB-Q", caller)])),
        ],
        // Only `cab-q`: the walk's `q` matches it by suffix.
        resolver: vec![named("cab-q", found)],
    }
}
