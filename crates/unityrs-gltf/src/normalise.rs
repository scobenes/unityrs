//! Fixed-width normalisation shared by analysis and streaming, without staging.
use crate::validate::invalid;
use crate::{Accessor, GltfError, Kind};

pub(crate) fn row(a: &Accessor, row: usize, normalise: bool) -> Result<[f32; 16], GltfError> {
    let mut out = [0f32; 16];
    let width = a.kind.width();
    for (i, value) in out.iter_mut().take(width).enumerate() {
        *value = round(
            a.data
                .value(row * width + i)
                .ok_or_else(|| invalid("accessor data"))?,
        );
    }
    if normalise {
        match a.kind {
            Kind::Normal | Kind::Tangent => {
                // f64 intermediates avoid f32 overflow/underflow, with sqrt
                // and correctly rounded elementary operations, no libm trig.
                let [x, y, z, ..] = out;
                let length = (f64::from(x) * f64::from(x)
                    + f64::from(y) * f64::from(y)
                    + f64::from(z) * f64::from(z))
                .sqrt();
                if length > 0. && length.is_finite() {
                    for value in out.iter_mut().take(3) {
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            *value = (f64::from(*value) / length) as f32;
                        }
                    }
                }
                if a.kind == Kind::Tangent
                    && let Some(w) = out.get_mut(3)
                {
                    // Preserve the sign bit so X reflection commutes with
                    // w normalisation, including signed zero and NaN.
                    *w = 1.0f32.copysign(*w);
                }
            }
            Kind::Weights => {
                let mut total = 0.;
                for value in out.iter_mut().take(4) {
                    if !value.is_finite() || *value < 0. {
                        *value = 0.;
                    }
                    total += f64::from(*value);
                }
                if total == 0. {
                    out = [0.; 16];
                    if let Some(v) = out.first_mut() {
                        *v = 1.;
                    }
                } else {
                    for value in out.iter_mut().take(4) {
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            *value = (f64::from(*value) / total) as f32;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}
pub(crate) fn invalid_direction(v: &[f32; 16]) -> bool {
    let [x, y, z, ..] = *v;
    !x.is_finite() || !y.is_finite() || !z.is_finite() || (x == 0. && y == 0. && z == 0.)
}

/// Whether clamping the rounded weights leaves an all-zero row.
pub(crate) fn zero_weights(a: &Accessor, index: usize) -> Result<bool, GltfError> {
    Ok(row(a, index, false)?
        .iter()
        .take(4)
        .all(|v| !v.is_finite() || *v <= 0.))
}

/// Effective joint row, including the palette-zero fallback for zero rows.
pub(crate) fn joints(
    a: &Accessor,
    weights: &Accessor,
    index: usize,
) -> Result<[u32; 4], GltfError> {
    let w = row(weights, index, true)?;
    let reset = zero_weights(weights, index)?;
    let mut out = [0; 4];
    for (col, value) in out.iter_mut().enumerate() {
        if !reset && w.get(col).is_some_and(|v| *v != 0.) {
            *value = a
                .data
                .integer(index * 4 + col)
                .ok_or_else(|| invalid("joint data"))?;
        }
    }
    Ok(out)
}

// Rust leaves NaN cast payloads unspecified. Preserve the sign and the high
// payload bits explicitly and quiet them, so raw-mode bytes are host-stable.
#[allow(clippy::cast_possible_truncation)]
fn round(value: f64) -> f32 {
    if value.is_nan() {
        let bits = value.to_bits();
        let sign = ((bits >> 32) as u32) & 0x8000_0000;
        let payload = ((bits >> 29) as u32) & 0x007f_ffff;
        f32::from_bits(sign | 0x7fc0_0000 | payload)
    } else {
        value as f32
    }
}
