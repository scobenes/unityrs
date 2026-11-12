//! The cutter's failures and its metered paths, whose answers the spec
//! gives and the reference does not record (`docs/BATCH_SPEC.md` 7.3's
//! `inputs` and `skips` rows, section 3's answer 12, section 5's sheet
//! answers; `docs/plans/batch.md` 2.6 and 2.7): the failure scenes
//! (`unityrs_testkit::ngui::failure_scenes`), and a scene cut under a
//! bounded budget or meter, swept until every charge of the cut has been
//! the one refused.

use std::collections::BTreeSet;
use std::sync::Arc;

use unityrs_core::FileContext;
use unityrs_core::budget::{Counting, Reservation, Unbounded};
use unityrs_core::env::EnvOptions;
use unityrs_core::limits::Meter;
use unityrs_export::ngui::{Failure, OutputKind, SkipOutput};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, PropName, SceneBundle, atlas, failure_scenes, file, format, material,
    mono_script, scenes, sheet, sprite, texture, texture_streamed,
};

mod common;
use common::{
    Engine, Ran, big_sheet, refused_at, refused_in, refused_with_stack, run_in, run_with, site,
};
use unityrs_core::RawStr;
use unityrs_export::ngui::Skip;

fn scene(name: &str) -> AtlasScene {
    scenes()
        .unwrap()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

fn failed(f: &Failure) -> (&str, Option<&str>) {
    match f {
        Failure::Input(r) => (r.reason.as_str(), r.detail.as_deref()),
        Failure::Run(r) => panic!("a run failure: {r:?}"),
        Failure::Stopped => panic!("stopped"),
    }
}

#[test]
fn failing_inputs_fail_as_the_spec_names_them() {
    for (name, s) in failure_scenes().unwrap() {
        let got = run_with(&s, "x.bundle", FileContext::default(), &Meter::unlimited());
        let f = got.err().unwrap_or_else(|| panic!("{name}: cut"));
        let (reason, detail) = failed(&f);
        match name {
            "member_fails" => {
                assert_eq!(
                    reason, "dependency_failed(d.bundle, container_error)",
                    "{name}"
                );
                assert!(
                    detail.is_some_and(|d| !d.contains(' ')),
                    "{name}: {detail:?}"
                );
            }
            "member_not_a_bundle" => {
                assert_eq!(
                    (reason, detail),
                    ("dependency_failed(d.bundle, not_a_bundle)", None)
                );
            }
            "input_not_a_bundle" => assert_eq!((reason, detail), ("not_a_bundle", None)),
            "reach_fails" | "selection_fails" => {
                assert_eq!(reason, "entry_error", "{name}");
                assert!(
                    detail.is_some_and(|d| d.ends_with(" CAB-n")),
                    "{name}: {detail:?}"
                );
            }
            other => panic!("{other}: no answer"),
        }
    }
}

#[test]
fn a_repeated_member_id_is_refused() {
    use unityrs_core::Bytes;
    use unityrs_export::ngui::{self, Member, View};
    let s = scene("material_in_dependency");
    let mut members = common::view_of(&s, "x.bundle");
    let dep = members.iter().find(|m| m.id != "x.bundle").unwrap().clone();
    members.push(Member {
        id: dep.id.clone(),
        data: Bytes::from_static(b"other"),
    });
    let x = members.iter().find(|m| m.id == "x.bundle").unwrap().clone();
    let view = View {
        input: &x,
        members: &members,
    };
    let env = EnvOptions::batch(Arc::new(FileContext::default()));
    let admit = ngui::Admit {
        env: &env,
        meter: &Meter::unlimited(),
        hooks: &ngui::NoHooks,
        max_evictions: u64::MAX,
    };
    let got = ngui::register_bytes(view, admit);
    assert!(matches!(got, Err(Failure::Run(r)) if r.reason == "internal"));
}

#[test]
fn a_dependency_budgets_refusal_fails_the_input() {
    let s = scene("cleaned_names");
    let mut context = FileContext::default();
    context.budget = Arc::new(Counting::new(64));
    let f = run_with(&s, "x.bundle", context, &Meter::unlimited()).unwrap_err();
    assert_eq!(failed(&f), ("budget(dependency)", None));
}

/// What one cut under a bounded meter showed.
#[derive(Default)]
struct Seen {
    /// The kinds of skip rows: `(class, output)`.
    rows: BTreeSet<(String, &'static str)>,
    /// A sheet's decode refused: its texture and its atlas skipped for it.
    decode: bool,
    /// The texture's PNG alone refused: the texture skipped, its atlas's
    /// crops written.
    png: bool,
}

fn look(cut: &Ran, reason: &str, seen: &mut Seen) {
    assert!(
        cut.skips.iter().all(|s| s.reason.reason == reason),
        "{:?}",
        cut.skips
    );
    cut.assert_units();
    for s in &cut.skips {
        seen.rows.insert((s.class.clone(), s.output.name()));
    }
    let texture = cut.skips.iter().any(|s| s.output == SkipOutput::Texture);
    let atlases = cut
        .skips
        .iter()
        .filter(|s| s.output == SkipOutput::Atlas)
        .count();
    let crops = cut
        .handed
        .iter()
        .filter(|o| o.kind == OutputKind::AtlasSprite)
        .count();
    seen.decode |= texture && atlases == 1 && crops == 0;
    seen.png |= texture && atlases == 0 && crops == 2;
}

/// Cuts `s` under `meter(n)` for `n` up to the first that refuses nothing,
/// the sink dropping each output, and reports what the refusals were.
fn sweep(
    s: &AtlasScene,
    reason: impl Fn(u64) -> String,
    meter: impl Fn(u64) -> Arc<Meter>,
) -> Seen {
    let mut seen = Seen::default();
    let mut n = 0;
    loop {
        let engine = Engine {
            dropping: true,
            ..Engine::default()
        };
        match run_in(s, "x.bundle", FileContext::default(), &meter(n), &engine) {
            Ok(cut) if cut.skips.is_empty() => return seen,
            Ok(cut) => look(&cut, &reason(n), &mut seen),
            Err(f) => assert_eq!(failed(&f).0, reason(n), "the input's own refusal"),
        }
        n += 32;
        assert!(n < 1 << 24, "the cut never fits");
    }
}

/// Every kind of output a refusal skipped: the texture, an atlas whole (its
/// sheet's decode), its JSON and its crops.
fn every_output() -> BTreeSet<(String, &'static str)> {
    [
        ("MonoBehaviour", "atlas"),
        ("MonoBehaviour", "atlas_json"),
        ("MonoBehaviour", "atlas_sprite"),
        ("Texture2D", "texture"),
    ]
    .into_iter()
    .map(|(c, o)| (c.to_owned(), o))
    .collect()
}

#[test]
fn every_charge_refused_by_the_inputs_budget_is_its_objects_skip() {
    let seen = sweep(
        &big_sheet(),
        |_| "budget(input)".to_owned(),
        |n| Meter::new(u64::MAX, Arc::new(Counting::new(n)), Reservation::Input),
    );
    assert_eq!(seen.rows, every_output());
    assert!(
        seen.decode,
        "a sheet's decode refused skips its texture and its atlas"
    );
    assert!(seen.png, "the texture's PNG refused skips only the texture");
}

#[test]
fn every_charge_past_the_inputs_limit_is_its_objects_skip() {
    let seen = sweep(
        &big_sheet(),
        |n| format!("limit(input_memory, {n})"),
        |n| Meter::new(n, Arc::new(Unbounded), Reservation::Input),
    );
    assert_eq!(seen.rows, every_output());
    assert!(seen.decode && seen.png);
}

#[test]
fn each_named_refusal_skips_the_output_it_was_for() {
    // Each reservation of M_X refused in turn, named by the operation that
    // asked for it (review round 2's T1), with the sink dropping each
    // output: a sheet's decode skips its texture and its atlas in the
    // export; a PNG's encode and an output's own charge skip that output
    // alone; every unit is an output's that was handed over (C6).
    let s = big_sheet();
    let mut seen = BTreeSet::new();
    for at in 0.. {
        let (site, got) = refused_at(&s, at);
        let Some(site) = site else {
            break;
        };
        assert!(at < 1 << 16);
        let ran = match got {
            Ok(ran) => ran,
            Err(Failure::Input(r)) => {
                assert_eq!(r.reason, "budget(input)", "{at} ({site})");
                continue;
            }
            Err(f) => panic!("{at} ({site}): {f:?}"),
        };
        ran.assert_units();
        let rows: Vec<_> = ran
            .skips
            .iter()
            .map(|k| (k.output.name(), k.reason.reason.as_str()))
            .collect();
        let exported = &ran.skips[ran.plan_skips..];
        match site {
            "decode" => {
                assert_eq!(
                    rows,
                    [("texture", "budget(input)"), ("atlas", "budget(input)")]
                );
                assert_eq!(exported.len(), 2, "in the export");
                assert_eq!(ran.handed, []);
            }
            "png" | "metadata" | "crop" | "json" => {
                assert_eq!(rows.len(), 1, "{site}: {rows:?}");
                assert_eq!(exported.len(), 1, "{site}: in the export");
                assert_eq!(ran.handed.len(), 3, "{site}: the other three");
            }
            _ => {}
        }
        seen.insert(site);
    }
    for site in [
        "decode",
        "png",
        "metadata",
        "crop",
        "json",
        "head",
        "monoscript",
    ] {
        assert!(seen.contains(site), "no {site} refusal: {seen:?}");
    }
}

#[test]
fn each_read_refusal_is_its_objects_budget_skip() {
    // The read sites `big_sheet` has none of (review round 3's T5): over
    // cuts with a Material and sheets in a dependency and streamed sheets,
    // a refusal at a resource read, a name peek, a Material read, a
    // dereference or a texture read is the reading object's skip for the
    // budget, or the input's failure, or absorbed by the core (the cut
    // whole); each site is seen to skip at least once. The rows are tied to
    // the object read (review round 4's T3): an atlas's own Material read
    // or dereference skips that one atlas (a MonoBehaviour, its path id);
    // a texture's read or its data's skips that texture (a Texture2D) and
    // the atlases whose sheet it is in the whole cut, and no other. Each
    // row's exact object, found from the scene alone, is
    // `each_read_refusal_skips_the_object_its_scene_reads`'s.
    let mut skipped = BTreeSet::new();
    for name in [
        "material_in_dependency",
        "streamed_sheets",
        "missing_sheets",
    ] {
        let s = scene(name);
        let full = run_with(&s, "x.bundle", FileContext::default(), &Meter::unlimited()).unwrap();
        let sheet_of = |file: &RawStr, path_id: i64| {
            full.sheets
                .iter()
                .find(|r| r.atlas_file == *file && r.atlas_path_id == path_id)
                .map(|r| (r.texture_file.clone(), r.texture_path_id))
        };
        for at in 0.. {
            assert!(at < 1 << 16);
            let (site, got) = refused_in(&s, "x.bundle", at, true);
            let Some(site) = site else {
                break;
            };
            if !matches!(
                site,
                "resource" | "peek_name" | "material" | "deref" | "texture_read"
            ) {
                continue;
            }
            match got {
                Ok(ran) => {
                    ran.assert_units();
                    let budget: Vec<_> = ran
                        .skips
                        .iter()
                        .filter(|k| k.reason.reason == "budget(input)")
                        .collect();
                    let Some(first) = budget.first() else {
                        continue;
                    };
                    let id = |k: &Skip| (k.serialized_file.clone(), k.path_id);
                    let atlas = budget
                        .iter()
                        .all(|k| k.class == "MonoBehaviour" && id(k) == id(first));
                    // The texture: a Texture2D row's, else the atlases' sheet.
                    let texture = budget
                        .iter()
                        .find(|k| k.class == "Texture2D")
                        .map(|k| id(k))
                        .or_else(|| sheet_of(&first.serialized_file, first.path_id));
                    let sheeted = texture.is_some_and(|t| {
                        budget.iter().all(|k| match k.class.as_str() {
                            "Texture2D" => id(k) == t,
                            _ => sheet_of(&k.serialized_file, k.path_id) == Some(t.clone()),
                        })
                    });
                    let tied = match site {
                        "material" | "deref" => atlas,
                        "resource" | "texture_read" => sheeted,
                        _ => atlas || sheeted,
                    };
                    assert!(tied, "{name} {at} ({site}): {budget:?}");
                    skipped.insert(site);
                }
                Err(Failure::Input(r)) => assert_eq!(r.reason, "budget(input)", "{name} {at}"),
                Err(f) => panic!("{name} {at} ({site}): {f:?}"),
            }
        }
    }
    for site in ["resource", "peek_name", "material", "deref", "texture_read"] {
        assert!(skipped.contains(site), "no {site} skip: {skipped:?}");
    }
}

/// A skip's identity: its file, path id, class and output.
type Row = (String, i64, String, &'static str);

fn row(file: &str, path_id: i64, class: &str, output: &'static str) -> Row {
    (file.to_owned(), path_id, class.to_owned(), output)
}

/// Skip rows, each an object's identity and its reason.
type Rows = BTreeSet<(Row, String)>;

fn rows(of: &[(&Row, &str)]) -> Rows {
    of.iter()
        .map(|(r, why)| ((*r).clone(), (*why).to_owned()))
        .collect()
}

/// Two scenes whose every read's target is known by construction (review
/// round 5's T1), each with one atlas and one sheet: `m`, whose atlas names
/// its sheet through its own file's Material, whose `_MainTex` is a texture
/// in a dependency named as the input's sheet (the input's reads are the
/// ones charged to `M_X`), and `r`, whose sheet's data is in a resource.
/// Each gives its atlas's row and its sheet's.
fn known_scenes() -> [(AtlasScene, Row, Row); 2] {
    let one = |id: &str, data: Vec<u8>, deps: &[&str]| SceneBundle {
        id: id.to_owned(),
        data,
        deps: deps.iter().map(|d| (*d).to_owned()).collect(),
    };
    let sprites = || vec![sprite(b"s", (0, 0, 2, 2), [0; 8])];
    let dep = file(
        &[],
        &[texture(
            21,
            b"named",
            format::RGBA32,
            (1, 1),
            sheet(6, 1, 1, false),
        )],
        true,
    )
    .unwrap();
    let m = file(
        &["archive:/CAB-dm/CAB-dm"],
        &[
            mono_script(1, "NGUIAtlas"),
            atlas(2, b"a", (0, 1), (0, 20), sprites(), Vec::new()),
            material(20, "m", PropName::Plain, &[("_MainTex", (1, 21))]),
            texture(22, b"named", format::RGBA32, (2, 2), sheet(5, 2, 2, false)),
        ],
        true,
    )
    .unwrap();
    let r = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture_streamed(
                2,
                b"own",
                format::RGBA32,
                (2, 2),
                "archive:/CAB-xr/CAB-xr.resS",
                0,
                16,
            ),
            atlas(3, b"own", (0, 1), (0, 0), sprites(), Vec::new()),
        ],
        true,
    )
    .unwrap();
    let options = || CutOptions::textures("{name}.png");
    [
        (
            AtlasScene {
                bundles: vec![
                    one("d.bundle", bundle(&[("CAB-dm", dep)]), &[]),
                    one("x.bundle", bundle(&[("CAB-xm", m)]), &["d.bundle"]),
                ],
                inputs: vec!["x.bundle".to_owned()],
                options: options(),
            },
            row("CAB-xm", 2, "MonoBehaviour", "atlas"),
            row("CAB-xm", 22, "Texture2D", "texture"),
        ),
        (
            AtlasScene {
                bundles: vec![one(
                    "x.bundle",
                    bundle(&[("CAB-xr", r), ("CAB-xr.resS", sheet(7, 2, 2, false))]),
                    &[],
                )],
                inputs: vec!["x.bundle".to_owned()],
                options: options(),
            },
            row("CAB-xr", 3, "MonoBehaviour", "atlas"),
            row("CAB-xr", 2, "Texture2D", "texture"),
        ),
    ]
}

#[test]
fn each_read_refusal_skips_the_object_its_scene_reads() {
    // Review round 5's T1: the expected rows come from the scene, not from
    // the skips under test. Each scene has one atlas and one sheet, so each
    // row is exact (file, path id, class, output and reason):
    // - the atlas's `m_Script` dereferenced (recognition 1): the atlas
    //   alone, `budget(input)`, and it is no atlas;
    // - its Material path (section 5 step 1: the Material's read, a
    //   dereference, the `_MainTex` texture's name): the atlas,
    //   `budget(input)`, and then, as step 1 says, no name and step 2 runs:
    //   no texture has the atlas's name `a`, so `sheet_not_found` too;
    // - the sheet's read or its data's: the sheet and its atlas;
    // - the sheet's name peek: the sheet, which then owns no name, so its
    //   atlas finds none (4.4 step 1, section 5 step 2).
    // One expected set for each refusal, chosen by the read it was in, from
    // its stack captured at the hook (review round 6's T1): step 1's path
    // (`material_name`), recognition's (`script_class`), or neither. Only
    // scene `m` has a Material, so in `r` a name peek is the sheet's.
    let budget = "budget(input)";
    let mut skipped = BTreeSet::new();
    for (s, atlas_row, sheet_row) in known_scenes() {
        let has_material = atlas_row.0 == "CAB-xm";
        let full = run_with(&s, "x.bundle", FileContext::default(), &Meter::unlimited()).unwrap();
        assert_eq!(full.skips, [], "the scene cuts whole");
        let atlas_only = rows(&[(&atlas_row, budget)]);
        let step_1 = rows(&[(&atlas_row, budget), (&atlas_row, "sheet_not_found")]);
        let both = rows(&[(&atlas_row, budget), (&sheet_row, budget)]);
        let unnamed = rows(&[(&atlas_row, "sheet_not_found"), (&sheet_row, budget)]);
        for at in 0.. {
            assert!(at < 1 << 16);
            let (site, stack, got) = refused_with_stack(&s, "x.bundle", at);
            let Some(site) = site else {
                break;
            };
            let step_1_path = stack.contains("ngui::read::material_name");
            let recognition = stack.contains("Attempt::script_class");
            assert!(has_material || !step_1_path, "{at} ({site})");
            let ran = match got {
                Ok(ran) => ran,
                Err(Failure::Input(r)) => {
                    assert_eq!(r.reason, "budget(input)", "{at} ({site})");
                    continue;
                }
                Err(f) => panic!("{at} ({site}): {f:?}"),
            };
            let got: Rows = ran
                .skips
                .iter()
                .map(|k| {
                    let file = String::from_utf8_lossy(k.serialized_file.as_bytes());
                    let id = (
                        file.into_owned(),
                        k.path_id,
                        k.class.clone(),
                        k.output.name(),
                    );
                    (id, k.reason.reason.clone())
                })
                .collect();
            if got.is_empty() {
                continue;
            }
            let want = match site {
                "deref" if recognition => &atlas_only,
                "material" | "deref" | "peek_name" if step_1_path => &step_1,
                "peek_name" => &unnamed,
                "resource" | "texture_read" => &both,
                "material" | "deref" => panic!("{at} ({site}) outside its read: {stack}"),
                _ => continue,
            };
            assert_eq!(&got, want, "{at} ({site})");
            skipped.insert(site);
        }
    }
    for site in ["resource", "peek_name", "material", "deref", "texture_read"] {
        assert!(skipped.contains(site), "no {site} skip: {skipped:?}");
    }
}

#[test]
fn a_site_is_named_under_either_demangling() {
    // Review round 3's T5: the v0 form and the legacy one.
    for frame in [
        "  12: <unityrs_export::ngui::attempt::Attempt>::output\n",
        "  12: unityrs_export::ngui::attempt::Attempt::output::h0123456789abcdef\n",
    ] {
        assert_eq!(site(frame), "metadata", "{frame}");
    }
    let peeked = "  3: <unityrs_export::ngui::register::Peeked as \
                  unityrs_export::ngui::register::PeekBits>::load\n";
    assert_eq!(site(peeked), "peek_bits");
}

#[test]
fn a_refused_texture_output_leaves_its_atlas_without_its_unit() {
    // C6: the sheet's texture refused at its own charge, its atlas's JSON
    // and crops take the JSON's key as their unit, never the texture's. The
    // output's own charge (`metadata`), the one refusal the sink never
    // sees, is required (review round 3's T4).
    let s = big_sheet();
    let mut seen = BTreeSet::new();
    for at in 0.. {
        let (site, got) = refused_at(&s, at);
        let Some(site) = site else {
            break;
        };
        let Ok(ran) = got else {
            continue;
        };
        let texture_lost = ran.skips.iter().any(|k| k.output == SkipOutput::Texture);
        if matches!(site, "png" | "metadata") && texture_lost {
            let units: BTreeSet<_> = ran.handed.iter().map(|h| h.unit.clone()).collect();
            assert_eq!(
                units,
                BTreeSet::from([Some("big.json".to_owned())]),
                "{site}"
            );
            seen.insert(site);
        }
    }
    assert!(seen.contains("metadata"), "{seen:?}");
}

/// The value of field `name` of a MonoBehaviour of `fields` (integers), by
/// the core's `Parsed::field`, as a typetree dict keys it: by raw name.
fn field(fields: &[(&str, i32)], name: &[u8]) -> Option<i64> {
    use unityrs_core::env::{EnvOptions, Environment, Source};
    use unityrs_core::{Bytes, File, RawStr};
    use unityrs_testkit::ngui::{V, mono_behaviour};
    let fields = fields.iter().map(|(n, v)| (*n, V::Int(*v))).collect();
    let cab = file(&[], &[mono_behaviour(1, b"m", (0, 0), fields)], true).unwrap();
    let env = Environment::new(EnvOptions::batch(Arc::new(FileContext::default())));
    let Ok(File::Bundle(b)) = env.load_dependency(Source::Bytes {
        name: RawStr::from("x.bundle"),
        data: Bytes::from(bundle(&[("CAB-a", cab)])),
    }) else {
        panic!("a bundle");
    };
    let Ok(File::Serialized(sf)) = b.entry(0) else {
        panic!("a SerializedFile");
    };
    let parsed = sf.object(1).unwrap().read_typetree(None, true).unwrap();
    let map = parsed.value().as_dict().unwrap();
    parsed
        .field(map, name)
        .and_then(unityrs_core::Value::as_i64)
}

#[test]
fn a_monobehaviours_field_is_its_raw_names_last_value() {
    // The atlas is read as a dict (review round 2's C2): `m X` is not
    // `m_X`, and a repeated raw name keeps its last value.
    assert_eq!(field(&[("m X", 1)], b"m_X"), None);
    assert_eq!(field(&[("m X", 1)], b"m X"), Some(1));
    assert_eq!(field(&[("m_X", 1), ("m X", 2)], b"m_X"), Some(1));
    assert_eq!(field(&[("m X", 1), ("m_X", 2)], b"m_X"), Some(2));
    assert_eq!(
        field(&[("m_X", 1), ("m X", 2), ("m_X", 3)], b"m_X"),
        Some(3)
    );
    assert_eq!(field(&[("(int&)m_X?", 4)], b"m_X"), None);
    assert_eq!(field(&[("m_Y", 1)], b"m_X"), None);
}

#[test]
fn an_atlas_whose_own_tree_fails_is_skipped_for_it_first() {
    // The atlas's head is read with the fixed nodes, so it is recognised;
    // its own tree, six levels deep, passes the depth bound of 4 that every
    // other tree here keeps, and fails: before every other atlas skip. Its
    // name is empty, so `unnamed_atlas` would be its skip were the tree
    // read after the name (review round 2's T1). Rows that make no tree at
    // all fail the file's open, the input's `entry_error`.
    let sprites = || vec![sprite(b"s", (0, 0, 1, 1), [0; 8])];
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture(2, b"", format::RGBA32, (1, 1), sheet(0x10, 1, 1, false)),
            atlas(3, b"", (0, 1), (0, 0), sprites(), Vec::new()),
        ],
        true,
    )
    .unwrap();
    let s = AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: bundle(&[("CAB-xt", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions::default(),
    };
    let mut context = FileContext::default();
    context.limits.max_tree_depth = 4;
    let cut = run_with(&s, "x.bundle", context, &Meter::unlimited()).unwrap();
    let rows: Vec<_> = cut
        .skips
        .iter()
        .map(|k| (k.path_id, k.output.name(), k.reason.reason.as_str()))
        .collect();
    assert_eq!(rows, [(3, "atlas", "limit(tree_depth, 4)")]);
    assert!(cut.outputs.is_empty());
}
