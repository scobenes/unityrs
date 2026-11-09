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
//! (the file's [`crate::files::SerializedFile::pinned`]) and `S` their
//! bytes. Below that, a kept value keeps at most `max(S + 1 MiB, 5 S)` of
//! the buffer alive, its own slices included, and a copy would cost more
//! than it frees. The copy's charge rides in its `Bytes`, its owner record
//! included ([`crate::budget::charged_bytes`]), so it returns when the last
//! slice of it drops. A copy the meter or the budget refuses is not made:
//! the slices stay as they were, and the read or the parse goes on.
//! [`Copies`] counts what was copied, per file.

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

fn len64(b: &[u8]) -> u64 {
    u64::try_from(b.len()).unwrap_or(u64::MAX)
}

/// Copies the slices of `target` that lie in `within` (a slice of a buffer
/// of `pinned` bytes) into one buffer charged to `meter`, when
/// [`worth_copying`] says so (module docs), each slice then a slice of the
/// copy: the bytes copied, 0 when nothing was (no slice lies in `within`,
/// the copy would free too little, or the meter or the budget refused it).
pub(crate) fn copy_out(
    target: &mut dyn Slices,
    within: &[u8],
    pinned: u64,
    meter: &Arc<Meter>,
) -> u64 {
    if pinned <= UNPIN_MIN {
        return 0;
    }
    let start = within.as_ptr().addr();
    let end = start.saturating_add(within.len());
    let inside = |b: &[u8]| {
        let at = b.as_ptr().addr();
        !b.is_empty() && start <= at && at.saturating_add(b.len()) <= end
    };
    let mut s = 0u64;
    target.each(&mut |sl| {
        if inside(sl.bytes()) {
            s = s.saturating_add(len64(sl.bytes()));
        }
    });
    if !worth_copying(pinned, s) {
        return 0;
    }
    let Ok(mut buf) = DecodeBuf::new(meter, s, s, LimitKind::InputMemory) else {
        return 0;
    };
    let mut written = true;
    target.each(&mut |sl| {
        if written && inside(sl.bytes()) {
            written = buf.extend_from_slice(sl.bytes()).is_ok();
        }
    });
    if !written {
        return 0;
    }
    let Ok(copy) = buf.into_bytes() else {
        return 0;
    };
    let mut at = 0usize;
    target.each(&mut |mut sl| {
        if inside(sl.bytes()) {
            let n = sl.bytes().len();
            sl.set(copy.slice(at..at + n));
            at += n;
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
