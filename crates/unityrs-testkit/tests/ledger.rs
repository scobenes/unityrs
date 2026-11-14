//! The ledger as a global allocator's tally (`src/ledger.rs`): the
//! registered threads' aggregate heap, coherent with the charges, observed
//! with no allocation of its own. The tests share one ledger, so each holds
//! [`SERIAL`] and counts only what it measures, in [`counting`].
//!
//! No test waits without the deadlock guard, and none spins without giving
//! up the CPU: each body runs under [`guard::finishes`], every rendezvous is
//! a guarded channel, [`until`] yields, and a body's assertions come after
//! its threads are joined, so a failure never leaves a thread waiting on
//! the one that failed (CI's hang: a panic inside `thread::scope` left a
//! peer on an unguarded barrier and a parked request spinning).

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, stop_counting_this_thread};
use unityrs_testkit::guard::{self, DEADLOCK_GUARD};
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

/// Waits until `flag`, allocating nothing and yielding the CPU between
/// looks (a thread parked in the allocator may wait on one, and on a host
/// with fewer cores than threads the one that sets it must run); false past
/// [`DEADLOCK_GUARD`].
fn until(flag: &AtomicBool) -> bool {
    let deadline = Instant::now() + DEADLOCK_GUARD;
    while !flag.load(Ordering::Acquire) {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// Sets its flag when dropped, however the test's thread ends, so a thread
/// waiting on it in [`until`] goes on.
struct Opens<'a>(&'a AtomicBool);

impl Drop for Opens<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Lets the parked request go, and shuts the gate, when dropped.
struct Ungate;

impl Drop for Ungate {
    fn drop(&mut self) {
        GATE.store(0, Ordering::Release);
        GO.store(true, Ordering::Release);
    }
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

/// Runs `f` with the calling thread counted, and uncounted again however
/// `f` ends. Counted, a thread's exit would free what it never made (its
/// thread-locals, its handle) after `thread::scope` has joined it, into
/// whatever window the next test has armed: CI's failure, 256 bytes short
/// of both blocks. Nothing here waits while counted.
fn counting<T>(f: impl FnOnce() -> T) -> T {
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            stop_counting_this_thread();
        }
    }
    count_this_thread();
    let _stop = Stop;
    f()
}

/// Runs a test's body alone on a thread of its own, failing past the
/// deadlock guard instead of hanging. The worker, not the test's thread,
/// holds [`SERIAL`], so the guard covers the wait for it, and a worker the
/// guard leaves behind keeps it until it ends: the next body waits, under
/// its own guard, instead of running beside a hung one that may still
/// count blocks into its window or let [`GO`] go.
fn serially(what: &str, body: fn()) {
    let () = guard::finishes(what, move || {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        body();
    });
}

/// A block made on one registered thread and freed on another balances in
/// the aggregate; an unregistered thread's blocks are never seen, even
/// held while the ledger is read.
#[test]
fn cross_thread_frees_balance_and_unregistered_threads_are_not_counted() {
    serially("the cross-thread test", || {
        let before = ledger().observe();
        let (during, after) = std::thread::scope(|s| {
            let (held_tx, held_rx) = channel::<()>();
            let (release_tx, release_rx) = channel::<()>();
            s.spawn(move || {
                let unseen = black_box(vec![0_u8; 4096]);
                let _ = held_tx.send(());
                guard::hold(&release_rx, "the release of the unregistered block");
                drop(unseen);
            });
            guard::wait(&held_rx, "the unregistered block");
            let (block_tx, block_rx) = channel::<Vec<u8>>();
            s.spawn(move || {
                let block = counting(|| black_box(vec![7_u8; 1000]));
                let _ = block_tx.send(block);
            });
            let block = guard::recv(&block_rx, "the block");
            let during = ledger().observe();
            counting(|| drop(block));
            let after = ledger().observe();
            drop(release_tx);
            (during, after)
        });
        assert_eq!(
            during.heap - before.heap,
            i64::try_from(chunk(1000)).unwrap(),
            "{before:?} {during:?}"
        );
        assert_eq!(after.heap, before.heap, "{before:?} {after:?}");
    });
}

/// Observing allocates nothing: a registered thread's reading moves no
/// event count.
#[test]
fn an_observation_allocates_nothing() {
    serially("the observation test", || {
        let (first, second) = counting(|| {
            ledger().arm(0);
            let first = ledger().observe();
            (first, black_box(ledger().observe()))
        });
        assert_eq!(first.events, second.events);
        assert_eq!(first, second);
    });
}

/// Many threads, each charging before its block and releasing after its
/// free: no event sees a residual past the charges, whatever the
/// interleaving. A thread that allocates first is caught at that event.
#[test]
fn concurrent_charged_blocks_never_pass_and_an_uncharged_one_does() {
    serially("the concurrent test", || {
        let start = AtomicBool::new(false);
        ledger().arm(0);
        std::thread::scope(|s| {
            // Set however this thread ends, so no thread is left waiting.
            let _start = Opens(&start);
            for t in 0..8_u64 {
                let start = &start;
                s.spawn(move || {
                    // Released together, for contention; the spawns are
                    // this thread's, which is not counted.
                    assert!(until(start), "never started");
                    counting(|| {
                        for i in 0..500_u64 {
                            let size = 16 + (t * 37 + i * 13) % 3000;
                            ledger().charge(chunk(size));
                            let block = black_box(vec![1_u8; usize::try_from(size).unwrap()]);
                            drop(block);
                            ledger().release(chunk(size));
                        }
                    });
                });
            }
        });
        let o = ledger().observe();
        assert_eq!(o.first_violation, None, "{o:?}");
        assert!(o.max_residual <= 0, "{o:?}");
        assert_eq!(o.charged, 0);
        // Forced, so one order alone: armed, then a charge of 64, then
        // another thread's block of 256 with no charge of its own, then its
        // free, then the release. Its residual is exactly the block less
        // the charge.
        ledger().arm(0);
        ledger().charge(64);
        std::thread::scope(|s| {
            s.spawn(|| counting(|| drop(black_box(vec![2_u8; 256]))));
        });
        ledger().release(64);
        let o = ledger().observe();
        assert!(
            o.first_violation.is_some(),
            "an uncharged block passed: {o:?}"
        );
        assert_eq!(
            o.max_residual,
            i64::try_from(chunk(256)).unwrap() - 64,
            "{o:?}"
        );
    });
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
    serially("the racing free test", || {
        let both = i64::try_from(2 * chunk(SIZE as u64)).unwrap();
        PARKED.store(false, Ordering::Release);
        GO.store(false, Ordering::Release);
        ledger().arm(1_500_000);
        let parked = std::thread::scope(|s| {
            // Lets the parked request go however this thread ends.
            let ungate = Ungate;
            let (made_tx, made_rx) = channel::<()>();
            let (free_tx, free_rx) = channel::<()>();
            let (freed_tx, freed_rx) = channel::<()>();
            s.spawn(move || {
                let old = counting(|| black_box(vec![1_u8; SIZE]));
                let _ = made_tx.send(());
                guard::wait(&free_rx, "the free of the old block");
                // The old block goes while the new one is asked for: the
                // ledger drops it only once it is gone.
                counting(|| drop(old));
                let _ = freed_tx.send(());
            });
            guard::wait(&made_rx, "the old block");
            GATE.store(SIZE, Ordering::Release);
            s.spawn(|| counting(|| drop(black_box(vec![2_u8; SIZE]))));
            let parked = until(&PARKED).then(|| ledger().observe());
            let _ = free_tx.send(());
            guard::wait(&freed_rx, "the old block's free");
            drop(ungate);
            parked
        });
        let parked = parked.expect("the request never parked");
        assert_eq!(parked.peak_above_arm, both, "{parked:?}");
        assert_eq!(parked.max_residual, both, "{parked:?}");
        assert!(parked.first_violation.is_some(), "the allowance passed");
        let o = ledger().observe();
        assert_eq!(o.max_residual, both, "{o:?}");
        assert!(o.first_violation.is_some(), "{o:?}");
    });
}

/// A moving `realloc` holds both blocks at once: the peak sees both.
#[test]
fn a_growing_realloc_is_observed_with_both_blocks() {
    serially("the realloc test", || {
        let (o, again, grown) = counting(|| {
            let mut v: Vec<u8> = Vec::with_capacity(64);
            ledger().arm(0);
            v.reserve_exact(4096);
            let grown = v.capacity() as u64;
            let o = ledger().observe();
            let again = ledger().observe().heap;
            drop(black_box(v));
            (o, again, grown)
        });
        assert_eq!(o.peak_above_arm, i64::try_from(chunk(grown)).unwrap());
        assert_eq!(o.heap, again, "an observation moved the heap");
    });
}
