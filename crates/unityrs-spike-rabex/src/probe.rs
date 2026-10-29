//! Throughput (`bench`) and the API probes (`probe`) of docs/spikes/rabex.md.

use std::collections::HashMap;
use std::io::Cursor;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Instant;

use rabex::UnityVersion;
use rabex::files::SerializedFile;
use rabex::files::bundlefile::{BundleFile, BundleFileReader, ExtractionConfig};
use rabex::objects::ClassId;
use rabex::tpk::TpkTypeTreeBlob;
use rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex::typetree::{TypeTreeNode, TypeTreeProvider};
use rayon::prelude::*;
use serde_json::{Value as JsonValue, json};

use crate::detect::{FileType, check_file_type, parsed_as};
use crate::dump::{input_bytes, read_value};
use crate::json::sha256_hex;
use crate::tpk::{self, MaybeTpk, Tpk, VersionedCache};
use crate::value::{Val, element};
use crate::{Args, collect_inputs, outside_repo, peak_rss, pool};

// ------------------------------------------------------------------- bench

#[derive(Default, Clone, Copy)]
struct Tally {
    inputs: u64,
    serialized: u64,
    objects: u64,
    value_errors: u64,
    file_errors: u64,
    decompressed: u64,
    t_container: f64,
    t_serialized: f64,
    t_values: f64,
}

impl Tally {
    fn add(mut self, o: Tally) -> Tally {
        self.inputs += o.inputs;
        self.serialized += o.serialized;
        self.objects += o.objects;
        self.value_errors += o.value_errors;
        self.file_errors += o.file_errors;
        self.decompressed += o.decompressed;
        self.t_container += o.t_container;
        self.t_serialized += o.t_serialized;
        self.t_values += o.t_values;
        self
    }
}

/// What UnityPy's `load_file` + `parse_as_dict()` per object does, through rabex.
fn bench_serialized(bytes: &[u8], tpk: &Tpk, t: &mut Tally) -> Result<(), String> {
    let t0 = Instant::now();
    let sf = SerializedFile::from_reader(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let t1 = Instant::now();
    t.serialized += 1;
    for info in sf.objects() {
        t.objects += 1;
        let Ok(tt) = sf.get_typetree_for(info, tpk) else {
            t.value_errors += 1;
            continue;
        };
        let start = usize::try_from(info.m_Offset).unwrap_or(usize::MAX);
        let raw = bytes
            .get(start..start + info.m_Size as usize)
            .unwrap_or_default();
        if read_value(raw, &tt, sf.m_Header.m_Endianess).is_err() {
            t.value_errors += 1;
        }
    }
    t.t_serialized += (t1 - t0).as_secs_f64();
    t.t_values += t1.elapsed().as_secs_f64();
    Ok(())
}

fn bench_one(data: &[u8], name: &str, tpk: &Tpk, shim: bool) -> Tally {
    let input = input_bytes(data, shim);
    let data = &*input;
    let mut t = Tally {
        inputs: 1,
        ..Tally::default()
    };
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let ty = check_file_type(data);
        match parsed_as(ty, name) {
            "BundleFile" => {
                let t0 = Instant::now();
                let b =
                    BundleFile::from_reader(&mut Cursor::new(data), &ExtractionConfig::default())
                        .map_err(|e| e.to_string())?;
                t.t_container += t0.elapsed().as_secs_f64();
                t.decompressed += b.m_BlockData.len() as u64;
                for e in &b.m_DirectoryInfo {
                    let (Ok(s), Ok(n)) = (usize::try_from(e.offset), usize::try_from(e.size))
                    else {
                        continue;
                    };
                    let bytes = b.m_BlockData.get(s..s + n).ok_or("entry out of range")?;
                    if parsed_as(check_file_type(bytes), &e.path) == "SerializedFile" {
                        bench_serialized(bytes, tpk, &mut t)?;
                    }
                }
                Ok(())
            }
            "SerializedFile" => bench_serialized(data, tpk, &mut t),
            _ => Ok(()),
        }
    }));
    if !matches!(r, Ok(Ok(()))) {
        t.file_errors += 1;
    }
    t
}

fn rate(t: &Tally, wall: f64, bytes: u64) -> JsonValue {
    #[allow(clippy::cast_precision_loss)]
    let (objects, mb) = (t.objects as f64, bytes as f64 / 1e6);
    json!({
        "wall_s": wall,
        "objects_per_s": objects / wall,
        "mb_per_s": mb / wall,
        "objects": t.objects,
        "serialized_files": t.serialized,
        "value_errors": t.value_errors,
        "file_errors": t.file_errors,
        "decompressed_bytes": t.decompressed,
        "cpu_s": {"container": t.t_container, "serialized": t.t_serialized, "values": t.t_values},
    })
}

/// Reads each file inside the timed work, as UnityPy's `load_file(path)` does,
/// so peak RSS is the working set; the page cache is warmed first.
/// `--phase serial` or `parallel` runs one side only, so an outside sampler
/// (`scripts/mem_peak.py`) sees that side's peak alone.
pub fn bench(a: &Args) -> Result<(), String> {
    let (serial_on, parallel_on) = match a.phase.as_str() {
        "both" => (true, true),
        "serial" => (true, false),
        "parallel" => (false, true),
        p => return Err(format!("--phase {p}: expected serial, parallel or both")),
    };
    let tpk = tpk::load(a.tpk.as_deref())?;
    let inputs = collect_inputs(a)?;
    let mut bytes = 0u64;
    for (abs, _, _) in &inputs {
        bytes += std::fs::read(abs).map_err(|e| e.to_string())?.len() as u64;
    }
    let paths: Vec<String> = inputs
        .iter()
        .map(|i| i.0.to_string_lossy().into_owned())
        .collect();
    // One TPK cache for every thread (it is Sync), keyed by class and version.
    let run = |p: &String| bench_one(&std::fs::read(p).unwrap_or_default(), p, &tpk, a.shim);
    let mut runs = Vec::new();
    for _ in 0..a.repeat.max(1) {
        let mut r = serde_json::Map::new();
        if serial_on {
            let t0 = Instant::now();
            let serial = paths
                .iter()
                .fold(Tally::default(), |acc, p| acc.add(run(p)));
            r.insert(
                "serial".into(),
                rate(&serial, t0.elapsed().as_secs_f64(), bytes),
            );
            r.insert("peak_rss_after_serial".into(), json!(peak_rss()));
        }
        if parallel_on {
            let t0 = Instant::now();
            let par = pool(a.jobs)
                .install(|| paths.par_iter().map(run).reduce(Tally::default, Tally::add));
            r.insert(
                "parallel".into(),
                rate(&par, t0.elapsed().as_secs_f64(), bytes),
            );
            r.insert("jobs".into(), json!(a.jobs));
            r.insert("peak_rss_after_parallel".into(), json!(peak_rss()));
        }
        runs.push(JsonValue::Object(r));
    }
    let out =
        json!({"inputs": paths.len(), "input_bytes": bytes, "shim_fs6": a.shim, "runs": runs});
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    Ok(())
}

// --------------------------------------------------------------- send+sync

/// Autoref probe: the inherent method wins when the bound holds.
struct Probe<T: ?Sized>(PhantomData<T>);
#[allow(dead_code)] // unused when every probed type is Send
trait NotSend {
    fn send(&self) -> bool {
        false
    }
}
#[allow(clippy::unused_self)] // autoref probe: the receiver picks the impl
trait NotSync {
    fn sync(&self) -> bool {
        false
    }
}
impl<T: ?Sized> NotSend for Probe<T> {}
impl<T: ?Sized> NotSync for Probe<T> {}
#[allow(clippy::unused_self)]
impl<T: ?Sized + Send> Probe<T> {
    fn send(&self) -> bool {
        true
    }
}
#[allow(clippy::unused_self)]
impl<T: ?Sized + Sync> Probe<T> {
    fn sync(&self) -> bool {
        true
    }
}

macro_rules! traits {
    ($($t:ty),* $(,)?) => {
        vec![$(json!({
            "type": stringify!($t),
            "send": Probe::<$t>(PhantomData).send(),
            "sync": Probe::<$t>(PhantomData).sync(),
        })),*]
    };
}

fn send_sync() -> Vec<JsonValue> {
    use rabex::files::serializedfile::ObjectRef;
    use rabex::typetree::typetree_cache::TypeTreeCache as UnsyncCache;
    traits![
        BundleFile,
        BundleFileReader<Cursor<&[u8]>>,
        SerializedFile,
        TypeTreeNode,
        TpkTypeTreeBlob,
        TypeTreeCache<TpkTypeTreeBlob>,
        UnsyncCache<TpkTypeTreeBlob>,
        Tpk,
        ObjectRef<'static, JsonValue>,
        rabex::serde_typetree::Error,
        rabex::files::serializedfile::Error,
    ]
}

// ----------------------------------------------------------------- probes

/// A PPtr found inside a value: its field path and target.
struct Found {
    path: Vec<JsonValue>,
    file_id: i64,
    path_id: i64,
}

fn find_pptrs(
    v: &Val,
    node: &TypeTreeNode,
    path: &mut Vec<JsonValue>,
    out: &mut Vec<Found>,
    limit: usize,
) {
    if out.len() >= limit {
        return;
    }
    match v {
        Val::Dict(vals) if node.m_Type.starts_with("PPtr<") => {
            let get = |n: &str| {
                node.children
                    .iter()
                    .position(|c| c.m_Name == n)
                    .and_then(|i| vals.get(i))
            };
            if let (Some(Val::I(f)), Some(Val::I(p))) = (get("m_FileID"), get("m_PathID"))
                && *f > 0
            {
                out.push(Found {
                    path: path.clone(),
                    file_id: *f,
                    path_id: *p,
                });
            }
        }
        Val::Dict(vals) => {
            for (child, v) in node.children.iter().zip(vals) {
                path.push(json!(child.m_Name));
                find_pptrs(v, child, path, out, limit);
                path.pop();
            }
        }
        Val::List(items) => {
            if let Some(e) = element(node) {
                for (i, v) in items.iter().enumerate() {
                    path.push(json!(i));
                    find_pptrs(v, e, path, out, limit);
                    path.pop();
                }
            }
        }
        Val::Tuple(items) => {
            for (i, (v, child)) in items.iter().zip(&node.children).enumerate() {
                path.push(json!(i));
                find_pptrs(v, child, path, out, limit);
                path.pop();
            }
        }
        _ => {}
    }
}

fn field<'v>(v: &'v Val, node: &TypeTreeNode, name: &str) -> Option<&'v Val> {
    let Val::Dict(vals) = v else { return None };
    vals.get(node.children.iter().rposition(|c| c.m_Name == name)?)
}

fn field_node<'t>(node: &'t TypeTreeNode, name: &str) -> Option<&'t TypeTreeNode> {
    node.children.iter().rfind(|c| c.m_Name == name)
}

/// The CAB registry an Environment needs: entry name (lower-cased) to bundle.
/// rabex reads a bundle's directory without decompressing its data blocks.
fn cab_registry(bundles: &[(String, Vec<u8>)]) -> (HashMap<String, usize>, usize) {
    let mut reg = HashMap::new();
    let mut failed = 0;
    for (i, (_, data)) in bundles.iter().enumerate() {
        match BundleFileReader::from_reader(
            Cursor::new(data.as_slice()),
            &ExtractionConfig::default(),
        ) {
            Ok(r) => {
                for f in r.files() {
                    reg.insert(f.path.to_lowercase(), i);
                }
            }
            Err(_) => failed += 1,
        }
    }
    (reg, failed)
}

/// UnityPy keys a CAB by the last path component of an external, lower-cased.
fn cab_key(external: &str) -> String {
    external
        .rsplit('/')
        .next()
        .unwrap_or(external)
        .to_lowercase()
}

fn read_entry(data: &[u8], entry: &str) -> Result<Vec<u8>, String> {
    let r = BundleFileReader::from_reader(Cursor::new(data), &ExtractionConfig::default())
        .map_err(|e| e.to_string())?;
    let file = r
        .files()
        .iter()
        .find(|f| f.path.eq_ignore_ascii_case(entry))
        .ok_or("no such entry")?
        .clone();
    r.read_at_entry(&file).map_err(|e| e.to_string())
}

fn object_value(
    sf: &SerializedFile,
    bytes: &[u8],
    path_id: i64,
    tpk: &Tpk,
) -> Result<(String, Option<String>), String> {
    let info = sf.get_object_info(path_id).ok_or("no such path_id")?;
    let tt = sf.get_typetree_for(info, tpk).map_err(|e| e.to_string())?;
    let start = usize::try_from(info.m_Offset).map_err(|e| e.to_string())?;
    let raw = bytes
        .get(start..start + info.m_Size as usize)
        .ok_or("object out of range")?;
    let (v, _) = read_value(raw, &tt, sf.m_Header.m_Endianess).map_err(|e| e.1)?;
    let name = match field(&v, &tt, "m_Name") {
        Some(Val::Str(s)) => Some(s.clone()),
        _ => None,
    };
    Ok((tt.m_Type.clone(), name))
}

/// The Environment probes over one build's bundles, as the index selects
/// them (`--index --build B`); sources are taken in path order.
#[allow(clippy::too_many_lines)]
#[allow(clippy::many_single_char_names)] // a probe script: one linear pass
fn environment_probe(a: &Args, tpk: &Tpk) -> Result<JsonValue, String> {
    let bundles: Vec<(String, Vec<u8>)> = collect_inputs(a)?
        .into_iter()
        .map(|(abs, rel, _)| Ok((rel, std::fs::read(&abs).map_err(|e| e.to_string())?)))
        .collect::<Result<_, String>>()?;
    if bundles.is_empty() {
        return Err("probe: the selection holds no bundles".into());
    }
    let t0 = Instant::now();
    let (reg, reg_failed) = cab_registry(&bundles);
    let reg_s = t0.elapsed().as_secs_f64();

    let mut pptrs = Vec::new();
    let mut streams = Vec::new();
    let (mut pptr_ok, mut pptr_fail, mut stream_ok, mut stream_fail) = (0, 0, 0, 0);
    let mut builtin = 0;
    for (bname, data) in &bundles {
        if pptrs.len() >= 40 && streams.len() >= 20 {
            break;
        }
        let Ok(b) = BundleFile::from_reader(&mut Cursor::new(data), &ExtractionConfig::default())
        else {
            continue;
        };
        let mut per_bundle = 0;
        for e in &b.m_DirectoryInfo {
            let (Ok(s), Ok(n)) = (usize::try_from(e.offset), usize::try_from(e.size)) else {
                continue;
            };
            let bytes = &b.m_BlockData[s..s + n];
            if check_file_type(bytes) != FileType::AssetsFile || e.path.ends_with(".resS") {
                continue;
            }
            let Ok(sf) = SerializedFile::from_reader(&mut Cursor::new(bytes)) else {
                continue;
            };
            for info in sf.objects() {
                let Ok(tt) = sf.get_typetree_for(info, tpk) else {
                    continue;
                };
                let st = usize::try_from(info.m_Offset).unwrap_or(0);
                let Some(raw) = bytes.get(st..st + info.m_Size as usize) else {
                    continue;
                };
                let Ok((v, _)) = read_value(raw, &tt, sf.m_Header.m_Endianess) else {
                    continue;
                };
                // Cross-file PPtrs: one per object, into another CAB.
                if pptrs.len() < 40 && per_bundle < 3 {
                    let mut found = Vec::new();
                    find_pptrs(&v, &tt, &mut Vec::new(), &mut found, 4);
                    for f in found {
                        let ext = usize::try_from(f.file_id - 1)
                            .ok()
                            .and_then(|i| sf.m_Externals.get(i));
                        let Some(ext) = ext else { continue };
                        // Unity's built-in resources are not in any corpus; UnityPy cannot resolve them either.
                        if !ext.pathName.starts_with("archive:/") {
                            builtin += 1;
                            continue;
                        }
                        if per_bundle >= 3 {
                            break;
                        }
                        per_bundle += 1;
                        let key = cab_key(&ext.pathName);
                        let target = reg.get(&key).map(|&i| {
                            let (tname, tdata) = &bundles[i];
                            let r = read_entry(tdata, &key).and_then(|cab| {
                                let tsf = SerializedFile::from_reader(&mut Cursor::new(&cab))
                                    .map_err(|e| e.to_string())?;
                                object_value(&tsf, &cab, f.path_id, tpk)
                            });
                            (tname.clone(), r)
                        });
                        let (tbundle, result) = match target {
                            Some((t, Ok((ty, name)))) => {
                                pptr_ok += 1;
                                (json!(t), json!({"type": ty, "name": name}))
                            }
                            Some((t, Err(e))) => {
                                pptr_fail += 1;
                                (json!(t), json!({"error": e}))
                            }
                            None => {
                                pptr_fail += 1;
                                (JsonValue::Null, json!({"error": "CAB not in registry"}))
                            }
                        };
                        pptrs.push(json!({
                            "bundle": bname, "cab": e.path, "path_id": info.m_PathID, "class": tt.m_Type,
                            "field": f.path, "file_id": f.file_id, "target_path_id": f.path_id,
                            "external": ext.pathName, "target_bundle": tbundle, "rabex": result,
                        }));
                    }
                }
                // Streamed textures: m_StreamData into a .resS entry.
                if streams.len() < 20 && info.m_ClassID == ClassId::Texture2D {
                    let Some(sd) = field(&v, &tt, "m_StreamData") else {
                        continue;
                    };
                    let sdn = field_node(&tt, "m_StreamData").unwrap();
                    let (Some(Val::Str(p)), Some(off), Some(Val::I(size))) = (
                        field(sd, sdn, "path"),
                        field(sd, sdn, "offset"),
                        field(sd, sdn, "size"),
                    ) else {
                        continue;
                    };
                    let off = match off {
                        Val::I(o) => u64::try_from(*o).unwrap_or(0),
                        Val::U(o) => *o,
                        _ => continue,
                    };
                    if p.is_empty() {
                        continue;
                    }
                    let key = cab_key(p);
                    let got = reg
                        .get(&key)
                        .ok_or("resS not in registry".to_owned())
                        .and_then(|&i| {
                            let res = read_entry(&bundles[i].1, &key)?;
                            let (o, n) = (
                                usize::try_from(off).unwrap_or(usize::MAX),
                                usize::try_from(*size).unwrap_or(0),
                            );
                            res.get(o..o + n)
                                .map(sha256_hex)
                                .ok_or("range past the resS".to_owned())
                        });
                    match &got {
                        Ok(_) => stream_ok += 1,
                        Err(_) => stream_fail += 1,
                    }
                    streams.push(json!({
                        "bundle": bname, "cab": e.path, "path_id": info.m_PathID, "path": p,
                        "offset": off, "size": size,
                        "rabex": match got { Ok(h) => json!({"sha256": h}), Err(e) => json!({"error": e}) },
                    }));
                }
            }
        }
    }
    Ok(json!({
        "bundles": bundles.iter().map(|b| &b.0).collect::<Vec<_>>(),
        "registry": {"bundles": bundles.len(), "entries": reg.len(), "failed": reg_failed, "seconds": reg_s},
        "pptr": {"ok": pptr_ok, "failed": pptr_fail, "builtin_skipped": builtin, "cases": pptrs},
        "stream": {"ok": stream_ok, "failed": stream_fail, "cases": streams},
    }))
}

/// A node and its subtree as rows: `[level, type, name, byte_size, version, type_flags, meta_flag]`.
fn tpk_rows(n: &TypeTreeNode, level: u32, rows: &mut Vec<JsonValue>) {
    rows.push(json!([
        level,
        n.m_Type,
        n.m_Name,
        n.m_ByteSize,
        n.m_Version,
        n.m_TypeFlags,
        n.m_MetaFlag
    ]));
    for c in &n.children {
        tpk_rows(c, level + 1, rows);
    }
}

/// The TPK's range, and node rows for a few classes.
fn tpk_probe(blob: &TpkTypeTreeBlob) -> JsonValue {
    let classes = [
        1,
        4,
        21,
        28,
        43,
        48,
        49,
        74,
        83,
        114,
        115,
        128,
        142,
        213,
        687_078_895,
    ];
    let versions = [
        "5.6.7f1",
        "2018.4.11f1",
        "2020.3.48f1",
        "2022.3.0f1",
        "6000.0.50f1",
        "6000.3.17f1",
    ];
    let mut nodes = Vec::new();
    for v in versions {
        let ver: UnityVersion = v.parse().unwrap();
        for &c in &classes {
            let id = ClassId(c);
            let r = catch_unwind(AssertUnwindSafe(|| blob.get_typetree_node(id, &ver)));
            let rows = match r {
                Ok(Some(n)) => {
                    let mut rows = Vec::new();
                    tpk_rows(&n, 0, &mut rows);
                    json!(rows)
                }
                Ok(None) => JsonValue::Null,
                Err(_) => json!("panic"),
            };
            nodes.push(json!({"version": v, "class_id": c, "nodes": rows}));
        }
    }
    let unknown = catch_unwind(AssertUnwindSafe(|| {
        blob.get_typetree_node(ClassId(999_999), &"6000.3.17f1".parse().unwrap())
    }))
    .is_err();
    json!({
        "versions": blob.versions.len(),
        "first": blob.versions.first().map(ToString::to_string),
        "last": blob.versions.last().map(ToString::to_string),
        "classes": blob.class_information.len(),
        "unknown_class_panics": unknown,
        "nodes": nodes,
    })
}

/// A named call that may panic.
type PanicCase<'b> = (&'static str, Box<dyn Fn() + 'b>);

/// Inputs that make rabex 0.2.1 panic through its public API (found by
/// reading the source; no fuzzer ran, see docs/spikes/rabex.md).
fn panics(blob: &TpkTypeTreeBlob) -> Vec<JsonValue> {
    fn header(sig: &str) -> Vec<u8> {
        let mut b = format!("{sig}\0").into_bytes();
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\0" as &[u8]);
        b.extend_from_slice(b"2018.4.4f1\0" as &[u8]);
        b.resize(256, 0);
        b
    }
    let map_without_pair = TypeTreeNode {
        m_Type: "map".into(),
        m_Name: "m".into(),
        m_MetaFlag: Some(0),
        children: vec![TypeTreeNode {
            m_Type: "Array".into(),
            m_Name: "Array".into(),
            m_MetaFlag: Some(0),
            children: vec![TypeTreeNode {
                m_Type: "int".into(),
                m_Name: "size".into(),
                m_MetaFlag: Some(0),
                ..TypeTreeNode::default()
            }],
            ..TypeTreeNode::default()
        }],
        ..TypeTreeNode::default()
    };
    let cases: Vec<PanicCase<'_>> = vec![
        (
            "BundleFile::from_reader, signature UnityArchive",
            Box::new(|| {
                let _ = BundleFile::from_reader(
                    &mut Cursor::new(header("UnityArchive")),
                    &ExtractionConfig::default(),
                );
            }),
        ),
        (
            "BundleFileReader::from_reader, signature UnityWeb",
            Box::new(|| {
                let _ = BundleFileReader::from_reader(
                    Cursor::new(header("UnityWeb")),
                    &ExtractionConfig::default(),
                );
            }),
        ),
        (
            "TpkTypeTreeBlob::get_typetree_node, class absent from the TPK",
            Box::new(|| {
                let _ = blob.get_typetree_node(ClassId(999_999), &"6000.3.17f1".parse().unwrap());
            }),
        ),
        (
            "serde_typetree::from_slice, map node whose Array lacks its pair",
            Box::new(move || {
                let _ = rabex::serde_typetree::from_slice::<JsonValue, byteorder::LittleEndian>(
                    &1i32.to_le_bytes(),
                    &map_without_pair,
                );
            }),
        ),
    ];
    cases
        .into_iter()
        .map(|(name, f)| {
            let r = catch_unwind(AssertUnwindSafe(f));
            json!({"case": name, "panics": r.is_err()})
        })
        .collect()
}

/// An object-less, type-less, little-endian SerializedFile of `version`, laid
/// out to rabex 0.2.1's own reader gates: the Unity version at 9 and later,
/// the target platform at 10, `enable_type_tree` at 11, `bigIDEnabled` at
/// 7-13, script types at 11, ref types at 20, and the large-file header at 22.
fn empty_serialized_file(version: u32) -> Vec<u8> {
    let mut m = Vec::new();
    if version >= 9 {
        m.extend_from_slice(b"4.7.2f1\0");
    }
    if version >= 10 {
        m.extend_from_slice(&19i32.to_le_bytes()); // target platform
    }
    if version >= 11 {
        m.push(0); // enable_type_tree
    }
    m.extend_from_slice(&0i32.to_le_bytes()); // types
    if (7..14).contains(&version) {
        m.extend_from_slice(&0i32.to_le_bytes()); // bigIDEnabled
    }
    m.extend_from_slice(&0i32.to_le_bytes()); // objects
    if version >= 11 {
        m.extend_from_slice(&0i32.to_le_bytes()); // script types
    }
    m.extend_from_slice(&0i32.to_le_bytes()); // externals
    if version >= 20 {
        m.extend_from_slice(&0i32.to_le_bytes()); // ref types
    }
    m.push(0); // user information, ""
    let header_len: usize = if version >= 22 { 48 } else { 20 };
    let data_offset = (header_len + m.len()).div_ceil(16) * 16;
    let metadata_size = u32::try_from(m.len()).unwrap();
    let mut out = Vec::new();
    if version >= 22 {
        out.extend_from_slice(&[0; 8]); // the old metadata and file sizes, unused
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&[0; 4]); // the old data offset, unused
        out.extend_from_slice(&[0; 4]); // little-endian, 3 reserved bytes
        out.extend_from_slice(&metadata_size.to_be_bytes());
        let offset = i64::try_from(data_offset).unwrap();
        out.extend_from_slice(&offset.to_be_bytes()); // file size: no objects
        out.extend_from_slice(&offset.to_be_bytes()); // data offset
        out.extend_from_slice(&0i64.to_be_bytes()); // unknown
    } else {
        let offset = u32::try_from(data_offset).unwrap();
        out.extend_from_slice(&metadata_size.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes()); // file size: no objects
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes()); // data offset
        out.extend_from_slice(&[0; 4]); // little-endian, 3 reserved bytes
    }
    out.extend_from_slice(&m);
    out.resize(data_offset, 0);
    out
}

/// A panic payload's message.
fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

/// `write_serialized` on what rabex has just read, for SerializedFile
/// versions 9 to 22: one row per version, with the re-save's outcome
/// ("equal", "differs", "error: …", "panic: …", or "read error: …").
fn writer_versions() -> Vec<JsonValue> {
    use rabex::files::serializedfile::write_serialized;
    (9..=22)
        .map(|version| {
            let data = empty_serialized_file(version);
            let outcome = match SerializedFile::from_reader(&mut Cursor::new(&data)) {
                Err(e) => format!("read error: {e}"),
                Ok(sf) => {
                    let map = HashMap::new();
                    let r = catch_unwind(AssertUnwindSafe(|| {
                        let mut w = Cursor::new(Vec::new());
                        write_serialized(&mut w, &sf, &data, &map).map(|()| w.into_inner())
                    }));
                    match r {
                        Ok(Ok(b)) if b == data => "equal".to_owned(),
                        Ok(Ok(_)) => "differs".to_owned(),
                        Ok(Err(e)) => format!("error: {e}"),
                        Err(p) => format!("panic: {}", panic_message(&*p)),
                    }
                }
            };
            json!({"version": version, "resave": outcome})
        })
        .collect()
}

/// A node and its subtree as rows, or `None`.
fn flat(n: Option<&TypeTreeNode>) -> Option<Vec<JsonValue>> {
    n.map(|n| {
        let mut rows = Vec::new();
        tpk_rows(n, 0, &mut rows);
        rows
    })
}

/// The two-version reproduction of rabex's cache key: one fresh cache per
/// class, asked for an old version and then a new one, against the TPK's own
/// tree for each version. rabex's `sync::TypeTreeCache` serves the new
/// version the old tree; the spike's [`VersionedCache`] does not.
fn cache_probe(path: &Path) -> Result<JsonValue, String> {
    let blob = tpk::load_blob(path)?;
    let (old, new): (UnityVersion, UnityVersion) =
        ("5.6.7f1".parse().unwrap(), "6000.3.17f1".parse().unwrap());
    let mut rows = Vec::new();
    for class in [1, 21, 28, 48, 114, 115, 128] {
        let id = ClassId(class);
        let (b_old, b_new) = (
            flat(blob.get_typetree_node(id, &old).as_ref()),
            flat(blob.get_typetree_node(id, &new).as_ref()),
        );
        let rabex = TypeTreeCache::new(tpk::load_blob(path)?);
        let r_old = flat(rabex.get_typetree_node(id, &old).as_deref());
        let r_new = flat(rabex.get_typetree_node(id, &new).as_deref());
        let ours = VersionedCache::new(MaybeTpk(Some(tpk::load_blob(path)?)));
        let o_old = flat(ours.get_typetree_node(id, &old).as_deref());
        let o_new = flat(ours.get_typetree_node(id, &new).as_deref());
        rows.push(json!({
            "class_id": class,
            "tpk_nodes": [b_old.as_ref().map(Vec::len), b_new.as_ref().map(Vec::len)],
            "tpk_trees_differ": b_old != b_new,
            "rabex_cache_new_is_tpk_new": r_new == b_new,
            "rabex_cache_new_is_tpk_old": r_new == b_old,
            "rabex_cache_old_is_tpk_old": r_old == b_old,
            "versioned_cache_new_is_tpk_new": o_new == b_new,
            "versioned_cache_old_is_tpk_old": o_old == b_old,
        }));
    }
    Ok(json!({"versions": [old.to_string(), new.to_string()], "classes": rows}))
}

/// `rabex-dump probe INDEX.json --index --build B --tpk PATH`.
pub fn probe(a: &Args) -> Result<(), String> {
    let path = a.tpk.as_deref().ok_or("probe needs --tpk PATH")?;
    if !a.index || a.builds.is_none() {
        return Err("probe needs INDEX.json --index --build B".into());
    }
    let blob = tpk::load_blob(path)?;
    let tpk = tpk::load(Some(path))?;
    let out = json!({
        "panics": panics(&blob),
        "sf_writer": writer_versions(),
        "send_sync": send_sync(),
        "environment": environment_probe(a, &tpk)?,
        "tpk": tpk_probe(&blob),
        "cache": cache_probe(path)?,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    Ok(())
}

// ------------------------------------------------------------ value cases

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(s.get(i..i + 2).ok_or("odd hex")?, 16).map_err(|e| e.to_string())
        })
        .collect()
}

/// A tree from `[level, type, name, meta_flag]` rows in pre-order.
fn tree(rows: &[JsonValue]) -> Result<TypeTreeNode, String> {
    fn build(rows: &[JsonValue], i: &mut usize) -> Result<TypeTreeNode, String> {
        let row = rows[*i].as_array().ok_or("row is not a list")?;
        let level = row[0].as_u64().ok_or("level")?;
        let int = |v: &JsonValue| v.as_i64().and_then(|v| i32::try_from(v).ok()).ok_or("meta");
        let mut node = TypeTreeNode {
            m_Type: row[1].as_str().ok_or("type")?.to_owned(),
            m_Name: row[2].as_str().ok_or("name")?.to_owned(),
            m_Version: 1,
            m_ByteSize: -1,
            m_MetaFlag: Some(int(&row[3])?),
            ..TypeTreeNode::default()
        };
        *i += 1;
        while *i < rows.len() && rows[*i][0].as_u64() == Some(level + 1) {
            node.children.push(build(rows, i)?);
        }
        Ok(node)
    }
    build(rows, &mut 0)
}

/// Reads synthetic typetree cases (`scripts/value_cases.py` writes them)
/// through rabex and prints one JSON line per case.
pub fn values(path: &Path) -> Result<(), String> {
    use rabex::files::serializedfile::Endianness;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let cases: JsonValue = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    for case in cases.as_array().ok_or("cases is not a list")? {
        let root = tree(case["nodes"].as_array().ok_or("nodes")?)?;
        let data = unhex(case["hex"].as_str().ok_or("hex")?)?;
        let endian = if case["endian"] == ">" {
            Endianness::Big
        } else {
            Endianness::Little
        };
        let (mut kind, mut note, mut read, mut value, mut resave, mut resave_hex) = (
            JsonValue::Null,
            JsonValue::Null,
            JsonValue::Null,
            JsonValue::Null,
            "skipped".to_owned(),
            JsonValue::Null,
        );
        match read_value(&data, &root, endian) {
            Err((k, msg)) => {
                kind = json!(k);
                note = json!(msg);
            }
            Ok((v, n)) => {
                if n < data.len() {
                    kind = json!("check_read");
                    read = json!(n);
                }
                let mut out = String::new();
                match crate::encode::Encoder::default().encode(&mut out, &v, &root) {
                    Ok(()) => value = json!(out),
                    Err(e) => {
                        kind = json!("encode");
                        note = json!(e);
                    }
                }
                if kind.is_null() {
                    let ser = crate::value::Ser { v: &v, node: &root };
                    let r = catch_unwind(AssertUnwindSafe(|| {
                        rabex::serde_typetree::to_vec_endianed(&ser, &root, endian)
                    }));
                    resave = match r {
                        Ok(Ok(b)) => {
                            resave_hex = json!(crate::json::hex(&b));
                            if b == data { "equal" } else { "differs" }.into()
                        }
                        Ok(Err(e)) => format!("error: {e}"),
                        Err(_) => "error: panic".into(),
                    };
                }
            }
        }
        println!(
            "{}",
            json!({"name": case["name"], "kind": kind, "bytes_read": read, "value": value, "resave": resave, "resave_hex": resave_hex, "note": note})
        );
    }
    Ok(())
}

// -------------------------------------------------------------------- save

/// Re-saves each input through rabex's writers: every SerializedFile entry
/// with `write_serialized` (object bytes unchanged), then the bundle with
/// `write_bundle`, uncompressed. `scripts/save_compare.py` compares the
/// results with the originals and with UnityPy's `save(packer="none")`.
///
/// The output holds whole re-saved game bundles, so it must lie outside the
/// repository. The header shim applies only with `--shim-fs6`, and
/// `write_serialized`'s common-string map needs a TPK (`--tpk`).
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // a probe script: one linear pass
pub fn save(a: &Args) -> Result<(), String> {
    use rabex::files::bundlefile::{CompressionType, write_bundle};
    use rabex::files::serializedfile::{build_common_offset_map, write_serialized};
    use rabex::files::unityfile::FileEntry;
    let out_dir = outside_repo(&a.output.clone().ok_or("save needs -o DIR")?)?;
    let blob = a.tpk.as_deref().map(tpk::load_blob).transpose()?;
    let inputs = collect_inputs(a)?;
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut report = Vec::new();
    for (abs, rel, _) in inputs {
        let data = std::fs::read(&abs).map_err(|e| e.to_string())?;
        let input = input_bytes(&data, a.shim);
        let r = catch_unwind(AssertUnwindSafe(|| -> Result<JsonValue, String> {
            let b =
                BundleFile::from_reader(&mut Cursor::new(&*input), &ExtractionConfig::default())
                    .map_err(|e| e.to_string())?;
            let mut files = Vec::new();
            let mut body = Vec::new();
            let mut entries = Vec::new();
            for e in &b.m_DirectoryInfo {
                let (s, n) = (
                    usize::try_from(e.offset).map_err(|x| x.to_string())?,
                    usize::try_from(e.size).map_err(|x| x.to_string())?,
                );
                let bytes = &b.m_BlockData[s..s + n];
                let mut out = bytes.to_vec();
                if parsed_as(check_file_type(bytes), &e.path) == "SerializedFile" {
                    let sf = SerializedFile::from_reader(&mut Cursor::new(bytes))
                        .map_err(|x| x.to_string())?;
                    let r = catch_unwind(AssertUnwindSafe(|| {
                        let blob = blob
                            .as_ref()
                            .ok_or("write_serialized needs a TPK (--tpk)")?;
                        let version = sf
                            .m_UnityVersion
                            .clone()
                            .or_else(|| b.m_Header.unity_revision.clone())
                            .ok_or("no unity version")?;
                        let map = build_common_offset_map(blob, &version);
                        let mut w = Cursor::new(Vec::new());
                        write_serialized(&mut w, &sf, bytes, &map).map_err(|x| x.to_string())?;
                        Ok::<_, String>(w.into_inner())
                    }));
                    let saved = match r {
                        Ok(r) => r,
                        Err(p) => Err(format!(
                            "panic: {}",
                            p.downcast_ref::<String>()
                                .map_or("", |s| s.lines().next().unwrap_or_default())
                        )),
                    };
                    entries.push(match saved {
                        Ok(s) => {
                            let first = s.iter().zip(bytes).position(|(x, y)| x != y).unwrap_or(s.len().min(bytes.len()));
                            let equal = s == bytes;
                            out = s;
                            json!({"entry": e.path, "equal": equal, "size": bytes.len(), "resaved_size": out.len(), "first_diff": if equal { JsonValue::Null } else { json!(first) }})
                        }
                        Err(x) => json!({"entry": e.path, "error": x}),
                    });
                }
                let (offset, size) = (i64::try_from(body.len()), i64::try_from(out.len()));
                files.push(FileEntry {
                    offset: offset.map_err(|x| x.to_string())?,
                    size: size.map_err(|x| x.to_string())?,
                    flags: e.flags,
                    path: e.path.clone(),
                });
                body.extend_from_slice(&out);
            }
            let mut w = Cursor::new(Vec::new());
            write_bundle(
                &b.m_Header,
                &mut w,
                CompressionType::None,
                CompressionType::None,
                &files,
                &body,
            )
            .map_err(|x| x.to_string())?;
            let name = rel.replace('/', "__");
            std::fs::write(out_dir.join(&name), w.get_ref()).map_err(|x| x.to_string())?;
            Ok(json!({"input": rel, "saved": name, "serialized": entries}))
        }));
        report.push(match r {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => json!({"input": rel, "error": e}),
            Err(_) => json!({"input": rel, "error": "panic"}),
        });
    }
    let report = json!({"shim_fs6": a.shim, "bundles": report});
    std::fs::write(
        out_dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-block, entry-less UnityFS bundle at format 6 (2018.4): the
    /// header rabex 0.2.1 reads one byte too far into.
    fn format6_bundle() -> Vec<u8> {
        let mut info = vec![0u8; 16]; // uncompressed data hash
        info.extend_from_slice(&1i32.to_be_bytes()); // one block
        info.extend_from_slice(&4u32.to_be_bytes()); // uncompressed
        info.extend_from_slice(&4u32.to_be_bytes()); // compressed
        info.extend_from_slice(&0u16.to_be_bytes()); // stored
        info.extend_from_slice(&0i32.to_be_bytes()); // no entries
        let mut b = b"UnityFS\0".to_vec();
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\0");
        b.extend_from_slice(b"2018.4.4f1\0");
        let size = b.len() + 8 + 12 + info.len() + 4;
        b.extend_from_slice(&i64::try_from(size).unwrap().to_be_bytes());
        let n = u32::try_from(info.len()).unwrap();
        b.extend_from_slice(&n.to_be_bytes()); // blocks-info compressed size
        b.extend_from_slice(&n.to_be_bytes()); // blocks-info uncompressed size
        b.extend_from_slice(&0x40u32.to_be_bytes()); // combined, not compressed
        b.extend_from_slice(&info);
        b.extend_from_slice(b"data");
        b
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("unityrs-spike-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn save_report(input: &Path, out: &Path, shim: bool) -> JsonValue {
        let mut a = Args::new("save");
        a.input = input.to_owned();
        a.output = Some(out.to_owned());
        a.shim = shim;
        save(&a).unwrap();
        let text = std::fs::read_to_string(out.join("report.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn save_measures_rabex_as_published_unless_shimmed() {
        let dir = scratch("save-shim");
        let input = dir.join("fs6.bundle");
        std::fs::write(&input, format6_bundle()).unwrap();
        let plain = save_report(&input, &dir.join("plain"), false);
        assert_eq!(plain["shim_fs6"], json!(false));
        assert!(plain["bundles"][0]["error"].is_string(), "{plain}");
        let shimmed = save_report(&input, &dir.join("shimmed"), true);
        assert_eq!(shimmed["shim_fs6"], json!(true));
        assert!(shimmed["bundles"][0]["saved"].is_string(), "{shimmed}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rabex_writer_panics_below_serialized_file_version_14() {
        // rabex 0.2.1 reads these files, then its writer unwraps fields its
        // reader never set: the target platform at 9 (read from 10), and
        // bigIDEnabled at 10-13 (read and discarded).
        let rows = writer_versions();
        let outcome = |v: u64| {
            rows.iter()
                .find(|r| r["version"] == v)
                .and_then(|r| r["resave"].as_str())
                .unwrap()
                .to_owned()
        };
        for v in 9..=13 {
            let o = outcome(v);
            assert!(
                o.starts_with("panic: ") && o.contains("unwrap()"),
                "v{v}: {o}"
            );
        }
        for v in 14..=22 {
            assert_eq!(outcome(v), "equal", "v{v}");
        }
    }

    #[test]
    fn save_refuses_to_write_into_the_repository() {
        let dir = scratch("save-guard");
        let input = dir.join("fs6.bundle");
        std::fs::write(&input, format6_bundle()).unwrap();
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("save-guard-test-out");
        let mut a = Args::new("save");
        a.input = input;
        a.output = Some(out.clone());
        let err = save(&a).unwrap_err();
        assert!(
            err.contains("refusing to write into the repository"),
            "{err}"
        );
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
