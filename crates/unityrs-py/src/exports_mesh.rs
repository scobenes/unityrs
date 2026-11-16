// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshExporter.py, UnityPy/export/MeshRendererExporter.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
//! OBJ formatting over the existing geometry decoder and metered writer.
use super::{NativeEnvironment, NativeObject, context, named_error};
use crate::{errors, python as p};
use pyo3::{
    exceptions::{PyIndexError, PyNotImplementedError, PyTypeError, PyValueError},
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
    py.detach(move || {
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
                .map_err(decode_error)?;
            if processed.vertex_count() == 0
                || processed.vertices().is_none_or(VertexArray::is_empty)
            {
                return p::boolean(false);
            }
            let mut out = ChargedBytes::new(&meter);
            let root = parsed
                .value()
                .as_dict()
                .ok_or_else(|| PyTypeError::new_err("Mesh fields"))?;
            let Some(Value::Str(name)) = parsed.attr(root, b"m_Name") else {
                return Err(PyTypeError::new_err("Mesh.m_Name must be str"));
            };
            out.write_bytes(b"g ").map_err(mesh_error)?;
            out.write_bytes(name.as_bytes()).map_err(mesh_error)?;
            out.write_bytes(b"\n").map_err(mesh_error)?;
            if !names.is_empty() {
                out.write_bytes(b"mtllib ").map_err(mesh_error)?;
                out.write_bytes(name.as_bytes()).map_err(mesh_error)?;
                out.write_bytes(b".mtl\n").map_err(mesh_error)?;
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
            let triangles = processed.get_triangles(&meter).map_err(decode_error)?;
            meter
                .count_decode(triangles.submeshes().len() as u64)
                .map_err(errors::charge)?;
            for (i, sub) in triangles.submeshes().iter().enumerate() {
                out.write_bytes(b"g ").map_err(mesh_error)?;
                out.write_bytes(name.as_bytes()).map_err(mesh_error)?;
                out.write_args(format_args!("_{i}\n")).map_err(mesh_error)?;
                if let Some(name) = names.as_slice().get(i).filter(|name| !name.is_empty()) {
                    out.write_bytes(b"usemtl ").map_err(mesh_error)?;
                    out.write_bytes(name.as_bytes()).map_err(mesh_error)?;
                    out.write_bytes(b"\n").map_err(mesh_error)?;
                }
                meter.count_decode(sub.len()).map_err(errors::charge)?;
                for &[a, b, c] in sub.triangles() {
                    let (a, b, c) = (u64::from(a) + 1, u64::from(b) + 1, u64::from(c) + 1);
                    out.write_args(format_args!("f {c}/{c}/{c} {b}/{b}/{b} {a}/{a}/{a}\n"))
                        .map_err(mesh_error)?;
                }
                if !sub.cut().is_empty() || sub.empty() > 0 {
                    let got = sub.cut().len();
                    return Err(PyValueError::new_err(errors::message(&format_args!(
                        "not enough values to unpack (expected 3, got {got})"
                    ))));
                }
            }
            let _handoff = meter
                .charge((out.as_bytes().len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::string(out.as_bytes())
        })
    })
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
) -> PyResult<()> {
    out.meter()
        .count_decode(a.len() as u64)
        .map_err(errors::charge)?;
    let mut buf = PyFmtBuf::default();
    for i in 0..a.len() {
        out.write_bytes(prefix).map_err(mesh_error)?;
        for j in 0..width {
            if j > 0 {
                out.write_bytes(b" ").map_err(mesh_error)?;
            }
            let n = a
                .component(i, j)
                .ok_or_else(|| PyIndexError::new_err("tuple index out of range"))?;
            let n = if negate && j == 0 {
                PyNum::Float(match n {
                    PyNum::Int(0) => 0.,
                    _ => -n.as_f64(),
                })
            } else {
                n
            };
            out.write_bytes(pyfmt::g9(n, &mut buf).as_bytes())
                .map_err(mesh_error)?;
        }
        out.write_bytes(b"\n").map_err(mesh_error)?;
    }
    Ok(())
}

#[pyfunction]
pub(super) fn export_material(
    py: Python<'_>,
    input: &Bound<'_, PyAny>,
    reader: &NativeObject,
    environment: &NativeEnvironment,
) -> PyResult<Py<PyAny>> {
    let (limits, meter) = context(Some(reader));
    let snapshot = crate::value_input::material_snapshot(input.as_unbound(), &meter, &limits)?;
    py.detach(move || {
        crate::worker::run(limits.stack_needed(), || {
            let parsed: Parsed = snapshot.build()?;
            let material = unityrs_mesh::obj::MaterialView::new(
                &parsed,
                reader.inner.file(),
                &environment.inner,
                &meter,
            )
            .map_err(mesh_error)?;
            let mut out = ChargedBytes::new(&meter);
            unityrs_mesh::obj::export_material(&material, &mut out).map_err(mesh_error)?;
            let _handoff = meter
                .charge((out.as_bytes().len() as u64).saturating_mul(4))
                .map_err(errors::charge)?;
            p::string(out.as_bytes())
        })
    })
}

fn decode_error(error: unityrs_meshdecode::MeshError) -> PyErr {
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

fn mesh_error(error: MeshError) -> PyErr {
    match error {
        MeshError::Decode(error) => decode_error(error),
        MeshError::Charge(error) => errors::charge(error),
        MeshError::Core(error) => errors::core(error),
        _ => PyValueError::new_err(errors::message(&error)),
    }
}
