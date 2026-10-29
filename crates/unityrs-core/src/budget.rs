//! Memory budgets: the mechanism in the core, the policy in the batch engine.
//!
//! A budget is shared by many inputs (a batch's admission pool, its
//! dependency pool). The core never asks it directly: every charge an
//! input's [`crate::limits::Meter`] grants is also booked here, as a
//! [`Permit`] that rides inside the charge, so the budget sees everything
//! every input holds, and gets it back when the last owner drops it. The
//! request never blocks: blocking inside a rayon pool can deadlock, so a
//! batch engine does any waiting of its own *before* it hands work to the
//! pool (plan section 4.1 lists what the core guarantees it).
//!
//! A decoder writes through a [`DecodeBuf`], which charges the input's meter
//! before every growth, so no decoded byte exists unpaid, a refused growth
//! allocates nothing, and a dropped buffer (a failed decode) returns its
//! charge.
//!
//! Owner: the scaffold (implemented); package C maintains it.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;

use crate::error::{LimitError, LimitKind};
use crate::limits::{Charge, ChargeError, Meter, grow_charged, grow_counted, heap_cost};

/// Which kind of load a charge belongs to, so a refusal says which pool
/// refused (plan section 4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reservation {
    /// An input the caller loaded: everything its meter charges.
    Input,
    /// A file the environment loaded as a dependency (its LRU), and
    /// everything that file's meter charges.
    Dependency,
}

/// A refused reservation.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("memory budget refused {requested} bytes for {what:?}")]
pub struct BudgetExceeded {
    /// Bytes asked for.
    pub requested: u64,
    /// What they were for.
    pub what: Reservation,
}

/// A source of permits.
///
/// **Contract** (plan section 3.16). The core calls a budget, and a permit's
/// home, wherever it charges or returns memory, *including while it holds
/// one of its own locks* (publishing a decoded run grows the bundle's run
/// table under its mutex; a value replaced in a cell or a registry slot
/// returns its charge as it drops). So an implementation must not block (it
/// refuses instead: blocking inside a pool can deadlock it), must not call
/// into the core, and must not take a lock that a thread may hold while it
/// calls the core (Python's GIL among them): a budget that does any of these
/// can deadlock against a core lock. [`Unbounded`] and [`Counting`] are
/// atomic counters. The permits of one pool share one home (one
/// `Arc<dyn PermitHome>`): the core merges permits by home, and keeps one
/// record per distinct home a charge absorbs, so a budget that made a home
/// per reservation would grow every owner's bookkeeping.
pub trait MemoryBudget: Send + Sync + fmt::Debug {
    /// Reserves `bytes`, or refuses at once.
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded>;
}

/// Where a permit returns its bytes. The same contract as [`MemoryBudget`]:
/// called under core locks, so it must not block or call the core.
pub trait PermitHome: Send + Sync {
    /// Takes `bytes` back.
    fn release(&self, bytes: u64);
}

/// Bytes reserved from a budget, returned when the permit drops.
///
/// A permit can take in another ([`Permit::absorb`]), so a buffer that grew
/// past its first reservation carries every top-up in one owner, and can give
/// back what its buffer no longer holds ([`Permit::shrink_to`]). Permits
/// merge by home: an empty permit takes the home of the first one it
/// absorbs, a permit of the same home adds its bytes, and one of another
/// home merges into the record kept for that home, so a permit holds one
/// record per distinct home whatever it absorbed ([`Self::records`]).
#[must_use = "a permit returns its bytes when dropped"]
pub struct Permit {
    bytes: u64,
    home: Option<Arc<dyn PermitHome>>,
    /// Absorbed permits whose home differs from this one's, one per home.
    more: Vec<Permit>,
}

fn same_home(a: Option<&Arc<dyn PermitHome>>, b: Option<&Arc<dyn PermitHome>>) -> bool {
    match (a, b) {
        (None, None) => true,
        // Compare the data addresses only: two vtables for one type may differ.
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

impl Permit {
    /// A permit that belongs to no budget.
    pub fn detached(bytes: u64) -> Self {
        Self {
            bytes,
            home: None,
            more: Vec::new(),
        }
    }

    /// A permit that returns its bytes to `home`.
    pub fn new(bytes: u64, home: Arc<dyn PermitHome>) -> Self {
        Self {
            bytes,
            home: Some(home),
            more: Vec::new(),
        }
    }

    /// The bytes this permit holds, absorbed permits included.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.more
            .iter()
            .fold(self.bytes, |n, p| n.saturating_add(p.bytes()))
    }

    /// Takes `other` in: its bytes are returned when this permit drops
    /// (merged by home, type docs).
    pub fn absorb(&mut self, mut other: Permit) {
        for p in std::mem::take(&mut other.more) {
            self.absorb(p);
        }
        if other.bytes == 0 {
            // Nothing to return: dropped without a record.
            other.home = None;
            return;
        }
        if same_home(self.home.as_ref(), other.home.as_ref()) {
            self.bytes = self.bytes.saturating_add(other.bytes);
        } else if let Some(kept) = self
            .more
            .iter_mut()
            .find(|p| same_home(p.home.as_ref(), other.home.as_ref()))
        {
            kept.bytes = kept.bytes.saturating_add(other.bytes);
        } else if self.bytes == 0 {
            // An empty permit (a charge's first, detached) takes the home.
            self.home = other.home.take();
            self.bytes = other.bytes;
        } else {
            self.more.push(other);
            return;
        }
        other.bytes = 0;
        other.home = None;
    }

    /// How many records this permit keeps besides its own: one per other
    /// home it absorbed.
    #[must_use]
    pub fn records(&self) -> usize {
        self.more.len()
    }

    /// Returns every byte above `keep` to its budget now.
    pub fn shrink_to(&mut self, keep: u64) {
        let mut excess = self.bytes().saturating_sub(keep);
        while excess > 0 {
            let Some(last) = self.more.last_mut() else {
                break;
            };
            if last.bytes <= excess {
                excess -= last.bytes;
                self.more.pop();
            } else {
                last.release(excess);
                excess = 0;
            }
        }
        if excess > 0 {
            self.release(excess);
        }
    }

    fn release(&mut self, n: u64) {
        let n = n.min(self.bytes);
        if let Some(home) = &self.home {
            home.release(n);
        }
        self.bytes -= n;
    }
}

impl fmt::Debug for Permit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Permit")
            .field("bytes", &self.bytes())
            .field("attached", &self.home.is_some())
            .field("more", &self.more.len())
            .finish()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(home) = self.home.take() {
            home.release(self.bytes);
        }
    }
}

/// A budget that never refuses: UnityPy's behaviour, and the default.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unbounded;

impl MemoryBudget for Unbounded {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        Ok(Permit::detached(bytes))
    }
}

/// A non-blocking budget with a fixed limit, counting what is alive.
#[derive(Debug)]
pub struct Counting {
    limit: u64,
    used: Arc<CountingHome>,
}

#[derive(Debug, Default)]
struct CountingHome {
    used: AtomicU64,
}

impl PermitHome for CountingHome {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl Counting {
    /// A budget of `limit` bytes.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            used: Arc::new(CountingHome::default()),
        }
    }

    /// Bytes reserved and not yet returned.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used.used.load(Ordering::Acquire)
    }
}

impl MemoryBudget for Counting {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let refused = BudgetExceeded {
            requested: bytes,
            what,
        };
        self.used
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.limit)
            })
            .map_err(|_| refused)?;
        Ok(Permit::new(bytes, self.used.clone()))
    }
}

/// The owner of a decoded buffer inside its `Bytes`: the buffer and the
/// charge that paid for it, returned when the last slice drops.
struct Charged {
    buf: Vec<u8>,
    _charge: Charge,
}

impl AsRef<[u8]> for Charged {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

/// What wrapping a buffer in `Bytes` allocates besides the buffer: the
/// shared owner record `Bytes::from_owner` boxes (the buffer, its charge and
/// a reference count), with room to spare.
pub const OWNER_COST: u64 = heap_cost(256);

/// `buf` as `Bytes` that carry `charge`, which must already cover `buf`'s
/// allocation: the owner record is charged first ([`OWNER_COST`]), and the
/// whole charge returns when the last slice drops, whoever holds it.
pub fn charged_bytes(buf: Vec<u8>, mut charge: Charge) -> Result<Bytes, ChargeError> {
    charge.grow(OWNER_COST)?;
    Ok(Bytes::from_owner(Charged {
        buf,
        _charge: charge,
    }))
}

/// A decoder's output buffer that pays before it grows.
///
/// A codec writes a block's output here and nowhere else
/// ([`crate::codec::BlockCodec::decode`]); the core's other byte buffers
/// (a path read, the writer's output) use it too. Every growth first checks
/// the current segment's cap, then charges the new allocation to the input's
/// meter (which books it with the budget), and only then grows; a refused
/// growth allocates nothing. Capacity grows geometrically, and a refused
/// doubling retries at exactly the size needed.
///
/// **Segments.** A run of several blocks is one buffer, but each block has
/// its own cap ([`crate::limits::Limits::max_block_output`]): the caller
/// starts each block with [`Self::begin_block`], and a block that writes past
/// its own cap fails there, whatever the run's size. Every decode is charged
/// (a first decode, a re-decode, a measuring decode): the meter counts what is
/// held, not what was first seen. A block's segment (capped by
/// [`LimitKind::BlockOutput`]) also counts every byte written into it
/// ([`Self::extend_from_slice`], [`Self::grow_zeroed`]) as the decode work
/// of its meter's open ([`crate::limits::Limits::max_decode_work`]), which
/// is never returned, so a block decoded again and again (a failing block
/// read for every entry, a span re-decoded for every range) stops there even
/// though each attempt returns its memory. The count is of writes, not of
/// growths: a buffer sized up front, a scratch buffer reused across decodes
/// and output written in small pieces count every byte (round 6: growths
/// alone were counted, so all three decoded without bound). Room made ahead
/// ([`Self::new`]'s `initial`, [`Self::reserve`]) is memory, charged, and
/// counts as work only when it is written.
///
/// **Drop.** A buffer dropped unfinished (a decode that failed) returns its
/// charge with its allocation.
pub struct DecodeBuf {
    buf: Vec<u8>,
    charge: Charge,
    seg_start: usize,
    seg_cap: u64,
    seg_kind: LimitKind,
    /// Bumped by every [`Self::begin_block`] and every [`Self::truncate`]
    /// below the segment's start, so [`crate::codec::decode_block`] can see
    /// that a codec left its own segment (round 6).
    seg_epoch: u64,
}

impl fmt::Debug for DecodeBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodeBuf")
            .field("len", &self.buf.len())
            .field("capacity", &self.buf.capacity())
            .field("charged", &self.charge.bytes())
            .field("seg_start", &self.seg_start)
            .field("seg_cap", &self.seg_cap)
            .finish_non_exhaustive()
    }
}

impl DecodeBuf {
    /// An empty buffer charging `meter`, with room for `initial` bytes (cut
    /// to `cap`), charged before it is allocated. Its first segment may hold
    /// `cap` bytes; passing that is a `cap_kind` limit error.
    pub fn new(
        meter: &Arc<Meter>,
        initial: u64,
        cap: u64,
        cap_kind: LimitKind,
    ) -> Result<Self, ChargeError> {
        let mut out = Self {
            buf: Vec::new(),
            charge: Charge::zero(meter),
            seg_start: 0,
            seg_cap: cap,
            seg_kind: cap_kind,
            seg_epoch: 0,
        };
        let initial = usize::try_from(initial.min(cap)).unwrap_or(usize::MAX);
        grow_charged(&mut out.buf, &mut out.charge, initial, initial)?;
        Ok(out)
    }

    /// Starts a new segment at the current length (the next block of a run):
    /// from here the buffer may grow by at most `cap` bytes.
    pub fn begin_block(&mut self, cap: u64, cap_kind: LimitKind) {
        self.seg_start = self.buf.len();
        self.seg_cap = cap;
        self.seg_kind = cap_kind;
        self.seg_epoch = self.seg_epoch.wrapping_add(1);
    }

    /// The meter this buffer charges: a codec charges what it holds beside
    /// its output (an LZMA dictionary) to the same input.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// The cap of the current segment ([`Self::begin_block`]'s `cap`).
    #[must_use]
    pub fn segment_cap(&self) -> u64 {
        self.seg_cap
    }

    /// Changes whenever a segment begins or the bytes are cut below the
    /// current segment's start.
    pub(crate) fn segment_epoch(&self) -> u64 {
        self.seg_epoch
    }

    /// The bytes written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// The bytes written in the current segment (since the last
    /// [`Self::begin_block`]).
    #[must_use]
    pub fn segment_len(&self) -> usize {
        self.buf.len().saturating_sub(self.seg_start)
    }

    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The allocation.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// The bytes charged for this buffer: [`heap_cost`] of its capacity.
    #[must_use]
    pub fn charged(&self) -> u64 {
        self.charge.bytes()
    }

    /// The bytes written.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    fn seg_end(&self) -> usize {
        let cap = usize::try_from(self.seg_cap).unwrap_or(usize::MAX);
        self.seg_start.saturating_add(cap)
    }

    /// Makes room for `additional` more bytes: the segment's cap is checked
    /// and the meter charged before the buffer grows. The capacity doubles
    /// while the meter can pay for the old and the new block at once, and
    /// grows to exactly what is needed when it cannot. Room is memory, not
    /// work: what is written into it counts ([`Self::extend_from_slice`]).
    pub fn reserve(&mut self, additional: usize) -> Result<(), ChargeError> {
        self.make_room(additional, 0)
    }

    /// Room for `n` bytes about to be written. In a block's segment (one
    /// capped by [`LimitKind::BlockOutput`]) the `n` bytes are also the
    /// open's decode work, counted in one atomic check-and-add
    /// ([`crate::limits::Meter::count_decode`]) whether or not the buffer
    /// grows: when it grows, once the meter and the budget granted the
    /// growth and before it allocates. Decodes on several threads never pass
    /// the bound together, a write the bound refuses
    /// ([`LimitKind::DecodeWork`]) writes and allocates nothing, and a growth
    /// the meter or the budget refuses counts no work, so a refusal leaves
    /// the input as it was and the same read succeeds once there is room.
    fn room_to_write(&mut self, n: usize) -> Result<(), ChargeError> {
        let work = if self.seg_kind == LimitKind::BlockOutput {
            u64::try_from(n).unwrap_or(u64::MAX)
        } else {
            0
        };
        self.make_room(n, work)
    }

    fn make_room(&mut self, additional: usize, work: u64) -> Result<(), ChargeError> {
        let need = self.buf.len().saturating_add(additional);
        if need > self.seg_end() {
            return Err(ChargeError::Limit(LimitError {
                kind: self.seg_kind,
                value: u64::try_from(need - self.seg_start).unwrap_or(u64::MAX),
                max: self.seg_cap,
            }));
        }
        grow_counted(&mut self.buf, &mut self.charge, need, usize::MAX, work)
    }

    /// Appends `data`.
    pub fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), ChargeError> {
        self.room_to_write(data.len())?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// Appends `n` zero bytes and returns them for a decoder to fill; cut the
    /// unused tail with [`Self::truncate`]. All `n` count as written.
    pub fn grow_zeroed(&mut self, n: usize) -> Result<&mut [u8], ChargeError> {
        self.room_to_write(n)?;
        let start = self.buf.len();
        self.buf.resize(start + n, 0);
        Ok(self.buf.get_mut(start..).unwrap_or_default())
    }

    /// Cuts the written bytes to `len`. The allocation and its charge stay
    /// until [`Self::into_bytes`] or drop.
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(len);
        if self.buf.len() < self.seg_start {
            self.seg_start = self.buf.len();
            self.seg_epoch = self.seg_epoch.wrapping_add(1);
        }
    }

    /// The output as `Bytes` that carry their charge ([`charged_bytes`]).
    /// A buffer with spare capacity is shrunk to its length when the meter
    /// can also pay for the smaller block while the allocator moves it, and
    /// is kept (charged as it is) otherwise.
    pub fn into_bytes(mut self) -> Result<Bytes, ChargeError> {
        let len = self.buf.len();
        let cap = self.buf.capacity();
        if cap > len {
            let bytes = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
            if let Ok(smaller) = self.charge.meter().charge(heap_cost(bytes(len))) {
                self.buf.shrink_to_fit();
                self.charge.release(heap_cost(bytes(cap)));
                self.charge.absorb(smaller);
            }
        }
        charged_bytes(self.buf, self.charge)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Counting, DecodeBuf, MemoryBudget, OWNER_COST, Permit, Reservation, Unbounded};
    use crate::error::LimitKind;
    use crate::limits::{ChargeError, Meter, heap_cost};

    fn meter(limit: u64) -> Arc<Meter> {
        Meter::new(limit, Arc::new(Unbounded), Reservation::Input)
    }

    #[test]
    fn a_permit_lives_as_long_as_its_last_slice() {
        let budget = Arc::new(Counting::new(1000));
        let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let mut out = DecodeBuf::new(&m, 60, 1 << 20, LimitKind::BlockOutput).unwrap();
        out.extend_from_slice(&[7; 60]).unwrap();
        assert_eq!(
            budget.used(),
            heap_cost(60),
            "the meter booked it with the budget"
        );
        let whole = out.into_bytes().unwrap();
        let slice = whole.slice(10..20);
        drop(whole);
        let held = heap_cost(60) + OWNER_COST;
        assert_eq!(budget.used(), held, "the buffer and its owner record");
        assert_eq!(m.used(), held);
        drop(slice);
        assert_eq!((budget.used(), m.used()), (0, 0));
    }

    #[test]
    fn a_decode_is_refused_before_it_outgrows_the_meter() {
        // A block declared at 10 bytes that keeps decoding: the meter holds
        // 200 bytes of buffer, and the owner record wrapping it at the end.
        let limit = 200 + OWNER_COST;
        let m = meter(limit);
        let mut out = DecodeBuf::new(&m, 10, 1 << 30, LimitKind::BlockOutput).unwrap();
        assert_eq!(m.used(), heap_cost(10));
        out.extend_from_slice(&[1; 10]).unwrap();
        let e = out.grow_zeroed(1000).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::InputMemory),
            "{e:?}"
        );
        assert_eq!(out.capacity(), 10, "never allocated past the meter");
        assert_eq!(out.len(), 10);
        assert_eq!(m.used(), heap_cost(10), "a refused growth charges nothing");
        // Growth the meter can pay for goes through, charged as it grows.
        out.extend_from_slice(&[2; 80]).unwrap();
        assert!(m.used() <= limit);
        assert_eq!(m.used(), out.charged());
        let bytes = out.into_bytes().unwrap();
        assert_eq!(
            m.used(),
            heap_cost(90) + OWNER_COST,
            "the charge equals the buffer kept and its owner record"
        );
        drop(bytes);
        assert_eq!(m.used(), 0);
    }

    #[test]
    fn new_charges_before_it_allocates() {
        let m = meter(100);
        let _held = m.charge(90).unwrap();
        let e = DecodeBuf::new(&m, 64 << 20, 1 << 30, LimitKind::BlockOutput).unwrap_err();
        assert!(matches!(e, ChargeError::Limit(_)), "{e:?}");
        assert_eq!(m.used(), 90);
    }

    #[test]
    fn each_block_of_a_run_has_its_own_cap() {
        let m = meter(1 << 20);
        let mut out = DecodeBuf::new(&m, 0, 16, LimitKind::BlockOutput).unwrap();
        out.extend_from_slice(&[0; 16]).unwrap();
        let e = out.extend_from_slice(&[0]).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::BlockOutput && l.max == 16 && l.value == 17),
            "{e:?}"
        );
        // The run's next block may write 16 more, and no further.
        out.begin_block(16, LimitKind::BlockOutput);
        out.extend_from_slice(&[1; 16]).unwrap();
        let before = out.capacity();
        let e = out.grow_zeroed(1).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::BlockOutput),
            "{e:?}"
        );
        assert_eq!(out.capacity(), before, "the cap is checked before growing");
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn a_dropped_decode_returns_its_charge() {
        let m = meter(1 << 20);
        let mut out = DecodeBuf::new(&m, 0, 1 << 20, LimitKind::BlockOutput).unwrap();
        out.extend_from_slice(&[0; 1000]).unwrap();
        assert!(m.used() >= 1000);
        // The codec fails here (a corrupt stream): the buffer is dropped.
        drop(out);
        assert_eq!(m.used(), 0);
    }

    #[test]
    fn a_full_meter_still_fits_data_that_fits() {
        // 100 bytes in two writes (60 then 40) under a meter that holds 100
        // bytes' allocation and no doubling past it.
        let m = meter(heap_cost(60) + heap_cost(100));
        let mut out = DecodeBuf::new(&m, 60, 1 << 20, LimitKind::BlockOutput).unwrap();
        out.extend_from_slice(&[1; 60]).unwrap();
        out.extend_from_slice(&[2; 40]).unwrap();
        assert_eq!(out.len(), 100);
        // Wrapping it needs the owner record, which this meter has no room for.
        assert!(out.into_bytes().is_err());
        assert_eq!(m.used(), 0);
    }

    #[test]
    fn a_decode_repeated_for_every_read_stops_at_the_work_bound() {
        // Round 3: a block that fails after decoding 1 MiB, measured again
        // by every entry's read. Each attempt returns its memory; the work
        // is counted and stops the fourth attempt before it allocates.
        let limits = crate::limits::Limits {
            max_decode_work: 3 << 20,
            ..crate::limits::Limits::default()
        };
        let m = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
        for _ in 0..3 {
            let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
            out.grow_zeroed(1 << 20).unwrap();
            drop(out); // the codec failed
        }
        assert_eq!(m.used(), 0);
        let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        let e = out.grow_zeroed(1 << 20).unwrap_err();
        assert!(
            matches!(&e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork),
            "{e:?}"
        );
        assert_eq!(out.capacity(), 0, "refused before it allocated");
        // A path read or a writer's output is not decode work.
        let mut path = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::InputBytes).unwrap();
        path.grow_zeroed(1 << 20).unwrap();
    }

    #[test]
    fn every_byte_written_counts_whatever_the_buffer_already_holds() {
        // Round 6 (Codex M, claude6 M1): work was counted only on a write
        // that grew the buffer, and only that write's own bytes, so a buffer
        // sized up front (W1), one scratch buffer reused across decodes (W2)
        // and output written in small pieces (W3) decoded without bound.
        let mib = 1usize << 20;
        let work = |max: u64| {
            let limits = crate::limits::Limits {
                max_decode_work: max,
                ..crate::limits::Limits::default()
            };
            Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input)
        };
        let is_work =
            |e: &ChargeError| matches!(e, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork);
        // W1: pre-sized from blocks-info, ten blocks of 1 MiB.
        let m = work(3 << 20);
        let mut done = 0;
        for _ in 0..10 {
            let mut out = DecodeBuf::new(&m, 1 << 20, 1 << 30, LimitKind::BlockOutput).unwrap();
            out.begin_block(1 << 30, LimitKind::BlockOutput);
            match out.grow_zeroed(mib) {
                Ok(_) => done += 1,
                Err(e) => assert!(is_work(&e), "{e:?}"),
            }
        }
        assert_eq!((done, m.decoded()), (3, 3 << 20), "W1");
        // W2: one scratch buffer, truncated and reused for every decode.
        let m = work(3 << 20);
        let mut scratch = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        let mut done = 0;
        for _ in 0..100 {
            scratch.truncate(0);
            scratch.begin_block(1 << 30, LimitKind::BlockOutput);
            match scratch.grow_zeroed(mib) {
                Ok(_) => done += 1,
                Err(e) => assert!(is_work(&e), "{e:?}"),
            }
        }
        assert_eq!((done, m.decoded()), (3, 3 << 20), "W2");
        // W3: 64 MiB written in 4 KiB pieces counts all 64 MiB.
        let m = work(u64::MAX);
        let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        for _ in 0..(64 << 20) / 4096 {
            out.extend_from_slice(&[0; 4096]).unwrap();
        }
        assert_eq!(m.decoded(), 64 << 20, "W3");
        // Room made ahead is memory, not work: only what is written counts.
        let m = work(u64::MAX);
        let mut out = DecodeBuf::new(&m, 1 << 20, 1 << 30, LimitKind::BlockOutput).unwrap();
        out.reserve(1 << 20).unwrap();
        assert_eq!(m.decoded(), 0);
        out.extend_from_slice(&[1; 10]).unwrap();
        assert_eq!(m.decoded(), 10);
    }

    #[test]
    fn a_refused_growth_counts_no_work_and_the_read_succeeds_once_there_is_room() {
        // Round 4: decode work was counted before the growth was granted, so
        // refusals that decoded nothing used up the allowance, and the
        // transient refusal became a permanent `DecodeWork`.
        let limits = crate::limits::Limits {
            max_decode_work: 4 << 20,
            ..crate::limits::Limits::default()
        };
        let pool = Arc::new(Counting::new(2 << 20));
        let m = Meter::for_input(&limits, pool.clone(), Reservation::Dependency);
        // Another job holds all of the pool but 64 KiB.
        let other = Meter::new(u64::MAX, pool.clone(), Reservation::Input);
        let held = other.charge((2 << 20) - (64 << 10)).unwrap();
        for _ in 0..10 {
            // Ten 1 MiB refusals: more than twice the work bound.
            let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
            let e = out.grow_zeroed(1 << 20).unwrap_err();
            assert!(
                matches!(&e, ChargeError::Budget(b) if b.what == Reservation::Dependency),
                "{e:?}"
            );
            assert!(e.is_transient());
        }
        assert_eq!(m.decoded(), 0, "a refused growth decodes nothing");
        drop(held);
        // The pool has room again: the same decode succeeds, and counts.
        let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        out.grow_zeroed(1 << 20).unwrap();
        assert_eq!(m.decoded(), 1 << 20);
        drop(out);
        assert_eq!((m.used(), pool.used()), (0, 0));
    }

    #[test]
    fn decodes_on_several_threads_never_pass_the_work_bound_together() {
        // Round 5 (Codex M1): the bound was checked before a growth and
        // counted after it, so threads that checked at once all passed and
        // all counted. This budget holds each thread at its first
        // reservation until all eight have arrived (a test budget may block;
        // a real one must not), so every thread has checked the bound before
        // any of them counts: eight 1 MiB growths against 4 MiB of work.
        use std::sync::atomic::{AtomicUsize, Ordering};
        #[derive(Debug)]
        struct Gate {
            barrier: std::sync::Barrier,
            arrived: AtomicUsize,
            threads: usize,
        }
        impl MemoryBudget for Gate {
            fn try_reserve(
                &self,
                bytes: u64,
                _what: Reservation,
            ) -> Result<Permit, super::BudgetExceeded> {
                if self.arrived.fetch_add(1, Ordering::AcqRel) < self.threads {
                    self.barrier.wait();
                }
                Ok(Permit::detached(bytes))
            }
        }
        const THREADS: usize = 8;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let limits = crate::limits::Limits {
                max_decode_work: 4 << 20,
                ..crate::limits::Limits::default()
            };
            let gate = Arc::new(Gate {
                barrier: std::sync::Barrier::new(THREADS),
                arrived: AtomicUsize::new(0),
                threads: THREADS,
            });
            let m = Meter::for_input(&limits, gate, Reservation::Input);
            let outcomes: Vec<_> = (0..THREADS)
                .map(|_| {
                    let m = m.clone();
                    std::thread::spawn(move || {
                        let mut out = DecodeBuf::new(&m, 0, 1 << 30, LimitKind::BlockOutput)?;
                        out.grow_zeroed(1 << 20)?;
                        Ok::<_, ChargeError>(out)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect();
            let granted = outcomes.iter().filter(|o| o.is_ok()).count();
            let refused = outcomes
                .iter()
                .filter(
                    |o| matches!(o, Err(ChargeError::Limit(l)) if l.kind == LimitKind::DecodeWork),
                )
                .count();
            let held = m.used();
            let decoded = m.decoded();
            drop(outcomes);
            let _ = tx.send((granted, refused, decoded, held, m.used()));
        });
        let (granted, refused, decoded, held, after) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the growths did not finish");
        assert_eq!(decoded, 4 << 20, "the work bound held");
        assert_eq!((granted, refused), (4, 4));
        assert_eq!(
            held,
            4 * heap_cost(1 << 20),
            "a refused growth kept no charge"
        );
        assert_eq!(after, 0);
    }

    #[test]
    fn permits_merge_by_home() {
        let a = Counting::new(1000);
        let b = Counting::new(1000);
        // A charge's first permit is detached and empty: it takes the home.
        let mut p = Permit::detached(0);
        for _ in 0..100 {
            p.absorb(a.try_reserve(1, Reservation::Input).unwrap());
            p.absorb(Permit::detached(0));
            p.absorb(b.try_reserve(2, Reservation::Input).unwrap());
        }
        assert_eq!(p.records(), 1, "one record for the second home");
        assert_eq!((p.bytes(), a.used(), b.used()), (300, 100, 200));
        drop(p);
        assert_eq!((a.used(), b.used()), (0, 0));
    }

    #[test]
    fn absorbed_permits_from_another_home_still_return() {
        let a = Counting::new(100);
        let b = Counting::new(100);
        let mut p = a.try_reserve(10, Reservation::Input).unwrap();
        p.absorb(a.try_reserve(5, Reservation::Input).unwrap());
        p.absorb(b.try_reserve(7, Reservation::Input).unwrap());
        p.absorb(Permit::detached(3));
        assert_eq!(p.bytes(), 25);
        assert_eq!((a.used(), b.used()), (15, 7));
        p.shrink_to(12);
        assert_eq!(p.bytes(), 12);
        assert_eq!((a.used(), b.used()), (12, 0));
        drop(p);
        assert_eq!((a.used(), b.used()), (0, 0));
    }
}
