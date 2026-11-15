//! Primitive-local attribute omissions and material variants; no payload copies.
use crate::{
    Attribute, GltfError, Layout, Primitive, Scene, layout::PrimitiveLayout, validate::invalid,
};
use unityrs_core::limits::Meter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UvGaps {
    Preserve,
    Omit,
}

pub(crate) fn keep(l: &Layout, p: PrimitiveLayout, kind: Attribute, id: usize) -> bool {
    match kind {
        Attribute::Normal => p.normal,
        Attribute::Tangent => p.tangent,
        Attribute::Color | Attribute::CustomColor => {
            l.accessors.as_slice().get(id).is_some_and(|a| a.valid)
        }
        Attribute::TexCoord(_) => l
            .accessors
            .as_slice()
            .get(id)
            .is_some_and(|a| a.valid && (p.uv_gaps == UvGaps::Preserve || !a.uv_filler)),
        _ => true,
    }
}

pub(crate) fn material(
    s: &Scene,
    l: &mut Layout,
    p: &Primitive,
    pl: &mut PrimitiveLayout,
    meter: &Meter,
) -> Result<(), GltfError> {
    meter.count_decode(p.attributes.len() as u64 * 2 + 5)?;
    // TextureInfo currently addresses UV0. Surviving UV sets are compacted in
    // source order, so the first valid set becomes its fallback UV0.
    if p.attributes
        .as_slice()
        .iter()
        .any(|&(kind, id)| matches!(kind, Attribute::TexCoord(_)) && keep(l, *pl, kind, id))
    {
        return Ok(());
    }
    // Raw mode does not omit UVs and preserves texture references too. A
    // genuinely absent UV is already handled by collection's material mapper.
    if !p
        .attributes
        .as_slice()
        .iter()
        .any(|&(kind, id)| matches!(kind, Attribute::TexCoord(_)) && !keep(l, *pl, kind, id))
    {
        return Ok(());
    }
    let Some(source) = p.material else {
        return Ok(());
    };
    let m = s
        .materials
        .as_slice()
        .get(source)
        .ok_or_else(|| invalid("material source"))?;
    let count = m.textures().iter().flatten().count() as u64;
    if count == 0 {
        return Ok(());
    }
    l.report.dropped_texture_references += count;
    meter.count_decode(l.textureless_materials.len() as u64)?;
    let variant = if let Some(id) = l
        .textureless_materials
        .as_slice()
        .iter()
        .position(|&id| id == source)
    {
        id
    } else {
        let id = l.textureless_materials.len();
        l.textureless_materials.push(source)?;
        id
    };
    pl.material = Some(s.materials.len() + variant);
    Ok(())
}
