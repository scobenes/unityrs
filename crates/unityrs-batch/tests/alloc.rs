//! The engine's counting allocator (`unityrs_batch::alloc`,
//! `docs/plans/batch.md` 2.7), installed here as the binary installs it,
//! its hook feeding a testkit `Tally` through [`Fed`] as the engine's
//! `tally` hook does (2.11): each block at its `heap_cost`, a growing
//! `realloc` at its peak, a shrinking one in place unless `System` moves
//! it, the phase-1 measurement, and, in children, a refused allocation
//! counted with no allocation of the allocator's own, and a hook's panic
//! aborting the process instead of unwinding out of the allocator.

use std::alloc::Layout;
use std::hint::black_box;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use unityrs_batch::alloc::{self, Counting, Hook};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::child;
use unityrs_testkit::counted::{Tally, count_this_thread, counted};

/// The testkit's `Tally` fed by the engine's allocator: its counted
/// threads' blocks, and its refusals on every thread.
struct Fed<T: Tally>(T);

impl<T: Tally> Hook for Fed<T> {
    fn refuses(&self, size: usize) -> bool {
        self.0.refuses(size)
    }
    fn refuses_realloc(&self, new_size: usize) -> bool {
        self.0.refuses_realloc(new_size)
    }
    fn grew(&self, size: usize, zeroed: bool) {
        if counted() {
            self.0.grew(size, zeroed);
        }
    }
    fn shrank(&self, size: usize) {
        if counted() {
            self.0.shrank(size);
        }
    }
}

/// The blocks the counted threads made.
static GREW: AtomicU64 = AtomicU64::new(0);
/// Whether a request of [`REFUSED`] bytes is refused.
static REFUSE: AtomicBool = AtomicBool::new(false);
const REFUSED: usize = 12_345;
/// Whether a block of [`PANICKED`] bytes makes the hook panic.
static PANIC: AtomicBool = AtomicBool::new(false);
const PANICKED: usize = 23_456;

struct Probe;

impl Tally for Probe {
    fn refuses(&self, size: usize) -> bool {
        size == REFUSED && REFUSE.load(Ordering::Relaxed)
    }
    fn grew(&self, size: usize, _zeroed: bool) {
        // Disarmed before the panic, whose own blocks then pass.
        assert!(
            !(size == PANICKED && PANIC.swap(false, Ordering::Relaxed)),
            "the hook panics"
        );
        GREW.fetch_add(1, Ordering::Relaxed);
    }
    fn shrank(&self, _size: usize) {}
}

#[global_allocator]
static ALLOC: Counting<Fed<Probe>> = Counting(Fed(Probe));

fn diff(a: u64, b: u64) -> i64 {
    i64::try_from(a - b).unwrap()
}

fn cost(n: usize) -> i64 {
    i64::try_from(heap_cost(n as u64)).unwrap()
}

/// Each block counts at its `heap_cost` while it lives, on its thread.
#[test]
fn a_block_counts_at_its_heap_cost_while_it_lives() {
    let start = alloc::live();
    let v: Vec<u8> = black_box(Vec::with_capacity(1000));
    let during = alloc::live();
    drop(v);
    let after = alloc::live();
    assert_eq!(during - start, cost(1000));
    assert_eq!(after, start);
}

/// A growing `realloc` counts both blocks at its peak; a shrinking one
/// shrinks in place, but one `System` moves (an alignment over 16, or a
/// new size below the alignment) counts at its peak too.
#[test]
fn a_realloc_counts_at_its_peak_unless_it_shrinks_in_place() {
    let start = alloc::live();
    alloc::reset_peak();
    let mut v: Vec<u8> = black_box(Vec::with_capacity(100));
    v.reserve_exact(10_000);
    let (grown, grown_peak) = (alloc::live(), alloc::peak());
    // From the grown block alone, so a shrink counted at its peak (the new
    // block added before the old is subtracted) would raise it.
    alloc::reset_peak();
    v.shrink_to(50);
    let (shrunk, shrunk_peak) = (alloc::live(), alloc::peak());
    drop(v);
    assert_eq!(grown - start, cost(10_000));
    assert_eq!(grown_peak - start, cost(100) + cost(10_000));
    assert_eq!(shrunk - start, cost(50));
    assert_eq!(shrunk_peak, grown, "a shrink in place adds no peak");

    // An alignment `malloc` gives (16), but a new size below it: std's
    // `realloc_fallback` moves it.
    let small = Layout::from_size_align(64, 16).unwrap();
    // SAFETY: a non-zero layout; the block is reallocated under it and
    // freed once under the layout of its new size.
    unsafe {
        let p = std::alloc::alloc(small);
        assert!(!p.is_null());
        alloc::reset_peak();
        let before = alloc::live();
        let q = std::alloc::realloc(p, small, 8);
        assert!(!q.is_null());
        let (after, peak) = (alloc::live(), alloc::peak());
        std::alloc::dealloc(q, Layout::from_size_align(8, 16).unwrap());
        assert_eq!(after - before, cost(8) - cost(64));
        assert_eq!(
            peak - before,
            cost(8),
            "a shrink below its alignment counts at its peak"
        );
    }

    let layout = Layout::from_size_align(4096, 32).unwrap();
    // SAFETY: a non-zero layout; the block is reallocated under it and
    // freed once under the layout of its new size.
    unsafe {
        let p = std::alloc::alloc(layout);
        assert!(!p.is_null());
        alloc::reset_peak();
        let before = alloc::live();
        let q = std::alloc::realloc(p, layout, 64);
        assert!(!q.is_null());
        let (after, peak) = (alloc::live(), alloc::peak());
        std::alloc::dealloc(q, Layout::from_size_align(64, 32).unwrap());
        assert_eq!(after - before, cost(64) - cost(4096));
        assert_eq!(peak - before, cost(64), "a moved shrink counts at its peak");
    }
    assert_eq!(alloc::live(), start);
}

/// Phase 1's measurement: what the closure left and its peak above the
/// start, the peak reset first (`docs/plans/batch.md` 2.7, 5.3).
#[test]
fn measured_gives_what_is_kept_and_the_peak_above_the_start() {
    let old = black_box(vec![0_u8; 50_000]);
    drop(old);
    let (kept, m) = alloc::measured(|| {
        let scratch = black_box(vec![0_u8; 5000]);
        drop(scratch);
        black_box(Vec::<u8>::with_capacity(100))
    });
    assert_eq!(i64::try_from(m.kept).unwrap(), cost(100));
    assert_eq!(i64::try_from(m.peak).unwrap(), cost(5000));
    drop(kept);
}

const PROBE_VAR: &str = "UNITYRS_BATCH_ALLOC_PROBE";

/// Run by [`a_refused_allocation_is_counted_and_counting_allocates_nothing`]
/// in a child, on a fresh thread, so the allocator's thread-locals are
/// first touched there: one allocation tells the hook of one block (an
/// allocation of the allocator's own would be a second, through itself),
/// reading the counts tells it of none, and a refused allocation and a
/// refused `realloc` each count one refusal on that thread alone and no
/// block.
#[test]
#[ignore = "run by a_refused_allocation_is_counted_and_counting_allocates_nothing"]
fn refusal_probe() {
    if std::env::var_os(PROBE_VAR).is_none() {
        return;
    }
    let outer = alloc::refusals();
    let seen = std::thread::spawn(|| {
        count_this_thread();
        let g0 = GREW.load(Ordering::Relaxed);
        let (r0, l0) = (alloc::refusals(), alloc::live());
        let _ = (alloc::peak(), alloc::refusals(), alloc::live());
        let g_read = GREW.load(Ordering::Relaxed);
        let b = black_box(Box::new([7_u8; 100]));
        let (g1, l1) = (GREW.load(Ordering::Relaxed), alloc::live());
        REFUSE.store(true, Ordering::Relaxed);
        let fresh = black_box(Vec::<u8>::new())
            .try_reserve_exact(REFUSED)
            .is_err();
        let r1 = alloc::refusals();
        let mut v: Vec<u8> = Vec::with_capacity(16);
        let (g2, l2) = (GREW.load(Ordering::Relaxed), alloc::live());
        let grown = v.try_reserve_exact(REFUSED).is_err();
        let (r2, g3, l3) = (
            alloc::refusals(),
            GREW.load(Ordering::Relaxed),
            alloc::live(),
        );
        REFUSE.store(false, Ordering::Relaxed);
        drop((b, v));
        [
            diff(g_read, g0),
            diff(g1, g0),
            l1 - l0,
            i64::from(fresh),
            diff(r1, r0),
            i64::from(grown),
            diff(r2, r1),
            diff(g3, g2),
            l3 - l2,
        ]
    })
    .join()
    .unwrap();
    let want = [0, 1, cost(100), 1, 1, 1, 1, 0, 0];
    assert_eq!(seen, want, "reads, one block, its cost, refusals, no block");
    assert_eq!(alloc::refusals(), outer, "another thread's refusals");
    println!("refusal probe passed");
}

/// The probe in a child of its own, where its refusals and its fresh
/// thread are the process's only ones of their kind; a `SIGKILL` from
/// outside is run again (`unityrs_testkit::child`).
#[test]
fn a_refused_allocation_is_counted_and_counting_allocates_nothing() {
    let exe = std::env::current_exe().unwrap();
    let out = child::output(
        Command::new(exe)
            .args([
                "--exact",
                "refusal_probe",
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(PROBE_VAR, "1"),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}\n{err}");
    assert!(text.contains("refusal probe passed"), "{text}\n{err}");
}

const PANIC_VAR: &str = "UNITYRS_BATCH_ALLOC_PANIC";

/// Run by [`a_hook_s_panic_aborts_the_process`] in a child: a block the
/// hook panics on, on a counted thread. The panic must end the process
/// with `SIGABRT` inside the allocator, so neither line after it is
/// written.
#[test]
#[ignore = "run by a_hook_s_panic_aborts_the_process"]
fn panic_probe() {
    if std::env::var_os(PANIC_VAR).is_none() {
        return;
    }
    let r = std::panic::catch_unwind(|| {
        count_this_thread();
        println!("panic probe armed");
        PANIC.store(true, Ordering::Relaxed);
        black_box(Vec::<u8>::with_capacity(PANICKED))
    });
    println!("panic probe unwound: {}", r.is_err());
}

/// A hook's panic aborts (134 in a shell, `SIGABRT`) instead of unwinding
/// out of the allocator, even under a `catch_unwind` that would catch it.
#[test]
fn a_hook_s_panic_aborts_the_process() {
    let exe = std::env::current_exe().unwrap();
    let out = child::output(
        Command::new(exe)
            .args([
                "--exact",
                "panic_probe",
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(PANIC_VAR, "1"),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{text}\n{err}");
    assert!(text.contains("panic probe armed"), "{text}\n{err}");
    assert!(!text.contains("panic probe unwound"), "{text}\n{err}");
    assert!(err.contains("the hook panics"), "{err}");
}
