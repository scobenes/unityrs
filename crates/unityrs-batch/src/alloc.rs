//! The counting allocator (`docs/plans/batch.md` 2.7's "The counting
//! allocator", A13; `docs/plans/extract.md` E49, E59): the binary's
//! `#[global_allocator]`, which wraps `System` and keeps three counts per
//! thread, each a const-initialised thread-local `Cell` that allocates
//! nothing:
//!
//! - [`refusals`]: each null it returns, `System`'s or a [`Hook`]'s
//!   refusal. A worker reads it before and after an attempt, and any rise
//!   makes the attempt's outcome `host_memory`, absorbed refusals included
//!   (2.7). zlib's blocks come through here too: `unityrs-imageio`'s
//!   `zalloc` and `zfree` call Rust's allocator (2.6 step 10).
//! - [`live`] and [`peak`]: the thread's live bytes in the charge unit
//!   (`unityrs_core::limits::heap_cost`, glibc's chunk), and their peak,
//!   which the main thread reads across phase 1 to measure `C_spec` and
//!   `p` (5.3, [`measured`]). A growing `realloc` counts at its peak, the
//!   new block added before the old is subtracted. A shrinking one
//!   subtracts the old block, then adds the new, since glibc shrinks in
//!   place, unless `System::realloc` allocates, copies and frees instead
//!   (an alignment over `malloc`'s, or a new size below the alignment:
//!   std's `realloc_fallback`), which counts at its peak as a growth does.
//!
//! A [`Hook`] sees each request before `System` does and each block made
//! or freed after (a free once the block is gone), and may refuse a
//! request: the binary's is [`NoHook`]; a test's feeds the testkit's `Tally`
//! (`docs/plans/batch.md` 2.11's `tally` row), which a test file bridges
//! to [`Hook`] (`tests/alloc.rs`), so the engine never depends on the
//! testkit. A hook that panics aborts the process: a global allocator must
//! not unwind.
//!
//! With [`crate::sys`], this is one of the crate's two modules with
//! `unsafe` code (`tests/unsafe_scan.rs` pins both): `GlobalAlloc`'s
//! methods forwarding to `System`, and fallible boxed file-adapter storage.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use unityrs_core::limits::heap_cost;

std::thread_local! {
    /// Nulls this allocator returned on this thread.
    static REFUSALS: Cell<u64> = const { Cell::new(0) };
    /// This thread's live bytes, in `heap_cost` units: a block freed on
    /// another thread than its own moves both threads' counts.
    static LIVE: Cell<i64> = const { Cell::new(0) };
    /// The most [`LIVE`] has been since the thread's start or its last
    /// [`reset_peak`].
    static PEAK: Cell<i64> = const { Cell::new(0) };
}

/// `malloc`'s alignment, below which std's `System` calls `realloc` itself
/// (`library/std/src/sys/alloc`'s `MIN_ALIGN`).
const MIN_ALIGN: usize = if cfg!(target_pointer_width = "64") {
    16
} else {
    8
};

/// The nulls this allocator has returned on the calling thread.
#[must_use]
pub fn refusals() -> u64 {
    REFUSALS.try_with(Cell::get).unwrap_or(0)
}

/// The calling thread's live bytes, in `heap_cost` units (negative when it
/// freed more of other threads' blocks than it holds of its own).
#[must_use]
pub fn live() -> i64 {
    LIVE.try_with(Cell::get).unwrap_or(0)
}

/// The calling thread's peak of [`live`] since its start or its last
/// [`reset_peak`].
#[must_use]
pub fn peak() -> i64 {
    PEAK.try_with(Cell::get).unwrap_or(0)
}

/// Sets the calling thread's peak to its live count.
pub fn reset_peak() {
    let _ = PEAK.try_with(|p| p.set(live()));
}

/// What the main thread's counts say of phase 1 (`docs/plans/batch.md`
/// 5.3), each in `heap_cost` units, from its live count at the start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    /// The heap left at the end: `C_spec`'s measured part.
    pub kept: u64,
    /// The peak above the start: `p`.
    pub peak: u64,
}

/// Runs `f` on the calling thread, its peak reset to its live count first
/// (2.7), and gives what `f` left and its peak. Under any other global
/// allocator both read 0.
pub fn measured<T>(f: impl FnOnce() -> T) -> (T, Measured) {
    reset_peak();
    let start = live();
    let out = f();
    let above = |n: i64| u64::try_from(n.saturating_sub(start)).unwrap_or(0);
    let m = Measured {
        kept: above(live()),
        peak: above(peak()),
    };
    (out, m)
}

/// What a [`Counting`] allocator's owner sees of its blocks. It is called
/// inside the allocator, on every thread, so it must not allocate, and it
/// must not panic: one that does aborts the process.
pub trait Hook: Sync {
    /// Whether a request of `size` bytes is refused without reaching
    /// `System`. Never, by default.
    fn refuses(&self, size: usize) -> bool {
        let _ = size;
        false
    }

    /// Whether a `realloc` to `new_size` bytes is refused: [`Self::refuses`]
    /// by default.
    fn refuses_realloc(&self, new_size: usize) -> bool {
        self.refuses(new_size)
    }

    /// A block of `size` bytes is about to be asked of `System` (a fresh
    /// block, or a `realloc`'s new one): told before it can be live, so a
    /// hook that counts here never holds less than the live heap.
    fn growing(&self, size: usize) {
        let _ = size;
    }

    /// The request [`Self::growing`] told of failed: no block was made.
    fn failed(&self, size: usize) {
        let _ = size;
    }

    /// A block of `size` bytes became live; `zeroed` when `alloc_zeroed`
    /// made it.
    fn grew(&self, size: usize, zeroed: bool) {
        let _ = (size, zeroed);
    }

    /// A block of `size` bytes was freed: told once `System` has freed it.
    fn shrank(&self, size: usize) {
        let _ = size;
    }
}

/// The binary's hook: sees nothing, refuses nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHook;

impl Hook for NoHook {}

/// The counting allocator: `System`, its nulls and its thread's live bytes
/// counted, each block told to its [`Hook`] once `System` has answered.
/// The binary installs it as
/// `#[global_allocator] static A: Counting = Counting::new();`.
#[derive(Debug, Default)]
pub struct Counting<H: Hook = NoHook>(pub H);

impl Counting {
    /// The allocator with no hook.
    #[must_use]
    pub const fn new() -> Self {
        Self(NoHook)
    }
}

/// `heap_cost` of `size`, signed.
fn cost(size: usize) -> i64 {
    i64::try_from(heap_cost(size as u64)).unwrap_or(i64::MAX)
}

/// Adds `size`'s cost to the thread's live count, raising its peak.
fn add(size: usize) {
    let _ = LIVE.try_with(|l| {
        let now = l.get().saturating_add(cost(size));
        l.set(now);
        let _ = PEAK.try_with(|p| p.set(p.get().max(now)));
    });
}

/// Subtracts `size`'s cost from the thread's live count.
fn sub(size: usize) {
    let _ = LIVE.try_with(|l| l.set(l.get().saturating_sub(cost(size))));
}

fn refused() {
    let _ = REFUSALS.try_with(|r| r.set(r.get().saturating_add(1)));
}

/// Aborts the process when dropped: armed around each call into a hook,
/// and forgotten when the call returns, so it drops only while a hook's
/// panic unwinds. `abort` neither allocates nor unwinds.
struct NoUnwind;

impl Drop for NoUnwind {
    fn drop(&mut self) {
        std::process::abort();
    }
}

#[inline]
fn no_unwind<R>(f: impl FnOnce() -> R) -> R {
    let armed = NoUnwind;
    let r = f();
    std::mem::forget(armed);
    r
}

impl<H: Hook> Counting<H> {
    /// A fresh block's answer: counted when made, a null counted as a
    /// refusal.
    fn made(&self, p: *mut u8, size: usize, zeroed: bool) -> *mut u8 {
        if p.is_null() {
            refused();
            no_unwind(|| self.0.failed(size));
        } else {
            add(size);
            no_unwind(|| self.0.grew(size, zeroed));
        }
        p
    }
}

// SAFETY: every method forwards to `System` under the caller's contract,
// which is `System`'s, and otherwise only updates this thread's `Cell`s
// (const-initialised, with no destructor, so their access never allocates
// or fails) and tells the hook sizes through `no_unwind`, so nothing
// unwinds out of a method; a refusal returns null, which `GlobalAlloc`
// allows any allocation to.
unsafe impl<H: Hook> GlobalAlloc for Counting<H> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if no_unwind(|| self.0.refuses(layout.size())) {
            refused();
            return std::ptr::null_mut();
        }
        no_unwind(|| self.0.growing(layout.size()));
        // SAFETY: the caller's contract is `System.alloc`'s.
        let p = unsafe { System.alloc(layout) };
        self.made(p, layout.size(), false)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if no_unwind(|| self.0.refuses(layout.size())) {
            refused();
            return std::ptr::null_mut();
        }
        no_unwind(|| self.0.growing(layout.size()));
        // SAFETY: the caller's contract is `System.alloc_zeroed`'s.
        let p = unsafe { System.alloc_zeroed(layout) };
        self.made(p, layout.size(), true)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        sub(layout.size());
        // SAFETY: the caller's contract is `System.dealloc`'s: `ptr` came
        // from this allocator, which is `System`'s.
        unsafe { System.dealloc(ptr, layout) }
        // The hook hears of a free once the block is gone.
        no_unwind(|| self.0.shrank(layout.size()));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if no_unwind(|| self.0.refuses_realloc(new_size)) {
            refused();
            return std::ptr::null_mut();
        }
        no_unwind(|| self.0.growing(new_size));
        // SAFETY: the caller's contract is `System.realloc`'s.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            // The old block stays live and counted.
            refused();
            no_unwind(|| self.0.failed(new_size));
            return p;
        }
        let old = layout.size();
        let in_place = layout.align() <= MIN_ALIGN && layout.align() <= new_size;
        if new_size <= old && in_place {
            sub(old);
            add(new_size);
            no_unwind(|| {
                self.0.shrank(old);
                self.0.grew(new_size, false);
            });
        } else {
            // Both blocks may be live while it copies: its peak.
            add(new_size);
            sub(old);
            no_unwind(|| {
                self.0.grew(new_size, false);
                self.0.shrank(old);
            });
        }
        p
    }
}

/// Fallible storage for the spool's small file adapters. The caller's fixed
/// overhead charge covers the allocation, including its `heap_cost`.
pub(crate) fn try_box<T>(value: T) -> std::io::Result<Box<T>> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return Ok(Box::new(value));
    }
    // SAFETY: the layout has nonzero size and valid alignment. `alloc` uses
    // the global allocator, including its refusal counter and test hook.
    let ptr = unsafe { std::alloc::alloc(layout) }.cast::<T>();
    if ptr.is_null() {
        return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory));
    }
    // SAFETY: ptr owns one properly aligned T-sized block; initialize it once
    // and transfer it to Box, which drops T and frees the same global layout.
    Ok(unsafe {
        ptr.write(value);
        Box::from_raw(ptr)
    })
}
