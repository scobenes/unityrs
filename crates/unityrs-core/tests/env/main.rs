//! The Environment's own rules (plan sections 3.13 and 7.5): the load modes'
//! differences, dereferences outside the oracle's reach, resources located
//! without parsing, the dependency LRU and its pool, and the lock rules.
//! What UnityPy decides is replayed from its record in `env_fixtures.rs`.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod container;
mod deref;
mod files;
mod locks;
mod lru;
mod modes;
mod preload;
mod resources;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::MemoryBudget;
use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::Meter;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, FileContext, ObjectRef, RawStr};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

fn src(name: &str, data: &[u8]) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: Bytes::copy_from_slice(data),
    }
}

fn key(s: &str) -> RawStr {
    RawStr::from(s)
}

fn unlimited() -> Arc<Meter> {
    Meter::unlimited()
}

fn env(mode: LoadMode) -> Environment {
    let mut o = EnvOptions::unitypy();
    o.load = mode;
    Environment::new(o)
}

fn ctx_with(budget: Arc<dyn MemoryBudget>) -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    c.budget = budget;
    Arc::new(c)
}

fn batch(ctx: Arc<FileContext>, dep_cache_bytes: u64) -> Environment {
    let mut o = EnvOptions::batch(ctx);
    o.dep_cache_bytes = dep_cache_bytes;
    Environment::new(o)
}

fn keys(env: &Environment) -> Vec<String> {
    env.registry()
        .keys()
        .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
        .collect()
}

fn serialized_cab(env: &Environment, name: &str) -> Arc<SerializedFile> {
    match env.get_cab(&key(name), &unlimited()).unwrap() {
        Some(Cab::Serialized(sf)) => sf,
        other => panic!("{name}: {other:?}"),
    }
}

fn object(sf: &Arc<SerializedFile>, path_id: i64) -> ObjectRef {
    sf.object(path_id).unwrap()
}

fn name_of(o: &ObjectRef) -> String {
    let p = o.parse_as_dict(true).unwrap();
    match p.get(&[b"m_Name"]) {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    }
}

fn stream(o: &ObjectRef) -> (RawStr, u64, u64) {
    let p = o.parse_as_dict(true).unwrap();
    let Some(Value::Str(path)) = p.get(&[b"m_StreamData", b"path"]) else {
        panic!("no path")
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
    (path.clone(), offset, size)
}

fn file_of(scene: &unityrs_testkit::env::Scene, i: usize) -> Source {
    let (n, d) = &scene.files[i];
    Source::Bytes {
        name: RawStr::new(n.clone()),
        data: Bytes::from(d.clone()),
    }
}

/// A file of the scratch directory this test owns, removed on drop.
struct TempFile(PathBuf);

impl TempFile {
    fn new(tag: &str, data: &[u8]) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unityrs-env-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, data).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn named_bundle(cab: &str, pad: usize) -> Vec<u8> {
    bundle(&[
        (cab, serialized(&[], &[Obj::named(1, cab, &[])])),
        ("pad.resS", res_bytes(pad, 1)),
    ])
}

/// A temporary directory, removed with what it holds when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("unityrs-env-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
