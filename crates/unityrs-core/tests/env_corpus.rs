//! G5's corpus half (plan section 6): the spike's Environment probe
//! (`docs/spikes/rabex.md` section 6), through the core's Environment in
//! both load modes. It runs only when given a corpus, and writes its cases
//! outside the repository for `tools/fixtures/env.py --g5` to check against
//! UnityPy 1.25.2:
//!
//!     UNITYRS_CORPUS=/corpus/index.json UNITYRS_G5_OUT=<file outside the repo> \
//!         cargo test -p unityrs-core --release --test env_corpus -- --nocapture
//!     python tools/fixtures/env.py --g5 <that file> --corpus /corpus
//!
//! The selection is the spike's: build A's bundles in path order, loaded
//! into one Environment; for each bundle, each `SerializedFile` entry in
//! directory order and each object in table order, read as a dict: up to
//! four pointers per object whose `m_FileID` is positive, into an external
//! under `archive:/` (a built-in is skipped), at most three per bundle,
//! while fewer than 40 are taken; and each `Texture2D` whose `m_StreamData`
//! names a path, while fewer than 20 are taken. Each pointer is
//! dereferenced and each stream read through the Environment. Nothing of
//! the corpus is written into the repository.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, LoadMode, PPtr, Source};
use unityrs_core::files::File;
use unityrs_core::typetree::{NodeId, NodeList};
use unityrs_core::{Parsed, Value};

/// FNV-1a, 64 bits: what `env.py --g5` computes over UnityPy's bytes.
fn fnv64(b: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= u64::from(x);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

struct Found {
    path: Vec<J>,
    file_id: i32,
    path_id: i64,
}

/// The element node of an array node (a vector's `Array` child's data
/// child, or an `Array`'s own).
fn element(list: &NodeList, id: NodeId) -> NodeId {
    let array = match list.children(id).first() {
        Some(&a) if list.type_name(a) == b"Array" => a,
        _ => id,
    };
    list.children(array).get(1).copied().unwrap_or(array)
}

/// The spike's `find_pptrs`: depth first, a `PPtr<...>` dict with a
/// positive `m_FileID` found, at most `limit`.
fn find_pptrs(
    p: &Parsed,
    list: &NodeList,
    v: &Value,
    node: NodeId,
    path: &mut Vec<J>,
    out: &mut Vec<Found>,
    limit: usize,
) {
    if out.len() >= limit {
        return;
    }
    match v {
        Value::Dict(m) if list.type_name(node).starts_with(b"PPtr<") => {
            let f = p.field(m, b"m_FileID").and_then(Value::as_i64);
            let id = p.field(m, b"m_PathID").and_then(Value::as_i64);
            if let (Some(f), Some(id)) = (f, id)
                && f > 0
            {
                out.push(Found {
                    path: path.clone(),
                    file_id: i32::try_from(f).unwrap(),
                    path_id: id,
                });
            }
        }
        Value::Dict(m) => {
            for (k, v) in m.entries() {
                path.push(json!(String::from_utf8_lossy(list.name(*k))));
                find_pptrs(p, list, v, *k, path, out, limit);
                path.pop();
            }
        }
        Value::List(items) => {
            let e = element(list, node);
            for (i, v) in items.iter().enumerate() {
                path.push(json!(i));
                find_pptrs(p, list, v, e, path, out, limit);
                path.pop();
            }
        }
        // A pair is a tuple in UnityPy's dict: its halves by index.
        Value::Tuple(items) => {
            for (i, (c, v)) in list.children(node).iter().zip(items.iter()).enumerate() {
                path.push(json!(i));
                find_pptrs(p, list, v, *c, path, out, limit);
                path.pop();
            }
        }
        _ => {}
    }
}

fn name_of(p: &Parsed) -> J {
    match p.get(&[b"m_Name"]) {
        Some(Value::Str(s)) => json!(String::from_utf8_lossy(s.as_bytes())),
        _ => J::Null,
    }
}

/// The probe over `bundles` (`(relative path, absolute path)`) in `mode`.
fn probe(bundles: &[(String, PathBuf)], mode: LoadMode) -> J {
    let mut options = EnvOptions::unitypy();
    options.load = mode;
    let env = Environment::new(options);
    let loaded: Vec<(String, File)> = bundles
        .iter()
        .map(|(rel, abs)| (rel.clone(), env.load(Source::Path(abs.clone())).unwrap()))
        .collect();
    let mut pptrs = Vec::new();
    let mut streams = Vec::new();
    let mut builtin = 0;
    for (rel, file) in &loaded {
        if pptrs.len() >= 40 && streams.len() >= 20 {
            break;
        }
        let File::Bundle(bundle) = file else { continue };
        let mut per_bundle = 0;
        for (i, d) in bundle.directory().iter().enumerate() {
            if d.path.as_bytes().ends_with(b".resS") {
                continue;
            }
            let Ok(File::Serialized(sf)) = bundle.entry(i) else {
                continue;
            };
            for obj in sf.objects() {
                let Ok(p) = obj.parse_as_dict(true) else {
                    continue;
                };
                let Some(list) = p.lists().first() else {
                    continue;
                };
                if pptrs.len() < 40 && per_bundle < 3 {
                    let mut found = Vec::new();
                    find_pptrs(
                        &p,
                        list,
                        p.value(),
                        list.root(),
                        &mut Vec::new(),
                        &mut found,
                        4,
                    );
                    for f in found {
                        let ext = usize::try_from(f.file_id - 1)
                            .ok()
                            .and_then(|i| sf.externals().get(i));
                        let Some(ext) = ext else { continue };
                        if !ext.path.as_bytes().starts_with(b"archive:/") {
                            builtin += 1;
                            continue;
                        }
                        if per_bundle >= 3 {
                            break;
                        }
                        per_bundle += 1;
                        let ptr = PPtr {
                            file_id: f.file_id,
                            path_id: f.path_id,
                        };
                        let result = match env.deref(&sf, ptr) {
                            Ok(t) => match t.parse_as_dict(true) {
                                Ok(tp) => json!({"class_id": t.class_id(), "name": name_of(&tp)}),
                                Err(e) => json!({"error": format!("read: {e}")}),
                            },
                            Err(e) => json!({"error": e.to_string()}),
                        };
                        pptrs.push(json!({
                            "bundle": rel, "cab": String::from_utf8_lossy(d.path.as_bytes()),
                            "path_id": obj.path_id(), "field": f.path, "file_id": f.file_id,
                            "target_path_id": f.path_id, "core": result,
                        }));
                    }
                }
                if streams.len() < 20 && obj.class_id() == 28 {
                    let get = |k: &[u8]| p.get(&[b"m_StreamData", k]);
                    let (Some(Value::Str(path)), Some(off), Some(size)) =
                        (get(b"path"), get(b"offset"), get(b"size"))
                    else {
                        continue;
                    };
                    let (Some(off), Some(size)) = (off.as_u64(), size.as_u64()) else {
                        continue;
                    };
                    if path.is_empty() {
                        continue;
                    }
                    let result = match env.resource(&sf, path, off, size) {
                        Ok(bytes) => json!({"fnv64": fnv64(&bytes), "len": bytes.len()}),
                        Err(e) => json!({"error": e.to_string()}),
                    };
                    streams.push(json!({
                        "bundle": rel, "cab": String::from_utf8_lossy(d.path.as_bytes()),
                        "path_id": obj.path_id(), "path": String::from_utf8_lossy(path.as_bytes()),
                        "offset": off, "size": size, "core": result,
                    }));
                }
            }
        }
    }
    json!({"pptr": pptrs, "stream": streams, "builtin_skipped": builtin})
}

/// G5 on the corpus: 41 pointers and 20 streams resolve, the same in both
/// modes. Skipped (passing) without `UNITYRS_CORPUS` and `UNITYRS_G5_OUT`.
#[test]
fn the_spikes_environment_probe_resolves_in_both_modes() {
    let (Ok(index), Ok(out)) = (
        std::env::var("UNITYRS_CORPUS"),
        std::env::var("UNITYRS_G5_OUT"),
    ) else {
        eprintln!("env_corpus: set UNITYRS_CORPUS and UNITYRS_G5_OUT to run G5's corpus probe");
        return;
    };
    let index = PathBuf::from(index);
    let root = index.parent().unwrap().to_path_buf();
    let doc: J = serde_json::from_str(&std::fs::read_to_string(&index).unwrap()).unwrap();
    let mut rels: Vec<String> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            f["kind"] == "bundle"
                && (f["build"].as_i64() == Some(4100) || f["build"].as_str() == Some("4100"))
        })
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    rels.sort();
    let bundles: Vec<(String, PathBuf)> = rels.iter().map(|r| (r.clone(), root.join(r))).collect();
    let eager = probe(&bundles, LoadMode::Eager);
    let lazy = probe(&bundles, LoadMode::Lazy);
    assert_eq!(eager, lazy, "the modes agree");
    let ok = |k: &str| {
        eager[k]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["core"].get("error").is_none())
            .count()
    };
    let report = json!({"bundles": rels, "probe": eager});
    std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    eprintln!(
        "env_corpus: {} bundles, pointers {}/{}, streams {}/{}, {} built-ins skipped",
        bundles.len(),
        ok("pptr"),
        eager["pptr"].as_array().unwrap().len(),
        ok("stream"),
        eager["stream"].as_array().unwrap().len(),
        eager["builtin_skipped"]
    );
    assert_eq!(
        (ok("pptr"), eager["pptr"].as_array().unwrap().len()),
        (41, 41)
    );
    assert_eq!(
        (ok("stream"), eager["stream"].as_array().unwrap().len()),
        (20, 20)
    );
}
