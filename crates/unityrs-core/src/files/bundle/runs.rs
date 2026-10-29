// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! Block outcomes and runs, as the `bundle` module docs describe them: each
//! block's kept length or failure, measuring decodes, and the cached runs a
//! read slices, or decodes and publishes.

use std::ops::Range;
use std::sync::PoisonError;

use bytes::Bytes;

use super::decode::decode_block_into;
use super::{BundleFile, no_entry};
use crate::budget::DecodeBuf;
use crate::codec::Codec;
use crate::error::{Error, LimitKind};
use crate::limits::MeteredVec;
use crate::once::get_or_publish_result;

impl BundleFile {
    /// Block `i`'s length, if known (an error is the block's kept failure).
    fn known(&self, i: usize) -> Option<Result<u64, Error>> {
        Some(
            self.lengths
                .get(i)?
                .get()?
                .as_ref()
                .map(|n| *n)
                .map_err(|k| k.error().clone()),
        )
    }

    /// Decodes block `i` onto the end of `out` and records its length (a
    /// failure the input decides is kept, a refusal is not): the length.
    fn decode_into(&self, i: usize, out: &mut DecodeBuf) -> Result<u64, Error> {
        let before = out.len();
        let (Some(b), Some(range)) = (self.blocks.get(i), self.stored.get(i)) else {
            return Err(no_entry(i));
        };
        let input = self.source.slice(range.clone());
        let got = decode_block_into(
            &self.ctx,
            self.meter(),
            input,
            b.uncompressed,
            u32::from(b.flags_u16()),
            i,
            out,
        )
        .map(|()| u64::try_from(out.len() - before).unwrap_or(u64::MAX));
        if got.is_err() {
            out.truncate(before);
        }
        let cell = self.lengths.get(i).ok_or_else(|| no_entry(i))?;
        let mine = got?;
        let published = *get_or_publish_result(cell, self.meter(), || Ok(mine))?;
        if published != mine {
            return Err(Error::Decompress {
                codec: Codec::from_flags(u32::from(b.flags_u16())),
                detail: format!("block {i} decoded to {mine} bytes, before to {published}").into(),
            });
        }
        Ok(mine)
    }

    /// Decodes block `i` and records the outcome, keeping a failure the
    /// input decides.
    fn decode_or_keep(&self, i: usize, out: &mut DecodeBuf) -> Result<u64, Error> {
        let got = self.decode_into(i, out);
        if let (Err(e), Some(cell)) = (&got, self.lengths.get(i)) {
            // A refusal (meter, budget) is not kept; anything else is.
            let _ = get_or_publish_result(cell, self.meter(), || Err(e.clone()));
        }
        got
    }

    /// A run's buffer with room for `initial` bytes, charged before it is
    /// allocated. The room is memory; each block's decode counts what it
    /// writes into it as the open's work (`DecodeBuf`, round 6).
    fn new_buf(&self, initial: u64) -> Result<DecodeBuf, Error> {
        Ok(DecodeBuf::new(
            self.meter(),
            initial,
            u64::MAX,
            LimitKind::BlockOutput,
        )?)
    }

    /// Every block's length, measured: unknown blocks are decoded with their
    /// output discarded. The joined total.
    pub(super) fn measure_all(&self) -> Result<u64, Error> {
        let mut total = 0u64;
        for i in 0..self.blocks.len() {
            let n = if let Some(n) = self.known(i) {
                n?
            } else {
                let mut scratch = self.new_buf(0)?;
                self.decode_or_keep(i, &mut scratch)?
            };
            total = total.saturating_add(n);
        }
        Ok(total)
    }

    /// The joined bytes `[lo, min(hi, total))`, as one pass over the blocks:
    /// every block starting below `hi` gets a known length (decoding the
    /// unknown ones; one wholly before `lo` is decoded with its output
    /// discarded), the blocks overlapping the read form its span, and the
    /// span is a cached run's slice, a slice of the input (consecutive
    /// stored blocks), or decoded into a new run that is published.
    pub(super) fn read_span(&self, lo: u64, hi: u64) -> Result<Bytes, Error> {
        let n = self.blocks.len();
        let mut acc = 0u64;
        let mut first: Option<(usize, u64)> = None;
        // The run being decoded, from block `first`.
        let mut buf: Option<DecodeBuf> = None;
        let mut i = 0;
        while i < n && acc < hi {
            let len = if let Some(len) = self.known(i) {
                let len = len?;
                if acc.saturating_add(len) > lo && first.is_none() {
                    first = Some((i, acc));
                }
                if let Some(b) = buf.as_mut() {
                    self.decode_or_keep(i, b)?;
                }
                len
            } else if first.is_none() {
                // Perhaps wholly before the read: decoded on its own, kept
                // as the run's start only when it reaches the read.
                let mut scratch = self.new_buf(0)?;
                let len = self.decode_or_keep(i, &mut scratch)?;
                if acc.saturating_add(len) > lo {
                    first = Some((i, acc));
                    buf = Some(scratch);
                }
                len
            } else {
                if buf.is_none() {
                    // Start the run with the span's known blocks so far.
                    let mut b = self.new_buf(0)?;
                    if let Some((f, _)) = first {
                        for k in f..i {
                            self.decode_or_keep(k, &mut b)?;
                        }
                    }
                    buf = Some(b);
                }
                let b = buf.as_mut().ok_or_else(|| no_entry(i))?;
                self.decode_or_keep(i, b)?
            };
            acc = acc.saturating_add(len);
            i += 1;
        }
        let Some((first, first_at)) = first else {
            return Ok(Bytes::new());
        };
        let end = i;
        let from = usize::try_from(lo.saturating_sub(first_at)).unwrap_or(usize::MAX);
        let to = usize::try_from(hi.min(acc) - first_at).unwrap_or(usize::MAX);
        let run = if let Some(b) = buf {
            self.publish(first, end, b.into_bytes()?)
        } else if let Some(bytes) = self.cached(first, end, first_at)? {
            bytes
        } else if let Some(bytes) = self.stored_span(first, end) {
            bytes
        } else {
            let mut b = self.new_buf(acc - first_at)?;
            for k in first..end {
                self.decode_or_keep(k, &mut b)?;
            }
            self.publish(first, end, b.into_bytes()?)
        };
        let to = to.min(run.len());
        Ok(run.slice(from.min(to)..to))
    }

    /// The joined offset where block `first` starts (every length before it
    /// is known when a run starting there exists).
    fn joined_start(&self, first: usize) -> Result<u64, Error> {
        let mut at = 0u64;
        for k in 0..first {
            at = at.saturating_add(self.known(k).unwrap_or(Ok(0))?);
        }
        Ok(at)
    }

    /// A cached run covering blocks `first..end`, sliced to start at block
    /// `first` (joined offset `first_at`).
    fn cached(&self, first: usize, end: usize, first_at: u64) -> Result<Option<Bytes>, Error> {
        let hit = {
            let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            runs.as_slice()
                .iter()
                .find(|(f, e, _)| *f <= first && end <= *e)
                .map(|(f, _, b)| (*f, b.clone()))
        };
        let Some((f, bytes)) = hit else {
            return Ok(None);
        };
        let skip = first_at - self.joined_start(f)?;
        let skip = usize::try_from(skip).unwrap_or(usize::MAX).min(bytes.len());
        Ok(Some(bytes.slice(skip..)))
    }

    /// Blocks `first..end` as one slice of the input, when every one is
    /// stored and each follows the previous in the input.
    fn stored_span(&self, first: usize, end: usize) -> Option<Bytes> {
        let mut at: Option<Range<usize>> = None;
        for k in first..end {
            let b = self.blocks.get(k)?;
            if Codec::from_flags(u32::from(b.flags_u16())) != Codec::None {
                return None;
            }
            let r = self.stored.get(k)?;
            at = match at {
                None => Some(r.clone()),
                Some(a) if a.end == r.start || r.is_empty() => Some(a.start..a.end.max(r.end)),
                Some(_) => return None,
            };
        }
        at.map(|r| self.source.slice(r))
    }

    /// Publishes the run of blocks `first..end`: an equal run already there
    /// answers instead, and runs it covers are dropped (after the lock).
    /// The list they wait in is charged before it is allocated (plan 3.15);
    /// when the meter refuses it, the new run is returned uncached and the
    /// runs it covers stay, as when the table's growth is refused.
    fn publish(&self, first: usize, end: usize, bytes: Bytes) -> Bytes {
        let mut dropped: MeteredVec<(usize, usize, Bytes)> = MeteredVec::new(self.meter());
        let out = {
            let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, _, b)) = runs
                .as_slice()
                .iter()
                .find(|(f, e, _)| *f == first && *e == end)
            {
                b.clone()
            } else {
                let all = runs.as_mut_slice();
                let mut k = 0;
                let mut keep_n = all.len();
                while k < keep_n {
                    if all.get(k).is_some_and(|(f, e, _)| first <= *f && *e <= end) {
                        all.swap(k, keep_n - 1);
                        keep_n -= 1;
                    } else {
                        k += 1;
                    }
                }
                let covered = all.len() - keep_n;
                if covered > 0 && dropped.reserve_exact(covered).is_err() {
                    return bytes;
                }
                for (f, e, b) in all.get_mut(keep_n..).unwrap_or_default() {
                    // Within the room just reserved: no growth, no refusal.
                    let _ = dropped.push((*f, *e, std::mem::take(b)));
                }
                runs.truncate(keep_n);
                // A refused growth of the table keeps the run uncached.
                let _ = runs.push((first, end, bytes.clone()));
                bytes
            }
        };
        drop(dropped);
        out
    }
}
