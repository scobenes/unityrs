//! Dict-mode component facts. Null references are None; a failed non-null
//! reference retains its reason and emits an issue. Refusals propagate.
use crate::{
    Component, Issue, Lod, LodInfo, PPtrTarget, RendererInfo, SceneError, SkinInfo, builtin,
    read::{Read, floats, list, uints},
};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::{Environment, PPtr};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::value::Value;

pub(crate) struct Reader<'a> {
    pub env: &'a Environment,
    pub meter: &'a Arc<Meter>,
    pub issues: &'a mut MeteredVec<Issue>,
}
impl Reader<'_> {
    pub fn target(
        &mut self,
        from: &ObjectRef,
        ptr: PPtr,
    ) -> Result<Option<PPtrTarget>, SceneError> {
        if !ptr.is_set() {
            return Ok(None);
        }
        if let Some(builtin) = builtin::recognize(from.file(), ptr, self.meter)? {
            self.issues.push(Issue::Builtin {
                from: from.clone(),
                builtin: builtin.clone(),
            })?;
            return Ok(Some(PPtrTarget::Builtin(builtin)));
        }
        match self.env.deref(from.file(), ptr) {
            Ok(o) => Ok(Some(PPtrTarget::Object(o))),
            Err(e) => {
                let error = SceneError::Deref(e);
                if error.resource_refusal() {
                    return Err(error);
                }
                let SceneError::Deref(reason) = error else {
                    return Err(error);
                };
                self.issues.push(Issue::Unresolved {
                    from: from.clone(),
                    pointer: ptr,
                    reason: reason.clone(),
                })?;
                Ok(Some(PPtrTarget::Unresolved(reason)))
            }
        }
    }
    fn targets(
        &mut self,
        from: &ObjectRef,
        r: &Read<'_>,
        name: &'static str,
    ) -> Result<MeteredVec<Option<PPtrTarget>>, SceneError> {
        let mut out = MeteredVec::new(self.meter);
        if let Some(v) = r.field(r.p.value(), name)? {
            return self.target_list(from, r, v);
        }
        out.reserve_exact(0)?;
        Ok(out)
    }
    fn target_list(
        &mut self,
        from: &ObjectRef,
        r: &Read<'_>,
        v: &Value,
    ) -> Result<MeteredVec<Option<PPtrTarget>>, SceneError> {
        let items = list(v)?;
        let mut out = MeteredVec::with_capacity(self.meter, items.len())?;
        self.meter.count_decode(items.len() as u64)?;
        for item in items {
            out.push(self.target(from, r.ptr(item)?)?)?;
        }
        Ok(out)
    }
    fn optional(
        &mut self,
        from: &ObjectRef,
        r: &Read<'_>,
        name: &'static str,
    ) -> Result<Option<PPtrTarget>, SceneError> {
        match r.optional_ptr(name)? {
            Some(p) => self.target(from, p),
            None => Ok(None),
        }
    }
    fn renderer(&mut self, object: &ObjectRef, r: &Read<'_>) -> Result<RendererInfo, SceneError> {
        let static_batch = r
            .field(r.p.value(), "m_StaticBatchInfo")?
            .map(|v| {
                let first = u16::try_from(r.uint(v, "firstSubMesh")?)
                    .map_err(|_| SceneError::Malformed("firstSubMesh"))?;
                let count = u16::try_from(r.uint(v, "subMeshCount")?)
                    .map_err(|_| SceneError::Malformed("subMeshCount"))?;
                Ok::<_, SceneError>((first, count))
            })
            .transpose()?;
        Ok(RendererInfo {
            object: object.clone(),
            materials: self.targets(object, r, "m_Materials")?,
            static_batch,
            subset_indices: r
                .field(r.p.value(), "m_SubsetIndices")?
                .map_or_else(|| Ok(MeteredVec::new(self.meter)), |v| uints(v, self.meter))?,
            enabled: r.boolean("m_Enabled", true)?,
            static_batch_root: self.optional(object, r, "m_StaticBatchRoot")?,
        })
    }
    pub fn component(&mut self, object: ObjectRef) -> Result<Component, SceneError> {
        // A Transform or an opaque component needs no second parse.
        if !matches!(object.class_id(), 23 | 33 | 137 | 205) {
            return Ok(Component::Other(object.class_id(), object));
        }
        let p = object.parse_as_dict(false)?;
        let r = Read {
            p: &p,
            meter: self.meter,
        };
        Ok(match object.class_id() {
            33 => Component::MeshFilter {
                mesh: self.optional(&object, &r, "m_Mesh")?,
                object,
            },
            23 => Component::MeshRenderer(self.renderer(&object, &r)?),
            137 => Component::SkinnedMeshRenderer(SkinInfo {
                renderer: self.renderer(&object, &r)?,
                mesh: self.optional(&object, &r, "m_Mesh")?,
                bones: self.targets(&object, &r, "m_Bones")?,
                root_bone: self.optional(&object, &r, "m_RootBone")?,
                blend_weights: r.field(p.value(), "m_BlendShapeWeights")?.map_or_else(
                    || Ok(MeteredVec::new(self.meter)),
                    |v| floats(v, self.meter),
                )?,
            }),
            205 => {
                let items = list(r.root("m_LODs")?)?;
                let mut lods = MeteredVec::with_capacity(self.meter, items.len())?;
                self.meter.count_decode(items.len() as u64)?;
                for item in items {
                    lods.push(Lod {
                        screen_relative_height: r.float(item, "screenRelativeHeight")?,
                        fade_transition_width: r
                            .field(item, "fadeTransitionWidth")?
                            .map_or(Ok(0.), |v| crate::read::f32_value(v, "fadeTransitionWidth"))?,
                        renderers: {
                            let items = list(r.req(item, "renderers")?)?;
                            let mut out = MeteredVec::with_capacity(self.meter, items.len())?;
                            self.meter.count_decode(items.len() as u64)?;
                            for item in items {
                                out.push(self.target(&object, r.ptr(r.req(item, "renderer")?)?)?)?;
                            }
                            out
                        },
                    })?;
                }
                Component::LodGroup(LodInfo {
                    object: object.clone(),
                    local_reference_point: r.xyz(r.root("m_LocalReferencePoint")?)?,
                    size: r.float(p.value(), "m_Size")?,
                    enabled: r.boolean("m_Enabled", true)?,
                    lods,
                })
            }
            id => return Err(SceneError::WrongClass(id)),
        })
    }
}
