//! Every byte the dump hashes is counted as its open's work before it is
//! hashed (`unityrs_dump::input`, "Work"; `PARITY.md` section 19.3, "the
//! dump's hashing"). Directory rows, objects and values may overlap, so K
//! of them over the same S bytes, a slice each to the core, were K x S of
//! hashing to the dump, counted by nothing: a small bundle of many rows,
//! each over the same decoded span, ran for hours while the core's own work
//! stayed flat.
//!
//! - What the dump adds to its open's work is exactly the bytes it hashes:
//!   K x S for K rows over S bytes, for K objects over S bytes, and for K
//!   values (each summarised by its sha256) over S bytes; an object read
//!   whole and re-saved counts its raw bytes, its value's and its resave's.
//!   Besides, each object's read and resave count their own work, the
//!   core's (`unityrs_core::walk`, "Work").
//! - Past `max_decode_work` the input is its `DecodeWork` refusal, in g3b's
//!   grammar (`limit DecodeWork: <v> > <max>`), and with the bound lifted
//!   (`--unbounded DecodeWork`) it dumps whole.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::check_file_type;
use unityrs_core::files::parse_file;
use unityrs_core::limits::Meter;
use unityrs_core::walk::VALUE_WORK;
use unityrs_core::{Bytes, FileContext, LimitKind, RawStr};
use unityrs_dump::DumpOptions;
use unityrs_dump::input::{dump_on, load_deep};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::{ObjectSpec, build as build_serialized, one_object};

const K: usize = 8;
const S: usize = 64 << 10;

/// A UnityFS bundle (stored blocks) of `k` directory rows, each over the
/// same `s` bytes: built with one entry of `s` bytes and `k - 1` empty ones,
/// then each row's offset and size rewritten to `0` and `s`.
fn overlapping_rows(k: usize, s: usize) -> Vec<u8> {
    let names: Vec<Vec<u8>> = (0..k).map(|i| format!("row-{i:03}").into_bytes()).collect();
    let spec = BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::None,
        block_size: s,
        entries: names
            .iter()
            .enumerate()
            .map(|(i, name)| EntrySpec {
                name: name.clone(),
                data: if i == 0 { vec![7; s] } else { Vec::new() },
                flags: 0,
            })
            .collect(),
        ..BundleSpec::default()
    };
    let mut out = build(&spec);
    for name in &names {
        let mut key = name.clone();
        key.push(0);
        let at = out
            .windows(key.len())
            .position(|w| w == key.as_slice())
            .unwrap();
        // `offset: i64, size: i64, flags: u32`, big-endian, before the path.
        let row = at - 20;
        out[row..row + 8].copy_from_slice(&0_i64.to_be_bytes());
        out[row + 8..row + 16].copy_from_slice(&i64::try_from(s).unwrap().to_be_bytes());
    }
    out
}

/// A SerializedFile of `k` objects whose `byte_size` is `s`, each starting
/// where the last one's `s` bytes do (the others hold none), so every
/// object's raw bytes are the same `s`; each reads nothing (`check_read`).
fn overlapping_objects(k: usize, s: usize) -> Vec<u8> {
    let mut spec = one_object(1, vec![NodeSpec::new(0, "Base", "Base")], Vec::new());
    let first = spec.objects[0].clone();
    spec.objects = (1..=k)
        .map(|i| ObjectSpec {
            path_id: i64::try_from(i).unwrap(),
            data: if i == k { vec![7; s] } else { Vec::new() },
            byte_size: Some(u32::try_from(s).unwrap()),
            ..first.clone()
        })
        .collect();
    build_serialized(&spec)
}

/// A SerializedFile of `k` objects at one offset, each reading the same
/// `vector<UInt8>` of `s` bytes, and each of `byte_size` `size`.
fn byte_arrays(k: usize, s: usize, size: usize) -> Vec<u8> {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "vector", "data"),
        NodeSpec::new(2, "Array", "Array"),
        NodeSpec::new(3, "int", "size"),
        NodeSpec::new(3, "UInt8", "data"),
    ];
    let mut spec = one_object(1, nodes, Vec::new());
    let first = spec.objects[0].clone();
    let mut last = i32::try_from(s).unwrap().to_le_bytes().to_vec();
    last.extend(vec![7; s]);
    spec.objects = (1..=k)
        .map(|i| ObjectSpec {
            path_id: i64::try_from(i).unwrap(),
            data: if i == k { last.clone() } else { Vec::new() },
            byte_size: Some(u32::try_from(size).unwrap()),
            ..first.clone()
        })
        .collect();
    build_serialized(&spec)
}

/// [`byte_arrays`] of `byte_size` 0, so each reads its array past its end
/// (a `check_read` error, whose value the dump keeps): a value's bytes are a
/// zero-copy slice of the file, so the `k` values cost the core's read
/// nothing each, and their summaries' sha256s were `k` x `s` of hashing.
fn overlapping_values(k: usize, s: usize) -> Vec<u8> {
    byte_arrays(k, s, 0)
}

/// The three inputs, what each of their `K` records holds once, and the
/// work each record's read counts before its hash (the core's,
/// `unityrs_core::walk`, "Work"): none for a row; the root of an object
/// that reads nothing; the root, the vector and its `S` bytes read one by
/// one by the pure-Python reader.
fn inputs() -> [(&'static str, Vec<u8>, String, u64); 3] {
    [
        ("rows", overlapping_rows(K, S), format!("\"size\":{S},"), 0),
        (
            "objects",
            overlapping_objects(K, S),
            format!("\"raw_size\":{S},"),
            VALUE_WORK,
        ),
        (
            "values",
            overlapping_values(K, S),
            format!("{{\"t\":\"u8s\",\"len\":{S},"),
            2 * VALUE_WORK + bytes(S),
        ),
    ]
}

fn with_work(max: u64) -> DumpOptions {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_decode_work = max;
    DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    }
}

/// The work the core's load of `data` counts, and the dump's records of it
/// with the work its whole dump counted.
fn work(data: &[u8], opts: &DumpOptions) -> (u64, String, u64) {
    let ctx = &opts.context;
    let meter = || Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let load = meter();
    let bytes = Bytes::copy_from_slice(data);
    let ty = check_file_type(&bytes, &ctx.limits).unwrap();
    let file = parse_file(RawStr::from("in"), bytes, ty, None, &load, ctx).unwrap();
    load_deep(&file).unwrap();
    let dump = meter();
    let text = dump_on(b"in", Bytes::copy_from_slice(data), &dump, opts).text();
    (load.decoded(), text, dump.decoded())
}

fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

#[test]
fn the_dump_counts_every_byte_it_hashes_as_its_opens_work() {
    for (what, data, each, read) in inputs() {
        let (load, text, dumped) = work(&data, &DumpOptions::default());
        assert_eq!(text.lines().count(), K + 1, "{what}: {text}");
        assert!(!text.contains(r#""kind":"parse""#), "{what}: {text}");
        assert_eq!(text.matches(each.as_str()).count(), K, "{what}: {text}");
        assert_eq!(
            dumped - load,
            bytes(K * S) + bytes(K) * read,
            "{what}: the core's load counted {load}"
        );
    }
    // Each object read whole, and re-saved: its raw bytes, its value's and
    // its resave's, each counted once; and the read's and the resave's own
    // work, each the root, the vector and its `S` bytes.
    let data = byte_arrays(K, S, 4 + S);
    let (load, text, dumped) = work(&data, &DumpOptions::default());
    assert_eq!(text.matches(r#""resave":"equal""#).count(), K, "{text}");
    let walk = 2 * (2 * VALUE_WORK + bytes(S));
    assert_eq!(dumped - load, bytes(K * (3 * S + 8)) + bytes(K) * walk);
}

/// The work at which the dump's first refused count lands, `load` counted
/// and `max` the allowance, for records that each read (`read`, counted
/// whole once done) and then hash `S` bytes (counted only while it fits).
fn refused_at(load: u64, read: u64, max: u64) -> u64 {
    let mut done = load;
    loop {
        assert!(done <= max, "every read starts within the allowance");
        done += read;
        if done + bytes(S) > max {
            return done + bytes(S);
        }
        done += bytes(S);
    }
}

#[test]
fn hashing_past_the_opens_work_is_its_decode_work_refusal() {
    for (what, data, _, read) in inputs() {
        let (load, whole, _) = work(&data, &DumpOptions::default());
        // Room for three and a half hashes after the load, with the reads
        // before them: a hash is refused, and nothing past it is hashed.
        let max = load + bytes(3 * S + S / 2);
        let (_, text, dumped) = work(&data, &with_work(max));
        assert_eq!(
            text.lines().count(),
            1,
            "{what}: one record for the input: {text}"
        );
        let want = format!(
            r#""error":{{"kind":"parse","note":"limit DecodeWork: {} > {max}"}}"#,
            refused_at(load, read, max)
        );
        assert!(text.contains(&want), "{what}: {text}");
        assert!(dumped <= max, "{what}: {dumped} counted past {max}");
        // Lifted, the bound is gone and the input dumps whole.
        let lifted = with_work(max)
            .with_unbounded(&[LimitKind::DecodeWork])
            .unwrap();
        assert_eq!(work(&data, &lifted).1, whole, "{what}");
    }
}
