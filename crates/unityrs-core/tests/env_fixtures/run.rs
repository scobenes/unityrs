//! The harness that replays a scene's steps, in UnityPy's shape
//! (`tools/fixtures/env.py`'s `Run`).

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::container::ContainerPath;
use unityrs_core::env::{
    Cab, CabResolver, DerefError, EnvOptions, Environment, LoadMode, PPtr, RawCab, Source,
};
use unityrs_core::error::{Error, NotFound};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::env::{Scene, scenes};

pub(crate) fn load(name: &str) -> Option<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/d")
        .join(name);
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

pub(crate) fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub(crate) fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// UnityPy's `env.path`: the scene's resolver files, keyed `@/<name>` as
/// `tools/fixtures/env.py` writes the temporary directory.
#[derive(Debug)]
pub(crate) struct SceneResolver(Vec<(Vec<u8>, Bytes)>);

impl CabResolver for SceneResolver {
    /// UnityPy's `find_file` walk of `env.path` (`environment.py:330-354`):
    /// the file of the name asked, else the first whose path ends with it.
    /// The walk's last step matches lower-cased names and opens the
    /// lower-cased absolute path, which `find_sensitive_path` cannot correct
    /// (`helpers/ImportHelper.py:159-173` splits it into a directory and a
    /// name), so it loads nothing the steps before missed. A resource
    /// fallback's `load_file` takes the name only, and no scene's resource
    /// candidate is the end of another resolver file's name.
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        let (n, data) = self
            .0
            .iter()
            .find(|(n, _)| n == name)
            .or_else(|| self.0.iter().find(|(n, _)| n.ends_with(name)))?;
        let mut key = b"@/".to_vec();
        key.extend_from_slice(n);
        Some(Source::Bytes {
            name: RawStr::new(key),
            data: data.clone(),
        })
    }
}

/// The exception UnityPy raises where the core returns `e`.
pub(crate) fn deref_kind(e: &DerefError) -> &'static str {
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
pub(crate) fn error_kind(e: &Error) -> String {
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
pub(crate) fn file_name(name: &RawStr) -> String {
    let b = name.as_bytes();
    text(&b[b.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)..])
}

pub(crate) fn kind(f: &File) -> &'static str {
    match f {
        File::Serialized(_) => "serialized",
        File::Bundle(_) => "bundle",
        File::Raw(_) | File::Web(_) => "raw",
    }
}

pub(crate) struct Run {
    pub(crate) scene: Scene,
    pub(crate) env: Environment,
    /// Each file's bytes: files of equal bytes share one allocation, as one
    /// input a caller loads twice does (the scene `nested_reloaded`).
    bytes: Vec<Bytes>,
}

impl Run {
    pub(crate) fn new(scene: Scene, mode: LoadMode) -> Self {
        Self::with(scene, mode, EnvOptions::unitypy())
    }

    pub(crate) fn with(scene: Scene, mode: LoadMode, mut options: EnvOptions) -> Self {
        options.load = mode;
        options.resolver = Some(Arc::new(SceneResolver(
            scene
                .resolver
                .iter()
                .map(|(n, d)| (n.clone(), Bytes::from(d.clone())))
                .collect(),
        )));
        let mut bytes: Vec<Bytes> = Vec::new();
        for (i, (_, data)) in scene.files.iter().enumerate() {
            let same = scene.files[..i].iter().position(|(_, d)| d == data);
            bytes.push(same.map_or_else(|| Bytes::from(data.clone()), |j| bytes[j].clone()));
        }
        Self {
            scene,
            env: Environment::new(options),
            bytes,
        }
    }

    pub(crate) fn state(&self) -> J {
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

    pub(crate) fn load(&self, i: usize) -> J {
        let (name, _) = &self.scene.files[i];
        let source = Source::Bytes {
            name: RawStr::new(name.clone()),
            data: self.bytes[i].clone(),
        };
        json!(if self.env.load(source).is_ok() {
            "ok"
        } else {
            "error"
        })
    }

    pub(crate) fn objects(&self) -> Vec<unityrs_core::object::ObjectRef> {
        self.env.objects().filter_map(Result::ok).collect()
    }

    pub(crate) fn name_of(o: &unityrs_core::object::ObjectRef) -> J {
        match o.parse_as_dict(true) {
            Ok(p) => match p.get(&[b"m_Name"]) {
                Some(Value::Str(s)) => json!(text(s.as_bytes())),
                _ => J::Null,
            },
            Err(_) => json!("error"),
        }
    }

    pub(crate) fn reads(&self) -> J {
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

    pub(crate) fn derefs(&self) -> J {
        self.derefs_of(self.objects())
    }

    /// Every pointer of `objects` dereferenced (`env.py`'s `derefs`).
    pub(crate) fn derefs_of(&self, objects: Vec<unityrs_core::object::ObjectRef>) -> J {
        let mut out = Vec::new();
        for o in objects {
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

    pub(crate) fn resources(&self) -> J {
        self.resources_of(self.objects())
    }

    /// Every resource of `objects` read (`env.py`'s `resources`).
    pub(crate) fn resources_of(&self, objects: Vec<unityrs_core::object::ObjectRef>) -> J {
        let mut out = Vec::new();
        for o in objects {
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

    pub(crate) fn container(&self) -> J {
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
    pub(crate) fn container_of(&self, o: &unityrs_core::object::ObjectRef) -> J {
        match self.env.container_path(o) {
            Ok(None) => J::Null,
            Ok(Some(ContainerPath::Str(s))) => json!(text(s.as_bytes())),
            Ok(Some(ContainerPath::NotAString)) => json!("<not a str>"),
            Err(e) => json!(error_kind(&e)),
        }
    }

    pub(crate) fn paths(&self) -> J {
        let out: Vec<J> = self
            .objects()
            .iter()
            .map(|o| self.container_of(o))
            .collect();
        json!(out)
    }

    /// `obj.container` for the objects of every `files` entry loaded as a
    /// dependency, which `env.objects` leaves out: `[key, path id, path]`.
    pub(crate) fn dep_paths(&self) -> J {
        // No scene holds a web file.
        fn walk(run: &Run, key: &str, file: &File, out: &mut Vec<J>) {
            match file {
                File::Serialized(sf) => out.extend(
                    sf.objects()
                        .map(|o| json!([key, o.path_id(), run.container_of(&o)])),
                ),
                File::Bundle(b) => {
                    for i in 0..b.directory().len() {
                        if let Ok(f) = b.entry(i) {
                            walk(run, key, &f, out);
                        }
                    }
                }
                File::Web(_) | File::Raw(_) => {}
            }
        }
        let mut out = Vec::new();
        for f in self.env.files().filter(|f| f.is_dependency) {
            walk(self, &text(f.name.as_bytes()), &f.file, &mut out);
        }
        json!(out)
    }

    /// `get_cab` of every name in `names`: what it finds (its kind, its
    /// name, and a `SerializedFile`'s path ids), `null` for none.
    pub(crate) fn get_cabs(&self, names: &J) -> J {
        let out: Vec<J> = names
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                let name = RawStr::new(n.as_str().unwrap().as_bytes().to_vec());
                match self.env.get_cab(&name, &Meter::unlimited()) {
                    Ok(None) => J::Null,
                    Ok(Some(Cab::Serialized(sf))) => json!([
                        "serialized",
                        file_name(sf.name()),
                        sf.objects().map(|o| o.path_id()).collect::<Vec<_>>()
                    ]),
                    Ok(Some(Cab::Raw(RawCab::InBundle { bundle, dir_index }))) => {
                        json!(["raw", text(bundle.directory()[dir_index].path.as_bytes())])
                    }
                    Ok(Some(Cab::Raw(RawCab::File(raw)))) => {
                        json!(["raw", text(raw.name.as_bytes())])
                    }
                    Err(e) => json!(error_kind(&e)),
                }
            })
            .collect();
        json!(out)
    }

    pub(crate) fn step(&self, op: &J) -> J {
        let result = match op[0].as_str().unwrap() {
            "load" => self.load(usize::try_from(op[1].as_u64().unwrap()).unwrap()),
            "reads" => self.reads(),
            "derefs" => self.derefs(),
            "resources" => self.resources(),
            "container" => self.container(),
            "paths" => self.paths(),
            "dep_paths" => self.dep_paths(),
            "get_cab" => self.get_cabs(&op[1]),
            other => panic!("unknown step {other}"),
        };
        let mut s = self.state();
        s["op"] = op.clone();
        s["result"] = result;
        s
    }

    pub(crate) fn steps(&self, ops: &[J]) -> Vec<J> {
        ops.iter().map(|op| self.step(op)).collect()
    }
}

/// Every object of `files` (the handles of the files a scene loaded as
/// dependencies, in load order), walked as `env.py`'s `dep_objects` walks
/// UnityPy's: a bundle's entries in directory order, a nested bundle's in
/// place.
pub(crate) fn dep_objects(files: &[File]) -> Vec<unityrs_core::object::ObjectRef> {
    // No scene holds a web file.
    fn walk(file: &File, out: &mut Vec<unityrs_core::object::ObjectRef>) {
        match file {
            File::Serialized(sf) => out.extend(sf.objects()),
            File::Bundle(b) => {
                for i in 0..b.directory().len() {
                    if let Ok(f) = b.entry(i) {
                        walk(&f, out);
                    }
                }
            }
            File::Web(_) | File::Raw(_) => {}
        }
    }
    let mut out = Vec::new();
    for f in files {
        walk(f, &mut out);
    }
    out
}

pub(crate) fn scene_by_name(name: &str) -> Scene {
    scenes()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| s)
        .unwrap()
}

pub(crate) fn recorded() -> Vec<(String, Vec<J>)> {
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

pub(crate) fn ops(steps: &[J]) -> Vec<J> {
    steps.iter().map(|s| s["op"].clone()).collect()
}
