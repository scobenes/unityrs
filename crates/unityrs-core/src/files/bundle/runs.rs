// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! Block outcomes and runs, as the `bundle` module docs describe them: each
//! block's kept length or failure, measuring decodes, the known prefix a
//! read searches for its blocks, and the cached runs a read slices, or
//! decodes and publishes. A read costs O(log B + log R) steps besides the
//! blocks it learns or decodes, each counted as the open's work (`index`'s
//! module docs).

use std::ops::Range;
use std::sync::PoisonError;

use bytes::Bytes;

use super::decode::decode_block_into;
use super::index::{Published, STEP, search_steps};
use super::open::known_at_open;
use super::{BundleFile, no_entry};
use crate::budget::DecodeBuf;
use crate::codec::Codec;
use crate::error::{Error, LimitKind};
use crate::limits::MeteredVec;
use crate::once::get_or_publish_result;

/// A read's blocks, `first..end`, from joined offset `first_at` to `end_at`,
/// with the run decoded from `first` when the walk that found them decoded
/// it.
struct Span {
    first: usize,
    first_at: u64,
    end: usize,
    end_at: u64,
    buf: Option<DecodeBuf>,
}

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

    /// Counts `n` steps as the open's work (`index`'s module docs), refused
    /// with [`LimitKind::DecodeWork`] when they would pass its allowance.
    fn steps(&self, n: u64) -> Result<(), Error> {
        if n > 0 {
            self.meter().count_decode(n.saturating_mul(STEP))?;
        }
        Ok(())
    }

    /// Adds block `k`, of length `len`, to the known prefix: one step.
    fn learn(&self, k: usize, len: u64) -> Result<(), Error> {
        self.steps(1)?;
        self.ends.learn(k, len);
        Ok(())
    }

    /// Decodes block `i` onto the end of `out` and records its length (a
    /// failure the input decides is kept, a refusal is not): the length.
    /// What it wrote is counted ([`Self::counts`]), as written again when
    /// the block's length was known before it began, but from its stored
    /// bytes at open.
    fn decode_into(&self, i: usize, out: &mut DecodeBuf) -> Result<u64, Error> {
        let before = out.len();
        let (Some(b), Some(range)) = (self.blocks.get(i), self.stored.get(i)) else {
            return Err(no_entry(i));
        };
        let known = self.lengths.get(i).is_some_and(|c| c.get().is_some());
        let again = known && !known_at_open(b, &self.ctx);
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
        self.counts.wrote(mine, again);
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

    /// Decodes blocks `from..to`, already known, onto the end of `out`: a
    /// step each, besides what they write.
    fn decode_again(&self, from: usize, to: usize, out: &mut DecodeBuf) -> Result<(), Error> {
        for k in from..to {
            self.decode_or_keep(k, out)?;
            self.steps(1)?;
        }
        Ok(())
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

    /// A measuring decode's buffer, dropped unused: its bytes are counted as
    /// discarded ([`Self::counts`]).
    fn discard(&self, scratch: Option<DecodeBuf>) {
        if let Some(s) = scratch {
            self.counts
                .dropped(u64::try_from(s.len()).unwrap_or(u64::MAX));
        }
    }

    /// Block `k`'s length, measured when it is not known: decoded into a new
    /// buffer, which comes back with it.
    fn measure(&self, k: usize) -> Result<(u64, Option<DecodeBuf>), Error> {
        if let Some(len) = self.known(k) {
            return Ok((len?, None));
        }
        let mut scratch = self.new_buf(0)?;
        let len = self.decode_or_keep(k, &mut scratch)?;
        Ok((len, Some(scratch)))
    }

    /// Every block's length, measured: the known prefix is extended to the
    /// last block, unknown blocks decoded with their output discarded. The
    /// joined total; once every block is known, one lookup.
    pub(super) fn measure_all(&self) -> Result<u64, Error> {
        let n = self.blocks.len();
        for k in self.ends.known()..n {
            let (len, scratch) = self.measure(k)?;
            self.discard(scratch);
            self.learn(k, len)?;
        }
        Ok(self.ends.start(n))
    }

    /// The joined bytes `[lo, min(hi, total))`. Every block starting below
    /// `hi` gets a known length (the known prefix is extended to it, decoding
    /// the unknown blocks; one wholly before `lo` is decoded with its output
    /// discarded), the blocks overlapping the read form its span, and the
    /// span is a cached run's slice, a slice of the input (consecutive stored
    /// blocks, which is then cached as a run), or decoded into a new run that
    /// is published. Blocks the known prefix holds are found by binary
    /// search, and a cached run by one search of the run index.
    pub(super) fn read_span(&self, lo: u64, hi: u64) -> Result<Bytes, Error> {
        Ok(self.read_span_pinned(lo, hi)?.0)
    }

    /// [`Self::read_span`], with the length of the buffer the bytes are a
    /// slice of, which a slice of them keeps alive ([`Self::pins`]).
    pub(super) fn read_span_pinned(&self, lo: u64, hi: u64) -> Result<(Bytes, u64), Error> {
        let Some((first, first_at, buf)) = self.first_block(lo, hi)? else {
            return Ok((Bytes::new(), 0));
        };
        let Span {
            first,
            first_at,
            end,
            end_at,
            buf,
        } = self.span_end(first, first_at, buf, hi)?;
        let from = usize::try_from(lo.saturating_sub(first_at)).unwrap_or(usize::MAX);
        let to = usize::try_from(hi.min(end_at).saturating_sub(first_at)).unwrap_or(usize::MAX);
        let (run, pinned) = if let Some(b) = buf {
            self.publish(first, end, first_at, b.into_bytes()?, true)
        } else if let Some(hit) = self.cached(first, end, first_at)? {
            hit
        } else if let Some(bytes) = self.stored_span(first, end)? {
            self.publish(first, end, first_at, bytes, false)
        } else {
            let mut b = self.new_buf(end_at.saturating_sub(first_at))?;
            self.decode_again(first, end, &mut b)?;
            self.publish(first, end, first_at, b.into_bytes()?, true)
        };
        let to = to.min(run.len());
        Ok((run.slice(from.min(to)..to), pinned))
    }

    /// What a slice of `run` keeps alive: the buffer of the bundle's own
    /// bytes when `run` is a slice of them (stored blocks; its
    /// [`BundleFile::pinned`]), else `run`'s own length, a decoded run being
    /// a buffer of its own.
    fn pins(&self, run: &Bytes) -> u64 {
        let (src, at) = (self.source.as_ptr().addr(), run.as_ptr().addr());
        let within =
            src <= at && at.saturating_add(run.len()) <= src.saturating_add(self.source.len());
        if within && !run.is_empty() {
            self.pinned()
        } else {
            u64::try_from(run.len()).unwrap_or(u64::MAX)
        }
    }

    /// The read's first block, the first that ends past `lo` if it starts
    /// below `hi`: its index, where it starts and, when the walk decoded it,
    /// its buffer. Found by binary search when the known prefix reaches past
    /// `lo`; otherwise the prefix is extended a block at a time until a
    /// block ends past `lo` or starts at `hi`.
    fn first_block(
        &self,
        lo: u64,
        hi: u64,
    ) -> Result<Option<(usize, u64, Option<DecodeBuf>)>, Error> {
        let known = self.ends.known();
        let (first, probes) = self.ends.first_ending_past(0, known, lo);
        self.steps(probes)?;
        if first < known {
            let at = self.ends.start(first);
            return Ok((at < hi).then_some((first, at, None)));
        }
        let mut at = self.ends.start(known);
        for k in known..self.blocks.len() {
            if at >= hi {
                break;
            }
            let (len, scratch) = self.measure(k)?;
            self.learn(k, len)?;
            if at.saturating_add(len) > lo {
                return Ok(Some((k, at, scratch)));
            }
            self.discard(scratch);
            at = at.saturating_add(len);
        }
        Ok(None)
    }

    /// The read's span from its first block to the last block that starts
    /// below `hi`. Found by binary search when the known prefix reaches `hi`
    /// and the walk decoded nothing; otherwise the prefix is extended to it,
    /// and from the first block of the span that is not known (or from
    /// `first`, when the walk decoded it) the run is decoded: its known
    /// blocks again, then each block as it is learned.
    fn span_end(
        &self,
        first: usize,
        first_at: u64,
        mut buf: Option<DecodeBuf>,
        hi: u64,
    ) -> Result<Span, Error> {
        let mut k = first + 1;
        if buf.is_none() {
            let known = self.ends.known();
            let (last, probes) = self.ends.first_ending_at(first, known, hi);
            self.steps(probes)?;
            if last < known {
                return Ok(Span {
                    first,
                    first_at,
                    end: last + 1,
                    end_at: self.ends.end(last),
                    buf: None,
                });
            }
            k = k.max(known);
        }
        let mut at = self.ends.start(k);
        while k < self.blocks.len() && at < hi {
            let len = match (self.known(k), buf.as_mut()) {
                (Some(len), None) => len?,
                (_, Some(b)) => self.decode_or_keep(k, b)?,
                (None, None) => {
                    let mut b = self.new_buf(0)?;
                    self.decode_again(first, k, &mut b)?;
                    let len = self.decode_or_keep(k, &mut b)?;
                    buf = Some(b);
                    len
                }
            };
            self.learn(k, len)?;
            at = at.saturating_add(len);
            k += 1;
        }
        Ok(Span {
            first,
            first_at,
            end: k,
            end_at: at,
            buf,
        })
    }

    /// A cached run covering blocks `first..end`, sliced to start at block
    /// `first` (joined offset `first_at`): one search of the run index. With
    /// what a slice of it keeps alive ([`Self::pins`]).
    fn cached(
        &self,
        first: usize,
        end: usize,
        first_at: u64,
    ) -> Result<Option<(Bytes, u64)>, Error> {
        let hit = {
            let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            self.steps(search_steps(runs.len()))?;
            runs.covering(first, end)
        };
        Ok(hit.map(|(f, bytes)| {
            (
                self.slice_from_block(f, &bytes, first_at),
                self.pins(&bytes),
            )
        }))
    }

    /// `bytes`, a run from block `f`, from joined offset `first_at` on (every
    /// block before `f` is in the known prefix).
    fn slice_from_block(&self, f: usize, bytes: &Bytes, first_at: u64) -> Bytes {
        let skip = first_at.saturating_sub(self.ends.start(f));
        let skip = usize::try_from(skip).unwrap_or(usize::MAX).min(bytes.len());
        bytes.slice(skip..)
    }

    /// Blocks `first..end` as one slice of the input, when every one is
    /// stored, stored blocks are their own bytes ([`crate::codec::Codecs::stores_as_is`]),
    /// and each follows the previous in the input: a step for each block it
    /// checks. Every block of the span
    /// has a known length, and a stored block's is known only within the
    /// block cap ([`crate::limits::Limits::max_block_output`]: preset at open
    /// only within it, and otherwise from its codec, which refuses a range
    /// past it), so no block of a slice is past the cap.
    fn stored_span(&self, first: usize, end: usize) -> Result<Option<Bytes>, Error> {
        if !self.ctx.codecs.stores_as_is() {
            return Ok(None);
        }
        let mut at: Option<Range<usize>> = None;
        for k in first..end {
            self.steps(1)?;
            let (Some(b), Some(r)) = (self.blocks.get(k), self.stored.get(k)) else {
                return Ok(None);
            };
            if Codec::from_flags(u32::from(b.flags_u16())) != Codec::None {
                return Ok(None);
            }
            at = match at {
                None => Some(r.clone()),
                Some(a) if a.end == r.start || r.is_empty() => Some(a.start..a.end.max(r.end)),
                Some(_) => return Ok(None),
            };
        }
        Ok(at.map(|r| self.source.slice(r)))
    }

    /// Publishes `bytes` as the run of blocks `first..end` (from joined
    /// offset `first_at`) and returns the bytes from block `first` on: a
    /// cached run that covers the span answers instead, and the runs the new
    /// one covers are dropped, after the lock. Its search and every run it
    /// drops are steps, each run counted as it is found. When the meter
    /// refuses them, the room for the runs it drops or the index's growth,
    /// the run is returned uncached and the index is unchanged, as a refused
    /// growth of the table left it. A `decoded` run a cached one answers for
    /// is dropped, its bytes counted as discarded ([`Self::counts`]). With
    /// what a slice of the bytes returned keeps alive ([`Self::pins`]).
    fn publish(
        &self,
        first: usize,
        end: usize,
        first_at: u64,
        bytes: Bytes,
        decoded: bool,
    ) -> (Bytes, u64) {
        let mut dropped: MeteredVec<Bytes> = MeteredVec::new(self.meter());
        let out = {
            let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            let uncached = |bytes: Bytes| {
                let pinned = self.pins(&bytes);
                (bytes, pinned)
            };
            if self.steps(search_steps(runs.len())).is_err() {
                return uncached(bytes);
            }
            for _ in runs.inside(first, end) {
                if self.steps(1).is_err() {
                    return uncached(bytes);
                }
            }
            match runs.publish(first, end, &bytes, &mut dropped) {
                Published::Covered(f, b) => {
                    if decoded {
                        self.counts
                            .dropped(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                    }
                    (self.slice_from_block(f, &b, first_at), self.pins(&b))
                }
                Published::Cached | Published::Refused => uncached(bytes),
            }
        };
        drop(dropped);
        out
    }
}
