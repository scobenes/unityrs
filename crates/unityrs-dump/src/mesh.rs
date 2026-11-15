//! L6 corpus JSONL, paired with `tools/parity/mesh_dump.py`.
//!
//! Arrays hash framed tuple lengths, Python numeric tags and big-endian
//! binary64/i64 values. Triangles hash whole/cut tuples, empty tuple counts
//! and the handler's mutated index counts. OBJ bytes stream into SHA-256.
//! Every variable-sized buffer uses `Text` or `MeteredVec`; work is counted
//! before hashing. Resource/limit refusals propagate and stop the dump.
use crate::{
    DumpOptions, InputDump, json,
    text::{Sink, Text},
};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, io, path::Path, sync::Arc};
use unityrs_core::{
    Bytes, ObjectRef, RawStr,
    detect::{FileType, check_entry_type},
    env::{Cab, EnvOptions, Environment, RawCab},
    files::{File, parse_file},
    limits::{Charge, Meter, MeteredVec},
};
use unityrs_mesh::{
    MeshError, ProcessedOwned, Triangles,
    obj::{MeteredWrite, Written},
};
use unityrs_meshdecode::{Component, VertexArray};

fn u64_len(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
fn length(hash: &mut Sha256, n: usize) {
    hash.update(u64_len(n).to_be_bytes());
}
fn basename(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b'/' || *b == b'\\')
        .next()
        .unwrap_or(name)
}

fn array_digest(a: &VertexArray, meter: &Arc<Meter>) -> Result<[u8; 32], MeshError> {
    meter.count_decode(17u64.saturating_add(u64_len(a.len()).saturating_mul(8)))?;
    let mut hash = Sha256::new();
    hash.update(b"array-v1\0");
    length(&mut hash, a.len());
    for i in 0..a.len() {
        let count = a.tuple_len(i).ok_or(MeshError::Field("tuple"))?;
        meter.count_decode(u64_len(count).saturating_mul(9))?;
        length(&mut hash, count);
        for j in 0..count {
            match a.component(i, j).ok_or(MeshError::Field("component"))? {
                Component::Int(n) => {
                    hash.update(b"i");
                    hash.update(n.to_be_bytes());
                }
                Component::Float(n) => {
                    hash.update(b"f");
                    hash.update(n.to_bits().to_be_bytes());
                }
                _ => return Err(MeshError::Field("component")),
            }
        }
    }
    Ok(hash.finalize().into())
}

fn index_digest(indices: &[u32], meter: &Arc<Meter>) -> Result<[u8; 32], MeshError> {
    meter.count_decode(19u64.saturating_add(u64_len(indices.len()).saturating_mul(4)))?;
    let mut hash = Sha256::new();
    hash.update(b"indices-v1\0");
    length(&mut hash, indices.len());
    for n in indices {
        hash.update(n.to_be_bytes());
    }
    Ok(hash.finalize().into())
}

fn triangles_digest(t: &Triangles, meter: &Arc<Meter>) -> Result<[u8; 32], MeshError> {
    meter.count_decode(21u64.saturating_add(u64_len(t.submeshes().len()).saturating_mul(32)))?;
    let mut hash = Sha256::new();
    hash.update(b"triangles-v1\0");
    length(&mut hash, t.submeshes().len());
    for sub in t.submeshes() {
        meter.count_decode(
            u64_len(sub.triangles().len())
                .saturating_mul(12)
                .saturating_add(u64_len(sub.cut().len()).saturating_mul(4)),
        )?;
        length(&mut hash, sub.triangles().len());
        for tri in sub.triangles() {
            for n in tri {
                hash.update(n.to_be_bytes());
            }
        }
        length(&mut hash, sub.cut().len());
        for n in sub.cut() {
            hash.update(n.to_be_bytes());
        }
        hash.update(sub.empty().to_be_bytes());
        hash.update(sub.index_count().to_be_bytes());
    }
    Ok(hash.finalize().into())
}

fn arrays(out: &mut Text, p: &ProcessedOwned, meter: &Arc<Meter>) -> Result<(), MeshError> {
    let _ = write!(
        out,
        "{{\"vertex_count\":{},\"use16\":{},\"index_buffer\":",
        p.vertex_count(),
        p.use_16bit_indices()
    );
    if let Some(indices) = p.index_buffer() {
        json::write_hex_str(out, &index_digest(indices.as_slice(), meter)?);
    } else {
        out.push_str("null");
    }
    out.push_str(",\"attrs\":{");
    let mut first = true;
    let mut one = |name: &str, a: Option<&VertexArray>| -> Result<(), MeshError> {
        if let Some(a) = a {
            if !first {
                out.push(',');
            }
            first = false;
            json::write_str(out, name);
            out.push(':');
            json::write_hex_str(out, &array_digest(a, meter)?);
        }
        Ok(())
    };
    for (name, a) in [
        ("m_Vertices", p.vertices()),
        ("m_Normals", p.normals()),
        ("m_Tangents", p.tangents()),
        ("m_Colors", p.colors()),
        ("m_BoneWeights", p.bone_weights()),
        ("m_BoneIndices", p.bone_indices()),
    ] {
        one(name, a)?;
    }
    for (i, name) in [
        "m_UV0", "m_UV1", "m_UV2", "m_UV3", "m_UV4", "m_UV5", "m_UV6", "m_UV7",
    ]
    .iter()
    .enumerate()
    {
        one(name, p.uv(i))?;
    }
    one("m_BindPose", p.bind_pose())?;
    out.push_str("}}");
    Ok(())
}

/// SHA-256 output without staging the OBJ text.
struct ObjHash {
    hash: Sha256,
    meter: Arc<Meter>,
}
impl MeteredWrite for ObjHash {
    fn meter(&self) -> &Arc<Meter> {
        &self.meter
    }
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), MeshError> {
        self.meter.count_decode(u64_len(bytes.len()))?;
        self.hash.update(bytes);
        Ok(())
    }
}
impl io::Write for ObjHash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_bytes(bytes)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// Avoid MeshError::python's allocated diagnostic: the protocol needs only
// the exception class. Unknown/new refusals stay explicitly Rust-specific.
fn exception(error: &MeshError) -> Option<&'static str> {
    use unityrs_meshdecode::MeshError as D;
    match error {
        MeshError::Charge(_)
        | MeshError::Core(_)
        | MeshError::Parse(_)
        | MeshError::Decode(D::Charge(_) | D::Resource(_)) => None,
        MeshError::Decode(D::Assertion(_)) => Some("AssertionError"),
        MeshError::Decode(D::NoAttribute(_)) => Some("AttributeError"),
        MeshError::Decode(D::Struct(_)) => Some("error"),
        MeshError::Decode(D::Index(_)) | MeshError::Field("attribute tuple") => Some("IndexError"),
        MeshError::Decode(D::StopIteration) => Some("StopIteration"),
        MeshError::Decode(D::Overflow) => Some("OverflowError"),
        MeshError::Decode(
            D::UnknownMeshType
            | D::VersionType(_)
            | D::InvalidFormat { .. }
            | D::UnknownChannel(_)
            | D::NoChannels
            | D::OutOfBounds
            | D::Unpack { .. }
            | D::ByteRange
            | D::Topology,
        )
        | MeshError::Field("triangle tuple") => Some("ValueError"),
        MeshError::Decode(D::Unsupported(_)) => Some("unityrs:Unsupported"),
        MeshError::Decode(D::WrappedAccess) => Some("unityrs:WrappedAccess"),
        _ => Some("unityrs:Malformed"),
    }
}

fn result(object: &ObjectRef, env: &Environment, meter: &Arc<Meter>) -> Result<Text, MeshError> {
    let m = unityrs_mesh::read_mesh_for_obj(object, env, meter)?;
    let mut obj = ObjHash {
        hash: Sha256::new(),
        meter: meter.clone(),
    };
    let written = unityrs_mesh::obj::export_mesh_obj(&m, None, &mut obj)?;
    let mut out = Text::on(meter);
    out.push_str("{\"arrays\":");
    arrays(&mut out, &m.processed, meter)?;
    out.push_str(",\"triangles\":");
    json::write_hex_str(&mut out, &triangles_digest(&m.triangles, meter)?);
    out.push_str(",\"obj\":");
    match written {
        Written::Text => json::write_hex_str(&mut out, &obj.hash.finalize()),
        Written::False => out.push_str("null"),
    }
    out.push('}');
    Ok(out)
}

/// One mesh row. Its returned charge must live as long as its string.
/// Input/limit/resource failures propagate; matching oracle exceptions are rows.
pub fn record(
    rel: &[u8],
    object: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<(String, Charge), MeshError> {
    let answer = match result(object, env, meter) {
        Ok(answer) => answer,
        Err(error) => {
            let Some(class) = exception(&error) else {
                return Err(error);
            };
            let mut answer = Text::on(meter);
            answer.push_str("{\"error\":");
            json::write_str(&mut answer, class);
            answer.push('}');
            answer
        }
    };
    let (answer, _answer_charge) = answer.finish()?;
    meter.count_decode(
        u64_len(rel.len())
            .saturating_add(u64_len(object.file().name().as_bytes().len()))
            .saturating_add(u64_len(answer.len())),
    )?;
    let mut out = Text::on(meter);
    out.push_str("{\"input\":");
    json::write_name(&mut out, rel);
    out.push_str(",\"file\":");
    json::write_hex_str(&mut out, basename(object.file().name().as_bytes()));
    let _ = write!(out, ",\"path_id\":{},\"result\":", object.path_id());
    out.push_str(&answer);
    out.push('}');
    Ok(out.finish()?)
}

// The input was parsed on a caller-input meter; Registry::register explicitly
// supports these owned CABs. Traverse the same files dict as Environment::objects,
// registering all resources before a Mesh asks for them. No second parse/load.
fn register_input(
    file: &File,
    name: &RawStr,
    env: &Environment,
    meter: &Arc<Meter>,
    objects: &mut MeteredVec<ObjectRef>,
) -> Result<(), MeshError> {
    match file {
        File::Serialized(sf) => {
            env.registry()
                .register(name, Cab::Serialized(sf.clone()), meter)?;
            meter.count_decode(u64_len(sf.object_infos().len()))?;
            for object in sf.objects() {
                if object.class_id() == 43 {
                    objects.push(object)?;
                }
            }
        }
        File::Raw(raw) => {
            env.registry()
                .register(name, Cab::Raw(RawCab::File(raw.clone())), meter)?;
        }
        File::Bundle(_) | File::Web(_) => {
            meter.count_decode(u64_len(file.container_files().len()))?;
            for slot in file.container_files() {
                let child = file.container_entry(slot.dir_index)?;
                register_input(&child, &slot.name, env, meter, objects)?;
            }
        }
    }
    Ok(())
}

/// Load the exact bytes the run hashed for its corpus pin. Embedded bundle
/// resources resolve through the environment; rows sort by raw file name/path id.
pub(crate) fn dump_on(
    rel: &[u8],
    data: Bytes,
    path: &Path,
    opts: &DumpOptions,
    sha256: [u8; 32],
    bytes_in: u64,
    meter: &Arc<Meter>,
) -> Result<InputDump, MeshError> {
    let mut options = EnvOptions::unitypy();
    options.context = opts.context.clone();
    options.resolver = Some(Arc::new(crate::mesh_resource::Directory::new(
        path,
        meter,
        &opts.context,
    )?));
    let env = Environment::new(options);
    // Parse on the same meter as the original read/hash and output. An
    // Environment load would silently create another input allowance. Use
    // memory-reader semantics for these exact bytes, like canonical dump.
    let kind =
        check_entry_type(&data, &opts.context.limits, meter).map_err(unityrs_core::Error::from)?;
    if kind == FileType::Zip {
        return Err(MeshError::Field("ZIP corpus input unsupported"));
    }
    let name = RawStr::charged(rel, meter)?;
    let file = parse_file(name.clone(), data, kind, None, meter, &opts.context)?;
    crate::input::load_deep(&file)?;
    let mut objects = MeteredVec::new(meter);
    register_input(&file, &name, &env, meter, &mut objects)?;
    meter.count_decode(u64_len(objects.len()))?;
    let max_name = objects
        .as_slice()
        .iter()
        .map(|o| o.file().name().as_bytes().len())
        .max()
        .unwrap_or(0);
    let comparisons = u64_len(objects.len()).saturating_mul(u64::from(objects.len().bit_width()));
    meter.count_decode(comparisons.saturating_mul(u64_len(max_name).saturating_add(8)))?;
    objects.as_mut_slice().sort_unstable_by(|a, b| {
        basename(a.file().name().as_bytes())
            .cmp(basename(b.file().name().as_bytes()))
            .then(a.path_id().cmp(&b.path_id()))
    });
    let mut lines = MeteredVec::new(meter);
    let mut charge = Charge::zero(meter);
    for object in objects.as_slice() {
        let (line, paid) = record(rel, object, &env, meter)?;
        lines.push(line)?;
        charge.absorb(paid);
    }
    let count = u64_len(lines.len());
    let (lines, paid) = lines.into_parts();
    charge.absorb(paid);
    Ok(InputDump {
        lines,
        charge: Some(charge),
        files: 0,
        objects: count,
        sha256: Some(sha256),
        bytes_in,
        ..InputDump::default()
    })
}
