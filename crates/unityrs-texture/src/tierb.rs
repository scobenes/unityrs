//! Independent closed forms, not ports of UnityPy's broken conversions.
//! PARITY.md section 17.1 defines each Tier B rule; sections 19.1 and
//! 19.2 apply the float rule to `RGBAHalf` and high-byte rule to RGB48.
//! No allocation: `decode::plan` validates input and charges the output.

use crate::Format;

#[derive(Clone, Copy)]
enum Channel {
    Half,
    Float,
    Unsigned,
    Signed,
    SharedExponent,
    Yuy2,
}

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub(crate) channels: u8,
    size: u8,
    channel: Channel,
}

impl Layout {
    pub(crate) const fn bytes(self) -> u64 {
        self.channels as u64 * self.size as u64
    }
}

pub(crate) const fn layout(format: Format) -> Option<Layout> {
    use Channel::{Float, Half, SharedExponent, Signed, Unsigned, Yuy2};
    let (channels, size, channel) = match format {
        Format::RHalf => (1, 2, Half),
        Format::RgHalf => (2, 2, Half),
        Format::RgbaHalf => (4, 2, Half),
        Format::RFloat => (1, 4, Float),
        Format::RgFloat => (2, 4, Float),
        Format::RgbFloat => (3, 4, Float),
        Format::RgbaFloat => (4, 4, Float),
        Format::R16 => (1, 2, Unsigned),
        Format::Rg16 => (2, 1, Unsigned),
        Format::Rg32 => (2, 2, Unsigned),
        Format::Rgb48 => (3, 2, Unsigned),
        Format::Rgba64 => (4, 2, Unsigned),
        Format::R8Signed => (1, 1, Signed),
        Format::Rg16Signed => (2, 1, Signed),
        Format::Rgb24Signed => (3, 1, Signed),
        Format::Rgba32Signed => (4, 1, Signed),
        Format::R16Signed => (1, 2, Signed),
        Format::Rg32Signed => (2, 2, Signed),
        Format::Rgb48Signed => (3, 2, Signed),
        Format::Rgba64Signed => (4, 2, Signed),
        // The stored unit is one word / one two-byte pixel respectively;
        // both produce RGB, as `mode` reads from this channel count.
        Format::Rgb9e5Float => (1, 4, SharedExponent),
        Format::Yuy2 => (1, 2, Yuy2),
        _ => return None,
    };
    Some(Layout {
        channels,
        size,
        channel,
    })
}

/// Widen binary16 exactly to binary64, including subnormals, signed zero,
/// infinities and NaNs. All finite significands have at most eleven bits.
fn half(bits: u16) -> f64 {
    let exponent = (bits >> 10) & 31;
    let fraction = bits & 1023;
    let magnitude = match exponent {
        0 => f64::from(fraction) * 2.0f64.powi(-24),
        31 if fraction == 0 => f64::INFINITY,
        31 => f64::NAN,
        _ => f64::from(1024 + fraction) * 2.0f64.powi(i32::from(exponent) - 25),
    };
    if bits & 0x8000 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

/// PARITY section 17.1's shared conversion after a decoder yields binary16.
pub(crate) fn half_byte(bits: u16) -> u8 {
    float(half(bits))
}

/// Clamp, multiply and round half up in f64. Casting the bounded integral
/// result is exact; NaN maps explicitly to zero.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn float(value: f64) -> u8 {
    if value.is_nan() {
        0
    } else {
        (value.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
    }
}

fn channel(kind: Channel, bytes: &[u8]) -> u8 {
    match (kind, bytes) {
        (Channel::Half, &[lo, hi]) => float(half(u16::from_le_bytes([lo, hi]))),
        (Channel::Float, &[a, b, c, d]) => float(f64::from(f32::from_le_bytes([a, b, c, d]))),
        (Channel::Unsigned, &[byte] | &[_, byte]) => byte,
        // Adding the signed offset is precisely toggling the sign bit of
        // the high byte; no signed overflow or rounding is involved.
        (Channel::Signed, &[byte] | &[_, byte]) => byte ^ 128,
        _ => 0,
    }
}

fn clip(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(0)
}

fn yuv(luma: u8, u: u8, v: u8) -> [u8; 3] {
    let luma = i32::from(luma) - 16;
    let blue_chroma = i32::from(u) - 128;
    let red_chroma = i32::from(v) - 128;
    [
        clip((298 * luma + 409 * red_chroma + 128) >> 8),
        clip((298 * luma - 100 * blue_chroma - 208 * red_chroma + 128) >> 8),
        clip((298 * luma + 516 * blue_chroma + 128) >> 8),
    ]
}

fn shared(bytes: &[u8], out: &mut [u8]) {
    let &[a, b, c, d] = bytes else { return };
    let word = u32::from_le_bytes([a, b, c, d]);
    let scale = 2.0f64.powi(i32::try_from(word >> 27).unwrap_or(0) - 24);
    for (dst, shift) in out.iter_mut().zip([0, 9, 18]) {
        *dst = float(f64::from((word >> shift) & 511) * scale);
    }
}

/// Row-major stored pixels to the output's RGB(A) rows, flipped vertically.
/// YUY2's shared chroma stays within each even-width row.
pub(crate) fn decode(format: Format, data: &[u8], w: usize, h: usize, out: &mut [u8]) {
    let Some(layout) = layout(format) else { return };
    let stride = usize::try_from(layout.bytes()).unwrap_or(0);
    let bands = usize::try_from(format.mode().bytes_per_pixel()).unwrap_or(0);
    let row = w.saturating_mul(stride);
    let output_row = w.saturating_mul(bands);
    if row == 0 || output_row == 0 {
        return;
    }
    for (dst, src) in out
        .chunks_exact_mut(output_row)
        .zip(data.chunks_exact(row).take(h).rev())
    {
        if matches!(layout.channel, Channel::Yuy2) {
            for (pair, bytes) in dst
                .as_chunks_mut::<6>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0)
            {
                let &[y0, u, y1, v] = bytes;
                let [r0, g0, b0] = yuv(y0, u, v);
                let [r1, g1, b1] = yuv(y1, u, v);
                *pair = [r0, g0, b0, r1, g1, b1];
            }
        } else {
            for (pixel, bytes) in dst.chunks_exact_mut(bands).zip(src.chunks_exact(stride)) {
                if matches!(layout.channel, Channel::SharedExponent) {
                    shared(bytes, pixel);
                } else {
                    for (dst, bytes) in pixel
                        .iter_mut()
                        .zip(bytes.chunks_exact(usize::from(layout.size)))
                    {
                        *dst = channel(layout.channel, bytes);
                    }
                }
            }
        }
    }
}
