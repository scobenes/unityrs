//! A finished [`Text`] holds what its charge covers (review round 13):
//! `Text::finish` gave back its spare room with `String::shrink_to_fit`, and
//! glibc's `realloc` keeps a shrunk mapped block mapped, rounded up to a page,
//! so a record of 100,000 bytes, written into room of 140,000 (a mapped
//! block), held 102,392 bytes against a charge of 100,016 (this test, with
//! the move undone). It now moves such a record to a new allocation
//! (`unityrs_core::limits::shrink_moves`).
//!
//! Linux and glibc only (`malloc_usable_size` and `mallopt`, which every
//! glibc has); elsewhere the test says it skipped.

#![allow(unsafe_code)]

use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{CHUNK_SLACK, Meter, heap_cost};
use unityrs_dump::text::{Sink, Text};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::ffi::{c_int, c_void};

    unsafe extern "C" {
        fn malloc_usable_size(ptr: *mut c_void) -> usize;
        fn mallopt(param: c_int, value: c_int) -> c_int;
    }

    pub const SUPPORTED: bool = true;

    /// What the chunk of the block at `p` holds, at least: its usable bytes
    /// and an 8-byte header (a mapped chunk's is 16).
    pub fn held(p: *const u8) -> u64 {
        // SAFETY: `p` is the start of a live block from glibc's malloc (the
        // system allocator, this binary's global one), which
        // `malloc_usable_size` only reads.
        let usable = unsafe { malloc_usable_size(p.cast_mut().cast()) };
        u64::try_from(usable).unwrap() + 8
    }

    /// Fixes glibc's `mmap` threshold at its default, 128 KiB
    /// (`M_MMAP_THRESHOLD`, -3), so room of 128 KiB can be mapped whatever
    /// ran before, and asks for no spare room when a heap grows
    /// (`M_TOP_PAD`, -2), so a block that outgrows its heap is moved: a
    /// text's room, grown by `realloc`, is then a mapped block once it
    /// reaches 128 KiB, as it is wherever its heap has no room left.
    pub fn fix_mmap_threshold() {
        // SAFETY: each takes and returns integers.
        assert_eq!(unsafe { mallopt(-3, 128 << 10) }, 1, "M_MMAP_THRESHOLD");
        assert_eq!(unsafe { mallopt(-2, 0) }, 1, "M_TOP_PAD");
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod glibc {
    pub const SUPPORTED: bool = false;

    pub fn held(_: *const u8) -> u64 {
        0
    }

    pub fn fix_mmap_threshold() {}
}

#[test]
fn a_finished_text_holds_what_its_charge_covers() {
    if !glibc::SUPPORTED {
        eprintln!("skipped: glibc's chunks are measured only on Linux with glibc");
        return;
    }
    glibc::fix_mmap_threshold();
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let mut text = Text::on(&meter);
    // Room of 70,000 bytes, then of 140,000 (doubled), 100,000 written.
    text.push_str(&"a".repeat(70_000));
    text.push_str(&"b".repeat(30_000));
    let (s, charge) = text.finish().unwrap();
    let len = u64::try_from(s.len()).unwrap();
    assert_eq!(len, 100_000);
    assert_eq!(charge.bytes(), heap_cost(len));
    let held = glibc::held(s.as_ptr());
    assert!(
        held <= heap_cost(len) + CHUNK_SLACK,
        "a text of {len} bytes holds {held}, charged {}",
        heap_cost(len)
    );
}
