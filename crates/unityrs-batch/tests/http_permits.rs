//! Permits remain occupied while a timed-out lookup owns its token.
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};
use unityrs_batch::{
    http::permit::{FINISHER_BASE, Operation, PermitPool, RequestPermit},
    sys::reset::SOCKET_CAPACITY,
};

#[test]
fn dropping_a_request_quarantines_its_lookup_and_pool_occupancy() {
    let pool = PermitPool::new(1).unwrap();
    let mut request = pool.acquire().unwrap();
    let lookup = request.begin(Operation::Lookup).unwrap();
    assert!(request.begin(Operation::Write).is_none());
    drop(request);
    assert!(pool.acquire().is_none());
    assert_eq!(pool.snapshot().outstanding, 1);
    drop(lookup);
    assert_eq!(pool.snapshot().outstanding, 0);
    assert!(pool.acquire().is_some());
}

#[test]
fn lookup_return_does_not_release_a_live_request() {
    let pool = PermitPool::new(1).unwrap();
    let mut request = pool.acquire().unwrap();
    drop(request.begin(Operation::Lookup).unwrap());
    assert!(pool.acquire().is_none());
    drop(request.begin(Operation::Write).unwrap());
    drop(request);
    assert_eq!(pool.snapshot().occupied, 0);
}

#[test]
fn capacity_and_one_operation_per_permit_bound_all_outstanding_tasks() {
    assert!(PermitPool::new(0).is_err());
    let pool = PermitPool::new(3).unwrap();
    let mut a = pool.acquire().unwrap();
    let mut b = pool.acquire().unwrap();
    let c = pool.acquire().unwrap();
    assert!(pool.acquire().is_none());
    let lookup = a.begin(Operation::Lookup).unwrap();
    let write = b.begin(Operation::Write).unwrap();
    assert_eq!(pool.snapshot().outstanding, 2);
    assert_eq!(pool.snapshot().occupied, 3);
    drop((a, b, c));
    assert_eq!(pool.snapshot().occupied, 2);
    drop((lookup, write));
    assert_eq!(pool.snapshot().occupied, 0);
}

#[test]
fn moved_storage_keeps_the_slot_and_refuses_over_the_exact_allowance() {
    use unityrs_batch::http::permit::NETWORK_ALLOWANCE;
    let pool = PermitPool::new(1).unwrap();
    let mut request = pool.acquire().unwrap();
    let charge = request.charge(NETWORK_ALLOWANCE).unwrap();
    assert!(request.charge(1).is_none());
    let write = request.begin(Operation::Write).unwrap();
    drop(request);
    drop(write);
    assert!(pool.acquire().is_none());
    drop(charge);
    assert_eq!(pool.snapshot().occupied, 0);
    assert!(pool.acquire().is_some());
}

#[test]
fn a_lookup_return_wakes_the_waiting_try_without_releasing_its_request() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::{Context, Poll, Wake, Waker};
    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    let mut context = Context::from_waker(&waker);
    let pool = PermitPool::new(1).unwrap();
    let mut request = pool.acquire().unwrap();
    let token = request.begin(Operation::Lookup).unwrap();
    assert_eq!(request.poll_available(&mut context), Poll::Pending);
    drop(token);
    assert_eq!(count.0.load(Ordering::Acquire), 1);
    assert_eq!(request.poll_available(&mut context), Poll::Ready(()));
    assert!(pool.acquire().is_none());
}

#[test]
fn bases_place_the_two_pools_on_disjoint_socket_entries() {
    let controller = PermitPool::with_base(0, 256).unwrap();
    let finisher = PermitPool::with_base(FINISHER_BASE, 256).unwrap();
    // Hold every permit: a dropped one is free again.
    let held: Vec<_> = std::iter::from_fn(|| controller.acquire())
        .take(257)
        .chain(std::iter::from_fn(|| finisher.acquire()).take(257))
        .collect();
    let mut indices: Vec<_> = held.iter().map(RequestPermit::index).collect();
    assert_eq!(indices.len(), 512);
    assert_eq!(indices[256], FINISHER_BASE);
    indices.dedup();
    assert_eq!(indices.len(), 512);
    assert!(indices.iter().all(|i| *i < SOCKET_CAPACITY));
    for (base, capacity) in [(0, 0), (FINISHER_BASE, SOCKET_CAPACITY), (usize::MAX, 1)] {
        assert!(PermitPool::with_base(base, capacity).is_err());
    }
}

#[derive(Default)]
struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn the_pool_is_idle_only_once_a_quarantined_lookup_returns() {
    let pool = PermitPool::new(2).unwrap();
    let count = Arc::new(Count::default());
    let waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(pool.poll_idle(&mut cx).is_ready());
    let mut request = pool.acquire().unwrap();
    let lookup = request.begin(Operation::Lookup).unwrap();
    drop(request);
    assert_eq!(pool.poll_idle(&mut cx), Poll::Pending);
    let woken = count.0.load(Ordering::SeqCst);
    drop(lookup);
    assert_eq!(count.0.load(Ordering::SeqCst), woken + 1);
    assert!(pool.poll_idle(&mut cx).is_ready());
}
/// A permit held by a lookup its request outlived (the try timed out and
/// dropped its join handle) frees on the lookup's return, and that return
/// wakes a caller waiting for a permit: no waker of the request's is left.
#[test]
fn a_waiting_acquire_is_woken_by_a_quarantined_lookups_return() {
    let pool = PermitPool::new(1).unwrap();
    let count = Arc::new(Count::default());
    let waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    let mut request = pool.acquire().unwrap();
    // The request's own wait registered (and its readiness cleared) a waker.
    assert!(request.poll_available(&mut cx).is_ready());
    let lookup = request.begin(Operation::Lookup).unwrap();
    drop(request);
    assert!(pool.poll_acquire(&mut cx).is_pending());
    let woken = count.0.load(Ordering::SeqCst);
    drop(lookup);
    assert_eq!(count.0.load(Ordering::SeqCst), woken + 1);
    let Poll::Ready(permit) = pool.poll_acquire(&mut cx) else {
        panic!("the returned lookup freed no permit");
    };
    // A request's end wakes a waiting acquire too.
    assert!(pool.poll_acquire(&mut cx).is_pending());
    let woken = count.0.load(Ordering::SeqCst);
    drop(permit);
    assert_eq!(count.0.load(Ordering::SeqCst), woken + 1);
    assert!(pool.poll_acquire(&mut cx).is_ready());
}
