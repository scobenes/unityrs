// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshExporter.py, UnityPy/export/MeshRendererExporter.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! OBJ formatting over the existing geometry decoder and metered writer.
use super::{NativeEnvironment, NativeObject, context, named_error};
use crate::worker_error::Failure;
use crate::{errors, python as p};
use pyo3::{
    exceptions::{PyNotImplementedError, PyValueError},
    prelude::*,
};
use std::sync::Arc;
use unityrs_core::{
    Parsed, RawStr, Value,
    limits::{Meter, MeteredVec},
};
use unityrs_mesh::{
    MeshError,
    obj::{ChargedBytes, MeteredWrite},
    pyfmt::{self, PyFmtBuf, PyNum},
};
use unityrs_meshdecode::{FileResources, MeshHandler, Source, Version, VertexArray};

#[pyfunction]
#[pyo3(signature = (input, version, reader=None, environment=None, material_names=None))]
pub(super) fn export_mesh(
    py: Python<'_>,
    input: &Bound<'_, PyAny>,
    version: u64,
    reader: Option<&NativeObject>,
    environment: Option<&NativeEnvironment>,
    material_names: Option<&Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(reader);
    let snapshot = crate::value_input::export_snapshot(input.as_unbound(), &meter, &limits)?;
    let names = names(material_names, &meter)?;
    let native = py.detach(|| {
        crate::worker::run(limits.stack_needed(), || {
            let parsed = snapshot.build()?;
            let resources = reader
                .zip(environment)
                .map(|(reader, environment)| FileResources {
                    env: &environment.inner,
                    file: reader.inner.file(),
                });
            let version = Version::of(&unityrs_core::UnityVersion::from_packed(version));
            let processed = MeshHandler::new(Source::mesh(&parsed), version)
                .process(
                    resources
                        .as_ref()
                        .map(|v| v as &dyn unityrs_meshdecode::Resources),
                    &meter,
                )
                .map_err(Failure::MeshDecode)?;
            if processed.vertex_count() == 0
                || processed.vertices().is_none_or(VertexArray::is_empty)
            {
                return Ok(None);
            }
            let mut out = ChargedBytes::new(&meter);
            let root = parsed
                .value()
                .as_dict()
                .ok_or_else(|| Failure::Static("TypeError", "Mesh fields"))?;
            let Some(Value::Str(name)) = parsed.attr(root, b"m_Name") else {
                return Err(Failure::Static("TypeError", "Mesh.m_Name must be str"));
            };
            out.write_bytes(b"g ").map_err(Failure::Mesh)?;
            out.write_bytes(name.as_bytes()).map_err(Failure::Mesh)?;
            out.write_bytes(b"\n").map_err(Failure::Mesh)?;
            if !names.is_empty() {
                out.write_bytes(b"mtllib ").map_err(Failure::Mesh)?;
                out.write_bytes(name.as_bytes()).map_err(Failure::Mesh)?;
                out.write_bytes(b".mtl\n").map_err(Failure::Mesh)?;
            }
            if let Some(array) = processed.vertices() {
                attribute(array, b"v ", 3, true, &mut out)?;
            }
            if let Some(array) = processed.uv(0) {
                attribute(array, b"vt ", 2, false, &mut out)?;
            }
            if let Some(array) = processed.normals() {
                attribute(array, b"vn ", 3, true, &mut out)?;
            }
            let triangles = processed
                .get_triangles(&meter)
                .map_err(Failure::MeshDecode)?;
            meter
                .count_decode(triangles.submeshes().len() as u64)
                .map_err(Failure::charge)?;
            for (i, sub) in triangles.submeshes().iter().enumerate() {
                out.write_bytes(b"g ").map_err(Failure::Mesh)?;
                out.write_bytes(name.as_bytes()).map_err(Failure::Mesh)?;
                out.write_args(format_args!("_{i}\n"))
                    .map_err(Failure::Mesh)?;
                if let Some(name) = names.as_slice().get(i).filter(|name| !name.is_empty()) {
                    out.write_bytes(b"usemtl ").map_err(Failure::Mesh)?;
                    out.write_bytes(name.as_bytes()).map_err(Failure::Mesh)?;
                    out.write_bytes(b"\n").map_err(Failure::Mesh)?;
                }
                meter.count_decode(sub.len()).map_err(Failure::charge)?;
                for &[a, b, c] in sub.triangles() {
                    let (a, b, c) = (u64::from(a) + 1, u64::from(b) + 1, u64::from(c) + 1);
                    out.write_args(format_args!("f {c}/{c}/{c} {b}/{b}/{b} {a}/{a}/{a}\n"))
                        .map_err(Failure::Mesh)?;
                }
                if !sub.cut().is_empty() || sub.empty() > 0 {
                    let got = sub.cut().len();
                    return Err(Failure::Message(
                        "ValueError",
                        errors::message(&format_args!(
                            "not enough values to unpack (expected 3, got {got})"
                        )),
                    ));
                }
            }
            Ok::<_, Failure>(Some(out))
        })
    })?;
    let native = crate::worker_error::finish(native)?;
    let Some(native) = native else {
        return p::boolean(false);
    };
    let _handoff = meter
        .charge((native.as_bytes().len() as u64).saturating_mul(4))
        .map_err(errors::charge)?;
    p::string(native.as_bytes())
}

fn names(input: Option<&Bound<'_, PyAny>>, meter: &Arc<Meter>) -> PyResult<MeteredVec<RawStr>> {
    let mut out = MeteredVec::new(meter);
    if let Some(input) = input {
        Python::attach(|_py| -> PyResult<()> {
            let count = input.len()?;
            meter.count_decode(count as u64).map_err(errors::charge)?;
            out.reserve(count).map_err(errors::charge)?;
            for i in 0..count {
                let value =
                    crate::buffer_snapshot::BufferSnapshot::new(&input.get_item(i)?, meter)?;
                out.push(RawStr::charged(&value, meter).map_err(errors::charge)?)
                    .map_err(errors::charge)?;
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn attribute(
    a: &VertexArray,
    prefix: &[u8],
    width: usize,
    negate: bool,
    out: &mut ChargedBytes,
) -> crate::worker_error::Result<()> {
    out.meter()
        .count_decode(a.len() as u64)
        .map_err(Failure::charge)?;
    let mut buf = PyFmtBuf::default();
    for i in 0..a.len() {
        out.write_bytes(prefix).map_err(Failure::Mesh)?;
        for j in 0..width {
            if j > 0 {
                out.write_bytes(b" ").map_err(Failure::Mesh)?;
            }
            let n = a
                .component(i, j)
                .ok_or_else(|| Failure::Static("IndexError", "tuple index out of range"))?;
            let n = if negate && j == 0 {
                PyNum::Float(match n {
                    PyNum::Int(0) => 0.,
                    _ => -n.as_f64(),
                })
            } else {
                n
            };
            out.write_bytes(pyfmt::g9(n, &mut buf).as_bytes())
                .map_err(Failure::Mesh)?;
        }
        out.write_bytes(b"\n").map_err(Failure::Mesh)?;
    }
    Ok(())
}

#[pyfunction]
#[pyo3(signature = (input, reader=None, environment=None, texture_names=None))]
pub(super) fn export_material(
    py: Python<'_>,
    input: &Bound<'_, PyAny>,
    reader: Option<&NativeObject>,
    environment: Option<&NativeEnvironment>,
    texture_names: Option<&Bound<'_, PyAny>>,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(reader);
    let snapshot = crate::value_input::material_snapshot(input.as_unbound(), &meter, &limits)?;
    let texture_names = texture_names
        .map(|input| resolved_names(input, &meter))
        .transpose()?;
    let native = py.detach(|| {
        crate::worker::run(limits.stack_needed(), || {
            let parsed: Parsed = snapshot.build()?;
            let material = if let Some(names) = &texture_names {
                unityrs_mesh::obj::MaterialView::with_texture_names(
                    &parsed,
                    names.as_slice(),
                    &meter,
                )
            } else if let Some((reader, environment)) = reader.zip(environment) {
                unityrs_mesh::obj::MaterialView::new(
                    &parsed,
                    reader.inner.file(),
                    &environment.inner,
                    &meter,
                )
            } else {
                unityrs_mesh::obj::MaterialView::with_texture_names(&parsed, &[], &meter)
            }
            .map_err(Failure::Mesh)?;
            let mut out = ChargedBytes::new(&meter);
            unityrs_mesh::obj::export_material(&material, &mut out).map_err(Failure::Mesh)?;
            Ok::<_, Failure>(out)
        })
    })?;
    let native = crate::worker_error::finish(native)?;
    let _handoff = meter
        .charge((native.as_bytes().len() as u64).saturating_mul(4))
        .map_err(errors::charge)?;
    p::string(native.as_bytes())
}

fn resolved_names(
    input: &Bound<'_, PyAny>,
    meter: &Arc<Meter>,
) -> PyResult<MeteredVec<(RawStr, RawStr)>> {
    let count = input.len()?;
    meter.count_decode(count as u64).map_err(errors::charge)?;
    let mut out = MeteredVec::with_capacity(meter, count).map_err(errors::charge)?;
    for i in 0..count {
        let row = input.get_item(i)?;
        let key = crate::buffer_snapshot::BufferSnapshot::new(&row.get_item(0)?, meter)?;
        let name = crate::buffer_snapshot::BufferSnapshot::new(&row.get_item(1)?, meter)?;
        out.push((
            RawStr::charged(&key, meter).map_err(errors::charge)?,
            RawStr::charged(&name, meter).map_err(errors::charge)?,
        ))
        .map_err(errors::charge)?;
    }
    Ok(out)
}

pub(crate) fn decode_error(error: unityrs_meshdecode::MeshError) -> PyErr {
    match error {
        unityrs_meshdecode::MeshError::Charge(error) => errors::charge(error),
        unityrs_meshdecode::MeshError::Resource(error) => errors::core(error),
        _ => {
            if let Some((name, message)) = error.python() {
                named_error(name, message)
            } else {
                PyNotImplementedError::new_err(errors::message(&error))
            }
        }
    }
}

pub(crate) fn mesh_error(error: MeshError) -> PyErr {
    match error {
        MeshError::Decode(error) => decode_error(error),
        MeshError::Charge(error) => errors::charge(error),
        MeshError::Core(error) => errors::core(error),
        _ => PyValueError::new_err(errors::message(&error)),
    }
}
