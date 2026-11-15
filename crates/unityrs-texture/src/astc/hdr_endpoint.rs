// Ported from Arm astcenc commit 631fb04 (Apache-2.0):
// Source/astcenc_color_unquantize.cpp:285-759,762-948, HDR endpoints and
// profile expansion; astcenc_internal.h:178-196, endpoint mode numbers.
// Copyright 2011-2024 Arm Limited. See THIRD_PARTY_LICENSES.

/// Decode endpoints for the full HDR profile. HDR lanes contain raw 16-bit
/// LNS values; linear LDR lanes contain replicated UNORM16 values.
pub(super) fn unpack(mode: u8, values: &[u8]) -> Option<([[u16; 4]; 2], [bool; 4])> {
    if mode > 15 || values.len() < usize::from((mode >> 2) + 1) * 2 {
        return None;
    }
    if !matches!(mode, 2 | 3 | 7 | 11 | 14 | 15) {
        return Some((super::endpoint::unpack_ldr(mode, values)?, [false; 4]));
    }
    let v = |i| values.get(i).copied().map(i32::from);
    let rgb = match mode {
        2 => {
            let (v0, v1) = (v(0)?, v(1)?);
            let (low, high) = if v1 >= v0 {
                (v0 << 4, v1 << 4)
            } else {
                ((v1 << 4) + 8, (v0 << 4) - 8)
            };
            [[low; 3], [high; 3]]
        }
        3 => {
            let (v0, v1) = (v(0)?, v(1)?);
            let (low, delta) = if v0 & 0x80 != 0 {
                (((v1 & 0xe0) << 4) | ((v0 & 0x7f) << 2), (v1 & 0x1f) << 2)
            } else {
                (((v1 & 0xf0) << 4) | ((v0 & 0x7f) << 1), (v1 & 0xf) << 1)
            };
            [[low; 3], [(low + delta).min(4095); 3]]
        }
        7 => rgb_offset([v(0)?, v(1)?, v(2)?, v(3)?])?,
        _ => rgb_direct([v(0)?, v(1)?, v(2)?, v(3)?, v(4)?, v(5)?])?,
    };
    let alpha = match mode {
        14 => [
            u16::try_from(v(6)?).ok()? * 257,
            u16::try_from(v(7)?).ok()? * 257,
        ],
        15 => hdr_alpha([v(6)?, v(7)?])?,
        _ => [0x7800; 2],
    };
    let mut out = [[0; 4]; 2];
    for (([r, g, b], a), pixel) in rgb.into_iter().zip(alpha).zip(&mut out) {
        *pixel = [lns(r)?, lns(g)?, lns(b)?, a];
    }
    Some((out, [true, true, true, mode != 14]))
}

fn lns(value: i32) -> Option<u16> {
    u16::try_from(value.clamp(0, 4095) << 4).ok()
}

fn rotate([r, g, b]: [i32; 3], major: i32) -> [i32; 3] {
    match major {
        1 => [g, r, b],
        2 => [b, g, r],
        _ => [r, g, b],
    }
}

/// astcenc_color_unquantize.cpp:285-433, RGB plus an unsigned offset.
fn rgb_offset([v0, v1, v2, v3]: [i32; 4]) -> Option<[[i32; 3]; 2]> {
    let selector = ((v0 & 0xc0) >> 6) | ((v1 & 0x80) >> 5) | ((v2 & 0x80) >> 4);
    let (major, mode) = if selector & 0xc != 0xc {
        (selector >> 2, selector & 3)
    } else if selector != 0xf {
        (selector & 3, 4)
    } else {
        (0, 5)
    };
    let [bit0, bit1, bit2, bit3, bit4, bit5, bit6] = [
        (v1 >> 6) & 1,
        (v1 >> 5) & 1,
        (v2 >> 6) & 1,
        (v2 >> 5) & 1,
        (v3 >> 7) & 1,
        (v3 >> 6) & 1,
        (v3 >> 5) & 1,
    ];
    let onehot = 1 << mode;
    let field = |bit: i32, shift: u32, mask: i32| {
        if onehot & mask != 0 { bit << shift } else { 0 }
    };
    let mut red = (v0 & 0x3f)
        | field(bit4, 6, 0x3b)
        | field(bit3, 6, 0x04)
        | field(bit5, 7, 0x10)
        | field(bit2, 7, 0x0f)
        | field(bit1, 8, 0x05)
        | field(bit0, 8, 0x0a)
        | field(bit0, 9, 0x05)
        | field(bit6, 9, 0x02)
        | field(bit3, 10, 0x01)
        | field(bit5, 10, 0x02);
    let mut green = (v1 & 0x1f) | field(bit0, 6, 0x30) | field(bit1, 5, 0x3a);
    let mut blue = (v2 & 0x1f) | field(bit2, 6, 0x30) | field(bit3, 5, 0x3a);
    let mut scale =
        (v3 & 0x1f) | field(bit6, 5, 0x3d) | field(bit5, 6, 0x2d) | field(bit4, 7, 0x04);
    let shift = [1, 1, 2, 3, 4, 5]
        .get(usize::try_from(mode).ok()?)
        .copied()?;
    red <<= shift;
    green <<= shift;
    blue <<= shift;
    scale <<= shift;
    if mode != 5 {
        green = red - green;
        blue = red - blue;
    }
    let high = rotate([red, green, blue], major);
    Some([high.map(|c| c - scale), high])
}

/// astcenc_color_unquantize.cpp:435-599, including its direct fallback.
fn rgb_direct([v0, v1, v2, v3, v4, v5]: [i32; 6]) -> Option<[[i32; 3]; 2]> {
    let mode = ((v1 & 0x80) >> 7) | ((v2 & 0x80) >> 6) | ((v3 & 0x80) >> 5);
    let major = ((v4 & 0x80) >> 7) | ((v5 & 0x80) >> 6);
    if major == 3 {
        return Some([
            [v0 << 4, v2 << 4, (v4 & 0x7f) << 5],
            [v1 << 4, v3 << 4, (v5 & 0x7f) << 5],
        ]);
    }
    let [bit0, bit1, bit2, bit3, bit4, bit5] = [
        (v2 >> 6) & 1,
        (v3 >> 6) & 1,
        (v4 >> 6) & 1,
        (v5 >> 6) & 1,
        (v4 >> 5) & 1,
        (v5 >> 5) & 1,
    ];
    let onehot = 1 << mode;
    let field = |bit: i32, shift: u32, mask: i32| {
        if onehot & mask != 0 { bit << shift } else { 0 }
    };
    let mut a = v0
        | ((v1 & 0x40) << 2)
        | field(bit0, 9, 0xa4)
        | field(bit2, 9, 0x08)
        | field(bit4, 9, 0x50)
        | field(bit5, 10, 0x50)
        | field(bit1, 10, 0xa0)
        | field(bit2, 11, 0xc0);
    let mut b0 = (v2 & 0x3f) | field(bit0, 6, 0x5b) | field(bit2, 7, 0x12);
    let mut b1 = (v3 & 0x3f) | field(bit1, 6, 0x5b) | field(bit3, 7, 0x12);
    let mut c = (v1 & 0x3f) | field(bit1, 6, 0x04) | field(bit3, 6, 0xe8) | field(bit2, 7, 0x20);
    let d0 = (v4 & 0x7f) | field(bit4, 5, 0xaf) | field(bit2, 6, 0x05);
    let d1 = (v5 & 0x7f) | field(bit5, 5, 0xaf) | field(bit3, 6, 0x05);
    let dbits = [7, 6, 7, 6, 5, 6, 5, 6]
        .get(usize::try_from(mode).ok()?)
        .copied()?;
    let sign = 1 << (dbits - 1);
    let extend = |d| (d & (sign - 1)) - (d & sign);
    let shift = (mode >> 1) ^ 3;
    a <<= shift;
    b0 <<= shift;
    b1 <<= shift;
    c <<= shift;
    let d0 = extend(d0) << shift;
    let d1 = extend(d1) << shift;
    Some([
        rotate([a - c, a - b0 - c - d0, a - b1 - c - d1], major),
        rotate([a, a - b0, a - b1], major),
    ])
}

/// astcenc_color_unquantize.cpp:689-738, all four alpha selectors.
fn hdr_alpha([v6, v7]: [i32; 2]) -> Option<[u16; 2]> {
    let selector = ((v6 >> 7) & 1) | ((v7 >> 6) & 2);
    let (mut base, mut delta) = (v6 & 0x7f, v7 & 0x7f);
    if selector == 3 {
        return Some([lns(base << 5)?, lns(delta << 5)?]);
    }
    base |= (delta << (selector + 1)) & 0x780;
    delta &= 0x3f >> selector;
    delta ^= 32 >> selector;
    delta -= 32 >> selector;
    base <<= 4 - selector;
    delta <<= 4 - selector;
    Some([lns(base)?, lns(base + delta)?])
}

#[cfg(test)]
#[path = "hdr_endpoint_tests.rs"]
mod tests;
