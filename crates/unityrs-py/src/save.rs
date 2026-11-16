// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/BundleFile.py, UnityPy/files/WebFile.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Save bindings: files/File.py:92-121, files/File.py:145-148,
//! files/BundleFile.py:188-393, files/WebFile.py:58-112,
//! files/ObjectReader.py:181-184. Core writes run on a detached worker.
use crate::{errors, files::NativeFile, object::NativeObject, python as p};
use pyo3::{
    exceptions::{PyNotImplementedError, PyTypeError, PyValueError},
    prelude::*,
    pybacked::PyBackedBytes,
    types::PyTuple,
};
use std::sync::Arc;
use unityrs_core::{
    File, RawStr,
    files::{
        BundlePacker, BundleSaveOptions, SaveError, SaveLayout, SaveOptions, WebPacker,
        WebSaveOptions, WriteableCab,
    },
    limits::Meter,
};

pub(crate) fn error(error: SaveError) -> PyErr {
    match error {
        SaveError::Charge(e) => errors::charge(e),
        SaveError::Read(e) => errors::core(e),
        SaveError::CabExists => {
            PyValueError::new_err("This cab already exists and isn't an EndianBinaryWriter")
        }
        SaveError::Unsupported(_)
        | SaveError::UnsupportedCodec(_)
        | SaveError::HeadTierWritesNotImplemented => {
            PyNotImplementedError::new_err(errors::message(&error))
        }
        _ => PyValueError::new_err(errors::message(&error)),
    }
}

fn context(file: &File) -> Option<&Arc<unityrs_core::FileContext>> {
    match file {
        File::Serialized(f) => Some(f.context()),
        File::Bundle(f) => Some(f.context()),
        File::Web(f) => Some(f.context()),
        File::Raw(_) => None,
    }
}

fn packer(value: Option<&Py<PyAny>>) -> PyResult<BundlePacker> {
    Python::attach(|py| {
        let Some(value) = value else {
            return Ok(BundlePacker::None);
        };
        let value = value.bind(py);
        if !value.is_truthy()? {
            return Ok(BundlePacker::None);
        }
        if let Ok(s) = value.extract::<&str>() {
            return match s {
                "none" => Ok(BundlePacker::None),
                "original" => Ok(BundlePacker::Original),
                "lz4" => Ok(BundlePacker::Lz4),
                "lzma" => Ok(BundlePacker::Lzma),
                "lz4hc" => Ok(BundlePacker::Lz4Hc),
                _ => Err(PyNotImplementedError::new_err((
                    "UnityFS - Packer:",
                    value.clone().unbind(),
                ))),
            };
        }
        if value.is_instance_of::<PyTuple>() {
            let (data, blocks) = value.extract::<(u32, u16)>()?;
            return Ok(BundlePacker::Flags { data, blocks });
        }
        Err(PyNotImplementedError::new_err((
            "UnityFS - Packer:",
            value.clone().unbind(),
        )))
    })
}

pub(crate) fn file_save(
    py: Python<'_>,
    file: &NativeFile,
    mode: &str,
    value: Option<Py<PyAny>>,
    data_flag: u32,
    block_info_flag: u16,
    signature: &str,
) -> PyResult<Py<PyAny>> {
    py.detach(move || {
        let ctx = context(&file.inner)
            .ok_or_else(|| PyTypeError::new_err("raw readers have no save method"))?;
        crate::worker::run(crate::worker::load_stack(&ctx.limits), || {
            let layout = SaveLayout::UnityPy;
            let bytes = match &file.inner {
                File::Serialized(f) => f.save(SaveOptions { layout }),
                File::Bundle(f) => match mode {
                    "fs" => f.save_fs(data_flag, block_info_flag, SaveOptions { layout }),
                    "web_raw" => f.save_web_raw(SaveOptions { layout }),
                    _ => f.save(BundleSaveOptions {
                        layout,
                        packer: if f.header().signature.as_bytes() == b"UnityFS" {
                            packer(value.as_ref())?
                        } else {
                            BundlePacker::None
                        },
                    }),
                },
                File::Web(f) => {
                    let pack = Python::attach(|py| -> PyResult<WebPacker> {
                        let Some(value) = &value else {
                            return Ok(f.packer());
                        };
                        let value = value.bind(py);
                        if !value.is_truthy()? {
                            return Ok(f.packer());
                        }
                        Ok(match value.extract::<&str>().unwrap_or("") {
                            "gzip" => WebPacker::Gzip,
                            "brotli" => WebPacker::Brotli,
                            _ => WebPacker::None,
                        })
                    })?;
                    f.save_with_signature(
                        WebSaveOptions {
                            layout,
                            packer: pack,
                        },
                        signature,
                    )
                }
                File::Raw(_) => {
                    return Err(PyTypeError::new_err("raw readers have no save method"));
                }
            }
            .map_err(error)?;
            let _copy = file
                .inner
                .meter()
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            p::bytes(&bytes)
        })
    })
}

pub(crate) fn file_is_changed(py: Python<'_>, file: &NativeFile) -> bool {
    py.detach(|| file.inner.is_changed())
}

pub(crate) fn file_mark_changed(py: Python<'_>, file: &NativeFile) {
    py.detach(|| file.inner.mark_changed());
}

pub(crate) fn object_set_raw_data(
    py: Python<'_>,
    object: &NativeObject,
    data: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, object.inner.file().meter())?;
    py.detach(move || object.inner.set_raw_data(data.as_ref()).map_err(error))
}

pub(crate) fn object_metadata(py: Python<'_>, object: &NativeObject) -> PyResult<Py<PyAny>> {
    py.detach(|| {
        let result = p::dict()?;
        p::field(
            &result,
            "script_type_index",
            &match object.inner.serialized_type() {
                Some(t) => p::int(i64::from(t.script_type_index))?,
                None => p::none(),
            },
        )?;
        Ok(result)
    })
}

pub(crate) fn file_metadata(py: Python<'_>, file: &NativeFile) -> PyResult<Py<PyAny>> {
    py.detach(|| {
        let mut charge = unityrs_core::limits::Charge::zero(file.inner.meter());
        charge.grow(512).map_err(errors::charge)?;
        let result = p::dict()?;
        if let File::Bundle(bundle) = &file.inner {
            p::field(
                &result,
                "_uses_block_alignment",
                &p::boolean(bundle.header().block_alignment)?,
            )?;
            let flags = p::dict()?;
            for slot in bundle.files() {
                charge
                    .grow(128 + (slot.name.len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
                let value = bundle
                    .directory()
                    .get(slot.dir_index)
                    .and_then(|entry| entry.flags)
                    .unwrap_or(0);
                p::set(
                    &flags,
                    &p::string(slot.name.as_bytes())?,
                    &p::uint(u64::from(value))?,
                )?;
            }
            p::field(&result, "_entry_flags", &flags)?;
            p::field(
                &result,
                "dataflags",
                &p::uint(u64::from(bundle.header().data_flags.unwrap_or(64)))?,
            )?;
            p::field(
                &result,
                "_block_info_flags",
                &p::uint(u64::from(bundle.blocks().first().map_or(64, |b| b.flags)))?,
            )?;
            for (key, value) in [
                ("version_player", &bundle.header().version_player),
                ("version_engine", &bundle.header().version_engine),
            ] {
                charge
                    .grow((value.len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
                p::field(&result, key, &p::string(value.as_bytes())?)?;
            }
        }
        if let File::Serialized(source) = &file.inner {
            charge
                .grow(
                    source
                        .user_information()
                        .map_or(0, |v| (v.len() as u64).saturating_mul(4)),
                )
                .map_err(errors::charge)?;
            p::field(
                &result,
                "userInformation",
                &match source.user_information() {
                    Some(v) => p::string(v.as_bytes())?,
                    None => p::none(),
                },
            )?;
            p::field(&result, "unknown", &p::int(source.header().unknown)?)?;
        }
        if let File::Web(web) = &file.inner {
            charge
                .grow((web.signature().len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::field(
                &result,
                "signature",
                &p::string(web.signature().as_bytes())?,
            )?;
            p::field(
                &result,
                "packer",
                &p::string(match web.packer() {
                    WebPacker::None => b"none",
                    WebPacker::Gzip => b"gzip",
                    WebPacker::Brotli => b"brotli",
                })?,
            )?;
        }
        Ok(result)
    })
}

#[pyclass(frozen, module = "unityrs._native")]
pub(crate) struct NativeCab {
    inner: WriteableCab,
    meter: Arc<Meter>,
}

#[pymethods]
impl NativeCab {
    fn write_bytes(&self, py: Python<'_>, value: PyBackedBytes, position: u64) -> PyResult<usize> {
        py.detach(move || {
            self.inner.write_at(position, &value).map_err(error)?;
            Ok(value.len())
        })
    }
    fn data(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let bytes = self.inner.save().map_err(error)?;
            let _copy = self
                .meter
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            p::bytes(&bytes)
        })
    }
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let mut copy = self
                .meter
                .charge(512 + (self.inner.name().len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            let result = p::dict()?;
            p::field(&result, "name", &p::string(self.inner.name().as_bytes())?)?;
            p::field(&result, "flags", &p::uint(u64::from(self.inner.flags()))?)?;
            let path = self.inner.path();
            if let Some(path) = &path {
                copy.grow((path.len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
            }
            p::field(
                &result,
                "endian",
                &p::string(
                    if self.inner.endian() == unityrs_core::stream::Endian::Little {
                        b"<"
                    } else {
                        b">"
                    },
                )?,
            )?;
            p::field(
                &result,
                "path",
                &match path {
                    Some(path) => p::string(path.as_bytes())?,
                    None => p::none(),
                },
            )?;
            Ok(result)
        })
    }
}

pub(crate) fn file_get_writeable_cab(
    py: Python<'_>,
    file: &NativeFile,
    name: PyBackedBytes,
    guid: Option<[u8; 16]>,
) -> PyResult<Option<NativeCab>> {
    py.detach(move || {
        let name = RawStr::charged(&name, file.inner.meter()).map_err(errors::charge)?;
        let cab = match &file.inner {
            File::Serialized(f) => {
                if let Some(guid) = guid {
                    f.get_writeable_cab_with_guid(Some(&name), guid)
                } else {
                    f.get_writeable_cab(Some(&name))
                }
            }
            File::Bundle(f) => f.get_writeable_cab(Some(&name)).map(Some),
            File::Web(f) => f.get_writeable_cab(Some(&name)).map(Some),
            File::Raw(_) => {
                return Err(PyTypeError::new_err(
                    "raw readers cannot create CAB writers",
                ));
            }
        }
        .map_err(error)?;
        Ok(cab.map(|inner| NativeCab {
            inner,
            meter: file.inner.meter().clone(),
        }))
    })
}

// The optional WebFile mapping contains caller-owned snapshots. Every retained
// handle and every output byte is charged before construction.
pub(crate) fn save_web_entries(
    py: Python<'_>,
    file: &NativeFile,
    rows: Py<PyAny>,
    packer: Option<Py<PyAny>>,
    signature: PyBackedBytes,
) -> PyResult<Py<PyAny>> {
    py.detach(move || {
        let File::Web(web) = &file.inner else {
            return Err(PyTypeError::new_err("not a WebFile"));
        };
        crate::worker::run(crate::worker::load_stack(&web.context().limits), || {
            let meter = file.inner.meter();
            let mut entries = unityrs_core::limits::MeteredVec::new(meter);
            let count = Python::attach(|py| rows.bind(py).len())?;
            entries.reserve_exact(count).map_err(errors::charge)?;
            let mut header = signature
                .len()
                .checked_add(5)
                .ok_or_else(|| PyValueError::new_err("WebFile size"))?;
            for index in 0..count {
                let pair = Python::attach(|py| {
                    rows.bind(py)
                        .get_item(index)?
                        .extract::<(PyBackedBytes, PyBackedBytes)>()
                })?;
                header = header
                    .checked_add(12)
                    .and_then(|n| n.checked_add(pair.0.len()))
                    .ok_or_else(|| PyValueError::new_err("WebFile size"))?;
                entries.push(pair).map_err(errors::charge)?;
            }
            let mut out = buffer(meter)?;
            append(&mut out, &signature)?;
            append(&mut out, &[0])?;
            append(&mut out, &web_i32(header)?)?;
            let mut offset = header;
            for (name, data) in entries.as_slice() {
                append(&mut out, &web_i32(offset)?)?;
                append(&mut out, &web_i32(data.len())?)?;
                append(&mut out, &web_i32(name.len())?)?;
                append(&mut out, name)?;
                offset = offset
                    .checked_add(data.len())
                    .ok_or_else(|| PyValueError::new_err("WebFile size"))?;
                web_i32(offset)?;
            }
            for (_, data) in entries.as_slice() {
                append(&mut out, data)?;
            }
            let pack = Python::attach(|py| -> PyResult<WebPacker> {
                let Some(value) = &packer else {
                    return Ok(web.packer());
                };
                let value = value.bind(py);
                if !value.is_truthy()? {
                    return Ok(web.packer());
                }
                Ok(match value.extract::<&str>().unwrap_or("") {
                    "gzip" => WebPacker::Gzip,
                    "brotli" => WebPacker::Brotli,
                    _ => WebPacker::None,
                })
            })?;
            let bytes = web_compress(out, pack, meter)?
                .into_bytes()
                .map_err(errors::charge)?;
            let _copy = meter
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            p::bytes(&bytes)
        })
    })
}

fn buffer(meter: &Arc<Meter>) -> PyResult<unityrs_core::budget::DecodeBuf> {
    unityrs_core::budget::DecodeBuf::new(
        meter,
        0,
        u64::MAX,
        unityrs_core::error::LimitKind::InputMemory,
    )
    .map_err(errors::charge)
}
fn append(out: &mut unityrs_core::budget::DecodeBuf, bytes: &[u8]) -> PyResult<()> {
    out.meter()
        .count_decode(bytes.len() as u64)
        .map_err(errors::charge)?;
    out.extend_from_slice(bytes).map_err(errors::charge)
}
fn web_i32(value: usize) -> PyResult<[u8; 4]> {
    i32::try_from(value)
        .map(i32::to_le_bytes)
        .map_err(|_| PyValueError::new_err("WebFile size"))
}

// Ported from unityrs-core's WebFile stored-stream encoders. The decompressed
// representation follows files/WebFile.py:58-112; compression bytes are Tier B.
fn web_compress(
    data: unityrs_core::budget::DecodeBuf,
    packer: WebPacker,
    meter: &Arc<Meter>,
) -> PyResult<unityrs_core::budget::DecodeBuf> {
    if packer == WebPacker::None {
        return Ok(data);
    }
    let mut out = buffer(meter)?;
    if packer == WebPacker::Gzip {
        append(&mut out, &[31, 139, 8, 0, 0, 0, 0, 0, 0, 255])?;
        for (index, chunk) in data.as_slice().chunks(65535).enumerate() {
            append(&mut out, &[u8::from(index == (data.len() - 1) / 65535)])?;
            let size =
                u16::try_from(chunk.len()).map_err(|_| PyValueError::new_err("gzip block size"))?;
            append(&mut out, &size.to_le_bytes())?;
            append(&mut out, &(!size).to_le_bytes())?;
            append(&mut out, chunk)?;
        }
        meter
            .count_decode(data.len() as u64)
            .map_err(errors::charge)?;
        let mut crc = !0u32;
        for byte in data.as_slice() {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        append(&mut out, &(!crc).to_le_bytes())?;
        let size = u32::try_from(data.len()).map_err(|_| PyValueError::new_err("gzip size"))?;
        append(&mut out, &size.to_le_bytes())?;
    } else {
        let mut metadata = [0u8; 38];
        if let Some(header) = metadata.get_mut(..2) {
            header.copy_from_slice(&[0xac, 0x11]);
        }
        if let Some(marker) = metadata.get_mut(32..) {
            marker.copy_from_slice(b"brotli");
        }
        append(&mut out, &metadata)?;
        for chunk in data.as_slice().chunks(65536) {
            let len = u32::try_from(chunk.len() - 1)
                .map_err(|_| PyValueError::new_err("brotli block size"))?;
            let bits = ((len << 3) | (1 << 19)).to_le_bytes();
            if let Some(bits) = bits.get(..3) {
                append(&mut out, bits)?;
            }
            append(&mut out, chunk)?;
        }
        append(&mut out, &[3])?;
    }
    Ok(out)
}

pub(crate) fn save_bundle_entries(
    py: Python<'_>,
    file: &NativeFile,
    mode: &str,
    rows: Py<PyAny>,
    value: Option<Py<PyAny>>,
    flags: (u32, u16),
    header: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let (data_flag, block_info_flag) = flags;
    let (signature, version, player, engine, alignment, data_flags, block_flags) = header
        .extract::<(
            PyBackedBytes,
            u32,
            PyBackedBytes,
            PyBackedBytes,
            bool,
            u32,
            u16,
        )>()?;
    py.detach(move || {
        let File::Bundle(bundle) = &file.inner else {
            return Err(PyTypeError::new_err("not a bundle"));
        };
        crate::worker::run(crate::worker::load_stack(&bundle.context().limits), || {
            let meter = file.inner.meter();
            let mut entries = unityrs_core::limits::MeteredVec::new(meter);
            let count = Python::attach(|py| rows.bind(py).len())?;
            entries.reserve_exact(count).map_err(errors::charge)?;
            for index in 0..count {
                let (name, flags, data) = Python::attach(|py| {
                    rows.bind(py)
                        .get_item(index)?
                        .extract::<(PyBackedBytes, u32, PyBackedBytes)>()
                })?;
                let name = RawStr::charged(&name, meter).map_err(errors::charge)?;
                let mut bytes = buffer(meter)?;
                append(&mut bytes, &data)?;
                entries
                    .push((name, flags, bytes.into_bytes().map_err(errors::charge)?))
                    .map_err(errors::charge)?;
            }
            let engine_heap = bundle
                .header()
                .engine_version
                .as_ref()
                .and_then(|v| v.as_ref().ok())
                .map_or(0, |v| v.version.heap_bytes());
            let _header_copy = meter
                .charge(unityrs_core::limits::heap_cost(engine_heap as u64))
                .map_err(errors::charge)?;
            let mut header = bundle.header().clone();
            header.signature = RawStr::charged(&signature, meter).map_err(errors::charge)?;
            header.format = version;
            header.version_player = RawStr::charged(&player, meter).map_err(errors::charge)?;
            header.version_engine = RawStr::charged(&engine, meter).map_err(errors::charge)?;
            header.block_alignment = alignment;
            header.data_flags = Some(data_flags);
            header.layout = if signature.as_ref() == b"UnityFS" || version == 6 {
                unityrs_core::files::LayoutKind::Fs
            } else {
                unityrs_core::files::LayoutKind::WebRaw
            };
            let options = SaveOptions {
                layout: SaveLayout::UnityPy,
            };
            let bytes = if mode == "web_raw" {
                bundle.save_web_raw_with_snapshot(options, entries.as_slice(), &header)
            } else {
                bundle.save_with_snapshot(
                    BundleSaveOptions {
                        layout: options.layout,
                        packer: if signature.as_ref() == b"UnityFS" {
                            packer(value.as_ref())?
                        } else {
                            BundlePacker::None
                        },
                    },
                    entries.as_slice(),
                    &header,
                    block_flags,
                    if mode == "fs" {
                        Some((data_flag, block_info_flag))
                    } else {
                        None
                    },
                )
            }
            .map_err(error)?;
            let _copy = meter
                .charge(unityrs_core::limits::heap_cost(bytes.len() as u64))
                .map_err(errors::charge)?;
            p::bytes(&bytes)
        })
    })
}
