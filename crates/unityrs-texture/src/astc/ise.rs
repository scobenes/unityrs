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

    #[cfg(test)]
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

/// The highest endpoint precision, `QUANT_6` (4) to `QUANT_256` (20),
/// whose `count` integers (at most 18) fit `available` bits (at most 111):
/// astcenc's `quant_mode_table` (astcenc_quantization.cpp:800-903), which
/// [`FITTING`] holds as [`Quant::bit_count`]'s search finds it.
pub(super) fn highest_fitting(count: usize, available: usize) -> Option<Quant> {
    match FITTING.get(count).and_then(|row| row.get(available)) {
        Some(&index) => Quant::new(index),
        None => fitting_search(count, available),
    }
}

/// [`highest_fitting`] by search: the highest precision whose bits fit.
fn fitting_search(count: usize, available: usize) -> Option<Quant> {
    (4..=20)
        .rev()
        .filter_map(Quant::new)
        .find(|q| q.bit_count(count).is_some_and(|n| n <= available))
}

/// [`fitting_search`] for every count to 18 and every available bit count
/// to 111, built at compile time; an index past 20 where none fits.
#[allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    reason = "evaluated at compile time, where `From` and `get` are not callable; q is at most 20"
)]
const FITTING: [[u8; 112]; 19] = {
    let mut table = [[u8::MAX; 112]; 19];
    let mut count: usize = 0;
    while count < 19 {
        let mut available = 0;
        while available < 112 {
            let mut q = 20;
            while q >= 4 {
                let (bits, radix) = BTQ[q];
                let extra = match radix {
                    3 => (count * 8).div_ceil(5),
                    5 => (count * 7).div_ceil(3),
                    _ => 0,
                };
                if count * bits as usize + extra <= available {
                    table[count][available] = q as u8;
                    break;
                }
                q -= 1;
            }
            available += 1;
        }
        count += 1;
    }
    table
};

/// Decode at most 64 indices into caller-owned storage. Tails supply zero
/// for the missing high trit/quint code bits, as astcenc's zeroed `tq_blocks`
/// do (astcenc_integer_sequence.cpp:664-739). No temporary heap allocation.
pub(super) fn decode(quant: Quant, data: &[u8], offset: usize, out: &mut [u8]) -> Option<()> {
    if out.len() > 64
        || offset.checked_add(quant.bit_count(out.len())?)? > data.len().checked_mul(8)?
    {
        return None;
    }
    // A block's 16 bytes (or fewer) are read as one integer. The check
    // above bounds every read: a sequence's fields take exactly
    // `bit_count` bits, its last group's too.
    let word = match <[u8; 16]>::try_from(data) {
        Ok(block) => block,
        Err(_) if data.len() < 16 => {
            let mut word = [0; 16];
            word.get_mut(..data.len())?.copy_from_slice(data);
            word
        }
        Err(_) => return decode_with(quant, offset, out, |pos, count| read_bits(data, pos, count)),
    };
    decode_word(quant, u128::from_le_bytes(word), offset, out);
    Some(())
}

/// Each trit group's fields after its values, `(width, shift)` into the
/// 8-bit code; each quint group's into the 7-bit code
/// (astcenc_integer_sequence.cpp:664-739).
const TRIT_FIELDS: [(u32, u32); 5] = [(2, 0), (2, 2), (1, 4), (2, 5), (1, 7)];
const QUINT_FIELDS: [(u32, u32); 3] = [(3, 0), (2, 3), (2, 5)];

/// [`decode`] from `word`, the data as a little-endian integer, whose bits
/// from `offset` hold the whole sequence. Each group is read from the 64
/// bits at its start: at most 38 bits a trit group (five values of 6
/// bits and 8 code bits), 22 a quint group and 64 eight plain values.
fn decode_word(quant: Quant, word: u128, offset: usize, out: &mut [u8]) {
    let (bits, radix) = quant.layout();
    let bits = u32::from(bits);
    match radix {
        3 => groups(word, offset, bits, out, &TRIT_FIELDS, |code| {
            tables::TRITS_OF_INTEGER.get(code & 0xff).unwrap_or(&[0; 5])
        }),
        5 => groups(word, offset, bits, out, &QUINT_FIELDS, |code| {
            tables::QUINTS_OF_INTEGER
                .get(code & 0x7f)
                .unwrap_or(&[0; 3])
        }),
        _ => groups(word, offset, bits, out, &[(0, 0); 8], |_| &[0; 8]),
    }
}

/// The groups of `N` values from `word`'s bit `offset`: each value's
/// `bits` low bits, each followed by its field of the group's code,
/// `fields`' `(width, shift)`; `digits` maps the code to the values' high
/// digits. A short last group reads only its own values' fields, and the
/// code's missing bits are 0.
#[inline(always)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::inline_always,
    reason = "each field is masked to at most 8 bits; measured: each radix's loop"
)]
fn groups<'a, const N: usize>(
    word: u128,
    offset: usize,
    bits: u32,
    out: &mut [u8],
    fields: &[(u32, u32); N],
    digits: impl Fn(usize) -> &'a [u8; N],
) {
    let mask = (1_u64 << bits) - 1;
    let mut rest = u32::try_from(offset)
        .ok()
        .and_then(|p| word.checked_shr(p))
        .unwrap_or(0);
    let mut group = |values: &mut [u8]| {
        let mut g = rest as u64;
        let (mut code, mut used) = (0, 0);
        for (dst, &(width, shift)) in values.iter_mut().zip(fields) {
            *dst = (g & mask) as u8;
            g >>= bits;
            code |= ((g & ((1 << width) - 1)) as usize) << shift;
            g >>= width;
            used += bits + width;
        }
        // Plain values have no digits (all 0) and up to 8 bits.
        for (dst, &digit) in values.iter_mut().zip(digits(code)) {
            *dst |= digit.checked_shl(bits).unwrap_or(0);
        }
        rest = rest.checked_shr(used).unwrap_or(0);
    };
    let (chunks, tail) = out.as_chunks_mut::<N>();
    for values in chunks {
        group(values);
    }
    if !tail.is_empty() {
        group(tail);
    }
}

/// [`decode`]'s groups, each field read by `read(offset, count)`: for data
/// longer than a block, and in the tests the reference [`decode_word`]
/// is held to.
pub(super) fn decode_with(
    quant: Quant,
    offset: usize,
    out: &mut [u8],
    read: impl Fn(usize, usize) -> Option<u32>,
) -> Option<()> {
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
            *dst = u8::try_from(read(pos, usize::from(bits))?).ok()?;
            pos += usize::from(bits);
            code |= read(pos, width)? << shift;
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
