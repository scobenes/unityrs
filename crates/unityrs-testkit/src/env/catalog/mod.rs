//! The scenes, one function each, and [`scenes`], which lists them by
//! name for `tools/fixtures/env.py`.

mod nested;

pub use nested::{
    deref_sibling_nested, nested_duplicates, nested_replaced_unread, preload_nested_external,
    resource_in_nested_cab, resource_in_own_nested,
};

use super::{Bundle, Obj, Scene, bundle, bundle_spec, res_bytes, row, serialized};
use crate::bundle::{BlockOverride, TestCodec};

fn named(name: &str, data: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    (name.as_bytes().to_vec(), data)
}

/// Two bundles: an object in the first points (`m_FileID` 1) at an object in
/// the second's CAB; the first also holds a `.resS` the object reads.
#[must_use]
pub fn cross_bundle_pointer() -> Scene {
    let cab_a = serialized(
        &["archive:/CAB-B/CAB-B"],
        &[
            Obj::named(1, "a1", &[(1, 7), (0, 2)]),
            Obj::streamed(2, "tex", "archive:/CAB-A/CAB-A.resS", 4, 8),
            Obj::asset_bundle(
                3,
                Bundle {
                    preload: vec![(1, 7), (0, 2)],
                    rows: vec![row("assets/a1.prefab", (0, 2), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let cab_b = serialized(&[], &[Obj::named(7, "b7", &[])]);
    Scene {
        files: vec![
            named(
                "a.bundle",
                bundle(&[("CAB-A", cab_a), ("CAB-A.resS", res_bytes(32, 0))]),
            ),
            named("b.bundle", bundle(&[("CAB-B", cab_b)])),
        ],
        resolver: Vec::new(),
    }
}

/// Two loose `SerializedFile`s loaded at top level under names with
/// directories (`a/CAB-A`, `b/CAB-B`): an object in A points (`m_FileID` 1,
/// external `archive:/CAB-B/CAB-B`) at an object in B. The pointer resolves,
/// as in UnityPy, whose Environment is a top-level file's container
/// (`find_file` finds `cab-b`); without B it is `DerefError::NotFound`.
#[must_use]
pub fn loose_external_pointer() -> Scene {
    Scene {
        files: vec![
            named(
                "a/CAB-A",
                serialized(&["archive:/CAB-B/CAB-B"], &[Obj::named(1, "a1", &[(1, 5)])]),
            ),
            named("b/CAB-B", serialized(&[], &[Obj::named(5, "b5", &[])])),
        ],
        resolver: Vec::new(),
    }
}

/// One bundle whose first CAB holds a pointer for every dereference rule:
/// null, a missing path id, `m_FileID` past the end, a negative id in range
/// and one below it, `i32::MIN`, an external nobody has, a raw sibling, a
/// sibling matched by its lower-cased name, and one found by `find_file`
/// under a key with a directory.
#[must_use]
pub fn deref_rules() -> Scene {
    let externals = [
        "archive:/CAB-RAW/CAB-RAW.resS",
        "archive:/CAB-MISSING/CAB-MISSING",
        "archive:/CAB-E2/Cab-E2",
        "archive:/elsewhere/CAB-D",
    ];
    let cab_e = serialized(
        &externals,
        &[Obj::named(
            1,
            "e1",
            &[
                (0, 0),
                (0, 99),
                (5, 1),
                (-1, 11),
                (-5, 1),
                (i32::MIN, 1),
                (2, 1),
                (1, 1),
                (3, 9),
                (3, 10),
                (4, 11),
                (0, 1),
            ],
        )],
    );
    Scene {
        files: vec![named(
            "e.bundle",
            bundle(&[
                ("CAB-E", cab_e),
                ("CAB-RAW.resS", res_bytes(16, 3)),
                ("CAB-E2", serialized(&[], &[Obj::named(9, "e9", &[])])),
                ("dir/CAB-D", serialized(&[], &[Obj::named(11, "d11", &[])])),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A bundle of a good CAB, its `.resS` and a CAB that fails to parse: the
/// load fails, `files` stays empty, and the first two are registered.
#[must_use]
pub fn bad_cab() -> Scene {
    let good = serialized(
        &[],
        &[
            Obj::named(1, "g1", &[]),
            Obj::streamed(2, "tex", "archive:/CAB-G/CAB-G.resS", 0, 4),
        ],
    );
    // An AssetBundle without `m_Container`: UnityPy's object-mode read at
    // parse (`files/SerializedFile.py:316-321`) fails the file.
    let bad = serialized(
        &[],
        &[Obj::asset_bundle(
            1,
            Bundle {
                no_container: true,
                ..Bundle::default()
            },
        )],
    );
    Scene {
        files: vec![named(
            "g.bundle",
            bundle(&[
                ("CAB-G", good),
                ("CAB-G.resS", res_bytes(16, 7)),
                ("CAB-BAD", bad),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A bundle whose second entry is a bundle under the path of the first,
/// a CAB (plan 3.13 (b)).
#[must_use]
pub fn nested_after_cab() -> Scene {
    let inner = bundle(&[("CAB-INNER", serialized(&[], &[Obj::named(2, "inner", &[])]))]);
    Scene {
        files: vec![named(
            "n.bundle",
            bundle(&[
                ("CAB-N", serialized(&[], &[Obj::named(1, "n1", &[])])),
                ("CAB-N", inner),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A bundle with a block after its last entry's bytes that fails to decode
/// (plan 3.13 (c)).
#[must_use]
pub fn last_block_fails() -> Scene {
    let mut spec = bundle_spec(&[("CAB-L", serialized(&[], &[Obj::named(1, "l1", &[(0, 1)])]))]);
    let blocks = spec
        .entries
        .iter()
        .map(|e| e.data.len())
        .sum::<usize>()
        .div_ceil(spec.block_size);
    spec.block_overrides.push(BlockOverride {
        block: blocks,
        // An id no codec has: UnityPy's `CompressionFlags(9)` raises.
        codec: Some(TestCodec::Raw(9)),
        uncompressed: Some(16),
        compressed: None,
    });
    Scene {
        files: vec![named("l.bundle", crate::bundle::build(&spec))],
        resolver: Vec::new(),
    }
}

/// A container whose preload table points every way the preload skips:
/// past the externals, at a file nobody has, at a missing object, null,
/// and rows whose spans fall outside the table.
#[must_use]
pub fn preload_skips() -> Scene {
    let skipped = serialized(
        &["archive:/CAB-NONE/CAB-NONE"],
        &[
            Obj::named(1, "p1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(0, 1), (9, 1), (1, 1), (0, 77), (0, 0)],
                    rows: vec![
                        row("assets/p1.prefab", (0, 5), (0, 1)),
                        row("assets/out.prefab", (3, 9), (0, 1)),
                        row("assets/neg.prefab", (-1, 2), (0, 1)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named("p.bundle", bundle(&[("CAB-P", skipped)]))],
        resolver: Vec::new(),
    }
}

/// A preload pointer below the externals: `IndexError`, raised.
#[must_use]
pub fn preload_below_start() -> Scene {
    let file = serialized(
        &["archive:/CAB-NONE/CAB-NONE"],
        &[
            Obj::named(1, "q1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(-3, 1)],
                    rows: vec![row("assets/q1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named("q.bundle", bundle(&[("CAB-Q", file)]))],
        resolver: Vec::new(),
    }
}

/// A preload pointer at a raw sibling: `AttributeError`, raised.
#[must_use]
pub fn preload_raw_target() -> Scene {
    let file = serialized(
        &["archive:/CAB-R/CAB-R.resS"],
        &[
            Obj::named(1, "r1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 1)],
                    rows: vec![row("assets/r1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named(
            "r.bundle",
            bundle(&[("CAB-R", file), ("CAB-R.resS", res_bytes(8, 1))]),
        )],
        resolver: Vec::new(),
    }
}

/// `AssetBundle`s read in object mode: two fields whose names clean to one
/// key (the last wins), one without `m_MainAsset` (`UnknownObject`), and
/// one without `m_Container` (the file fails).
#[must_use]
pub fn asset_bundle_modes() -> Scene {
    let shadow = serialized(
        &[],
        &[
            Obj::named(1, "s1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    rows: vec![row("assets/first.prefab", (0, 0), (0, 1))],
                    shadow_container: Some(vec![
                        row("assets/second.prefab", (0, 0), (0, 1)),
                        row("assets/third.prefab", (0, 0), (0, 2)),
                    ]),
                    ..Bundle::default()
                },
            ),
        ],
    );
    let unknown = serialized(
        &[],
        &[
            Obj::named(1, "u1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    rows: vec![row("assets/u1.prefab", (0, 0), (0, 1))],
                    no_main_asset: true,
                    ..Bundle::default()
                },
            ),
        ],
    );
    let missing = serialized(
        &[],
        &[
            Obj::named(1, "m1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    no_container: true,
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![
            named("shadow.bundle", bundle(&[("CAB-S", shadow)])),
            named("unknown.bundle", bundle(&[("CAB-U", unknown)])),
            named("missing.bundle", bundle(&[("CAB-M", missing)])),
        ],
        resolver: Vec::new(),
    }
}

/// A loaded bundle whose CAB points at, and reads a resource from, files
/// only the resolver has: `find_file` loads `cab-dep`'s bundle as a
/// dependency (a container, so the pointer has no target), whose own copy of
/// `CAB-H` then replaces the loaded one's key, and `cab-loose`, a
/// `SerializedFile` it resolves into; the resource fallback loads
/// `CAB-H2.resS` under the name it asked for, twice (the first and last
/// candidates are one name).
#[must_use]
pub fn resolver_loads() -> Scene {
    let cab_h = |name: &str| {
        serialized(
            &["archive:/CAB-DEP/CAB-DEP", "archive:/CAB-LOOSE/CAB-LOOSE"],
            &[
                Obj::named(1, name, &[(1, 3), (2, 5)]),
                Obj::streamed(2, "tex", "archive:/CAB-H2/CAB-H2.resS", 2, 6),
                Obj::asset_bundle(
                    4,
                    Bundle {
                        rows: vec![row(&format!("assets/{name}.prefab"), (0, 0), (0, 1))],
                        ..Bundle::default()
                    },
                ),
            ],
        )
    };
    let dep = bundle(&[
        ("CAB-DEP", serialized(&[], &[Obj::named(3, "dep3", &[])])),
        ("CAB-H", cab_h("h-copy")),
    ]);
    Scene {
        files: vec![named("h.bundle", bundle(&[("CAB-H", cab_h("h1"))]))],
        resolver: vec![
            // Lower-case: `find_file` asks for the cleaned (lower-cased)
            // external name, and UnityPy finds only an exact file name
            // (its walk keys a lower-cased path its open then misses).
            named("cab-dep", dep),
            named(
                "cab-loose",
                serialized(&[], &[Obj::named(5, "loose5", &[])]),
            ),
            named("CAB-H2.resS", res_bytes(24, 9)),
        ],
    }
}

/// A loose file whose preload pointer names a CAB only the resolver has:
/// the container pass's `find_file` registers it, and UnityPy's loop over
/// `env.cabs` raises `RuntimeError` (the dict changed size); the next pass
/// ends.
#[must_use]
pub fn preload_loads_new_cab() -> Scene {
    let file = serialized(
        &["archive:/CAB-NEW/CAB-NEW"],
        &[
            Obj::named(1, "x1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 3)],
                    rows: vec![row("assets/x1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named("x/CAB-X", file)],
        resolver: vec![named(
            "cab-new",
            serialized(&[], &[Obj::named(3, "new3", &[])]),
        )],
    }
}

/// Container paths (`obj.container`): rows whose spans overlap (the first
/// row to reach an object gives its path), a row's own asset (which beats
/// any span), two rows of one asset (the last wins), and a span reaching an
/// object in a sibling CAB.
#[must_use]
pub fn preload_paths() -> Scene {
    let pp = serialized(
        &["archive:/CAB-PQ/CAB-PQ"],
        &[
            Obj::named(1, "p1", &[]),
            Obj::named(2, "p2", &[]),
            Obj::named(3, "p3", &[]),
            Obj::named(4, "p4", &[]),
            Obj::named(5, "p5", &[]),
            Obj::asset_bundle(
                9,
                Bundle {
                    preload: vec![(0, 1), (0, 2), (0, 3), (1, 7), (0, 4)],
                    rows: vec![
                        row("assets/a.prefab", (0, 3), (0, 1)),
                        row("assets/b.prefab", (1, 4), (0, 2)),
                        row("assets/c.prefab", (0, 0), (0, 5)),
                        row("assets/d.prefab", (0, 0), (0, 5)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let pq = serialized(&[], &[Obj::named(7, "q7", &[])]);
    Scene {
        files: vec![named(
            "pp.bundle",
            bundle(&[("CAB-PP", pp), ("CAB-PQ", pq)]),
        )],
        resolver: Vec::new(),
    }
}

/// A container row whose asset is in another file (`m_FileID` 1): UnityPy
/// keys its path by path id alone, so the object of that path id in the
/// container's own file gets it; the core keys only a row whose asset is in
/// the container's own file (`PARITY.md` section 19.2), so neither file's
/// object does.
#[must_use]
pub fn container_other_file() -> Scene {
    let of = serialized(
        &["archive:/CAB-OG/CAB-OG"],
        &[
            Obj::named(1, "of1", &[]),
            Obj::asset_bundle(
                9,
                Bundle {
                    rows: vec![row("assets/other.prefab", (0, 0), (1, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let og = serialized(&[], &[Obj::named(1, "og1", &[])]);
    Scene {
        files: vec![named(
            "of.bundle",
            bundle(&[("CAB-OF", of), ("CAB-OG", og)]),
        )],
        resolver: Vec::new(),
    }
}

/// A resource read's fallback loads, through the resolver, a file whose
/// object points (`m_FileID` 1) at a CAB of the loaded bundle. UnityPy's
/// `load_file` parses the fallback's file with the Environment as its
/// container (`environment.py:107-108`, `:154`), so the pointer resolves as
/// a top-level file's does.
#[must_use]
pub fn fallback_file_pointer() -> Scene {
    let f = serialized(
        &["LOOSE-T"],
        &[Obj::streamed(1, "tex", "archive:/CAB-R/CAB-R.resS", 0, 4)],
    );
    let t = serialized(&[], &[Obj::named(5, "t5", &[])]);
    Scene {
        files: vec![named("f.bundle", bundle(&[("CAB-F", f), ("CAB-T", t)]))],
        resolver: vec![named(
            "LOOSE-T",
            serialized(&["archive:/CAB-T/CAB-T"], &[Obj::named(4, "l4", &[(1, 5)])]),
        )],
    }
}

/// A preload whose externals UnityPy's `find_file` reaches two ways: `H`
/// (`xab.assets`) by the registry, where a caller's copy holds the key, and
/// `A` (`ab.assets`) by its walk's suffix match (`environment.py:345-354`),
/// which loads the resolver's `xab.assets` under that name, so the name
/// asked misses again: every dereference of `A` loads a new copy, which
/// takes the key, and the next dereference of `H` finds that copy. One row
/// spans `H`, `A`, `H`, `A` (path ids 5, 5, 6, 7): the caller's copy gets 5
/// alone, the last copy 7 alone (review round 2: the walk kept its first
/// answer for each external).
#[must_use]
pub fn preload_suffix_external() -> Scene {
    let x = |prefix: &str| {
        serialized(
            &[],
            &[
                Obj::named(5, &format!("{prefix}5"), &[]),
                Obj::named(6, &format!("{prefix}6"), &[]),
                Obj::named(7, &format!("{prefix}7"), &[]),
            ],
        )
    };
    let r = serialized(
        &["archive:/CAB-B/xab.assets", "archive:/CAB-B/ab.assets"],
        &[
            Obj::named(1, "r1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 5), (2, 5), (1, 6), (2, 7)],
                    rows: vec![row("assets/r.prefab", (0, 4), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![
            // Under the key `xab.assets`, as the resolver's file.
            named("caller/xab.assets", x("caller")),
            named("r.bundle", bundle(&[("CAB-R", r)])),
        ],
        // Only `xab.assets`: the walk's `ab.assets` matches it by suffix.
        resolver: vec![named("xab.assets", x("found"))],
    }
}

/// Two bundles that each hold a CAB of one name, `CAB-S`, with its own
/// `.resS`, container row and object 7; the first also holds `CAB-X`, which
/// points at `CAB-S`'s object 7. UnityPy's one registry keeps the later
/// copy under each key, so a resource read from either copy's texture
/// reads the later `.resS`, and the container lists only the later row;
/// a pointer matches the asking file's own bundle first
/// (`classes/PPtr.py:75-78`), so `CAB-X` reads its own bundle's object 7
/// (`PARITY.md` section 19.2, same-named CABs).
#[must_use]
pub fn same_named_cabs() -> Scene {
    let cab_s = |name: &str| {
        serialized(
            &[],
            &[
                Obj::named(7, &format!("{name}7"), &[]),
                Obj::streamed(2, &format!("tex-{name}"), "archive:/CAB-S/CAB-S.resS", 0, 4),
                Obj::asset_bundle(
                    3,
                    Bundle {
                        rows: vec![row(&format!("assets/{name}.prefab"), (0, 0), (0, 7))],
                        ..Bundle::default()
                    },
                ),
            ],
        )
    };
    let x = serialized(&["archive:/CAB-S/CAB-S"], &[Obj::named(1, "x1", &[(1, 7)])]);
    let a = bundle(&[
        ("CAB-X", x),
        ("CAB-S", cab_s("a")),
        ("CAB-S.resS", res_bytes(8, 10)),
    ]);
    let b = bundle(&[("CAB-S", cab_s("b")), ("CAB-S.resS", res_bytes(8, 200))]);
    Scene {
        files: vec![named("a.bundle", a), named("b.bundle", b)],
        resolver: Vec::new(),
    }
}

/// Every scene, by name: what `tools/fixtures/env.py` records.
#[must_use]
pub fn scenes() -> Vec<(&'static str, Scene)> {
    vec![
        ("cross_bundle_pointer", cross_bundle_pointer()),
        ("loose_external_pointer", loose_external_pointer()),
        ("deref_rules", deref_rules()),
        ("bad_cab", bad_cab()),
        ("nested_after_cab", nested_after_cab()),
        ("last_block_fails", last_block_fails()),
        ("preload_skips", preload_skips()),
        ("preload_below_start", preload_below_start()),
        ("preload_raw_target", preload_raw_target()),
        ("asset_bundle_modes", asset_bundle_modes()),
        ("resolver_loads", resolver_loads()),
        ("preload_loads_new_cab", preload_loads_new_cab()),
        ("preload_paths", preload_paths()),
        ("container_other_file", container_other_file()),
        ("fallback_file_pointer", fallback_file_pointer()),
        ("nested_duplicates", nested_duplicates()),
        ("preload_suffix_external", preload_suffix_external()),
        ("same_named_cabs", same_named_cabs()),
        ("preload_nested_external", preload_nested_external()),
        ("resource_in_own_nested", resource_in_own_nested()),
        ("deref_sibling_nested", deref_sibling_nested()),
        ("resource_in_nested_cab", resource_in_nested_cab()),
        ("nested_replaced_unread", nested_replaced_unread()),
    ]
}

#[cfg(test)]
mod tests {
    use super::scenes;

    #[test]
    fn every_scene_has_files_with_distinct_names() {
        for (name, scene) in scenes() {
            assert!(!scene.files.is_empty(), "{name}");
            let mut names: Vec<_> = scene.files.iter().map(|f| &f.0).collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), scene.files.len(), "{name}");
        }
    }
}
