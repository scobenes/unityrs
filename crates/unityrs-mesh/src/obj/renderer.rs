// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshRendererExporter.py's export_mesh_renderer. See THIRD_PARTY_LICENSES.
//! Corrected oracle (PARITY section 19.1): absent optional batch fields
//! mean zero offset; submesh i binds material i-first, never shifted early.
use super::{MaterialView, MeteredWrite, ObjSink, export_material, export_mesh_obj, texture, view};
use crate::{MeshError, read_mesh_for_obj};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, RawStr,
    env::Environment,
    limits::{Meter, MeteredVec},
    objmode::{Class, Obj},
};
const RENDERER: Class = Class {
    types: &["MeshRenderer", "SkinnedMeshRenderer"],
    required: &["m_GameObject", "m_Materials"],
    optional: &["m_StaticBatchInfo", "m_SubsetIndices"],
    slotted: false,
};
const BATCH: Class = Class {
    types: &["StaticBatchInfo"],
    required: &["firstSubMesh", "subMeshCount"],
    optional: &[],
    slotted: true,
};
/// Summary of files produced; no mesh means no files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RendererExport {
    pub mesh: bool,
    pub materials: usize,
    pub textures: usize,
}
const _: () = crate::send_sync::<RendererExport>();
/// Exports one renderer through the caller's metered file sink. The caller
/// creates the destination directory, including for a renderer without mesh.
pub fn export_mesh_renderer(
    r: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
    sink: &mut dyn ObjSink,
) -> Result<RendererExport, MeshError> {
    if !matches!(r.class_id(), 23 | 137) {
        return Err(MeshError::Field("renderer class"));
    }
    let Some(object) = unityrs_scene::accessors::get_mesh(r, env, meter)? else {
        return Ok(RendererExport::default());
    };
    let m = read_mesh_for_obj(&object, env, meter)?;
    let parsed = r.parse_as_dict(false)?;
    let renderer = Obj::root(&parsed, &RENDERER, meter)?;
    let first = if let Some(batch) = renderer.class("m_StaticBatchInfo", &BATCH)?
        && batch.uint("subMeshCount", u32::MAX.into())?.unwrap_or(0) > 0
    {
        batch.uint("firstSubMesh", u32::MAX.into())?.unwrap_or(0)
    } else if let Some((_, subset)) = renderer.get("m_SubsetIndices")? {
        view::min_index(subset, meter)?.unwrap_or(0)
    } else {
        0
    };
    let slots = renderer
        .get("m_Materials")?
        .ok_or(MeshError::Field("m_Materials"))?
        .1;
    let slots = view::list(slots)?;
    let mut materials = MeteredVec::new(meter);
    let mut names = MeteredVec::with_capacity(meter, m.submeshes.len())?;
    let mut report = RendererExport {
        mesh: true,
        ..RendererExport::default()
    };
    meter.count_decode(m.submeshes.len() as u64)?;
    for i in 0..m.submeshes.len() {
        let Some(slot) = (i as u64)
            .checked_sub(first)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| slots.get(n))
        else {
            names.push(None)?;
            continue;
        };
        let ptr = view::pointer(&parsed, slot, meter)?;
        if !ptr.is_set() {
            names.push(None)?;
            continue;
        }
        let object = env.deref(r.file(), ptr)?;
        let mat = object.parse_as_dict(false)?;
        let material = MaterialView::new(&mat, object.file(), env, meter)?;
        names.push(Some(material.name.clone()))?;
        for &(key, value) in material.texture_entries(false)?.as_slice() {
            if let Some((tex, name)) = material.texture(key, value)? {
                texture::write(&tex, env, meter, &name, sink)?;
                report.textures += 1;
            }
        }
        materials.push((object, mat))?;
    }
    let mut refs: MeteredVec<Option<&RawStr>> = MeteredVec::with_capacity(meter, names.len())?;
    meter.count_decode(names.len() as u64)?;
    for n in names.as_slice() {
        refs.push(n.as_ref())?;
    }
    // UnityPy's UTF-8 file writer rejects surrogateescaped names. Raw mesh
    // export itself preserves those bytes; filesystem render export refuses.
    sink.file(m.name.as_bytes(), ".obj", &mut |out| {
        if export_mesh_obj(&m, Some(refs.as_slice()), &mut super::sink::Utf8(out))?
            == super::Written::False
        {
            return Err(MeshError::Field("vertex-less renderer"));
        }
        Ok(())
    })?;
    sink.file(m.name.as_bytes(), ".mtl", &mut |out| {
        let out = &mut super::sink::Utf8(out);
        for (i, (object, mat)) in materials.as_slice().iter().enumerate() {
            if i > 0 {
                out.write_bytes(b"\n")?;
            }
            export_material(&MaterialView::new(mat, object.file(), env, meter)?, out)?;
        }
        Ok(())
    })?;
    report.materials = materials.len();
    Ok(report)
}
