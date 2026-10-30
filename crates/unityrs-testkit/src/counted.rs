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
use std::sync::Barrier;

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
    let barrier = Barrier::new(2);
    std::thread::scope(|s| {
        s.spawn(|| {
            barrier.wait();
            let block = std::hint::black_box(vec![1_u8; 1 << 16]);
            barrier.wait();
            barrier.wait();
            drop(block);
        });
        let before = live();
        barrier.wait();
        barrier.wait();
        let during = live();
        barrier.wait();
        assert_eq!(
            during, before,
            "another thread's allocation moved the count"
        );
    });
}

/// What a test file counts of the blocks its [`Allocator`] sees. It is
/// called inside the allocator, so it must not allocate.
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
/// old block live and counted. A `realloc` that succeeds counts its new
/// block before it frees the old one, the worst case of a moving `realloc`:
/// both blocks live at once. A test file installs it with its own tally:
/// `#[global_allocator] static ALLOC: Allocator<Mine> = Allocator(Mine);`.
pub struct Allocator<T>(pub T);

// SAFETY: every method forwards to `System` under the caller's contract,
// which is `System`'s, and only tells the tally sizes; a refusal returns
// null, which `GlobalAlloc` allows any allocation to.
unsafe impl<T: Tally> GlobalAlloc for Allocator<T> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if self.0.refuses(layout.size()) {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System.alloc`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && counted() {
            self.0.grew(layout.size(), false);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if self.0.refuses(layout.size()) {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System.alloc_zeroed`'s.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() && counted() {
            self.0.grew(layout.size(), true);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if counted() {
            self.0.shrank(layout.size());
        }
        // SAFETY: the caller's contract is `System.dealloc`'s: `ptr` came
        // from this allocator, which is `System`'s.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if self.0.refuses_realloc(new_size) {
            return ptr::null_mut();
        }
        // SAFETY: the caller's contract is `System.realloc`'s.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() && counted() {
            self.0.grew(new_size, false);
            self.0.shrank(layout.size());
        }
        p
    }
}

/// Every `file:line` of a `.rs` file under `root` (`target` directories
/// left out) that implements `GlobalAlloc` itself, outside this module:
/// a counting allocator of its own, whose accounting no test here checks.
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
        if path.ends_with("unityrs-testkit/src/counted.rs") {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (i, line) in text.lines().enumerate() {
            if !line.trim_start().starts_with("//") && line.contains("GlobalAlloc for") {
                found.push(format!("{}:{}", path.display(), i + 1));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
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

    /// A `realloc` that fails counts nothing: its old block stays live and
    /// counted (review round 4: the copies counted the new block, then freed
    /// the old one whatever `realloc` returned). One that succeeds counts
    /// the new block before freeing the old.
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
            let q = a.realloc(p, layout, HUGE);
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
            assert!(
                System
                    .alloc(Layout::from_size_align(HUGE, 8).unwrap())
                    .is_null()
            );
        }
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

    /// Every global allocator in the workspace is an [`Allocator`].
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
