// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/legacy_patch/GameObject.py and UnityPy/export/MeshRendererExporter.py's get_mesh. See THIRD_PARTY_LICENSES.
//! `PARITY.md` sections 14.2, 19.2 and 15.4. Empty component lists and
//! legacy (class id, `PPtr`) pairs use the stated Tier B answers. Accessors
//! return the first matching object's handle (UnityPy returns its `PPtr`).
//! Failed dereferences propagate, including built-ins in `get_mesh`.
//!
//! UnityPy-facing fields use core's `objmode::Obj`: the last child of a
//! clean name wins, including component lists and renderer/mesh pointers.
//! Graph facts instead use `read::Read` and `read::components`, whose raw
//! dict names are preserved. Legacy component tuples use the Tier B dict
//! pointer reader after selecting the list through object mode.
//! Object-view shape/type refusals (`ViewError::Malformed`) are Tier U;
//! they are not claimed as matching Python exceptions. Resource refusals
//! remain fatal; ordinary malformed data is recoverable in graph walks.
use crate::{
    SceneError,
    read::{Read, list},
};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::{Environment, PPtr};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::objmode::{Class, Obj, ViewError};
use unityrs_core::value::{Parsed, Value};

// Only required attributes are read here; every missing pointer is an
// attribute error regardless of whether object mode built a known class.
// Thus these descriptors need only the required fields we consume, with
// no optional/default or slots-dependent behaviour to model.
const GAME_OBJECT: Class = Class {
    types: &["GameObject"],
    required: &["m_Component"],
    optional: &[],
    slotted: false,
};
const RENDERER: Class = Class {
    types: &["MeshRenderer", "SkinnedMeshRenderer"],
    required: &["m_GameObject"],
    optional: &[],
    slotted: false,
};
const MESH_FILTER: Class = Class {
    types: &["MeshFilter"],
    required: &["m_Mesh"],
    optional: &[],
    slotted: false,
};
const COMPONENT_PAIR: Class = Class {
    types: &["ComponentPair"],
    required: &["component"],
    optional: &[],
    slotted: false,
};
const POINTER: Class = Class {
    types: &["PPtr<GameObject>", "PPtr<Component>", "PPtr<Mesh>"],
    required: &["m_FileID", "m_PathID"],
    optional: &[],
    slotted: false,
};

fn pointer(obj: Obj<'_>, name: &'static str) -> Result<PPtr, SceneError> {
    let ptr = obj
        .class(name, &POINTER)?
        .ok_or(ViewError::NoAttribute(name))?;
    let file_id = ptr
        .int32("m_FileID")?
        .ok_or(ViewError::NoAttribute("m_FileID"))?;
    let path_id = ptr
        .get("m_PathID")?
        .ok_or(ViewError::NoAttribute("m_PathID"))?
        .1
        .as_i64()
        .ok_or(SceneError::Malformed("m_PathID"))?;
    Ok(PPtr {
        file_id: i32::try_from(file_id).map_err(|_| SceneError::Malformed("m_FileID"))?,
        path_id,
    })
}

/// Component pointers in serialized order, including null pointers.
/// `None`/an absent optional component field and an empty list mean no
/// components; old pairs contribute their second item.
pub fn components(go: &Parsed) -> Result<MeteredVec<PPtr>, SceneError> {
    let r = Read {
        p: go,
        meter: go.charge().meter(),
    };
    let mut out = MeteredVec::new(r.meter);
    let view = Obj::root(go, &GAME_OBJECT, r.meter)?;
    let v = match view.get("m_Component") {
        Ok(Some((_, v))) => v,
        Ok(None) | Err(ViewError::NoAttribute(_)) => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    let items = list(v)?;
    out.reserve_exact(items.len())?;
    r.meter.count_decode(items.len() as u64)?;
    if matches!(items.first(), Some(Value::Tuple(_))) {
        // Tier B: UnityPy's legacy accessor raises instead of returning
        // the second item. Preserve our documented dict-pointer answer.
        for item in items {
            let Value::Tuple(pair) = item else {
                return Err(SceneError::Malformed("component pair"));
            };
            out.push(r.ptr(pair.get(1).ok_or(SceneError::Malformed("component pair"))?)?)?;
        }
    } else {
        let pairs = view
            .list("m_Component", &COMPONENT_PAIR)?
            .ok_or(ViewError::NoAttribute("m_Component"))?;
        for pair in pairs.iter() {
            out.push(pointer(pair?, "component")?)?;
        }
    }
    Ok(out)
}

/// First component with exactly `class_id`, as UnityPy tests its type.
pub fn component(
    go: &ObjectRef,
    env: &Environment,
    class_id: i32,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    if go.class_id() != 1 {
        return Err(SceneError::WrongClass(go.class_id()));
    }
    let parsed = go.parse_as_dict(false)?;
    let ptrs = components(&parsed)?;
    meter.count_decode(ptrs.len() as u64)?;
    for ptr in ptrs.as_slice() {
        let obj = env.deref(go.file(), *ptr)?;
        if obj.class_id() == class_id {
            return Ok(Some(obj));
        }
    }
    Ok(None)
}
/// `GameObject.m_Transform` (exact Transform class).
pub fn m_transform(
    go: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    component(go, env, 4, meter)
}
/// `GameObject.m_MeshFilter`.
pub fn m_mesh_filter(
    go: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    component(go, env, 33, meter)
}
/// `GameObject.m_Animator`.
pub fn m_animator(
    go: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    component(go, env, 95, meter)
}
/// `GameObject.m_Animation`.
pub fn m_animation(
    go: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    component(go, env, 111, meter)
}
/// `GameObject.m_MeshRenderer`.
pub fn m_mesh_renderer(
    go: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    component(go, env, 23, meter)
}
/// `GameObject.m_SkinnedMeshRenderer`.
pub fn m_skinned_mesh_renderer(
    go: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    component(go, env, 137, meter)
}

/// A `SkinnedMeshRenderer`'s own mesh, or a `MeshRenderer`'s `GameObject`'s
/// `MeshFilter` mesh; `None` without a filter. No special built-in handling.
pub fn get_mesh(
    renderer: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<Option<ObjectRef>, SceneError> {
    let parsed = renderer.parse_as_dict(false)?;
    let r = Obj::root(&parsed, &RENDERER, meter)?;
    if renderer.class_id() == 137 {
        let mesh = pointer(r, "m_Mesh")?;
        return if mesh.is_set() {
            Ok(Some(env.deref(renderer.file(), mesh)?))
        } else {
            Ok(None)
        };
    }
    let go = env.deref(renderer.file(), pointer(r, "m_GameObject")?)?;
    let p = go.parse_as_dict(false)?;
    let ptrs = components(&p)?;
    meter.count_decode(ptrs.len() as u64)?;
    for ptr in ptrs.as_slice().iter().filter(|p| p.is_set()) {
        let filter = env.deref(go.file(), *ptr)?;
        if filter.class_id() != 33 {
            continue;
        }
        let p = filter.parse_as_dict(false)?;
        let mesh = pointer(Obj::root(&p, &MESH_FILTER, meter)?, "m_Mesh")?;
        if mesh.is_set() {
            return Ok(Some(env.deref(filter.file(), mesh)?));
        }
    }
    Ok(None)
}
