//! What glibc holds for an allocation the meter charged, against
//! [`heap_cost`] (plan section 3.15; review round 13: `heap_cost`'s docs said
//! a charge covers the allocator's rounding, so that many small allocations
//! cannot hold more than they were charged, which glibc's rules break in two
//! ways).
//!
//! - glibc hands over a free chunk whole when what would be left of it could
//!   not be a chunk of its own, so a live allocation can hold
//!   [`CHUNK_SLACK`] bytes past its charge, and never more.
//! - glibc's `realloc` keeps a shrunk mapped block mapped, rounded up to a
//!   page, so a buffer whose spare room is given back from a block that may
//!   be mapped is moved to a new allocation instead (`shrink_moves`), and
//!   then holds what its charge covers.
//!
//! Linux and glibc only (`malloc_usable_size` and `mallopt`, which every
//! glibc has); elsewhere each test says it skipped. What each measured:
//!
//!     scripts/dev cargo test -p unityrs-core --test heap_rounding -- --nocapture

#![allow(unsafe_code)]

use std::sync::Arc;

use unityrs_core::budget::{DecodeBuf, Reservation, Unbounded};
use unityrs_core::error::LimitKind;
use unityrs_core::limits::{CHUNK_SLACK, Meter, MeteredVec, heap_cost};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::ffi::{c_int, c_void};

    unsafe extern "C" {
        fn malloc_usable_size(ptr: *mut c_void) -> usize;
        fn mallopt(param: c_int, value: c_int) -> c_int;
    }

    pub const SUPPORTED: bool = true;

    /// What the chunk of the block at `p` holds, at least: its usable bytes
    /// and an 8-byte header (a mapped chunk's is 16, which only rounds a page
    /// further).
    pub fn held(p: *const u8) -> u64 {
        // SAFETY: `p` is the start of a live block from glibc's malloc (the
        // system allocator, this binary's global one), which
        // `malloc_usable_size` only reads.
        let usable = unsafe { malloc_usable_size(p.cast_mut().cast()) };
        u64::try_from(usable).unwrap() + 8
    }

    /// Fixes glibc's `mmap` threshold at its default, 128 KiB, which the
    /// free of a mapped block would otherwise raise (`M_MMAP_THRESHOLD`,
    /// -3), so a block of 1 MiB is mapped whatever ran before.
    pub fn fix_mmap_threshold() {
        // SAFETY: takes and returns integers.
        assert_eq!(unsafe { mallopt(-3, 128 << 10) }, 1, "mallopt");
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

/// 1,000 chunks of 160 bytes, each between two that stay, are freed, and
/// 1,000 requests of 136 bytes follow, whose chunk is 144: glibc gives most
/// of them a free chunk of 160 whole, since the 16 bytes left could not be
/// a chunk (its minimum is 32). Each holds its [`heap_cost`] or
/// [`CHUNK_SLACK`] more, nothing else, and the slack is reached, so the
/// bound `heap_cost`'s docs and plan section 3.15 state is the one glibc
/// keeps.
#[test]
fn a_free_chunk_handed_over_whole_holds_at_most_chunk_slack_past_its_charge() {
    if !glibc::SUPPORTED {
        eprintln!("skipped: glibc's chunks are measured only on Linux with glibc");
        return;
    }
    let mut freed = Vec::with_capacity(1000);
    let mut kept = Vec::with_capacity(1000);
    for _ in 0..1000 {
        freed.push(vec![1_u8; 152]);
        kept.push(vec![2_u8; 24]);
    }
    drop(freed);
    let taken: Vec<Vec<u8>> = (0..1000).map(|_| Vec::with_capacity(136)).collect();
    let charge = heap_cost(136);
    let mut whole = 0;
    for v in &taken {
        let held = glibc::held(v.as_ptr());
        assert!(
            held == charge || held == charge + CHUNK_SLACK,
            "a request of 136 bytes holds {held}, charged {charge}"
        );
        whole += usize::from(held > charge);
    }
    assert!(
        whole > 0,
        "no free chunk was handed over whole: the test no longer reaches glibc's rule"
    );
    println!("{whole} of 1,000 requests of 136 bytes held {CHUNK_SLACK} bytes past {charge}");
    drop(kept);
}

/// The bytes at `p`, `len` of them, hold no more than their charge covers.
fn covered(p: *const u8, len: u64, what: &str) {
    let held = glibc::held(p);
    let charge = heap_cost(len);
    assert!(
        held <= charge + CHUNK_SLACK,
        "{what}: {len} bytes hold {held}, charged {charge}"
    );
    println!("{what}: {len} bytes hold {held}, charged {charge}");
}

/// A decode's buffer and a table, each with room for 1 MiB (a mapped block)
/// of which a few bytes are kept: given back its room, each holds what its
/// charge covers, and a buffer whose kept bytes are still 128 KiB or more
/// is shrunk where it is, to a page-rounded block `heap_cost` covers. Before
/// review round 13 `realloc` shrank the block in place, and glibc kept it
/// mapped: 10 bytes held 4,096 against a charge of 32.
#[test]
fn a_buffer_given_back_its_room_holds_what_its_charge_covers() {
    if !glibc::SUPPORTED {
        eprintln!("skipped: glibc's chunks are measured only on Linux with glibc");
        return;
    }
    glibc::fix_mmap_threshold();
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    for kept in [10_usize, 200_000] {
        // An LZMA block's window of 1 MiB, of which `kept` bytes decoded.
        let mut out = DecodeBuf::new(&meter, 0, 1 << 30, LimitKind::BlockOutput).unwrap();
        out.grow_zeroed(1 << 20).unwrap();
        out.truncate(kept);
        let bytes = out.into_bytes().unwrap();
        assert_eq!(bytes.len(), kept);
        covered(
            bytes.as_ptr(),
            u64::try_from(kept).unwrap(),
            "DecodeBuf::into_bytes",
        );
    }
    // A table with room for 2^17 eight-byte rows, of which 10 are kept.
    let mut rows: MeteredVec<u64> = MeteredVec::with_capacity(&meter, 1 << 17).unwrap();
    for i in 0..10 {
        rows.push(i).unwrap();
    }
    let (rows, charge) = rows.into_boxed_slice().unwrap();
    assert_eq!(charge.bytes(), heap_cost(80));
    covered(rows.as_ptr().cast(), 80, "MeteredVec::into_boxed_slice");
}
