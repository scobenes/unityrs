use super::super::partition::{PartitionTable, hash52, select};
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::Meter,
};

#[test]
fn known_hash_seeds() {
    // hash52 from astcenc_partition_tables.cpp:114-128, evaluated with
    // unsigned 32-bit wrap. Seed 0 is also immediate by hand.
    for (seed, want) in [
        (0, 0),
        (1, 0x0c46_f563),
        (1023, 0x2557_6bba),
        (1024, 0xbd3d_4343),
        (2048, 0xc83a_5fe6),
        (3072, 0x102c_ce30),
        (4095, 0x85c5_59b5),
    ] {
        assert_eq!(hash52(seed), want);
    }
}

#[test]
fn known_patterns_and_small_block_boundary() {
    // Row-major patterns from the original C++ select_partition, covering
    // the spec's squared nibbles, seed bias and small-block doubling.
    for (side, count, seed, pattern) in [
        (4, 2, 1, "0110111011001100"),
        (4, 2, 17, "1111111100000000"),
        (4, 3, 0, "2000200021112111"),
        (4, 3, 1023, "1012001202110211"),
        (4, 4, 1, "0233023322332233"),
        (4, 4, 17, "1130111133023322"),
        (6, 2, 1, "011110111110111110111100111100111000"),
        (6, 3, 17, "222011222012220012220112200112200122"),
        (6, 4, 1023, "333333333333333333333333222222222222"),
    ] {
        let got: String = (0..side)
            .flat_map(|y| {
                (0..side).map(move |x| {
                    char::from(b'0' + select(seed, count, x, y, side * side < 32).unwrap())
                })
            })
            .collect();
        assert_eq!(got, pattern);
    }
    // Hand-derived degenerate pattern: count=1 always returns 0. When
    // seed=0,count=2 the hash's first score beats the second at every texel.
    for y in 0..12 {
        for x in 0..12 {
            assert_eq!(select(0, 1, x, y, false), Some(0));
            assert_eq!(select(0, 2, x, y, false), Some(0));
        }
    }
    assert_eq!(select(1024, 2, 0, 0, false), None);
    assert_eq!(select(0, 0, 0, 0, false), None);
    assert_eq!(select(0, 5, 0, 0, false), None);
    assert_eq!(select(0, 2, 12, 0, false), None);
}

#[test]
fn compact_cache_matches_procedural_partitions_and_drops_its_charge() {
    let meter = Meter::unlimited();
    for (width, height) in [(4, 4), (8, 4), (12, 12)] {
        let charge = PartitionTable::charge_bytes(width, height).unwrap();
        let table = PartitionTable::new(width, height, &meter).unwrap().unwrap();
        assert_eq!(meter.used(), charge);
        for count in 1..=4 {
            for seed in 0..1024 {
                for y in 0..height {
                    for x in 0..width {
                        assert_eq!(
                            table.get(seed, count, x, y),
                            select(seed, count, x, y, u16::from(width) * u16::from(height) < 32)
                        );
                    }
                }
            }
        }
        assert_eq!(table.get(1024, 2, 0, 0), None);
        assert_eq!(table.get(0, 0, 0, 0), None);
        assert_eq!(table.get(0, 2, width, 0), None);
        drop(table);
        assert_eq!(meter.used(), 0);
    }
    let need = PartitionTable::charge_bytes(12, 12).unwrap();
    assert_eq!(need, 446_464);
    let short = Meter::new(need - 1, Arc::new(Unbounded), Reservation::Input);
    assert!(PartitionTable::new(12, 12, &short).is_err());
    assert_eq!(short.used(), 0);
    assert!(PartitionTable::new(255, 255, &meter).unwrap().is_none());
    assert_eq!(meter.used(), 0);
}
