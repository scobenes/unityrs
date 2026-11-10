//! The harness's own tests: each value case reaches the memory bound it
//! picks, and the `lz4` check takes the block-output bound for the limit it
//! is, at the bound and on each side of it.

use std::sync::Arc;

use unityrs_core::error::LimitKind;
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::walk::{ReadCtx, ReadDetail, ReadError, boost, python};
use unityrs_testkit::fuzz::{Row, TreeCase, lz4_case};

use crate::Case;

/// The bound a read stopped at, if a bound stopped it.
fn bound(e: &ReadError) -> Option<LimitKind> {
    match &e.detail {
        ReadDetail::Limit(l) | ReadDetail::Charge(ChargeError::Limit(l)) => Some(l.kind),
        _ => None,
    }
}

/// A list of `2^31 - 1` empty classes: a read reaches a bound, after one
/// value per element, whatever the bounds.
fn many_small_values(flags: u8) -> Vec<u8> {
    TreeCase {
        flags,
        start: 0,
        rows: vec![
            Row::new(0, b"Base", b"Base", 0),
            Row::new(1, b"vector", b"x", 0),
            Row::new(2, b"Array", b"Array", 0),
            Row::new(3, b"int", b"size", 0),
            Row::new(3, b"Empty", b"data", 0),
        ],
        refs: Vec::new(),
        data: i32::MAX.to_le_bytes().to_vec(),
    }
    .encode()
}

/// Where a read of `case` on `meter` stops: UnityPyBoost's reader's, or
/// with `python` the pure-Python one's from position 0.
fn stop(case: &Case, meter: &Arc<Meter>, python: bool) -> Option<LimitKind> {
    crate::with_stack(|| {
        let read = ReadCtx {
            endian: case.endian(),
            ref_types: case.ref_types(),
            options: &case.options,
            meter,
        };
        let r = if python {
            python::read(&case.data, 0, &case.tree, &read)
        } else {
            boost::read(&case.data, &case.tree, &read)
        };
        r.err().as_ref().and_then(bound)
    })
}

#[test]
fn a_value_case_reaches_the_memory_bound_it_picks() {
    for python in [false, true] {
        let large = Case::new(&many_small_values(0)).expect("the tree compiles");
        assert_eq!(
            stop(&large, &large.meter, python),
            Some(LimitKind::DecodeWork),
            "python {python}: at the large memory bound a read stops at the work bound"
        );
        let small =
            Case::new(&many_small_values(TreeCase::SMALL_MEMORY)).expect("the tree compiles");
        assert_eq!(
            stop(&small, &small.meter, python),
            Some(LimitKind::InputMemory),
            "python {python}: at the small memory bound a read stops at the memory bound"
        );
        // `write_after_read`'s second read, on a meter of its own.
        assert_eq!(
            stop(&small, &small.new_meter(), python),
            Some(LimitKind::InputMemory),
            "python {python}: a new meter keeps the case's memory bound"
        );
    }
}

/// An LZ4 block whose output is `len` bytes (25 or more): one literal, a
/// match of offset 1 that repeats it to `len - 5` bytes, and five last
/// literals.
fn block_of(len: usize) -> Vec<u8> {
    let mut extra = len - 6 - 19;
    let mut b = vec![0x1F, b'A', 1, 0];
    while extra >= 255 {
        b.push(255);
        extra -= 255;
    }
    b.push(u8::try_from(extra).expect("under 255"));
    b.extend_from_slice(&[0x50, b'B', b'C', b'D', b'E', b'F']);
    b
}

#[test]
fn the_lz4_check_takes_the_block_output_bound_for_the_limit_it_is() {
    let bound = usize::try_from(crate::WORK).expect("a usize");
    // liblz4 accepts each; unityrs below and at the bound, and refuses past
    // it with `BlockOutput`. A memory bound at the block bound refused the
    // block of exactly the bound (its allocation costs a page more), and a
    // block past it failed the check as a difference from liblz4.
    for len in [bound - 1, bound, bound + 1, 8_415_025] {
        for declared in [len, 16 << 20] {
            let declared = u32::try_from(declared).expect("a u32");
            crate::lz4::check(&lz4_case(declared, &block_of(len)));
        }
    }
}
