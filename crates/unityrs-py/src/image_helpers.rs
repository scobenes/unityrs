// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py:254-320, export/SpriteHelper.py:130-301. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! Metered extension preprocessing and standalone sprite geometry adapters.
use crate::{errors, images, object::NativeObject, python as p};
use pyo3::{
    exceptions::{PyIndexError, PySystemError, PyTypeError, PyValueError},
    prelude::*,
};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_meshdecode::{Component, SubMeshTriangles, Triangles, VertexArray};
use unityrs_texture::{Format, Image, Mode, platform, sprite::helpers};

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_astc_codec, module)?)?;
    module.add_function(wrap_pyfunction!(_texture_preprocess, module)?)?;
    module.add_function(wrap_pyfunction!(_sprite_geometry, module)?)?;
    module.add_function(wrap_pyfunction!(_sprite_copy_triangle, module)?)?;
    Ok(())
}

#[pyfunction]
#[pyo3(signature = (data, width, height, texture_format, version, platform_id, blob, reader=None))]
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the converter extension call"
)]
fn _texture_preprocess(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    width: i32,
    height: i32,
    texture_format: i32,
    version: (u16, u16),
    platform_id: i32,
    blob: Option<&Bound<'_, PyAny>>,
    reader: Option<&NativeObject>,
) -> PyResult<Py<PyAny>> {
    let meter = images::meter(reader);
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let blob = blob
        .map(|b| crate::buffer_snapshot::BufferSnapshot::new(b, &meter))
        .transpose()?;
    py.detach(move || {
        crate::worker::run(
            unityrs_core::limits::Limits::default().stack_needed(),
            || {
                let mut storage = MeteredVec::new(&meter);
                let mut format_id = texture_format;
                let mut width = width;
                let mut height = height;
                if platform_id == 11 && [7, 10, 28, 12, 29].contains(&format_id) {
                    storage = platform::swap_bytes_for_xbox(&data, &meter)
                        .map_err(|e| images::decode_error(e, format_id))?;
                }
                if platform_id == 38
                    && let Some(blob) = &blob
                    && let Some(bytes) = blob.get(8..12)
                {
                    let mut exponent_bytes = [0; 4];
                    exponent_bytes.copy_from_slice(bytes);
                    let exponent = u32::from_le_bytes(exponent_bytes);
                    if exponent > 0 {
                        format_id = match format_id {
                            3 => 4,
                            8 => 14,
                            other => other,
                        };
                        let layout = platform::switch_layout(format_id, width, height, exponent)
                            .map_err(|e| images::decode_error(e, format_id))?;
                        storage = platform::deswizzle(&data, &layout, &meter)
                            .map_err(|e| images::decode_error(e, format_id))?;
                        let size = layout.padded_size();
                        width = i32::try_from(size.0)
                            .map_err(|_| PyValueError::new_err("image size"))?;
                        height = i32::try_from(size.1)
                            .map_err(|_| PyValueError::new_err("image size"))?;
                    }
                }
                let mut format = Format::from_texture_format(format_id);
                if version >= (2017, 3) {
                    format = format.map(|f| match f {
                        Format::Dxt1Crunched => Format::UnityDxt1Crunched,
                        Format::Dxt5Crunched => Format::UnityDxt5Crunched,
                        other => other,
                    });
                }
                if let Some(format) = format.filter(|f| f.is_crunched()) {
                    storage = platform::unpack_crunch(
                        if storage.is_empty() {
                            &data
                        } else {
                            storage.as_slice()
                        },
                        format,
                        usize::try_from(width).unwrap_or(0),
                        usize::try_from(height).unwrap_or(0),
                        &meter,
                    )
                    .map_err(|error| match error {
                        unityrs_texture::DecodeError::Crunch(_) => {
                            let name = if format.is_unity_crunched() {
                                "unpack_unity_crunch"
                            } else {
                                "unpack_crunch"
                            };
                            PySystemError::new_err(errors::message(&format_args!(
                                "<built-in function {name}> returned NULL without setting an exception"
                            )))
                        }
                        other => images::decode_error(other, format_id),
                    })?;
                }
                let bytes = if storage.is_empty() {
                    &data
                } else {
                    storage.as_slice()
                };
                let _handoff = meter
                    .charge(bytes.len() as u64 + 1024)
                    .map_err(errors::charge)?;
                let out = p::list()?;
                for value in [
                    p::bytes(bytes)?,
                    p::int(i64::from(width))?,
                    p::int(i64::from(height))?,
                    p::int(i64::from(format_id))?,
                ] {
                    p::push(&out, &value)?;
                }
                Ok(out)
            },
        )
    })?
}

fn array(input: &Bound<'_, PyAny>, meter: &Arc<Meter>) -> PyResult<VertexArray> {
    let count = input.len()?;
    meter.count_decode(count as u64).map_err(errors::charge)?;
    let dimension = if count == 0 {
        0
    } else {
        input.get_item(0)?.len()?
    };
    let size = count
        .checked_mul(dimension)
        .ok_or_else(|| PyValueError::new_err("vertex size overflow"))?;
    let mut values = MeteredVec::with_capacity(meter, size).map_err(errors::charge)?;
    meter.count_decode(size as u64).map_err(errors::charge)?;
    for i in 0..count {
        let row = input.get_item(i)?;
        if row.len()? != dimension {
            return Err(PyTypeError::new_err(
                "sprite vertex tuples must have one dimension",
            ));
        }
        for j in 0..dimension {
            let value = row.get_item(j)?;
            let component = value.extract::<i64>().map_or_else(
                |_| value.extract::<f64>().map(Component::Float),
                |v| Ok(Component::Int(v)),
            )?;
            values.push(component).map_err(errors::charge)?;
        }
    }
    Ok(VertexArray::from_components(dimension, values))
}

fn triangles(input: &Bound<'_, PyAny>, meter: &Arc<Meter>) -> PyResult<Triangles> {
    let count = input.len()?;
    meter.count_decode(count as u64).map_err(errors::charge)?;
    let mut groups = MeteredVec::with_capacity(meter, count).map_err(errors::charge)?;
    for i in 0..count {
        let group = input.get_item(i)?;
        let count = group.len()?;
        meter.count_decode(count as u64).map_err(errors::charge)?;
        let mut tris = MeteredVec::with_capacity(meter, count).map_err(errors::charge)?;
        for j in 0..count {
            let tri = group.get_item(j)?;
            let triple = tri
                .extract::<[u32; 3]>()
                .map_err(|_| PyIndexError::new_err("list index out of range"))?;
            tris.push(triple).map_err(errors::charge)?;
        }
        groups
            .push(SubMeshTriangles::from_triangles(tris))
            .map_err(errors::charge)?;
    }
    Ok(Triangles::from_submeshes(groups))
}

fn image(data: &[u8], mode: &str, width: u32, height: u32, meter: &Arc<Meter>) -> PyResult<Image> {
    let mode = match mode {
        "RGBA" => Mode::Rgba,
        "RGB" => Mode::Rgb,
        _ => Mode::L,
    };
    unityrs_texture::image_from_pixels(mode, width, height, data, meter)
        .map_err(|e| images::decode_error(e, 4))
}

#[pyfunction]
#[pyo3(signature = (data, mode, width, height, positions, uv, groups, ppu, mask, reader=None))]
#[allow(
    clippy::too_many_arguments,
    reason = "standalone geometry plus Pillow storage"
)]
fn _sprite_geometry(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    mode: &Bound<'_, PyAny>,
    width: u32,
    height: u32,
    positions: &Bound<'_, PyAny>,
    uv: &Bound<'_, PyAny>,
    groups: &Bound<'_, PyAny>,
    ppu: f64,
    mask: bool,
    reader: Option<&NativeObject>,
) -> PyResult<Py<PyAny>> {
    let meter = images::meter(reader);
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    let (mode, _mode_charge) = crate::text_snapshot::snapshot(mode, &meter)?;
    let positions = array(positions, &meter)?;
    let uv = array(uv, &meter)?;
    let groups = triangles(groups, &meter)?;
    py.detach(move || {
        crate::worker::run(
            unityrs_core::limits::Limits::default().stack_needed(),
            || {
                let image = image(&data, &mode, width, height, &meter)?;
                let result = if mask {
                    helpers::mask_image(&image, Some(&positions), groups, ppu, &meter)
                } else {
                    helpers::render_mesh(&image, Some(&positions), Some(&uv), groups, ppu, &meter)
                }
                .map_err(images::sprite_error)?;
                images::output(&result, &meter)
            },
        )
    })?
}

#[pyfunction]
#[allow(
    clippy::too_many_arguments,
    reason = "two Pillow images and their corners"
)]
fn _sprite_copy_triangle(
    py: Python<'_>,
    source: &Bound<'_, PyAny>,
    mode: &Bound<'_, PyAny>,
    width: u32,
    height: u32,
    source_triangle: [[i64; 2]; 3],
    destination: &Bound<'_, PyAny>,
    destination_mode: &Bound<'_, PyAny>,
    destination_width: u32,
    destination_height: u32,
    destination_triangle: [[f64; 2]; 3],
) -> PyResult<Py<PyAny>> {
    let meter = images::meter(None);
    let source = crate::buffer_snapshot::BufferSnapshot::new(source, &meter)?;
    let destination = crate::buffer_snapshot::BufferSnapshot::new(destination, &meter)?;
    let (mode, _mode_charge) = crate::text_snapshot::snapshot(mode, &meter)?;
    let (destination_mode, _dest_mode_charge) =
        crate::text_snapshot::snapshot(destination_mode, &meter)?;
    py.detach(move || {
        crate::worker::run(
            unityrs_core::limits::Limits::default().stack_needed(),
            || {
                let source = image(&source, &mode, width, height, &meter)?;
                let destination = image(
                    &destination,
                    &destination_mode,
                    destination_width,
                    destination_height,
                    &meter,
                )?;
                let output = helpers::copy_image_triangle(
                    &source,
                    source_triangle,
                    &destination,
                    destination_triangle,
                    &meter,
                )
                .map_err(images::sprite_error)?;
                images::output(&output, &meter)
            },
        )
    })?
}

#[pyfunction]
fn _astc_codec(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    width: u32,
    height: u32,
    block: (u8, u8),
    compress: bool,
) -> PyResult<Py<PyAny>> {
    let meter = images::meter(None);
    let data = crate::buffer_snapshot::BufferSnapshot::new(data, &meter)?;
    py.detach(move || {
        crate::worker::run(
            unityrs_core::limits::Limits::default().stack_needed(),
            || {
                if compress {
                    let result =
                        unityrs_texture::write::astc_context(&data, width, height, block, &meter)
                            .map_err(images::write_error)?;
                    let _copy = meter
                        .charge(result.len() as u64 + 256)
                        .map_err(errors::charge)?;
                    p::bytes(result.as_slice())
                } else {
                    let result = platform::astc_context_decode(&data, width, height, block, &meter)
                        .map_err(|e| images::decode_error(e, 54))?;
                    let _copy = meter
                        .charge(result.pixels().len() as u64 + 256)
                        .map_err(errors::charge)?;
                    p::bytes(result.pixels())
                }
            },
        )
    })?
}
