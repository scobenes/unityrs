//! The ledger as a global allocator's tally (`src/ledger.rs`): the
//! registered threads' aggregate heap, coherent with the charges, observed
//! with no allocation of its own. The tests share one ledger, so each holds
//! [`SERIAL`] and registers only the threads it measures.

use std::hint::{black_box, spin_loop};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex, PoisonError};
use std::time::Instant;

use unityrs_testkit::counted::{Allocator, Tally, count_this_thread};
use unityrs_testkit::guard::DEADLOCK_GUARD;
use unityrs_testkit::ledger::Ledger;

/// The charge unit, glibc's chunk below the mapping threshold: the core's
/// `heap_cost` for the sizes these tests make.
fn chunk(n: u64) -> u64 {
    if n == 0 {
        0
    } else {
        (n + 8 + 15).max(32) & !15
    }
}

/// The ledger, with a gate that parks the one request of [`GATE`]'s size
/// in its `growing` call (counted, `System` not yet asked) until [`GO`]:
/// a forced interleaving, no timing.
struct Gated(Ledger);

/// The size whose next request parks, once; 0 for none.
static GATE: AtomicUsize = AtomicUsize::new(0);
/// A request is parked.
static PARKED: AtomicBool = AtomicBool::new(false);
/// The parked request may go on.
static GO: AtomicBool = AtomicBool::new(false);

/// Spins until `flag`, allocating nothing; false past [`DEADLOCK_GUARD`].
fn until(flag: &AtomicBool) -> bool {
    let deadline = Instant::now() + DEADLOCK_GUARD;
    while !flag.load(Ordering::Acquire) {
        if Instant::now() > deadline {
            return false;
        }
        spin_loop();
    }
    true
}

impl Tally for Gated {
    fn growing(&self, size: usize) {
        self.0.growing(size);
        let gated = GATE.compare_exchange(size, 0, Ordering::AcqRel, Ordering::Acquire);
        if size != 0 && gated.is_ok() {
            PARKED.store(true, Ordering::Release);
            // Past the guard it goes on, and the test's own wait fails.
            let _ = until(&GO);
        }
    }
    fn failed(&self, size: usize) {
        self.0.failed(size);
    }
    fn grew(&self, size: usize, zeroed: bool) {
        self.0.grew(size, zeroed);
    }
    fn shrank(&self, size: usize) {
        self.0.shrank(size);
    }
}

#[global_allocator]
static ALLOC: Allocator<Gated> = Allocator(Gated(Ledger::new(chunk)));

static SERIAL: Mutex<()> = Mutex::new(());

fn ledger() -> &'static Ledger {
    &ALLOC.0.0
}

/// A block made on one registered thread and freed on another balances in
/// the aggregate; an unregistered thread's blocks are never seen.
#[test]
fn cross_thread_frees_balance_and_unregistered_threads_are_not_counted() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let barrier = Barrier::new(2);
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            barrier.wait();
            let block = black_box(vec![7_u8; 1000]);
            tx.send(block).unwrap();
            barrier.wait();
        });
        s.spawn(|| {
            let _ = black_box(vec![0_u8; 4096]);
        });
        let before = ledger().observe();
        barrier.wait();
        let block = unityrs_testkit::guard::recv(&rx, "the block");
        let during = ledger().observe();
        barrier.wait();
        count_this_thread();
        drop(block);
        let after = ledger().observe();
        assert!(during.heap - before.heap >= 1000, "{before:?} {during:?}");
        assert!(after.heap <= before.heap + 64, "{before:?} {after:?}");
    });
}

/// Observing allocates nothing: a registered thread's reading moves no
/// event count.
#[test]
fn an_observation_allocates_nothing() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            ledger().arm(0);
            let first = ledger().observe();
            let second = black_box(ledger().observe());
            assert_eq!(first.events, second.events);
            assert_eq!(first, second);
        });
    });
}

/// Many threads, each charging before its block and releasing after its
/// free: no event sees a residual past the charges, whatever the
/// interleaving. A thread that allocates first is caught at that event.
#[test]
fn concurrent_charged_blocks_never_pass_and_an_uncharged_one_does() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let start = Barrier::new(9);
    ledger().arm(0);
    std::thread::scope(|s| {
        for t in 0..8_u64 {
            let start = &start;
            s.spawn(move || {
                count_this_thread();
                start.wait();
                for i in 0..500_u64 {
                    let size = 16 + (t * 37 + i * 13) % 3000;
                    ledger().charge(chunk(size));
                    let block = black_box(vec![1_u8; usize::try_from(size).unwrap()]);
                    drop(block);
                    ledger().release(chunk(size));
                }
            });
        }
        // Released once every thread is registered; the scope's own
        // spawns are this thread's, which is not counted.
        start.wait();
    });
    let o = ledger().observe();
    assert_eq!(o.first_violation, None, "{o:?}");
    assert!(o.max_residual <= 0, "{o:?}");
    assert_eq!(o.charged, 0);
    // Forced, so one order alone: armed, then the other thread's charge of
    // 64, then this block of 256 with no charge of its own, then its free,
    // then the release. Its residual is exactly the block less the charge.
    let forced = Barrier::new(2);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            ledger().arm(0);
            forced.wait();
            forced.wait();
            let block = black_box(vec![2_u8; 256]);
            drop(block);
            forced.wait();
        });
        forced.wait();
        ledger().charge(64);
        forced.wait();
        forced.wait();
        ledger().release(64);
    });
    let o = ledger().observe();
    assert!(
        o.first_violation.is_some(),
        "an uncharged block passed: {o:?}"
    );
    assert!(
        o.max_residual >= i64::try_from(chunk(256)).unwrap() - 64,
        "{o:?}"
    );
}

/// Codex's interleaving (the B12a review): one registered thread frees a
/// 1 MB block while another makes one. Told only once `System` answered,
/// the free was counted before its block went and the new block after it
/// came, so the ledger read 1,000,005 bytes while 2,000,008 were live, and
/// a 1,500,000-byte allowance passed. Counted conservatively, the new block
/// is counted before `System` is asked and the old one uncounted after it
/// is gone: parked at the request, with the old block not yet freed, both
/// are counted, and the allowance is passed.
#[test]
fn a_free_racing_an_allocation_is_counted_with_both_blocks() {
    const SIZE: usize = 1_000_000;
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let both = i64::try_from(2 * chunk(SIZE as u64)).unwrap();
    let made = Barrier::new(2);
    let free = Barrier::new(2);
    let freed = Barrier::new(2);
    PARKED.store(false, Ordering::Release);
    GO.store(false, Ordering::Release);
    ledger().arm(1_500_000);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            let old = black_box(vec![1_u8; SIZE]);
            made.wait();
            free.wait();
            drop(old);
            freed.wait();
        });
        made.wait();
        GATE.store(SIZE, Ordering::Release);
        s.spawn(|| {
            count_this_thread();
            drop(black_box(vec![2_u8; SIZE]));
        });
        assert!(until(&PARKED), "the request never parked");
        let parked = ledger().observe();
        assert!(parked.peak_above_arm >= both, "{parked:?}");
        assert!(parked.max_residual >= both, "{parked:?}");
        assert!(parked.first_violation.is_some(), "the allowance passed");
        // The old block goes while the new one is asked for: the ledger
        // drops it only once it is gone.
        free.wait();
        freed.wait();
        GO.store(true, Ordering::Release);
    });
    let o = ledger().observe();
    assert!(o.max_residual >= both, "{o:?}");
    assert!(o.first_violation.is_some(), "{o:?}");
}

/// A moving `realloc` holds both blocks at once: the peak sees both.
#[test]
fn a_growing_realloc_is_observed_with_both_blocks() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    std::thread::scope(|s| {
        s.spawn(|| {
            count_this_thread();
            let mut v: Vec<u8> = Vec::with_capacity(64);
            ledger().arm(0);
            v.reserve_exact(4096);
            let grown = v.capacity() as u64;
            let o = ledger().observe();
            assert_eq!(o.peak_above_arm, i64::try_from(chunk(grown)).unwrap());
            assert_eq!(
                o.heap,
                ledger().observe().heap,
                "an observation moved the heap"
            );
            drop(black_box(v));
        });
    });
}
