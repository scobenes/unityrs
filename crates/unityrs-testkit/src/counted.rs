//! The counting `#[global_allocator]` of the memory tests (plan section
//! 7.3), and the thread it counts: only the thread under test.
//!
//! libtest runs a test on a thread of its own while its main thread goes on
//! allocating (just after the spawn, and for its "has been running for over
//! 60 seconds" notice), so an allocator that counts every thread reads such
//! an allocation inside a measured window as the test's own, and a check
//! with no slack fails now and then. The core starts no thread of its own
//! ([`crate::waits`]), so the test's thread sees every allocation the core
//! makes for it. A test's first line is [`count_this_thread`], then
//! [`others_are_not_counted`] with the allocator's count.
//!
//! [`Allocator`] is every such allocator: a test file installs one with a
//! [`Tally`] of its own, which says what a block that became live or was
//! freed does to the file's counts. Only the allocator decides what
//! happened: a block counts once `System` has made it, so a failed
//! allocation or `realloc` counts nothing and the block a failed `realloc`
//! leaves stays counted (review round 4: seven test files' copies counted
//! the new block before `realloc` and freed the old one whatever it
//! returned). [`stray_allocators`] finds a global allocator of another
//! kind.
//!
//! Owner: package B.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};

std::thread_local! {
    /// Whether this thread's allocations are counted.
    static COUNTED: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread's allocations are counted. It allocates
/// nothing, so a global allocator may ask it.
#[must_use]
#[inline]
pub fn counted() -> bool {
    COUNTED.try_with(Cell::get).unwrap_or(false)
}

/// Counts the calling thread's allocations from now on.
pub fn count_this_thread() {
    COUNTED.with(|c| c.set(true));
}

/// Checks that an allocation on another thread does not move `live`, the
/// allocator's count: a second thread allocates 64 KiB and holds it while
/// the calling thread, which allocates nothing meanwhile, reads `live`
/// before and after. Run it before the test takes any baseline: the second
/// thread's start and end may leave a constant in the count (a block the
/// calling thread allocated for it and the other thread freed).
///
/// # Panics
///
/// When the other thread's allocation was counted.
pub fn others_are_not_counted(live: impl Fn() -> usize) {
    // Barrier's first wait can box a pthread mutex/condvar on Darwin, on
    // the counted thread and inside the before/during window. Atomics keep
    // the probe itself allocation-free on both Darwin and Linux.
    let step = AtomicU8::new(0);
    std::thread::scope(|s| {
        s.spawn(|| {
            while step.load(Ordering::Acquire) != 1 {
                std::thread::yield_now();
            }
            let block = std::hint::black_box(vec![1_u8; 1 << 16]);
            step.store(2, Ordering::Release);
            while step.load(Ordering::Acquire) != 3 {
                std::thread::yield_now();
            }
            drop(block);
        });
        let before = live();
        step.store(1, Ordering::Release);
        while step.load(Ordering::Acquire) != 2 {
            std::thread::yield_now();
        }
        let during = live();
        step.store(3, Ordering::Release);
        assert_eq!(
            during, before,
            "another thread's allocation moved the count"
        );
    });
}

/// What a test file counts of the blocks its [`Allocator`] sees. It is
/// called inside the allocator, so it must not allocate, and it must not
/// panic: a global allocator that unwinds is undefined behaviour
/// (`GlobalAlloc`'s safety contract), so [`Allocator`] ends any unwind out
/// of a tally's method by aborting the process (review round 6: a tally
/// that panicked unwound through the allocator). A failed `assert!` or
/// `unwrap` in a tally is therefore a `SIGABRT`, after the panic's
/// message, and never a test failure that unwinds.
pub trait Tally: Sync {
    /// Whether a request of `size` bytes fails without reaching `System`,
    /// on any thread (a test that refuses huge requests rather than meet the
    /// machine's limit). Never, by default.
    fn refuses(&self, size: usize) -> bool {
        let _ = size;
        false
    }

    /// Whether a `realloc` to `new_size` bytes fails without reaching
    /// `System`: [`Self::refuses`], by default. A test that refuses one
    /// fresh allocation of a size, and never a growth or shrink the code
    /// under test makes infallibly, answers `false` here.
    fn refuses_realloc(&self, new_size: usize) -> bool {
        self.refuses(new_size)
    }

    /// A block of `size` bytes became live on a counted thread; `zeroed`
    /// when `alloc_zeroed` made it.
    fn grew(&self, size: usize, zeroed: bool);

    /// A block of `size` bytes was freed on a counted thread.
    fn shrank(&self, size: usize);
}

/// A global allocator that forwards to `System` and tells its [`Tally`] of
/// each block made or freed on a [`counted`] thread, once `System` has
/// answered: a failure counts nothing, and a `realloc` that fails leaves its
/// old block live and counted. A growing `realloc` that succeeds counts its
/// new block before it frees the old one, the worst case of a moving
/// `realloc`: both blocks live at once. A shrinking one frees the old block
/// first, since glibc shrinks in place, unless `System` allocates, copies
/// and frees instead (an alignment over `malloc`'s, or a new size below the
/// alignment), which counts as a growth does: the engine's allocator's
/// rule (`unityrs_batch::alloc`). A tally that panics aborts the process
/// ([`Tally`]). A test file installs it with its own tally:
/// `#[global_allocator] static ALLOC: Allocator<Mine> = Allocator(Mine);`.
pub struct Allocator<T>(pub T);

/// `malloc`'s alignment, below which std's `System` calls `realloc` itself
/// (`library/std/src/sys/alloc`'s `MIN_ALIGN`).
const MIN_ALIGN: usize = if cfg!(target_pointer_width = "64") {
    16
} else {
    8
};

/// Aborts the process when dropped: armed around each call into a
/// [`Tally`] and disarmed (forgotten) when the call returns, so it drops
/// only while a tally's panic unwinds, and the unwind ends there instead of
/// leaving the allocator. `abort` neither allocates nor unwinds.
struct NoUnwind;

impl Drop for NoUnwind {
    fn drop(&mut self) {
        std::process::abort();
    }
}

/// Runs `f`, a call into a tally, with no unwind out of it ([`NoUnwind`]).
#[inline]
fn no_unwind<R>(f: impl FnOnce() -> R) -> R {
    let armed = NoUnwind;
    let r = f();
    std::mem::forget(armed);
    r
}

// SAFETY: every method forwards to `System` under the caller's contract,
// which is `System`'s, and only tells the tally sizes, through `no_unwind`,
// so no panic of the tally's unwinds out of a method; a refusal returns
// null, which `GlobalAlloc` allows any allocation to.
unsafe impl<T: Tally> GlobalAlloc for Allocator<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if no_unwind(|| self.0.refuses(layout.size())) {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System.alloc`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && counted() {
            no_unwind(|| self.0.grew(layout.size(), false));
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if no_unwind(|| self.0.refuses(layout.size())) {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System.alloc_zeroed`'s.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() && counted() {
            no_unwind(|| self.0.grew(layout.size(), true));
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counted() {
            no_unwind(|| self.0.shrank(layout.size()));
        }
        // SAFETY: the caller's contract is `System.dealloc`'s: `ptr` came
        // from this allocator, which is `System`'s.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if no_unwind(|| self.0.refuses_realloc(new_size)) {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System.realloc`'s.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() && counted() {
            no_unwind(|| {
                // `System` calls `realloc` itself only for an alignment of
                // at most `malloc`'s and no greater than the new size;
                // otherwise it allocates, copies and frees (std's
                // `realloc_fallback`), both blocks live: as the engine's
                // allocator counts it (`unityrs_batch::alloc`).
                let in_place = layout.align() <= MIN_ALIGN && layout.align() <= new_size;
                if new_size <= layout.size() && in_place {
                    // glibc shrinks a block where it is (a heap chunk split
                    // in place, a mapped one cut by `mremap`): no second
                    // block ever exists (plan section 3.15, perf stage 2).
                    self.0.shrank(layout.size());
                    self.0.grew(new_size, false);
                } else {
                    // A growth may move: both blocks exist until it is done.
                    self.0.grew(new_size, false);
                    self.0.shrank(layout.size());
                }
            });
        }
        p
    }
}

/// Every `file:line` of a `.rs` file under `root` (`target` directories
/// left out) that implements `GlobalAlloc` itself, outside this module:
/// a counting allocator of its own, whose accounting no test here checks.
/// The engine's (`unityrs_batch::alloc`, the binary's) is the one other,
/// its accounting held by its own tests, and its file is left out for that
/// one implementation only: a count there other than one is reported as
/// `<file>: <n> implementations, the exemption is for one`.
/// Comment lines are not scanned.
///
/// # Panics
/// When `root` cannot be read (a test helper).
#[must_use]
pub fn stray_allocators(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    crate::guard::rust_files(root, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in files {
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let hits: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                !line.trim_start().starts_with("//") && line.contains("GlobalAlloc for")
            })
            .map(|(i, _)| format!("{}:{}", path.display(), i + 1))
            .collect();
        // This module, whose scan names the pattern itself.
        if path.ends_with("unityrs-testkit/src/counted.rs") {
            continue;
        }
        // The engine's counting allocator, which its own tests check
        // (`unityrs-batch`'s `tests/alloc.rs`): that one, and no other.
        if path.ends_with("unityrs-batch/src/alloc.rs") {
            if hits.len() != 1 {
                found.push(crate::guard::exemption_broken(
                    &path,
                    hits.len(),
                    "implementations",
                ));
            }
            continue;
        }
        found.extend(hits);
    }
    found
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::hint::black_box;
    use std::sync::Mutex;

    use super::{Allocator, Tally, count_this_thread, stray_allocators};

    /// Every call, in order: `(+size)` grew, `(-size)` shrank.
    struct Log(Mutex<Vec<i64>>);

    impl Tally for Log {
        fn refuses(&self, size: usize) -> bool {
            size == 12_345
        }
        fn grew(&self, size: usize, _zeroed: bool) {
            self.0.lock().unwrap().push(i64::try_from(size).unwrap());
        }
        fn shrank(&self, size: usize) {
            self.0.lock().unwrap().push(-i64::try_from(size).unwrap());
        }
    }

    impl Log {
        fn take(&self) -> Vec<i64> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    /// No request `System` can meet: a `realloc` to it fails and returns null.
    const HUGE: usize = usize::MAX / 4;

    #[test]
    fn a_shrink_below_alignment_counts_both_live_blocks() {
        count_this_thread();
        let a = Allocator(Log(Mutex::new(Vec::new())));
        // Alignment no greater than malloc's: only the new size forces
        // System's allocate/copy/free fallback.
        let layout = Layout::from_size_align(64, super::MIN_ALIGN).unwrap();
        let small = Layout::from_size_align(1, layout.align()).unwrap();
        // SAFETY: non-zero layouts; the live pointer is used with its
        // current layout, retains its first byte, and is freed once.
        unsafe {
            let p = a.alloc(layout);
            assert!(!p.is_null());
            p.write(42);
            let q = a.realloc(p, layout, small.size());
            assert!(!q.is_null());
            assert_eq!(q.read(), 42);
            a.dealloc(q, small);
        }
        assert_eq!(a.0.take(), [64, 1, -64, -1]);
    }

    #[test]
    fn an_exempt_allocator_with_two_implementations_is_reported() {
        let dir = crate::scratch::Scratch::new("allocator-exemption");
        let file = dir.join("unityrs-batch/src/alloc.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "impl GlobalAlloc for A {}\nimpl GlobalAlloc for B {}\n",
        )
        .unwrap();
        assert_eq!(
            stray_allocators(dir.path()),
            [format!(
                "{}: 2 implementations, the exemption is for one",
                file.display()
            )]
        );
    }

    /// A `realloc` that fails counts nothing: its old block stays live and
    /// counted (review round 4: the copies counted the new block, then freed
    /// the old one whatever `realloc` returned). One that succeeds counts
    /// the new block before freeing the old. Each huge request's result goes
    /// through `black_box` before its null check: unused, an optimised build
    /// removes the request and folds the check (review round 6: the test
    /// failed every time under `cargo test --release`).
    #[test]
    fn a_failed_realloc_keeps_the_old_block_counted() {
        count_this_thread();
        // Not the global allocator: the test's own, with a log of its own.
        let a = Allocator(Log(Mutex::new(Vec::new())));
        let layout = Layout::from_size_align(64, 8).unwrap();
        // SAFETY: a non-zero layout; each pointer is used under the layout
        // it was made with, and freed once.
        unsafe {
            let p = a.alloc(layout);
            assert!(!p.is_null());
            assert_eq!(a.0.take(), [64]);
            let q = black_box(a.realloc(p, layout, HUGE));
            assert!(q.is_null(), "System met a request of {HUGE} bytes");
            assert!(a.0.take().is_empty(), "a failed realloc moved the count");
            let q = a.realloc(p, layout, 4096);
            assert!(!q.is_null());
            assert_eq!(
                a.0.take(),
                [4096, -64],
                "the new block before the old is freed"
            );
            let big = Layout::from_size_align(4096, 8).unwrap();
            assert!(a.realloc(q, big, 12_345).is_null(), "a refusal");
            assert!(
                a.alloc(Layout::from_size_align(12_345, 8).unwrap())
                    .is_null()
            );
            assert!(a.0.take().is_empty(), "a refusal counted");
            a.dealloc(q, big);
            assert_eq!(a.0.take(), [-4096]);
            let z = a.alloc_zeroed(layout);
            assert!(!z.is_null() && *z == 0);
            a.dealloc(z, layout);
            assert_eq!(a.0.take(), [64, -64]);
            // A thread that is not counted tells the tally nothing.
            std::thread::scope(|s| {
                s.spawn(|| {
                    let p = a.alloc(layout);
                    a.dealloc(p, layout);
                });
            });
            assert!(a.0.take().is_empty(), "another thread counted");
            // `System` itself, for contrast: the huge request fails there.
            assert!(black_box(System.alloc(Layout::from_size_align(HUGE, 8).unwrap())).is_null());
        }
    }

    /// Panics in every method, as a tally with a failing `assert!` would.
    struct Panicking;

    impl Tally for Panicking {
        fn grew(&self, _: usize, _: bool) {
            panic!("the tally's own panic");
        }
        fn shrank(&self, _: usize) {
            panic!("the tally's own panic");
        }
    }

    const UNWIND_VAR: &str = "UNITYRS_TESTKIT_TALLY_PANIC";

    /// Run by [`a_tally_that_panics_aborts_and_never_unwinds`] in a child:
    /// a counted allocation whose tally panics, under `catch_unwind`. Were
    /// the panic to unwind out of the allocator, `catch_unwind` would catch
    /// it and the probe would pass. `killed:N:DIR` is first killed by
    /// `SIGKILL` from outside on each of its first `N` runs
    /// (`crate::child::kill_self_while`).
    #[test]
    #[ignore = "run by a_tally_that_panics_aborts_and_never_unwinds"]
    fn tally_panic_probe() {
        let Ok(what) = std::env::var(UNWIND_VAR) else {
            return;
        };
        if let Some(spec) = what.strip_prefix("killed:") {
            crate::child::kill_self_while(spec);
        }
        count_this_thread();
        let a = Allocator(Panicking);
        let layout = Layout::from_size_align(64, 8).unwrap();
        let unwound = std::panic::catch_unwind(|| {
            // SAFETY: a non-zero layout. The block leaks if the call returns.
            black_box(unsafe { a.alloc(layout) });
        });
        assert!(unwound.is_ok(), "the tally's panic unwound out of alloc");
    }

    /// A tally that panics ends the process with `SIGABRT` after its
    /// panic's message: no unwind leaves the allocator, which `GlobalAlloc`
    /// forbids (review round 6). The probe runs in a child process, since
    /// it aborts; a `SIGKILL` from outside is run again
    /// ([`crate::child::output`]), and the probe's own abort comes back at
    /// once (review round 8: one kill failed the test).
    #[cfg(unix)]
    #[test]
    fn a_tally_that_panics_aborts_and_never_unwinds() {
        let marks = crate::child::Marks::new("tally-panic-once");
        for (spec, killed) in [
            ("1".to_owned(), 0),
            (format!("killed:{}", marks.killed(1)), 1),
        ] {
            tally_panic_aborts(&spec);
            assert_eq!(marks.runs(), killed, "{spec}");
        }
    }

    /// The tally-panic probe, run with `spec`, aborts after its panic.
    #[cfg(unix)]
    fn tally_panic_aborts(spec: &str) {
        use std::os::unix::process::ExitStatusExt;
        let exe = std::env::current_exe().unwrap();
        let out = crate::child::output(
            std::process::Command::new(exe)
                .args([
                    "--exact",
                    "counted::tests::tally_panic_probe",
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(UNWIND_VAR, spec),
        )
        .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.signal(),
            Some(6),
            "{spec}: the probe did not abort: {:?}\n{err}",
            out.status
        );
        assert!(err.contains("the tally's own panic"), "{spec}: {err}");
        assert!(
            !err.contains("panic in a function that cannot unwind"),
            "{spec}: std's nounwind abort reached instead of our guard: {err}"
        );
    }

    /// Refuses a fresh allocation of 12,345 bytes, never a `realloc`.
    struct FreshOnly(Log);

    impl Tally for FreshOnly {
        fn refuses(&self, size: usize) -> bool {
            self.0.refuses(size)
        }
        fn refuses_realloc(&self, _new_size: usize) -> bool {
            false
        }
        fn grew(&self, size: usize, zeroed: bool) {
            self.0.grew(size, zeroed);
        }
        fn shrank(&self, size: usize) {
            self.0.shrank(size);
        }
    }

    /// A tally that refuses a fresh allocation may let a `realloc` to the
    /// same size by (the dump's staging test: the code under test shrinks
    /// into exact allocations infallibly); by default it refuses both
    /// (`a_failed_realloc_keeps_the_old_block_counted`).
    #[test]
    fn a_tally_may_refuse_fresh_allocations_and_no_realloc() {
        count_this_thread();
        let a = Allocator(FreshOnly(Log(Mutex::new(Vec::new()))));
        let layout = Layout::from_size_align(64, 8).unwrap();
        let refused = Layout::from_size_align(12_345, 8).unwrap();
        // SAFETY: non-zero layouts; each pointer is used under the layout
        // it was made with, and freed once.
        unsafe {
            assert!(a.alloc(refused).is_null());
            assert!(a.alloc_zeroed(refused).is_null());
            let p = a.alloc(layout);
            assert!(!p.is_null());
            let q = a.realloc(p, layout, 12_345);
            assert!(!q.is_null(), "a realloc refused");
            a.dealloc(q, refused);
            assert_eq!(a.0.0.take(), [64, 12_345, -64, -12_345]);
        }
    }

    /// Every global allocator in the workspace is an [`Allocator`] or the
    /// engine's counting allocator (`unityrs_batch::alloc`).
    #[test]
    fn every_global_allocator_in_the_workspace_is_this_one() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let found = stray_allocators(&crates);
        assert!(
            found.is_empty(),
            "global allocators of their own, whose counts no test checks:\n{}",
            found.join("\n")
        );
    }
}
