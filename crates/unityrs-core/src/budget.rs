//! Memory budgets: the mechanism in the core, the policy in the batch engine.
//!
//! Before core allocates a buffer whose size the input chose (a decoded block
//! run, an inflated stream), it asks the file's [`MemoryBudget`] for a
//! [`Permit`]. The request never blocks: blocking inside a rayon pool can
//! deadlock, so the batch engine does its blocking admission *before* it hands
//! work to the pool, and gives core a prepaid budget (plan section 4.1).
//!
//! A permit rides inside the buffer it paid for ([`budgeted_bytes`]), so the
//! bytes are returned only when the last `Bytes` slice of that buffer drops,
//! whoever holds it, and it covers the buffer's whole capacity. A decoder
//! writes through a [`DecodeBuf`], which reserves before every growth, so no
//! decoded byte exists unpaid. The budget pays for decoded and inflated
//! buffers only; parsed metadata, typetree rows and values are bounded by
//! [`crate::limits::Limits`] instead (plan section 3.15).
//!
//! Owner: the scaffold (implemented); package C maintains it.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;

use crate::error::{Error, LimitError, LimitKind};
use crate::limits::{Limits, Meter, Metered};

/// What a reservation pays for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reservation {
    /// A bundle's decoded block run.
    BundleData,
    /// An inflated gzip or brotli stream.
    Inflate,
    /// A file loaded only as a dependency (the environment's LRU).
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

struct Budgeted {
    buf: Vec<u8>,
    _permit: Permit,
}

impl AsRef<[u8]> for Budgeted {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

/// Turns a buffer and the permit that paid for it into `Bytes`, with books
/// equal to the allocation: the buffer is shrunk to its length, then the
/// permit is topped up from `budget` to the buffer's capacity (refused: the
/// buffer is dropped and [`BudgetExceeded`] returned) or trimmed to it. The
/// permit is released when the last slice of the buffer drops.
pub fn budgeted_bytes(
    mut buf: Vec<u8>,
    mut permit: Permit,
    budget: &dyn MemoryBudget,
    what: Reservation,
) -> Result<Bytes, BudgetExceeded> {
    buf.shrink_to_fit();
    let cap = u64::try_from(buf.capacity()).unwrap_or(u64::MAX);
    let held = permit.bytes();
    if held < cap {
        permit.absorb(budget.try_reserve(cap - held, what)?);
    } else {
        permit.shrink_to(cap);
    }
    Ok(Bytes::from_owner(Budgeted {
        buf,
        _permit: permit,
    }))
}

/// A decoder's output buffer that pays before it grows.
///
/// A codec writes a block's output here and nowhere else
/// ([`crate::codec::BlockCodec::decode`]). Every growth first checks the
/// hard cap (the smaller of [`crate::limits::Limits::max_block_output`] and,
/// with [`Self::with_meter`], what the input may still decode) and reserves
/// the new capacity from the budget; only then does the buffer grow. So an
/// LZMA block that declares 1 byte and decodes towards its cap is refused at
/// the first growth past what the budget or the limits allow, before the
/// bytes exist, and the books always cover the allocation. Capacity grows
/// geometrically up to the cap (amortised O(1) writes).
pub struct DecodeBuf<'a> {
    buf: Vec<u8>,
    permit: Permit,
    budget: &'a dyn MemoryBudget,
    what: Reservation,
    cap: u64,
    cap_kind: LimitKind,
    meter: Option<(&'a Meter, &'a Limits)>,
    /// Bytes charged to the meter: the largest length reached, less refunds.
    charged: u64,
}

impl fmt::Debug for DecodeBuf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodeBuf")
            .field("len", &self.buf.len())
            .field("capacity", &self.buf.capacity())
            .field("booked", &self.permit.bytes())
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

impl<'a> DecodeBuf<'a> {
    /// An empty buffer with room for `initial` bytes (cut to `cap`), reserved
    /// from `budget` first. `cap` is the most it may ever hold; passing it is
    /// a [`LimitKind`] `cap_kind` error.
    pub fn new(
        budget: &'a dyn MemoryBudget,
        what: Reservation,
        initial: u64,
        cap: u64,
        cap_kind: LimitKind,
    ) -> Result<Self, Error> {
        let mut out = Self {
            buf: Vec::new(),
            permit: Permit::detached(0),
            budget,
            what,
            cap,
            cap_kind,
            meter: None,
            charged: 0,
        };
        out.grow_capacity(initial.min(cap))?;
        Ok(out)
    }

    /// Also charges every byte the buffer's length reaches to `meter`'s
    /// [`Metered::DecodedBytes`] (the first decode of a block; a block whose
    /// actual length is already known decodes without it).
    #[must_use]
    pub fn with_meter(mut self, meter: &'a Meter, limits: &'a Limits) -> Self {
        self.meter = Some((meter, limits));
        self
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

    /// The bytes the budget has booked for this buffer (never below
    /// [`Self::capacity`]).
    #[must_use]
    pub fn booked(&self) -> u64 {
        self.permit.bytes()
    }

    /// The bytes written.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    fn cap_error(&self, value: u64) -> Error {
        Error::Limit(LimitError {
            kind: self.cap_kind,
            value,
            max: self.cap,
        })
    }

    fn grow_capacity(&mut self, want: u64) -> Result<(), Error> {
        let have = u64::try_from(self.buf.capacity()).unwrap_or(u64::MAX);
        if want <= have {
            return Ok(());
        }
        if want > self.cap {
            return Err(self.cap_error(want));
        }
        let booked = self.permit.bytes();
        if want > booked {
            self.permit
                .absorb(self.budget.try_reserve(want - booked, self.what)?);
        }
        let len = u64::try_from(self.buf.len()).unwrap_or(u64::MAX);
        let extra = usize::try_from(want - len).map_err(|_| self.cap_error(want))?;
        self.buf
            .try_reserve_exact(extra)
            .map_err(|_| BudgetExceeded {
                requested: want,
                what: self.what,
            })?;
        // The allocator may round up; book what it gave.
        let got = u64::try_from(self.buf.capacity()).unwrap_or(u64::MAX);
        let booked = self.permit.bytes();
        if got > booked {
            match self.budget.try_reserve(got - booked, self.what) {
                Ok(p) => self.permit.absorb(p),
                Err(e) => {
                    self.buf.shrink_to(self.buf.len());
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }

    /// Makes room for `additional` more bytes: the cap is checked and the
    /// budget pays before the buffer grows.
    pub fn reserve(&mut self, additional: usize) -> Result<(), Error> {
        let len = u64::try_from(self.buf.len()).unwrap_or(u64::MAX);
        let need = len.saturating_add(u64::try_from(additional).unwrap_or(u64::MAX));
        let have = u64::try_from(self.buf.capacity()).unwrap_or(u64::MAX);
        if need <= have {
            return Ok(());
        }
        if need > self.cap {
            return Err(self.cap_error(need));
        }
        let want = need.max(have.saturating_mul(2)).min(self.cap);
        self.grow_capacity(want)
    }

    fn charge_to(&mut self, len: u64) -> Result<(), Error> {
        if let Some((meter, limits)) = self.meter
            && len > self.charged
        {
            meter.charge(Metered::DecodedBytes, len - self.charged, limits)?;
            self.charged = len;
        }
        Ok(())
    }

    /// Appends `data`.
    pub fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), Error> {
        self.reserve(data.len())?;
        let len = u64::try_from(self.buf.len() + data.len()).unwrap_or(u64::MAX);
        self.charge_to(len)?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// Appends `n` zero bytes and returns them for a decoder to fill; cut the
    /// unused tail with [`Self::truncate`].
    pub fn grow_zeroed(&mut self, n: usize) -> Result<&mut [u8], Error> {
        self.reserve(n)?;
        let start = self.buf.len();
        let len = u64::try_from(start + n).unwrap_or(u64::MAX);
        self.charge_to(len)?;
        self.buf.resize(start + n, 0);
        Ok(self.buf.get_mut(start..).unwrap_or_default())
    }

    /// Cuts the written bytes to `len`, refunding the meter what it was
    /// charged past `len`. The allocation and its booking stay until
    /// [`Self::into_bytes`].
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(len);
        let len = u64::try_from(self.buf.len()).unwrap_or(u64::MAX);
        if let Some((meter, _)) = self.meter
            && self.charged > len
        {
            meter.refund(Metered::DecodedBytes, self.charged - len);
            self.charged = len;
        }
    }

    /// The output as `Bytes` that carry their booking ([`budgeted_bytes`]).
    pub fn into_bytes(self) -> Result<Bytes, Error> {
        Ok(budgeted_bytes(
            self.buf,
            self.permit,
            self.budget,
            self.what,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Counting, DecodeBuf, MemoryBudget, Permit, Reservation, Unbounded, budgeted_bytes,
    };
    use crate::error::{Error, LimitKind};
    use crate::limits::{Limits, Meter, Metered};

    #[test]
    fn a_permit_lives_as_long_as_its_last_slice() {
        let budget = Counting::new(100);
        let permit = budget.try_reserve(60, Reservation::BundleData).unwrap();
        assert!(budget.try_reserve(50, Reservation::BundleData).is_err());
        let whole = budgeted_bytes(vec![7; 60], permit, &budget, Reservation::BundleData).unwrap();
        let slice = whole.slice(10..20);
        drop(whole);
        assert_eq!(budget.used(), 60);
        drop(slice);
        assert_eq!(budget.used(), 0);
        assert!(budget.try_reserve(100, Reservation::Inflate).is_ok());
    }

    #[test]
    fn budgeted_bytes_books_the_allocation_not_the_permit() {
        let budget = Counting::new(1 << 20);
        // An underpaid buffer is topped up to its capacity.
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(b"eleven byte");
        let permit = budget.try_reserve(10, Reservation::BundleData).unwrap();
        let b = budgeted_bytes(buf, permit, &budget, Reservation::BundleData).unwrap();
        assert_eq!(budget.used(), 11, "shrunk to its length, then booked whole");
        drop(b);
        assert_eq!(budget.used(), 0);
        // An overpaid permit gives back the excess at once.
        let permit = budget.try_reserve(1000, Reservation::BundleData).unwrap();
        let b = budgeted_bytes(vec![1; 100], permit, &budget, Reservation::BundleData).unwrap();
        assert_eq!(budget.used(), 100);
        drop(b);
        // A 1 MiB buffer paid by a 1-byte permit is refused when the budget cannot top it up.
        let small = Counting::new(1000);
        let permit = small.try_reserve(1, Reservation::BundleData).unwrap();
        let r = budgeted_bytes(vec![0; 1 << 20], permit, &small, Reservation::BundleData);
        assert!(r.is_err());
        assert_eq!(small.used(), 0);
    }

    #[test]
    fn a_decode_is_refused_before_it_outgrows_the_budget() {
        // A block declared at 10 bytes that keeps decoding: the budget holds 100.
        let budget = Counting::new(100);
        let mut out = DecodeBuf::new(
            &budget,
            Reservation::BundleData,
            10,
            1 << 30,
            LimitKind::BlockOutput,
        )
        .unwrap();
        assert_eq!(budget.used(), 10);
        out.extend_from_slice(&[1; 10]).unwrap();
        let e = out.grow_zeroed(200).unwrap_err();
        assert!(matches!(e, Error::Budget(_)), "{e:?}");
        assert!(out.capacity() <= 100, "never allocated past the budget");
        assert!(out.booked() >= u64::try_from(out.capacity()).unwrap());
        assert_eq!(out.len(), 10);
        // Growth the budget can pay for goes through, booked as it grows.
        out.extend_from_slice(&[2; 80]).unwrap();
        assert!(budget.used() <= 100);
        assert_eq!(budget.used(), out.booked());
        let bytes = out.into_bytes().unwrap();
        assert_eq!(budget.used(), 90, "the books equal the buffer kept");
        drop(bytes);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_decode_stops_at_its_cap_and_at_the_inputs_meter() {
        let mut out = DecodeBuf::new(
            &Unbounded,
            Reservation::BundleData,
            4,
            16,
            LimitKind::BlockOutput,
        )
        .unwrap();
        out.extend_from_slice(&[0; 16]).unwrap();
        let e = out.extend_from_slice(&[0]).unwrap_err();
        assert!(
            matches!(e, Error::Limit(l) if l.kind == LimitKind::BlockOutput && l.max == 16),
            "{e:?}"
        );

        let limits = Limits {
            max_bundle_uncompressed: 50,
            ..Limits::default()
        };
        let meter = Meter::new();
        meter.charge(Metered::DecodedBytes, 40, &limits).unwrap();
        let mut out = DecodeBuf::new(
            &Unbounded,
            Reservation::BundleData,
            0,
            1 << 20,
            LimitKind::BlockOutput,
        )
        .unwrap()
        .with_meter(&meter, &limits);
        let filled = out.grow_zeroed(8).unwrap();
        assert_eq!(filled.len(), 8);
        out.truncate(5);
        assert_eq!(
            meter.used(Metered::DecodedBytes),
            45,
            "a cut tail is refunded"
        );
        let e = out.grow_zeroed(6).unwrap_err();
        assert!(
            matches!(e, Error::Limit(l) if l.kind == LimitKind::BundleUncompressed),
            "{e:?}"
        );
        assert_eq!(meter.used(Metered::DecodedBytes), 45);
    }

    #[test]
    fn absorbed_permits_from_another_home_still_return() {
        let a = Counting::new(100);
        let b = Counting::new(100);
        let mut p = a.try_reserve(10, Reservation::BundleData).unwrap();
        p.absorb(a.try_reserve(5, Reservation::BundleData).unwrap());
        p.absorb(b.try_reserve(7, Reservation::BundleData).unwrap());
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
