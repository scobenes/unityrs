//! A dependency's record ([`Dependency`]): what reopens it after an
//! eviction, and the weak handle on the file the LRU keeps open;
//! [`open_source`], which reads a source, detects it and parses it; and
//! [`with_room`], the one retry every dependency read or registration the
//! pool refuses takes.

use std::cell::Cell;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{DepCache, lock};
use crate::budget::Reservation;
use crate::detect::check_file_type_with_reader;
use crate::env::{DerefError, Fetch, Source, within_input_bytes};
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File, RawFile, SerializedFile, parse_file_with_reader};
use crate::limits::{ChargeError, Meter, pool_refusals, refusal};
use crate::options::FileContext;
use crate::pystr::is_split_path;
use crate::raw::RawStr;
use crate::stream::ReaderKind;
use crate::typetree::TreeError;
use crate::walk::{ParseError, ReadDetail};

/// A dependency the Environment loaded (through the resolver, or
/// `Environment::load_dependency`): what reopens it, and the handle the LRU
/// keeps while it is cached.
#[derive(Debug)]
pub(crate) struct Dependency {
    /// The name it was loaded under.
    pub(crate) name: RawStr,
    /// Where its bytes come from again after an eviction.
    pub(crate) source: Source,
    /// Its current open: the LRU's handle, weakly, and its meter, under one
    /// lock, so a reopen publishes both in one step and no lock is taken
    /// while another is held (plan section 3.16; review round 6: the meter
    /// had a lock of its own, taken while this one was held).
    pub(crate) open: Mutex<Open>,
    /// The context it is parsed with.
    pub(crate) ctx: Arc<FileContext>,
    /// The LRU that keeps its open handle, weakly: the LRU holds the
    /// dependency, so a strong handle back would keep both alive after the
    /// Environment drops.
    pub(crate) cache: Weak<DepCache>,
    /// How many registry slots reach it ([`DepSlot`]): when the last goes,
    /// nothing can reach it any more and the LRU lets go of it.
    pub(crate) slots: AtomicUsize,
}

/// A dependency's current open ([`Dependency::open`]).
#[derive(Debug)]
pub(crate) struct Open {
    /// The LRU's handle, weakly: an eviction drops the LRU's strong one.
    pub(crate) file: Option<WeakFile>,
    /// The meter of the open. An eviction keeps it; the next open is on its
    /// [`Meter::reopen`], which charges the same memory account and has its
    /// own work allowance, so an old open's handle that someone still holds
    /// keeps counting against the old one (plan section 3.13).
    pub(crate) meter: Arc<Meter>,
}

impl Open {
    /// An open of `file` on `meter`.
    pub(crate) fn of(file: &File, meter: Arc<Meter>) -> Self {
        Self {
            file: WeakFile::of(file),
            meter,
        }
    }

    fn alive(&self) -> Option<File> {
        self.file.as_ref().and_then(WeakFile::upgrade)
    }
}

/// A registry slot's handle on its dependency, counted in
/// [`Dependency::slots`]: made with the slot's kind (before the slot, so
/// a registration refused on the way counts it out again) and dropped with
/// it. The count is decided in one atomic step, so of two slots that drop
/// at once exactly one sees itself the last and makes the LRU let go
/// (review round 2: each tested the dependency's `Arc` count, saw the
/// other's handle, and neither did, which left the dependency in an LRU
/// that never evicts).
#[derive(Debug)]
pub(crate) struct DepSlot(Arc<Dependency>);

impl DepSlot {
    pub(crate) fn new(dep: &Arc<Dependency>) -> Self {
        dep.slots.fetch_add(1, Ordering::AcqRel);
        Self(dep.clone())
    }
}

impl Deref for DepSlot {
    type Target = Arc<Dependency>;

    fn deref(&self) -> &Arc<Dependency> {
        &self.0
    }
}

impl Drop for DepSlot {
    fn drop(&mut self) {
        let last = self.0.slots.fetch_sub(1, Ordering::AcqRel) == 1;
        if let Some(cache) = self.0.cache.upgrade() {
            #[cfg(test)]
            cache.hook(super::Hook::SlotReleased);
            if last {
                cache.forget(&self.0);
            }
        }
    }
}

impl Dependency {
    /// Whether no registry slot reaches it ([`DepSlot`]).
    pub(crate) fn unreachable(&self) -> bool {
        self.slots.load(Ordering::Acquire) == 0
    }
}

/// A weak handle on a dependency's top-level file.
#[derive(Debug)]
pub(crate) enum WeakFile {
    /// A `SerializedFile`.
    Serialized(Weak<SerializedFile>),
    /// A bundle.
    Bundle(Weak<BundleFile>),
    /// A raw file.
    Raw(Weak<RawFile>),
}

impl WeakFile {
    pub(crate) fn of(file: &File) -> Option<Self> {
        Some(match file {
            File::Serialized(f) => Self::Serialized(Arc::downgrade(f)),
            File::Bundle(f) => Self::Bundle(Arc::downgrade(f)),
            File::Raw(f) => Self::Raw(Arc::downgrade(f)),
            File::Web(_) => return None,
        })
    }

    fn upgrade(&self) -> Option<File> {
        match self {
            Self::Serialized(w) => w.upgrade().map(File::Serialized),
            Self::Bundle(w) => w.upgrade().map(File::Bundle),
            Self::Raw(w) => w.upgrade().map(File::Raw),
        }
    }
}

/// Reads a source's bytes on `meter` (a [`Source::Path`] through
/// [`crate::env::read_path`], charged first; a [`Source::Fetch`] from its
/// fetcher; [`Source::Bytes`] as they are), detects them and parses them as
/// a top-level file named `name`. A path UnityPy would read as the parts of
/// a split file (`environment.reSplit`, [`is_split_path`]) is refused with
/// [`Unsupported::Split`] before anything is read: UnityPy merges the parts
/// (`environment.py:111-116`), which P1 does not (plan section 1). A path is
/// read by the stream reader's rules, as UnityPy reads the file it opens
/// for one (`environment.py:136-139`); bytes, fetched or given, by the
/// memory reader's ([`ReaderKind`]).
///
/// Every source is bounded by [`crate::limits::Limits::max_input_bytes`]
/// ([`within_input_bytes`]): a path by [`crate::env::read_path`], from its
/// length before anything is read; a fetch from the size its fetcher states
/// ([`Fetch::size_hint`]) before it is fetched, then from the length of the
/// bytes it returns; given bytes from their length. Bytes above the bound
/// are refused here, before anything detects, parses or keeps them, and
/// drop with whatever charge they carry (review round 10: a fetch was not
/// bounded at all).
pub(crate) fn open_source(
    name: &RawStr,
    source: &Source,
    meter: &Arc<Meter>,
    ctx: &Arc<FileContext>,
) -> Result<File, Error> {
    let reader = match source {
        Source::Path(_) => ReaderKind::Stream,
        Source::Bytes { .. } | Source::Fetch { .. } => ReaderKind::Memory,
    };
    let data = match source {
        Source::Bytes { data, .. } => data.clone(),
        Source::Path(p) => {
            // UnityPy environment.py:111-127: a `.splitN` name, or a missing
            // path whose `.split0` exists.
            let parts = || {
                let mut first = p.clone().into_os_string();
                first.push(".split0");
                !p.exists() && std::path::Path::new(&first).exists()
            };
            if is_split_path(p.as_os_str().as_encoded_bytes()) || parts() {
                return Err(Error::Unsupported(Unsupported::Split));
            }
            crate::env::read_path(p, &ctx.limits, meter)?
        }
        Source::Fetch { fetch, .. } => {
            if let Some(stated) = fetch.size_hint() {
                within_input_bytes(stated, &ctx.limits)?;
            }
            fetched(fetch.as_ref())?
        }
    };
    within_input_bytes(u64::try_from(data.len()).unwrap_or(u64::MAX), &ctx.limits)?;
    // UnityPy environment.py:139-154
    let ty = check_file_type_with_reader(&data, &ctx.limits, reader)?;
    parse_file_with_reader(name.clone(), data, ty, None, meter, ctx, reader)
}

/// `fetch`'s bytes, a refusal of the dependency pool it returns counted
/// ([`from_callers`]).
fn fetched(fetch: &dyn Fetch) -> Result<bytes::Bytes, Error> {
    from_callers(fetch.fetch())
}

/// What the caller's code returned to the core (a [`Fetch`]'s bytes, a
/// [`crate::codec::BlockCodec`]'s decode, a
/// [`crate::codec::BlockDecryptor`]'s decryption), a refusal of the
/// dependency pool in it ([`pool_refused`]) counted in [`pool_refusals`] as
/// it enters the core, as every refusal the core makes is counted where it
/// is made. [`with_room`] tells a refusal a retry already met from a later
/// one by that count, so a refusal the caller's code made itself (a fetcher
/// or a codec that reserves its bytes from the pool), had it moved no
/// count, would match a mark an earlier refusal left and go untried
/// (review round 9, for a fetcher's).
///
/// It is counted whatever was counted while the code ran (review round 10:
/// a fetcher's was counted only when nothing had been, so a fetcher that
/// read through the Environment, let go of the refusal its read met twice,
/// then refused the pool itself, returned a refusal that matched its
/// read's mark, and was not retried; and a codec's or a decryptor's was
/// never counted). The returned value cannot say where it came from (a
/// [`crate::budget::BudgetExceeded`] is the caller's to build), so a
/// refusal the code passes on from a call of its own into the Environment,
/// which that call's retry already met, is counted again too: the
/// [`with_room`] around the code then retries it once more, one more run of
/// the code, the price of never skipping the retry the code's own refusal
/// is owed (`room_tests.rs` pins both). The built-in codecs return through
/// the same call, so a refusal of theirs, counted where it was made, is
/// counted again there; that changes no decision, since they make no call
/// into the Environment, whose `with_room` alone sets the mark the count
/// is compared with.
pub(crate) fn from_callers<T>(returned: Result<T, Error>) -> Result<T, Error> {
    returned.inspect_err(|e| {
        if pool_refused(e) {
            // `refusal` counts the refusal it builds; the caller's own is
            // the one returned.
            let _counted = refusal(0, Reservation::Dependency);
        }
    })
}

thread_local! {
    /// The LRU (its address) a [`with_room`] running on this thread is
    /// retrying in: no `with_room` of that Environment retries inside it.
    static RETRYING: Cell<Option<usize>> = const { Cell::new(None) };
    /// The LRU, and this thread's count of pool refusals
    /// ([`pool_refusals`]), when a [`with_room`] of that Environment last
    /// returned a refusal that a retry had already met: an enclosing one
    /// that gets a refusal while the count is still there holds that one.
    /// Cleared when the outermost `with_room` running on this thread
    /// returns ([`Running`]), so it never outlives the call that set it.
    static RETRIED: Cell<Option<(usize, u64)>> = const { Cell::new(None) };
    /// How many [`with_room`]s are running on this thread, nested.
    static RUNNING: Cell<usize> = const { Cell::new(0) };
}

/// Restores [`RETRYING`] when a retry ends, a panic included.
struct Retrying(Option<usize>);

impl Retrying {
    fn enter(at: usize) -> Self {
        Self(RETRYING.replace(Some(at)))
    }
}

impl Drop for Retrying {
    fn drop(&mut self) {
        RETRYING.set(self.0);
    }
}

/// One [`with_room`] running on this thread ([`RUNNING`]). When the
/// outermost one returns, a panic included, [`RETRIED`] is cleared: no
/// `with_room` encloses it, so the mark has no one left to tell, and a
/// later call's retry depends only on that call (review round 9: the mark
/// stayed, and a later call on the thread whose refusal moved no count,
/// a fetcher's own, matched it and was not retried).
struct Running;

impl Running {
    fn enter() -> Self {
        RUNNING.set(RUNNING.get().saturating_add(1));
        Self
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let left = RUNNING.get().saturating_sub(1);
        RUNNING.set(left);
        if left == 0 {
            RETRIED.set(None);
        }
    }
}

/// Runs `f`, work an Environment call does through a dependency's meter (a
/// dependency's load, a lookup that opens or reads one, a reach of its
/// nested container, a read from its file); when the dependency pool
/// refuses it ([`pool_refused`]), evicts every cached dependency but `keep`
/// and runs it once more, the second refusal being the caller's (plan
/// section 3.13: "a dependency read the pool refuses evicts and retries
/// once"). One rule, wherever the calls nest: a refusal is retried by the
/// innermost `with_room` it reaches, once, and by no other. So a
/// `with_room` passes on, untried, a refusal one inside it already retried
/// (it knows it by this thread's count of pool refusals, which no refusal
/// has moved since: [`RETRIED`]), and none of an Environment retries inside
/// a retry of that Environment's ([`RETRYING`]): the room is made, and
/// what the second run meets is final. A refusal another one inside it met
/// and let go (a `settle` leaves a refusal for the next call) moves the
/// count, so a later one is retried (review round 8: `resource` and
/// `deref` had no retry, and calls that nest, a lookup inside a
/// dereference inside a container pass, retried one refusal at every
/// level). Every refusal moves the count, one the caller's code returns (a
/// fetcher's, a codec's, a decryptor's) included, as it returns
/// ([`from_callers`]), and the mark is cleared when the outermost `with_room`
/// returns ([`Running`]), so whether a refusal is retried depends on the
/// current call alone, never on what the thread ran before it (review
/// round 9). `f` must be work that can run twice: a lookup, a read, or a
/// whole load (whose registrations a second run makes again at its own
/// ranks).
pub(crate) fn with_room<T, E: PoolRefusal>(
    cache: Option<&DepCache>,
    keep: Option<&Arc<Dependency>>,
    mut f: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let at = cache.map_or(0, |c| std::ptr::from_ref(c).addr());
    let _running = Running::enter();
    let first = f();
    let Err(e) = &first else {
        return first;
    };
    if !e.pool_refused() {
        return first;
    }
    if RETRYING.get() == Some(at) || RETRIED.get() == Some((at, pool_refusals())) {
        RETRIED.set(Some((at, pool_refusals())));
        return first;
    }
    drop(first);
    if let Some(cache) = cache {
        cache.evict_others(keep);
    }
    let second = {
        let _retrying = Retrying::enter(at);
        f()
    };
    if matches!(&second, Err(e) if e.pool_refused()) {
        RETRIED.set(Some((at, pool_refusals())));
    }
    second
}

/// An error that may be the dependency pool's refusal, as it is or wrapped
/// ([`pool_refused`]): what [`with_room`] retries.
pub(crate) trait PoolRefusal {
    /// Whether it is the dependency pool refusing a charge.
    fn pool_refused(&self) -> bool;
}

impl PoolRefusal for Error {
    fn pool_refused(&self) -> bool {
        pool_refused(self)
    }
}

impl PoolRefusal for DerefError {
    fn pool_refused(&self) -> bool {
        deref_refused(self)
    }
}

impl PoolRefusal for ParseError {
    fn pool_refused(&self) -> bool {
        parse_refused(self)
    }
}

/// Whether `e` is the dependency pool refusing a charge
/// ([`Reservation::Dependency`]), as it is or wrapped by an error that
/// carries one: an entry's, a tree's, an object read's or a load's, the
/// wrappers [`crate::once::CellError::is_transient`] looks through (review
/// round 7: a refusal while a reopened dependency's entry was parsed came
/// back as the entry's error, and the retry, which matched only a bare
/// refusal, missed it).
pub(crate) fn pool_refused(e: &Error) -> bool {
    match e {
        Error::Budget(b) => b.what == Reservation::Dependency,
        Error::Entry { source, .. } => pool_refused(source),
        Error::Tree(t) => tree_refused(t),
        Error::AssetBundle(p) => parse_refused(p),
        Error::Deref(d) => deref_refused(d),
        _ => false,
    }
}

fn charge_refused(c: &ChargeError) -> bool {
    matches!(c, ChargeError::Budget(b) if b.what == Reservation::Dependency)
}

fn tree_refused(t: &TreeError) -> bool {
    matches!(t, TreeError::Charge(c) if charge_refused(c))
}

fn deref_refused(d: &DerefError) -> bool {
    matches!(d, DerefError::Load(e) if pool_refused(e))
}

fn parse_refused(p: &ParseError) -> bool {
    match p {
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Charge(c) => charge_refused(c),
            ReadDetail::Tree(t) => tree_refused(t),
            _ => false,
        },
        ParseError::Tree(t) => tree_refused(t),
        ParseError::Deref(d) => deref_refused(d),
        _ => false,
    }
}

impl Dependency {
    /// [`with_room`] for a read or a registration of this dependency, which
    /// the eviction keeps.
    pub(crate) fn with_room<T, E: PoolRefusal>(
        self: &Arc<Self>,
        f: impl FnMut() -> Result<T, E>,
    ) -> Result<T, E> {
        with_room(self.cache.upgrade().as_deref(), Some(self), f)
    }

    /// The handle the LRU keeps, opening the dependency again from its
    /// source when it was evicted (on [`Meter::reopen`] of its meter). The
    /// source is read and parsed with no lock held; the first open to
    /// publish wins, and publishes its handle and its meter in one step.
    /// A meter it replaces is dropped after the lock.
    pub(crate) fn handle(self: &Arc<Self>) -> Result<File, Error> {
        let (alive, meter) = {
            let open = lock(&self.open);
            (open.alive(), open.meter.clone())
        };
        if let Some(f) = alive {
            if let Some(cache) = self.cache.upgrade() {
                cache.touch(self, &f)?;
            }
            return Ok(f);
        }
        let meter = meter.reopen();
        let file = open_source(&self.name, &self.source, &meter, &self.ctx)?;
        let (file, _replaced) = {
            let mut open = lock(&self.open);
            if let Some(theirs) = open.alive() {
                (theirs, None)
            } else {
                let mine = Open::of(&file, meter);
                (file, Some(std::mem::replace(&mut *open, mine)))
            }
        };
        if let Some(cache) = self.cache.upgrade() {
            cache.touch(self, &file)?;
        }
        Ok(file)
    }

    /// The open handle if it is alive, without opening anything.
    pub(crate) fn alive(&self) -> Option<File> {
        lock(&self.open).alive()
    }

    /// The meter of its current open.
    pub(crate) fn meter(&self) -> Arc<Meter> {
        lock(&self.open).meter.clone()
    }
}
