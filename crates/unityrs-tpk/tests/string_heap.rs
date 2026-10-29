//! F3's string site (`crates/unityrs-rabex/UPSTREAM.md`): a TPK string's
//! 7-bit length prefix can say 2^35 - 1 bytes. rabex 0.2.1 allocated that
//! much before reading; the fork reads through `take` and grows by the bytes
//! that exist. Under a counting allocator, the heap a blob read adds above
//! where it started is at most the tail's size, plus what the blob's other
//! tables hold.
//!
//! Its own test binary: the allocator counts every thread, so nothing else
//! runs beside it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts live heap bytes and their peak; refuses any one request over
/// 1 GiB, so the upstream allocation fails the test rather than the machine.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
const REFUSE: usize = 1 << 30;

fn grew(n: usize) {
    let now = LIVE.fetch_add(n, Ordering::SeqCst) + n;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() > REFUSE {
            return std::ptr::null_mut();
        }
        // SAFETY: as `System.alloc`.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.size() > REFUSE {
            return std::ptr::null_mut();
        }
        // SAFETY: as `System.alloc_zeroed`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as `System.dealloc`.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > REFUSE {
            return std::ptr::null_mut();
        }
        // SAFETY: as `System.realloc`.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// A blob with no versions, classes, common strings or nodes, and one string
/// whose length prefix is `len` over `tail`.
fn blob(len: u64, tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1_i64.to_le_bytes());
    for _ in 0..5 {
        out.extend_from_slice(&0_i32.to_le_bytes()); // versions, classes, common, indices, nodes
    }
    out.extend_from_slice(&1_i32.to_le_bytes()); // strings
    let mut n = len;
    loop {
        let byte = u8::try_from(n & 0x7F).unwrap();
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(tail);
    out
}

/// One test, so no other test's allocations run beside it.
#[test]
fn a_string_longer_than_its_tail_costs_at_most_the_tail() {
    for tail_len in [0, 1, 100, 8192, 8193, 100_000] {
        let tail = vec![b'a'; tail_len];
        let bytes = blob((1 << 35) - 1, &tail);
        let start = LIVE.load(Ordering::SeqCst);
        PEAK.store(start, Ordering::SeqCst);
        let got = rabex::tpk::TpkTypeTreeBlob::from_reader_bounded(
            &mut bytes.as_slice(),
            unityrs_tpk::TpkProvider::MAX_RECORDS,
        );
        let peak = PEAK.load(Ordering::SeqCst) - start;
        assert!(got.is_err(), "tail {tail_len}");
        drop(got);
        assert!(
            peak <= tail_len,
            "tail {tail_len}: the read held {peak} bytes above its start"
        );
    }
    // And a string its tail holds reads whole.
    let bytes = blob(5, b"hello");
    let got = rabex::tpk::TpkTypeTreeBlob::from_reader_bounded(
        &mut bytes.as_slice(),
        unityrs_tpk::TpkProvider::MAX_RECORDS,
    )
    .unwrap();
    assert_eq!(got.string_buffer, ["hello"]);
}
