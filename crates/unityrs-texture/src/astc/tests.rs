//! Component tests use the Khronos Data Format Specification's ASTC chapter
//! (Block Mode, Integer Sequence Encoding, Partition Pattern Generation and
//! Endpoint Unquantization), plus astcenc commit 631fb04's own decode tables.
//! <https://registry.khronos.org/DataFormat/specs/1.3/dataformat.1.3.html>
//! These are component checks, not oracle fixtures or a parity claim.

use super::{block_mode, endpoint, ise, partition, read_bits};
use unityrs_testkit::Rng;

#[path = "block_mode_tests.rs"]
mod block_modes;
#[path = "endpoint_tests.rs"]
mod endpoints;
#[path = "ise_tests.rs"]
mod integer_sequences;
#[path = "memory_tests.rs"]
mod memory;
#[path = "partition_tests.rs"]
mod partitions;

fn put(data: &mut [u8], offset: usize, width: usize, value: u32) {
    for i in 0..width {
        let bit = offset + i;
        data[bit / 8] &= !(1 << (bit % 8));
        data[bit / 8] |= u8::try_from((value >> i) & 1).unwrap() << (bit % 8);
    }
}

#[test]
fn bounded_bit_reads() {
    assert_eq!(read_bits(&[0x96, 0x35], 4, 8), Some(0x59));
    assert_eq!(read_bits(&[0x96], 7, 1), Some(1));
    assert_eq!(read_bits(&[], 0, 0), Some(0));
    assert_eq!(read_bits(&[], usize::MAX, 1), None);
    assert_eq!(read_bits(&[0; 8], 0, 17), None);
    assert_eq!(read_bits(&[0], 7, 2), None);
}

/// Every component sees 10,000 seeded mutations, invalid arguments and
/// truncated bitstreams. A panic fails directly, without containment.
#[test]
fn seeded_mutations_do_not_panic() {
    let mut rng = Rng::new(0xa57c_1d12);
    let mut data = [0; 16];
    for _ in 0..10_000 {
        let flips = rng.below(32) + 1;
        rng.flip_bits(&mut data, flips);
        let mode = u16::from_le_bytes([data[0], data[1]]) & 2047;
        if let Some(block_mode::BlockMode::Weights(w)) = block_mode::decode(mode)
            && let Some(layout) = endpoint::decode_layout(&data, w)
            && let Some(values) = endpoint::decode_values(&data, &layout)
        {
            for (&mode, v) in layout.modes.iter().zip(values) {
                let _ = endpoint::unpack_ldr(mode, &v);
            }
        }
        let _ = block_mode::for_footprint(mode, data[2], data[3]);
        let _ = endpoint::unpack_ldr(data[0], &data[..rng.below(9)]);
        let _ = partition::select(
            u16::from_le_bytes([data[0], data[1]]),
            data[2],
            data[3],
            data[4],
            data[5] & 1 != 0,
        );
        let _ = partition::hash52(u32::from_le_bytes(data[..4].try_into().unwrap()));
        let _ = partition::PartitionTable::charge_bytes(data[0], data[1]);
        let q = ise::Quant::new(u8::try_from(rng.below(21)).unwrap()).unwrap();
        let count = rng.below(67);
        let mut out = [0; 66];
        let _ = ise::decode(q, &data[..rng.below(17)], rng.below(140), &mut out[..count]);
        let _ = q.unquant_color(data[0]);
        let _ = q.unquant_weight(data[1]);
        if q.index() != 0 {
            assert_eq!(q.bit_count(usize::MAX), None);
        }
    }
}
