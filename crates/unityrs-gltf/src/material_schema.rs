//! Fixed order material JSON and range/reference validation.
// Defaults and Unity enum values require exact comparisons.
#![allow(clippy::float_cmp)]
use crate::{AlphaMode, GltfError, Material, Scene, TextureInfo, json::Json, validate::invalid};
use unityrs_core::limits::Meter;
type Result<T> = std::result::Result<T, GltfError>;
pub(crate) fn validate_material(m: &Material, textures: usize, meter: &Meter) -> Result<()> {
    meter.count_decode(48)?;
    crate::validate::extras(&m.extras, meter)?;
    if m.base_color
        .iter()
        .chain(&m.emissive)
        .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
        || [m.metallic, m.roughness, m.occlusion_strength]
            .iter()
            .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
        || !m.alpha_cutoff.is_finite()
        || m.alpha_cutoff < 0.
        || !m.normal_scale.is_finite()
        || !m.emissive_strength.is_finite()
        || m.emissive_strength < 0.
    {
        return Err(invalid("material factor range"));
    }
    for info in m.textures().into_iter().flatten() {
        if info.texture >= textures
            || info
                .scale
                .iter()
                .chain(&info.offset)
                .any(|v| !v.is_finite())
        {
            return Err(invalid("material texture reference or transform"));
        }
    }
    Ok(())
}
pub(crate) fn extensions(s: &Scene, j: &mut Json<'_>) -> Result<()> {
    j.work(s.materials.len().saturating_mul(5))?;
    let unlit = s.materials.as_slice().iter().any(|m| m.unlit);
    let transform = s
        .materials
        .as_slice()
        .iter()
        .any(|m| m.textures().into_iter().flatten().any(transformed));
    let strength = s
        .materials
        .as_slice()
        .iter()
        .any(|m| m.emissive_strength != 1.);
    if unlit || transform || strength {
        j.raw(",\"extensionsUsed\":[")?;
        if unlit {
            j.string(b"KHR_materials_unlit")?;
        }
        if transform {
            if unlit {
                j.raw(",")?;
            }
            j.string(b"KHR_texture_transform")?;
        }
        if strength {
            if unlit || transform {
                j.raw(",")?;
            }
            j.string(b"KHR_materials_emissive_strength")?;
        }
        j.raw("]")?;
    }
    Ok(())
}
fn transformed(t: TextureInfo) -> bool {
    t.scale != [1.; 2] || t.offset != [0.; 2]
}
fn info(j: &mut Json<'_>, t: TextureInfo, extra: Option<(&str, f64)>) -> Result<()> {
    j.raw("{\"index\":")?;
    j.number(t.texture)?;
    if let Some((key, value)) = extra {
        j.raw(",")?;
        j.string(key.as_bytes())?;
        j.raw(":")?;
        j.float(value)?;
    }
    if transformed(t) {
        j.raw(",\"extensions\":{\"KHR_texture_transform\":{\"offset\":")?;
        j.floats(&t.offset)?;
        j.raw(",\"scale\":")?;
        j.floats(&t.scale)?;
        j.raw("}}")?;
    }
    j.raw("}")
}
pub(crate) fn materials(s: &Scene, j: &mut Json<'_>) -> Result<()> {
    if s.materials.is_empty() {
        return Ok(());
    }
    j.raw(",\"materials\":[")?;
    for (i, m) in s.materials.as_slice().iter().enumerate() {
        if i > 0 {
            j.raw(",")?;
        }
        j.raw("{\"doubleSided\":")?;
        j.raw(if m.double_sided { "true" } else { "false" })?;
        if m.base_color != [1.; 4]
            || m.base_color_texture.is_some()
            || m.metallic_roughness_texture.is_some()
            || m.metallic != 1.
            || m.roughness != 1.
        {
            j.raw(",\"pbrMetallicRoughness\":{\"baseColorFactor\":")?;
            j.floats(&m.base_color)?;
            if let Some(t) = m.base_color_texture {
                j.raw(",\"baseColorTexture\":")?;
                info(j, t, None)?;
            }
            if let Some(t) = m.metallic_roughness_texture {
                j.raw(",\"metallicRoughnessTexture\":")?;
                info(j, t, None)?;
            }
            j.raw(",\"metallicFactor\":")?;
            j.float(m.metallic)?;
            j.raw(",\"roughnessFactor\":")?;
            j.float(m.roughness)?;
            j.raw("}")?;
        }
        for (key, t, value) in [
            (
                "normalTexture",
                m.normal_texture,
                Some(("scale", m.normal_scale)),
            ),
            (
                "occlusionTexture",
                m.occlusion_texture,
                Some(("strength", m.occlusion_strength)),
            ),
            ("emissiveTexture", m.emissive_texture, None),
        ] {
            if let Some(t) = t {
                j.raw(",")?;
                j.string(key.as_bytes())?;
                j.raw(":")?;
                info(j, t, value)?;
            }
        }
        if m.emissive != [0.; 3] {
            j.raw(",\"emissiveFactor\":")?;
            j.floats(&m.emissive)?;
        }
        if m.alpha_mode != AlphaMode::Opaque {
            j.raw(",\"alphaMode\":")?;
            j.string(if m.alpha_mode == AlphaMode::Mask {
                b"MASK"
            } else {
                b"BLEND"
            })?;
            if m.alpha_mode == AlphaMode::Mask {
                j.raw(",\"alphaCutoff\":")?;
                j.float(m.alpha_cutoff)?;
            }
        }
        if m.unlit || m.emissive_strength != 1. {
            j.raw(",\"extensions\":{")?;
            if m.unlit {
                j.raw("\"KHR_materials_unlit\":{}")?;
            }
            if m.emissive_strength != 1. {
                if m.unlit {
                    j.raw(",")?;
                }
                j.raw("\"KHR_materials_emissive_strength\":{\"emissiveStrength\":")?;
                j.float(m.emissive_strength)?;
                j.raw("}")?;
            }
            j.raw("}")?;
        }
        super::schema::name(j, m.name.as_ref())?;
        super::schema::extras(j, &m.extras)?;
        j.raw("}")?;
    }
    j.raw("]")
}
