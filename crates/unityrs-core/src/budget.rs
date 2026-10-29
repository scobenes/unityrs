//! Memory budgets: the mechanism in the core, the policy in the batch engine.
//!
//! A budget is shared by many inputs (a batch's admission pool, its
//! dependency pool). The core never asks it directly: every charge an
//! input's [`crate::limits::Meter`] grants is also booked here, as a
//! [`Permit`] that rides inside the charge, so the budget sees everything
//! every input holds, and gets it back when the last owner drops it. The
//! request never blocks: blocking inside a rayon pool can deadlock, so the
//! batch engine does its blocking admission *before* it hands work to the
//! pool, and gives core a prepaid budget (plan section 4.1).
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
use crate::limits::{Charge, ChargeError, Meter, grow_charged, heap_cost};

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

/// A source of permits. Implementations must not block.
pub trait MemoryBudget: Send + Sync + fmt::Debug {
    /// Reserves `bytes`, or refuses at once.
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded>;
}

/// Where a permit returns its bytes.
pub trait PermitHome: Send + Sync {
    /// Takes `bytes` back.
    fn release(&self, bytes: u64);
}

/// Bytes reserved from a budget, returned when the permit drops.
///
/// A permit can take in another ([`Permit::absorb`]), so a buffer that grew
/// past its first reservation carries every top-up in one owner, and can give
/// back what its buffer no longer holds ([`Permit::shrink_to`]).
#[must_use = "a permit returns its bytes when dropped"]
pub struct Permit {
    bytes: u64,
    home: Option<Arc<dyn PermitHome>>,
    /// Absorbed permits whose home differs from this one's.
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

    /// Takes `other` in: its bytes are returned when this permit drops.
    pub fn absorb(&mut self, mut other: Permit) {
        self.more.append(&mut other.more);
        if same_home(self.home.as_ref(), other.home.as_ref()) {
            self.bytes = self.bytes.saturating_add(other.bytes);
            other.bytes = 0;
            other.home = None;
        } else {
            self.more.push(other);
        }
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
/// held, not what was first seen.
///
/// **Drop.** A buffer dropped unfinished (a decode that failed) returns its
/// charge with its allocation.
pub struct DecodeBuf {
    buf: Vec<u8>,
    charge: Charge,
    seg_start: usize,
    seg_cap: u64,
    seg_kind: LimitKind,
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
    }

    /// The bytes written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
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
    /// grows to exactly what is needed when it cannot.
    pub fn reserve(&mut self, additional: usize) -> Result<(), ChargeError> {
        let need = self.buf.len().saturating_add(additional);
        if need > self.seg_end() {
            return Err(ChargeError::Limit(LimitError {
                kind: self.seg_kind,
                value: u64::try_from(need - self.seg_start).unwrap_or(u64::MAX),
                max: self.seg_cap,
            }));
        }
        grow_charged(&mut self.buf, &mut self.charge, need, usize::MAX)
    }

    /// Appends `data`.
    pub fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), ChargeError> {
        self.reserve(data.len())?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// Appends `n` zero bytes and returns them for a decoder to fill; cut the
    /// unused tail with [`Self::truncate`].
    pub fn grow_zeroed(&mut self, n: usize) -> Result<&mut [u8], ChargeError> {
        self.reserve(n)?;
        let start = self.buf.len();
        self.buf.resize(start + n, 0);
        Ok(self.buf.get_mut(start..).unwrap_or_default())
    }

    /// Cuts the written bytes to `len`. The allocation and its charge stay
    /// until [`Self::into_bytes`] or drop.
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(len);
        self.seg_start = self.seg_start.min(self.buf.len());
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
