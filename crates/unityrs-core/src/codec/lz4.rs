// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
// Ported from liblz4 1.9.4 (BSD-2-Clause): lib/lz4.c, LZ4_decompress_generic. See THIRD_PARTY_LICENSES.
//
// The decoder below translates liblz4's. liblz4 1.9.4's notice, from lib/lz4.c:
//
// LZ4 - Fast LZ compression algorithm
// Copyright (C) 2011-2020, Yann Collet.
//
// BSD 2-Clause License (http://www.opensource.org/licenses/bsd-license.php)
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//     * Redistributions of source code must retain the above copyright
// notice, this list of conditions and the following disclaimer.
//     * Redistributions in binary form must reproduce the above
// copyright notice, this list of conditions and the following disclaimer
// in the documentation and/or other materials provided with the
// distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

//! Compression types 2 and 3: [`Lz4`], with liblz4 1.9.4's acceptance.

use bytes::Bytes;

use super::{BlockCodec, Codec, Decoded};
use crate::budget::DecodeBuf;
use crate::error::Error;

/// Compression types 2 and 3: an LZ4 block, `uncompressed` as the maximum.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lz4;

const _: () = crate::send_sync::<Lz4>();

impl BlockCodec for Lz4 {
    /// `lz4.block.decompress(data, uncompressed_size)` (python-lz4 4.4.4
    /// over liblz4 1.9.4's `LZ4_decompress_safe`): a declared size above
    /// `i32::MAX` is python-lz4's `OverflowError`; otherwise the block is
    /// scanned with liblz4's exact control flow (`lz4_run`), which gives
    /// its verdict and the output's length, and only then is the output
    /// allocated (that length, charged) and decoded. python-lz4 reserves
    /// the whole declared size before it decodes, so where the machine
    /// cannot reserve it UnityPy raises `MemoryError` and unityrs reads on
    /// (`PARITY.md` section 19.3, "a count UnityPy allocates before it
    /// reads").
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        let fail = |detail: String| Error::Decompress {
            codec: Codec::Lz4,
            detail: detail.into(),
        };
        let Ok(capacity) = i32::try_from(uncompressed) else {
            return Err(fail(
                "OverflowError: signed integer is greater than maximum".to_owned(),
            ));
        };
        let capacity = usize::try_from(capacity).unwrap_or(0);
        let lz4_error = |at: usize| {
            fail(format!(
                "LZ4BlockError: Decompression failed: corrupt input or insufficient space in \
                 destination buffer. Error code: {}",
                at.saturating_add(1)
            ))
        };
        let n = lz4_run(input, capacity, None).map_err(lz4_error)?;
        let dst = out.grow_zeroed(n)?;
        lz4_run(input, capacity, Some(dst)).map_err(lz4_error)?;
        Ok(Decoded::Written)
    }
}

// liblz4 1.9.4 (lz4.c) constants.
const MINMATCH: usize = 4;
const LASTLITERALS: usize = 5;
const MFLIMIT: usize = 12;
const FASTLOOP_SAFE_DISTANCE: usize = 64;
const ML_MASK: usize = 15;
const RUN_MASK: usize = 15;

/// `read_variable_length` (lz4.c 1.9.4): the bytes at `*ip` summed while each
/// is 255; an error (`rvl_error`) once `*ip` passes `limit`, or, with
/// `initial_check`, when it starts at or past it. `limit` is signed: a
/// limit below the input's start fails the first check.
fn read_variable_length(
    src: &[u8],
    ip: &mut usize,
    limit: isize,
    initial_check: bool,
) -> Option<usize> {
    let past = |ip: usize| isize::try_from(ip).map_or(true, |i| i > limit);
    if initial_check && isize::try_from(*ip).map_or(true, |i| i >= limit) {
        return None;
    }
    let mut length = 0usize;
    loop {
        let s = *src.get(*ip)?;
        *ip += 1;
        length = length.saturating_add(usize::from(s));
        if past(*ip) {
            return None;
        }
        if s != 255 {
            return Some(length);
        }
    }
}

/// Copies a match of `length` bytes at `offset` behind `op` within `dst`,
/// byte by byte in LZ77 order; offset 0 writes zeros, as liblz4 does (it
/// zeroes four bytes, then copies them onto themselves).
fn copy_match(dst: &mut [u8], op: usize, offset: usize, length: usize) {
    let end = op.saturating_add(length).min(dst.len());
    if offset == 0 {
        if let Some(d) = dst.get_mut(op..end) {
            d.fill(0);
        }
        return;
    }
    let Some(src) = op.checked_sub(offset) else {
        return;
    };
    let mut at = op;
    while at < end {
        // Each pass copies what is already final: up to `offset` bytes, then
        // twice that, as the copied run lengthens.
        let step = (at - src).min(end - at);
        dst.copy_within(src..src + step, at);
        at += step;
    }
}

/// The LZ4 decoder's state: liblz4's `ip` and `op` as offsets, and the
/// output when this pass writes one.
struct Lz4State<'a, 'o> {
    src: &'a [u8],
    dst: Option<&'o mut [u8]>,
    ip: usize,
    op: usize,
    /// `iend` and `oend`, signed as liblz4's pointer comparisons are.
    iend: isize,
    oend: isize,
}

/// Where the safe loop is entered.
enum Lz4Entry {
    /// The loop's top.
    Top,
    /// `safe_literal_copy`, with the token and the literal length.
    Literals(usize, usize),
    /// `safe_match_copy`, with the offset and the length (MINMATCH added).
    Match(usize, usize),
}

fn signed(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

impl Lz4State<'_, '_> {
    fn token(&mut self) -> Result<usize, usize> {
        let t = self.src.get(self.ip).copied().ok_or(self.ip)?;
        self.ip += 1;
        Ok(usize::from(t))
    }

    fn le16(&mut self) -> Result<usize, usize> {
        let b = self
            .src
            .get(self.ip..self.ip.saturating_add(2))
            .and_then(|b| <[u8; 2]>::try_from(b).ok())
            .ok_or(self.ip)?;
        self.ip += 2;
        Ok(usize::from(u16::from_le_bytes(b)))
    }

    fn varlen(&mut self, limit: isize, initial_check: bool) -> Result<usize, usize> {
        read_variable_length(self.src, &mut self.ip, limit, initial_check).ok_or(self.ip)
    }

    /// Copies `n` literals and advances both positions.
    fn literals(&mut self, n: usize) {
        if let (Some(d), Some(from)) = (
            self.dst.as_deref_mut(),
            self.src.get(self.ip..self.ip.saturating_add(n)),
        ) && let Some(to) = d.get_mut(self.op..self.op.saturating_add(n))
        {
            to.copy_from_slice(from);
        }
        self.ip = self.ip.saturating_add(n);
        self.op = self.op.saturating_add(n);
    }

    /// Copies a match and advances the output.
    fn copy(&mut self, offset: usize, n: usize) {
        if let Some(d) = self.dst.as_deref_mut() {
            copy_match(d, self.op, offset, n);
        }
        self.op = self.op.saturating_add(n);
    }

    /// The match length at `_copy_match` from the token's `mlen`, MINMATCH
    /// added.
    fn match_length(&mut self, mlen: usize) -> Result<usize, usize> {
        let mut length = mlen;
        if length == ML_MASK {
            length += self.varlen(self.iend - signed(LASTLITERALS) + 1, false)?;
        }
        Ok(length + MINMATCH)
    }

    /// From the safe loop's literal copy (`cpy = op + length`, then
    /// `safe_literal_copy`) through the match's offset and length; `None`
    /// when this was the last sequence.
    fn safe_literals(
        &mut self,
        token: usize,
        length: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        let cpy = signed(self.op).saturating_add(signed(length));
        let lit_end = signed(self.ip).saturating_add(signed(length));
        if cpy > self.oend - signed(MFLIMIT) || lit_end > self.iend - signed(2 + 1 + LASTLITERALS) {
            // The last sequence: it must end the input exactly and fit.
            if lit_end != self.iend || cpy > self.oend {
                return Err(self.ip);
            }
            self.literals(length);
            return Ok(None);
        }
        self.literals(length);
        let offset = self.le16()?;
        let length = self.match_length(token & ML_MASK)?;
        Ok(Some((offset, length)))
    }

    /// The fast loop, run while 64 bytes of output remain; where the safe
    /// loop takes over.
    fn fast_loop(&mut self) -> Result<Lz4Entry, usize> {
        loop {
            let token = self.token()?;
            let mut length = token >> 4;
            if length == RUN_MASK {
                length += self.varlen(self.iend - signed(RUN_MASK), true)?;
                if signed(self.op).saturating_add(signed(length)) > self.oend - 32
                    || signed(self.ip).saturating_add(signed(length)) > self.iend - 32
                {
                    return Ok(Lz4Entry::Literals(token, length));
                }
            } else if signed(self.ip) > self.iend - (16 + 1) {
                return Ok(Lz4Entry::Literals(token, length));
            }
            self.literals(length);
            let offset = self.le16()?;
            let mut length = token & ML_MASK;
            if length == ML_MASK {
                length += self.varlen(self.iend - signed(LASTLITERALS) + 1, false)? + MINMATCH;
                if offset > self.op {
                    return Err(self.ip);
                }
            } else {
                length += MINMATCH;
            }
            if signed(self.op).saturating_add(signed(length))
                >= self.oend - signed(FASTLOOP_SAFE_DISTANCE)
            {
                return Ok(Lz4Entry::Match(offset, length));
            }
            if offset > self.op {
                return Err(self.ip);
            }
            self.copy(offset, length);
        }
    }

    /// The safe loop with its two-stage shortcut, from `entry`; the decoded
    /// length.
    fn safe_loop(&mut self, mut entry: Lz4Entry) -> Result<usize, usize> {
        // liblz4's `shortiend` and `shortoend`.
        let short_in = self.iend - 14 - 2;
        let short_out = self.oend - 14 - 18;
        loop {
            let (offset, length) = match std::mem::replace(&mut entry, Lz4Entry::Top) {
                Lz4Entry::Match(offset, length) => (offset, length),
                Lz4Entry::Literals(token, length) => match self.safe_literals(token, length)? {
                    Some(m) => m,
                    None => return Ok(self.op),
                },
                Lz4Entry::Top => {
                    let token = self.token()?;
                    let mut length = token >> 4;
                    if length != RUN_MASK
                        && signed(self.ip) < short_in
                        && signed(self.op) <= short_out
                    {
                        self.literals(length);
                        let mlen = token & ML_MASK;
                        let offset = self.le16()?;
                        if mlen != ML_MASK && offset >= 8 && offset <= self.op {
                            self.copy(offset, mlen + MINMATCH);
                            continue;
                        }
                        (offset, self.match_length(mlen)?)
                    } else {
                        if length == RUN_MASK {
                            length += self.varlen(self.iend - signed(RUN_MASK), true)?;
                        }
                        match self.safe_literals(token, length)? {
                            Some(m) => m,
                            None => return Ok(self.op),
                        }
                    }
                }
            };
            // `safe_match_copy`: the offset within the output, and the last
            // LASTLITERALS bytes left to literals.
            if offset > self.op {
                return Err(self.ip);
            }
            if signed(self.op).saturating_add(signed(length)) > self.oend - signed(LASTLITERALS) {
                return Err(self.ip);
            }
            self.copy(offset, length);
        }
    }
}

/// liblz4 1.9.4's `LZ4_decompress_generic` as `LZ4_decompress_safe` calls
/// it (a full block, no dictionary), on x86-64, where `LZ4_FAST_DEC_LOOP`
/// is on: the fast loop while 64 bytes of output remain, then the safe loop
/// with its two-stage shortcut. Every check is liblz4's at liblz4's place, so
/// the verdict is liblz4's; the copies are exact (liblz4's wild copies write
/// past a copy's end, but only bytes a later copy overwrites or the result
/// leaves out).
///
/// Returns the decoded length, or the input offset at which liblz4 fails (it
/// returns `-(offset) - 1`). With `dst` (exactly the decoded length, from an
/// earlier call without it), the output is written there.
fn lz4_run(src: &[u8], output_size: usize, dst: Option<&mut [u8]>) -> Result<usize, usize> {
    if output_size == 0 {
        return if src == [0] { Ok(0) } else { Err(0) };
    }
    if src.is_empty() {
        return Err(0);
    }
    let mut st = Lz4State {
        src,
        dst,
        ip: 0,
        op: 0,
        iend: signed(src.len()),
        oend: signed(output_size),
    };
    let entry = if st.oend >= signed(FASTLOOP_SAFE_DISTANCE) {
        st.fast_loop()?
    } else {
        Lz4Entry::Top
    };
    st.safe_loop(entry)
}
