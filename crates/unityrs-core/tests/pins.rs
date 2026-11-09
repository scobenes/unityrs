//! What a value kept past its input's handles keeps alive (plan section 8,
//! R3; `unityrs_core::pin`), on testkit bundles of two entries laid back to
//! back: a `SerializedFile` of one object (a `string` and a `TypelessData`,
//! whose values are slices of the file) and a padding entry. Eagerly loaded,
//! the bundle's joined data is one decoded run, so the run's length `L` is
//! the two entries' lengths; the value's slices are `S` bytes (its string's
//! and its data's), and its tree's names `T` bytes, a slice per row's type
//! and name (`Base`/`Base`, `string`/`m_Name`, `TypelessData`/`m_Data`:
//! 38). A copy is made when `L - S >= max(1 MiB, 4 S)`, and it holds
//! `heap_cost(S)` and its owner record (`OWNER_COST`), on the input's meter.

use std::sync::Arc;

use unityrs_core::budget::{OWNER_COST, Reservation};
use unityrs_core::files::{BundleFile, SerializedFile};
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::pin::{Copies, UNPIN_MIN, worth_copying};
use unityrs_core::{Bytes, File, FileContext, Parsed, RawStr, Value};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::{build as build_file, one_object};

/// The tree's names, a slice per row's type and name.
const T: u64 = 4 + 4 + 6 + 6 + 12 + 6;

fn bytes(n: u64, seed: u64) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from((i * 13 + seed) % 251).unwrap())
        .collect()
}

/// A file of one object: `name` (a `string`) and `data` (`TypelessData`).
fn object_file(name: &[u8], data: &[u8]) -> Vec<u8> {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "string", "m_Name"),
        NodeSpec::new(1, "TypelessData", "m_Data"),
    ];
    let len = |b: &[u8]| i32::try_from(b.len()).unwrap().to_le_bytes();
    let mut body = Vec::new();
    body.extend_from_slice(&len(name));
    body.extend_from_slice(name);
    while body.len() % 4 != 0 {
        body.push(0);
    }
    body.extend_from_slice(&len(data));
    body.extend_from_slice(data);
    build_file(&one_object(1, nodes, body))
}

/// A bundle of `file` and `pad` padding bytes, cut into blocks of `block`.
fn bundle(file: &[u8], pad: u64, codec: TestCodec, block: u64) -> Vec<u8> {
    build(&BundleSpec {
        data_codec: codec,
        block_size: usize::try_from(block).unwrap(),
        entries: vec![
            EntrySpec {
                name: b"CAB-pins".to_vec(),
                data: file.to_vec(),
                flags: 4,
            },
            EntrySpec {
                name: b"CAB-pins.resS".to_vec(),
                data: bytes(pad, 7),
                flags: 0,
            },
        ],
        ..BundleSpec::default()
    })
}

struct Read {
    meter: Arc<Meter>,
    bundle: Arc<BundleFile>,
    file: Arc<SerializedFile>,
    value: Parsed,
}

/// Opens `data`, loads it eagerly (one run) or reads only its file's
/// entry, and reads the object.
fn read(data: &[u8], eager: bool) -> Read {
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let bundle = BundleFile::open(
        RawStr::from("pins.bundle"),
        Bytes::copy_from_slice(data),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    if eager {
        bundle.load_all().unwrap();
    }
    let File::Serialized(file) = bundle.entry(0).unwrap() else {
        panic!("not a serialized file");
    };
    let value = file.objects().next().unwrap().parse_as_dict(true).unwrap();
    Read {
        meter,
        bundle,
        file,
        value,
    }
}

/// The value's string and data.
fn slices(p: &Parsed) -> (Vec<u8>, Bytes) {
    let Value::Dict(m) = p.value() else {
        panic!("{:?}", p.value())
    };
    let [(_, Value::Str(s)), (_, Value::Bytes(b))] = m.entries() else {
        panic!("{m:?}")
    };
    (s.as_bytes().to_vec(), b.clone())
}

/// What `value` keeps alive once every handle of its input is dropped,
/// beyond its own charge and its node lists'.
fn kept_beyond_own(r: Read) -> u64 {
    let Read {
        meter,
        bundle,
        file,
        value,
    } = r;
    drop((file, bundle));
    let own = value.charge().bytes()
        + value
            .lists()
            .iter()
            .map(|l| l.charge().bytes())
            .sum::<u64>();
    meter.used() - own
}

fn copies(values: u64, value_bytes: u64, trees: u64, tree_bytes: u64) -> Copies {
    let mut c = Copies::default();
    (c.values, c.value_bytes, c.trees, c.tree_bytes) = (values, value_bytes, trees, tree_bytes);
    c
}

/// A small value (S = 100) in a run of 2 MiB and more: its slices and its
/// tree's names are copied out, each into one buffer, so once the handles
/// drop it keeps those two copies alive and nothing of the run. Before the
/// fix it kept the run (`heap_cost(L)` and more): a kept value held all of
/// its input's decoded bytes.
#[test]
fn a_kept_value_copies_its_slices_out_of_a_large_run() {
    let (name, data) = (bytes(60, 1), bytes(40, 2));
    let file = object_file(&name, &data);
    let r = read(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), true);
    let l = u64::try_from(file.len()).unwrap() + (2 << 20);
    assert_eq!(r.file.pinned(), l, "the run of both entries");
    assert_eq!(r.file.copies(), copies(1, 100, 1, T));
    let (s, b) = slices(&r.value);
    assert_eq!((s.as_slice(), &b[..]), (name.as_slice(), data.as_slice()));
    let run = r.file.data().as_ptr().addr()..r.file.data().as_ptr().addr() + r.file.data().len();
    assert!(!run.contains(&b.as_ptr().addr()), "the data is a copy");
    let kept = kept_beyond_own(r);
    assert_eq!(
        kept,
        heap_cost(100) + OWNER_COST + heap_cost(T) + OWNER_COST
    );
}

/// The threshold on the file's run: `L - S` of exactly `max(1 MiB, 4 S)`
/// copies, a byte less does not (the padding sets `L`). Under 1 MiB of
/// slices the floor is 1 MiB; at 512 KiB of slices it is four times them.
#[test]
fn a_value_is_copied_out_exactly_when_the_run_is_long_enough() {
    for data_len in [40_u64, (512 << 10) - 60] {
        let (name, data) = (bytes(60, 1), bytes(data_len, 2));
        let s = 60 + data_len;
        let file = object_file(&name, &data);
        let own = u64::try_from(file.len()).unwrap();
        let need = s + UNPIN_MIN.max(4 * s);
        for (pad, copied) in [(need - own, true), (need - own - 1, false)] {
            let r = read(&bundle(&file, pad, TestCodec::Lz4, 1 << 16), true);
            assert_eq!(r.file.pinned(), own + pad);
            assert_eq!(worth_copying(own + pad, s), copied);
            let c = r.file.copies();
            assert_eq!(
                (c.values, c.value_bytes),
                if copied { (1, s) } else { (0, 0) },
                "S {s}, L {}",
                own + pad
            );
            assert_eq!(
                (c.trees, c.tree_bytes),
                (1, T),
                "the names are copied at both"
            );
        }
    }
}

/// Read lazily, an entry is a slice of a run of its own blocks only, so
/// what its value keeps alive is that run (64 KiB blocks: under 1 MiB, no
/// copy). Of a stored bundle it is a slice of the bundle's own bytes, which
/// it keeps alive whole (2 MiB of padding: copied).
#[test]
fn what_a_slice_keeps_alive_is_its_run_or_the_bundles_own_bytes() {
    let file = object_file(&bytes(60, 1), &bytes(40, 2));
    let own = u64::try_from(file.len()).unwrap();
    let lz4 = read(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), false);
    assert!(lz4.file.pinned() < UNPIN_MIN, "{}", lz4.file.pinned());
    assert!(lz4.file.pinned() >= own);
    assert_eq!(lz4.file.copies(), Copies::default());
    let data = bundle(&file, 2 << 20, TestCodec::None, 1 << 16);
    let stored = read(&data, false);
    assert_eq!(stored.file.pinned(), u64::try_from(data.len()).unwrap());
    assert_eq!(stored.file.copies(), copies(1, 100, 1, T));
}
