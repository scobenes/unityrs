//! `BundleFile::counts` (plan section 8, R2 and R3): on a testkit bundle
//! whose blocks are known by construction, each read's decodes, discarded
//! decodes, decodes again and cached runs are the counts the layout gives.
//! The builder lays the entries out back to back in directory order and
//! cuts the joined data into blocks of `block_size` bytes, so with entries
//! of `block_size` bytes each entry is one block.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded};
use unityrs_core::files::BundleCounts;
use unityrs_core::{Bytes, Error};
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::guard;

use super::{ctx_with, meter_for, open, open_with};

/// The block size, and the length of each one-block entry.
const S: u64 = 64;

fn entries(sizes: &[u64]) -> Vec<EntrySpec> {
    sizes
        .iter()
        .enumerate()
        .map(|(i, n)| EntrySpec {
            name: format!("counts/entry-{i:02}").into_bytes(),
            data: (0..*n)
                .map(|j| u8::try_from((j * 7 + 3) % 251).unwrap())
                .collect(),
            flags: 0,
        })
        .collect()
}

fn bundle(sizes: &[u64], codec: TestCodec) -> BundleSpec {
    BundleSpec {
        data_codec: codec,
        block_size: usize::try_from(S).unwrap(),
        entries: entries(sizes),
        ..BundleSpec::default()
    }
}

fn counts(decoded: u64, discarded: u64, redecoded: u64, runs: (u64, u64, u64)) -> BundleCounts {
    let mut c = BundleCounts::default();
    c.decoded = decoded;
    c.discarded = discarded;
    c.redecoded = redecoded;
    (c.runs, c.runs_held, c.runs_peak) = runs;
    c
}

/// Five LZ4 blocks, an entry in each, read lazily from the last entry.
/// Reading entry 4 learns blocks 0-3 first, each decoded with its output
/// dropped (4 x S discarded), then decodes block 4 into the run it reads
/// from. Entry 0's block is known, so its read decodes it again into a run
/// of its own (S again). `load_all` then decodes all five again into one
/// run (5 x S), which replaces the two cached runs.
#[test]
fn a_late_entry_read_first_discards_every_block_before_it() {
    let b = open(&build(&bundle(&[S; 5], TestCodec::Lz4)));
    assert_eq!(b.blocks().len(), 5);
    assert_eq!(
        b.counts(),
        BundleCounts::default(),
        "nothing decoded at open"
    );
    b.entry_bytes(4).unwrap();
    assert_eq!(b.counts(), counts(5 * S, 4 * S, 0, (1, S, S)));
    b.entry_bytes(0).unwrap();
    assert_eq!(b.counts(), counts(6 * S, 4 * S, S, (2, 2 * S, 2 * S)));
    b.entry_bytes(4).unwrap();
    b.entry_bytes(0).unwrap();
    assert_eq!(
        b.counts(),
        counts(6 * S, 4 * S, S, (2, 2 * S, 2 * S)),
        "cached"
    );
    b.load_all().unwrap();
    assert_eq!(b.counts(), counts(11 * S, 4 * S, 6 * S, (1, 5 * S, 5 * S)));
}

/// Two entries of 1.5 blocks over three LZ4 blocks: block 1 is in both
/// spans. Entry 0 decodes blocks 0-1 into a run; entry 1's first block is
/// block 1, known and in no run that reaches block 2, so its read decodes
/// block 1 again, then block 2. `load_all` decodes all three again into a
/// run of 3 x S that drops both, so the runs held 4 x S at most and 3 x S
/// after.
#[test]
fn a_straddled_block_is_decoded_again_for_each_span() {
    let b = open(&build(&bundle(&[3 * S / 2, 3 * S / 2], TestCodec::Lz4)));
    assert_eq!(b.blocks().len(), 3);
    b.entry_bytes(0).unwrap();
    assert_eq!(b.counts(), counts(2 * S, 0, 0, (1, 2 * S, 2 * S)));
    b.entry_bytes(1).unwrap();
    assert_eq!(b.counts(), counts(4 * S, 0, S, (2, 4 * S, 4 * S)));
    b.entry_bytes(0).unwrap();
    assert_eq!(b.counts(), counts(4 * S, 0, S, (2, 4 * S, 4 * S)), "cached");
    b.load_all().unwrap();
    assert_eq!(b.counts(), counts(7 * S, 0, 4 * S, (1, 3 * S, 4 * S)));
}

/// Eagerly, every block is decoded once into one run.
#[test]
fn load_all_decodes_each_block_once_into_one_run() {
    let b = open(&build(&bundle(&[S, 3 * S, S / 2], TestCodec::Lz4)));
    let total = S * 9 / 2;
    b.load_all().unwrap();
    for i in 0..3 {
        b.entry_bytes(i).unwrap();
    }
    assert_eq!(b.counts(), counts(total, 0, 0, (1, total, total)));
}

/// Stored blocks are their own bytes: their lengths are known at open, a
/// span of them is a slice of the input (a run, counted by its length, with
/// nothing decoded), and one copied into a run with a compressed block is
/// a first copy, never a decode again. Blocks 0-1 are stored, block 2 LZ4;
/// entry 1 spans blocks 1 and 2.
#[test]
fn a_stored_block_known_at_open_is_copied_not_decoded_again() {
    let mut spec = bundle(&[S, 2 * S], TestCodec::Lz4);
    spec.block_overrides = [0, 1]
        .map(|block| BlockOverride {
            block,
            codec: Some(TestCodec::None),
            uncompressed: None,
            compressed: None,
        })
        .to_vec();
    let b = open(&build(&spec));
    assert_eq!(b.blocks().len(), 3);
    b.entry_bytes(1).unwrap();
    assert_eq!(b.counts(), counts(2 * S, 0, 0, (1, 2 * S, 2 * S)));
    b.entry_bytes(0).unwrap();
    assert_eq!(
        b.counts(),
        counts(2 * S, 0, 0, (2, 3 * S, 3 * S)),
        "a slice"
    );
}

/// A directory row of size -1 reads to the end of the joined data, so its
/// read measures every block for the total, each decoded with its output
/// dropped (5 x S), and then decodes all five again into its run.
#[test]
fn measuring_the_total_discards_every_block() {
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        ..bundle(&[S; 5], TestCodec::Lz4)
    });
    // A row is offset (i64), size (i64), flags (u32), then the name.
    let name = b"counts/entry-00\0";
    let at = data.windows(name.len()).position(|w| w == name).unwrap();
    assert_eq!(data.windows(name.len()).filter(|w| *w == name).count(), 1);
    data[at - 12..at - 4].copy_from_slice(&(-1i64).to_be_bytes());
    let b = open(&data);
    assert_eq!(b.directory()[0].size, -1);
    assert_eq!(b.entry_bytes(0).unwrap().len(), 5 * 64);
    assert_eq!(b.counts(), counts(10 * S, 5 * S, 5 * S, (1, 5 * S, 5 * S)));
}

/// LZ4, through [`Codecs::unitypy`]'s codec, with the first two decodes
/// held until both have begun (`arrived`, then `go`).
struct Rendezvous {
    lz4: Arc<dyn BlockCodec>,
    calls: AtomicUsize,
    arrived: Mutex<Sender<()>>,
    go: Mutex<Receiver<()>>,
}

impl BlockCodec for Rendezvous {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < 2 {
            self.arrived.lock().unwrap().send(()).unwrap();
            guard::wait(&self.go.lock().unwrap(), "the other thread's decode");
        }
        self.lz4.decode(input, uncompressed, out)
    }
}

/// Two threads read a one-block entry at once, each decoding the block
/// before either learns its length: both count a first decode, and the run
/// the second publishes is dropped for the first's (discarded).
#[test]
fn a_run_a_concurrent_publish_beat_is_discarded() {
    let (arrived, arrivals) = channel();
    let (go, gate) = channel();
    let codec = Arc::new(Rendezvous {
        lz4: Codecs::unitypy().get(Codec::Lz4).unwrap().clone(),
        calls: AtomicUsize::new(0),
        arrived: Mutex::new(arrived),
        go: Mutex::new(gate),
    });
    let c = ctx_with(|c| {
        let mut codecs = Codecs::unitypy();
        codecs.set(Codec::Lz4, codec).unwrap();
        c.codecs = Arc::new(codecs);
    });
    // Blocks-info stored, so the open decodes nothing through the codec.
    let spec = BundleSpec {
        info_codec: TestCodec::None,
        ..bundle(&[S], TestCodec::Lz4)
    };
    let b = open_with(&build(&spec), &c, &meter_for(&c)).unwrap();
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let b = b.clone();
            std::thread::spawn(move || b.entry_bytes(0).map(|e| e.len()))
        })
        .collect();
    guard::wait(&arrivals, "the first decode");
    guard::wait(&arrivals, "the second decode");
    go.send(()).unwrap();
    go.send(()).unwrap();
    for r in readers {
        assert_eq!(r.join().unwrap().unwrap(), 64);
    }
    assert_eq!(b.counts(), counts(2 * S, S, 0, (1, S, S)));
}
