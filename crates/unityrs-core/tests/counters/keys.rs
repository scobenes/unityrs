//! A bundle's key (`unityrs_core::testing`, "The key"): a nested bundle's
//! path counted however long the bundles above it live, its counts kept
//! while its root drops meanwhile; a reserve's old tables freed with the
//! lock let go; a bundle a caller's codec opens while the core decodes is
//! bare; and with no table reserved, the core's side of the counters
//! allocates nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded, Stored};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::BundleFile;
use unityrs_core::limits::Meter;
use unityrs_core::testing;
use unityrs_core::{Bytes, Error, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::guard::finishes;

use super::{
    FREE_READS, MADE, READ_ON_FREE, cab_bundle, id_of, in_child, lazy, nested, nesting, probing,
    row, src, tables, the_bundle, whole,
};

#[test]
fn a_nested_bundle_read_after_the_bundles_above_it_drop_is_counted_at_its_path() {
    in_child("keys::kept_inner_probe");
}

/// Round 2: the path was found by walking up to the bundles themselves, so
/// with the outer bundle dropped the read was lost and the flag set.
#[test]
#[ignore = "run by a_nested_bundle_read_after_the_bundles_above_it_drop_is_counted_at_its_path"]
fn kept_inner_probe() {
    if !probing() {
        return;
    }
    testing::reserve_span_reads(16, 64, 17);
    let env = lazy();
    let file = env.load(src("outer", &nesting(&cab_bundle("k")))).unwrap();
    let id = Some(id_of("outer"));
    let outer = Arc::downgrade(the_bundle(&file));
    let inner = nested(the_bundle(&file), 0);
    drop(file);
    drop(env);
    assert!(outer.upgrade().is_none(), "the outer bundle is gone");
    inner.entry_bytes(1).unwrap();
    let (slots, _, overflowed) = tables();
    assert!(!overflowed);
    assert_eq!(slots, [row(id, &[0], 1), row(id, &[0, 1], 1)]);
}

/// Set when [`Owner`] drops.
static DROPPED: AtomicBool = AtomicBool::new(false);

/// A source's bytes whose owner reads the tables when it drops, as a
/// caller's owner may run any code: dropped under the tables' lock, it
/// would wait on that lock.
struct Owner(Vec<u8>);

impl AsRef<[u8]> for Owner {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        testing::span_reads(|_| ());
        DROPPED.store(true, Ordering::Release);
    }
}

/// A bundle of one LZ4 block whose row 0 is `inner`: its rows are decoded
/// copies, which keep nothing of its own bytes alive.
fn packed(inner: &[u8]) -> Vec<u8> {
    build(&BundleSpec {
        info_codec: TestCodec::Lz4,
        data_codec: TestCodec::Lz4,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"inner".to_vec(),
            data: inner.to_vec(),
            flags: 4,
        }],
        ..BundleSpec::default()
    })
}

#[test]
fn a_leafs_counts_survive_its_root_dropping_meanwhile() {
    in_child("keys::concurrent_drop_probe");
}

/// A leaf reads while another thread drops its root, whose owner reads the
/// tables as it drops: every read is counted at the leaf's path, and the
/// drop finishes. Both threads are counted with [`READ_ON_FREE`] set, so
/// every block either frees reads the tables first: a free under the
/// tables' lock, a read's or the drop's, would wait on that lock and fail
/// the guard (round 4). Round 2 held each bundle above the leaf during a
/// read; a node now holds a link and a row, and a count borrows its key
/// (the compile-time checks beside `Ancestry` in `src/testing.rs`); a
/// reserve's free is `reserve_free_probe`'s.
#[test]
#[ignore = "run by a_leafs_counts_survive_its_root_dropping_meanwhile"]
fn concurrent_drop_probe() {
    const READS: u64 = 200;
    if !probing() {
        return;
    }
    testing::reserve_span_reads(16, 64, 17);
    let env = lazy();
    let data = Bytes::from_owner(Owner(packed(&packed(&cab_bundle("l")))));
    let file = env
        .load(Source::Bytes {
            name: RawStr::from("root"),
            data,
        })
        .unwrap();
    let id = Some(id_of("root"));
    let root = Arc::downgrade(the_bundle(&file));
    let leaf = nested(&nested(the_bundle(&file), 0), 0);
    let (gone, dropped) = finishes("a leaf's reads while its root drops", move || {
        let gate = Barrier::new(2);
        READ_ON_FREE.store(true, Ordering::Release);
        std::thread::scope(|s| {
            let gate = &gate;
            s.spawn(move || {
                unityrs_testkit::counted::count_this_thread();
                gate.wait();
                drop(file);
                drop(env);
            });
            unityrs_testkit::counted::count_this_thread();
            gate.wait();
            for _ in 0..READS {
                leaf.entry_bytes(1).unwrap();
            }
        });
        READ_ON_FREE.store(false, Ordering::Release);
        (root.upgrade().is_none(), DROPPED.load(Ordering::Acquire))
    });
    assert!(gone && dropped, "the root and its owner were dropped");
    assert!(
        FREE_READS.load(Ordering::Relaxed) > 0,
        "the frees read the tables"
    );
    let (slots, _, overflowed) = tables();
    assert!(!overflowed);
    assert_eq!(
        slots,
        [
            row(id, &[0], 1),
            row(id, &[0, 0], 1),
            row(id, &[0, 0, 1], READS)
        ]
    );
}

#[test]
fn a_reserve_frees_the_old_tables_with_the_lock_let_go() {
    in_child("keys::reserve_free_probe");
}

/// Round 3: a reserve swapped its new tables in under the lock, so the old
/// tables were freed there, and an allocator that reads the tables when a
/// block is freed waited on the lock it held.
#[test]
#[ignore = "run by a_reserve_frees_the_old_tables_with_the_lock_let_go"]
fn reserve_free_probe() {
    if !probing() {
        return;
    }
    // Tables with room and counts in them.
    testing::reserve_span_reads(16, 64, 17);
    testing::hooks_once(&RawStr::from("old"));
    assert_ne!(tables().0, []);
    let read = finishes(
        "a reserve over tables an allocator reads as they free",
        || {
            unityrs_testkit::counted::count_this_thread();
            READ_ON_FREE.store(true, Ordering::Release);
            testing::reserve_span_reads(4, 8, 4);
            READ_ON_FREE.store(false, Ordering::Release);
            FREE_READS.load(Ordering::Relaxed)
        },
    );
    assert!(read > 0, "the old tables' free read the tables");
    let (slots, names, overflowed) = tables();
    assert!(slots.is_empty() && names.is_empty() && !overflowed);
}

/// A stored codec that, the first time it decodes, opens a bundle of its
/// own and reads a row of it.
struct Opener {
    own: Bytes,
    opened: AtomicBool,
}

impl BlockCodec for Opener {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        if !self.opened.swap(true, Ordering::AcqRel) {
            let ctx = Arc::new(FileContext::unitypy());
            let own = BundleFile::open(
                RawStr::from("own"),
                self.own.clone(),
                None,
                &Meter::unlimited(),
                &ctx,
            )?;
            own.entry(0)?;
        }
        Stored.decode(input, uncompressed, out)
    }
}

#[test]
fn a_bundle_a_codec_opens_while_the_core_decodes_is_bare() {
    in_child("keys::codec_probe");
}

/// Round 2: the load's key was a thread-local scope around the core's
/// parse, which the blocks-info's codec ran inside, so the codec's own
/// bundle was counted as the load's.
#[test]
#[ignore = "run by a_bundle_a_codec_opens_while_the_core_decodes_is_bare"]
fn codec_probe() {
    if !probing() {
        return;
    }
    // A bundle whose blocks-info is stored under the custom id 5.
    let entry = |name: &str, data: Vec<u8>| EntrySpec {
        name: name.as_bytes().to_vec(),
        data,
        flags: 4,
    };
    let inner = cab_bundle("c");
    let data = build(&BundleSpec {
        info_codec: TestCodec::Raw(5),
        block_size: 1 << 20,
        entries: vec![entry("inner", inner)],
        ..BundleSpec::default()
    });
    let opener = Arc::new(Opener {
        own: Bytes::from(cab_bundle("o")),
        opened: AtomicBool::new(false),
    });
    let mut codecs = Codecs::unitypy();
    codecs.set(Codec::Other(5), opener.clone()).unwrap();
    let mut ctx = FileContext::unitypy();
    ctx.codecs = Arc::new(codecs);
    testing::reserve_span_reads(4, 8, 17);
    let env = Environment::new(EnvOptions::batch(Arc::new(ctx)));
    let file = env.load(src("l", &data)).unwrap();
    assert!(opener.opened.load(Ordering::Acquire), "the codec ran");
    the_bundle(&file).entry(0).unwrap();
    let id = Some(id_of("l"));
    assert_eq!(tables().0, [row(None, &[0], 1), row(id, &[0], 1)]);
}

#[test]
fn with_no_table_reserved_the_cores_hooks_allocate_nothing() {
    in_child("keys::hooks_probe");
}

/// The core's side alone, in a window where nothing else allocates: a load
/// id's draw, a top-level and two nested keys, and the span-read hook (round
/// 2: a run with no table was compared with one recording, so an allocation
/// both made passed).
#[test]
#[ignore = "run by with_no_table_reserved_the_cores_hooks_allocate_nothing"]
fn hooks_probe() {
    if !probing() {
        return;
    }
    unityrs_testkit::counted::count_this_thread();
    let name = RawStr::from("hooks");
    let made = || {
        let before = MADE.load(Ordering::Relaxed);
        testing::hooks_once(&name);
        MADE.load(Ordering::Relaxed) - before
    };
    assert_eq!(made(), 0, "with no table reserved");
    // The window sees an allocation: with a table, the nested keys' nodes.
    testing::reserve_span_reads(4, 16, 4);
    assert_eq!(made(), 2, "the two nested keys' nodes");
    let id = Some(id_of("hooks"));
    let (slots, _, overflowed) = tables();
    assert!(!overflowed);
    assert_eq!(
        slots,
        [
            row(id, &[0], 1),
            whole(id, &[], 1),
            row(id, &[0, 0], 1),
            whole(id, &[0], 1),
            row(id, &[0, 1, 0], 1),
            whole(id, &[0, 1], 1),
        ]
    );
}
