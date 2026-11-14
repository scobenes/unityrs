// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_symbolic_physical.cpp:426-536, endpoint mode headers and precision selection; astcenc_color_unquantize.cpp:35-283,762-948, LDR endpoints. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
use super::{
    block_mode::WeightMode,
    ise::{self, Quant},
    read_bits,
};

/// The colour header, without decoding weights, infill or a plane selector.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub(super) modes: [u8; 4],
    pub(super) partitions: u8,
    pub(super) seed: u16,
    pub(super) quant: Quant,
    pub(super) value_count: usize,
    pub(super) value_offset: usize,
    pub(super) below_weights: usize,
}
const _: () = crate::send_sync::<Layout>();

/// Physical CEM encoding: single mode, shared mode, or classes differing
/// by at most one with their low bits at the two ends of the colour data.
/// astcenc_symbolic_physical.cpp:426-517. None means an error-colour block.
pub(super) fn decode_layout(data: &[u8; 16], weights: WeightMode) -> Option<Layout> {
    let partitions = u8::try_from(read_bits(data, 11, 2)? + 1).ok()?;
    if weights.dual_plane && partitions == 4 {
        return None;
    }
    let mut below_weights = 128_usize.checked_sub(usize::from(weights.bits))?;
    let mut modes = [0; 4];
    let mut high_size = 0;
    let seed = if partitions == 1 {
        *modes.first_mut()? = u8::try_from(read_bits(data, 13, 4)?).ok()?;
        0
    } else {
        high_size = 3 * usize::from(partitions) - 4;
        below_weights = below_weights.checked_sub(high_size)?;
        let encoded = read_bits(data, 23, 6)? | (read_bits(data, below_weights, high_size)? << 6);
        let class = encoded & 3;
        if class == 0 {
            modes.fill(u8::try_from((encoded >> 2) & 15).ok()?);
            below_weights += high_size;
            high_size = 0;
        } else {
            for (i, mode) in modes.iter_mut().take(usize::from(partitions)).enumerate() {
                let endpoint_class = ((encoded >> (2 + i)) & 1) + class - 1;
                let low = (encoded >> (2 + usize::from(partitions) + 2 * i)) & 3;
                *mode = u8::try_from((endpoint_class << 2) | low).ok()?;
            }
        }
        u16::try_from(read_bits(data, 13, 10)?).ok()?
    };
    let value_count = modes
        .iter()
        .take(usize::from(partitions))
        .map(|mode| usize::from((mode >> 2) + 1) * 2)
        .sum();
    if value_count > 18 {
        return None;
    }
    let available = if partitions == 1 { 111_usize } else { 99_usize };
    let available = available.saturating_sub(
        usize::from(weights.bits) + high_size + if weights.dual_plane { 2 } else { 0 },
    );
    // quant_mode_table (astcenc_quantization.cpp:800-903) selects the
    // highest precision fitting the available bits; compute that small
    // search rather than retaining the encoder's 1280-byte table.
    let quant = (4..=20)
        .rev()
        .filter_map(Quant::new)
        .find(|q| q.bit_count(value_count).is_some_and(|n| n <= available))?;
    Some(Layout {
        modes,
        partitions,
        seed,
        quant,
        value_count,
        value_offset: if partitions == 1 { 17 } else { 29 },
        below_weights,
    })
}

/// ISE decode and unscramble the endpoint integers. The returned values
/// are 8-bit endpoints, before the mode's delta/scale/blue transforms.
/// astcenc_symbolic_physical.cpp:519-536.
pub(super) fn decode_values(data: &[u8; 16], layout: &Layout) -> Option<[[u8; 8]; 4]> {
    let mut packed = [0; 18];
    ise::decode(
        layout.quant,
        data,
        layout.value_offset,
        packed.get_mut(..layout.value_count)?,
    )?;
    let mut result = [[0; 8]; 4];
    let mut offset = 0;
    for (&mode, values) in layout
        .modes
        .iter()
        .zip(&mut result)
        .take(usize::from(layout.partitions))
    {
        let count = usize::from((mode >> 2) + 1) * 2;
        for value in values.iter_mut().take(count) {
            *value = layout.quant.unquant_color(*packed.get(offset)?)?;
            offset += 1;
        }
    }
    Some(result)
}

/// astcenc_vecmathlib_common_4.h:354-363, called with (delta, base).
fn transfer(base: i32, delta: i32) -> (i32, i32) {
    let base = (base >> 1) | (delta & 0x80);
    let delta = (delta >> 1) & 0x3f;
    (
        base,
        if delta & 0x20 != 0 {
            delta - 0x40
        } else {
            delta
        },
    )
}

/// astcenc_color_unquantize.cpp:35-41: signed arithmetic shift, including
/// values outside 0..255 in the delta path; clamp happens after contraction.
fn unblue([r, g, b, a]: [i32; 4]) -> [i32; 4] {
    [(r + b) >> 1, (g + b) >> 1, b, a]
}

/// Unpack a mode's unquantized values for astcenc's linear LDR profile.
/// Returns replicated 16-bit endpoints, ready for T2b's interpolation.
/// HDR modes in an LDR profile give magenta (not a structural parse error),
/// astcenc_color_unquantize.cpp:900-913. No HDR decode or pixel store here.
pub(super) fn unpack_ldr(mode: u8, input: &[u8]) -> Option<[[u16; 4]; 2]> {
    if mode > 15 || input.len() < usize::from((mode >> 2) + 1) * 2 {
        return None;
    }
    let v = |i| input.get(i).copied().map(i32::from);
    let (mut a, mut b) = match mode {
        // Luminance and luminance delta, :183-217.
        0 | 1 => {
            let (l0, l1) = if mode == 0 {
                (v(0)?, v(1)?)
            } else {
                let l0 = (v(0)? >> 2) | (v(1)? & 0xc0);
                (l0, (l0 + (v(1)? & 0x3f)).min(255))
            };
            ([l0, l0, l0, 255], [l1, l1, l1, 255])
        }
        // LA and signed LA delta, :226-283.
        4 | 5 => {
            let (l0, l1, a0, a1) = if mode == 4 {
                (v(0)?, v(1)?, v(2)?, v(3)?)
            } else {
                let (l, dl) = transfer(v(0)?, v(1)?);
                let (a, da) = transfer(v(2)?, v(3)?);
                (l, (l + dl).clamp(0, 255), a, (a + da).clamp(0, 255))
            };
            ([l0, l0, l0, a0], [l1, l1, l1, a1])
        }
        // RGB scale (and direct alpha), :137-172.
        6 | 10 => {
            let [red, green, blue, scale] = [v(0)?, v(1)?, v(2)?, v(3)?];
            let (a0, a1) = if mode == 10 {
                (v(4)?, v(5)?)
            } else {
                (255, 255)
            };
            (
                [
                    (red * scale) >> 8,
                    (green * scale) >> 8,
                    (blue * scale) >> 8,
                    a0,
                ],
                [red, green, blue, a1],
            )
        }
        // RGB(A), direct or delta, :43-124.
        8 | 9 | 12 | 13 => {
            let mut a = [v(0)?, v(2)?, v(4)?, 0];
            let mut b = [v(1)?, v(3)?, v(5)?, 0];
            if mode >= 12 {
                a = [v(0)?, v(2)?, v(4)?, v(6)?];
                b = [v(1)?, v(3)?, v(5)?, v(7)?];
            }
            let swap = if mode & 1 != 0 {
                for (base, delta) in a.iter_mut().zip(&mut b) {
                    (*base, *delta) = transfer(*base, *delta);
                }
                let swap = b.iter().take(3).sum::<i32>() < 0;
                for (base, delta) in a.iter().zip(&mut b) {
                    *delta += base;
                }
                swap
            } else {
                a.iter().take(3).sum::<i32>() > b.iter().take(3).sum::<i32>()
            };
            if swap { (unblue(b), unblue(a)) } else { (a, b) }
        }
        // 2,3,7,11,14,15 are HDR endpoints, invalid in the LDR profile.
        _ => ([255, 0, 255, 255], [255, 0, 255, 255]),
    };
    if mode == 8 || mode == 9 {
        if let Some(alpha) = a.last_mut() {
            *alpha = 255;
        }
        if let Some(alpha) = b.last_mut() {
            *alpha = 255;
        }
    }
    let mut out = [[0; 4]; 2];
    for (src, dst) in [a, b].iter().zip(&mut out) {
        for (&value, channel) in src.iter().zip(dst) {
            *channel = u16::try_from(value.clamp(0, 255)).ok()? * 257;
        }
    }
    Some(out)
}
