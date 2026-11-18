//! Material mapping and per-unit source-identity caches. Only resource and
//! limit refusals escape collection; malformed source data is diagnosed.
use crate::collect::{diagnostics, raw_text, same};
use crate::{GltfError, Material, Profile, Scene, Text, TextureInfo};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef,
    env::Environment,
    limits::{Meter, MeteredVec},
};
use unityrs_scene::{PPtrTarget, RendererInfo};
mod input;
mod mapper;
mod pixels;
mod texture;
pub use input::{MaterialInput, TextureInput};
pub use mapper::{CorePbr, UnlitExtras};
pub(super) type Result<T> = std::result::Result<T, GltfError>;
/// Injectable material policy. A mapper may request shared textures through
/// `MapCx`; data errors returned as Invalid degrade only this material.
pub trait MaterialMapper: Send + Sync {
    fn map(&self, material: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<Material>;
}
const _: () = crate::send_sync::<dyn MaterialMapper>();
struct Cached {
    object: ObjectRef,
    material: Option<usize>,
    skip: bool,
    uv: bool,
    issues: MeteredVec<crate::CollectionIssue>,
    reported: MeteredVec<ObjectRef>,
}
struct Shader {
    object: ObjectRef,
    name: Text,
}
struct CachedTexture {
    object: ObjectRef,
    usage: pixels::Usage,
    albedo: Option<ObjectRef>,
    mirror_once: bool,
    packed_sampler_conflict: bool,
    texture: Option<usize>,
    reason: Option<&'static str>,
}
pub(crate) struct State {
    pub(crate) builtins: crate::collect::builtin::Cache,
    materials: MeteredVec<Cached>,
    shaders: MeteredVec<Shader>,
    textures: MeteredVec<CachedTexture>,
}
impl State {
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            builtins: crate::collect::builtin::Cache::new(meter),
            materials: MeteredVec::new(meter),
            shaders: MeteredVec::new(meter),
            textures: MeteredVec::new(meter),
        }
    }
    fn shader(
        &mut self,
        env: &Environment,
        object: &ObjectRef,
        parsed: &unityrs_core::Parsed,
        meter: &Arc<Meter>,
    ) -> Result<Option<usize>> {
        let ptr = input::pointer(
            parsed,
            input::attr(parsed, parsed.value(), b"m_Shader", meter)?,
            meter,
        )?;
        if !ptr.is_set() {
            return Ok(None);
        }
        let (shader, _owner) = env
            .deref_held(object.file(), ptr, meter)
            .map_err(|e| GltfError::Mesh(e.into()))?;
        meter.count_decode(self.shaders.len() as u64)?;
        if let Some(i) = self
            .shaders
            .as_slice()
            .iter()
            .position(|s| same(&s.object, &shader))
        {
            return Ok(Some(i));
        }
        let read = || -> Result<Text> {
            let p = shader
                .parse_as_dict(false)
                .map_err(|e| GltfError::Mesh(e.into()))?;
            let name = if let Some(form) = input::optional(&p, p.value(), b"m_ParsedForm", meter)? {
                input::string(input::attr(&p, form, b"m_Name", meter)?)?
            } else {
                input::string(input::attr(&p, p.value(), b"m_Name", meter)?)?
            };
            raw_text(meter, name.as_bytes())
        };
        let name = match read() {
            Ok(name) => name,
            Err(e) if recoverable(&e) => crate::text(meter, "")?,
            Err(e) => return Err(e),
        };
        let i = self.shaders.len();
        self.shaders.push(Shader {
            object: shader,
            name,
        })?;
        Ok(Some(i))
    }
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "Keep material cache insertion and rollback together"
    )]
    pub(crate) fn binding(
        &mut self,
        env: &Environment,
        scene: &mut Scene,
        profile: &Profile,
        mapper: &dyn MaterialMapper,
        renderer: &RendererInfo,
        submesh: usize,
        uv: bool,
        node: usize,
        meter: &Arc<Meter>,
    ) -> Result<(Option<usize>, bool)> {
        let slot = material_slot(renderer, submesh, meter)?;
        let entry = slot.and_then(|i| renderer.materials.as_slice().get(i));
        if entry.is_none() {
            diagnostics::record(scene, node, "submesh_without_material", 1, meter)?;
            return Ok((None, false));
        }
        let target = entry.and_then(Option::as_ref);
        let resolved;
        let object = match target {
            Some(PPtrTarget::Object(o)) => o,
            Some(PPtrTarget::Builtin(b)) if profile.builtin == crate::Builtin::Resolve => {
                resolved = match self.builtins.resolve(env, b, meter) {
                    Ok(o) => o,
                    Err(e) if recoverable(&e) => {
                        diagnostics::record(scene, node, "missing_material", 1, meter)?;
                        return Ok((None, false));
                    }
                    Err(e) => return Err(e),
                };
                &resolved
            }
            _ => {
                diagnostics::record(scene, node, "missing_material", 1, meter)?;
                return Ok((None, false));
            }
        };
        meter.count_decode(self.materials.len() as u64)?;
        if let Some(c) = self
            .materials
            .as_mut_slice()
            .iter_mut()
            .find(|c| same(&c.object, object) && c.uv == uv)
        {
            meter.count_decode(c.reported.len() as u64)?;
            if !c.issues.is_empty()
                && !c
                    .reported
                    .as_slice()
                    .iter()
                    .any(|r| same(r, &renderer.object))
            {
                meter.count_decode(c.issues.len() as u64)?;
                copy_issues(
                    scene,
                    node,
                    renderer.object.path_id(),
                    c.issues.as_slice(),
                    meter,
                )?;
                c.reported.push(renderer.object.clone())?;
            }
            return Ok((c.material, c.skip));
        }
        let (images, textures, samplers, cached) = (
            scene.images.len(),
            scene.textures.len(),
            scene.samplers.len(),
            self.textures.len(),
        );
        let issue_start = scene.collection_issues.len();
        let result = self.map(
            env,
            scene,
            profile,
            mapper,
            object,
            uv,
            node,
            renderer.object.path_id(),
            meter,
        );
        let (material, skip) = match result {
            Ok(result) => result,
            Err(e) if recoverable(&e) => {
                scene.images.truncate(images);
                scene.textures.truncate(textures);
                scene.samplers.truncate(samplers);
                self.textures.truncate(cached);
                diagnostics::record(scene, node, "malformed_material", 1, meter)?;
                (None, false)
            }
            Err(e) => return Err(e),
        };
        let issues = crate::array(
            meter,
            scene
                .collection_issues
                .as_slice()
                .get(issue_start..)
                .ok_or(GltfError::Invalid("material issue range"))?,
        )?;
        let mut reported = MeteredVec::new(meter);
        if !issues.is_empty() {
            reported.push(renderer.object.clone())?;
        }
        self.materials.push(Cached {
            object: object.clone(),
            material,
            skip,
            uv,
            issues,
            reported,
        })?;
        Ok((material, skip))
    }
    #[allow(clippy::too_many_arguments)]
    fn map(
        &mut self,
        env: &Environment,
        scene: &mut Scene,
        profile: &Profile,
        mapper: &dyn MaterialMapper,
        object: &ObjectRef,
        uv: bool,
        node: usize,
        renderer_path_id: i64,
        meter: &Arc<Meter>,
    ) -> Result<(Option<usize>, bool)> {
        if object.class_id() != 21 {
            return Err(GltfError::Invalid("source not material"));
        }
        let p = object
            .parse_as_dict(false)
            .map_err(|e| GltfError::Mesh(e.into()))?;
        let shader = match self.shader(env, object, &p, meter) {
            Ok(s) => s,
            Err(e) if recoverable(&e) => None,
            Err(e) => return Err(e),
        };
        // A borrowed cache name cannot be held while the texture cache mutates.
        let name = match shader.and_then(|i| self.shaders.as_slice().get(i)) {
            Some(s) => Text(crate::array(meter, s.name.as_bytes())?),
            None => crate::text(meter, "")?,
        };
        if mapper::matches(&profile.shader_rules.skip, name.as_bytes(), meter)? {
            diagnostics::record(scene, node, "shader_skipped", 1, meter)?;
            return Ok((None, true));
        }
        let input = MaterialInput::new(&p, object, name.as_bytes(), meter)?;
        let mut cx = MapCx {
            env,
            scene,
            profile,
            meter,
            state: self,
            source: object,
            uv,
            node,
            renderer_path_id,
            issues: MeteredVec::new(meter),
        };
        if input.shader_name.is_empty() {
            cx.report("unknown_shader")?;
        }
        let mut material = mapper.map(&input, &mut cx)?;
        for (key, value) in cx.issues.as_slice() {
            diagnostics::merge(
                &mut material.extras,
                meter,
                std::str::from_utf8(key.as_bytes()).map_err(|_| GltfError::Invalid("issue key"))?,
                value.as_bytes(),
            )?;
        }
        // Custom mappers are checked before insertion as source data too.
        crate::material_schema::validate_material(&material, cx.scene.textures.len(), meter)?;
        let id = cx.scene.materials.len();
        cx.scene.materials.push(material)?;
        Ok((Some(id), false))
    }
}
fn copy_issues(
    scene: &mut Scene,
    node: usize,
    renderer: i64,
    issues: &[crate::CollectionIssue],
    meter: &Arc<Meter>,
) -> Result<()> {
    for issue in issues {
        diagnostics::record(scene, node, issue.reason, issue.count, meter)?;
        if let (Some(resource), Some(row)) = (
            issue.resource,
            scene.collection_issues.as_mut_slice().last_mut(),
        ) {
            row.resource = Some(crate::RendererResource {
                renderer_path_id: renderer,
                ..resource
            });
        }
    }
    Ok(())
}

/// One mapping's texture resolution and diagnostics context. Decodes are
/// released after each PNG; encoded images remain charged in the scene.
pub struct MapCx<'a> {
    pub env: &'a Environment,
    pub profile: &'a Profile,
    pub meter: &'a Arc<Meter>,
    scene: &'a mut Scene,
    state: &'a mut State,
    source: &'a ObjectRef,
    uv: bool,
    node: usize,
    renderer_path_id: i64,
    issues: crate::Extras,
}
const _: () = crate::send_sync::<MapCx<'static>>();
impl MapCx<'_> {
    /// Record a counted reason in Report, node extras, and material extras.
    pub fn report(&mut self, reason: &'static str) -> Result<()> {
        diagnostics::record(self.scene, self.node, reason, 1, self.meter)?;
        diagnostics::merge(
            &mut self.issues,
            self.meter,
            "material_skips",
            reason.as_bytes(),
        )
    }
    fn resource_issue(
        &mut self,
        reason: &'static str,
        pointer: unityrs_core::env::PPtr,
    ) -> Result<()> {
        self.report(reason)?;
        let issue = self
            .scene
            .collection_issues
            .as_mut_slice()
            .last_mut()
            .ok_or(GltfError::Invalid("resource diagnostic"))?;
        issue.resource = Some(crate::RendererResource {
            renderer_path_id: self.renderer_path_id,
            owner_path_id: self.source.path_id(),
            pointer,
        });
        Ok(())
    }

    /// Decode one `Texture2D` by identity, embed its PNG, and apply Unity ST.
    pub fn texture(&mut self, texture: TextureInput, key: &[u8]) -> Result<Option<TextureInfo>> {
        self.texture_as(texture, key, pixels::Usage::Color, None)
    }
    #[allow(clippy::too_many_lines)]
    fn texture_as(
        &mut self,
        texture: TextureInput,
        key: &[u8],
        usage: pixels::Usage,
        albedo: Option<TextureInput>,
    ) -> Result<Option<TextureInfo>> {
        if !self.uv {
            self.report("missing_texture_uv")?;
            return Ok(None);
        }
        if !texture.pointer.is_set() {
            return Ok(None);
        }
        if texture
            .scale
            .iter()
            .chain(&texture.offset)
            .any(|x| !x.is_finite())
        {
            self.report("invalid_texture_transform")?;
            return Ok(None);
        }
        let [sx, sy] = texture.scale;
        let [ox, oy] = texture.offset;
        let offset = [ox, 1. - sy - oy];
        if offset.iter().any(|v| !v.is_finite()) {
            self.report("invalid_texture_transform")?;
            return Ok(None);
        }
        let (object, _owner) =
            match self
                .env
                .deref_held(self.source.file(), texture.pointer, self.meter)
            {
                Ok(o) => o,
                Err(e) if diagnostics::deref_data(&e) => {
                    self.resource_issue("missing_texture", texture.pointer)?;
                    return Ok(None);
                }
                Err(e) => return Err(GltfError::Mesh(e.into())),
            };
        let (albedo, _albedo_owner) = match albedo {
            Some(t) if t.pointer.is_set() => {
                match self
                    .env
                    .deref_held(self.source.file(), t.pointer, self.meter)
                {
                    Ok((object, owner)) => (Some(object), owner),
                    Err(e) if diagnostics::deref_data(&e) => {
                        self.resource_issue("missing_texture", t.pointer)?;
                        (None, None)
                    }
                    Err(e) => return Err(GltfError::Mesh(e.into())),
                }
            }
            _ => (None, None),
        };
        self.meter.count_decode(self.state.textures.len() as u64)?;
        let (id, reason, mirror_once, packed_sampler_conflict) = if let Some(c) =
            self.state.textures.as_slice().iter().find(|c| {
                same(&c.object, &object)
                    && c.usage == usage
                    && match (&c.albedo, &albedo) {
                        (None, None) => true,
                        (Some(a), Some(b)) => same(a, b),
                        _ => false,
                    }
            }) {
            (
                c.texture,
                c.reason,
                c.mirror_once,
                c.packed_sampler_conflict,
            )
        } else {
            let result = texture::embed(
                self.env,
                self.scene,
                &object,
                key,
                usage,
                albedo.as_ref(),
                self.meter,
            );
            let (id, reason, mirror_once, packed_sampler_conflict) = match result {
                Ok(e) => (
                    Some(e.texture),
                    e.alpha_reason,
                    e.mirror_once,
                    e.packed_sampler_conflict,
                ),
                Err(e) if recoverable(&e) => (None, Some(texture_reason(&e)), false, false),
                Err(e) => return Err(e),
            };
            self.state.textures.push(CachedTexture {
                object,
                usage,
                albedo,
                mirror_once,
                packed_sampler_conflict,
                texture: id,
                reason,
            })?;
            (id, reason, mirror_once, packed_sampler_conflict)
        };
        if packed_sampler_conflict {
            self.report("packed_sampler_conflict")?;
        }
        if mirror_once {
            self.report("mirror_once")?;
        }
        if let Some(reason) = reason {
            self.resource_issue(reason, texture.pointer)?;
        }
        Ok(id.map(|texture| TextureInfo {
            texture,
            scale: [sx, sy],
            offset,
        }))
    }
}
fn texture_reason(e: &GltfError) -> &'static str {
    match e {
        GltfError::Invalid(
            reason @ ("undecodable_texture"
            | "missing_texture"
            | "unsupported_texture_platform"
            | "texture_encode_failed"
            | "texture_too_large"),
        ) => reason,
        _ => "texture_malformed",
    }
}
fn recoverable(e: &GltfError) -> bool {
    use unityrs_mesh::MeshError as M;
    match e {
        GltfError::Invalid(_) => true,
        GltfError::Mesh(M::Parse(e)) => diagnostics::parse_data(e),
        GltfError::Mesh(M::Core(e)) => diagnostics::core_data(e),
        GltfError::Mesh(M::Deref(e)) => diagnostics::deref_data(e),
        _ => false,
    }
}

/// Material slot paired with a legacy subset index, or offset by a modern batch.
fn material_slot(renderer: &RendererInfo, submesh: usize, meter: &Meter) -> Result<Option<usize>> {
    if let Some((first, _)) = renderer.static_batch.filter(|(_, n)| *n > 0) {
        return Ok(submesh.checked_sub(usize::from(first)));
    }
    meter.count_decode(renderer.subset_indices.len() as u64)?;
    if renderer.subset_indices.is_empty() {
        return Ok(Some(submesh));
    }
    Ok(renderer
        .subset_indices
        .as_slice()
        .iter()
        .position(|&i| i as usize == submesh))
}
/// A procedural primitive uses the renderer's first material slot.
pub(crate) fn first_submesh(renderer: &RendererInfo, meter: &Meter) -> Result<usize> {
    meter.count_decode(1)?;
    Ok(renderer
        .static_batch
        .filter(|(_, n)| *n > 0)
        .map(|(first, _)| usize::from(first))
        .or_else(|| {
            renderer
                .subset_indices
                .as_slice()
                .first()
                .map(|&n| n as usize)
        })
        .unwrap_or(0))
}
