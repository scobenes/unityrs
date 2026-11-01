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
mod pool;
mod preload;
mod reach;
mod resources;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::Meter;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, FileContext, ObjectRef, RawStr};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};
use unityrs_testkit::scratch::Scratch;

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

/// A file in a scratch folder of its own, removed with it on drop
/// (`unityrs_testkit::scratch`).
struct TempFile(
    PathBuf,
    #[allow(dead_code, reason = "held for its drop, which removes the folder")] Scratch,
);

impl TempFile {
    fn new(tag: &str, data: &[u8]) -> Self {
        let dir = Scratch::new(&format!("env-{tag}"));
        let path = dir.join(tag);
        std::fs::write(&path, data).unwrap();
        Self(path, dir)
    }
}

fn named_bundle(cab: &str, pad: usize) -> Vec<u8> {
    bundle(&[
        (cab, serialized(&[], &[Obj::named(1, cab, &[])])),
        ("pad.resS", res_bytes(pad, 1)),
    ])
}

/// A temporary directory, removed with what it holds when dropped
/// (`unityrs_testkit::scratch`).
struct TempDir(
    PathBuf,
    #[allow(dead_code, reason = "held for its drop, which removes the folder")] Scratch,
);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = Scratch::new(&format!("env-{tag}"));
        Self(dir.to_path_buf(), dir)
    }
}

/// A budget that grants inputs anything and dependencies up to a limit,
/// recording the most the dependencies held at once.
#[derive(Debug)]
pub(crate) struct DepPool {
    pub(crate) limit: AtomicU64,
    pub(crate) used: Arc<PoolHome>,
}

#[derive(Debug, Default)]
pub(crate) struct PoolHome {
    pub(crate) used: AtomicU64,
    pub(crate) peak: AtomicU64,
}

impl unityrs_core::budget::PermitHome for PoolHome {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl DepPool {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            limit: AtomicU64::new(limit),
            used: Arc::new(PoolHome::default()),
        }
    }
}

impl MemoryBudget for DepPool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Input {
            return Ok(Permit::detached(bytes));
        }
        let limit = self.limit.load(Ordering::Acquire);
        let used = &self.used.used;
        let now =
            used.try_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                u.checked_add(bytes).filter(|n| *n <= limit)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })? + bytes;
        self.used.peak.fetch_max(now, Ordering::AcqRel);
        Ok(Permit::new(bytes, self.used.clone()))
    }
}
