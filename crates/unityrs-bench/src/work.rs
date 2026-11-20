//! The Rust workloads (plan section 4.2): `unityrs-bench run WORKLOAD
//! FIXTURE`, one per row of `tools/bench/workloads.json`, each in three
//! phases.
//!
//! 1. [`setup`], neither timed nor counted: the fixture's bytes in memory
//!    and, for the API rows after `load`, the environment loaded and the
//!    objects listed.
//! 2. The scope, the only part timed and counted: `#[inline(never)] fn
//!    run_<name>` ([`SCOPES`]), whose outputs are held in memory. The gate
//!    counts only it, under callgrind's `--toggle-collect` of
//!    `*unityrs_bench::work::run_<name>*` (`tools/bench/callgrind.py`).
//! 3. [`digest`], after the scope: the outputs hashed as
//!    `tools/bench/py_workloads.py` hashes them (the object table, the entry
//!    bytes, and each image's mode, size and pixels), but for the typetree
//!    row, whose values are hashed in the canonical dump encoding
//!    (`docs/CANONICAL_DUMP.md`) instead of the script's JSON.
//!
//! Every scope calls its operation with its own name, so no two have the
//! same body: identical functions may be merged into one symbol, and the
//! toggle would then name a scope that is never entered.

use std::hint::black_box;
use std::sync::Arc;

use serde_json::{Map, Value as Json, json};
use sha2::{Digest, Sha256};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::{Bytes, File, ObjectRef, Parsed, RawStr, Value};
use unityrs_texture::{Format, Image, TextureFields};

/// What a scope reads, made by [`setup`].
pub enum Input {
    /// The fixture's bytes (the load rows).
    Bytes(Bytes),
    /// The environment that loaded the fixture, and the objects the scope
    /// reads (every object, or every `Texture2D`).
    Objects(Environment, Vec<ObjectRef>),
}

/// What a scope made, held until [`digest`] reads it.
pub enum Output {
    /// The environment, and each object's path id, class id and byte size.
    Table(Environment, Vec<(i64, i32, u32)>),
    /// The environment, and every non-serialized entry's bytes.
    Entries(Environment, Vec<Bytes>),
    /// Each object's `parse_as_dict()` value.
    Values(Vec<Parsed>),
    /// Each texture's image.
    Images(Vec<Image>),
}

/// The operation a workload's scope runs (`workloads.json`'s `op`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Load from bytes; list every file's object table.
    Load,
    /// Load from bytes; read every non-serialized entry's bytes.
    LoadEntries,
    /// `parse_as_dict()` on every object.
    Typetree,
    /// Each `Texture2D` read and decoded.
    Texture,
}

/// Why a phase failed.
pub type Failure = String;

/// A scope: the workload's operation on its input.
pub type Scope = fn(&Input) -> Result<Output, Failure>;

/// A scope's function name: `run_`, then the workload's name lower-cased
/// with every character but a letter or digit made `_`. `callgrind.py`
/// derives the same.
#[must_use]
pub fn scope_name(workload: &str) -> String {
    let id: String = workload
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("run_{id}")
}

macro_rules! scopes {
    ($($name:literal => $func:ident : $op:ident,)*) => {
        $(
            #[doc = concat!("The scope of `", $name, "`.")]
            #[inline(never)]
            pub fn $func(input: &Input) -> Result<Output, Failure> {
                operate(black_box($name), Op::$op, input)
            }
        )*

        /// Every workload: its name, its scope's function name, its
        /// operation and its scope.
        pub const SCOPES: &[(&str, &str, Op, Scope)] = &[
            $(($name, stringify!($func), Op::$op, $func),)*
        ];
    };
}

scopes! {
    "load-objects-lz4" => run_load_objects_lz4: Load,
    "load-objects-lzma" => run_load_objects_lzma: Load,
    "load-objects-2019" => run_load_objects_2019: Load,
    "load-many-blocks" => run_load_many_blocks: LoadEntries,
    "typetree-objects-lz4" => run_typetree_objects_lz4: Typetree,
    "typetree-objects-2019" => run_typetree_objects_2019: Typetree,
    "texture-RGBA32" => run_texture_rgba32: Texture,
    "texture-RGB24" => run_texture_rgb24: Texture,
    "texture-ARGB4444" => run_texture_argb4444: Texture,
    "texture-DXT1" => run_texture_dxt1: Texture,
    "texture-BC4" => run_texture_bc4: Texture,
    "texture-ETC_RGB4" => run_texture_etc_rgb4: Texture,
    "texture-EAC_R" => run_texture_eac_r: Texture,
    "texture-PVRTC_RGBA4" => run_texture_pvrtc_rgba4: Texture,
    "texture-DXT5" => run_texture_dxt5: Texture,
    "texture-BC5" => run_texture_bc5: Texture,
    "texture-BC6H" => run_texture_bc6h: Texture,
    "texture-BC7" => run_texture_bc7: Texture,
    "texture-ETC2_RGBA8" => run_texture_etc2_rgba8: Texture,
    "texture-ATC_RGBA8" => run_texture_atc_rgba8: Texture,
    "texture-ASTC_RGBA_4x4" => run_texture_astc_rgba_4x4: Texture,
    "texture-ASTC_RGBA_8x8" => run_texture_astc_rgba_8x8: Texture,
}

/// The workload named `name`: its operation and scope.
#[must_use]
pub fn find(name: &str) -> Option<(Op, Scope)> {
    SCOPES
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|&(_, _, op, scope)| (op, scope))
}

/// The key a fixture is loaded under (UnityPy's `stream_name` of bytes).
const NAME: &str = "fixture";

fn load(data: &Bytes) -> Result<Environment, Failure> {
    let env = Environment::new(EnvOptions::unitypy());
    env.load(Source::Bytes {
        name: RawStr::from(NAME),
        data: data.clone(),
    })
    .map_err(|e| format!("load: {e}"))?;
    Ok(env)
}

fn objects(env: &Environment) -> Result<Vec<ObjectRef>, Failure> {
    env.objects()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("objects: {e}"))
}

/// The scope's input for `op` from the fixture's `data`. A load row's
/// setup is the bytes alone; an API row's loads them, lists the objects
/// (keeping the `Texture2D`s for the texture row), then loads once more
/// and drops it, the warm-up `py_workloads.py`'s `warm` does for the
/// first use's one-time costs.
pub fn setup(op: Op, data: Bytes) -> Result<Input, Failure> {
    if matches!(op, Op::Load | Op::LoadEntries) {
        drop(load(&data)?);
        return Ok(Input::Bytes(data));
    }
    let env = load(&data)?;
    let mut objs = objects(&env)?;
    if op == Op::Texture {
        objs.retain(|o| o.class_id() == 28);
    }
    Ok(Input::Objects(env, objs))
}

/// Runs `op` on `input`; `name` is the calling scope's, which keeps each
/// scope's body its own.
fn operate(name: &str, op: Op, input: &Input) -> Result<Output, Failure> {
    black_box(name);
    match (op, input) {
        (Op::Load, Input::Bytes(data)) => {
            let env = load(data)?;
            let table = env
                .objects()
                .map(|o| o.map(|o| (o.path_id(), o.class_id(), o.info().byte_size)))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("objects: {e}"))?;
            Ok(Output::Table(env, table))
        }
        (Op::LoadEntries, Input::Bytes(data)) => {
            let env = load(data)?;
            let entries = entries(&env)?;
            Ok(Output::Entries(env, entries))
        }
        (Op::Typetree, Input::Objects(_, objs)) => objs
            .iter()
            .map(|o| {
                o.parse_as_dict(true)
                    .map_err(|e| format!("parse_as_dict: {e}"))
            })
            .collect::<Result<_, _>>()
            .map(Output::Values),
        (Op::Texture, Input::Objects(_, objs)) => objs
            .iter()
            .map(texture)
            .collect::<Result<_, _>>()
            .map(Output::Images),
        _ => Err(format!("{name}: the input is not the operation's")),
    }
}

/// Every non-serialized entry's bytes, in `files` order, nested containers
/// entered depth first (`py_workloads.py`'s `files_of`).
fn entries(env: &Environment) -> Result<Vec<Bytes>, Failure> {
    fn walk(file: &File, out: &mut Vec<Bytes>) -> Result<(), Failure> {
        match file {
            File::Raw(raw) => out.push(raw.data.clone()),
            File::Bundle(_) | File::Web(_) => {
                for i in 0..file.container_files().len() {
                    let entry = file
                        .container_entry(i)
                        .map_err(|e| format!("entry {i}: {e}"))?;
                    walk(&entry, out)?;
                }
            }
            File::Serialized(_) => {}
        }
        Ok(())
    }
    let mut out = Vec::new();
    for entry in env.files() {
        walk(&entry.file, &mut out)?;
    }
    Ok(out)
}

fn field<'a>(p: &'a Parsed, name: &[u8]) -> Result<&'a Value, Failure> {
    p.value()
        .as_dict()
        .and_then(|m| p.attr(m, name))
        .ok_or_else(|| format!("no {}", String::from_utf8_lossy(name)))
}

fn int(p: &Parsed, name: &[u8]) -> Result<i32, Failure> {
    field(p, name)?
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| format!("{} is not an int", String::from_utf8_lossy(name)))
}

/// `obj.read().image`: the object read as the wheel reads it, then its
/// inline data decoded in the format it names.
fn texture(obj: &ObjectRef) -> Result<Image, Failure> {
    let (p, _) = obj.read_object_fields().map_err(|e| format!("read: {e}"))?;
    let id = int(&p, b"m_TextureFormat")?;
    let fields = TextureFields {
        texture_format: id,
        width: int(&p, b"m_Width")?,
        height: int(&p, b"m_Height")?,
        platform: obj.file().target_platform().unwrap_or(0),
        platform_blob: None,
    };
    let data = match field(&p, b"image_data")? {
        Value::Bytes(b) => b.clone(),
        _ => return Err("image_data is not bytes".into()),
    };
    let format = Format::from_texture_format(id).ok_or_else(|| format!("format {id}"))?;
    let meter: &Arc<_> = obj.file().meter();
    unityrs_texture::decode_in_as(format, &fields, &data, meter).map_err(|e| format!("decode: {e}"))
}

fn finish(h: Sha256) -> String {
    use std::fmt::Write as _;
    h.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The output's digest and counts (what the scope processed, beside the
/// source bytes the caller adds), as `py_workloads.py` names them.
pub fn digest(out: &Output) -> Result<(String, Map<String, Json>), Failure> {
    let mut h = Sha256::new();
    let counts = match out {
        Output::Table(_, table) => {
            for (path_id, class_id, size) in table {
                h.update(format!("{path_id},{class_id},{size}\n").as_bytes());
            }
            json!({"objects": table.len()})
        }
        Output::Entries(_, entries) => {
            for e in entries {
                h.update(e);
            }
            let bytes: usize = entries.iter().map(Bytes::len).sum();
            json!({"entries": entries.len(), "entry_bytes": bytes})
        }
        Output::Values(values) => {
            for v in values {
                let text = unityrs_dump::encode::encode(v).map_err(|e| format!("encode: {e}"))?;
                h.update(text.as_bytes());
                h.update(b"\n");
            }
            json!({"objects": values.len()})
        }
        Output::Images(images) => {
            let mut pixels = 0u64;
            for i in images {
                let px = i.pixels();
                h.update(
                    format!(
                        "{} {}x{} {}\n",
                        i.mode().name(),
                        i.width(),
                        i.height(),
                        px.len()
                    )
                    .as_bytes(),
                );
                h.update(px);
                pixels += u64::from(i.width()) * u64::from(i.height());
            }
            json!({"textures": images.len(), "pixels": pixels})
        }
    };
    match counts {
        Json::Object(m) => Ok((finish(h), m)),
        _ => Err("counts".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scope_is_named_for_its_workload_and_none_prefixes_another() {
        for (name, func, ..) in SCOPES {
            assert_eq!(scope_name(name), *func, "{name}");
        }
        for (a, fa, ..) in SCOPES {
            for (b, fb, ..) in SCOPES {
                assert!(a == b || !fb.starts_with(fa), "{fa} prefixes {fb}");
            }
        }
    }
}
