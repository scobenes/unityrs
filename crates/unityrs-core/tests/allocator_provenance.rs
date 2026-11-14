//! Allocator provenance must survive wrappers without becoming a pool refusal.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use unityrs_core::{
    Error,
    budget::{AllocError, BudgetExceeded, MemoryBudget, Permit, Reservation},
    detect::DetectError,
    env::DerefError,
    limits::{ChargeError, Meter},
    once::CellError,
    typetree::TreeError,
    walk::{ParseError, ReadDetail, ReadError, ReadErrorKind},
};

#[derive(Debug, Default)]
struct Observer(AtomicU64);
impl MemoryBudget for Observer {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        Ok(Permit::detached(bytes))
    }
    fn allocator_refused(&self, _: AllocError) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn allocator_wrappers_are_transient_without_meter_work_or_owned_heap() {
    let observer = Arc::new(Observer::default());
    let meter = Meter::new(u64::MAX, observer.clone(), Reservation::Dependency);
    let alloc = meter.allocator_refused(4096);
    assert_eq!(
        alloc,
        AllocError {
            requested: 4096,
            what: Reservation::Dependency
        }
    );
    let charge = ChargeError::from(alloc);
    assert!(charge.is_transient());
    assert!(!charge.is_meter_refusal());
    let tree = TreeError::from(alloc);
    let detail = ReadDetail::from(alloc);
    let errors = [
        Error::from(alloc),
        Error::from(charge),
        Error::from(tree.clone()),
        Error::from(DetectError::from(alloc)),
        Error::AssetBundle(Arc::new(ParseError::from(alloc))),
        Error::AssetBundle(Arc::new(ParseError::Read(ReadError {
            kind: ReadErrorKind::Other,
            at: 0,
            detail,
        }))),
        Error::Deref(Arc::new(DerefError::from(alloc))),
    ];
    for error in errors {
        assert!(error.is_transient(), "{error:?}");
        assert!(!error.is_meter_refusal());
    }
    assert_eq!(Error::from(alloc).heap_bytes(), 0);
    assert_eq!(tree.heap_bytes(), 0);
    let _clone = Error::from(alloc).clone();
    assert_eq!(
        observer.0.load(Ordering::Relaxed),
        1,
        "wrapping never notifies again"
    );
}

#[test]
fn allocator_failure_is_not_published_or_charged_as_repeated_work() {
    use unityrs_core::once::{ResultCell, get_or_publish_result};
    let meter = Meter::with_decode_work(
        4096,
        4096,
        Arc::new(Observer::default()),
        Reservation::Input,
    );
    let cell: ResultCell<u8, Error> = ResultCell::new();
    let first = get_or_publish_result(&cell, &meter, || {
        let _charge = meter.charge(2048)?;
        Err(meter.allocator_refused(512).into())
    });
    assert!(matches!(first, Err(Error::Allocator(_))));
    assert!(cell.get().is_none());
    assert_eq!(meter.decoded(), 0, "host failures spend no input work");
    assert_eq!(*get_or_publish_result(&cell, &meter, || Ok(7)).unwrap(), 7);
}

thread_local! { static ALLOCATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
struct Tally;
impl unityrs_testkit::counted::Tally for Tally {
    fn grew(&self, _: usize, _: bool) {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get().saturating_add(1)));
    }
    fn shrank(&self, _: usize) {}
}
#[global_allocator]
static ALLOC: unityrs_testkit::counted::Allocator<Tally> =
    unityrs_testkit::counted::Allocator(Tally);

#[test]
fn notifying_converting_and_cloning_allocator_errors_allocates_nothing() {
    let observer = Arc::new(Observer::default());
    let meter = Meter::new(u64::MAX, observer, Reservation::Input);
    let error = Error::AssetBundle(Arc::new(ParseError::from(AllocError {
        requested: 32,
        what: Reservation::Input,
    })));
    unityrs_testkit::counted::count_this_thread();
    let before = ALLOCATIONS.with(std::cell::Cell::get);
    let a = meter.allocator_refused(4096);
    std::hint::black_box((
        Error::from(a),
        TreeError::from(a),
        ReadDetail::from(a),
        ParseError::from(a),
        DerefError::from(a),
        DetectError::from(a),
        error.clone(),
    ));
    assert_eq!(ALLOCATIONS.with(std::cell::Cell::get), before);
}
