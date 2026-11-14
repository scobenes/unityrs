// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshExporter.py. See THIRD_PARTY_LICENSES.
//! Raw OBJ text (PARITY section 15.4), streamed without normalisation.
//! On failure the caller discards any partial output. `False` writes nothing.
mod material;
mod renderer;
mod sink;
mod texture;
mod view;
use crate::{
    Mesh, MeshError,
    pyfmt::{self, PyFmtBuf, PyNum},
};
pub use material::{MaterialView, export_material};
pub use renderer::{RendererExport, export_mesh_renderer};
pub use sink::{ChargedBytes, MeteredWrite, ObjSink, Writer};
use std::sync::Arc;
use unityrs_core::{RawStr, limits::Meter};
use unityrs_meshdecode::VertexArray;

/// Whether UnityPy returns text or False.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    Text,
    False,
}
const _: () = crate::send_sync::<Written>();

fn attribute(
    a: &VertexArray,
    prefix: &[u8],
    width: usize,
    negate: bool,
    out: &mut dyn MeteredWrite,
) -> Result<(), MeshError> {
    out.meter().count_decode(a.len() as u64)?;
    let mut buf = PyFmtBuf::default();
    for i in 0..a.len() {
        out.write_bytes(prefix)?;
        for j in 0..width {
            if j > 0 {
                out.write_bytes(b" ")?;
            }
            let n = a
                .component(i, j)
                .ok_or(MeshError::Field("attribute tuple"))?;
            let n = if negate && j == 0 {
                // Python -int(0) is still zero, whereas -float(0) is -0.
                PyNum::Float(match n {
                    PyNum::Int(0) => 0.,
                    _ => -n.as_f64(),
                })
            } else {
                n
            };
            out.write_bytes(pyfmt::g9(n, &mut buf).as_bytes())?;
        }
        out.write_bytes(b"\n")?;
    }
    Ok(())
}

/// UnityPy's OBJ text, retaining its face syntax even without UVs/normals.
pub fn export_mesh_obj(
    m: &Mesh,
    material_names: Option<&[Option<&RawStr>]>,
    out: &mut dyn MeteredWrite,
) -> Result<Written, MeshError> {
    let Some(positions) = m.processed.vertices().filter(|v| !v.is_empty()) else {
        return Ok(Written::False);
    };
    if m.processed.vertex_count() == 0 {
        return Ok(Written::False);
    }
    let names = material_names.unwrap_or_default();
    out.write_bytes(b"g ")?;
    out.write_bytes(m.name.as_bytes())?;
    out.write_bytes(b"\n")?;
    if !names.is_empty() {
        out.write_bytes(b"mtllib ")?;
        out.write_bytes(m.name.as_bytes())?;
        out.write_bytes(b".mtl\n")?;
    }
    attribute(positions, b"v ", 3, true, out)?;
    if let Some(a) = m.processed.uv(0) {
        attribute(a, b"vt ", 2, false, out)?;
    }
    if let Some(a) = m.processed.normals() {
        attribute(a, b"vn ", 3, true, out)?;
    }
    out.meter()
        .count_decode(m.triangles.submeshes().len() as u64)?;
    for (i, sub) in m.triangles.submeshes().iter().enumerate() {
        out.write_bytes(b"g ")?;
        out.write_bytes(m.name.as_bytes())?;
        out.write_args(format_args!("_{i}\n"))?;
        if let Some(Some(name)) = names.get(i).filter(|n| n.is_some_and(|n| !n.is_empty())) {
            out.write_bytes(b"usemtl ")?;
            out.write_bytes(name.as_bytes())?;
            out.write_bytes(b"\n")?;
        }
        out.meter().count_decode(sub.len())?;
        for &[a, b, c] in sub.triangles() {
            let (a, b, c) = (u64::from(a) + 1, u64::from(b) + 1, u64::from(c) + 1);
            out.write_args(format_args!("f {c}/{c}/{c} {b}/{b}/{b} {a}/{a}/{a}\n"))?;
        }
        if !sub.cut().is_empty() || sub.empty() > 0 {
            return Err(MeshError::Field("triangle tuple"));
        }
    }
    Ok(Written::Text)
}
/// Convenience output retaining its meter charge until dropped.
pub fn export_mesh_obj_owned(
    m: &Mesh,
    names: Option<&[Option<&RawStr>]>,
    meter: &Arc<Meter>,
) -> Result<Option<ChargedBytes>, MeshError> {
    let mut out = ChargedBytes::new(meter);
    Ok(match export_mesh_obj(m, names, &mut out)? {
        Written::False => None,
        Written::Text => Some(out),
    })
}
