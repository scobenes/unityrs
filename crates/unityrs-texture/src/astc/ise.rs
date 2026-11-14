// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_integer_sequence.cpp:337-433,651-739, bounded integer sequence decoding. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
use super::{read_bits, tables};

/// Quantization index, in astcenc's `QUANT_2` through `QUANT_256` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Quant(u8);
const _: () = crate::send_sync::<Quant>();

// astcenc_integer_sequence.cpp:352-374 (bits, radix); radix 1 means bits only.
const BTQ: [(u8, u8); 21] = [
    (1, 1),
    (0, 3),
    (2, 1),
    (0, 5),
    (1, 3),
    (3, 1),
    (1, 5),
    (2, 3),
    (4, 1),
    (2, 5),
    (3, 3),
    (5, 1),
    (3, 5),
    (4, 3),
    (6, 1),
    (4, 5),
    (5, 3),
    (7, 1),
    (5, 5),
    (6, 3),
    (8, 1),
];

impl Quant {
    pub(super) fn new(index: u8) -> Option<Self> {
        (index < 21).then_some(Self(index))
    }

    pub(super) fn index(self) -> usize {
        usize::from(self.0)
    }

    pub(super) fn layout(self) -> (u8, u8) {
        BTQ.get(self.index()).copied().unwrap_or((0, 1))
    }

    pub(super) fn levels(self) -> u16 {
        let (bits, radix) = self.layout();
        u16::from(radix) << bits
    }

    /// Equivalent to astcenc_integer_sequence.cpp:394-433's scale/divisor
    /// table: ceil(8*n/5) trit bits or ceil(7*n/3) quint bits, plus low bits.
    pub(super) fn bit_count(self, count: usize) -> Option<usize> {
        let (bits, radix) = self.layout();
        let extra = match radix {
            3 => count.checked_mul(8)?.checked_add(4)? / 5,
            5 => count.checked_mul(7)?.checked_add(2)? / 3,
            _ => 0,
        };
        count.checked_mul(usize::from(bits))?.checked_add(extra)
    }

    /// Packed indices are scrambled, not in increasing weight order.
    /// astcenc_symbolic_physical.cpp:401-424.
    pub(super) fn unquant_weight(self, value: u8) -> Option<u8> {
        tables::WEIGHTS
            .get(self.index())?
            .get(usize::from(value))
            .copied()
    }

    /// astcenc_symbolic_physical.cpp:512-536; `QUANT_6` is the minimum
    /// permitted endpoint precision, even though ISE supports lower ranges.
    pub(super) fn unquant_color(self, value: u8) -> Option<u8> {
        tables::COLOR
            .get(self.index().checked_sub(4)?)?
            .get(usize::from(value))
            .copied()
    }
}

/// Decode at most 64 indices into caller-owned storage. Tails supply zero
/// for the missing high trit/quint code bits, as astcenc's zeroed `tq_blocks`
/// do (astcenc_integer_sequence.cpp:664-739). No temporary heap allocation.
pub(super) fn decode(quant: Quant, data: &[u8], offset: usize, out: &mut [u8]) -> Option<()> {
    if out.len() > 64
        || offset.checked_add(quant.bit_count(out.len())?)? > data.len().checked_mul(8)?
    {
        return None;
    }
    let (bits, radix) = quant.layout();
    let widths: &[usize] = match radix {
        3 => &[2, 2, 1, 2, 1],
        5 => &[3, 2, 2],
        _ => &[0],
    };
    let mut pos = offset;
    for group in out.chunks_mut(widths.len()) {
        let mut code = 0;
        let mut shift = 0;
        for (dst, &width) in group.iter_mut().zip(widths) {
            *dst = u8::try_from(read_bits(data, pos, usize::from(bits))?).ok()?;
            pos += usize::from(bits);
            code |= read_bits(data, pos, width)? << shift;
            pos += width;
            shift += width;
        }
        let digits: &[u8] = match radix {
            3 => tables::TRITS_OF_INTEGER.get(usize::try_from(code).ok()?)?,
            5 => tables::QUINTS_OF_INTEGER.get(usize::try_from(code).ok()?)?,
            _ => &[0],
        };
        if radix != 1 {
            for (dst, &digit) in group.iter_mut().zip(digits) {
                *dst |= digit << bits;
            }
        }
    }
    Some(())
}
