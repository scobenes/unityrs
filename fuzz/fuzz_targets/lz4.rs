//! Plan section 8's R9: unityrs's LZ4 block decoder (`codec::Lz4`, through
//! `codec::decode_block` as a bundle decodes a block) against liblz4
//! 1.9.4's `LZ4_decompress_safe`, the decoder python-lz4 4.4.4 runs for
//! UnityPy's `lz4.block.decompress(data, uncompressed_size)`. The input is
//! `unityrs_testkit::fuzz::lz4_case`: the declared size, then the block.
//!
//! The two agree on accepting or refusing every block, on the bytes when
//! both accept, and on the error code when both refuse (unityrs's message
//! carries liblz4's code, negated, as python-lz4's does). The one
//! difference allowed is `PARITY.md`'s LZ4 row: python-lz4 refuses a
//! declared size above 2^31 - 1 with `OverflowError` before liblz4 runs,
//! and unityrs refuses it so (an input of 2^31 bytes or more, the row's
//! other refusal, is longer than any fuzz input).
//!
//! liblz4 decodes into a buffer of the declared size, and its verdict
//! depends on that size, not only on the output's length (a literal run
//! that ends within 12 bytes of the buffer's end must be the block's last).
//! What it compares with the end of the buffer is the output so far, or
//! that plus one sequence's length (each term under 256 `n` + 15 for a
//! block of `n` bytes: every 255 a length adds costs a byte of input), at
//! most 64 bytes before the end. So a buffer of 512 `n` + 4096 bytes takes
//! every branch any larger declared size would, and the comparison stays
//! exact without reserving gigabytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use unityrs_core::Bytes;
use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{Codec, Lz4, decode_block};
use unityrs_core::error::{Error, LimitKind};

/// liblz4 1.9.4, as `LZ4_versionNumber` gives it.
const LIBLZ4: i32 = 10904;

/// liblz4's verdict: the output, or the (negative) error code.
fn liblz4(block: &[u8], capacity: i32) -> Result<Vec<u8>, i32> {
    let room = usize::try_from(capacity).unwrap_or(0);
    let mut out: Vec<u8> = Vec::with_capacity(room);
    let n = i32::try_from(block.len()).unwrap_or(i32::MAX);
    // SAFETY: `block` is `n` readable bytes and `out` has room for
    // `capacity` bytes; `LZ4_decompress_safe` reads and writes no more, and
    // returns how many it wrote (or a negative code, having written fewer).
    let r = unsafe {
        lz4_sys::LZ4_decompress_safe(block.as_ptr().cast(), out.as_mut_ptr().cast(), n, capacity)
    };
    let written = usize::try_from(r).map_err(|_| r)?;
    // SAFETY: liblz4 initialised the first `written` bytes, within `room`.
    unsafe { out.set_len(written.min(room)) };
    Ok(out)
}

fuzz_target!(|data: &[u8]| {
    let Some((head, block)) = data.split_first_chunk::<4>() else {
        return;
    };
    // SAFETY: no arguments, no state.
    assert_eq!(
        unsafe { lz4_sys::LZ4_versionNumber() },
        LIBLZ4,
        "lz4-sys builds another liblz4"
    );
    let declared = u32::from_le_bytes(*head);
    // One decode allocates its output once, at the length the scan found,
    // so the memory bound can be the work bound: no output of a fuzz input
    // (at most 255 times its length) reaches it, and a limit error would be
    // a difference from liblz4.
    let mut ctx = unityrs_core::FileContext::unitypy();
    ctx.limits = unityrs_fuzz::limits();
    ctx.limits.max_input_memory = unityrs_fuzz::WORK;
    ctx.limits.max_block_output = unityrs_fuzz::WORK;
    let limits = ctx.limits;
    let meter = unityrs_fuzz::meter(&ctx);
    let Ok(mut out) = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory) else {
        return;
    };
    let input = Bytes::copy_from_slice(block);
    let ours = decode_block(
        Codec::Lz4,
        &Lz4,
        &input,
        declared,
        &mut out,
        limits.max_block_output,
    );
    let detail = |r: &Result<Option<Bytes>, Error>| match r {
        Err(Error::Decompress { detail, .. }) => detail.to_string(),
        other => panic!("unityrs gave {other:?}, not a decode error"),
    };
    let Ok(capacity) = i32::try_from(declared) else {
        // PARITY.md's LZ4 row: python-lz4's OverflowError, liblz4 never runs.
        let d = detail(&ours);
        assert!(d.starts_with("OverflowError"), "declared {declared}: {d}");
        return;
    };
    let reach =
        i32::try_from(block.len().saturating_mul(512).saturating_add(4096)).unwrap_or(i32::MAX);
    let theirs = liblz4(block, capacity.min(reach));
    if capacity > reach {
        // The module docs' argument, checked where it is used: a larger
        // buffer gives liblz4's verdict at `reach`.
        let larger = liblz4(block, capacity.min(reach.saturating_mul(2)));
        assert!(
            larger == theirs,
            "declared {declared}: liblz4 depends on room past {reach}"
        );
    }
    match (theirs, &ours) {
        (Ok(theirs), Ok(None)) => assert!(
            out.as_slice() == theirs.as_slice(),
            "declared {declared}: both accept, the bytes differ ({} and {} bytes)",
            out.len(),
            theirs.len()
        ),
        (Err(code), Err(_)) => {
            let d = detail(&ours);
            assert!(
                d.ends_with(&format!("Error code: {}", -i64::from(code))),
                "declared {declared}: liblz4 refuses with {code}, unityrs with {d}"
            );
        }
        (theirs, ours) => panic!(
            "declared {declared}: liblz4 {:?}, unityrs {:?}",
            theirs.map(|v| v.len()),
            ours.as_ref().map(|_| out.len())
        ),
    }
});
