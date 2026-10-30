//! The scenes of nested containers (plan 3.13, difference (b)).

use super::{Bundle, Obj, Scene, bundle, named, res_bytes, row, serialized};

/// A bundle of two nested bundles that each hold a CAB of one name, with a
/// container row whose preload span reaches object 3: an eager load
/// registers both in directory order, so the second's is the key's value,
/// the container lists its row alone and the preload walks its table alone.
/// Under `LoadMode::Lazy` each is registered when its nested bundle is
/// reached, in either order with the same result, and the container pass
/// reaches both before it walks (review round 4: the pass walked the first
/// one's table, at a key the second's replaced after the walk had passed
/// it, and never the second's).
#[must_use]
pub fn nested_duplicates() -> Scene {
    let inner = |name: &str| {
        let cab = serialized(
            &[],
            &[
                Obj::named(1, name, &[]),
                Obj::named(3, "x3", &[]),
                Obj::asset_bundle(
                    2,
                    Bundle {
                        preload: vec![(0, 3)],
                        rows: vec![row(&format!("assets/{name}.prefab"), (0, 1), (0, 1))],
                        ..Bundle::default()
                    },
                ),
            ],
        );
        bundle(&[("CAB-DUP", cab)])
    };
    Scene {
        files: vec![named(
            "dup.bundle",
            bundle(&[("N1", inner("from-n1")), ("N2", inner("from-n2"))]),
        )],
        resolver: Vec::new(),
    }
}

/// `o.bundle`: `CAB-O` and a nested bundle `INNER` of `inner`'s entries.
fn outer_with_inner(cab_o: Vec<u8>, inner: &[(&str, Vec<u8>)]) -> (Vec<u8>, Vec<u8>) {
    named(
        "o.bundle",
        bundle(&[("CAB-O", cab_o), ("INNER", bundle(inner))]),
    )
}

/// A preload whose first and third elements name `CAB-I` in the nested
/// bundle `INNER` of another bundle, and whose second names `INNER` itself.
/// An eager load registered `CAB-I` at load, so the first element finds it
/// and `CAB-I`'s object 5 gets the row's path. Under `LoadMode::Lazy` the
/// container pass reaches `INNER` before it walks (plan 3.13 (b)), so the
/// first element finds `CAB-I` too (review round 3: the walk kept the first
/// element's miss).
#[must_use]
pub fn preload_nested_external() -> Scene {
    let f = serialized(
        &["archive:/CAB-I/CAB-I", "archive:/O/INNER"],
        &[
            Obj::named(1, "f1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 5), (2, 9), (1, 5)],
                    rows: vec![row("assets/x.prefab", (0, 3), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let o = serialized(&[], &[Obj::named(7, "o7", &[])]);
    let i = serialized(&[], &[Obj::named(5, "i5", &[])]);
    Scene {
        files: vec![
            named("f.bundle", bundle(&[("CAB-F", f)])),
            outer_with_inner(o, &[("CAB-I", i)]),
        ],
        resolver: Vec::new(),
    }
}

/// A texture whose stream names `inner`, a nested bundle of its own bundle
/// that holds `inner.resS`: UnityPy's candidates miss `inner` (an eager load
/// gives a nested bundle no key) and read `inner.resS`. Under
/// `LoadMode::Lazy` `inner` is a key, and asking for it as the first
/// candidate registers `inner.resS` for the last (review round 3: the read
/// through the texture's own bundle skipped that registration and failed).
#[must_use]
pub fn resource_in_own_nested() -> Scene {
    let o = serialized(&[], &[Obj::streamed(1, "tex", "archive:/O/inner", 0, 4)]);
    Scene {
        files: vec![named(
            "o.bundle",
            bundle(&[
                ("CAB-O", o),
                ("inner", bundle(&[("inner.resS", res_bytes(8, 40))])),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A pointer to `INNER`, a nested bundle beside the asking CAB, then one to
/// `CAB-I` inside it: the first matches the sibling (a bundle, which holds
/// no objects), the second is `find_file`'s, which finds `CAB-I` (an eager
/// load registered it). Under `LoadMode::Lazy` the first dereference names
/// the nested bundle, which registers its entries as a `get` of its name
/// does, so the second finds `CAB-I` too (plan 3.13 (b)).
#[must_use]
pub fn deref_sibling_nested() -> Scene {
    let o = serialized(
        &["archive:/O/INNER", "archive:/CAB-I/CAB-I"],
        &[Obj::named(7, "o7", &[(1, 5), (2, 5)])],
    );
    let i = serialized(&[], &[Obj::named(5, "i5", &[])]);
    Scene {
        files: vec![outer_with_inner(o, &[("CAB-I", i)])],
        resolver: Vec::new(),
    }
}

/// A texture in `CAB-I`, inside the nested bundle `INNER`, whose stream
/// names `CAB-I.resS`, `INNER`'s other entry: an eager load registered both
/// at load, so the read finds the `.resS` by name. Under `LoadMode::Lazy` a
/// reader reached through `env.objects()` (or a batch job's own handle)
/// reaches its own container before the lookup (review round 4: the read
/// missed every candidate and raised `FileNotFoundError`).
#[must_use]
pub fn resource_in_nested_cab() -> Scene {
    let i = serialized(
        &[],
        &[Obj::streamed(1, "tex", "archive:/CAB-I/CAB-I.resS", 0, 4)],
    );
    Scene {
        files: vec![named(
            "o.bundle",
            bundle(&[(
                "INNER",
                bundle(&[("CAB-I", i), ("CAB-I.resS", res_bytes(8, 40))]),
            )]),
        )],
        resolver: Vec::new(),
    }
}

/// `o.bundle` holds the nested bundle `INNER` of `CAB-I` (with a container
/// row), then `p.bundle` holds a `SerializedFile` named `INNER`: the second
/// load's `INNER` is the key's value. An eager load registered `CAB-I` at
/// the first load, so it is a key and its row is listed. Under
/// `LoadMode::Lazy` the second registration drops the first's unread slot,
/// so it reaches it first (review round 4: `CAB-I` was never registered).
#[must_use]
pub fn nested_replaced_unread() -> Scene {
    let i = serialized(
        &[],
        &[
            Obj::named(5, "i5", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    rows: vec![row("assets/i.prefab", (0, 0), (0, 5))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let p = serialized(&[], &[Obj::named(1, "p1", &[])]);
    Scene {
        files: vec![
            named("o.bundle", bundle(&[("INNER", bundle(&[("CAB-I", i)]))])),
            named("p.bundle", bundle(&[("INNER", p)])),
        ],
        resolver: Vec::new(),
    }
}

/// `resource_in_nested_cab`'s `o.bundle`, loaded a second time from the same
/// bytes as `o2.bundle` (a replay hands two files of equal bytes one
/// allocation, as a caller that loads one input twice does): an eager load
/// registers `CAB-I` and its `.resS` at each load, so the read from either
/// load's texture finds the `.resS`. Under `LoadMode::Lazy` the reload's slot
/// for `INNER` stands in for the first load's, and a reader inside the first
/// load's `INNER` reaches it through the reload's handle (review round 5: it
/// reached nothing, and the read missed every candidate).
#[must_use]
pub fn nested_reloaded() -> Scene {
    let mut scene = resource_in_nested_cab();
    let (_, data) = scene.files[0].clone();
    scene.files.push(named("o2.bundle", data));
    scene
}

/// `o.bundle` holds the nested bundle `INNER` of three raw entries, which
/// `p.bundle`'s raw entry `INNER` drops: an eager load registered the three
/// at the first load. Under `LoadMode::Lazy` the second registration drops
/// the first's unread slot, so it reaches it first (review round 5: a reach
/// of this shape that the input's meter refused was redone by every lookup
/// by name, uncounted).
#[must_use]
pub fn nested_raw_replaced() -> Scene {
    let inner = bundle(&[
        ("R0.resS", res_bytes(4, 10)),
        ("R1.resS", res_bytes(4, 20)),
        ("R2.resS", res_bytes(4, 30)),
    ]);
    Scene {
        files: vec![
            named("o.bundle", bundle(&[("INNER", inner)])),
            named("p.bundle", bundle(&[("INNER", res_bytes(4, 50))])),
        ],
        resolver: Vec::new(),
    }
}
