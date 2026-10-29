//! Package D's oracle fixtures, replayed (plan section 7.5 and G5): every
//! testkit Environment scene, run through `tools/fixtures/env.py`'s steps,
//! compared step by step with what UnityPy 1.25.2 recorded under `Eager`
//! (but the one step `PARITY.md` section 19.2 decides otherwise, pinned
//! here), and `Lazy` compared with `Eager` (identical on every scene but the
//! ones the plan's differences (a)-(c) name, which `tests/env.rs` pins); and
//! `str.lower`, `ntpath` and `reSplit` against Python.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::container::ContainerPath;
use unityrs_core::env::{CabResolver, DerefError, EnvOptions, Environment, LoadMode, PPtr, Source};
use unityrs_core::error::{Error, NotFound};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::pystr::{
    is_split_path, ntpath_basename, ntpath_splitext, py_lower, simplify_name,
};
use unityrs_core::value::Value;
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::env::{Scene, scenes};

fn load(name: &str) -> Option<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/d")
        .join(name);
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// UnityPy's `env.path`: a name the scene's resolver holds exactly, keyed
/// `@/<name>` as `tools/fixtures/env.py` writes the temporary directory.
#[derive(Debug)]
struct SceneResolver(Vec<(Vec<u8>, Bytes)>);

impl CabResolver for SceneResolver {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        let (n, data) = self.0.iter().find(|(n, _)| n == name)?;
        let mut key = b"@/".to_vec();
        key.extend_from_slice(n);
        Some(Source::Bytes {
            name: RawStr::new(key),
            data: data.clone(),
        })
    }
}

/// The exception UnityPy raises where the core returns `e`.
fn deref_kind(e: &DerefError) -> &'static str {
    match e {
        DerefError::Null => "ValueError",
        DerefError::FileIdPastEnd { .. } | DerefError::NotFound { .. } => "FileNotFoundError",
        DerefError::FileIdBelowStart { .. } => "IndexError",
        DerefError::MissingObject { .. } => "KeyError",
        DerefError::RawTarget { .. } => "AttributeError",
        DerefError::ContainerDropped { .. } => "ContainerDropped",
        DerefError::Load(_) => "load",
        _ => "other",
    }
}

/// The exception UnityPy raises where the core returns `e`, for the steps
/// that return a file-level error (a resource read, the container).
fn error_kind(e: &Error) -> String {
    match e {
        Error::NotFound(NotFound::Resource(_)) => "FileNotFoundError".into(),
        Error::Malformed {
            what: "resource", ..
        } => "AttributeError".into(),
        // The preload's raise.
        Error::Deref(d) => deref_kind(d).into(),
        // The container pass's loop over `env.cabs` when it grew.
        Error::Malformed {
            what: "container",
            detail,
        } if detail.contains("changed size") => "RuntimeError".into(),
        other => format!("core: {other}"),
    }
}

/// `File.name` in UnityPy: `os.path.basename` of the name it was loaded
/// under (`files/File.py:38`); the core keeps the whole name.
fn file_name(name: &RawStr) -> String {
    let b = name.as_bytes();
    text(&b[b.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)..])
}

fn kind(f: &File) -> &'static str {
    match f {
        File::Serialized(_) => "serialized",
        File::Bundle(_) => "bundle",
        File::Raw(_) | File::Web(_) => "raw",
    }
}

struct Run {
    scene: Scene,
    env: Environment,
}

impl Run {
    fn new(scene: Scene, mode: LoadMode) -> Self {
        let mut options = EnvOptions::unitypy();
        options.load = mode;
        options.resolver = Some(Arc::new(SceneResolver(
            scene
                .resolver
                .iter()
                .map(|(n, d)| (n.clone(), Bytes::from(d.clone())))
                .collect(),
        )));
        Self {
            scene,
            env: Environment::new(options),
        }
    }

    fn state(&self) -> J {
        let files: Vec<J> = self
            .env
            .files()
            .map(|f| json!([text(f.name.as_bytes()), f.is_dependency, kind(&f.file)]))
            .collect();
        let cabs: Vec<J> = self
            .env
            .registry()
            .keys()
            .map(|k| json!(text(k.as_bytes())))
            .collect();
        let objects: Vec<J> = self
            .env
            .objects()
            .map(|o| match o {
                Ok(o) => json!([file_name(o.file().name()), o.path_id(), o.class_id()]),
                Err(e) => json!(format!("core: {e}")),
            })
            .collect();
        json!({"files": files, "cabs": cabs, "objects": objects})
    }

    fn load(&self, i: usize) -> J {
        let (name, data) = &self.scene.files[i];
        let source = Source::Bytes {
            name: RawStr::new(name.clone()),
            data: Bytes::from(data.clone()),
        };
        json!(if self.env.load(source).is_ok() {
            "ok"
        } else {
            "error"
        })
    }

    fn objects(&self) -> Vec<unityrs_core::object::ObjectRef> {
        self.env.objects().filter_map(Result::ok).collect()
    }

    fn name_of(o: &unityrs_core::object::ObjectRef) -> J {
        match o.parse_as_dict(true) {
            Ok(p) => match p.get(&[b"m_Name"]) {
                Some(Value::Str(s)) => json!(text(s.as_bytes())),
                _ => J::Null,
            },
            Err(_) => json!("error"),
        }
    }

    fn reads(&self) -> J {
        let out: Vec<J> = self
            .objects()
            .iter()
            .map(|o| {
                if o.class_id() != 142 {
                    return Self::name_of(o);
                }
                match o.parse_as_dict(true) {
                    Ok(p) => match p.value() {
                        Value::Dict(m) => {
                            json!(p.entries(m).map(|(k, _)| text(k)).collect::<Vec<_>>())
                        }
                        _ => J::Null,
                    },
                    Err(_) => json!("error"),
                }
            })
            .collect();
        json!(out)
    }

    fn derefs(&self) -> J {
        let mut out = Vec::new();
        for o in self.objects() {
            if o.class_id() != 1 {
                continue;
            }
            let p = o.parse_as_dict(true).unwrap();
            let Value::Dict(m) = p.value() else { continue };
            for (k, v) in p.entries(m) {
                if !k.starts_with(b"m_Ptr") {
                    continue;
                }
                let Value::Dict(fields) = v else { continue };
                let target = PPtr {
                    file_id: i32::try_from(p.field(fields, b"m_FileID").unwrap().as_i64().unwrap())
                        .unwrap(),
                    path_id: p.field(fields, b"m_PathID").unwrap().as_i64().unwrap(),
                };
                out.push(match self.env.deref(o.file(), target) {
                    Ok(t) => json!([
                        file_name(t.file().name()),
                        t.path_id(),
                        t.class_id(),
                        Self::name_of(&t)
                    ]),
                    Err(e) => json!(deref_kind(&e)),
                });
            }
        }
        json!(out)
    }

    fn resources(&self) -> J {
        let mut out = Vec::new();
        for o in self.objects() {
            if o.class_id() != 28 {
                continue;
            }
            let p = o.parse_as_dict(true).unwrap();
            let path = match p.get(&[b"m_StreamData", b"path"]) {
                Some(Value::Str(s)) => s.clone(),
                _ => panic!("no path"),
            };
            let offset = p
                .get(&[b"m_StreamData", b"offset"])
                .unwrap()
                .as_u64()
                .unwrap();
            let size = p
                .get(&[b"m_StreamData", b"size"])
                .unwrap()
                .as_u64()
                .unwrap();
            out.push(match self.env.resource(o.file(), &path, offset, size) {
                Ok(b) => json!(hex(&b)),
                Err(e) => json!(error_kind(&e)),
            });
        }
        json!(out)
    }

    fn container(&self) -> J {
        let items = match self.env.container() {
            Ok(items) => items,
            Err(e) => return json!(error_kind(&e)),
        };
        let mut rows = Vec::new();
        for item in items {
            match item {
                Ok(i) => rows.push(json!([
                    i.row.path.as_ref().map(|p| text(p.as_bytes())),
                    i.row.preload_index,
                    i.row.preload_size,
                    i.row.file_id,
                    i.row.path_id
                ])),
                Err(e) => return json!(error_kind(&e)),
            }
        }
        json!(rows)
    }

    /// `obj.container` for every object.
    fn paths(&self) -> J {
        let out: Vec<J> = self
            .objects()
            .iter()
            .map(|o| match self.env.container_path(o) {
                Ok(None) => J::Null,
                Ok(Some(ContainerPath::Str(s))) => json!(text(s.as_bytes())),
                Ok(Some(ContainerPath::NotAString)) => json!("<not a str>"),
                Err(e) => json!(error_kind(&e)),
            })
            .collect();
        json!(out)
    }

    fn step(&self, op: &J) -> J {
        let result = match op[0].as_str().unwrap() {
            "load" => self.load(usize::try_from(op[1].as_u64().unwrap()).unwrap()),
            "reads" => self.reads(),
            "derefs" => self.derefs(),
            "resources" => self.resources(),
            "container" => self.container(),
            "paths" => self.paths(),
            other => panic!("unknown step {other}"),
        };
        let mut s = self.state();
        s["op"] = op.clone();
        s["result"] = result;
        s
    }

    fn steps(&self, ops: &[J]) -> Vec<J> {
        ops.iter().map(|op| self.step(op)).collect()
    }
}

fn scene_by_name(name: &str) -> Scene {
    scenes()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| s)
        .unwrap()
}

fn recorded() -> Vec<(String, Vec<J>)> {
    let doc = load("env.json").expect("tests/fixtures/d/env.json");
    doc["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_owned(),
                s["steps"].as_array().unwrap().clone(),
            )
        })
        .collect()
}

fn ops(steps: &[J]) -> Vec<J> {
    steps.iter().map(|s| s["op"].clone()).collect()
}

/// The fixture's scenes are the testkit's, byte for byte. With
/// `UNITYRS_WRITE_SCENES=<path>` it writes them there instead, as JSON lines
/// (`tools/fixtures/env.py` reads them).
#[test]
fn scenes_are_the_testkit_scenes() {
    let all = scenes();
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
        std::fs::write(out, lines).unwrap();
        return;
    }
    let doc = load("env.json").expect("tests/fixtures/d/env.json");
    let cases = doc["scenes"].as_array().unwrap();
    assert_eq!(cases.len(), all.len());
    for (c, (n, s)) in cases.iter().zip(&all) {
        assert_eq!(c["name"].as_str().unwrap(), *n);
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

/// The scenes on which `Lazy` differs from `Eager`, each by one of plan
/// 3.13's differences (`tests/env.rs` pins what each difference is).
const LAZY_DIFFERS: &[&str] = &[
    // (a): the failing entry fails only the reads that parse it.
    "bad_cab",
    "asset_bundle_modes",
    // (b): a nested container's own name is a CAB key.
    "nested_after_cab",
    "nested_duplicates",
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
