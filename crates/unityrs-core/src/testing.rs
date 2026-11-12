//! The core's test-only counters (`docs/plans/batch.md` section 6, CR8),
//! built only under the `test-counters` feature, which the testkit's feature
//! of the same name enables. A build without the feature has none of this:
//! the core's side of it (`counters`) is then unit structs and empty
//! functions (plan section 4.1, the property *CR8's counters cost a build
//! without the feature nothing*).
//!
//! **Refusals.** Process-wide counters the core only writes, read with
//! [`refusals`] as a difference (before and after a run, in a process of
//! the test's own: every thread's refusals land in them):
//! - each [`LimitKind`] refusal, once per `LimitError` built: its one
//!   constructor, `LimitError::refused`, counts (`Meter::work_refused`, for
//!   an open whose work has passed its allowance, builds through it),
//!   whether a caller returns it or a path absorbs it (a growth's retry at
//!   a smaller size, a cache publish that keeps its run uncached); a kept
//!   failure answered again, or a copy of a refusal returned again, counts
//!   nothing, and neither does [`crate::limits::Meter::work_used_up`], a
//!   query whose answer, a `WorkPast`, is no error. One event can be two
//!   refusals: a lazy computation the input's meter refuses whose grants
//!   take the open's work past its allowance (a registration's reach, `env::registry`, and the container pass's
//!   walk of a preload table, `env::container_index`) counts the meter's
//!   [`LimitKind::InputMemory`], which it absorbs, and the
//!   [`LimitKind::DecodeWork`] it returns in its place;
//! - each refusal a [`crate::budget::MemoryBudget`] returns to a meter, by
//!   the [`Reservation`] it names;
//! - each refusal of the allocator the core meets, by its meter's
//!   [`Reservation`]: a `try_reserve_exact` of the core's two marked sites,
//!   the fallible helper (`limits::vec::reserve_exact`, which every charged
//!   allocation but one goes through) and the move of a buffer given back
//!   its room (`limits::vec::shrink_charged`), and liblzma's memory error.
//!   Both sites mark their thread around that call
//!   ([`in_fallible_reserve`]), so a test's allocator can refuse that
//!   allocation alone (the engine's `point` hook's `refuse`).
//!
//! **Span reads.** Each read of a bundle's joined data (`read_span_pinned`,
//! `files::bundle`), which an entry's parse, `entry_range` and
//! `entry_bytes` make with their row and `load_all` with the whole file, is
//! counted once, keyed by the **load id** of the bundle it reads and the
//! row's **index path**: the directory indexes from the load's top-level
//! file to the row (`Rank::path`, `env::registry`, for a registered row). A
//! load draws its id once, from one process-wide counter, before its first
//! attempt (`Environment::load`, a dependency's load and a resolver's,
//! before `with_room`, so a retry keeps it), records it with the name it
//! gives the core in the load-name table, and its dependency keeps it for
//! every reopen; a nested bundle takes its container's id and its row's
//! path. A bundle opened outside a load (`parse_file` or
//! [`crate::files::BundleFile::open`] called directly, by a test or by a
//! caller's code the core runs, a fetcher, a codec or a provider) is read
//! under no load id, the **bare** key ([`SpanRead::load`] `None`). A cached
//! entry that answers again reads nothing, as does a twin's kept failure,
//! and the blocks-info a bundle decodes at its open is no span read.
//!
//! **The key.** A bundle's key is given to it, not inherited: the core's
//! own opens, a load's of its source and an entry's parse, give the bundle
//! they made its key when the parse returns, before anything reads it, and
//! a bundle no such open made keeps the bare key it was opened with. No
//! thread-local scope carries a key, so a bundle a caller's code opens
//! while the core runs it is bare (round 2: a scope around the core's parse
//! also covered the codecs it ran). A nested bundle's key is its load's id
//! and a node of its own, which links to its container's node and names
//! its row; the nodes hold no bundle, so a read is counted at its path
//! however long the bundles above it live, and nothing a read touches is
//! freed under the tables' lock (round 2: the path was found by walking up
//! to the bundles themselves).
//!
//! **The tables.** [`reserve_span_reads`] preallocates a load-name table and
//! a span-read table; until then the core records no name and no read and
//! allocates nothing for them. A key's node is allocated at the open of a
//! nested bundle while a table is reserved, and at no other time; a bundle
//! opened while no table is reserved is never counted, nor anything nested
//! in it, so a test reserves before the load it counts. Recording never
//! allocates: a read's index path is built in the table's own room from
//! its key's nodes, so any path the reserved width holds is counted. A
//! full table, a name longer than [`NAME_BYTES`] or a path deeper than the
//! table's width sets a sticky overflow flag ([`SpanReads::overflowed`]),
//! which fails the test reading it, so a count is never silently dropped.
//! [`span_reads`] reads both.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::budget::Reservation;
use crate::error::{Error, LimitKind};
use crate::files::File;
use crate::raw::RawStr;

/// `$index`, the counter index of each `$ty` value, by an exhaustive match
/// built from one list, and `$count`, the list's length: a variant added to
/// `$ty` and not to the list fails to build, so no refusal of it can fall
/// outside the counters (round 1: the count was a literal 11 beside a
/// hand-written match).
macro_rules! counter_index {
    ($index:ident, $count:ident, $ty:ident, $order:ident, [$($v:ident),+ $(,)?]) => {
        #[allow(clippy::enum_variant_names)]
        #[repr(usize)]
        enum $order {
            $($v),+
        }

        /// How many counters there are, one a variant.
        const $count: usize = [$($order::$v),+].len();

        const fn $index(x: $ty) -> usize {
            match x {
                $($ty::$v => $order::$v as usize),+
            }
        }
    };
}

counter_index!(
    kind_index,
    LIMIT_KINDS,
    LimitKind,
    KindOrder,
    [
        InputMemory,
        InputBytes,
        BlockOutput,
        DecodeWork,
        Inflate,
        ContainerDepth,
        TreeDepth,
        ValueDepth,
        ValuesPerObject,
        ZeroSizeElements,
        Count,
    ]
);

counter_index!(
    reservation_index,
    RESERVATIONS,
    Reservation,
    ReservationOrder,
    [Input, Dependency, Provider]
);

/// The longest name the load-name table records; a longer one sets the
/// overflow flag (`PATH_MAX`).
pub const NAME_BYTES: usize = 4096;

static LIMITS: [AtomicU64; LIMIT_KINDS] = [const { AtomicU64::new(0) }; LIMIT_KINDS];
static BUDGETS: [AtomicU64; RESERVATIONS] = [const { AtomicU64::new(0) }; RESERVATIONS];
static ALLOCATOR: [AtomicU64; RESERVATIONS] = [const { AtomicU64::new(0) }; RESERVATIONS];

/// The next load id (0 is the bare key).
static NEXT_LOAD: AtomicU64 = AtomicU64::new(1);

/// Whether tables are reserved: until they are, nothing is recorded.
static RESERVED: AtomicBool = AtomicBool::new(false);

/// The sticky overflow flag.
static OVERFLOW: AtomicBool = AtomicBool::new(false);

static TABLES: Mutex<Option<Tables>> = Mutex::new(None);

thread_local! {
    /// Whether this thread is inside the fallible helper's
    /// `try_reserve_exact` ([`in_fallible_reserve`]).
    static IN_FALLIBLE: Cell<bool> = const { Cell::new(false) };
    /// Whether this thread is running [`span_reads`]'s reader, which holds
    /// the tables' lock: a read or a load it makes is not recorded (it sets
    /// the overflow flag instead of waiting on itself).
    static VISITING: Cell<bool> = const { Cell::new(false) };
}

fn bump(counters: &[AtomicU64], at: usize) {
    if let Some(c) = counters.get(at) {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

/// A refusal of `kind`'s bound (`LimitError::refused`).
pub(crate) fn limit(kind: LimitKind) {
    bump(&LIMITS, kind_index(kind));
}

/// A budget's refusal of a meter's charge for `what`.
pub(crate) fn budget(what: Reservation) {
    bump(&BUDGETS, reservation_index(what));
}

/// The allocator's refusal of a charged allocation on a meter for `what`.
pub(crate) fn allocator(what: Reservation) {
    bump(&ALLOCATOR, reservation_index(what));
}

/// The refusals counted so far in this process ([`refusals`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refusals {
    limits: [u64; LIMIT_KINDS],
    budgets: [u64; RESERVATIONS],
    allocator: [u64; RESERVATIONS],
}

const _: () = crate::send_sync::<Refusals>();

impl Refusals {
    /// Refusals of `kind`'s bound.
    #[must_use]
    pub fn limit(&self, kind: LimitKind) -> u64 {
        self.limits.get(kind_index(kind)).copied().unwrap_or(0)
    }

    /// Budget refusals of a meter's charge for `what`.
    #[must_use]
    pub fn budget(&self, what: Reservation) -> u64 {
        self.budgets
            .get(reservation_index(what))
            .copied()
            .unwrap_or(0)
    }

    /// Allocator refusals of a charged allocation on a meter for `what`:
    /// each `try_reserve_exact` error, a `CapacityOverflow` (a size past
    /// `isize::MAX`, refused before any allocator is asked) as well as an
    /// allocator's refusal, and each memory error of liblzma's.
    #[must_use]
    pub fn allocator(&self, what: Reservation) -> u64 {
        self.allocator
            .get(reservation_index(what))
            .copied()
            .unwrap_or(0)
    }

    /// Every refusal counted, of any kind.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.limits
            .iter()
            .chain(&self.budgets)
            .chain(&self.allocator)
            .fold(0, |n, c| n.saturating_add(*c))
    }

    /// What was counted after `earlier`, a reading of the same process.
    #[must_use]
    pub fn since(&self, earlier: &Self) -> Self {
        let diff = |now: &[u64], then: &[u64], out: &mut [u64]| {
            for ((o, n), t) in out.iter_mut().zip(now).zip(then) {
                *o = n.saturating_sub(*t);
            }
        };
        let mut out = Self::default();
        diff(&self.limits, &earlier.limits, &mut out.limits);
        diff(&self.budgets, &earlier.budgets, &mut out.budgets);
        diff(&self.allocator, &earlier.allocator, &mut out.allocator);
        out
    }
}

/// Every refusal counted so far in this process, on any thread.
#[must_use]
pub fn refusals() -> Refusals {
    let read = |counters: &[AtomicU64], out: &mut [u64]| {
        for (o, c) in out.iter_mut().zip(counters) {
            *o = c.load(Ordering::Relaxed);
        }
    };
    let mut out = Refusals::default();
    read(&LIMITS, &mut out.limits);
    read(&BUDGETS, &mut out.budgets);
    read(&ALLOCATOR, &mut out.allocator);
    out
}

/// Whether the calling thread is inside a `try_reserve_exact` of the
/// core's two marked sites (`limits::vec::reserve_exact`, the fallible
/// helper, and `limits::vec::shrink_charged`'s move), the places the core
/// allocates what it can be refused. It allocates nothing, so a global
/// allocator may ask it: one that refuses an allocation only while this
/// holds refuses the core nothing it cannot take as a refusal.
#[must_use]
#[inline]
pub fn in_fallible_reserve() -> bool {
    IN_FALLIBLE.try_with(Cell::get).unwrap_or(false)
}

/// Marks this thread as inside the fallible helper while it lives.
pub(crate) struct Fallible(bool);

impl Fallible {
    pub(crate) fn enter() -> Self {
        Self(IN_FALLIBLE.try_with(|c| c.replace(true)).unwrap_or(false))
    }
}

impl Drop for Fallible {
    fn drop(&mut self) {
        let _ = IN_FALLIBLE.try_with(|c| c.set(self.0));
    }
}

/// A load's id: drawn once per load ([`Self::draw`]), or the bare key of
/// a bundle opened outside a load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoadId(u64);

impl LoadId {
    /// No load: a bundle opened outside one.
    pub(crate) const BARE: Self = Self(0);

    /// A new load's id, recorded with `name`, the name the load gives the
    /// core, in the load-name table when one is reserved.
    pub(crate) fn draw(name: &RawStr) -> Self {
        let id = NEXT_LOAD.fetch_add(1, Ordering::Relaxed);
        if RESERVED.load(Ordering::Acquire) {
            record(|t| t.name(id, name.as_bytes()));
        }
        Self(id)
    }

    fn key(self) -> Option<u64> {
        (self.0 != 0).then_some(self.0)
    }
}

/// A nested bundle's place in its load: the row of the bundle it was
/// opened from, and that bundle's own node when it is nested too. It holds
/// no bundle and nothing of one, and dropping it runs no code of a
/// caller's.
#[derive(Debug)]
pub(crate) struct Ancestry {
    up: Option<Arc<Ancestry>>,
    row: usize,
}

// Nothing a read touches is freed under the tables' lock, by construction
// (round 3): a node holds its link and its row and nothing else (a field
// added fails this exhaustive pattern), and a count borrows its key.
const _: fn(Ancestry) -> (Option<Arc<Ancestry>>, usize) = |Ancestry { up, row }| (up, row);
const _: fn(&mut Tables, &Key, SpanRow) -> bool = Tables::count;

/// What a bundle's reads are counted under (module docs, "The key").
#[derive(Clone, Debug)]
pub(crate) enum Key {
    /// Nothing: the bundle was opened while no table was reserved, or
    /// nested in one that was.
    Untracked,
    /// A load's top-level bundle, or one opened outside a load (the bare
    /// key).
    Top(LoadId),
    /// A bundle opened from a row of another: its load's id and its node.
    Nested { load: LoadId, node: Arc<Ancestry> },
}

impl Key {
    /// A load's top-level bundle's key, while a table is reserved.
    pub(crate) fn load(load: LoadId) -> Self {
        if RESERVED.load(Ordering::Acquire) {
            Self::Top(load)
        } else {
            Self::Untracked
        }
    }
}

/// A bundle's key: the one it was opened with, until the core's open that
/// made it gives it its own ([`adopt`]).
#[derive(Debug)]
pub(crate) struct Origin {
    opened: Key,
    given: OnceLock<Key>,
}

impl Origin {
    /// A bundle opened now: bare while a table is reserved.
    pub(crate) fn opened() -> Self {
        Self {
            opened: Key::load(LoadId::BARE),
            given: OnceLock::new(),
        }
    }

    fn key(&self) -> &Key {
        self.given.get().unwrap_or(&self.opened)
    }

    /// Gives the bundle its key, once: the core's open that made it calls
    /// this ([`adopt`]), as does [`hooks_once`], so a test of the hooks
    /// counts the real assignment.
    fn give(&self, key: Key) {
        // Set once, at the open that made it.
        let _ = self.given.set(key);
    }

    /// The key of a bundle opened from row `row` of this one: a node of its
    /// own, allocated only while a table is reserved.
    pub(crate) fn child(&self, row: usize) -> Key {
        if !RESERVED.load(Ordering::Acquire) {
            return Key::Untracked;
        }
        let (load, up) = match self.key() {
            Key::Untracked => return Key::Untracked,
            Key::Top(load) => (*load, None),
            Key::Nested { load, node } => (*load, Some(Arc::clone(node))),
        };
        Key::Nested {
            load,
            node: Arc::new(Ancestry { up, row }),
        }
    }
}

/// Gives the bundle the core's open made, if it made one, the key `key`
/// makes: called when the parse returns, before the bundle is read.
pub(crate) fn adopt(got: &Result<File, Error>, key: impl FnOnce() -> Key) {
    if let Ok(File::Bundle(b)) = got {
        b.span_origin().give(key());
    }
}

/// The row a span read is for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SpanRow {
    /// A directory row.
    Row(usize),
    /// `load_all`'s read of the whole file.
    Whole,
}

impl SpanRow {
    /// `load_all`'s read of the whole file.
    pub(crate) const WHOLE: Self = Self::Whole;

    /// Directory row `i`.
    pub(crate) const fn row(i: usize) -> Self {
        Self::Row(i)
    }
}

/// One span read of the bundle keyed `origin`, for `row`. The key is the
/// bundle's own, which the read borrows: nothing is cloned, so nothing is
/// dropped, under the tables' lock.
pub(crate) fn span_read(origin: &Origin, row: SpanRow) {
    if !RESERVED.load(Ordering::Acquire) {
        return;
    }
    let key = origin.key();
    if !matches!(key, Key::Untracked) {
        record(|t| t.count(key, row));
    }
}

/// Each of the core's counting hooks once, as a load and two nested opens
/// make them: a load id's draw, a top-level key and two nested ones, and a
/// row's and a whole read of each bundle. For a test that counts what they
/// allocate: with no table reserved, nothing (`tests/counters/keys.rs`);
/// with one, the two nested keys' nodes. Not the core's API.
#[doc(hidden)]
pub fn hooks_once(name: &RawStr) {
    let top = Origin::opened();
    let load = LoadId::draw(name);
    top.give(Key::load(load));
    let inner = Origin::opened();
    inner.give(top.child(0));
    let deeper = Origin::opened();
    deeper.give(inner.child(1));
    for o in [&top, &inner, &deeper] {
        span_read(o, SpanRow::row(0));
        span_read(o, SpanRow::WHOLE);
    }
}

fn lock() -> MutexGuard<'static, Option<Tables>> {
    TABLES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether this thread is running [`span_reads`]'s reader.
fn visiting() -> bool {
    VISITING.try_with(Cell::get).unwrap_or(false)
}

/// `f` on the tables, when they are reserved; when it records nothing, the
/// overflow flag is set under the tables' lock, so a reserve that starts
/// over after it is not marked by it (round 1: the flag was set after the
/// lock was let go). Inside [`span_reads`]'s reader, which holds the lock,
/// nothing is recorded and the flag is set.
fn record(f: impl FnOnce(&mut Tables) -> bool) {
    if visiting() {
        OVERFLOW.store(true, Ordering::Release);
        return;
    }
    let mut held = lock();
    if let Some(t) = held.as_mut()
        && !f(t)
    {
        OVERFLOW.store(true, Ordering::Release);
    }
}

/// The two tables, each allocated once, at [`reserve_span_reads`].
#[derive(Debug)]
struct Tables {
    /// `(load id, start, length)` of each name in `arena`.
    names: Vec<(u64, usize, usize)>,
    arena: Vec<u8>,
    slots: Vec<Slot>,
    /// Slot `i`'s path is `paths[i * width..][..slot.len]`.
    paths: Vec<usize>,
    width: usize,
    /// Room for one path, which a read's is built in from its key's nodes
    /// before it is matched with a slot's.
    scratch: Vec<usize>,
}

#[derive(Debug)]
struct Slot {
    load: LoadId,
    whole: bool,
    len: usize,
    count: u64,
}

impl Tables {
    /// Tables of `names` names, `slots` slots and paths `width` deep, or
    /// `None` when the allocator refuses them.
    fn with_room(names: usize, slots: usize, width: usize) -> Option<Self> {
        let mut t = Self {
            names: Vec::new(),
            arena: Vec::new(),
            slots: Vec::new(),
            paths: Vec::new(),
            width,
            scratch: Vec::new(),
        };
        t.names.try_reserve_exact(names).ok()?;
        t.arena
            .try_reserve_exact(names.checked_mul(NAME_BYTES)?)
            .ok()?;
        t.slots.try_reserve_exact(slots).ok()?;
        t.paths.try_reserve_exact(slots.checked_mul(width)?).ok()?;
        t.scratch.try_reserve_exact(width).ok()?;
        Some(t)
    }

    /// Records `name` as load `id`'s: false when it does not fit.
    fn name(&mut self, id: u64, name: &[u8]) -> bool {
        let at = self.arena.len();
        let fits = name.len() <= NAME_BYTES
            && self.names.len() < self.names.capacity()
            && at.saturating_add(name.len()) <= self.arena.capacity();
        if fits {
            self.arena.extend_from_slice(name);
            self.names.push((id, at, name.len()));
        }
        fits
    }

    /// Counts a read of `row` of the bundle keyed `key`: false when its
    /// path is deeper than the table's width or the table is full. Its path
    /// is built in `scratch` from the key's nodes, which it borrows, so
    /// nothing is allocated or freed.
    fn count(&mut self, key: &Key, row: SpanRow) -> bool {
        let (load, mut node) = match key {
            Key::Untracked => return true,
            Key::Top(load) => (*load, None),
            Key::Nested { load, node } => (*load, Some(&**node)),
        };
        let width = self.width;
        let scratch = &mut self.scratch;
        scratch.clear();
        while let Some(n) = node {
            if scratch.len() >= width {
                return false;
            }
            scratch.push(n.row);
            node = n.up.as_deref();
        }
        scratch.reverse();
        let whole = match row {
            SpanRow::Row(i) if scratch.len() < width => {
                scratch.push(i);
                false
            }
            SpanRow::Row(_) => return false,
            SpanRow::Whole => true,
        };
        let path = self.scratch.as_slice();
        let len = path.len();
        let paths = &self.paths;
        let same = |i: usize, s: &Slot| {
            let at = i.saturating_mul(width);
            let p = paths.get(at..at.saturating_add(s.len)).unwrap_or_default();
            s.load == load && s.whole == whole && p == path
        };
        if let Some(s) = self
            .slots
            .iter_mut()
            .enumerate()
            .find_map(|(i, s)| same(i, s).then_some(s))
        {
            s.count = s.count.saturating_add(1);
            return true;
        }
        let end = self.paths.len().saturating_add(width);
        if self.slots.len() >= self.slots.capacity() || end > self.paths.capacity() {
            return false;
        }
        self.paths.extend_from_slice(&self.scratch);
        self.paths.resize(end, 0);
        self.slots.push(Slot {
            load,
            whole,
            len,
            count: 1,
        });
        true
    }
}

/// Preallocates the load-name table, of `names` names of at most
/// [`NAME_BYTES`] bytes each, and the span-read table, of `slots` slots
/// whose paths are at most `width` indices deep (`max_container_depth + 1`
/// for a read of any row a load can reach), and starts recording into
/// them. Called again, it starts over: new, empty tables, the overflow flag
/// cleared. The caller makes it outside any measured window: it is the one
/// allocation the counts make. When the allocator refuses the tables,
/// nothing is reserved and the overflow flag is set.
///
/// # Panics
///
/// Inside [`span_reads`]'s reader, which holds the tables' lock: a test's
/// mistake, which a panic reports where waiting on the lock would hang.
pub fn reserve_span_reads(names: usize, slots: usize, width: usize) {
    assert!(
        !visiting(),
        "testing::reserve_span_reads called inside testing::span_reads's reader"
    );
    let tables = Tables::with_room(names, slots, width);
    let mut held = lock();
    OVERFLOW.store(tables.is_none(), Ordering::Release);
    RESERVED.store(tables.is_some(), Ordering::Release);
    // The old tables are freed once the lock is let go: their free may run
    // a test allocator's code, which may read the tables (round 3: they
    // were freed under the lock, so that read waited on it).
    let old = std::mem::replace(&mut *held, tables);
    drop(held);
    drop(old);
}

/// Runs `f` on the tables as they are now, which it reads without a copy:
/// the tables' lock is held while it runs, so `f` runs test code under a
/// lock of the core's (`docs/plans/core.md` 4.1's lock rule names this
/// exception). `f` allocates what it wants; the tables do not grow.
///
/// `f` must not call into the core, nor wait on a thread that does: another
/// thread's read waits for `f` to return, so `f` waiting on that thread
/// deadlocks, and a read on `f`'s own thread is not counted but sets the
/// overflow flag.
///
/// # Panics
///
/// When `f` calls this or [`reserve_span_reads`]: the lock is checked
/// before it is taken, so a nested call panics rather than waits on
/// itself (round 1: it deadlocked).
pub fn span_reads<R>(f: impl FnOnce(&SpanReads<'_>) -> R) -> R {
    /// Clears [`VISITING`] again when `f` returns or unwinds.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = VISITING.try_with(|c| c.set(self.0));
        }
    }
    assert!(
        !visiting(),
        "testing::span_reads called inside its own reader"
    );
    let held = lock();
    let _restore = Restore(VISITING.try_with(|c| c.replace(true)).unwrap_or(false));
    f(&SpanReads {
        tables: held.as_ref(),
        overflowed: OVERFLOW.load(Ordering::Acquire),
    })
}

/// The load-name and span-read tables, as [`span_reads`] shows them.
#[derive(Debug)]
pub struct SpanReads<'a> {
    tables: Option<&'a Tables>,
    overflowed: bool,
}

const _: () = crate::send_sync::<SpanReads<'static>>();

impl<'a> SpanReads<'a> {
    /// Whether [`reserve_span_reads`] reserved the tables.
    #[must_use]
    pub fn reserved(&self) -> bool {
        self.tables.is_some()
    }

    /// Whether a count or a name did not fit since the tables were
    /// reserved (or the tables could not be): a test that reads it set
    /// fails, since a count is missing.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Each load's id and the name it gave the core, in the order the names
    /// were recorded, which loads on several threads may make other than
    /// the order of their ids.
    pub fn names(&self) -> impl Iterator<Item = (u64, &'a [u8])> + use<'a> {
        let t = self.tables;
        t.into_iter().flat_map(|t| {
            t.names.iter().map(|&(id, at, len)| {
                let name = t.arena.get(at..at.saturating_add(len)).unwrap_or_default();
                (id, name)
            })
        })
    }

    /// The name load `id` gave the core, if the table holds it.
    #[must_use]
    pub fn name(&self, id: u64) -> Option<&'a [u8]> {
        self.names().find_map(|(i, n)| (i == id).then_some(n))
    }

    /// Each span-read slot, in the order its first read was counted.
    pub fn reads(&self) -> impl Iterator<Item = SpanRead<'a>> + use<'a> {
        let t = self.tables;
        t.into_iter().flat_map(|t| {
            t.slots.iter().enumerate().map(|(i, s)| {
                let at = i.saturating_mul(t.width);
                SpanRead {
                    load: s.load.key(),
                    path: t
                        .paths
                        .get(at..at.saturating_add(s.len))
                        .unwrap_or_default(),
                    whole: s.whole,
                    count: s.count,
                }
            })
        })
    }
}

/// One span-read slot ([`SpanReads::reads`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpanRead<'a> {
    /// The load id, or `None` for the bare key (a bundle opened outside a
    /// load).
    pub load: Option<u64>,
    /// The index path: of the row read, or, for a whole read, of the
    /// bundle read whole (empty for a top-level bundle).
    pub path: &'a [usize],
    /// `load_all`'s read of the whole file, not of one row.
    pub whole: bool,
    /// How many reads were counted.
    pub count: u64,
}

const _: () = crate::send_sync::<SpanRead<'static>>();

#[cfg(test)]
#[path = "testing_tests.rs"]
mod tests;
