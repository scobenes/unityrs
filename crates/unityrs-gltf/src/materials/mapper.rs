//! The built-in mappings are intentionally conservative: custom shaders retain
//! their saved properties and base colour; drawable rules opt them into PBR.
// Defaults and Unity enum values require exact comparisons.
#![allow(clippy::float_cmp)]
use super::{
    GltfError, MapCx, MaterialInput, Result,
    pixels::{Usage, linear},
};
use crate::collect::{append_fmt, extra, raw_text};
use crate::{AlphaMode, Material, Text};
use unityrs_core::limits::{Meter, MeteredVec};
/// Base colour and `KHR_materials_unlit`, plus allowlisted saved properties.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnlitExtras;
const _: () = crate::send_sync::<UnlitExtras>();
/// Built-in Standard and Universal Render Pipeline/Lit core PBR factors/maps.
#[derive(Debug, Clone, Copy, Default)]
pub struct CorePbr;
const _: () = crate::send_sync::<CorePbr>();
impl super::MaterialMapper for UnlitExtras {
    fn map(&self, m: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<Material> {
        let mut out = base(m, cx)?;
        out.unlit = true;
        out.metallic = 0.;
        out.roughness = 1.;
        Ok(out)
    }
}
impl super::MaterialMapper for CorePbr {
    fn map(&self, m: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<Material> {
        let known = matches!(
            m.shader_name,
            b"Standard" | b"Standard (Specular setup)" | b"Universal Render Pipeline/Lit"
        );
        if !known
            && !matches(&cx.profile.shader_rules.drawable, m.shader_name, cx.meter)?
            && !matches(&cx.profile.shader_rules.additive, m.shader_name, cx.meter)?
        {
            if !m.shader_name.is_empty() {
                cx.report("unknown_shader")?;
            }
            return super::MaterialMapper::map(&UnlitExtras, m, cx);
        }
        let mut out = base(m, cx)?;
        out.metallic = bounded(m.float(b"_Metallic")?.unwrap_or(0.), 0., cx)?;
        let smooth = if m.shader_name == b"Universal Render Pipeline/Lit" {
            m.float(b"_Smoothness")?.or(m.float(b"_Glossiness")?)
        } else {
            m.float(b"_Glossiness")?.or(m.float(b"_Smoothness")?)
        }
        .unwrap_or(0.5);
        out.roughness = 1. - bounded(smooth, 0.5, cx)?;
        metallic(&mut out, m, cx, smooth)?;
        let normal_scale = finite(
            m.float(b"_BumpScale")?
                .or(m.float(b"_NormalScale")?)
                .unwrap_or(1.),
            1.,
            cx,
        )?;
        let standard = matches!(m.shader_name, b"Standard" | b"Standard (Specular setup)");
        out.normal_texture = map_texture_as(
            m,
            cx,
            &[b"_BumpMap", b"_NormalMap"],
            if standard {
                Usage::ScaledNormal(normal_scale)
            } else {
                Usage::Normal
            },
        )?;
        out.normal_scale = if standard { 1. } else { normal_scale };
        out.occlusion_texture = map_texture_as(m, cx, &[b"_OcclusionMap"], Usage::Occlusion)?;
        out.occlusion_strength = bounded(m.float(b"_OcclusionStrength")?.unwrap_or(1.), 1., cx)?;
        if m.keyword(b"_EMISSION")? {
            let color = m.color(b"_EmissionColor")?.unwrap_or([0.; 4]);
            for (slot, v) in out.emissive.iter_mut().zip(color) {
                *slot = linear(finite(v, 0., cx)?.max(0.));
            }
            let max = out.emissive.iter().copied().fold(1., f64::max);
            if max.is_finite() {
                out.emissive_strength = max;
                for v in &mut out.emissive {
                    *v /= max;
                }
            } else {
                cx.report("invalid_material_value")?;
                out.emissive = [0.; 3];
            }
            out.emissive_texture = map_texture(m, cx, &[b"_EmissionMap"])?;
        }
        Ok(out)
    }
}
fn metallic(
    out: &mut Material,
    m: &MaterialInput<'_>,
    cx: &mut MapCx<'_>,
    smooth: f64,
) -> Result<()> {
    let specular = m.shader_name == b"Standard (Specular setup)"
        || (m.shader_name == b"Universal Render Pipeline/Lit"
            && m.float(b"_WorkflowMode")? == Some(0.));
    if specular {
        cx.report("specular_workflow")?;
        out.metallic = 0.;
        for key in [b"_SpecColor".as_slice(), b"_SpecGlossMap"] {
            saved_extra(out, m, cx, key)?;
        }
    }
    // Unity shader sources: UnityStandardInput.cginc MetallicGloss/SpecularGloss
    // https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/CGIncludes/UnityStandardInput.cginc
    // URP SampleMetallicSpecGloss (LitInput / MetallicSpecGloss):
    // https://github.com/Unity-Technologies/Graphics/blob/master/Packages/com.unity.render-pipelines.universal/Shaders/Utils/MetallicSpecGloss.hlsl
    let gloss = if specular {
        b"_SpecGlossMap".as_slice()
    } else {
        b"_MetallicGlossMap"
    };
    let urp = m.shader_name == b"Universal Render Pipeline/Lit";
    let scale = bounded(
        m.float(if urp {
            b"_Smoothness"
        } else {
            b"_GlossMapScale"
        })?
        .unwrap_or(if urp { smooth } else { 1. }),
        1.,
        cx,
    )?;
    let albedo_alpha = albedo_smoothness(m)?;
    let albedo = if albedo_alpha {
        base_texture(m)?.map(|(_, t)| t)
    } else {
        None
    };
    if let Some((key, mut texture)) = find_texture(m, &[gloss])? {
        shared_st(m, &mut texture)?;
        out.metallic_roughness_texture = cx.texture_as(
            texture,
            key,
            Usage::Metallic {
                smoothness: scale,
                metallic: specular.then_some(0.),
                albedo_alpha,
            },
            albedo,
        )?;
        if out.metallic_roughness_texture.is_some() {
            out.metallic = if specular { 0. } else { 1. };
            out.roughness = 1.;
        }
    } else if let Some(texture) = albedo {
        // With no gloss map Unity still permits smoothness from albedo alpha.
        let smoothness = scale;
        out.metallic_roughness_texture = cx.texture_as(
            texture,
            b"_SmoothnessTextureChannel",
            Usage::Metallic {
                smoothness,
                metallic: Some(out.metallic),
                albedo_alpha: false,
            },
            None,
        )?;
        if out.metallic_roughness_texture.is_some() {
            out.roughness = 1.;
            if !specular {
                out.metallic = 1.;
            }
        }
    } else if albedo_alpha {
        out.roughness = 1. - scale;
    }
    Ok(())
}
fn finite(x: f64, default: f64, cx: &mut MapCx<'_>) -> Result<f64> {
    if x.is_finite() {
        Ok(x)
    } else {
        cx.report("invalid_material_value")?;
        Ok(default)
    }
}
fn bounded(x: f64, default: f64, cx: &mut MapCx<'_>) -> Result<f64> {
    let x = finite(x, default, cx)?;
    let v = x.clamp(0., 1.);
    if v != x {
        cx.report("material_value_clamped")?;
    }
    Ok(v)
}
fn base(m: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<Material> {
    let mut out = Material::new(cx.meter);
    out.name = Some(raw_text(cx.meter, m.name.as_bytes())?);
    let urp = m.shader_name == b"Universal Render Pipeline/Lit";
    let texture_only_unlit = matches!(
        m.shader_name,
        b"Unlit/Texture" | b"Unlit/Transparent" | b"Unlit/Transparent Cutout"
    );
    let color = if texture_only_unlit {
        saved_extra(&mut out, m, cx, b"_Color")?;
        None
    } else if urp {
        m.color(b"_BaseColor")?.or(m.color(b"_Color")?)
    } else {
        m.color(b"_Color")?.or(m.color(b"_BaseColor")?)
    }
    .unwrap_or([1.; 4]);
    for (i, (slot, v)) in out.base_color.iter_mut().zip(color).enumerate() {
        let v = bounded(v, 1., cx)?;
        *slot = if i < 3 { linear(v) } else { v };
    }
    out.vertex_colors = !ignores_vertex_colors(m.shader_name);
    let usage = if albedo_smoothness(m)? {
        Usage::OpaqueColor
    } else {
        Usage::Color
    };
    out.base_color_texture = if m.shader_name == b"Unlit/Color" {
        // Unlit-Color.shader's fragment shader returns _Color without sampling:
        // https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/DefaultResourcesExtra/Unlit/Unlit-Color.shader
        // Preserve texture references left over from a previous shader in extras.
        saved_extra(&mut out, m, cx, b"_MainTex")?;
        saved_extra(&mut out, m, cx, b"_BaseMap")?;
        None
    } else if urp {
        map_texture_as(m, cx, &[b"_BaseMap", b"_MainTex"], usage)?
    } else {
        map_texture_as(m, cx, &[b"_MainTex", b"_BaseMap"], usage)?
    };
    let mode = m.float(b"_Mode")?.unwrap_or(0.);
    let cutoff = m.float(b"_Cutoff")?;
    out.alpha_mode = if matches(&cx.profile.shader_rules.additive, m.shader_name, cx.meter)? {
        AlphaMode::Blend
    } else if m.shader_name == b"Unlit/Transparent" {
        // Unlit-Alpha.shader declares Blend SrcAlpha OneMinusSrcAlpha, even
        // with an inherited queue and no saved alpha controls:
        // https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/DefaultResourcesExtra/Unlit/Unlit-Alpha.shader
        AlphaMode::Blend
    } else if m.shader_name == b"Unlit/Transparent Cutout" {
        // Unlit-AlphaTest.shader clips texture alpha against _Cutoff (default 0.5):
        // https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/DefaultResourcesExtra/Unlit/Unlit-AlphaTest.shader
        AlphaMode::Mask
    } else if urp {
        if m.float(b"_Surface")? == Some(1.) {
            if m.float(b"_AlphaClip")? == Some(1.) {
                cx.report("alpha_clip_dropped")?;
                extra(
                    &mut out.extras,
                    cx.meter,
                    "_Cutoff",
                    crate::collect::number(cx.meter, cutoff.unwrap_or(0.5))?,
                )?;
            }
            AlphaMode::Blend
        } else if m.float(b"_AlphaClip")? == Some(1.) {
            AlphaMode::Mask
        } else {
            AlphaMode::Opaque
        }
    } else if matches!(m.shader_name, b"Standard" | b"Standard (Specular setup)") {
        if mode == 2. || mode == 3. {
            AlphaMode::Blend
        } else if mode == 1. {
            AlphaMode::Mask
        } else {
            AlphaMode::Opaque
        }
    } else if m.render_queue >= 3000 || m.float(b"_Surface")? == Some(1.) {
        AlphaMode::Blend
    } else if m.float(b"_AlphaClip")? == Some(1.) || mode == 1. {
        AlphaMode::Mask
    } else {
        AlphaMode::Opaque
    };
    out.alpha_cutoff = bounded(cutoff.unwrap_or(0.5), 0.5, cx)?;
    out.double_sided = m.float(b"_Cull")? == Some(0.);
    extra(
        &mut out.extras,
        cx.meter,
        "shader",
        raw_text(cx.meter, m.shader_name)?,
    )?;
    allowlisted(&mut out, m, cx)?;
    Ok(out)
}
// URP LitGUI disables albedo-alpha smoothness for transparent surfaces,
// even when a saved channel value survives a switch from opaque.
fn albedo_smoothness(m: &MaterialInput<'_>) -> Result<bool> {
    Ok(known_lit(m.shader_name)
        && m.float(b"_SmoothnessTextureChannel")? == Some(1.)
        && !(m.shader_name == b"Universal Render Pipeline/Lit"
            && m.float(b"_Surface")? == Some(1.)))
}
fn known_lit(shader: &[u8]) -> bool {
    matches!(
        shader,
        b"Standard" | b"Standard (Specular setup)" | b"Universal Render Pipeline/Lit"
    )
}
fn ignores_vertex_colors(shader: &[u8]) -> bool {
    matches!(
        shader,
        b"Standard"
            | b"Standard (Specular setup)"
            | b"Universal Render Pipeline/Lit"
            | b"Universal Render Pipeline/Simple Lit"
            | b"Universal Render Pipeline/Unlit"
            | b"Unlit/Texture"
            | b"Unlit/Color"
            | b"Unlit/Transparent"
            | b"Unlit/Transparent Cutout"
    )
}
fn base_texture<'a>(m: &MaterialInput<'_>) -> Result<Option<(&'a [u8], super::TextureInput)>> {
    if m.shader_name == b"Universal Render Pipeline/Lit" {
        find_texture(m, &[b"_BaseMap", b"_MainTex"])
    } else {
        find_texture(m, &[b"_MainTex", b"_BaseMap"])
    }
}
fn find_texture<'a>(
    m: &MaterialInput<'_>,
    keys: &[&'a [u8]],
) -> Result<Option<(&'a [u8], super::TextureInput)>> {
    for &key in keys {
        if let Some(mut texture) = m.texture(key)? {
            if !texture.pointer.is_set() {
                continue;
            }
            if let Some([sx, sy, ox, oy]) = match key {
                b"_MainTex" => m.color(b"_MainTex_ST")?,
                b"_BaseMap" => m.color(b"_BaseMap_ST")?,
                _ => None,
            } {
                texture.scale = [sx, sy];
                texture.offset = [ox, oy];
            }
            return Ok(Some((key, texture)));
        }
    }
    Ok(None)
}
fn map_texture(
    m: &MaterialInput<'_>,
    cx: &mut MapCx<'_>,
    keys: &[&[u8]],
) -> Result<Option<crate::TextureInfo>> {
    map_texture_as(m, cx, keys, Usage::Color)
}
fn map_texture_as(
    m: &MaterialInput<'_>,
    cx: &mut MapCx<'_>,
    keys: &[&[u8]],
    usage: Usage,
) -> Result<Option<crate::TextureInfo>> {
    let Some((key, mut texture)) = find_texture(m, keys)? else {
        return Ok(None);
    };
    shared_st(m, &mut texture)?;
    cx.texture_as(texture, key, usage, None)
}
fn shared_st(m: &MaterialInput<'_>, texture: &mut super::TextureInput) -> Result<()> {
    if !known_lit(m.shader_name) {
        return Ok(());
    }
    // Base ST applies even when the base texture reference is null.
    let keys: [&[u8]; 2] = if m.shader_name == b"Universal Render Pipeline/Lit" {
        [b"_BaseMap", b"_MainTex"]
    } else {
        [b"_MainTex", b"_BaseMap"]
    };
    let mut base = base_texture(m)?.map(|(_, t)| t);
    if base.is_none() {
        for key in keys {
            if let Some(t) = m.texture(key)? {
                base = Some(t);
                break;
            }
        }
    }
    let explicit = m.color(if m.shader_name == b"Universal Render Pipeline/Lit" {
        b"_BaseMap_ST"
    } else {
        b"_MainTex_ST"
    })?;
    let [sx, sy, ox, oy] = explicit
        .or(base.map(|t| [t.scale[0], t.scale[1], t.offset[0], t.offset[1]]))
        .unwrap_or([1., 1., 0., 0.]);
    texture.scale = [sx, sy];
    texture.offset = [ox, oy];
    Ok(())
}
fn saved_extra(
    out: &mut Material,
    m: &MaterialInput<'_>,
    cx: &MapCx<'_>,
    key: &[u8],
) -> Result<()> {
    if out
        .extras
        .as_slice()
        .iter()
        .any(|(k, _)| k.as_bytes() == key)
    {
        return Ok(());
    }
    let mut value = Text(MeteredVec::new(cx.meter));
    if let Some(v) = m.color(key)? {
        append_fmt(
            &mut value,
            cx.meter,
            format_args!("{},{},{},{}", v[0], v[1], v[2], v[3]),
        )?;
    } else if let Some(t) = m.texture(key)? {
        append_fmt(
            &mut value,
            cx.meter,
            format_args!(
                "{}:{};scale={},{};offset={},{}",
                t.pointer.file_id,
                t.pointer.path_id,
                t.scale[0],
                t.scale[1],
                t.offset[0],
                t.offset[1]
            ),
        )?;
    } else {
        return Ok(());
    }
    extra(
        &mut out.extras,
        cx.meter,
        std::str::from_utf8(key).map_err(|_| GltfError::Invalid("extra key"))?,
        value,
    )
}
fn allowlisted(out: &mut Material, m: &MaterialInput<'_>, cx: &MapCx<'_>) -> Result<()> {
    for key in cx.profile.material_property_allowlist.as_slice() {
        cx.meter.count_decode(
            (out.extras.len() as u64).saturating_mul(key.as_bytes().len() as u64 + 1),
        )?;
        if out
            .extras
            .as_slice()
            .iter()
            .any(|(k, _)| k.as_bytes() == key.as_bytes())
        {
            continue;
        }
        let mut value = Text(MeteredVec::new(cx.meter));
        if let Some(v) = m.color(key.as_bytes())? {
            append_fmt(
                &mut value,
                cx.meter,
                format_args!(
                    "{},{},{},{}",
                    v.first().copied().unwrap_or(0.),
                    v.get(1).copied().unwrap_or(0.),
                    v.get(2).copied().unwrap_or(0.),
                    v.get(3).copied().unwrap_or(0.)
                ),
            )?;
        } else if let Some(v) = m.float(key.as_bytes())? {
            append_fmt(&mut value, cx.meter, format_args!("{v}"))?;
        } else if let Some(t) = m.texture(key.as_bytes())? {
            let [sx, sy] = t.scale;
            let [ox, oy] = t.offset;
            append_fmt(
                &mut value,
                cx.meter,
                format_args!(
                    "{}:{};scale={sx},{sy};offset={ox},{oy}",
                    t.pointer.file_id, t.pointer.path_id
                ),
            )?;
        } else {
            continue;
        }
        extra(
            &mut out.extras,
            cx.meter,
            std::str::from_utf8(key.as_bytes())
                .map_err(|_| GltfError::Invalid("allowlist UTF-8"))?,
            value,
        )?;
    }
    Ok(())
}
/// Bytewise glob with * and ?, no allocations or regex backtracking. Count
/// the worst-case retry comparisons before the scan.
pub(super) fn matches(patterns: &MeteredVec<Text>, name: &[u8], meter: &Meter) -> Result<bool> {
    for pattern in patterns.as_slice() {
        let p = pattern.as_bytes();
        meter.count_decode((p.len() as u64 + 1).saturating_mul(name.len() as u64 + 1))?;
        let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
        while j < name.len() {
            if p.get(i)
                .is_some_and(|b| *b == b'?' || Some(b) == name.get(j))
            {
                i += 1;
                j += 1;
            } else if p.get(i) == Some(&b'*') {
                star = Some(i);
                i += 1;
                retry = j;
            } else if let Some(s) = star {
                retry += 1;
                j = retry;
                i = s + 1;
            } else {
                break;
            }
        }
        if j == name.len() {
            while p.get(i) == Some(&b'*') {
                i += 1;
            }
            if i == p.len() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
