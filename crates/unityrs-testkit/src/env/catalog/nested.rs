//! The scenes of nested containers (plan 3.13, difference (b)).

use super::{Bundle, DepScene, Obj, Scene, bundle, named, res_bytes, row, serialized};

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

/// `o.bundle` holds the nested bundle `INNER` of `CAB-I`, whose row
/// `assets/i.prefab` has a preload span reaching object 2 of `CAB-Z`, and
/// then `CAB-Z`, whose own row's span reaches its object 2 too; it is
/// loaded a second time from the same bytes as `o2.bundle`. An eager load
/// registers `CAB-I` before `CAB-Z` at each load, so the container lists
/// `CAB-I`'s row first and its walk gives `CAB-Z`'s object 2 the path
/// `assets/i.prefab`. Under `LoadMode::Lazy` the reload's slot for `INNER`
/// stands in for the first load's, and its entries take the first load's
/// positions (review round 6: they took the reload's, after `CAB-Z`, so the
/// container listed `CAB-Z`'s row first and its walk gave object 2 the path
/// `assets/z.prefab`).
#[must_use]
pub fn nested_reloaded_spans() -> Scene {
    let i = serialized(
        &["archive:/CAB-Z/CAB-Z"],
        &[
            Obj::named(1, "i1", &[]),
            Obj::asset_bundle(
                3,
                Bundle {
                    preload: vec![(1, 2)],
                    rows: vec![row("assets/i.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let z = serialized(
        &[],
        &[
            Obj::named(2, "z2", &[]),
            Obj::named(4, "z4", &[]),
            Obj::asset_bundle(
                3,
                Bundle {
                    preload: vec![(0, 2)],
                    rows: vec![row("assets/z.prefab", (0, 1), (0, 4))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let o = bundle(&[("INNER", bundle(&[("CAB-I", i)])), ("CAB-Z", z)]);
    Scene {
        files: vec![named("o.bundle", o.clone()), named("o2.bundle", o)],
        resolver: Vec::new(),
    }
}

/// [`nested_reloaded_spans`] with `CAB-O` before `INNER` and `CAB-J` before
/// `CAB-I` inside it, so a test can stop the first load after its first
/// key, before it registers `INNER`, and stop a reach of the reload's
/// `INNER` between two of its keys. An eager load gives the container and
/// the paths of [`nested_reloaded_spans`]. Under `LoadMode::Lazy`, when the
/// first load registers its `INNER` while the reload's slot is being
/// reached, a container pass that runs once both loads have returned must
/// walk `CAB-I` before `CAB-Z` (review round 7: the reach marked the slot
/// reached and only then moved its entries to the first load's positions,
/// so a pass in between walked `CAB-Z` first and gave its object 2 the path
/// `assets/z.prefab` for good).
#[must_use]
pub fn nested_reloaded_spans_raced() -> Scene {
    let i = serialized(
        &["archive:/CAB-Z/CAB-Z"],
        &[
            Obj::named(1, "i1", &[]),
            Obj::asset_bundle(
                3,
                Bundle {
                    preload: vec![(1, 2)],
                    rows: vec![row("assets/i.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let z = serialized(
        &[],
        &[
            Obj::named(2, "z2", &[]),
            Obj::named(4, "z4", &[]),
            Obj::asset_bundle(
                3,
                Bundle {
                    preload: vec![(0, 2)],
                    rows: vec![row("assets/z.prefab", (0, 1), (0, 4))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let o_cab = serialized(&[], &[Obj::named(1, "o1", &[])]);
    let j = serialized(&[], &[Obj::named(1, "j1", &[])]);
    let o = bundle(&[
        ("CAB-O", o_cab),
        ("INNER", bundle(&[("CAB-J", j), ("CAB-I", i)])),
        ("CAB-Z", z),
    ]);
    Scene {
        files: vec![named("o.bundle", o.clone()), named("o2.bundle", o)],
        resolver: Vec::new(),
    }
}

/// `f.bundle` holds `CAB-F`; the dependency `a.bundle` holds the nested
/// bundle `INNER` of `CAB-D`, whose row `assets/d.prefab` has a preload span
/// reaching `CAB-F`'s object 1; the dependency `b.bundle` holds `CAB-B`; and
/// `p.bundle` holds a raw entry `INNER`, which drops `a.bundle`'s. An eager
/// load registered `CAB-D` at `a.bundle`'s load, so the container pass walks
/// its table and `CAB-F`'s object 1 gets the path `assets/d.prefab`. Under
/// `LoadMode::Lazy`, with options that evict `a.bundle` when `b.bundle`
/// loads, the dropped `INNER` is an evicted dependency's, which the
/// container pass opens again to reach (plan section 9, Q13, revised after
/// review round 6: nothing reached it, and the path was missing).
#[must_use]
pub fn nested_dropped_evicted() -> DepScene {
    let f = serialized(&[], &[Obj::named(1, "f1", &[])]);
    let d = serialized(
        &["archive:/CAB-F/CAB-F"],
        &[
            Obj::named(1, "d1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 1)],
                    rows: vec![row("assets/d.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let b = serialized(&[], &[Obj::named(1, "b1", &[])]);
    DepScene {
        scene: Scene {
            files: vec![
                named("f.bundle", bundle(&[("CAB-F", f)])),
                named("a.bundle", bundle(&[("INNER", bundle(&[("CAB-D", d)]))])),
                named("b.bundle", bundle(&[("CAB-B", b)])),
                named("p.bundle", bundle(&[("INNER", res_bytes(4, 50))])),
            ],
            resolver: Vec::new(),
        },
        dependencies: vec![1, 2],
    }
}

/// [`nested_dropped_evicted`] without `p.bundle`: the dependency
/// `a.bundle`'s `INNER` stays the key's value. An eager load registered
/// `CAB-D` at `a.bundle`'s load, so the container pass walks its table and
/// `CAB-F`'s object 1 gets the path `assets/d.prefab`. Under
/// `LoadMode::Lazy`, with options that evict `a.bundle` when `b.bundle`
/// loads, the container pass reaches `INNER` through its own slot, opening
/// `a.bundle` again; a pool that refuses that open, or any later read or
/// registration of the reach, makes room by evicting the other
/// dependencies and tries once more, as every other dependency read does
/// (plan section 3.13; review round 7: the reach's open failed every pass).
#[must_use]
pub fn nested_evicted() -> DepScene {
    let mut scene = nested_dropped_evicted();
    scene.scene.files.truncate(3);
    scene
}

/// Reads from inside dependencies: `d.bundle` holds `CAB-D`, its `.resS`
/// and its sibling `CAB-DE`; `n.bundle` holds the same shape inside the
/// nested bundle `INNER` (`CAB-N`, `CAB-N.resS`, `CAB-NE`); `e.bundle`
/// holds `CAB-E` and `CAB-E.resS`; all three are loaded as dependencies.
/// `CAB-D`'s and `CAB-N`'s pointers name a sibling, `CAB-E` (another
/// dependency, found by `find_file`'s registry lookup) and, from `CAB-D`,
/// `CAB-R` (only the resolver has it, lower-cased as `find_file` asks); their
/// textures read their own `.resS`, `CAB-E.resS` and, from `CAB-D`,
/// `CAB-X.resS`, which a resource fallback loads (a bundle of that name
/// whose one entry is the `.resS`). Every dereference and resource read
/// starts from a dependency's file, so each charges the dependency pool
/// (review round 8: a pool refusal there failed the call, with no eviction
/// and no retry).
#[must_use]
pub fn dependency_reads() -> DepScene {
    let d = serialized(
        &[
            "archive:/CAB-DE/CAB-DE",
            "archive:/CAB-E/CAB-E",
            "archive:/CAB-R/CAB-R",
        ],
        &[
            Obj::named(1, "d1", &[(1, 7), (2, 1), (3, 1)]),
            Obj::streamed(2, "dtex", "archive:/CAB-D/CAB-D.resS", 4, 8),
            Obj::streamed(3, "etex", "archive:/CAB-E/CAB-E.resS", 2, 4),
            Obj::streamed(4, "xtex", "archive:/CAB-X/CAB-X.resS", 1, 3),
        ],
    );
    let n = serialized(
        &["archive:/CAB-NE/CAB-NE", "archive:/CAB-E/CAB-E"],
        &[
            Obj::named(1, "n1", &[(1, 3), (2, 1)]),
            Obj::streamed(2, "ntex", "archive:/CAB-N/CAB-N.resS", 2, 6),
        ],
    );
    DepScene {
        scene: Scene {
            files: vec![
                named(
                    "d.bundle",
                    bundle(&[
                        ("CAB-D", d),
                        ("CAB-D.resS", res_bytes(64, 7)),
                        ("CAB-DE", serialized(&[], &[Obj::named(7, "de7", &[])])),
                    ]),
                ),
                named(
                    "n.bundle",
                    bundle(&[(
                        "INNER",
                        bundle(&[
                            ("CAB-N", n),
                            ("CAB-N.resS", res_bytes(32, 40)),
                            ("CAB-NE", serialized(&[], &[Obj::named(3, "ne3", &[])])),
                        ]),
                    )]),
                ),
                named(
                    "e.bundle",
                    bundle(&[
                        ("CAB-E", serialized(&[], &[Obj::named(1, "e1", &[])])),
                        ("CAB-E.resS", res_bytes(16, 90)),
                    ]),
                ),
            ],
            resolver: vec![
                named("cab-r", serialized(&[], &[Obj::named(1, "r1", &[])])),
                named("CAB-X.resS", bundle(&[("CAB-X.resS", res_bytes(8, 120))])),
            ],
        },
        dependencies: vec![0, 1, 2],
    }
}
