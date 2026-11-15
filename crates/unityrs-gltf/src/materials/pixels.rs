//! Slot encoding and Unity channel conversion, after the decoder's row flip.
use super::{GltfError, Result, texture::Source};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Usage {
    Color,
    OpaqueColor,
    Normal,
    ScaledNormal(f64),
    Occlusion,
    Metallic {
        smoothness: f64,
        metallic: Option<f64>,
        albedo_alpha: bool,
    },
}
pub(super) fn linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn byte(v: f64) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}
fn texel(src: &Source, i: usize) -> Result<[f64; 4]> {
    let width = match src.image.mode() {
        unityrs_texture::Mode::L => 1,
        unityrs_texture::Mode::Rgb => 3,
        unityrs_texture::Mode::Rgba => 4,
    };
    let p = src
        .image
        .pixels()
        .get(i * width..i * width + width)
        .ok_or(GltfError::Invalid("texture_malformed"))?;
    let c = |i| p.get(i).copied().map_or(1., |v| f64::from(v) / 255.);
    Ok(if width == 1 {
        [c(0), c(0), c(0), 1.]
    } else {
        [c(0), c(1), c(2), c(3)]
    })
}
fn wrapped(i: i64, n: u32, wrap: u32) -> usize {
    let n = i64::from(n);
    let i = match wrap {
        10497 => i.rem_euclid(n),
        33648 => {
            let p = i.rem_euclid(n * 2);
            if p < n { p } else { n * 2 - 1 - p }
        }
        _ => i.clamp(0, n - 1),
    };
    usize::try_from(i).unwrap_or(0)
}
#[allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]
fn sample(src: &Source, x: u32, y: u32, w: u32, h: u32, decode_rgb: bool) -> Result<[f64; 4]> {
    let (sw, sh) = (src.image.width(), src.image.height());
    let read = |i| {
        let mut p = texel(src, i)?;
        if decode_rgb {
            for c in &mut p[..3] {
                *c = linear(*c);
            }
        }
        Ok(p)
    };
    if (sw, sh) == (w, h) {
        return read(y as usize * sw as usize + x as usize);
    }
    let (u, v) = (
        (f64::from(x) + 0.5) / f64::from(w) * f64::from(sw),
        (f64::from(y) + 0.5) / f64::from(h) * f64::from(sh),
    );
    let sample = |x, y| {
        read(wrapped(y, sh, src.sampler.wrap_t) * sw as usize + wrapped(x, sw, src.sampler.wrap_s))
    };
    if src.sampler.mag_filter == 9728 {
        return sample(u.floor() as i64, v.floor() as i64);
    }
    let (u, v) = (u - 0.5, v - 0.5);
    let (x, y) = (u.floor() as i64, v.floor() as i64);
    let (fx, fy) = (u - u.floor(), v - v.floor());
    let (a, b, c, d) = (
        sample(x, y)?,
        sample(x + 1, y)?,
        sample(x, y + 1)?,
        sample(x + 1, y + 1)?,
    );
    let mut out = [0.; 4];
    for (slot, (((a, b), c), d)) in out.iter_mut().zip(a.into_iter().zip(b).zip(c).zip(d)) {
        *slot = (a * (1. - fx) + b * fx) * (1. - fy) + (c * (1. - fx) + d * fx) * fy;
    }
    Ok(out)
}
pub(super) fn dimensions(src: &Source, albedo: Option<&Source>) -> (u32, u32) {
    (
        src.image.width().max(albedo.map_or(0, |a| a.image.width())),
        src.image
            .height()
            .max(albedo.map_or(0, |a| a.image.height())),
    )
}
pub(super) fn convert(
    src: &Source,
    usage: Usage,
    albedo: Option<&Source>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>> {
    let (w, h) = dimensions(src, albedo);
    let size = u64::from(w) * u64::from(h) * 4;
    meter.count_decode(size)?;
    let mut out = MeteredVec::with_capacity(
        meter,
        usize::try_from(size).map_err(|_| GltfError::Invalid("texture_too_large"))?,
    )?;
    // Decode RGB before filtering, as Unity's sRGB texture sampling does.
    let color = matches!(usage, Usage::Color | Usage::OpaqueColor);
    let source_srgb = src.srgb.unwrap_or(color);
    for y in 0..h {
        for x in 0..w {
            let mut p = sample(src, x, y, w, h, source_srgb && !color)?;
            if !source_srgb && matches!(usage, Usage::Color | Usage::OpaqueColor) {
                for c in &mut p[..3] {
                    *c = srgb(*c);
                }
            }
            p = match usage {
                Usage::Color => p,
                Usage::OpaqueColor => [p[0], p[1], p[2], 1.],
                Usage::Occlusion => [p[1], p[1], p[2], p[3]],
                Usage::Normal | Usage::ScaledNormal(_) => {
                    // UnityCG.cginc UnpackNormalmapRGorAG:
                    // https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/CGIncludes/UnityCG.cginc
                    // Unity and glTF both use tangent-space +Y. Keep green's sign;
                    // geometry's V flip is accompanied by the tangent-w conversion.
                    let (mut nx, mut ny) = (p[0] * p[3] * 2. - 1., p[1] * 2. - 1.);
                    // Standard's UnpackScaleNormal scales before reconstructing Z.
                    // URP Packing.hlsl UnpackNormalScale scales after Z; glTF's
                    // normalTexture.scale preserves that path without baking.
                    if let Usage::ScaledNormal(scale) = usage {
                        nx *= scale;
                        ny *= scale;
                    }
                    let nz = (1. - nx * nx - ny * ny).max(0.).sqrt();
                    let length = (nx * nx + ny * ny + nz * nz).sqrt();
                    [
                        nx / length * 0.5 + 0.5,
                        ny / length * 0.5 + 0.5,
                        nz / length * 0.5 + 0.5,
                        1.,
                    ]
                }
                Usage::Metallic {
                    smoothness,
                    metallic,
                    albedo_alpha,
                } => {
                    let a = albedo
                        .map(|s| sample(s, x, y, w, h, false).map(|p| p[3]))
                        .transpose()?
                        .unwrap_or(if albedo_alpha { 1. } else { p[3] });
                    [0., 1. - a * smoothness, metallic.unwrap_or(p[0]), 1.]
                }
            };
            out.extend_from_slice(&p.map(byte))?;
        }
    }
    Ok(out)
}
