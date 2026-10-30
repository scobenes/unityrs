//! The cost of a read (plan section 3.6, **Runs**), and its bytes over random
//! block layouts. A read finds its blocks in the known prefix and its run in
//! the run index by binary search, and counts every step as the open's work
//! (`Meter::decoded`), so K reads over B blocks count O(K log B) steps,
//! eagerly and lazily, where each read walked the blocks from the first one
//! (K x B, counted by nothing). Every read of a random layout, in a random
//! order, eagerly, lazily and on several threads at once, is the Python
//! slice of the joined data that the builder laid out.

use std::ops::Range;
use std::sync::Arc;

use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded};
use unityrs_core::files::BundleFile;
use unityrs_core::{Bytes, Error, FileContext};
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build};

use super::{ctx, ctx_with, meter_for, open, open_with};

/// The work one step counts (`files::bundle::index`, `STEP`).
const STEP: u64 = 8;

/// The rows a binary search of `n` rows reads at most, ⌈log2 n⌉ + 1, as the
/// prefix's search counts them.
fn search(n: usize) -> u64 {
    n.checked_sub(1).map_or(0, |m| u64::from(m.bit_width()) + 1)
}

fn n64(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

/// Blocks, and directory rows over them, in the step tests.
const B: usize = 2048;

fn byte(i: usize) -> u8 {
    u8::try_from(i % 251).unwrap()
}

/// [`B`] one-byte blocks with a one-byte entry in each.
fn one_byte_entries(codec: TestCodec) -> Vec<u8> {
    build(&BundleSpec {
        data_codec: codec,
        block_size: 1,
        entries: (0..B)
            .map(|i| EntrySpec {
                name: format!("e{i:05}").into_bytes(),
                data: vec![byte(i)],
                flags: 0,
            })
            .collect(),
        ..BundleSpec::default()
    })
}

/// The work of reading every entry of `b` in `order`, each read checked.
fn read_work(b: &BundleFile, order: impl Iterator<Item = usize>) -> u64 {
    let before = b.meter().decoded();
    for i in order {
        assert_eq!(&b.entry_bytes(i).unwrap()[..], &[byte(i)], "entry {i}");
    }
    b.meter().decoded() - before
}

/// Fix pass 4's unfinished item: every read walked the blocks from block 0
/// to its span's end and scanned every cached run, and nothing counted it,
/// so K rows over B blocks cost K x B steps, quadratic in the rows of a
/// bundle of one-byte entries. A read now searches the
/// known prefix twice (its first block, its span's end) and the run index
/// once, and counts each row a search reads: after `load_all`, K reads over
/// B blocks count at most K (2 log B + 1) steps; and lazily, last entry
/// first, each read also learns the blocks before it (once each, over all
/// the reads), decodes its own block again into a run and publishes it.
/// With the old walk each read counted none of this; a linear search of the
/// prefix counts K x B / 2 rows.
#[test]
fn k_reads_over_b_blocks_cost_log_b_steps_each_eagerly_and_lazily() {
    for codec in [TestCodec::None, TestCodec::Lz4] {
        let data = one_byte_entries(codec);
        let c = ctx();
        let eager = open_with(&data, &c, &meter_for(&c)).unwrap();
        assert_eq!(eager.blocks().len(), B);
        eager.load_all().unwrap();
        let work = read_work(&eager, 0..B);
        let most = STEP * n64(B) * (2 * search(B) + 1);
        assert!(
            (STEP * n64(B) * 3..=most).contains(&work),
            "{codec:?}, eagerly: {work} counted, at most {most}"
        );
        // Each read again: the same searches, the same steps.
        assert_eq!(read_work(&eager, 0..B), work, "{codec:?}");

        let lazy = open_with(&data, &c, &meter_for(&c)).unwrap();
        let work = read_work(&lazy, (0..B).rev());
        // Per read: two prefix searches, the run index searched to look
        // up and to publish, and one block decoded again (a step, and at
        // most its byte); per block, learned once (a step, and its byte
        // when it is measured).
        let per_read = 2 * search(B) + 2 * search(B) + 1;
        let most = STEP * (n64(B) * per_read + n64(B)) + 2 * n64(B);
        assert!(
            (STEP * n64(B) * 3..=most).contains(&work),
            "{codec:?}, lazily: {work} counted, at most {most}"
        );
    }
}

/// One entry of `len` stored bytes in one-byte blocks, and `rows` directory
/// rows, every one over all of it (row 0 holds it; the others, empty, are
/// patched to its offset and size).
fn rows_over_one_stored_span(len: usize, rows: usize) -> Vec<u8> {
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::None,
        block_size: 1,
        entries: (0..rows)
            .map(|k| EntrySpec {
                name: format!("s{k:04}").into_bytes(),
                data: if k == 0 {
                    (0..len).map(byte).collect()
                } else {
                    Vec::new()
                },
                flags: 0,
            })
            .collect(),
        ..BundleSpec::default()
    });
    for k in 1..rows {
        let key = format!("s{k:04}\0").into_bytes();
        let at = data.windows(key.len()).position(|w| w == key).unwrap();
        data[at - 20..at - 12].copy_from_slice(&0_i64.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&i64::try_from(len).unwrap().to_be_bytes());
    }
    data
}

/// Stored blocks are their own bytes, so a read whose blocks are all stored
/// is one slice of the input, found by checking each block of its span. The
/// slice is now cached as a run, so K rows over one stored span of L blocks
/// check its blocks once (the first read learns and checks them, 2 L steps),
/// and each later read is its searches (two of the prefix, one of the run
/// index), not another K x L steps.
#[test]
fn rows_over_one_stored_span_check_its_blocks_once() {
    const L: usize = 2048;
    const K: usize = 64;
    let b = open(&rows_over_one_stored_span(L, K));
    assert_eq!(b.blocks().len(), L);
    let want: Vec<u8> = (0..L).map(byte).collect();
    let before = b.meter().decoded();
    for i in 0..K {
        assert_eq!(&b.entry_bytes(i).unwrap()[..], &want[..], "row {i}");
    }
    let work = b.meter().decoded() - before;
    let most = STEP * (2 * n64(L) + 1 + n64(K) * (2 * search(L) + 1));
    assert!(
        (STEP * 2 * n64(L)..=most).contains(&work),
        "{work} counted, at most {most}"
    );
}

/// A read of nothing at a block's start reads no block, as the walk from
/// block 0 stopped before a block starting at the read's end: it costs the
/// one search for its first block. Entry 1 of [`super::small_entries`] (100
/// bytes in, LZ4 blocks of 64) is read first, so every block is known; its
/// range `28..28` is joined offset 128, where block 2 starts.
#[test]
fn a_read_of_nothing_at_a_blocks_start_reads_no_block() {
    let data = build(&BundleSpec {
        data_codec: TestCodec::Lz4,
        block_size: 64,
        entries: super::small_entries(),
        ..BundleSpec::default()
    });
    let b = open(&data);
    b.entry_bytes(1).unwrap();
    let blocks = b.blocks().len();
    assert_eq!(blocks, 7);
    let before = b.meter().decoded();
    assert!(b.entry_range(1, 28..28).unwrap().is_empty());
    let work = b.meter().decoded() - before;
    assert!(
        (1..=STEP * search(blocks)).contains(&work),
        "{work}: more than one search of {blocks} blocks"
    );
}

/// Copies a block as it is: slot 0's codec that is not the built-in one,
/// so stored blocks are measured by decoding them
/// (`Codecs::stores_as_is` is false).
struct Copying;

impl BlockCodec for Copying {
    fn decode(&self, input: &Bytes, _: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
        out.extend_from_slice(input)?;
        Ok(Decoded::Written)
    }
}

/// `reader.Position = offset; reader.read(size)` on `len` joined bytes
/// (`files/File.py:78-81`, `EndianBinaryReader.py:250-256`): size 0 reads
/// nothing, -1 reads to the end, and any other size is the Python slice
/// `view[offset : offset + size]`, negative indexes from the end.
fn python_read(len: usize, offset: i64, size: i64) -> Range<usize> {
    if size == 0 {
        return 0..0;
    }
    let l = i128::try_from(len).unwrap();
    let start = i128::from(offset);
    let stop = if size == -1 {
        l
    } else {
        start + i128::from(size)
    };
    let index = |i: i128| usize::try_from(if i < 0 { i + l } else { i }.clamp(0, l)).unwrap();
    let (a, b) = (index(start), index(stop));
    a..b.max(a)
}

/// A random bundle: its bytes, the joined data the builder laid out, each
/// directory row's offset and size, and the context to read it with.
struct Layout {
    data: Vec<u8>,
    joined: Vec<u8>,
    rows: Vec<(i64, i64)>,
    ctx: Arc<FileContext>,
}

fn random_offset(rng: &mut Rng, len: usize) -> i64 {
    let l = i64::try_from(len).unwrap();
    match rng.below(8) {
        0 => -1 - i64::try_from(rng.below(len + 4)).unwrap(),
        1 => [i64::MIN, i64::MAX, l, l + 1][rng.below(4)],
        _ => i64::try_from(rng.below(len + 6)).unwrap() - 2,
    }
}

fn random_size(rng: &mut Rng, len: usize) -> i64 {
    match rng.below(8) {
        0 => -1,
        1 => 0,
        2 => -2 - i64::try_from(rng.below(len + 4)).unwrap(),
        3 => i64::MAX,
        _ => i64::try_from(rng.below(len + 6)).unwrap(),
    }
}

/// Random data cut into random rows, in blocks of a random size and codec;
/// stored blocks are sometimes emptied (their bytes moved into the next
/// block's), sometimes read through [`Copying`], and sometimes among
/// compressed ones; each row then patched to a random offset and size.
fn layout(rng: &mut Rng) -> Layout {
    let len = rng.below(160);
    let joined: Vec<u8> = (0..len).map(|_| byte(rng.below(251))).collect();
    let rows = 1 + rng.below(6);
    let mut cuts: Vec<usize> = (1..rows).map(|_| rng.below(len + 1)).collect();
    cuts.sort_unstable();
    let bounds: Vec<usize> = std::iter::once(0)
        .chain(cuts)
        .chain(std::iter::once(len))
        .collect();
    let entries: Vec<EntrySpec> = bounds
        .windows(2)
        .enumerate()
        .map(|(i, w)| EntrySpec {
            name: format!("r{i:03}").into_bytes(),
            data: joined[w[0]..w[1]].to_vec(),
            flags: 0,
        })
        .collect();
    let block_size = 1 + rng.below(24);
    let data_codec = [TestCodec::None, TestCodec::Lz4, TestCodec::Lzma][rng.below(3)];
    let blocks = len.div_ceil(block_size);
    let mut block_overrides = Vec::new();
    if data_codec != TestCodec::None {
        // Some blocks stored, so known at open between ones a read decodes.
        block_overrides = (0..blocks)
            .filter(|_| rng.below(4) == 0)
            .map(|block| BlockOverride {
                block,
                codec: Some(TestCodec::None),
                ..BlockOverride::default()
            })
            .collect();
    } else if rng.below(2) == 0 {
        let mut carry = 0;
        for k in 0..blocks {
            let size = block_size.min(len - k * block_size) + carry;
            let empty = k + 1 < blocks && rng.below(3) == 0;
            carry = if empty { size } else { 0 };
            block_overrides.push(BlockOverride {
                block: k,
                compressed: Some(u32::try_from(if empty { 0 } else { size }).unwrap()),
                ..BlockOverride::default()
            });
        }
    }
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec,
        block_size,
        entries,
        block_overrides,
        ..BundleSpec::default()
    });
    let rows: Vec<(i64, i64)> = (0..rows)
        .map(|i| {
            let (offset, size) = (random_offset(rng, len), random_size(rng, len));
            let key = format!("r{i:03}\0").into_bytes();
            let at = data.windows(key.len()).position(|w| w == key).unwrap();
            data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
            data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
            (offset, size)
        })
        .collect();
    let ctx = if rng.below(4) == 0 {
        let mut codecs = Codecs::unitypy();
        codecs.set(Codec::None, Arc::new(Copying)).unwrap();
        ctx_with(|c| c.codecs = Arc::new(codecs))
    } else {
        ctx()
    };
    Layout {
        data,
        joined,
        rows,
        ctx,
    }
}

/// A read: a row's bytes, or a range of them.
#[derive(Clone, Debug)]
enum Read {
    Entry(usize),
    Range(usize, Range<u64>),
}

fn random_reads(rng: &mut Rng, rows: usize) -> Vec<Read> {
    (0..3 * rows)
        .map(|_| {
            let i = rng.below(rows);
            if rng.below(2) == 0 {
                Read::Entry(i)
            } else {
                let a = n64(rng.below(20));
                Read::Range(i, a..n64(rng.below(20)))
            }
        })
        .collect()
}

impl Layout {
    /// What `read` returns: the row's Python slice of the joined data, and
    /// for a range, its part of that slice, short at the end.
    fn want(&self, read: &Read) -> &[u8] {
        let (i, range) = match read {
            Read::Entry(i) => (*i, None),
            Read::Range(i, r) => (*i, Some(r.clone())),
        };
        let (offset, size) = self.rows[i];
        let span = python_read(self.joined.len(), offset, size);
        let Some(r) = range else {
            return &self.joined[span];
        };
        let (start, end) = (n64(span.start), n64(span.end));
        let lo = start.saturating_add(r.start).min(end);
        let hi = start.saturating_add(r.end).clamp(lo, end);
        &self.joined[usize::try_from(lo).unwrap()..usize::try_from(hi).unwrap()]
    }

    fn open(&self) -> Arc<BundleFile> {
        open_with(&self.data, &self.ctx, &meter_for(&self.ctx)).unwrap()
    }

    fn check(&self, b: &BundleFile, reads: &[Read], how: &str) {
        for read in reads {
            let got = match read {
                Read::Entry(i) => b.entry_bytes(*i),
                Read::Range(i, r) => b.entry_range(*i, r.clone()),
            };
            assert_eq!(
                got.as_deref().map_err(ToString::to_string),
                Ok(self.want(read)),
                "{how}: {read:?} of rows {:?} over {} bytes in {} blocks",
                self.rows,
                self.joined.len(),
                b.blocks().len()
            );
        }
    }
}

/// The new prefix and run index against the model a read must match: over
/// random block layouts (empty blocks, stored blocks as slices of the input
/// or through a codec, LZ4, LZMA) and random rows (negative offsets, sizes
/// of -1 and past the end), every read in a random order, then after
/// `load_all`, and on four threads at once each in its own order, is the
/// Python slice of the data the builder joined (`load_all` decodes every
/// block before it parses the rows, which random bytes may fail).
#[test]
fn random_layouts_read_in_random_orders_are_slices_of_the_joined_data() {
    let mut rng = Rng::new(19);
    for case in 0..400 {
        let l = layout(&mut rng);
        let rows = l.rows.len();
        let lazy = l.open();
        assert_eq!(lazy.directory().len(), rows, "case {case}");
        l.check(&lazy, &random_reads(&mut rng, rows), "lazy");
        let eager = l.open();
        // Every block decodes; a random entry may not parse, as in UnityPy.
        let loaded = eager.load_all();
        assert!(
            matches!(loaded, Ok(()) | Err(Error::Entry { .. })),
            "case {case}: {loaded:?}"
        );
        l.check(&eager, &random_reads(&mut rng, rows), "eager");
        let shared = l.open();
        let orders: Vec<Vec<Read>> = (0..4).map(|_| random_reads(&mut rng, rows)).collect();
        std::thread::scope(|s| {
            for order in &orders {
                let (l, b) = (&l, &shared);
                s.spawn(move || l.check(b, order, "threads"));
            }
        });
    }
}
