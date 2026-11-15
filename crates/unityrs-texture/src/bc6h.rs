//! BC6H signed and unsigned, implemented from Khronos Data Format Specification
//! 1.3 section 20.2 (Tables 114, 118, 119, 121, 122 and Equation 2):
//! <https://registry.khronos.org/DataFormat/specs/1.3/dataformat.1.3.html#BPTC>
//! The independent fixture oracle is bcdec; no third-party decoder is ported
//! here. Everything uses bounded stack arrays and checked slice access.
mod tables;

use crate::HdrError;

struct Mode {
    number: u32,
    transformed: bool,
    two: bool,
    bits: u32,
    delta: [u32; 3],
    // (endpoint scalar, low target bit, width); negative width reverses bits.
    fields: &'static [(usize, u32, i8)],
}

struct Bits {
    word: u128,
    position: u32,
}

impl Bits {
    fn read(&mut self, width: u32) -> Option<u32> {
        let value = self.word.checked_shr(self.position)? & ((1u128 << width) - 1);
        self.position = self.position.checked_add(width)?;
        u32::try_from(value).ok()
    }
}

fn sign_extend(value: i32, bits: u32) -> i32 {
    let sign = 1i32 << (bits - 1);
    (value ^ sign) - sign
}

fn unquantize(value: i32, bits: u32, signed: bool) -> i32 {
    if signed {
        if bits >= 16 {
            return value;
        }
        let magnitude = value.abs();
        let expanded = if magnitude == 0 {
            0
        } else if magnitude >= (1 << (bits - 1)) - 1 {
            0x7fff
        } else {
            ((magnitude << 15) + 0x4000) >> (bits - 1)
        };
        if value < 0 { -expanded } else { expanded }
    } else if bits >= 15 || value == 0 {
        value
    } else if value == (1 << bits) - 1 {
        0xffff
    } else {
        ((value << 15) + 0x4000) >> (bits - 1)
    }
}

fn finish(value: i32, signed: bool) -> Option<u16> {
    let bits = if signed {
        ((value.abs() * 31) >> 5) | if value < 0 { 0x8000 } else { 0 }
    } else {
        (value * 31) >> 6
    };
    u16::try_from(bits).ok()
}

/// Raw binary16 RGB values in stored raster order. Only reserved modes fail.
pub(crate) fn block(data: &[u8; 16], signed: bool) -> Option<[[u16; 3]; 16]> {
    let word = u128::from_le_bytes(*data);
    let prefix = u32::try_from(word & 31).ok()?;
    let number = if prefix & 3 < 2 { prefix & 3 } else { prefix };
    let mode = tables::MODES.iter().find(|m| m.number == number)?;
    let mut bits = Bits {
        word,
        position: if number < 2 { 2 } else { 5 },
    };
    let mut endpoints = [0i32; 12];
    for &(target, low, width) in mode.fields {
        let count = u32::from(width.unsigned_abs());
        let mut value = bits.read(count)?;
        if width < 0 {
            value = value.reverse_bits() >> (32 - count);
        }
        *endpoints.get_mut(target)? |= i32::try_from(value << low).ok()?;
    }
    let count = if mode.two { 12 } else { 6 };
    for c in 0..3 {
        let base = *endpoints.get(c)?;
        for e in 0..count / 3 {
            let value = endpoints.get_mut(e * 3 + c)?;
            if e != 0 && mode.transformed {
                *value = (base + sign_extend(*value, *mode.delta.get(c)?)) & ((1 << mode.bits) - 1);
            }
            if signed {
                *value = sign_extend(*value, mode.bits);
            }
            *value = unquantize(*value, mode.bits, signed);
        }
    }
    let partition = if mode.two {
        usize::try_from(bits.read(5)?).ok()?
    } else {
        0
    };
    let anchor = if mode.two {
        *tables::ANCHORS.get(partition)?
    } else {
        0
    };
    let mask = if mode.two {
        *tables::PARTITIONS.get(partition)?
    } else {
        0
    };
    let weights: &[i32] = if mode.two {
        &[0, 9, 18, 27, 37, 46, 55, 64]
    } else {
        &[0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64]
    };
    let mut result = [[0; 3]; 16];
    for (texel, channels) in result.iter_mut().enumerate() {
        let width = if mode.two { 3 } else { 4 } - u32::from(texel == 0 || texel == anchor);
        let index = usize::try_from(bits.read(width)?).ok()?;
        let weight = *weights.get(index)?;
        let subset = usize::from((mask >> texel) & 1);
        for (c, out) in channels.iter_mut().enumerate() {
            let low = *endpoints.get(subset * 6 + c)?;
            let high = *endpoints.get(subset * 6 + 3 + c)?;
            *out = finish(((64 - weight) * low + weight * high + 32) >> 6, signed)?;
        }
    }
    Some(result)
}

/// Unity's unsigned BC6H form, cropped to dimensions and vertically flipped.
/// Planning has already charged and allocated the exact RGB output.
pub(crate) fn decode(data: &[u8], w: usize, h: usize, out: &mut [u8]) -> Result<(), HdrError> {
    decode_image(data, w, h, out, false)
}

/// Explicit signed BC6H image path; Unity's `TextureFormat` does not distinguish it.
pub(crate) fn decode_signed(
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
) -> Result<(), HdrError> {
    decode_image(data, w, h, out, true)
}

fn decode_image(
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
    signed: bool,
) -> Result<(), HdrError> {
    if w == 0 || h == 0 {
        return Ok(());
    }
    let invalid = HdrError::InvalidBlock;
    let length = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(3))
        .ok_or(invalid)?;
    let out = out.get_mut(..length).ok_or(invalid)?;
    let across = w.div_ceil(4);
    let needed = across
        .checked_mul(h.div_ceil(4))
        .and_then(|n| n.checked_mul(16))
        .ok_or(invalid)?;
    let data = data.get(..needed).ok_or(invalid)?;
    for (i, bytes) in data.as_chunks::<16>().0.iter().enumerate() {
        let pixels = block(bytes, signed).ok_or(HdrError::ReservedMode)?;
        let left = (i % across) * 4;
        let top = (i / across) * 4;
        for (texel, pixel) in pixels.iter().enumerate() {
            let x = left + texel % 4;
            let y = top + texel / 4;
            if x >= w || y >= h {
                continue;
            }
            let offset = ((h - 1 - y) * w + x) * 3;
            for (dest, source) in out
                .get_mut(offset..offset + 3)
                .ok_or(invalid)?
                .iter_mut()
                .zip(pixel)
            {
                *dest = crate::tierb::half_byte(*source);
            }
        }
    }
    Ok(())
}
