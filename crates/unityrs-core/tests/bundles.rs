//! `BundleFile`'s lazy reads against its eager load (plan sections 3.6 and
//! 7.4): one layout in every read order, runs and kept block outcomes,
//! refusals that are not kept, decode work, no lock across caller code, the
//! entries' weak parent handle, entry slicing as Python slices, and the
//! input's meter over several files and hostile directories.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use unityrs_core::budget::{Counting, DecodeBuf, MemoryBudget, Reservation, Unbounded};
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded, Lz4};
use unityrs_core::files::{BundleFile, File, ParentKind};
use unityrs_core::limits::Meter;
use unityrs_core::stream::py_read;
use unityrs_core::{Bytes, Error, FileContext, LimitKind, RawStr, Unsupported};
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build, matrix};
use unityrs_testkit::serialized::build as file;
use unityrs_testkit::{Rng, blob::NodeSpec, serialized::one_object};

fn ctx() -> Arc<FileContext> {
    Arc::new(FileContext::unitypy())
}

/// The UnityPy context, changed by `f`.
fn ctx_with(f: impl FnOnce(&mut FileContext)) -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    f(&mut c);
    Arc::new(c)
}

fn meter_for(ctx: &FileContext) -> Arc<Meter> {
    Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input)
}

fn open_with(
    data: &[u8],
    ctx: &Arc<FileContext>,
    meter: &Arc<Meter>,
) -> Result<Arc<BundleFile>, Error> {
    BundleFile::open(
        RawStr::from("test.bundle"),
        Bytes::copy_from_slice(data),
        None,
        meter,
        ctx,
    )
}

fn open(data: &[u8]) -> Arc<BundleFile> {
    let c = ctx();
    open_with(data, &c, &meter_for(&c)).unwrap()
}

fn by_name(name: &str) -> Vec<u8> {
    matrix()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name}"))
        .1
}

fn all_entries(b: &BundleFile, order: &[usize]) -> Vec<Result<Bytes, String>> {
    let mut out = vec![Err(String::new()); b.directory().len()];
    for &i in order {
        out[i] = b.entry_bytes(i).map_err(|e| e.to_string());
    }
    out
}

fn small_entries() -> Vec<EntrySpec> {
    vec![
        EntrySpec {
            name: b"a".to_vec(),
            data: (0..100u8).collect(),
            flags: 0,
        },
        EntrySpec {
            name: b"b".to_vec(),
            data: (0..300u32).map(|i| (i % 7) as u8).collect(),
            flags: 0,
        },
    ]
}

/// Bundles whose blocks before an entry decode to other lengths than they
/// declare: every entry read lazily, in every order, equals `load_all`'s.
#[test]
fn one_layout_in_every_read_order() {
    let mut inputs: Vec<Vec<u8>> = [
        "lz4-declared-larger.bundle",
        "lzma-overlong.bundle",
        "stored-declared-differs.bundle",
        "stored-compressed-larger.bundle",
        "truncated.bundle",
        "tiny-blocks.bundle",
        "nested.bundle",
    ]
    .iter()
    .map(|n| by_name(n))
    .collect();
    for codec in [TestCodec::Lz4, TestCodec::Lzma, TestCodec::None] {
        inputs.push(build(&BundleSpec {
            data_codec: codec,
            block_size: 64,
            entries: small_entries(),
            block_overrides: vec![BlockOverride {
                block: 0,
                uncompressed: Some(1000),
                ..BlockOverride::default()
            }],
            ..BundleSpec::default()
        }));
    }
    let mut rng = Rng::new(7);
    for data in &inputs {
        let eager = open(data);
        let n = eager.directory().len();
        eager.load_all().unwrap();
        let want = all_entries(&eager, &(0..n).collect::<Vec<_>>());
        let mut orders: Vec<Vec<usize>> = vec![(0..n).collect(), (0..n).rev().collect()];
        for _ in 0..4 {
            let mut o: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                o.swap(i, rng.below(i + 1));
            }
            orders.push(o);
        }
        for order in orders {
            let lazy = open(data);
            assert_eq!(all_entries(&lazy, &order), want, "order {order:?}");
            // Entries parse the same way too.
            for i in 0..n {
                assert!(lazy.entry(i).is_ok());
            }
        }
    }
}

/// LZ4, counting the decodes of each block (by where its bytes start in
/// the input).
#[derive(Default)]
struct CountingLz4 {
    calls: Mutex<std::collections::HashMap<usize, u32>>,
    base: Mutex<usize>,
}

impl CountingLz4 {
    fn calls(&self, at: usize) -> u32 {
        let base = *self.base.lock().unwrap();
        self.calls
            .lock()
            .unwrap()
            .get(&(base + at))
            .copied()
            .unwrap_or(0)
    }
}

impl BlockCodec for CountingLz4 {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        *self
            .calls
            .lock()
            .unwrap()
            .entry(input.as_ptr() as usize)
            .or_default() += 1;
        Lz4.decode(input, uncompressed, out)
    }
}

#[test]
fn ranges_in_one_block_decode_it_once_and_a_straddling_read_decodes_again() {
    let spec = BundleSpec {
        data_codec: TestCodec::Lz4,
        info_codec: TestCodec::None,
        block_size: 64,
        entries: small_entries(),
        ..BundleSpec::default()
    };
    let data = build(&spec);
    let codec = Arc::new(CountingLz4::default());
    let mut codecs = Codecs::unitypy();
    codecs.set(Codec::Lz4, codec.clone()).unwrap();
    let ctx = ctx_with(|c| c.codecs = Arc::new(codecs));
    let joined: Vec<u8> = spec.entries.iter().flat_map(|e| e.data.clone()).collect();
    let b = open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
    *codec.base.lock().unwrap() = b.source().as_ptr() as usize;
    // Each block's stored bytes, in order, follow the stored blocks-info.
    let starts: Vec<usize> = {
        let total: usize = b.blocks().iter().map(|x| x.bytes_read as usize).sum();
        let mut at = data.len() - total;
        b.blocks()
            .iter()
            .map(|x| {
                let s = at;
                at += x.bytes_read as usize;
                s
            })
            .collect()
    };
    let calls = |k: usize| codec.calls(starts[k]);
    // Two ranges of entry 1 in block 2 (joined 128..192): one decode of it.
    let r1 = b.entry_range(1, 30..40).unwrap();
    let r2 = b.entry_range(1, 50..60).unwrap();
    assert_eq!(&r1[..], &joined[130..140]);
    assert_eq!(&r2[..], &joined[150..160]);
    assert_eq!(
        calls(2),
        1,
        "a second range in the block is a slice of its run"
    );
    // Entry 0 is blocks 0 and 1; entry 1 (joined 100..400) starts in block 1,
    // which its read decodes again into its own run.
    let b = open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
    *codec.base.lock().unwrap() = b.source().as_ptr() as usize;
    codec.calls.lock().unwrap().clear();
    assert_eq!(&b.entry_bytes(0).unwrap()[..], &joined[..100]);
    assert_eq!((calls(0), calls(1)), (1, 1));
    assert_eq!(&b.entry_bytes(1).unwrap()[..], &joined[100..400]);
    assert_eq!((calls(0), calls(1), calls(2)), (1, 2, 1));
    // `load_all` decodes every block once more into one run; every later
    // read is a slice of it.
    b.load_all().unwrap();
    let after: Vec<u32> = (0..starts.len()).map(calls).collect();
    b.entry_bytes(0).unwrap();
    b.entry_bytes(1).unwrap();
    b.entry_range(1, 5..6).unwrap();
    assert_eq!((0..starts.len()).map(calls).collect::<Vec<_>>(), after);
}

#[test]
fn a_failing_block_is_decoded_once_and_a_late_one_fails_only_load_all() {
    let data = by_name("lz4-declared-smaller.bundle");
    let b = open(&data);
    let m = b.meter().clone();
    assert!(b.entry_bytes(0).is_err());
    let after = m.decoded();
    let again = b.entry_bytes(0);
    assert!(
        matches!(
            again,
            Err(Error::Decompress {
                codec: Codec::Lz4,
                ..
            })
        ),
        "{again:?}"
    );
    assert_eq!(m.decoded(), after, "the kept failure answers");
    assert!(b.load_all().is_err());

    // A late LZHAM block: the first entry reads lazily, `load_all` fails.
    let late = build(&BundleSpec {
        block_size: 160,
        entries: small_entries(),
        block_overrides: vec![BlockOverride {
            block: 2,
            codec: Some(TestCodec::Raw(4)),
            ..BlockOverride::default()
        }],
        ..BundleSpec::default()
    });
    let b = open(&late);
    assert_eq!(&b.entry_bytes(0).unwrap()[..], &small_entries()[0].data[..]);
    assert!(matches!(
        b.load_all(),
        Err(Error::Unsupported(Unsupported::Lzham))
    ));
    assert!(b.entry_bytes(1).is_err());
}

#[test]
fn a_bundle_of_corrupt_blocks_opens_and_decodes_nothing_until_read() {
    let spec = BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 32,
        entries: small_entries(),
        ..BundleSpec::default()
    };
    let mut data = build(&spec);
    // Every data block follows blocks-info: overwrite them all.
    let blocks_from = data.len() - 120;
    for b in &mut data[blocks_from..] {
        *b = 0xF0;
    }
    let b = open(&data);
    assert_eq!(b.directory().len(), 2);
    assert_eq!(b.files().len(), 2);
    assert_eq!(b.meter().decoded(), 0, "open decodes no data block");
    assert!(b.entry_bytes(1).is_err());
    assert!(b.load_all().is_err());
}

#[test]
fn a_refused_block_is_not_kept() {
    // The budget: K refusals of the same read count no decode work (the
    // allowance below holds one decode of the entry, not K), and the same
    // read succeeds once the pool has room.
    let pool = Arc::new(Counting::new(1 << 20));
    let ctx = ctx_with(|c| {
        c.budget = pool.clone();
        c.limits.max_decode_work = 350_000;
    });
    let data = build(&BundleSpec {
        block_size: 100_000,
        entries: vec![EntrySpec {
            name: b"big".to_vec(),
            data: vec![3; 300_000],
            flags: 0,
        }],
        ..BundleSpec::default()
    });
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    // Another holder takes most of the pool.
    let other = pool
        .try_reserve((1 << 20) - pool.used() - 1000, Reservation::Input)
        .unwrap();
    // The work so far: the blocks-info decoded at open.
    let work = meter.decoded();
    for _ in 0..8 {
        let refused = b.entry_bytes(0);
        assert!(matches!(refused, Err(Error::Budget(_))), "{refused:?}");
    }
    assert_eq!(meter.decoded(), work, "a refused growth counts no work");
    drop(other);
    assert_eq!(
        b.entry_bytes(0).unwrap().len(),
        300_000,
        "the same read succeeds once there is room"
    );

    // The meter: two bundles of one open share it; while the first holds
    // its run the second's block is refused, and once the first drops, the
    // same read of the second succeeds.
    // One read of the entry peaks near 500 KB (its run grows 200 KB to
    // 300 KB holding both); two need far more.
    let ctx = ctx_with(|c| c.limits.max_input_memory = 700_000);
    let meter = meter_for(&ctx);
    let first = open_with(&data, &ctx, &meter).unwrap();
    let second = open_with(&data, &ctx, &meter).unwrap();
    let held = first.entry_bytes(0).unwrap();
    let refused = second.entry_bytes(0);
    assert!(
        matches!(&refused, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{refused:?}"
    );
    drop((held, first));
    assert_eq!(second.entry_bytes(0).unwrap().len(), 300_000);
}

#[test]
fn decode_work_stops_a_lazy_read_and_a_reopen_on_the_same_meter_shares_it() {
    let ctx = ctx_with(|c| {
        c.limits.max_block_output = 4096;
        c.limits.max_decode_work = 64 * 4096;
    });
    let data = build(&BundleSpec {
        block_size: 4096,
        entries: vec![
            EntrySpec {
                name: b"first".to_vec(),
                data: vec![1; 4096 * 100],
                flags: 0,
            },
            EntrySpec {
                name: b"last".to_vec(),
                data: vec![2; 10],
                flags: 0,
            },
        ],
        ..BundleSpec::default()
    });
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let held = meter.used();
    // The last entry: every block before it measured (decoded to the block
    // cap and dropped) until the work bound stops the read.
    let got = b.entry_bytes(1);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    assert_eq!(
        meter.used(),
        held,
        "measuring decodes returned their memory"
    );
    // Opened again on the same meter (no restart), the allowance is spent:
    // even a read of ten blocks stops.
    let ten = 0..4096 * 10;
    let again = open_with(&data, &ctx, &meter).unwrap();
    let got = again.entry_range(0, ten.clone());
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    // A reopen (a new open of the input) has its own allowance.
    let fresh = open_with(&data, &ctx, &meter.reopen()).unwrap();
    assert_eq!(fresh.entry_range(0, ten).unwrap().len(), 4096 * 10);
}

#[test]
fn each_block_of_a_run_is_capped_on_its_own() {
    let ctx = ctx_with(|c| c.limits.max_block_output = 4096);
    let entry: Vec<u8> = (0..3 * 4096 + 100).map(|i: u32| (i % 251) as u8).collect();
    let spec = BundleSpec {
        block_size: 4096,
        entries: vec![EntrySpec {
            name: b"three-blocks".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        ..BundleSpec::default()
    };
    // A run of four blocks passes the cap; each block is within it.
    let b = open_with(&build(&spec), &ctx, &meter_for(&ctx)).unwrap();
    assert_eq!(b.entry_bytes(0).unwrap(), entry);
    b.load_all().unwrap();
    // One block of 8 KiB does not.
    let one = build(&BundleSpec {
        block_size: 8192,
        ..spec
    });
    let b = open_with(&one, &ctx, &meter_for(&ctx)).unwrap();
    let got = b.entry_bytes(0);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput),
        "{got:?}"
    );
}

#[test]
fn an_lzma_block_that_outgrows_its_declaration_is_charged_until_the_last_slice_drops() {
    let entry: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
    let data = build(&BundleSpec {
        data_codec: TestCodec::Lzma,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"grown".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        block_overrides: vec![BlockOverride {
            block: 0,
            codec: None,
            uncompressed: Some(1000),
            compressed: None,
        }],
        ..BundleSpec::default()
    });
    let c = ctx();
    let meter = meter_for(&c);
    let b = open_with(&data, &c, &meter).unwrap();
    let held = meter.used();
    let slice = b.entry_bytes(0).unwrap();
    assert_eq!(
        slice, entry,
        "the block's actual output, not its declaration"
    );
    assert!(
        meter.used() >= held + 100_000,
        "charged at its actual length: {} over {held}",
        meter.used()
    );
    drop(b);
    assert!(meter.used() >= 100_000, "the slice keeps its run's charge");
    drop(slice);
    assert_eq!(meter.used(), 0, "returned with the last slice");
}

/// Round 2's second hostile input: rows whose spans are nested suffixes of
/// the joined data, the shortest first. Read lazily in directory order,
/// with every entry's bytes kept (as an Environment keeps what it parsed),
/// each read decodes its own run, charged, until the meter stops one with
/// `InputMemory`; the eager load, one run, fits.
#[test]
fn nested_suffix_spans_stop_a_lazy_read_at_the_meter_where_load_all_fits() {
    const N: usize = 32;
    const B: usize = 1 << 16;
    let entries: Vec<EntrySpec> = (0..N)
        .map(|k| EntrySpec {
            name: format!("suffix-{k:02}").into_bytes(),
            data: (0..B)
                .map(|i| ((i * 31 + k) % 251).to_le_bytes()[0])
                .collect(),
            flags: 0,
        })
        .collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: B,
        entries,
        ..BundleSpec::default()
    });
    for k in 0..N {
        let key = format!("suffix-{k:02}\0").into_bytes();
        let at = data.windows(key.len()).position(|w| w == key).unwrap();
        let offset = i64::try_from((N - 1 - k) * B).unwrap();
        let size = i64::try_from((k + 1) * B).unwrap();
        data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    }
    let ctx = ctx_with(|c| c.limits.max_input_memory = 6 << 20);
    let eager = open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
    eager.load_all().unwrap();
    drop(eager);

    let meter = meter_for(&ctx);
    let lazy = open_with(&data, &ctx, &meter).unwrap();
    let mut kept = Vec::new();
    let mut last = meter.used();
    let stopped = loop {
        assert!(kept.len() < N, "every read fit");
        match lazy.entry_bytes(kept.len()) {
            Ok(bytes) => {
                assert_eq!(bytes.len(), (kept.len() + 1) * B);
                assert!(meter.used() > last, "each re-decode is charged");
                last = meter.used();
                kept.push(bytes);
            }
            Err(e) => break e,
        }
    };
    assert!(
        matches!(&stopped, Error::Limit(l) if l.kind == LimitKind::InputMemory),
        "{stopped:?}"
    );
    assert!(kept.len() > 1 && meter.used() <= 6 << 20);
}

/// A codec that, while decoding, reads entries of its own bundle: another
/// entry and the one being read.
struct Reentrant {
    bundle: OnceLock<Weak<BundleFile>>,
    depth: Mutex<u32>,
}

impl BlockCodec for Reentrant {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        let enter = {
            let mut d = self.depth.lock().unwrap();
            *d += 1;
            *d == 1
        };
        if enter && let Some(b) = self.bundle.get().and_then(Weak::upgrade) {
            let _ = b.entry_bytes(0);
            let _ = b.entry_bytes(1);
        }
        *self.depth.lock().unwrap() -= 1;
        Lz4.decode(input, uncompressed, out)
    }
}

/// A codec that takes a lock another thread holds while it reads the same
/// bundle.
struct Locking(Arc<Mutex<()>>);

impl BlockCodec for Locking {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        let _g = self.0.lock().unwrap();
        Lz4.decode(input, uncompressed, out)
    }
}

fn custom_bundle() -> Vec<u8> {
    build(&BundleSpec {
        info_codec: TestCodec::Lz4,
        data_codec: TestCodec::Raw(5),
        block_size: 1 << 20,
        entries: small_entries(),
        ..BundleSpec::default()
    })
    .into_iter()
    .collect::<Vec<u8>>()
}

/// The custom-codec bundle with its one data block LZ4-compressed under id 5.
fn custom_lz4_bundle() -> Vec<u8> {
    let entries = small_entries();
    let joined: Vec<u8> = entries.iter().flat_map(|e| e.data.clone()).collect();
    let block = unityrs_testkit::bundle::compress(TestCodec::Lz4, &joined);
    build(&BundleSpec {
        info_codec: TestCodec::Lz4,
        data_codec: TestCodec::Raw(5),
        block_size: 1 << 20,
        entries,
        block_overrides: vec![BlockOverride {
            block: 0,
            codec: Some(TestCodec::Raw(5)),
            compressed: Some(u32::try_from(block.len()).unwrap()),
            ..BlockOverride::default()
        }],
        ..BundleSpec::default()
    })
    .into_iter()
    .collect::<Vec<u8>>()
    .pipe(|raw| replace_block(&raw, &joined, &block))
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

/// The stored block `plain` swapped for `packed` (the builder stores a
/// `Raw` codec's data as it is).
fn replace_block(raw: &[u8], plain: &[u8], packed: &[u8]) -> Vec<u8> {
    let at = raw.windows(plain.len()).position(|w| w == plain).unwrap();
    let mut out = raw[..at].to_vec();
    out.extend_from_slice(packed);
    out.extend_from_slice(&raw[at + plain.len()..]);
    out
}

fn guarded(f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("no lock is held across caller code (10 s guard)");
}

#[test]
fn a_codec_may_read_its_own_bundle_while_decoding() {
    let _ = custom_bundle();
    for eager in [false, true] {
        guarded(move || {
            let codec = Arc::new(Reentrant {
                bundle: OnceLock::new(),
                depth: Mutex::new(0),
            });
            let mut codecs = Codecs::unitypy();
            codecs.set(Codec::Other(5), codec.clone()).unwrap();
            let ctx = ctx_with(|c| c.codecs = Arc::new(codecs));
            let b = open_with(&custom_lz4_bundle(), &ctx, &meter_for(&ctx)).unwrap();
            let _ = codec.bundle.set(Arc::downgrade(&b));
            if eager {
                b.load_all().unwrap();
            }
            assert_eq!(&b.entry_bytes(1).unwrap()[..], &small_entries()[1].data[..]);
        });
    }
}

#[test]
fn a_codec_may_wait_for_a_thread_that_reads_the_same_bundle() {
    for eager in [false, true] {
        guarded(move || {
            let lock = Arc::new(Mutex::new(()));
            let mut codecs = Codecs::unitypy();
            codecs
                .set(Codec::Other(5), Arc::new(Locking(lock.clone())))
                .unwrap();
            let ctx = ctx_with(|c| c.codecs = Arc::new(codecs));
            let b = open_with(&custom_lz4_bundle(), &ctx, &meter_for(&ctx)).unwrap();
            let held = lock.lock().unwrap();
            let reader = {
                let b = b.clone();
                std::thread::spawn(move || {
                    if eager {
                        b.load_all().unwrap();
                    }
                    b.entry_bytes(0).unwrap()
                })
            };
            // While the reader's codec waits for the lock, this thread reads
            // the same bundle's directory and files.
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(b.files().len(), 2);
            let _ = b.uncompressed_size();
            drop(held);
            assert_eq!(reader.join().unwrap().len(), 100);
        });
    }
}

#[test]
fn an_entry_holds_its_bundle_weakly() {
    let data = build(&BundleSpec {
        entries: vec![EntrySpec {
            name: b"CAB-x".to_vec(),
            data: file(&one_object(
                1,
                vec![NodeSpec::new(0, "GameObject", "Base")],
                vec![],
            )),
            flags: 4,
        }],
        ..BundleSpec::default()
    });
    let b = open(&data);
    let File::Serialized(sf) = b.entry(0).unwrap() else {
        panic!("a SerializedFile entry")
    };
    let parent = sf.parent().unwrap().clone();
    let ParentKind::Bundle {
        bundle,
        version_engine,
    } = &parent.kind
    else {
        panic!("a bundle parent")
    };
    assert_eq!(version_engine.as_bytes(), b"2022.3.0f1");
    assert_eq!(parent.depth, 1);
    assert!(bundle.upgrade().is_some());
    drop(b);
    assert!(
        bundle.upgrade().is_none(),
        "the entry does not keep its bundle alive"
    );
}

/// The directory row of `name` patched to `offset` and `size` (blocks-info
/// stored, so the row is in the file as written).
fn with_row(name: &[u8], offset: i64, size: i64) -> (Vec<u8>, Vec<u8>) {
    let entries = vec![
        EntrySpec {
            name: b"a".to_vec(),
            data: (0..50u8).collect(),
            flags: 0,
        },
        EntrySpec {
            name: b"zz-target".to_vec(),
            data: (100..130u8).collect(),
            flags: 0,
        },
    ];
    let joined: Vec<u8> = entries.iter().flat_map(|e| e.data.clone()).collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 16,
        entries,
        ..BundleSpec::default()
    });
    let mut key = name.to_vec();
    key.push(0);
    let at = data.windows(key.len()).position(|w| w == key).unwrap();
    data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
    data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    (data, joined)
}

#[test]
fn entry_slices_are_python_slices_of_the_joined_data() {
    for (offset, size) in [
        (0, 30),
        (-5, 3),
        (-5, 10),
        (-1, -1),
        (10, -1),
        (10, -3),
        (-100, 5),
        (70, 0),
        (75, 100),
        (80, 1),
        (200, 5),
        (i64::MIN, 4),
        (3, i64::MAX),
        (i64::MAX, i64::MAX),
        (-3, i64::MIN),
    ] {
        let (data, joined) = with_row(b"zz-target", offset, size);
        let b = open(&data);
        let (range, _) = py_read(joined.len(), i128::from(offset), size);
        let want = &joined[range];
        let got = b.entry_bytes(1).unwrap();
        assert_eq!(&got[..], want, "offset {offset} size {size}");
        assert!(b.load_all().is_ok());
        assert_eq!(&b.entry_bytes(1).unwrap()[..], want);
    }
}

#[test]
fn a_meter_spans_every_file_of_one_bundle() {
    let one = file(&one_object(
        1,
        vec![
            NodeSpec::new(0, "GameObject", "Base"),
            NodeSpec::new(1, "int", "m_A").size(4),
        ],
        vec![0; 4],
    ));
    let entry = |n: &str| EntrySpec {
        name: n.as_bytes().to_vec(),
        data: one.clone(),
        flags: 4,
    };
    let single = build(&BundleSpec {
        entries: vec![entry("CAB-1")],
        ..BundleSpec::default()
    });
    let double = build(&BundleSpec {
        entries: vec![entry("CAB-1"), entry("CAB-2")],
        ..BundleSpec::default()
    });
    // What one file of one entry costs, opened and loaded.
    let c = ctx();
    let m = meter_for(&c);
    let b = open_with(&single, &c, &m).unwrap();
    b.load_all().unwrap();
    let cost = m.used();
    drop(b);
    // A meter that holds one file's load but not two.
    let limit = cost + cost / 3;
    let ctx = ctx_with(|c| c.limits.max_input_memory = limit);
    let m = meter_for(&ctx);
    let b = open_with(&single, &ctx, &m).unwrap();
    b.load_all().unwrap();
    drop(b);
    let m = meter_for(&ctx);
    let b = open_with(&double, &ctx, &m).unwrap();
    let got = b.load_all();
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
}

#[test]
fn many_rows_of_one_entry_stop_at_the_meter() {
    let one = file(&one_object(
        1,
        vec![NodeSpec::new(0, "GameObject", "Base")],
        vec![0; 4],
    ));
    let entries: Vec<EntrySpec> = (0..2000)
        .map(|i| EntrySpec {
            name: format!("CAB-{i}").into_bytes(),
            data: one.clone(),
            flags: 4,
        })
        .collect();
    let data = build(&BundleSpec {
        block_size: 1 << 17,
        entries,
        ..BundleSpec::default()
    });
    let ctx = ctx_with(|c| c.limits.max_input_memory = 2 << 20);
    let m = meter_for(&ctx);
    let b = open_with(&data, &ctx, &m).unwrap();
    let got = b.load_all();
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
    assert!(m.used() <= 2 << 20);
}

#[test]
fn unsupported_bundles_are_named() {
    let base = BundleSpec {
        entries: small_entries(),
        ..BundleSpec::default()
    };
    let c = ctx();
    let m = meter_for(&c);
    let web7 = unityrs_testkit::bundle::build_with_signature(
        &BundleSpec {
            format: 7,
            ..base.clone()
        },
        b"UnityWeb",
    );
    assert!(matches!(
        open_with(&web7, &c, &m),
        Err(Error::Unsupported(Unsupported::WebRawLayout))
    ));
    let archive = unityrs_testkit::bundle::build_with_signature(&base, b"UnityArchive");
    assert!(matches!(
        open_with(&archive, &c, &m),
        Err(Error::Unsupported(Unsupported::UnityArchive))
    ));
    let enc = build(&BundleSpec {
        extra_flags: 0x1000,
        ..base.clone()
    });
    assert!(matches!(
        open_with(&enc, &c, &m),
        Err(Error::Unsupported(Unsupported::Encrypted))
    ));
    let lzham = build(&BundleSpec {
        info_codec: TestCodec::Raw(4),
        ..base
    });
    assert!(matches!(
        open_with(&lzham, &c, &m),
        Err(Error::Unsupported(Unsupported::Lzham))
    ));
}

#[test]
fn mutated_bundles_never_panic() {
    let inputs: Vec<Vec<u8>> = matrix()
        .into_iter()
        .filter(|(n, _)| n.ends_with(".bundle"))
        .map(|(_, b)| b)
        .collect();
    let mut rng = Rng::new(0x5EED);
    let ctx = ctx_with(|c| {
        c.limits.max_input_memory = 64 << 20;
        c.limits.max_block_output = 1 << 20;
    });
    for round in 0..10_000 {
        let mut data = inputs[round % inputs.len()].clone();
        match rng.below(3) {
            0 => {
                let n = 1 + rng.below(8);
                rng.flip_bits(&mut data, n);
            }
            1 => rng.truncate(&mut data),
            _ => {
                rng.flip_bits(&mut data, 1);
                rng.truncate(&mut data);
            }
        }
        let m = meter_for(&ctx);
        if let Ok(b) = open_with(&data, &ctx, &m) {
            for i in 0..b.directory().len().min(4) {
                let _ = b.entry_bytes(i);
                let _ = b.entry(i);
                let _ = b.entry_range(i, 1..9);
            }
            let _ = b.load_all();
        }
    }
}

#[test]
fn the_unbounded_budget_is_the_default() {
    // UnityPy mode books nothing outside the meter.
    let c = FileContext::unitypy();
    assert!(
        c.budget
            .try_reserve(u64::MAX / 2, Reservation::Input)
            .is_ok()
    );
    let _ = Unbounded;
}
