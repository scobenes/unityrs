//! What a block read's charges hold at their peak (plan section 3.15; perf
//! stage 2's fixes A and B), on testkit bundles whose blocks are known by
//! construction. Each input's meter books with a tracker that keeps the
//! peak of its live charges, taken from just before the read, so the
//! bundle's own tables are not in it.
//!
//! Each expected peak follows from the block's layout and `heap_cost`: a
//! growth holds the old and the new block at once (3.15), and the ladder of
//! sizes is the codec's. LZMA (`Lzma::decode`) asks for room in windows of
//! at most 1 MiB, the first its declared size, then a one-byte probe after
//! each full window; the room doubles, never past the block's declared end
//! while the block is short of it, and the attempt holds its dictionary
//! (`Lzma::decoder_charge` of the testkit's 512 KiB, `LZMA_DICT`) for the
//! whole decode. LZ4 asks for each block's measured length at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{
    BudgetExceeded, MemoryBudget, OWNER_COST, Permit, PermitHome, Reservation,
};
use unityrs_core::codec::Lzma;
use unityrs_core::files::BundleFile;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, EntrySpec, LZMA_DICT, TestCodec, build};

const MIB: u64 = 1 << 20;

/// Live charges and their peak.
#[derive(Debug, Default)]
struct Tracker {
    used: AtomicU64,
    peak: AtomicU64,
}

impl PermitHome for Tracker {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct Booked(Arc<Tracker>);

impl MemoryBudget for Booked {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        let now = self.0.used.fetch_add(bytes, Ordering::AcqRel) + bytes;
        self.0.peak.fetch_max(now, Ordering::AcqRel);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

impl Tracker {
    /// The peak from now on, above what is live now.
    fn reset_peak(&self) -> u64 {
        let now = self.used.load(Ordering::Acquire);
        self.peak.store(now, Ordering::Release);
        now
    }

    fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }
}

fn data(n: u64, seed: u64) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from((i * 7 + i / 4093 + seed) % 251).unwrap())
        .collect()
}

/// One entry of `n` bytes, cut into blocks of `block` bytes.
fn bundle(n: u64, block: u64, codec: TestCodec, overrides: Vec<BlockOverride>) -> Vec<u8> {
    build(&BundleSpec {
        data_codec: codec,
        block_size: usize::try_from(block).unwrap(),
        entries: vec![EntrySpec {
            name: b"charges/entry".to_vec(),
            data: data(n, 0),
            flags: 0,
        }],
        block_overrides: overrides,
        ..BundleSpec::default()
    })
}

/// Opens `bundle` on a tracked meter and reads its one entry: the peak of
/// the read's live charges, and what stays charged once it is done, both
/// above what the open left charged.
fn read_peak(bundle: &[u8]) -> (u64, u64) {
    let tracker = Arc::new(Tracker::default());
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(
        &ctx.limits,
        Arc::new(Booked(tracker.clone())),
        Reservation::Input,
    );
    let b = BundleFile::open(
        RawStr::from("charges.bundle"),
        Bytes::copy_from_slice(bundle),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    let base = tracker.reset_peak();
    let bytes = b.entry_bytes(0).unwrap();
    let held = tracker.used.load(Ordering::Acquire) - base;
    assert_eq!(
        bytes,
        Bytes::from(data(u64::try_from(bytes.len()).unwrap(), 0))
    );
    (tracker.peak() - base, held)
}

fn decoder() -> u64 {
    Lzma::decoder_charge(LZMA_DICT)
}

/// Fix A on the corpus's largest input's shape: one LZMA block, decoded in
/// 1 MiB windows. D = 3 MiB + 5. The room is 1 MiB (the first window),
/// 2 MiB (the probe after it), then the block's declared end, D (the next
/// window), and the last window asks for the 3 bytes left. Peak: the
/// dictionary, 2 MiB and D at once. Before the fix the room doubled to
/// 4 MiB, then, as the last 1 MiB window passed it, to 8 MiB; and the run's
/// exact block was charged beside it (fix B): 4 + 8 MiB with the
/// dictionary, then 8 MiB and D at once.
#[test]
fn an_lzma_blocks_room_ends_at_its_declared_size() {
    let d = 3 * MIB + 5;
    let (peak, held) = read_peak(&bundle(d, d, TestCodec::Lzma, Vec::new()));
    assert_eq!(peak, decoder() + heap_cost(2 * MIB) + heap_cost(d));
    // The run: its exact block, as it was decoded, and the owner record
    // (the run index's own table is small).
    assert!(held >= heap_cost(d) + OWNER_COST, "{held}");
    assert!(held < heap_cost(d) + OWNER_COST + 4096, "{held}");
}

/// Fix A's other half and fix B: an LZMA block that writes past its
/// declared size (1 MiB + 3 of its 3 MiB + 5) grows as before from there,
/// doubling to the segment's end. Room: 1 MiB, then its declared end
/// (1 MiB + 3: the probe), then twice that (2 MiB + 6: the probe past the
/// declared end), then 4 MiB + 12 (a 1 MiB window). Peak: the dictionary,
/// 2 MiB + 6 and 4 MiB + 12 at once. The run then gives its spare room
/// back in place (both blocks over 128 KiB), charging nothing beside it
/// (fix B): before, 4 MiB + 12 and the exact 3 MiB + 5 were charged at
/// once, which is the larger. Undoing fix A's growth past the declared end
/// (room stopped there for good) grows the room one window at a time.
#[test]
fn an_lzma_block_past_its_declared_size_grows_as_before_and_shrinks_in_place() {
    let d = 3 * MIB + 5;
    let declared = MIB + 3;
    let over = BlockOverride {
        block: 0,
        uncompressed: Some(u32::try_from(declared).unwrap()),
        ..BlockOverride::default()
    };
    let (peak, held) = read_peak(&bundle(d, d, TestCodec::Lzma, vec![over]));
    let (two, four) = (2 * declared, 4 * declared);
    assert_eq!(peak, decoder() + heap_cost(two) + heap_cost(four));
    assert!(held >= heap_cost(d) + OWNER_COST, "{held}");
    assert!(held < heap_cost(d) + OWNER_COST + 4096, "{held}");
}

/// Fix A on a run of LZ4 blocks: three blocks of 100 KiB, decoded into one
/// run. The room is each block's end: 100 KiB, 200 KiB, 300 KiB. Peak: 200
/// and 300 KiB at once. Before, the third block doubled the room to 400
/// KiB (200 and 400 KiB at once), and the run's exact 300 KiB was charged
/// beside the 400 when it gave the spare room back (fix B).
#[test]
fn a_run_of_lz4_blocks_grows_by_each_blocks_size() {
    let b = 100 << 10;
    let (peak, held) = read_peak(&bundle(3 * b, b, TestCodec::Lz4, Vec::new()));
    assert_eq!(peak, heap_cost(2 * b) + heap_cost(3 * b));
    assert!(held >= heap_cost(3 * b) + OWNER_COST, "{held}");
    assert!(held < heap_cost(3 * b) + OWNER_COST + 4096, "{held}");
}
