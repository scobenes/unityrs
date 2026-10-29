//! No lock across caller code: a codec may read its own bundle while it
//! decodes, or wait for a thread that reads it.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use super::{ctx_with, meter_for, open_with, small_entries};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded, Lz4};
use unityrs_core::files::BundleFile;
use unityrs_core::{Bytes, Error};
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, TestCodec, build};

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
