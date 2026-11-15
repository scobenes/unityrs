// Ported from Crunch legacy decoder in texture2ddecoder 1.0.5 (LicenseRef-Crunch): legacy CRN unpacking. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/crunch/crn_decomp.h:2205-2380, crn_decomp.h:3041-3325.
// Altered source: checked canonical codes, zero-extended reads without speculative memory access.
// Crunch Library Copyright (c) 2010-2016 Richard Geldreich, Jr.
// See LICENSE-CRUNCH and THIRD_PARTY_LICENSES.

use super::{CrunchError as Error, Result};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// MSB-first bits. Like `symbol_codec::get_bits` and `decode` in the
/// texture2ddecoder 1.0.5 sdist's crn_decomp.h:3210-3277, reads past the
/// region supply zero bits indefinitely. `start_decoding` still requires
/// a nonempty region (crn_decomp.h:3166-3180).
pub(super) struct Bits<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u8,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.is_empty() {
            return Err(Error::Truncated);
        }
        Ok(Self {
            data,
            byte: 0,
            bit: 0,
        })
    }

    pub fn read(&mut self, n: u8) -> Result<u32> {
        if n > 32 {
            return Err(Error::Huffman);
        }
        let mut value = 0;
        for _ in 0..n {
            let byte = self.data.get(self.byte).copied().unwrap_or(0);
            value = (value << 1) | u32::from((byte >> (7 - self.bit)) & 1);
            self.bit += 1;
            if self.bit == 8 {
                self.byte = self.byte.saturating_add(1).min(self.data.len());
                self.bit = 0;
            }
        }
        Ok(value)
    }
}

/// Canonical codes sorted by (length, symbol), as the reference decoder's
/// `sorted_symbol_order` (crn_decomp.h:2205-2380). No binary tree or 2^16
/// lookup allocation: at most sixteen steps per symbol.
pub(super) struct Model {
    symbols: MeteredVec<u16>,
    counts: [u32; 17],
    first: [u32; 17],
    starts: [usize; 17],
}

impl Model {
    // receive reads total_bits(8192) = 14 bits and does not enforce 8192
    // (crn_decomp.h:3085-3100). Match the full representable alphabet.
    pub const MAX_SYMBOLS: usize = 16383;

    pub fn new(lengths: &[u8], meter: &Arc<Meter>) -> Result<Self> {
        if lengths.is_empty() || lengths.len() > Self::MAX_SYMBOLS {
            return Err(Error::Huffman);
        }
        // Two length-byte scans plus writing at most one u16 per symbol.
        meter.count_decode(4 * lengths.len() as u64)?;
        let mut counts = [0u32; 17];
        for &len in lengths {
            if len > 16 {
                return Err(Error::Huffman);
            }
            if len > 0 {
                *counts.get_mut(usize::from(len)).ok_or(Error::Huffman)? += 1;
            }
        }
        let mut first = [0u32; 17];
        let mut starts = [0usize; 17];
        let (mut code, mut total, mut previous) = (0, 0, 0);
        for ((len, &count), (first, start)) in counts
            .iter()
            .enumerate()
            .skip(1)
            .zip(first.iter_mut().skip(1).zip(starts.iter_mut().skip(1)))
        {
            code = (code + previous) << 1;
            if code + count > (1 << len) {
                return Err(Error::Huffman);
            }
            *first = code;
            *start = total;
            total += usize::try_from(count).map_err(|_| Error::Huffman)?;
            previous = count;
        }
        if total == 0 {
            return Err(Error::Huffman);
        }
        let mut symbols = MeteredVec::with_capacity(meter, total)?;
        for _ in 0..total {
            symbols.push(0)?;
        }
        let mut positions = starts;
        for (symbol, &len) in lengths.iter().enumerate().filter(|(_, len)| **len != 0) {
            let at = positions.get_mut(usize::from(len)).ok_or(Error::Huffman)?;
            *symbols.as_mut_slice().get_mut(*at).ok_or(Error::Huffman)? =
                u16::try_from(symbol).map_err(|_| Error::Huffman)?;
            *at += 1;
        }
        Ok(Self {
            symbols,
            counts,
            first,
            starts,
        })
    }

    pub fn decode(&self, bits: &mut Bits<'_>) -> Result<u32> {
        let position = (bits.byte, bits.bit);
        let mut code = 0;
        for ((&count, &first), &start) in self
            .counts
            .iter()
            .skip(1)
            .zip(self.first.iter().skip(1))
            .zip(self.starts.iter().skip(1))
        {
            code = (code << 1) | bits.read(1)?;
            if code >= first && code - first < count {
                let index = start + usize::try_from(code - first).map_err(|_| Error::Huffman)?;
                return self
                    .symbols
                    .as_slice()
                    .get(index)
                    .copied()
                    .map(u32::from)
                    .ok_or(Error::Huffman);
            }
        }
        // The reference's slow length search reaches its sentinel for an
        // unassigned codeword and returns 0 before consuming bits
        // (texture2ddecoder 1.0.5 sdist crn_decomp.h:3252-3289).
        // Callers bound their loops by alphabet, palette or chunk counts;
        // none retries until this bit position advances.
        (bits.byte, bits.bit) = position;
        Ok(0)
    }

    /// Nested Huffman code lengths and four run symbols,
    /// crn_decomp.h:3041-3161. Counts come from a 14-bit field but the
    /// reference accepts every nonzero value of that field.
    pub fn receive(bits: &mut Bits<'_>, meter: &Arc<Meter>) -> Result<Self> {
        const ORDER: [usize; 21] = [
            17, 18, 19, 20, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15, 16,
        ];
        let total = usize::try_from(bits.read(14)?).map_err(|_| Error::Huffman)?;
        if total == 0 || total > Self::MAX_SYMBOLS {
            return Err(Error::Huffman);
        }
        // Zero extension can expand a tiny region into a full alphabet.
        // Charge its length expansion before the count-bounded loop.
        meter.count_decode(total as u64)?;
        let n = usize::try_from(bits.read(5)?).map_err(|_| Error::Huffman)?;
        if n == 0 || n > ORDER.len() {
            return Err(Error::Huffman);
        }
        let mut small_lengths = [0u8; 21];
        for &slot in ORDER.iter().take(n) {
            *small_lengths.get_mut(slot).ok_or(Error::Huffman)? =
                u8::try_from(bits.read(3)?).map_err(|_| Error::Huffman)?;
        }
        let small = Self::new(&small_lengths, meter)?;
        let mut lengths = MeteredVec::with_capacity(meter, total)?;
        let mut previous = 0;
        while lengths.len() < total {
            let symbol = small.decode(bits)?;
            let (len, count) = match symbol {
                0..=16 => (u8::try_from(symbol).map_err(|_| Error::Huffman)?, 1),
                17 => (0, bits.read(3)? + 3),
                18 => (0, bits.read(7)? + 11),
                19 | 20 if previous != 0 && !lengths.is_empty() => (
                    previous,
                    if symbol == 19 {
                        bits.read(2)? + 3
                    } else {
                        bits.read(6)? + 7
                    },
                ),
                _ => return Err(Error::Huffman),
            };
            let count = usize::try_from(count).map_err(|_| Error::Huffman)?;
            if count > total - lengths.len() {
                return Err(Error::Huffman);
            }
            for _ in 0..count {
                lengths.push(len)?;
            }
            previous = len;
        }
        Self::new(lengths.as_slice(), meter)
    }
}
