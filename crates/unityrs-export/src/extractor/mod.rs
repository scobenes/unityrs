// Ported from UnityPy 1.25.2 (MIT): tools/extractor.py. See THIRD_PARTY_LICENSES.
//! L9 extractor: tools/extractor.py:25-157, tools/extractor.py:165-319.
//! Container entries are never deduplicated. Stable priority is implemented
//! by sorting (priority, original ordinal), without allocating sort scratch.
//! Tier C failures are retained in the report; failure to allocate the
//! report itself ends the run. The caller supplies a confined filesystem
//! sink; all names handed to it have passed [`paths::validate`].
mod crawl;
mod images;
pub mod paths;
mod resource;
use crate::{audio, shader, simple};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, Parsed, RawStr, Value,
    env::Environment,
    limits::{ChargeError, Meter, MeteredVec},
};
use unityrs_mesh::obj::{self, MeteredWrite};

/// Export dispatch order, including UnityPy's observable insertion order.
pub const EXPORT_TYPES: [i32; 9] = [1, 213, 83, 128, 43, 114, 48, 49, 28];
/// Called first with metadata, then with the parsed object, as UnityPy does.
/// A missing MonoBehaviour tree is passed as None on the second call too.
pub trait AssetFilter: Send + Sync {
    fn accepts(&self, object: &ObjectRef, parsed: Option<&Parsed>) -> bool;
}
/// Options of `extract_assets`; loading is the caller's Environment policy.
#[derive(Clone, Copy)]
pub struct Options<'a> {
    pub use_container: bool,
    pub ignore_first_container_dirs: i64,
    pub append_path_id: bool,
    pub export_unknown_as_typetree: bool,
    pub asset_filter: Option<&'a dyn AssetFilter>,
}
impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            use_container: true,
            ignore_first_container_dirs: 0,
            append_path_id: false,
            export_unknown_as_typetree: false,
            asset_filter: None,
        }
    }
}
/// A sink must confine paths beneath its root and charge buffered output
/// before fallible allocation. A successful call has finished writing.
pub trait Sink {
    fn directory(&mut self, relative: &[u8]) -> Result<(), Error>;
    fn file(&mut self, relative: &[u8], bytes: &[u8]) -> Result<(), Error>;
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsafe extractor output path")]
    UnsafePath,
    #[error("invalid extractor field {0}")]
    Field(&'static str),
    #[error(transparent)]
    Core(#[from] unityrs_core::Error),
    #[error(transparent)]
    Deref(#[from] unityrs_core::env::DerefError),
    #[error(transparent)]
    Charge(#[from] ChargeError),
    #[error(transparent)]
    Simple(#[from] simple::Error),
    #[error(transparent)]
    Audio(#[from] audio::Error),
    #[error(transparent)]
    Shader(#[from] shader::Error),
    #[error(transparent)]
    Mesh(#[from] unityrs_mesh::MeshError),
    #[error(transparent)]
    MeshDecode(#[from] unityrs_meshdecode::MeshError),
    #[error(transparent)]
    Decode(#[from] unityrs_texture::DecodeError),
    #[error(transparent)]
    Sprite(#[from] unityrs_texture::sprite::SpriteError),
    #[error(transparent)]
    Fetch(#[from] unityrs_texture::fetch::Error),
    #[error("image writer: {0:?}")]
    Image(unityrs_imageio::writers::SaveError<ChargeError>),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
impl Error {
    /// Whether this terminal failure refused memory, allocation or bounded
    /// work, including a lazy input read nested inside an exporter error.
    #[must_use]
    pub fn resource_refusal(&self) -> bool {
        resource::refused(self)
    }
}
/// A completed write, including writes subsequently overwritten. Paths are relative.
#[derive(Debug)]
pub struct Written {
    pub object: ObjectRef,
    pub path: RawStr,
    pub size: u64,
}
/// An export failure with its original typed cause and requested relative path.
#[derive(Debug)]
pub struct Failure {
    pub object: ObjectRef,
    pub path: RawStr,
    pub error: Error,
}
/// Exported identities preserve UnityPy's duplicates (a Sprite returns its texture).
#[derive(Clone, Debug)]
pub struct Identity {
    pub file: Arc<unityrs_core::files::SerializedFile>,
    pub path_id: i64,
}
impl From<&ObjectRef> for Identity {
    fn from(object: &ObjectRef) -> Self {
        Self {
            file: object.file().clone(),
            path_id: object.path_id(),
        }
    }
}
#[derive(Debug)]
pub struct Report {
    pub written: MeteredVec<Written>,
    pub failures: MeteredVec<Failure>,
    pub exported: MeteredVec<Identity>,
}
impl Report {
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            written: MeteredVec::new(meter),
            failures: MeteredVec::new(meter),
            exported: MeteredVec::new(meter),
        }
    }
}
fn same(a: &ObjectRef, b: &ObjectRef) -> bool {
    Arc::ptr_eq(a.file(), b.file()) && a.path_id() == b.path_id()
}
fn contains(objects: &[ObjectRef], obj: &ObjectRef, meter: &Meter) -> Result<bool, Error> {
    meter.count_decode(objects.len() as u64)?;
    Ok(objects.iter().any(|a| same(a, obj)))
}
fn was_exported(objects: &[Identity], obj: &ObjectRef, meter: &Meter) -> Result<bool, Error> {
    meter.count_decode(objects.len() as u64)?;
    Ok(objects
        .iter()
        .any(|a| Arc::ptr_eq(&a.file, obj.file()) && a.path_id == obj.path_id()))
}
fn priority(id: i32) -> usize {
    EXPORT_TYPES.iter().position(|i| *i == id).unwrap_or(999)
}
struct Entry {
    object: ObjectRef,
    path: RawStr,
    ordinal: usize,
}
/// Extract a loaded Environment. Empty container mode stays empty: there is
/// no implicit fallback to all objects. Later entries overwrite earlier ones.
pub fn extract_assets(
    env: &Environment,
    options: &Options<'_>,
    sink: &mut dyn Sink,
    meter: &Arc<Meter>,
) -> Result<Report, Error> {
    let mut report = Report::new(meter);
    sink.directory(b"")?;
    let mut entries = MeteredVec::new(meter);
    if options.use_container {
        for item in env.container()? {
            let item = item?;
            let Some(ptr) = item.row.asset() else {
                continue;
            };
            let object = env.deref(&item.file, ptr)?;
            let path = item.row.path.unwrap_or_default();
            entries.push(Entry {
                object,
                path,
                ordinal: entries.len(),
            })?;
        }
    } else {
        for item in env.objects() {
            entries.push(Entry {
                object: item?,
                path: RawStr::default(),
                ordinal: entries.len(),
            })?;
        }
    }
    meter.count_decode(
        (entries.len() as u64).saturating_mul(1 + u64::from(entries.len().bit_width())),
    )?;
    entries
        .as_mut_slice()
        .sort_unstable_by_key(|e| (priority(e.object.class_id()), e.ordinal));
    for entry in entries.as_slice() {
        let obj = &entry.object;
        if options.asset_filter.is_some_and(|f| !f.accepts(obj, None)) {
            continue;
        }
        if !options.use_container && was_exported(report.exported.as_slice(), obj, meter)? {
            continue;
        }
        let path = if options.use_container {
            paths::container(
                entry.path.as_bytes(),
                options.ignore_first_container_dirs,
                meter,
            )
        } else {
            Ok(RawStr::default())
        };
        match path {
            Ok(path) => export_obj(
                env,
                obj,
                path.as_bytes(),
                !options.use_container,
                options,
                sink,
                meter,
                &mut report,
            )?,
            Err(error) => report.failures.push(Failure {
                object: obj.clone(),
                path: entry.path.clone(),
                error,
            })?,
        }
    }
    Ok(report)
}
/// Export one object, appending its name when requested. Recoverable failures
/// are appended to `report`; report allocation failures return Err.
#[allow(clippy::too_many_arguments)]
pub fn export_obj(
    env: &Environment,
    obj: &ObjectRef,
    path: &[u8],
    append_name: bool,
    options: &Options<'_>,
    sink: &mut dyn Sink,
    meter: &Arc<Meter>,
    report: &mut Report,
) -> Result<(), Error> {
    if let Err(error) = one(
        env,
        obj,
        path,
        append_name,
        options,
        sink,
        meter,
        report,
        false,
    ) {
        report.failures.push(Failure {
            object: obj.clone(),
            path: RawStr::charged(path, meter)?,
            error,
        })?;
    }
    Ok(())
}
fn name<'a>(obj: &ObjectRef, parsed: Option<&'a Parsed>) -> Result<&'a [u8], Error> {
    match parsed.and_then(|p| p.value().as_dict().and_then(|m| p.attr(m, b"m_Name"))) {
        Some(Value::Str(s)) => Ok(s.as_bytes()),
        Some(_) => Err(Error::Field("m_Name")),
        None => Ok(unityrs_core::classid::class_name(obj.class_id())
            .unwrap_or("UnknownType")
            .as_bytes()),
    }
}
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn one(
    env: &Environment,
    obj: &ObjectRef,
    path: &[u8],
    append_name: bool,
    options: &Options<'_>,
    sink: &mut dyn Sink,
    meter: &Arc<Meter>,
    report: &mut Report,
    crawl_id: bool,
) -> Result<(), Error> {
    if !EXPORT_TYPES.contains(&obj.class_id()) && !options.export_unknown_as_typetree {
        return Ok(());
    }
    paths::validate(path)?;
    let parsed = match simple::read(obj, env) {
        Ok(p) => Some(p),
        Err(e)
            if (obj.class_id() == 114 || options.export_unknown_as_typetree)
                && simple::read_failed(&e) =>
        {
            None
        }
        Err(e) => return Err(e.into()),
    };
    if options
        .asset_filter
        .is_some_and(|f| !f.accepts(obj, parsed.as_ref()))
    {
        return Ok(());
    }
    let path = if append_name {
        paths::join(path, name(obj, parsed.as_ref())?, meter)?
    } else {
        RawStr::charged(path, meter)?
    };
    let (stem, extension) = paths::split(path.as_bytes());
    let mut numbered = obj::ChargedBytes::new(meter);
    let stem = if options.append_path_id {
        numbered.write_bytes(stem)?;
        if crawl_id {
            numbered.write_args(format_args!("_{}", obj.path_id()))?;
        } else {
            numbered.write_bytes(b"_None")?;
        }
        numbered.as_bytes()
    } else {
        stem
    };
    match obj.class_id() {
        1 => {
            crawl::export(env, obj, stem, sink, meter, report)?;
            return Ok(());
        }
        28 | 213 => {
            let (image, texture) = images::image(
                env,
                obj,
                parsed.as_ref().ok_or(Error::Field("image tree"))?,
                meter,
            )?;
            images::save(
                obj,
                parsed.as_ref().ok_or(Error::Field("image tree"))?,
                &image,
                stem,
                extension,
                sink,
                meter,
                report,
            )?;
            report.exported.push(obj.into())?;
            if let Some(texture) = texture {
                report.exported.push(texture)?;
            }
            return Ok(());
        }
        83 => {
            let samples = audio::export(obj, env, true, meter)?;
            if samples.len() == 1 {
                if let Some(sample) = samples.as_slice().first() {
                    write(
                        obj,
                        stem,
                        b".wav",
                        sample.bytes.as_slice(),
                        sink,
                        meter,
                        report,
                    )?;
                }
            } else if !samples.is_empty() {
                sink.directory(stem)?;
                for sample in samples.as_slice() {
                    let name = paths::join(stem, sample.name.as_slice(), meter)?;
                    write(
                        obj,
                        name.as_bytes(),
                        b".wav",
                        sample.bytes.as_slice(),
                        sink,
                        meter,
                        report,
                    )?;
                }
            }
        }
        43 => {
            let mesh = unityrs_mesh::read_mesh_for_obj(obj, env, meter)?;
            if let Some(bytes) = obj::export_mesh_obj_owned(&mesh, None, meter)? {
                write(
                    obj,
                    stem,
                    default_ext(extension, b".obj"),
                    bytes.as_bytes(),
                    sink,
                    meter,
                    report,
                )?;
            }
        }
        48 => {
            let bytes = shader::export(obj, env, meter)?;
            write(
                obj,
                stem,
                default_ext(extension, b".txt"),
                bytes.as_slice(),
                sink,
                meter,
                report,
            )?;
        }
        49 | 128 | 114 => {
            if let Some(payload) = simple::export(obj, env, "", meter)? {
                let suffix = if obj.class_id() == 49 {
                    default_ext(extension, b".txt")
                } else {
                    payload.extension.as_bytes()
                };
                write(
                    obj,
                    stem,
                    suffix,
                    payload.bytes.as_slice(),
                    sink,
                    meter,
                    report,
                )?;
            }
        }
        _ => {
            if let Some(parsed) = parsed.as_ref() {
                match simple::json(parsed, meter) {
                    Ok(bytes) => write(obj, stem, b".json", bytes.as_slice(), sink, meter, report)?,
                    Err(simple::Error::Json(crate::pyjson::WriteError::Unwritable(_))) => {
                        let bytes = obj.raw_data()?;
                        write(obj, stem, b".bin", &bytes, sink, meter, report)?;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                let bytes = obj.raw_data()?;
                write(obj, stem, b".bin", &bytes, sink, meter, report)?;
            }
        }
    }
    report.exported.push(obj.into())?;
    Ok(())
}
fn default_ext<'a>(extension: &'a [u8], default: &'a [u8]) -> &'a [u8] {
    if extension.is_empty() {
        default
    } else {
        extension
    }
}
#[allow(clippy::too_many_arguments)]
fn write(
    obj: &ObjectRef,
    stem: &[u8],
    ext: &[u8],
    bytes: &[u8],
    sink: &mut dyn Sink,
    meter: &Arc<Meter>,
    report: &mut Report,
) -> Result<(), Error> {
    let path = paths::suffix(stem, ext, meter)?;
    // Reserve the report before publishing, so every completed write is reportable.
    report.written.reserve(1)?;
    meter.count_decode(bytes.len() as u64)?;
    sink.file(path.as_bytes(), bytes)?;
    report.written.push(Written {
        object: obj.clone(),
        path,
        size: bytes.len() as u64,
    })?;
    Ok(())
}
