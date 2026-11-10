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
use crate::limits::{
    Charge, ChargeError, Meter, grow_charged, grow_counted, heap_cost, shrink_charged,
};

/// Which kind of load a charge belongs to, so a refusal says which pool
/// refused (plan section 4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reservation {
    /// An input the caller loaded: everything its meter charges.
    Input,
    /// A file the environment loaded as a dependency (its LRU), and
    /// everything that file's meter charges.
    Dependency,
    /// A typetree provider's own data (a TPK's trees), which no input owns:
    /// a refusal of it is the allocator's, on whichever input's read asked.
    Provider,
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
/// growth allocates nothing. Capacity grows geometrically, never past the
/// current segment's end (its start and its cap: room no write may use is
/// never allocated, review round 13), nor, while what is needed fits it,
/// past the run's declared end: the block's start, the size
/// [`Self::begin_block`] was given, and what the run's next blocks declare
/// as far as its reader has read ahead ([`Self::set_ahead`]). Room a
/// well-formed run never writes is never allocated, so a block decoded in
/// windows ends at its declared size, and a run of blocks doubles, ending
/// at its last block's end. A refused doubling is asked again with half the
/// room past the size needed, down to exactly that size
/// ([`crate::limits::MeteredVec`]'s growth). A block that writes past that
/// declared end (an LZMA stream that ignores its size) grows geometrically
/// from there, to the segment's end.
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
/// counts as work only when it is written. Work counted ahead for what a
/// decode reads (`prepay`, the bundle reader's) is credit its writes use
/// first.
///
/// **Drop.** A buffer dropped unfinished (a decode that failed) returns its
/// charge with its allocation.
pub struct DecodeBuf {
    buf: Vec<u8>,
    charge: Charge,
    seg_start: usize,
    seg_cap: u64,
    seg_kind: LimitKind,
    /// The segment's declared size ([`Self::begin_block`]'s `declared`;
    /// the cap for a buffer's first segment): room grows past it, and past
    /// `ahead`, only for a write that needs it.
    seg_declared: u64,
    /// What the run's blocks after this one declare ([`Self::set_ahead`]).
    ahead: u64,
    /// Bumped by every [`Self::begin_block`] and every [`Self::truncate`]
    /// below the segment's start, so [`crate::codec::decode_block`] can see
    /// that a codec left its own segment (round 6).
    seg_epoch: u64,
    /// Work counted ahead ([`Self::prepay`]) that no write has used yet.
    prepaid: u64,
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
            seg_declared: cap,
            ahead: 0,
            seg_epoch: 0,
            prepaid: 0,
        };
        let initial = usize::try_from(initial.min(cap)).unwrap_or(usize::MAX);
        grow_charged(&mut out.buf, &mut out.charge, initial, initial)?;
        Ok(out)
    }

    /// Starts a new segment at the current length (the next block of a run):
    /// from here the buffer may grow by at most `cap` bytes, and its room
    /// grows past `declared` bytes (the block's declared size, a growth
    /// hint) only for a write that needs it (type docs).
    pub fn begin_block(&mut self, cap: u64, cap_kind: LimitKind, declared: u64) {
        self.seg_start = self.buf.len();
        self.seg_cap = cap;
        self.seg_kind = cap_kind;
        self.seg_declared = declared;
        self.seg_epoch = self.seg_epoch.wrapping_add(1);
    }

    /// What the run's blocks after the next one declare, `bytes` in all, as
    /// far as its reader reads ahead: room may grow that far past the next
    /// block's declared end (type docs), so a run of many blocks doubles its
    /// room rather than growing it to each block's end in turn (perf stage
    /// 2's review: 64 MiB in 4,096 blocks reallocated 4,096 times). It holds
    /// until it is set again; a buffer never given it reads no further than
    /// each block's own end.
    pub(crate) fn set_ahead(&mut self, bytes: u64) {
        self.ahead = bytes;
    }

    /// Counts `n` bytes as its meter's open's work now, in one checked add
    /// ([`crate::limits::Meter::count_decode`]), before what they pay for
    /// runs, and keeps them as credit for the next `n` bytes written into a
    /// block's segment, which then count nothing more. The bundle reader
    /// prepays a block's stored bytes before a codec reads them, so a decode
    /// counts the larger of what it reads and what it writes, and one that
    /// reads its whole block and fails before it writes still counts its
    /// read (review round 6: an LZ4 scan that failed counted nothing). A
    /// refusal ([`LimitKind::DecodeWork`]) counts nothing and leaves no
    /// credit.
    pub(crate) fn prepay(&mut self, n: u64) -> Result<(), ChargeError> {
        self.charge.meter().count_decode(n)?;
        self.prepaid = self.prepaid.saturating_add(n);
        Ok(())
    }

    /// Ends [`Self::prepay`]'s credit: what no write used, which stays
    /// counted unless the caller gives it back.
    pub(crate) fn end_prepaid(&mut self) -> u64 {
        std::mem::take(&mut self.prepaid)
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
    /// and the meter charged before the buffer grows. The capacity doubles,
    /// up to the segment's end (and to the run's declared end while what is
    /// needed fits it), while the meter can pay for the old and the new
    /// block at once, and grows to exactly what is needed when it cannot.
    /// Room is memory, not work: what is written into it counts
    /// ([`Self::extend_from_slice`]).
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
    /// Work already counted ahead ([`Self::prepay`]) pays for the bytes
    /// first, and is used only by a write that is made.
    fn room_to_write(&mut self, n: usize) -> Result<(), ChargeError> {
        let work = if self.seg_kind == LimitKind::BlockOutput {
            u64::try_from(n).unwrap_or(u64::MAX)
        } else {
            0
        };
        let credit = work.min(self.prepaid);
        self.make_room(n, work - credit)?;
        self.prepaid -= credit;
        Ok(())
    }

    fn make_room(&mut self, additional: usize, work: u64) -> Result<(), ChargeError> {
        let need = self.buf.len().saturating_add(additional);
        let end = self.seg_end();
        if need > end {
            return Err(ChargeError::Limit(LimitError {
                kind: self.seg_kind,
                value: u64::try_from(need - self.seg_start).unwrap_or(u64::MAX),
                max: self.seg_cap,
            }));
        }
        // Review round 13: the doubling was not capped, so a block whose
        // output ended just past a power of two of its first window held
        // room up to twice its cap (a TPK's LZMA payload of 16 MiB, 32 MiB),
        // which a later charge (an LZMA retry's dictionary) then had to fit
        // beside. Perf stage 2 (plan section 3.15): it still passed the
        // block's declared size, so an LZMA block of 69.4 MB decoded in
        // 1 MiB windows held 128 MiB of room, charged, and a run of LZ4
        // blocks doubled past its last block.
        let declared = self.seg_declared.saturating_add(self.ahead);
        let declared = usize::try_from(declared).unwrap_or(usize::MAX);
        let hint = self.seg_start.saturating_add(declared).min(end);
        let most = if need <= hint { hint } else { end };
        grow_counted(&mut self.buf, &mut self.charge, need, most, work)
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
    /// A buffer with spare capacity is shrunk to its length, its charge cut
    /// to the smaller block's ([`crate::limits::MeteredVec::shrink_to_fit`]'s
    /// rule): in place, charging nothing more (perf stage 2: the smaller
    /// block was charged beside the old one, which an in-place shrink never
    /// holds); or, for a block that may be mapped given back below 128 KiB
    /// ([`crate::limits::shrink_moves`]; review round 13: 10 bytes cut from
    /// room of 1 MiB, as an LZMA block's first window leaves a short block,
    /// held a page against a charge of 32 bytes), moved to a new block when
    /// the meter can also pay for it while the bytes move, and kept (charged
    /// as it is) when the meter or the allocator refuses that block.
    pub fn into_bytes(mut self) -> Result<Bytes, ChargeError> {
        // A refused move keeps the buffer as it is.
        let _ = shrink_charged(&mut self.buf, &mut self.charge);
        charged_bytes(self.buf, self.charge)
    }
}

#[cfg(test)]
mod tests;
