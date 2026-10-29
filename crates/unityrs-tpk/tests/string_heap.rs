//! A TPK string's 7-bit length prefix can say 2^35 - 1 bytes (`Tpk.py`'s
//! `read_string`, `:445-456`). rabex 0.2.1, whose reader unityrs once used,
//! allocated that much before reading. The port copies a string only once
//! its bytes are known to be there, so a length past the payload's end, which
//! it refuses (`PARITY.md` section 19.3), costs nothing: under a counting
//! allocator, a load whose last string's length runs past a tail of any size
//! holds the same heap at its peak, the provider's own fixed state.
//!
//! The allocator counts only the test's own thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::limits::Limits;
use unityrs_core::provider::TypeTreeProvider;
use unityrs_testkit::counted::{count_this_thread, counted, others_are_not_counted};
use unityrs_tpk::{TpkError, TpkProvider};

/// Counts live heap bytes and their peak; refuses any one request over
/// 1 GiB, so an allocation at the string's length fails the test rather
/// than the machine.
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
        if !p.is_null() && counted() {
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
        if !p.is_null() && counted() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as `System.dealloc`.
        unsafe { System.dealloc(ptr, layout) };
        if counted() {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > REFUSE {
            return std::ptr::null_mut();
        }
        // SAFETY: as `System.realloc`.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() && counted() {
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

/// A stored TPK whose blob has no versions, classes or nodes, one common
/// string (string 0), and one string whose length prefix is `len` over
/// `tail`.
fn tpk(len: u64, tail: &[u8]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&1_i64.to_le_bytes());
    for _ in 0..3 {
        blob.extend_from_slice(&0_i32.to_le_bytes()); // versions, classes, common versions
    }
    blob.extend_from_slice(&1_i32.to_le_bytes()); // common indices
    blob.extend_from_slice(&0_u16.to_le_bytes());
    blob.extend_from_slice(&0_i32.to_le_bytes()); // nodes
    blob.extend_from_slice(&1_i32.to_le_bytes()); // strings
    let mut n = len;
    loop {
        let byte = u8::try_from(n & 0x7F).unwrap();
        n >>= 7;
        if n == 0 {
            blob.push(byte);
            break;
        }
        blob.push(byte | 0x80);
    }
    blob.extend_from_slice(tail);
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    let size = u32::try_from(blob.len()).unwrap().to_le_bytes();
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(&blob);
    out
}

/// One test; the allocator counts its thread only.
#[test]
fn a_string_longer_than_its_tail_costs_nothing_however_long_the_tail() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::SeqCst));
    let limits = Limits::default();
    let mut peaks = Vec::new();
    for tail_len in [0, 1, 100, 8192, 8193, 100_000, 1 << 20] {
        let tail = vec![b'a'; tail_len];
        let bytes = tpk((1 << 35) - 1, &tail);
        let start = LIVE.load(Ordering::SeqCst);
        PEAK.store(start, Ordering::SeqCst);
        let got = TpkProvider::from_bytes(&bytes, &limits);
        let peak = PEAK.load(Ordering::SeqCst) - start;
        match got {
            Err(TpkError::Malformed(m)) => assert!(m.contains("longer than"), "{m}"),
            got => panic!("tail {tail_len}: {got:?}"),
        }
        peaks.push((tail_len, peak));
    }
    let (_, first) = peaks[0];
    assert!(
        first < 1024,
        "a load that fails at its string held {first} bytes"
    );
    assert!(
        peaks.iter().all(|&(_, peak)| peak == first),
        "the peak grew with the tail: {peaks:?}"
    );
    // And a string its tail holds reads whole.
    let provider = TpkProvider::from_bytes(&tpk(5, b"hello"), &limits).unwrap();
    let common: Vec<(u32, Vec<u8>)> = provider
        .common_strings()
        .unwrap()
        .iter()
        .map(|(o, s)| (o, s.as_bytes().to_vec()))
        .collect();
    assert_eq!(common, [(0, b"hello".to_vec())]);
}
