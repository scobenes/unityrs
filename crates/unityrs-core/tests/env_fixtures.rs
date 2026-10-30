//! Package D's oracle fixtures, replayed (plan section 7.5 and G5): every
//! testkit Environment scene, run through `tools/fixtures/env.py`'s steps,
//! compared step by step with what UnityPy 1.25.2 recorded under `Eager`
//! (but the one step `PARITY.md` section 19.2 decides otherwise, pinned
//! here), and `Lazy` compared with `Eager` (identical on every scene but the
//! ones the plan's differences (a)-(c) name, which `tests/env/modes.rs` pins); and
//! `str.lower`, `ntpath` and `reSplit` against Python.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "env_fixtures/evicted.rs"]
mod evicted;
#[path = "env_fixtures/lazy.rs"]
mod lazy;
#[path = "env_fixtures/refused.rs"]
mod refused;
#[path = "env_fixtures/run.rs"]
mod run;

use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::RawStr;
use unityrs_core::env::{EnvOptions, LoadMode};
use unityrs_core::limits::Meter;
use unityrs_core::pystr::{
    is_split_path, ntpath_basename, ntpath_splitext, py_lower, simplify_name,
};
use unityrs_testkit::env::{dependency_scenes, scenes};

use run::{Run, hex, load, ops, recorded, scene_by_name, text, unhex};

/// The fixture's scenes are the testkit's, byte for byte, its dependency
/// scenes too. With `UNITYRS_WRITE_SCENES=<path>` it writes them there
/// instead, as JSON lines, a dependency scene's with its `dependencies`
/// (`tools/fixtures/env.py` reads them).
#[test]
fn scenes_are_the_testkit_scenes() {
    let all = scenes();
    let deps = dependency_scenes();
    let pairs = |v: &[(Vec<u8>, Vec<u8>)]| -> Vec<J> {
        v.iter().map(|(n, d)| json!([text(n), hex(d)])).collect()
    };
    if let Ok(out) = std::env::var("UNITYRS_WRITE_SCENES") {
        let mut lines = String::new();
        for (n, s) in &all {
            let row = json!({"name": n, "files": pairs(&s.files), "resolver": pairs(&s.resolver)});
            lines.push_str(&row.to_string());
            lines.push('\n');
        }
        for (n, d) in &deps {
            let s = &d.scene;
            let row = json!({"name": n, "files": pairs(&s.files), "resolver": pairs(&s.resolver),
                "dependencies": d.dependencies});
            lines.push_str(&row.to_string());
            lines.push('\n');
        }
        std::fs::write(out, lines).unwrap();
        return;
    }
    let doc = load("env.json").expect("tests/fixtures/d/env.json");
    let check = |cases: &J, all: Vec<(&str, &unityrs_testkit::env::Scene)>| {
        let cases = cases.as_array().unwrap();
        assert_eq!(cases.len(), all.len());
        for (c, (n, s)) in cases.iter().zip(all) {
            assert_eq!(c["name"].as_str().unwrap(), n);
            assert_eq!(
                c["files"],
                json!(pairs(&s.files)),
                "{n}: the builder changed"
            );
            assert_eq!(
                c["resolver"],
                json!(pairs(&s.resolver)),
                "{n}: the builder changed"
            );
        }
    };
    check(&doc["scenes"], all.iter().map(|(n, s)| (*n, s)).collect());
    check(
        &doc["dependency_scenes"],
        deps.iter().map(|(n, d)| (*n, &d.scene)).collect(),
    );
    for (c, (n, d)) in doc["dependency_scenes"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&deps)
    {
        assert_eq!(c["dependencies"], json!(d.dependencies), "{n}");
    }
}

/// The steps whose result `PARITY.md` decides otherwise than UnityPy, by
/// scene and op: the core's `path_dict` keys a row by its asset's file and
/// path id, UnityPy's by path id alone (section 19.2), which
/// [`the_path_dict_keys_a_row_by_its_assets_file`] pins.
const EAGER_DIFFERS: &[(&str, &str)] = &[("container_other_file", "paths")];

/// A step without its result, for the pinned steps above.
fn state_only(step: &J) -> J {
    let mut s = step.clone();
    s["result"] = J::Null;
    s
}

/// G5's scene half: under `Eager` (UnityPy's mode, `EnvOptions::unitypy`)
/// every step of every scene ends in UnityPy's state with UnityPy's result
/// (a pinned step's state only).
#[test]
fn eager_runs_every_scene_as_unitypy_does() {
    let mut differences = Vec::new();
    for (name, steps) in recorded() {
        let run = Run::new(scene_by_name(&name), LoadMode::Eager);
        let ours = run.steps(&ops(&steps));
        for (want, got) in steps.iter().zip(&ours) {
            let pinned = EAGER_DIFFERS
                .iter()
                .any(|(n, op)| *n == name && want["op"][0] == *op);
            let (want, got) = if pinned {
                (&state_only(want), &state_only(got))
            } else {
                (want, got)
            };
            if want != got {
                differences.push(format!(
                    "{name}: {}\n  unitypy {want}\n  core    {got}",
                    want["op"]
                ));
                break;
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// `PARITY.md` section 19.2's `path_dict` row: UnityPy gives the object of
/// the row's path id in the container's own file the path of a row whose
/// asset is in another file; the core gives it to neither file's object.
#[test]
fn the_path_dict_keys_a_row_by_its_assets_file() {
    let (_, steps) = recorded()
        .into_iter()
        .find(|(n, _)| n == "container_other_file")
        .unwrap();
    let ops = ops(&steps);
    let at = ops.iter().position(|op| op[0] == "paths").unwrap();
    // `env.objects`: CAB-OF's object 1 and its AssetBundle, CAB-OG's object 1.
    assert_eq!(
        steps[at]["result"],
        json!(["assets/other.prefab", null, null])
    );
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let ours = Run::new(scene_by_name("container_other_file"), mode).steps(&ops);
        assert_eq!(ours[at]["result"], json!([null, null, null]), "{mode:?}");
    }
}

/// `PARITY.md` section 19.2's same-named CABs: P1 implements UnityPy's one
/// registry (Tier A) under every option set, `EnvOptions::batch()`'s
/// included, in both modes; the Tier C rule is not implemented. Scene
/// `same_named_cabs`, step for step as UnityPy recorded it: a resource read
/// from either bundle's `CAB-S` reads the later bundle's `.resS`, the
/// container lists the later bundle's row only, and a pointer from the
/// first bundle's `CAB-X` reads that bundle's own object 7.
#[test]
fn same_named_cabs_resolve_as_unitypys_under_every_option_set() {
    let (_, steps) = recorded()
        .into_iter()
        .find(|(n, _)| n == "same_named_cabs")
        .unwrap();
    let ops = ops(&steps);
    let at = |op: &str| ops.iter().position(|o| o[0] == op).unwrap();
    assert_eq!(
        steps[at("derefs")]["result"],
        json!([["CAB-S", 7, 1, "a7"]])
    );
    assert_eq!(
        steps[at("resources")]["result"],
        json!(["c8c9cacb", "c8c9cacb"])
    );
    assert_eq!(
        steps[at("container")]["result"],
        json!([["assets/b.prefab", 0, 0, 0, 7]])
    );
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let batch = EnvOptions::batch(Arc::new(unityrs_core::FileContext::unitypy()));
        let ours = Run::with(scene_by_name("same_named_cabs"), mode, batch).steps(&ops);
        assert_eq!(ours, steps, "{mode:?}");
    }
}

/// The scenes on which `Lazy` differs from `Eager`, each by one of plan
/// 3.13's differences (`tests/env/modes.rs` pins what each difference is).
const LAZY_DIFFERS: &[&str] = &[
    // (a): the failing entry fails only the reads that parse it.
    "bad_cab",
    "asset_bundle_modes",
    // (b): a nested container's own name is a CAB key.
    "nested_after_cab",
    "nested_duplicates",
    // (b), in the keys alone ([`LAZY_KEYS_DIFFER`]).
    "preload_nested_external",
    "resource_in_own_nested",
    "deref_sibling_nested",
    "resource_in_nested_cab",
    "nested_replaced_unread",
    "nested_reloaded",
    "nested_raw_replaced",
    "nested_reloaded_spans",
    // (c): the block after the last entry is never decoded.
    "last_block_fails",
];

/// `Lazy` is `Eager` on every well-formed scene, step for step.
#[test]
fn lazy_is_eager_on_every_well_formed_scene() {
    for (name, steps) in recorded() {
        if LAZY_DIFFERS.contains(&name.as_str()) {
            continue;
        }
        let ops = ops(&steps);
        let eager = Run::new(scene_by_name(&name), LoadMode::Eager).steps(&ops);
        let lazy = Run::new(scene_by_name(&name), LoadMode::Lazy).steps(&ops);
        assert_eq!(eager, lazy, "{name}");
    }
}

/// The scenes on which `Lazy` differs from UnityPy in the keys alone, by
/// difference (b), with the nested container whose own name is a key.
const LAZY_KEYS_DIFFER: &[(&str, &str)] = &[
    ("preload_nested_external", "inner"),
    ("resource_in_own_nested", "inner"),
    ("deref_sibling_nested", "inner"),
    ("resource_in_nested_cab", "inner"),
    ("nested_replaced_unread", "inner"),
    ("nested_reloaded", "inner"),
    ("nested_raw_replaced", "inner"),
    ("nested_reloaded_spans", "inner"),
];

/// Difference (b) and nothing more on [`LAZY_KEYS_DIFFER`]'s scenes, under
/// `EnvOptions::unitypy()` and `batch()`: every step's result, `files` and
/// `objects` are UnityPy's; its keys, the nested container's own name out
/// of both lists (a later load's CAB of that path is a key of UnityPy's),
/// are UnityPy's, in order, the container's entries missing until a step
/// reaches it, and all of them at the end. A nested container is reached
/// without a `get` of its name: by the container pass, as a resource
/// candidate read through the reader's own bundle, as the asking file's
/// sibling, from a file inside it, or by the registration that drops it
/// (review rounds 3 and 4: none of these registered its entries, so a path,
/// a read's bytes and a container row UnityPy gives were missing); a
/// reload's slot that stands in for the dropped one registers them at the
/// dropped one's positions (scene `nested_reloaded_spans`; review round 6:
/// at the reload's, so the container listed the rows in another order and
/// an object's path was another row's).
#[test]
fn lazy_reaches_what_unitypy_reaches_through_a_nested_container() {
    let keys = |step: &J| step["cabs"].as_array().unwrap().clone();
    for (name, nested) in LAZY_KEYS_DIFFER {
        let (_, steps) = recorded().into_iter().find(|(n, _)| n == name).unwrap();
        let ops = ops(&steps);
        let batch = EnvOptions::batch(Arc::new(unityrs_core::FileContext::unitypy()));
        for options in [EnvOptions::unitypy(), batch] {
            let ours = Run::with(scene_by_name(name), LoadMode::Lazy, options).steps(&ops);
            for (want, got) in steps.iter().zip(&ours) {
                let op = &want["op"];
                for part in ["result", "files", "objects"] {
                    assert_eq!(got[part], want[part], "{name} {op} {part}");
                }
                // A subsequence of UnityPy's keys, once `nested` is out.
                let mut theirs = keys(want)
                    .into_iter()
                    .filter(|k| k.as_str() != Some(nested));
                let ok = keys(got)
                    .iter()
                    .filter(|k| k.as_str() != Some(nested))
                    .all(|k| theirs.any(|t| t == *k));
                assert!(ok, "{name} {op}: keys {} of {}", got["cabs"], want["cabs"]);
            }
            let (want, got) = (keys(steps.last().unwrap()), keys(ours.last().unwrap()));
            let out = |k: Vec<J>| -> Vec<J> {
                k.into_iter()
                    .filter(|k| k.as_str() != Some(nested))
                    .collect()
            };
            let (want, got) = (out(want), out(got));
            assert_eq!(got, want, "{name}: the keys at the end");
        }
    }
}

/// Every scene that differs does differ (so the list above stays honest).
#[test]
fn the_scenes_listed_as_differing_do_differ() {
    for (name, steps) in recorded() {
        if !LAZY_DIFFERS.contains(&name.as_str()) {
            continue;
        }
        let ops = ops(&steps);
        let eager = Run::new(scene_by_name(&name), LoadMode::Eager).steps(&ops);
        let lazy = Run::new(scene_by_name(&name), LoadMode::Lazy).steps(&ops);
        assert_ne!(eager, lazy, "{name}");
    }
}

// ------------------------------------------------------------------ pystr

fn lowered(s: &str) -> Vec<u8> {
    py_lower(&RawStr::new(s.as_bytes().to_vec()), &Meter::unlimited())
        .unwrap()
        .as_bytes()
        .to_vec()
}

/// `py_lower` is Python 3.11's `str.lower` on every BMP scalar (those it
/// changes, and every other left alone), the astral samples, and each
/// final-sigma context.
#[test]
fn py_lower_is_python_lower_on_every_bmp_code_point() {
    let doc = load("pystr.json").expect("tests/fixtures/d/pystr.json");
    let changed: std::collections::HashMap<u32, String> = doc["bmp_changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                u32::try_from(r[0].as_u64().unwrap()).unwrap(),
                r[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut wrong = Vec::new();
    for cp in 0..0x1_0000_u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let want = changed
            .get(&cp)
            .cloned()
            .unwrap_or_else(|| hex(c.to_string().as_bytes()));
        let got = hex(&lowered(&c.to_string()));
        if got != want {
            wrong.push(format!("U+{cp:04X}: python {want}, core {got}"));
        }
    }
    for r in doc["astral"].as_array().unwrap() {
        let cp = u32::try_from(r[0].as_u64().unwrap()).unwrap();
        let c = char::from_u32(cp).unwrap();
        let got = hex(&lowered(&c.to_string()));
        if got != r[1].as_str().unwrap() {
            wrong.push(format!("U+{cp:04X}: python {}, core {got}", r[1]));
        }
    }
    for r in doc["sigma"].as_array().unwrap() {
        let s = String::from_utf8(unhex(r[0].as_str().unwrap())).unwrap();
        let got = hex(&lowered(&s));
        if got != r[1].as_str().unwrap() {
            wrong.push(format!("{s:?}: python {}, core {got}", r[1]));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `is_split_path` is `reSplit.match(path) is not None`.
#[test]
fn split_paths_are_unitypys() {
    let doc = load("pystr.json").expect("tests/fixtures/d/pystr.json");
    for r in doc["split"].as_array().unwrap() {
        let p = unhex(r[0].as_str().unwrap());
        assert_eq!(is_split_path(&p), r[1].as_bool().unwrap(), "{:?}", text(&p));
    }
}

/// `reSplit`'s `\d` is Python's decimal digit, on every code point: a
/// numeric character that is no decimal digit (`²`, `Ⅰ`) ends no split
/// name (review round 2: the core took any numeric character).
#[test]
fn split_digits_are_pythons_decimal_digits() {
    let doc = load("pystr.json").expect("tests/fixtures/d/pystr.json");
    let ranges: Vec<(u32, u32)> = doc["split_digits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let at = |i: usize| u32::try_from(r[i].as_u64().unwrap()).unwrap();
            (at(0), at(1))
        })
        .collect();
    assert!(ranges.len() > 1, "more than ASCII's digits");
    let mut wrong = Vec::new();
    let mut name = b"a.split".to_vec();
    for c in (0..0x11_0000_u32).filter_map(char::from_u32) {
        let cp = u32::from(c);
        let want = ranges.iter().any(|&(a, b)| (a..=b).contains(&cp));
        name.truncate(7);
        name.extend_from_slice(c.to_string().as_bytes());
        if is_split_path(&name) != want {
            wrong.push(format!("U+{cp:04X}: python {want}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `ntpath.basename`, `ntpath.splitext` and `simplify_name` as Python's.
#[test]
fn ntpath_is_pythons() {
    let doc = load("pystr.json").expect("tests/fixtures/d/pystr.json");
    for r in doc["ntpath"].as_array().unwrap() {
        let p = r[0].as_str().unwrap();
        let base = ntpath_basename(p.as_bytes());
        assert_eq!(text(base), r[1].as_str().unwrap(), "basename {p:?}");
        let (root, ext) = ntpath_splitext(base);
        assert_eq!(json!([text(root), text(ext)]), r[2], "splitext {p:?}");
        let simple =
            simplify_name(&RawStr::new(p.as_bytes().to_vec()), &Meter::unlimited()).unwrap();
        assert_eq!(
            text(simple.as_bytes()),
            r[3].as_str().unwrap(),
            "simplify {p:?}"
        );
    }
}
