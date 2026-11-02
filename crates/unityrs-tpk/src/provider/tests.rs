//! The provider's unit tests: its meter's charges, a kept error, and the
//! racing first requests.

use std::mem::size_of;

use unityrs_core::RawStr;
use unityrs_core::budget::OWNER_COST;
use unityrs_core::limits::{Limits, heap_cost};

use super::TpkProvider;

/// A stored TPK whose blob has no versions, classes or nodes, one
/// string, and `n` common-string indices, each naming it.
fn tpk(n: usize) -> Vec<u8> {
    let mut blob = 1_i64.to_le_bytes().to_vec();
    for _ in 0..3 {
        // Versions, classes, common-string versions.
        blob.extend_from_slice(&0_i32.to_le_bytes());
    }
    blob.extend_from_slice(&i32::try_from(n).unwrap().to_le_bytes());
    for _ in 0..n {
        blob.extend_from_slice(&0_u16.to_le_bytes());
    }
    blob.extend_from_slice(&0_i32.to_le_bytes());
    blob.extend_from_slice(&1_i32.to_le_bytes());
    blob.extend_from_slice(&[1, b's']);
    let size = u32::try_from(blob.len()).unwrap().to_le_bytes();
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(&blob);
    out
}

/// The common-string table has one entry per index the TPK lists, up to
/// [`TpkProvider::MAX_RECORDS`], and is charged to the provider's meter
/// with the rest of what the provider holds: every index costs its own
/// 2 bytes in the blob's table and its entry in the common strings'.
#[test]
fn the_common_string_table_is_charged_to_the_providers_meter() {
    let used = |n: usize| {
        let provider = TpkProvider::from_bytes(&tpk(n), &Limits::default()).unwrap();
        assert_eq!(provider.common.len(), n);
        provider.meter.used()
    };
    let n = TpkProvider::MAX_RECORDS;
    let bytes = |b: usize| heap_cost(u64::try_from(b).unwrap());
    let table = bytes(n * size_of::<(u32, RawStr)>());
    assert_eq!(used(n) - used(0), bytes(2 * n) + table);
}

/// The stored TPK `stored` with its payload LZ4-compressed as one run of
/// literals (a block liblz4 decodes at any length), and the block's size.
fn lz4_of(stored: &[u8]) -> (Vec<u8>, usize) {
    let blob = &stored[20..];
    let n = blob.len();
    let mut block = vec![u8::try_from(n.min(15)).unwrap() << 4];
    if n >= 15 {
        let mut rest = n - 15;
        while rest >= 255 {
            block.push(255);
            rest -= 255;
        }
        block.push(u8::try_from(rest).unwrap());
    }
    block.extend_from_slice(blob);
    let mut out = stored[..20].to_vec();
    out[5] = 1;
    out[12..16].copy_from_slice(&u32::try_from(block.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&block);
    (out, block.len())
}

/// What a load reads into memory is charged to the provider's meter
/// before it is allocated (round 4: neither was): the bytes of the file
/// a path names, exactly its length, returned once the TPK is loaded;
/// and the copy of a compressed payload the codec reads, with its
/// `Bytes` owner record.
#[test]
fn the_file_read_and_the_payload_copy_are_charged_to_the_providers_meter() {
    let bytes = |n: usize| heap_cost(u64::try_from(n).unwrap());
    let stored = tpk(1000);
    let from_bytes = TpkProvider::from_bytes(&stored, &Limits::default()).unwrap();
    let dir = unityrs_testkit::scratch::Scratch::new("tpk-charged-read");
    let path = dir.join("charged.tpk");
    std::fs::write(&path, &stored).unwrap();
    let from_path = TpkProvider::open(&path).unwrap();
    drop(dir);
    assert_eq!(
        from_path.meter.granted() - from_bytes.meter.granted(),
        bytes(stored.len()),
        "the file's buffer"
    );
    assert_eq!(
        from_path.meter.used(),
        from_bytes.meter.used(),
        "the buffer is returned once the TPK is loaded"
    );
    let (lz4, block) = lz4_of(&stored);
    let from_lz4 = TpkProvider::from_bytes(&lz4, &Limits::default()).unwrap();
    assert_eq!(from_lz4.common.len(), 1000);
    assert_eq!(
        from_lz4.meter.granted() - from_bytes.meter.granted(),
        bytes(block) + OWNER_COST,
        "the compressed payload's copy"
    );
    assert_eq!(from_lz4.meter.used(), from_bytes.meter.used());
}

/// A stored TPK of one class, 0, at version 2.0.0, whose release root,
/// node 0, lists node 1 twice, and so on down `levels` nodes: a tree of
/// `2^levels - 1` rows.
fn doubling_tpk(levels: u16) -> Vec<u8> {
    one_class_tpk(levels, 0)
}

/// [`doubling_tpk`]'s TPK with its class's release root at node
/// `release_root` (its editor root stays node 0).
fn one_class_tpk(levels: u16, release_root: u16) -> Vec<u8> {
    let v = (2_u64 << 48).to_le_bytes();
    let mut blob = 1_i64.to_le_bytes().to_vec();
    blob.extend_from_slice(&1_i32.to_le_bytes());
    blob.extend_from_slice(&v);
    // One class, one entry: an editor root and a release root (flags
    // 192), in that order.
    blob.extend_from_slice(&1_i32.to_le_bytes());
    blob.extend_from_slice(&0_i32.to_le_bytes());
    blob.extend_from_slice(&1_i32.to_le_bytes());
    blob.extend_from_slice(&v);
    blob.extend_from_slice(&[1, b'C', 0, 0, 0, 192, 0, 0]);
    blob.extend_from_slice(&release_root.to_le_bytes());
    // No common strings or indices.
    blob.extend_from_slice(&[0; 8]);
    blob.extend_from_slice(&i32::from(levels).to_le_bytes());
    for i in 0..levels {
        // `<HHihbIH`: strings 0 and 1, size -1, version 1, flags 0.
        blob.extend_from_slice(&[0, 0, 1, 0, 0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0, 0, 0]);
        let subs: &[u16] = if i + 1 < levels { &[i + 1, i + 1] } else { &[] };
        blob.extend_from_slice(&u16::try_from(subs.len()).unwrap().to_le_bytes());
        for s in subs {
            blob.extend_from_slice(&s.to_le_bytes());
        }
    }
    blob.extend_from_slice(&2_i32.to_le_bytes());
    blob.extend_from_slice(&[2, b's', b'0', 2, b's', b'1']);
    let size = u32::try_from(blob.len()).unwrap().to_le_bytes();
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(&blob);
    out
}

/// A walk's error is kept for its root, and its heap, the shared text
/// with its two reference counts, is charged to the provider's meter
/// while it is kept (crate docs, "Memory"): one charge for the first
/// request, none for a later one, which gets the same text without
/// walking again; and dropping the provider returns it with the rest of
/// what the provider held (review round 10: dropping the charge passed
/// every test).
#[test]
fn a_kept_walk_error_is_charged_to_the_providers_meter_while_kept() {
    use std::sync::Arc;

    use unityrs_core::UnityVersion;

    let v = UnityVersion::from_packed(2 << 48);
    // Release root 5 of a one-node table: `NODES[5]` raises IndexError.
    let provider = TpkProvider::from_bytes(&one_class_tpk(1, 5), &Limits::default()).unwrap();
    let meter = provider.meter.clone();
    let before = meter.used();
    let e = provider.node_list(0, &v).unwrap_err();
    assert!(e.detail.starts_with("IndexError"), "{e:?}");
    assert!(e.memory.is_none() && e.limit.is_none(), "{e:?}");
    let held = heap_cost(u64::try_from(e.detail.len() + 2 * size_of::<usize>()).unwrap());
    assert_eq!(meter.used() - before, held, "the kept error's heap");
    let again = provider.node_list(0, &v).unwrap_err();
    assert!(Arc::ptr_eq(&e.detail, &again.detail), "the kept error");
    assert_eq!(meter.used() - before, held, "charged once");
    drop((e, again, provider));
    assert_eq!(meter.used(), 0, "the provider returned what it held");
}

// The source scan behind "the provider starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code: this file's `#[cfg(test)] mod tests;` is in
// another.
/// The price of never waiting, as the crate docs state it ("Threads",
/// round 4): threads that ask for one release root before its first
/// answer is published each walk it, at most once each, and all of them
/// get the one list published. Eight threads released together are
/// granted at most eight walks' worth of rows, however they interleave.
#[cfg(test)]
#[test]
fn racing_first_requests_walk_a_root_at_most_once_per_thread() {
    use std::sync::{Arc, Barrier};

    use unityrs_core::UnityVersion;

    const THREADS: u64 = 8;
    let tpk = doubling_tpk(16);
    let v = UnityVersion::from_packed(2 << 48);
    let alone = TpkProvider::from_bytes(&tpk, &Limits::default()).unwrap();
    let before = alone.meter.granted();
    let list = alone.node_list(0, &v).unwrap().unwrap();
    assert_eq!(list.rows().len(), (1 << 16) - 1);
    let one = alone.meter.granted() - before;
    assert!(one > 0);
    let provider = Arc::new(TpkProvider::from_bytes(&tpk, &Limits::default()).unwrap());
    let before = provider.meter.granted();
    let start = Arc::new(Barrier::new(usize::try_from(THREADS).unwrap()));
    let lists: Vec<_> = (0..THREADS)
        .map(|_| {
            let (provider, start, v) = (provider.clone(), start.clone(), v.clone());
            std::thread::spawn(move || {
                start.wait();
                provider.node_list(0, &v).unwrap().unwrap()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    let raced = provider.meter.granted() - before;
    assert!(lists.iter().all(|l| Arc::ptr_eq(l, &lists[0])), "one list");
    assert!(
        (one..=THREADS * one).contains(&raced),
        "{raced} bytes granted, {} walks' worth",
        raced / one
    );
}
