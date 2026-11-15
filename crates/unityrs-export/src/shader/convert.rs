// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
//! Parsed form conversion: export/ShaderConverter.py:92-540.
use super::{
    Error, Program,
    output::Output,
    view::{Seq, View},
};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

pub(super) fn shader(
    v: View<'_>,
    platforms: Seq<'_>,
    programs: &[Program<'_>],
    out: &mut Output<'_>,
    meter: &Arc<Meter>,
) -> Result<(), Error> {
    out.text("Shader \"")?;
    out.text(v.field("m_Name")?.text()?)?;
    out.text("\" {\n")?;
    properties(v.field("m_PropInfo")?, out)?;
    for sub in v.field("m_SubShaders")?.list()? {
        out.text("SubShader {\n")?;
        let lod = sub.field("m_LOD")?.int()?;
        if lod != 0 {
            out.text(" LOD ")?;
            out.int(i128::from(lod))?;
            out.text("\n")?;
        }
        tags(sub.field("m_Tags")?, 1, out)?;
        for pass in sub.field("m_Passes")?.list()? {
            pass_write(pass, platforms, programs, out, meter)?;
        }
        out.text("}\n")?;
    }
    for (key, label) in [
        ("m_FallbackName", "Fall back"),
        ("m_CustomEditorName", "CustomEditor"),
    ] {
        let name = v.field(key)?.text()?;
        if !name.is_empty() {
            out.text(label)?;
            out.text(" \"")?;
            out.text(name)?;
            out.text("\"\n")?;
        }
    }
    out.text("}")
}
fn tags(v: View<'_>, indent: usize, out: &mut Output<'_>) -> Result<(), Error> {
    let tags = v.field("tags")?;
    if tags.empty() {
        return Ok(());
    }
    for _ in 0..indent {
        out.text(" ")?;
    }
    out.text("Tags { ")?;
    for pair in tags.list()? {
        let mut items = pair.list()?;
        let key = items.next().ok_or(Error::Field("tag key"))?.text()?;
        let value = items.next().ok_or(Error::Field("tag value"))?.text()?;
        out.text("\"")?;
        out.text(key)?;
        out.text("\" = \"")?;
        out.text(value)?;
        out.text("\" ")?;
    }
    out.text("}\n")
}
fn properties(v: View<'_>, out: &mut Output<'_>) -> Result<(), Error> {
    // Python joins a first element already ending in a newline with '\n'.
    out.text("Properties {\n")?;
    for prop in v.field("m_Props")?.list()? {
        out.text("\n")?;
        for attr in prop.field("m_Attributes")?.list()? {
            out.text("[")?;
            out.text(attr.text()?)?;
            out.text("] ")?;
        }
        out.text(prop.field("m_Name")?.text()?)?;
        out.text(" (\"")?;
        out.text(prop.field("m_Description")?.text()?)?;
        out.text("\", ")?;
        let kind = prop.field("m_Type")?.int()?;
        match kind {
            0 => out.text("Color")?,
            1 => out.text("Vector")?,
            2 => out.text("Float")?,
            3 => {
                out.text("Range(")?;
                out.float(prop.field("m_DefValue_1_")?.num()?, true)?;
                out.text(", ")?;
                out.float(prop.field("m_DefValue_2_")?.num()?, true)?;
                out.text(")")?;
            }
            4 => {
                let dim = prop.field("m_DefTexture")?.field("m_TexDim")?.int()?;
                out.text(match dim {
                    1 => "any",
                    2 => "2D",
                    3 => "3D",
                    4 => "Cube",
                    5 => "2DArray",
                    6 => "CubeArray",
                    _ => "",
                })?;
            }
            _ => {}
        }
        out.text(") = ")?;
        match kind {
            0 | 1 => {
                out.text("(")?;
                for (i, key) in [
                    "m_DefValue_0_",
                    "m_DefValue_1_",
                    "m_DefValue_2_",
                    "m_DefValue_3_",
                ]
                .iter()
                .enumerate()
                {
                    if i > 0 {
                        out.text(",")?;
                    }
                    out.float(prop.field(key)?.num()?, true)?;
                }
                out.text(")")?;
            }
            2 | 3 => out.float(prop.field("m_DefValue_0_")?.num()?, false)?,
            4 => {
                out.text("\"")?;
                out.text(prop.field("m_DefTexture")?.field("m_DefaultName")?.text()?)?;
                out.text("\" { }")?;
            }
            n => return Err(Error::PropertyType(n)),
        }
        out.text("\n")?;
    }
    out.text("\n}\n")
}
#[allow(clippy::float_cmp)] // UnityPy compares exact serialized state constants.
fn state(v: View<'_>, out: &mut Output<'_>) -> Result<(), Error> {
    let name = v.field("m_Name")?.text()?;
    if !name.is_empty() {
        out.text(" Name \"")?;
        out.text(name)?;
        out.text("\"\n")?;
    }
    let lod = v.field("m_LOD")?.int()?;
    if lod != 0 {
        out.text("  LOD ")?;
        out.int(i128::from(lod))?;
        out.text("\n")?;
    }
    tags(v.field("m_Tags")?, 2, out)?;
    let val = |key| v.field(key)?.field("val")?.num();
    if val("alphaToMask")? > 0. {
        out.text(" AlphaToMask On\n")?;
    }
    let clip = v.optional("zClip")?.ok_or(Error::Attribute("zClip"))?;
    if clip.field("val")?.num()? != 1. {
        out.text(" ZClip Off\n")?;
    }
    let z = val("zTest")?;
    if z != 4. {
        out.text(" ZTest ")?;
        out.text(match z {
            0. => "Off",
            1. => "Never",
            2. => "Less",
            3. => "Equal",
            5. => "Greater",
            6. => "NotEqual",
            7. => "GEqual",
            8. => "Always",
            _ => "",
        })?;
        out.text("\n")?;
    }
    if val("zWrite")? != 1. {
        out.text(" ZWrite Off\n")?;
    }
    let c = val("culling")?;
    if c != 2. {
        out.text(" Cull ")?;
        out.text(match c {
            0. => "Off",
            1. => "Front",
            _ => "",
        })?;
        out.text("\n")?;
    }
    let factor = val("offsetFactor")?;
    let units = val("offsetUnits")?;
    if factor != 0. || units != 0. {
        out.text(" Offset ")?;
        out.float(factor, false)?;
        out.text(", ")?;
        out.float(units, false)?;
        out.text("\n")?;
    }
    if v.field("lighting")?.boolean()? {
        out.text("  Lighting On\n")?;
    }
    out.text("  GpuProgramID ")?;
    out.int(i128::from(v.field("gpuProgramID")?.int()?))?;
    out.text("\n")
}
fn pass_write(
    v: View<'_>,
    platforms: Seq<'_>,
    programs: &[Program<'_>],
    out: &mut Output<'_>,
    meter: &Arc<Meter>,
) -> Result<(), Error> {
    let kind = v.field("m_Type")?.int()?;
    out.text(match kind {
        0 => " Pass ",
        1 => " UsePass ",
        2 => " GrabPass ",
        _ => "",
    })?;
    if kind == 1 {
        out.text("\"")?;
        out.text(v.field("m_UseName")?.text()?)?;
        return out.text("\"\n");
    }
    out.text("{\n")?;
    if kind == 2 {
        let name = v.field("m_TextureName")?.text()?;
        if !name.is_empty() {
            out.text(" \"")?;
            out.text(name)?;
            out.text("\"\n")?;
        }
    } else {
        state(v.field("m_State")?, out)?;
        for (key, label) in [
            ("progVertex", "vp"),
            ("progFragment", "fp"),
            ("progGeometry", "gp"),
            ("progHull", "hp"),
            ("progDomain", "dp"),
        ] {
            let stage = v.field(key)?;
            // Only the Tier A field. E3 can select player programs here.
            let subs = stage.field("m_SubPrograms")?;
            if !subs.empty() {
                out.text("Program \"")?;
                out.text(label)?;
                out.text("\" {\n")?;
                subprograms(subs, platforms, programs, out, meter)?;
                out.text("}\n")?;
            }
        }
    }
    out.text("}\n")
}
fn subprograms(
    v: View<'_>,
    platforms: Seq<'_>,
    programs: &[Program<'_>],
    out: &mut Output<'_>,
    meter: &Arc<Meter>,
) -> Result<(), Error> {
    let mut sorted = MeteredVec::new(meter);
    for (ordinal, sub) in v.list()?.enumerate() {
        meter.count_decode(1)?;
        sorted.push((
            sub.field("m_BlobIndex")?.int()?,
            sub.field("m_GpuProgramType")?.int()?,
            ordinal,
            sub,
        ))?;
    }
    // Include ordinal to reproduce both Python stable sorts without a sort allocation.
    super::sort::by_key(sorted.as_mut_slice(), |s| (s.0, s.1, s.2), meter)?;
    let mut at = 0;
    while let Some(first) = sorted.as_slice().get(at) {
        let mut end = at + 1;
        while let Some(next) = sorted.as_slice().get(end) {
            meter.count_decode(1)?;
            if (next.0, next.1) != (first.0, first.1) {
                break;
            }
            end += 1;
        }
        for i in 0..platforms.len().min(programs.len()) {
            meter.count_decode(1)?;
            let platform = platforms.int(i)?;
            if usable(platform, first.1)? {
                for sub in sorted.as_slice().get(at..end).ok_or(Error::Index)? {
                    out.text("SubProgram \"")?;
                    out.text(platform_name(platform))?;
                    out.text(" ")?;
                    if end - at > 1 {
                        out.text("hw_tier")?;
                        let tier = sub.3.field("m_ShaderHardwareTier")?.int()?;
                        // Python two-character formatting includes the sign in the two-character width.
                        if (0..10).contains(&tier) {
                            out.text("0")?;
                        }
                        out.int(i128::from(tier))?;
                        out.text(" ")?;
                    }
                    out.text("\" {\n")?;
                    programs.get(i).ok_or(Error::Index)?.write(sub.0, out)?;
                    out.text("\n}\n")?;
                }
                break;
            }
        }
        at = end;
    }
    Ok(())
}
// export/ShaderConverter.py:375-493. Metal intentionally accepts VS only.
pub(super) fn usable(platform: i64, kind: i64) -> Result<bool, Error> {
    Ok(match platform {
        0 => kind == 1,
        1 => (9..=12).contains(&kind),
        2 | 10 | 11 | 12 | 16 | 17 | 19 | 20 => (26..=30).contains(&kind),
        4 => (15..=22).contains(&kind),
        5 => kind == 5,
        8 => kind == 13 || kind == 14,
        9 => (2..=4).contains(&kind),
        14 => kind == 23,
        15 => (6..=8).contains(&kind),
        18 => kind == 25,
        _ => return Err(Error::NotImplemented),
    })
}
// export/ShaderConverter.py:496-540.
pub(super) fn platform_name(platform: i64) -> &'static str {
    match platform {
        0 => "openGL",
        1 => "d3d9",
        2 => "xbox360",
        3 => "ps3",
        4 => "d3d11",
        5 => "gles",
        6 => "glesdesktop",
        7 => "flash",
        8 => "d3d11_9x",
        9 => "gles3",
        10 => "psp2",
        11 => "ps4",
        12 => "xboxone",
        13 => "psm",
        14 => "metal",
        15 => "glcore",
        16 => "n3ds",
        17 => "wiiu",
        18 => "vulkan",
        19 => "switch",
        20 => "xboxone+_d3d12",
        _ => "unknown",
    }
}
