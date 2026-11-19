//! The benchmark suite's fixtures (`docs/plans/bench.md` section 2): public,
//! synthetic inputs, every byte from the testkit's builders and its
//! `SplitMix64` [`Rng`](unityrs_testkit::Rng), so anyone can regenerate them
//! and the manifest's sha256s hold them to the bytes measured.
//!
//! `unityrs-bench gen --scale full|gate DIR` writes each [`Fixture`] and
//! `manifest.json` ([`manifest`]). The full scale is the published
//! comparison; the gate scale shrinks each item's dimensions and keeps at
//! least one item of every kind. `benchmarks/fixtures.json` holds both
//! scales' manifests, and `tests/manifest.rs` regenerates the gate scale
//! against it.
//!
//! - [`objects`]: 20,000 objects of ten classes with embedded trees, in an
//!   LZ4HC bundle, an LZMA bundle and a 2019.4 `.assets` file.
//! - [`many_blocks`]: one 64 MiB entry in 16 KiB LZ4 blocks.
//! - [`textures`]: a bundle per texture format, every block legal by
//!   construction ([`blocks`], [`astc`]).

pub mod astc;
pub mod blocks;
mod files;
pub mod many_blocks;
pub mod objects;
pub mod textures;

pub use files::{BuildError, bundle, serialized};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The engine every fixture but the 2019.4 set is written for.
pub const UNITY_VERSION: &str = "2022.3.62f1";

/// The 2019.4 layout's engine.
pub const UNITY_2019: &str = "2019.4.40f1";

/// The fixture set's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// The published comparison.
    Full,
    /// The CI gate's and the quick run's: every kind, smaller items.
    Gate,
}

impl Scale {
    /// `full` or `gate`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Gate => "gate",
        }
    }

    /// The scale named `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "full" => Some(Self::Full),
            "gate" => Some(Self::Gate),
            _ => None,
        }
    }

    /// `full` at this scale, else `gate`.
    #[must_use]
    pub fn pick<T>(self, full: T, gate: T) -> T {
        match self {
            Self::Full => full,
            Self::Gate => gate,
        }
    }
}

/// One fixture file: its name, its bytes and its inventory (what a run
/// processes, by which throughput is computed).
#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    /// The file name.
    pub name: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// Object counts, pixels and entry sizes, by the fixture's kind.
    pub inventory: Value,
}

/// A seed from a label (64-bit FNV-1a), so each item's stream is its own
/// and a change to one leaves the others' bytes alone.
#[must_use]
pub fn seed_of(label: &str) -> u64 {
    label.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Every fixture's file name (the same at both scales), in the order
/// [`generate`] builds them.
#[must_use]
pub fn names() -> Vec<String> {
    let mut out: Vec<String> = objects::NAMES.iter().map(|n| (*n).to_owned()).collect();
    out.push(many_blocks::NAME.to_owned());
    out.extend(textures::FORMATS.iter().map(textures::file_name));
    out
}

/// The fixture `name` at `scale`, or `None` for a name [`names`] lacks.
#[must_use]
pub fn build(name: &str, scale: Scale) -> Option<Result<Fixture, String>> {
    if objects::NAMES.contains(&name) {
        return Some(objects::build(name, scale));
    }
    if name == many_blocks::NAME {
        return Some(Ok(many_blocks::build(scale)));
    }
    textures::FORMATS
        .iter()
        .find(|f| textures::file_name(f) == name)
        .map(|f| textures::build(f, scale))
}

/// The sha256 of `bytes`, in lower-case hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The manifest of `fixtures` at `scale`: each file's name, bytes and
/// sha256 (sorted by name), and each one's inventory.
#[must_use]
pub fn manifest(scale: Scale, fixtures: &[(String, u64, String, Value)]) -> Value {
    let mut rows: Vec<_> = fixtures.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let files: Vec<Value> = rows
        .iter()
        .map(|(name, bytes, sha, _)| json!({"name": name, "bytes": bytes, "sha256": sha}))
        .collect();
    let inventories: serde_json::Map<String, Value> = rows
        .iter()
        .map(|(name, _, _, inv)| (name.clone(), inv.clone()))
        .collect();
    json!({
        "schema_version": 1,
        "generator": "unityrs-bench gen",
        "scale": scale.name(),
        "unity_version": UNITY_VERSION,
        "files": files,
        "inventories": inventories,
    })
}

/// A manifest row of `fixture`: (name, bytes, sha256, inventory).
#[must_use]
pub fn row(fixture: &Fixture) -> (String, u64, String, Value) {
    (
        fixture.name.clone(),
        u64::try_from(fixture.bytes.len()).unwrap_or(u64::MAX),
        sha256_hex(&fixture.bytes),
        fixture.inventory.clone(),
    )
}

/// Every fixture at `scale`, handed to `sink` one at a time, so the full set
/// is never held in memory at once; then the manifest of them all.
pub fn generate(
    scale: Scale,
    mut sink: impl FnMut(&Fixture) -> std::io::Result<()>,
) -> Result<Value, GenError> {
    let mut rows = Vec::new();
    for name in names() {
        let fixture = build(&name, scale)
            .ok_or_else(|| GenError::Build(format!("{name}: no such fixture")))?
            .map_err(|e| GenError::Build(format!("{name}: {e}")))?;
        sink(&fixture).map_err(GenError::Io)?;
        rows.push(row(&fixture));
    }
    Ok(manifest(scale, &rows))
}

/// Why [`generate`] stopped.
#[derive(Debug)]
pub enum GenError {
    /// A builder refused its value.
    Build(String),
    /// The sink failed.
    Io(std::io::Error),
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(why) => write!(f, "build: {why}"),
            Self::Io(e) => write!(f, "write: {e}"),
        }
    }
}

impl std::error::Error for GenError {}
