//! What a slice keeps alive (plan sections 3.15 and 8, R3). A `Bytes`
//! slice of a decoded run, or of an input's own bytes, keeps the whole
//! buffer alive, with its charge, for as long as the slice lives: on the
//! corpus one value kept past its input's handles kept the input's whole
//! decoded run alive (up to 69.4 MB), through its strings and byte arrays
//! and through the node lists that name its keys, whose strings are slices
//! of the file too.
//!
//! So a value an object read returns ([`crate::object::ObjectRef`]) and
//! each typetree a `SerializedFile` parses copy their slices out, into one
//! buffer of their own charged to the same meter, when the slices would
//! keep much more than themselves alive: when `L - S >= max(1 MiB, 4 S)`
//! ([`worth_copying`]), `L` the length of the buffer they are slices of
//! (the file's [`crate::files::SerializedFile::pinned`]) and `S` the
//! distinct bytes they span. Below that, a kept value keeps at most
//! `max(S + 1 MiB, 5 S)` of the buffer alive, its own slices included, and
//! a copy would cost more than it frees. The copy's charge rides in its
//! `Bytes`, its owner record included ([`crate::budget::charged_bytes`]), so
//! it returns when the last slice of it drops. A copy the meter or the
//! budget refuses is not made: the slices stay as they were, and the read or
//! the parse goes on. [`Copies`] counts what was copied, per file. The
//! other slices the core hands out are not copied, and each keeps its whole
//! buffer alive as any slice does: [`crate::object::ObjectRef::raw_data`],
//! [`crate::files::BundleFile::entry_bytes`] and
//! [`crate::files::BundleFile::entry_range`],
//! [`crate::files::SerializedFile::data`], and the value of
//! [`crate::walk::read_typetree`], whose caller hands it the buffer.
//!
//! **Each byte once.** The copy holds each byte of the buffer that some
//! slice names once, however many slices name it, and each slice becomes the
//! slice of the copy at the same place in those bytes: slices that were one
//! buffer (an address and a length) are one buffer of the copy, and slices
//! that overlapped overlap there as they did. So what a tree's compile reads
//! and counts, which goes by buffer
//! (`typetree::tree::names`: rows that share a buffer cost what one row
//! naming it costs), is the same for its copied rows as for its rows read
//! from a short run, and a name 300 rows share is held once (perf stage 2's
//! review: each slice was copied on its own, so 300 rows naming one
//! 1,000-byte string held 300 copies of it, and their compile, which no
//! longer saw one buffer, counted 600,000 bytes of work where the same file
//! read lazily counted 12,000). To find those bytes the slices are listed
//! by where they lie, sorted, and merged where they overlap or touch: a
//! table of two words a slice, charged to the meter while the copy is made,
//! whose refusal is the copy's.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::error::LimitKind;
use crate::limits::{Meter, MeteredVec};
use crate::raw::RawStr;
use crate::typetree::NodeRow;

/// The least a copy frees: slices of a buffer this long or shorter are
/// never copied.
pub const UNPIN_MIN: u64 = 1 << 20;

/// Whether slices of `s` bytes, of a buffer of `pinned` bytes, are copied
/// out: `pinned - s >= max(1 MiB, 4 s)`, and there is something to copy.
#[must_use]
pub const fn worth_copying(pinned: u64, s: u64) -> bool {
    let four = s.saturating_mul(4);
    let floor = if four > UNPIN_MIN { four } else { UNPIN_MIN };
    s > 0 && pinned.saturating_sub(s) >= floor
}

/// A slice a copy may replace.
pub(crate) enum Slice<'a> {
    Str(&'a mut RawStr),
    Bytes(&'a mut Bytes),
}

impl Slice<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Str(s) => s.as_bytes(),
            Self::Bytes(b) => b,
        }
    }

    fn set(&mut self, to: Bytes) {
        match self {
            Self::Str(s) => **s = RawStr::from_shared(to),
            Self::Bytes(b) => **b = to,
        }
    }
}

/// What holds slices, each visited in one fixed order.
pub(crate) trait Slices {
    fn each(&mut self, f: &mut dyn FnMut(Slice<'_>));
}

impl Slices for MeteredVec<NodeRow> {
    fn each(&mut self, f: &mut dyn FnMut(Slice<'_>)) {
        for row in self.as_mut_slice() {
            f(Slice::Str(&mut row.ty));
            f(Slice::Str(&mut row.name));
        }
    }
}

fn len64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Where `b` lies in `within`, from and to, if it is a non-empty slice of it.
fn place(within: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    let at = b.as_ptr().addr().checked_sub(within.as_ptr().addr())?;
    let end = at.checked_add(b.len())?;
    (!b.is_empty() && end <= within.len()).then_some((at, end))
}

/// `spans` sorted and merged where they overlap or touch, in place: the
/// number of merged spans, which then lead the table.
fn merge(spans: &mut [(usize, usize)]) -> usize {
    spans.sort_unstable();
    let mut kept = 0usize;
    for i in 0..spans.len() {
        let Some(&(from, to)) = spans.get(i) else {
            break;
        };
        if let Some(last) = kept.checked_sub(1).and_then(|k| spans.get_mut(k))
            && from <= last.1
        {
            last.1 = last.1.max(to);
            continue;
        }
        // `kept <= i`: the slot is there.
        if let Some(slot) = spans.get_mut(kept) {
            *slot = (from, to);
        }
        kept += 1;
    }
    kept
}

/// Copies the slices of `target` that lie in `within` (a slice of a buffer
/// of `pinned` bytes) into one buffer charged to `meter`, when
/// [`worth_copying`] says so, each byte once (module docs), each slice then
/// a slice of the copy: the bytes copied, 0 when nothing was (no slice lies
/// in `within`, the copy would free too little, or the meter or the budget
/// refused it or its table of slices).
pub(crate) fn copy_out(
    target: &mut dyn Slices,
    within: &[u8],
    pinned: u64,
    meter: &Arc<Meter>,
) -> u64 {
    if pinned <= UNPIN_MIN {
        return 0;
    }
    let mut n = 0usize;
    target.each(&mut |sl| n += usize::from(place(within, sl.bytes()).is_some()));
    if n == 0 {
        return 0;
    }
    let Ok(mut table) = MeteredVec::with_capacity(meter, n) else {
        return 0;
    };
    let mut listed = true;
    target.each(&mut |sl| {
        if let Some(span) = place(within, sl.bytes()) {
            listed &= table.push(span).is_ok();
        }
    });
    if !listed {
        return 0;
    }
    let kept = merge(table.as_mut_slice());
    let Some(spans) = table.as_mut_slice().get_mut(..kept) else {
        return 0;
    };
    let s = spans
        .iter()
        .fold(0u64, |s, (from, to)| s.saturating_add(len64(to - from)));
    if !worth_copying(pinned, s) {
        return 0;
    }
    let Ok(mut buf) = DecodeBuf::new(meter, s, s, LimitKind::InputMemory) else {
        return 0;
    };
    for span in spans.iter_mut() {
        let Some(bytes) = within.get(span.0..span.1) else {
            return 0;
        };
        if buf.extend_from_slice(bytes).is_err() {
            return 0;
        }
        // From here a span is its start and where its bytes start in the copy.
        span.1 = buf.len() - (span.1 - span.0);
    }
    let Ok(copy) = buf.into_bytes() else {
        return 0;
    };
    target.each(&mut |mut sl| {
        if let Some((from, to)) = place(within, sl.bytes()) {
            // The span holding `from`: the last that starts at or before it.
            let i = spans.partition_point(|&(start, _)| start <= from);
            if let Some(&(start, at)) = i.checked_sub(1).and_then(|i| spans.get(i)) {
                let at = at + (from - start);
                sl.set(copy.slice(at..at + (to - from)));
            }
        }
    });
    s
}

/// What a file's reads and its parse copied out of the buffer its bytes
/// are a slice of (module docs; plan section 8, R3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Copies {
    /// Values whose slices an object read copied out.
    pub values: u64,
    /// The bytes those copies hold.
    pub value_bytes: u64,
    /// Typetrees whose strings the parse copied out.
    pub trees: u64,
    /// The bytes those copies hold.
    pub tree_bytes: u64,
}

const _: () = crate::send_sync::<Copies>();

/// [`Copies`] as a file counts them, on any thread: two relaxed atomics
/// per value copied, nothing per value read. A build with `--cfg
/// unityrs_no_counters` compiles the counting out, as it does the bundles'
/// counts (the copies are made all the same).
#[derive(Debug, Default)]
pub(crate) struct CopyCounts {
    values: AtomicU64,
    value_bytes: AtomicU64,
    trees: u64,
    tree_bytes: u64,
}

impl CopyCounts {
    /// The parse's typetree copies: `trees` of them, holding `bytes`.
    pub(crate) fn new(trees: u64, bytes: u64) -> Self {
        let on = cfg!(not(unityrs_no_counters));
        Self {
            trees: if on { trees } else { 0 },
            tree_bytes: if on { bytes } else { 0 },
            ..Self::default()
        }
    }

    /// A value read that copied `bytes` out (none for 0).
    pub(crate) fn value(&self, bytes: u64) {
        if cfg!(not(unityrs_no_counters)) && bytes > 0 {
            self.values.fetch_add(1, Ordering::Relaxed);
            self.value_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    pub(crate) fn get(&self) -> Copies {
        Copies {
            values: self.values.load(Ordering::Relaxed),
            value_bytes: self.value_bytes.load(Ordering::Relaxed),
            trees: self.trees,
            tree_bytes: self.tree_bytes,
        }
    }
}

#[cfg(test)]
mod tests;
