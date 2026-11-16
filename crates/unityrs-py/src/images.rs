// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py and export/SpriteHelper.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Image storage handoff for `export/Texture2DConverter.py:237-320` and
//! `export/SpriteHelper.py:62-127`. Python owns Pillow objects; decoding,
//! encoding and sprite raster work use the existing metered Rust ports.
use crate::{errors, object::NativeObject, python as p};
use pyo3::{
    exceptions::{PyMemoryError, PyNotImplementedError, PyTypeError, PyValueError},
    prelude::*,
};
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Limits, Meter, MeteredVec},
};
use unityrs_texture::{DecodeError, Format, Image, Mode, TextureFields};

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(_image_decode, m)?)?;
    m.add_function(wrap_pyfunction!(_image_encode, m)?)?;
    m.add_function(wrap_pyfunction!(_sprite_render, m)?)?;
    m.add_function(wrap_pyfunction!(_image_resource, m)?)?;
    m.add_function(wrap_pyfunction!(_array_format, m)?)?;
    Ok(())
}

#[pyfunction]
fn _array_format(py: Python<'_>, graphics_format: i32) -> PyResult<i32> {
    py.detach(|| {
        unityrs_texture::array::texture_format(graphics_format)
            .map_err(|e| PyNotImplementedError::new_err(errors::message(&e)))
    })
}

#[pyfunction]
#[pyo3(signature = (env, file, path, offset, size, guard=None))]
#[allow(
    clippy::needless_pass_by_value,
    reason = "PyO3 retains the owned Python buffer through detached work"
)]
fn _image_resource(
    py: Python<'_>,
    env: &crate::environment::NativeEnvironment,
    file: &crate::files::NativeFile,
    path: &Bound<'_, PyAny>,
    offset: i128,
    size: i128,
    guard: Option<Py<crate::traversal::TreeTraversal>>,
) -> PyResult<Py<PyAny>> {
    let unityrs_core::File::Serialized(file) = &file.inner else {
        return Err(PyTypeError::new_err(
            "resource owner is not a SerializedFile",
        ));
    };
    let path = crate::buffer_snapshot::BufferSnapshot::new(path, file.meter())?;
    py.detach(|| {
        crate::worker::run(crate::worker::load_stack(&file.context().limits), || {
            let stream = unityrs_texture::fetch::Stream {
                path: unityrs_core::RawStr::charged(&path, file.meter()).map_err(errors::charge)?,
                offset,
                size,
            };
            let (offset, size, slice) = match stream.span() {
                unityrs_texture::fetch::Span::Range(offset, size) => (offset, size, None),
                unityrs_texture::fetch::Span::Slice(start, stop) => {
                    (0, u64::MAX, Some((start, stop)))
                }
            };
            let bytes = env
                .inner
                .resource(file, &stream.path, offset, size)
                .map_err(|error| errors::resource(error, &stream.path))?;
            let data = if let Some((start, stop)) = slice {
                bytes
                    .get(unityrs_texture::fetch::python_slice(
                        bytes.len(),
                        start,
                        stop,
                    ))
                    .unwrap_or_default()
            } else {
                &bytes
            };
            let cost = unityrs_core::limits::heap_cost(data.len() as u64);
            // A facade decode retains the Python resource bytes beyond this
            // fetch, alongside blob and pixel staging. Its traversal owns
            // that reservation until the entire image operation returns.
            let _copy = if let Some(guard) = &guard {
                crate::traversal::TreeTraversal::shared_step(guard, cost, 0)?;
                None
            } else {
                Some(file.meter().charge(cost).map_err(errors::charge)?)
            };
            p::bytes(data)
        })
    })
}

fn meter(reader: Option<&NativeObject>) -> Arc<Meter> {
    reader.map_or_else(
        || Meter::for_input(&Limits::default(), Arc::new(Unbounded), Reservation::Input),
        |r| r.inner.file().meter().clone(),
    )
}

fn exception(class: &str, message: String) -> PyErr {
    crate::python::relay::attach(|py| {
        let cls = if matches!(class, "DecompressionBombError" | "UFuncTypeError") {
            let module = if class == "DecompressionBombError" {
                "PIL.Image"
            } else {
                "numpy.core._exceptions"
            };
            py.import(module).and_then(|m| m.getattr(class))
        } else {
            py.import("builtins").and_then(|m| m.getattr(class))
        };
        match cls.and_then(|cls| cls.cast_into::<pyo3::types::PyType>().map_err(Into::into)) {
            Ok(cls) => PyErr::from_type(cls, (message,)),
            Err(error) => error,
        }
    })
}

fn decode_error(error: DecodeError, format: i32) -> PyErr {
    match error {
        DecodeError::Charge(e) => errors::charge(e),
        DecodeError::NoImageData => PyValueError::new_err("Texture2D has no image data"),
        DecodeError::NegativeSize { .. } => PyValueError::new_err("Width and height must be >= 0"),
        DecodeError::NotEnoughData { needed, got } if matches!(format, 48..=59 | 66..=71) => {
            PyValueError::new_err(errors::message(&format_args!(
                "Invalid ASTC data size: {got} < {needed}"
            )))
        }
        DecodeError::NotEnoughData { .. } => PyValueError::new_err("not enough image data"),
        DecodeError::PillowMemoryError { .. } => PyMemoryError::new_err(()),
        e @ DecodeError::DecompressionBomb { .. } => {
            exception("DecompressionBombError", errors::message(&e))
        }
        e @ (DecodeError::TooLarge { .. } | DecodeError::Panicked { .. }) => {
            errors::LimitError::new_err(errors::message(&e))
        }
        e => PyNotImplementedError::new_err(errors::message(&e)),
    }
}

fn write_error(error: unityrs_texture::write::WriteError) -> PyErr {
    use unityrs_texture::write::WriteError;
    match error {
        WriteError::Charge(e) => errors::charge(e),
        WriteError::Decode(e) => decode_error(e, 4),
        e => PyNotImplementedError::new_err(errors::message(&e)),
    }
}

fn sprite_error(error: unityrs_texture::sprite::SpriteError) -> PyErr {
    if let unityrs_texture::sprite::SpriteError::Charge(e) = error {
        return errors::charge(e);
    }
    if let Some((class, message)) = error.python() {
        return exception(class, message);
    }
    PyNotImplementedError::new_err(errors::message(&error))
}

fn output(image: &Image, meter: &Arc<Meter>) -> PyResult<Py<PyAny>> {
    let _charge = meter
        .charge((image.pixels().len() as u64).saturating_add(1024))
        .map_err(errors::charge)?;
    let out = p::list()?;
    p::push(&out, &p::string(image.mode().name().as_bytes())?)?;
    p::push(&out, &p::uint(u64::from(image.width()))?)?;
    p::push(&out, &p::uint(u64::from(image.height()))?)?;
    p::push(&out, &p::bytes(image.pixels())?)?;
    Ok(out)
}

#[pyfunction]
#[pyo3(signature = (data, width, height, texture_format, version, platform, blob, reader=None))]
#[allow(
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    reason = "PyO3 retains image buffers through detached work"
)]
fn _image_decode(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    width: i32,
    height: i32,
    texture_format: i32,
    version: (u16, u16),
    platform: i32,
    blob: Option<&Bound<'_, PyAny>>,
    reader: Option<&NativeObject>,
) -> PyResult<Py<PyAny>> {
    let meter = meter(reader);
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let blob = blob
        .map(|v| crate::buffer_snapshot::BufferSnapshot::new(v, &meter))
        .transpose()?;
    let limits = reader.map_or_else(Limits::default, |r| r.inner.file().context().limits);
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            let fields = TextureFields {
                texture_format,
                width,
                height,
                platform,
                platform_blob: blob.as_deref(),
            };
            let mut format = Format::from_texture_format(texture_format).unwrap_or(Format::Rgba32);
            if version >= (2017, 3) {
                format = match format {
                    Format::Dxt1Crunched => Format::UnityDxt1Crunched,
                    Format::Dxt5Crunched => Format::UnityDxt5Crunched,
                    other => other,
                };
            }
            let image = unityrs_texture::decode_in_as(format, &fields, &data, &meter)
                .map_err(|e| decode_error(e, texture_format))?;
            output(&image, &meter)
        })
    })
}

#[pyfunction]
#[pyo3(signature = (data, width, height, pack_mode, texture_format, platform, blob, reader=None, switch_mips=false))]
#[allow(
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    reason = "PyO3 retains image buffers through detached work"
)]
fn _image_encode(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    width: u32,
    height: u32,
    pack_mode: &Bound<'_, PyAny>,
    texture_format: i32,
    platform: i32,
    blob: Option<&Bound<'_, PyAny>>,
    reader: Option<&NativeObject>,
    switch_mips: bool,
) -> PyResult<Py<PyAny>> {
    let meter = meter(reader);
    let (pack_mode, _mode_charge) = crate::text_snapshot::snapshot(pack_mode, &meter)?;
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let blob = blob
        .map(|v| crate::buffer_snapshot::BufferSnapshot::new(v, &meter))
        .transpose()?;
    let limits = reader.map_or_else(Limits::default, |r| r.inner.file().context().limits);
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            // Python resizes each mip with Pillow, so the core sees one
            // level at a time. Retain its gate for the requested mip count.
            if switch_mips {
                return Err(write_error(
                    unityrs_texture::write::WriteError::Unsupported("Switch mip chain"),
                ));
            }
            let mut alpha = MeteredVec::new(&meter);
            let (mode, data) = match pack_mode.as_str() {
                "A" => {
                    meter
                        .count_decode(data.len() as u64)
                        .map_err(errors::charge)?;
                    alpha
                        .reserve(data.len().saturating_mul(2))
                        .map_err(errors::charge)?;
                    for &byte in data.iter() {
                        alpha
                            .extend_from_slice(&[byte, byte])
                            .map_err(errors::charge)?;
                    }
                    ("LA", alpha.as_slice())
                }
                "R" => ("L", data.as_ref()),
                "RGB" => ("RGB", data.as_ref()),
                _ => ("RGBA", data.as_ref()),
            };
            let source = unityrs_texture::write::Source::new(width, height, mode, data, None)
                .map_err(write_error)?;
            let encoded = unityrs_texture::write::image_to_texture2d(
                &source,
                unityrs_texture::write::Options {
                    format: texture_format,
                    platform,
                    platform_blob: blob.as_deref(),
                    flip: false,
                    mipmap_count: 1,
                },
                &meter,
            )
            .map_err(write_error)?;
            let _charge = meter
                .charge((encoded.data.len() as u64).saturating_add(512))
                .map_err(errors::charge)?;
            let out = p::list()?;
            p::push(&out, &p::bytes(&encoded.data)?)?;
            p::push(&out, &p::int(i64::from(encoded.format))?)?;
            Ok(out)
        })
    })
}

#[pyfunction]
#[pyo3(signature = (data, mode, width, height, rect, settings, ppu, sprite, reader=None, version=(0, 0, 0, 0, 0)))]
#[allow(
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    reason = "PyO3 retains image buffers through detached work"
)]
fn _sprite_render(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    mode: &Bound<'_, PyAny>,
    width: u32,
    height: u32,
    rect: [f64; 4],
    settings: u64,
    ppu: f64,
    sprite: Py<PyAny>,
    reader: Option<&NativeObject>,
    version: (u16, u16, u16, u8, u8),
) -> PyResult<Py<PyAny>> {
    let meter = meter(reader);
    let (mode, _mode_charge) = crate::text_snapshot::snapshot(mode, &meter)?;
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let limits = reader.map_or_else(Limits::default, |r| r.inner.file().context().limits);
    let version =
        unityrs_meshdecode::Version::new(version.0, version.1, version.2, version.3, version.4);
    let snapshot = if settings & 2 == 0 {
        Some(crate::value_input::export_snapshot(
            &sprite, &meter, &limits,
        )?)
    } else {
        None
    };
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            let mode = match mode.as_str() {
                "RGB" => Mode::Rgb,
                "RGBA" => Mode::Rgba,
                _ => Mode::L,
            };
            let image = unityrs_texture::image_from_pixels(mode, width, height, &data, &meter)
                .map_err(|e| decode_error(e, 4))?;
            let sheet =
                unityrs_texture::sprite::Sheet::new(&image, None, &meter).map_err(sprite_error)?;
            let parsed = snapshot
                .map(crate::value_input::Snapshot::build)
                .transpose()?;
            let image = unityrs_texture::sprite::render(
                &sheet,
                &unityrs_texture::sprite::SpriteData {
                    texture_rect: rect,
                    settings_raw: settings,
                    pixels_to_units: ppu,
                },
                || {
                    let parsed = parsed
                        .as_ref()
                        .ok_or(unityrs_meshdecode::MeshError::Assertion(""))?;
                    unityrs_meshdecode::MeshHandler::new(
                        unityrs_meshdecode::Source::sprite(parsed),
                        version,
                    )
                    .process(None, &meter)
                },
                &meter,
            )
            .map_err(sprite_error)?;
            output(&image, &meter)
        })
    })
}
