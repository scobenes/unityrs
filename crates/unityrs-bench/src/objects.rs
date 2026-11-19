//! The object sets (plan section 2): 20,000 objects (1,000 at the gate
//! scale) of ten classes, each with its typetree embedded, about 1.2 KB
//! decoded, in three files:
//!
//! - `objects-lz4.bundle`: 2022.3, LZ4HC blocks of 128 KiB (liblz4's HC
//!   at level 9, as Unity writes them);
//! - `objects-lzma.bundle`: the same, one LZMA block;
//! - `objects-2019.assets`: the same objects, uncompressed, in the 2019.4
//!   layout (`SerializedFile` format 21).
//!
//! An object's fields draw on small vocabularies and quantised values, so
//! the data compresses about as an asset's does, with 32 seeded raw bytes
//! in each that do not compress.

use serde_json::{Map, Value, json};
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::TestCodec;
use unityrs_testkit::ngui::{Object, V, pptr, st, vector};

use crate::{Fixture, Scale, UNITY_2019, UNITY_VERSION, bundle, bundle_lz4hc, seed_of, serialized};

/// The three files, in build order.
pub const NAMES: [&str; 3] = [
    "objects-lz4.bundle",
    "objects-lzma.bundle",
    "objects-2019.assets",
];

/// The ten classes: id and name (the root type).
pub const CLASSES: [(i32, &str); 10] = [
    (1, "GameObject"),
    (4, "Transform"),
    (20, "Camera"),
    (21, "Material"),
    (23, "MeshRenderer"),
    (33, "MeshFilter"),
    (49, "TextAsset"),
    (54, "Rigidbody"),
    (65, "BoxCollider"),
    (108, "Light"),
];

/// Words names and tags are made of.
const WORDS: [&str; 16] = [
    "alpha", "beta", "gamma", "delta", "root", "body", "head", "hand", "foot", "light", "shadow",
    "mesh", "rock", "tree", "water", "cloud",
];

/// How many objects at `scale`.
#[must_use]
pub fn count(scale: Scale) -> usize {
    scale.pick(20_000, 1_000)
}

/// A float from 16 quarter steps: a value an asset repeats.
fn quarter(rng: &mut Rng) -> V {
    let q = u8::try_from(rng.below(16)).unwrap_or(0);
    V::Float(f32::from(q) * 0.25 - 2.0)
}

fn word(rng: &mut Rng) -> &'static str {
    WORDS
        .get(rng.below(WORDS.len()))
        .copied()
        .unwrap_or("alpha")
}

/// `n` values from `make`, each repeated four times.
fn runs(n: usize, rng: &mut Rng, make: impl Fn(&mut Rng) -> V) -> Vec<V> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let v = make(rng);
        for _ in 0..4.min(n - out.len()) {
            out.push(v.clone());
        }
    }
    out
}

/// Object `i` (path id `i + 1`) of `total`, of class `i % 10`.
fn object(i: usize, total: usize, rng: &mut Rng) -> Object {
    let (class_id, class) = CLASSES
        .get(i % CLASSES.len())
        .copied()
        .unwrap_or((1, "GameObject"));
    let k = i % CLASSES.len();
    let path_id = i64::try_from(i + 1).unwrap_or(i64::MAX);
    let name = format!("{}_{}_{i:05}", word(rng), class.to_ascii_lowercase());
    let refs = (0..4 + k % 3)
        .map(|_| {
            let to = i64::try_from(rng.below(total) + 1).unwrap_or(1);
            pptr("Object", (0, to))
        })
        .collect();
    // Weights and ids come in runs of four, as keyframes and indices do.
    let weights = runs(40 + 16 * k, rng, quarter);
    let ids = runs(64, rng, |rng| {
        V::Int(i32::try_from(rng.below(64)).unwrap_or(0))
    });
    let tags = (0..6)
        .map(|_| V::Str(word(rng).as_bytes().to_vec()))
        .collect();
    let entry = |rng: &mut Rng| {
        st(
            "Entry",
            vec![
                ("key", V::Str(word(rng).as_bytes().to_vec())),
                ("value", quarter(rng)),
                ("flags", V::Int(i32::try_from(rng.below(4)).unwrap_or(0))),
            ],
        )
    };
    let entries = (0..8).map(|_| entry(rng)).collect();
    let blob: Vec<u8> = (0..4).flat_map(|_| rng.next_u64().to_le_bytes()).collect();
    let position = st(
        "Vector3f",
        vec![
            ("x", quarter(rng)),
            ("y", quarter(rng)),
            ("z", quarter(rng)),
        ],
    );
    let matrix = (0..16).map(|_| quarter(rng)).collect();
    let value = st(
        class,
        vec![
            ("m_Name", V::Str(name.into_bytes())),
            ("m_Enabled", V::Bool(rng.below(2) == 1)),
            ("m_Layer", V::Int(i32::try_from(rng.below(32)).unwrap_or(0))),
            ("m_Position", position),
            ("m_Matrix", vector(V::Float(0.0), matrix)),
            ("m_Refs", vector(pptr("Object", (0, 0)), refs)),
            ("m_Weights", vector(V::Float(0.0), weights)),
            ("m_Ids", vector(V::Int(0), ids)),
            ("m_Tags", vector(V::Str(Vec::new()), tags)),
            ("m_Entries", vector(entry(&mut Rng::new(0)), entries)),
            ("m_Blob", V::Bytes(blob)),
        ],
    );
    Object::new(path_id, class_id, value)
}

/// Every object at `scale`, from one seeded stream.
#[must_use]
pub fn values(scale: Scale) -> Vec<Object> {
    let total = count(scale);
    let mut rng = Rng::new(seed_of("objects"));
    (0..total).map(|i| object(i, total, &mut rng)).collect()
}

/// The inventory: objects in all, and by class.
fn inventory(objects: &[Object]) -> Value {
    let mut by_class = Map::new();
    for (id, name) in CLASSES {
        let n = objects.iter().filter(|o| o.class_id == id).count();
        by_class.insert(name.to_owned(), json!(n));
    }
    json!({"kind": "objects", "objects": objects.len(), "serialized_files": 1, "by_class": by_class})
}

/// The file `name` (one of [`NAMES`]) at `scale`.
pub fn build(name: &str, scale: Scale) -> Result<Fixture, String> {
    let objects = values(scale);
    let bytes = if name == "objects-2019.assets" {
        serialized(21, UNITY_2019, &objects).map_err(|e| e.to_string())?
    } else {
        let file = serialized(22, UNITY_VERSION, &objects).map_err(|e| e.to_string())?;
        if name == "objects-lzma.bundle" {
            let block = file.len().max(1);
            let entry = vec![("CAB-bench-objects-lzma".to_owned(), file, 4)];
            bundle(UNITY_VERSION, entry, TestCodec::Lzma, block)
        } else {
            let entry = vec![("CAB-bench-objects-lz4".to_owned(), file, 4)];
            bundle_lz4hc(UNITY_VERSION, entry, 128 << 10)?
        }
    };
    Ok(Fixture {
        name: name.to_owned(),
        bytes,
        inventory: inventory(&objects),
    })
}
