//! Each row's name as a number, equal exactly where the names are: what
//! [`super::Tree::compile`] groups a class's children by, so that grouping
//! them reads no string (a Python dict's key collapse, plan section 3.8).
//!
//! **Work.** A blob's rows are zero-copy slices of its string buffer, and
//! every row that names one offset holds one slice of it
//! ([`crate::typetree::parse`]), so a name is read here once per *buffer*
//! (an address and a length), never once per row. The rows are sorted by
//! their name's length and address, which reads no byte of it; a buffer
//! whose length no other buffer of the list has is a name of its own and is
//! not read. Only buffers that share a length are read: each is hashed once
//! (FNV-1a), the buffers of one hash are compared byte for byte with the
//! first of them, and a run of one hash that holds more than one name (a
//! collision, which a hostile tree can make of any number of names: FNV-1a
//! is no defence) is sorted by its bytes in a merge sort of at most
//! ⌈log2 k⌉ passes of fewer than k comparisons each (review round 2: a scan
//! of the run's earlier names for each member was `O(k²)`), then its
//! neighbours compared. The buffers of one hash are taken, and a sort's
//! ties broken, in the order of the rows that first name them, never by
//! their addresses, so the same rows read and count the same bytes wherever
//! their buffers lie. Every byte read is the list's work
//! ([`crate::limits::Meter::count_work`] on [`NodeList::meter`],
//! `Limits::max_decode_work`), counted before it is read: a length's names
//! before they are hashed, and each comparison, as many bytes as its names
//! have, before it is made. So an allowance below it refuses the compile
//! ([`crate::LimitKind::DecodeWork`]) before it reads past the allowance
//! (review round 13: the compile hashed and compared every row's name and
//! searched every row's type, rows x length bytes, and counted none of it).
//! Distinct buffers of one content cannot be told apart without reading them
//! (R offsets of a NUL-less string buffer name R equal windows of 32,767
//! bytes, from a buffer of 32,767 + R bytes), so the allowance is what
//! bounds that work; rows that share a buffer cost it once, however many
//! there are (`work_tests::rows_naming_one_long_string_read_it_at_most_once`).
//!
//! A [`NameTable`] numbers a table of strings once, for the compiles of
//! rows that name only its strings, which then read no name (a TPK's trees,
//! which all name its one string table).

use std::cmp::Ordering;
use std::sync::Arc;

use crate::error::LimitError;
use crate::limits::{Meter, MeteredVec};
use crate::raw::RawStr;
use crate::typetree::{NodeId, NodeList, TreeError};

#[cfg(test)]
std::thread_local! {
    /// Name bytes this thread's compiles hashed or compared (tests only).
    pub(super) static READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Name comparisons this thread's compiles made (tests only).
    pub(super) static COMPARED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts `bytes` of names read (tests only).
fn read(bytes: usize) {
    #[cfg(test)]
    READ.with(|r| r.set(r.get().saturating_add(bytes as u64)));
    #[cfg(not(test))]
    let _ = bytes;
}

/// `a == b` on two names of one length, counted (tests only).
fn same(a: &[u8], b: &[u8]) -> bool {
    #[cfg(test)]
    COMPARED.with(|c| c.set(c.get() + 1));
    read(a.len());
    a == b
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn id32(i: usize) -> u32 {
    // Every count numbered here was checked against `u32` first.
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// A name's buffer: its length and its address, which read no byte of it.
/// Two names of one buffer are one name; while both are alive no other
/// buffer has that address.
fn buffer(name: &[u8]) -> (usize, usize) {
    (name.len(), name.as_ptr().addr())
}

/// Counts reading `names` names of `len` bytes as `meter`'s work, before
/// they are read.
fn count(meter: &Meter, names: usize, len: usize) -> Result<(), LimitError> {
    meter.count_work(as_u64(names).saturating_mul(as_u64(len)))
}

/// Each row's name id, and how many ids there may be (each is below it):
/// two rows' ids are equal exactly when their names are. `hash` groups the
/// names of one length ([`super::name_hash`]; a test's may make every name
/// collide). Fails when the meter refuses the tables
/// ([`TreeError::Charge`]) or the work ([`TreeError::Limit`],
/// [`crate::LimitKind::DecodeWork`]).
pub(super) fn name_ids(
    list: &NodeList,
    hash: fn(&[u8]) -> u64,
) -> Result<(MeteredVec<u32>, usize), TreeError> {
    number(
        list.len(),
        &|row| list.name(NodeId(row)),
        list.meter(),
        hash,
    )
}

/// [`name_ids`] of `n` names, `name(i)` the `i`th, the work counted on
/// `meter` and the tables charged to it.
fn number<'a>(
    n: usize,
    name: &impl Fn(u32) -> &'a [u8],
    meter: &Arc<Meter>,
    hash: fn(&[u8]) -> u64,
) -> Result<(MeteredVec<u32>, usize), TreeError> {
    let buffer = |row: u32| buffer(name(row));
    let mut order: MeteredVec<u32> = MeteredVec::with_capacity(meter, n)?;
    for row in 0..n {
        order.push(id32(row))?;
    }
    order
        .as_mut_slice()
        .sort_unstable_by_key(|row| (buffer(*row), *row));
    // Each row's buffer, numbered in that order (so the buffers of one
    // length are numbered consecutively), and a row of each buffer.
    let mut ids: MeteredVec<u32> = MeteredVec::with_capacity(meter, n)?;
    for _ in 0..n {
        ids.push(0)?;
    }
    let mut rows: MeteredVec<u32> = MeteredVec::new(meter);
    let mut last = None;
    for &row in order.as_slice() {
        let b = buffer(row);
        if last != Some(b) {
            rows.push(row)?;
            last = Some(b);
        }
        if let Some(id) = ids.as_mut_slice().get_mut(row as usize) {
            *id = id32(rows.len().saturating_sub(1));
        }
    }
    drop(order);
    // A buffer's first row, the least that names it, orders the buffers of
    // one hash, so what a compile reads and counts depends on its rows and
    // never on where their buffers lie in memory.
    let first = |b: u32| rows.as_slice().get(b as usize).copied().unwrap_or(0);
    let text = |b: u32| name(first(b));
    let names = buffer_names(&Buffers { text, first }, rows.len(), meter, hash)?;
    for id in ids.as_mut_slice() {
        *id = names.as_slice().get(*id as usize).copied().unwrap_or(*id);
    }
    Ok((ids, rows.len()))
}

/// The buffers [`buffer_names`] names: `text(b)` the `b`th's bytes (the
/// buffers sorted by length), `first(b)` the least row that names it.
struct Buffers<T, F> {
    text: T,
    first: F,
}

/// For each of `m` buffers, the number of the first buffer of its content.
fn buffer_names<'a>(
    bufs: &Buffers<impl Fn(u32) -> &'a [u8], impl Fn(u32) -> u32>,
    m: usize,
    meter: &Arc<Meter>,
    hash: fn(&[u8]) -> u64,
) -> Result<MeteredVec<u32>, TreeError> {
    let mut names: MeteredVec<u32> = MeteredVec::with_capacity(meter, m)?;
    for b in 0..m {
        names.push(id32(b))?;
    }
    let mut scratch = Scratch {
        hashes: MeteredVec::new(meter),
        run: MeteredVec::new(meter),
        tmp: MeteredVec::new(meter),
    };
    let mut start = 0;
    let text = &bufs.text;
    while start < m {
        let len = text(id32(start)).len();
        let mut end = start + 1;
        while end < m && text(id32(end)).len() == len {
            end += 1;
        }
        if end - start > 1 {
            one_length(
                meter,
                bufs,
                id32(start)..id32(end),
                len,
                hash,
                &mut scratch,
                names.as_mut_slice(),
            )?;
        }
        start = end;
    }
    Ok(names)
}

/// The tables [`one_length`] reuses, each charged as it grows.
struct Scratch {
    /// `(hash, first row, buffer)`.
    hashes: MeteredVec<(u64, u32, u32)>,
    run: MeteredVec<u32>,
    tmp: MeteredVec<u32>,
}

/// Names the buffers `buffers`, all of `len` bytes: each hashed, and the
/// buffers of one hash compared, in the order of their first rows (module
/// docs, "Work").
fn one_length<'a>(
    meter: &Meter,
    bufs: &Buffers<impl Fn(u32) -> &'a [u8], impl Fn(u32) -> u32>,
    buffers: std::ops::Range<u32>,
    len: usize,
    hash: fn(&[u8]) -> u64,
    s: &mut Scratch,
    names: &mut [u32],
) -> Result<(), TreeError> {
    count(meter, buffers.len(), len)?;
    s.hashes.truncate(0);
    for b in buffers {
        read(len);
        s.hashes.push((hash((bufs.text)(b)), (bufs.first)(b), b))?;
    }
    s.hashes.as_mut_slice().sort_unstable();
    let mut at = 0;
    while at < s.hashes.len() {
        let h = s.hashes.as_slice().get(at).map_or(0, |e| e.0);
        let mut to = at + 1;
        while s.hashes.as_slice().get(to).is_some_and(|e| e.0 == h) {
            to += 1;
        }
        if to - at > 1 {
            s.run.truncate(0);
            for e in s.hashes.as_slice().get(at..to).unwrap_or_default() {
                s.run.push(e.2)?;
            }
            one_hash(meter, bufs, len, s, names)?;
        }
        at = to;
    }
    Ok(())
}

/// Names the buffers in `s.run`, all of `len` bytes and one hash: one name
/// when each equals the first, else (a collision) sorted by their bytes and
/// each run of equal neighbours one name.
fn one_hash<'a>(
    meter: &Meter,
    bufs: &Buffers<impl Fn(u32) -> &'a [u8], impl Fn(u32) -> u32>,
    len: usize,
    s: &mut Scratch,
    names: &mut [u32],
) -> Result<(), TreeError> {
    let text = &bufs.text;
    let k = s.run.len();
    let Some(&first) = s.run.as_slice().first() else {
        return Ok(());
    };
    let mut one = true;
    for b in s.run.as_slice().get(1..).unwrap_or_default() {
        count(meter, 1, len)?;
        if !same(text(*b), text(first)) {
            one = false;
            break;
        }
    }
    if one {
        for b in s.run.as_slice() {
            if let Some(n) = names.get_mut(*b as usize) {
                *n = first;
            }
        }
        return Ok(());
    }
    s.tmp.truncate(0);
    for _ in 0..k {
        s.tmp.push(0)?;
    }
    merge_sort(s.run.as_mut_slice(), s.tmp.as_mut_slice(), |a, b| {
        count(meter, 1, len)?;
        #[cfg(test)]
        COMPARED.with(|c| c.set(c.get() + 1));
        read(len);
        let row = &bufs.first;
        Ok::<_, TreeError>(text(a).cmp(text(b)).then(row(a).cmp(&row(b))))
    })?;
    let mut leader = first;
    for (r, &b) in s.run.as_slice().iter().enumerate() {
        if r > 0 {
            count(meter, 1, len)?;
        }
        if r == 0 || !same(text(b), text(leader)) {
            leader = b;
        }
        if let Some(n) = names.get_mut(b as usize) {
            *n = leader;
        }
    }
    Ok(())
}

/// The names of a table of strings, numbered once ([`Self::new`]), so that
/// a compile of rows whose names are all the table's strings reads none of
/// them ([`super::Tree::compile_with_names`]). A TPK's trees all name its
/// one string table (`unityrs-tpk`), and each compile would otherwise read
/// again the strings its rows name: a TPK of R release roots whose trees
/// name two strings of one length, of 8 MiB each, read 16 MiB per root.
/// It knows a name by its buffer, never by its bytes, and keeps a clone of
/// every string, so no buffer it knows is freed, and its address taken by
/// another, while it lives: a row whose name is one of its strings (a clone
/// of it, or a slice of the same bytes at the same place) is that string.
pub struct NameTable {
    /// Each string's buffer and name id, sorted.
    buffers: MeteredVec<((usize, usize), u32)>,
    /// One past the largest name id.
    bound: usize,
    /// The strings, kept for their buffers.
    _strings: MeteredVec<RawStr>,
}

const _: () = crate::send_sync::<NameTable>();

impl NameTable {
    /// Numbers `strings`' names, the bytes it reads counted on `meter` as
    /// [`Tree::compile`](super::Tree::compile) counts a list's (module
    /// docs, "Work") and its tables charged to it.
    pub fn new(strings: &[RawStr], meter: &Arc<Meter>) -> Result<Self, TreeError> {
        if u32::try_from(strings.len()).is_err() {
            return Err(TreeError::TooManyNodes(strings.len()));
        }
        let text = |i: u32| strings.get(i as usize).map_or(&[][..], RawStr::as_bytes);
        let (ids, bound) = number(strings.len(), &text, meter, super::name_hash)?;
        let mut kept = MeteredVec::with_capacity(meter, strings.len())?;
        let mut buffers = MeteredVec::with_capacity(meter, strings.len())?;
        for (s, id) in strings.iter().zip(ids.as_slice()) {
            kept.push(s.clone())?;
            buffers.push((buffer(s.as_bytes()), *id))?;
        }
        buffers.as_mut_slice().sort_unstable();
        Ok(Self {
            buffers,
            bound,
            _strings: kept,
        })
    }

    /// Each row's name id from the table, and one past the largest, when
    /// every row's name is one of the table's strings; `None` otherwise.
    /// A binary search a row, which reads no name.
    pub(super) fn ids_of(
        &self,
        list: &NodeList,
    ) -> Result<Option<(MeteredVec<u32>, usize)>, TreeError> {
        let mut ids = MeteredVec::with_capacity(list.meter(), list.len())?;
        let table = self.buffers.as_slice();
        for row in list.rows() {
            let key = buffer(row.name.as_bytes());
            let at = table.partition_point(|(b, _)| *b < key);
            match table.get(at) {
                Some((b, id)) if *b == key => ids.push(*id)?,
                _ => return Ok(None),
            }
        }
        Ok(Some((ids, self.bound)))
    }
}

/// Sorts `v` by `cmp`, which may fail, in a bottom-up merge sort through
/// `tmp` (as long as `v`): ⌈log2 k⌉ passes over k elements, each of fewer
/// than k comparisons, so a refusal stops it between two comparisons.
fn merge_sort<E>(
    items: &mut [u32],
    tmp: &mut [u32],
    mut cmp: impl FnMut(u32, u32) -> Result<Ordering, E>,
) -> Result<(), E> {
    let count = items.len();
    let mut width = 1_usize;
    while width < count {
        let mut lo = 0;
        while lo < count {
            let mid = lo.saturating_add(width).min(count);
            let hi = mid.saturating_add(width).min(count);
            let (mut left_at, mut right_at) = (lo, mid);
            for out in lo..hi {
                let left = items.get(left_at).copied().filter(|_| left_at < mid);
                let right = items.get(right_at).copied().filter(|_| right_at < hi);
                let next = match (left, right) {
                    (Some(first), Some(second)) if cmp(second, first)? == Ordering::Less => {
                        right_at += 1;
                        second
                    }
                    (Some(first), _) => {
                        left_at += 1;
                        first
                    }
                    (None, Some(second)) => {
                        right_at += 1;
                        second
                    }
                    (None, None) => break,
                };
                if let Some(slot) = tmp.get_mut(out) {
                    *slot = next;
                }
            }
            lo = hi;
        }
        if let Some(merged) = tmp.get(..count) {
            items.copy_from_slice(merged);
        }
        width = width.saturating_mul(2);
    }
    Ok(())
}
