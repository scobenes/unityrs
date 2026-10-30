//! Every byte the dump hashes is counted as its open's work before it is
//! hashed (`unityrs_dump::input`, "Work"; `PARITY.md` section 19.3, "the
//! dump's hashing"). Directory rows, objects and values may overlap, so K
//! of them over the same S bytes, a slice each to the core, were K x S of
//! hashing to the dump, counted by nothing: a small bundle of many rows,
//! each over the same decoded span, ran for hours while the core's own work
//! stayed flat.
//!
//! - What the dump adds to its open's work is exactly the bytes it hashes:
//!   the input's own bytes, first, then K x S for K rows over S bytes, for K
//!   objects over S bytes, and for K
//!   values (each summarised by its sha256) over S bytes; an object read
//!   whole and re-saved counts its raw bytes, its value's and its resave's.
//!   Its reads of each row again add their steps, as any read's (the core's
//!   plan 3.6, "Runs"), and each object's read and resave count their own
//!   work, the core's (`unityrs_core::walk`, "Work").
//! - Past `max_decode_work` the input is its `DecodeWork` refusal, in g3b's
//!   grammar (`limit DecodeWork: <v> > <max>`), and with the bound lifted
//!   (`--unbounded DecodeWork`) it dumps whole. An input whose own bytes
//!   pass it is refused before its detection, its sha256 not taken.
//! - An input refused unread (past `max_input_bytes`) is hashed for its
//!   record from its file, which no limit bounds: chunk by chunk, each
//!   counted before it is hashed, so its hashing stops at the allowance and
//!   its sha256 is then not taken (`unityrs_dump::read`), which matches no
//!   index's pin.

mod common;

use std::sync::Arc;

use common::Tmp;
use serde_json::Value as Json;
use unityrs_core::budget::Reservation;
use unityrs_core::detect::check_file_type;
use unityrs_core::files::{File, parse_file};
use unityrs_core::limits::Meter;
use unityrs_core::walk::VALUE_WORK;
use unityrs_core::{Bytes, FileContext, LimitKind, RawStr};
use unityrs_dump::input::{dump_on, load_deep};
use unityrs_dump::json::{hex, sha256_hex};
use unityrs_dump::read::{CHUNK, NOT_TAKEN, ReadInput, read_on};
use unityrs_dump::run::{RunError, RunOptions, run};
use unityrs_dump::select::Selection;
use unityrs_dump::{DumpOptions, Levels};
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
/// work each record's object read counts before its hash (the core's,
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

/// The work the core's load of `data` counts, the dump's records of it with
/// the work its whole dump counted, and what one more read of a directory
/// row costs the loaded bundle (the dump reads each row's bytes again to
/// hash them; a read of a cached run counts its steps, the core's plan 3.6
/// "Runs"), 0 for a SerializedFile.
fn work(data: &[u8], opts: &DumpOptions) -> (u64, String, u64, u64) {
    let ctx = &opts.context;
    let meter = || Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let load = meter();
    let bytes = Bytes::copy_from_slice(data);
    let ty = check_file_type(&bytes, &ctx.limits).unwrap();
    let file = parse_file(RawStr::from("in"), bytes, ty, None, &load, ctx).unwrap();
    load_deep(&file).unwrap();
    let loaded = load.decoded();
    let read = match &file {
        File::Bundle(b) => {
            let each: Vec<u64> = (0..b.directory().len())
                .map(|i| {
                    let before = load.decoded();
                    b.entry_bytes(i).unwrap();
                    load.decoded() - before
                })
                .collect();
            assert!(each.windows(2).all(|w| w[0] == w[1]), "{each:?}");
            each.first().copied().unwrap_or(0)
        }
        _ => 0,
    };
    let dump = meter();
    let text = dump_on(b"in", Bytes::copy_from_slice(data), &dump, opts).text();
    (loaded, text, dump.decoded(), read)
}

fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

#[test]
fn the_dump_counts_every_byte_it_hashes_as_its_opens_work() {
    for (what, data, each, walk) in inputs() {
        let (load, text, dumped, read) = work(&data, &DumpOptions::default());
        assert_eq!(text.lines().count(), K + 1, "{what}: {text}");
        assert!(!text.contains(r#""kind":"parse""#), "{what}: {text}");
        assert_eq!(text.matches(each.as_str()).count(), K, "{what}: {text}");
        assert_eq!(
            dumped - load,
            bytes(data.len()) + bytes(K * S) + bytes(K) * (read + walk),
            "{what}: the core's load counted {load}"
        );
    }
    // Each object read whole, and re-saved: its raw bytes, its value's and
    // its resave's, each counted once; and the read's and the resave's own
    // work, each the root, the vector and its `S` bytes.
    let data = byte_arrays(K, S, 4 + S);
    let (load, text, dumped, _) = work(&data, &DumpOptions::default());
    assert_eq!(text.matches(r#""resave":"equal""#).count(), K, "{text}");
    let walk = 2 * (2 * VALUE_WORK + bytes(S));
    assert_eq!(
        dumped - load,
        bytes(data.len()) + bytes(K * (3 * S + 8)) + bytes(K) * walk
    );
}

/// The work at which the dump's first refused count lands, `load` counted
/// and `max` the allowance, for records that each read (`read`, counted
/// whole before the hash, and within the allowance here) and then hash `S`
/// bytes (counted only while it fits).
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
    for (what, data, _, walk) in inputs() {
        let (load, whole, _, read) = work(&data, &DumpOptions::default());
        // Room for three and a half hashes after the input's own and the
        // load, with the reads before them (each row read again, each object
        // read): a hash is refused, and nothing past it is hashed.
        let first = load + bytes(data.len());
        let max = first + bytes(3 * S + S / 2);
        let (_, text, dumped, _) = work(&data, &with_work(max));
        assert_eq!(
            text.lines().count(),
            1,
            "{what}: one record for the input: {text}"
        );
        let want = format!(
            r#""error":{{"kind":"parse","note":"limit DecodeWork: {} > {max}"}}"#,
            refused_at(first, read + walk, max)
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

/// An input whose own bytes are more than its work allowance is refused
/// before its detection: one `detect` record whose sha256 was not taken,
/// with nothing counted, and lifted it dumps whole. The input's bytes are
/// the first work its dump counts: an allowance of exactly their length
/// takes them, and the load is refused after.
#[test]
fn an_input_its_own_bytes_take_past_the_allowance_is_refused_before_detection() {
    let data = overlapping_rows(K, S);
    let len = bytes(data.len());
    let opts = with_work(len - 1);
    let meter = Meter::for_input(
        &opts.context.limits,
        opts.context.budget.clone(),
        Reservation::Input,
    );
    let d = dump_on(b"in", Bytes::copy_from_slice(&data), &meter, &opts);
    assert_eq!(meter.decoded(), 0, "a refused count counts nothing");
    assert_eq!(d.sha256, None);
    assert_eq!(d.lines.len(), 1, "{}", d.text());
    let rec: Json = serde_json::from_str(&d.lines[0]).unwrap();
    assert_eq!(rec["l0"]["type"], "error");
    assert_eq!(rec["l0"]["parsed_as"], "error");
    assert_eq!(rec["l0"]["size"], len);
    assert_eq!(rec["l0"]["sha256"], hex(&NOT_TAKEN));
    assert_eq!(rec["error"]["kind"], "detect");
    assert_eq!(
        rec["error"]["note"],
        format!(
            "its sha256 was not taken: limit DecodeWork: {len} > {}",
            len - 1
        )
    );
    // At exactly its length the input's bytes are taken, and what follows
    // (the load's work) is the parse's refusal.
    let meter = Meter::for_input(
        &with_work(len).context.limits,
        opts.context.budget.clone(),
        Reservation::Input,
    );
    let d = dump_on(
        b"in",
        Bytes::copy_from_slice(&data),
        &meter,
        &with_work(len),
    );
    assert_eq!(d.sha256.map(|s| hex(&s)), Some(sha256_hex(&data)));
    assert!(
        d.text()
            .contains(r#""kind":"parse","note":"limit DecodeWork: "#),
        "{}",
        d.text()
    );
    // Lifted, it dumps whole.
    let lifted = opts.with_unbounded(&[LimitKind::DecodeWork]).unwrap();
    let (_, whole, _, _) = work(&data, &DumpOptions::default());
    assert_eq!(work(&data, &lifted).1, whole);
}

/// A file of `n` bytes of the pattern `i % 251`, refused unread under a
/// `max_input_bytes` of 1,000.
fn refused_file(tmp: &Tmp, n: usize) -> (std::path::PathBuf, Vec<u8>) {
    let data: Vec<u8> = (0..n).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let path = tmp.0.join("in").join("big");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &data).unwrap();
    (path, data)
}

fn refused_limits(max_decode_work: u64) -> DumpOptions {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_bytes = 1000;
    ctx.limits.max_decode_work = max_decode_work;
    DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    }
}

/// An input refused unread is hashed for its record from its file, whose
/// length no limit bounds (it was refused for it): each chunk is counted
/// on the input's meter before it is hashed. Within the allowance the
/// count is exactly its length and the record has its sha256; past it,
/// the chunk the count refuses is not hashed, nothing past it is read, and
/// the record's sha256 is not taken, its note naming both refusals.
#[test]
fn a_refused_inputs_hash_is_counted_chunk_by_chunk_and_stops_at_the_allowance() {
    let tmp = Tmp::new("refused-hash");
    let n = 3 * CHUNK + 3392;
    let (path, data) = refused_file(&tmp, n);
    let record = |max: u64| {
        let opts = refused_limits(max);
        let ctx = &opts.context;
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let ReadInput::Refused(d) = read_on(b"big", &path, ctx, meter.clone()).unwrap() else {
            panic!("an input past max_input_bytes is refused");
        };
        assert_eq!(d.lines.len(), 1);
        let rec: Json = serde_json::from_str(&d.lines[0]).unwrap();
        (d.sha256, rec, meter.decoded())
    };

    let (sha, rec, counted) = record(1 << 20);
    assert_eq!(counted, bytes(n), "every byte hashed is counted, once");
    assert_eq!(sha.map(|s| hex(&s)), Some(sha256_hex(&data)));
    assert_eq!(rec["l0"]["sha256"], sha256_hex(&data));
    assert_eq!(rec["l0"]["size"], n);
    assert_eq!(rec["error"]["kind"], "detect");
    assert_eq!(
        rec["error"]["note"],
        format!("limit InputBytes: {n} > 1000")
    );

    // Room for two chunks and a half: two are counted and hashed, the
    // third's count is refused.
    let max = bytes(2 * CHUNK + CHUNK / 2);
    let (sha, rec, counted) = record(max);
    assert_eq!(
        counted,
        bytes(2 * CHUNK),
        "the refused chunk counts nothing"
    );
    assert_eq!(sha, None);
    assert_eq!(rec["l0"]["sha256"], hex(&NOT_TAKEN));
    assert_eq!(rec["l0"]["size"], n, "the length the file system gives");
    assert_eq!(rec["error"]["kind"], "detect");
    assert_eq!(
        rec["error"]["note"],
        format!(
            "limit InputBytes: {n} > 1000; its sha256 was not taken: limit DecodeWork: {} > {max}",
            3 * CHUNK
        )
    );
}

/// The run checks a refused input's sha256 against its index's pin: within
/// the allowance it is the file the index pins, and past it, its sha256
/// not taken, it is no file the index pins, so the run stops unless
/// mismatches are allowed.
#[test]
fn an_input_whose_sha256_was_not_taken_is_no_pinned_file() {
    let tmp = Tmp::new("not-taken-pin");
    let n = 3 * CHUNK + 3392;
    let (_, data) = refused_file(&tmp, n);
    let index = tmp.0.join("in").join("index.json");
    std::fs::write(
        &index,
        format!(
            r#"{{"files":[{{"path":"big","kind":"bundle","sha256":"{}"}}]}}"#,
            sha256_hex(&data)
        ),
    )
    .unwrap();
    let go = |max: u64, allow: bool| {
        run(&RunOptions {
            selection: Selection {
                inputs: vec![index.clone()],
                index: true,
                ..Selection::default()
            },
            output: tmp.0.join("out").join(format!("{max}-{allow}.jsonl")),
            dump: DumpOptions {
                levels: Levels::ALL,
                ..refused_limits(max)
            },
            stats: None,
            quiet: true,
            allow_index_mismatch: allow,
        })
    };
    assert_eq!(go(1 << 20, false).unwrap().index_sha256_mismatch, 0);
    let max = bytes(2 * CHUNK);
    match go(max, false) {
        Err(RunError::Failed(e)) => assert!(e.starts_with("big: its sha256 was not taken"), "{e}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(go(max, true).unwrap().index_sha256_mismatch, 1);
}

/// A gzip stream of `n` zero bytes, padded to the 128 bytes detection
/// needs to read a stream.
fn gzip_of_zeros(n: usize) -> Vec<u8> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&vec![0; n]).unwrap();
    let mut out = gz.finish().unwrap();
    out.resize(out.len().max(128), 0);
    out
}

/// The input's detection counts on its meter what a gzip stream's
/// inflation reads and writes, as the core counts an entry's
/// (`check_entry_type`): its dump counts the input's bytes, then exactly
/// the core's count of that detection; and an allowance that holds the
/// input's bytes but not the detection is the input's `detect` refusal.
#[test]
fn an_inputs_detection_counts_its_inflation_as_work() {
    let data = gzip_of_zeros(1 << 20);
    let len = bytes(data.len());
    let opts = DumpOptions::default();
    let ctx = &opts.context;
    let core = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    unityrs_core::detect::check_entry_type(&data, &ctx.limits, &core).unwrap();
    let detection = core.decoded();
    assert!(
        detection > 1 << 20,
        "the core counts what it inflates: {detection}"
    );
    let (_, text, dumped, _) = work_of(&data, &opts);
    assert!(text.contains(r#""parsed_as":"raw""#), "{text}");
    assert_eq!(dumped, len + detection);

    // The refused inflation's last chunk is work done, counted past the
    // bound (`Meter::add_work`), so the note's value is past it.
    let max = len + detection - 1;
    let (_, text, _, _) = work_of(&data, &with_work(max));
    assert!(
        text.contains(r#""error":{"kind":"detect","note":"detection: limit DecodeWork: "#)
            && text.contains(&format!(" > {max}\"}}")),
        "{text}"
    );
}

/// [`work`] without the core's own load (a file that is no container).
fn work_of(data: &[u8], opts: &DumpOptions) -> (u64, String, u64, u64) {
    let ctx = &opts.context;
    let dump = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let text = dump_on(b"in", Bytes::copy_from_slice(data), &dump, opts).text();
    (0, text, dump.decoded(), 0)
}

/// A SerializedFile of one object whose version string is `version`.
fn versioned(version: &[u8]) -> Vec<u8> {
    let mut spec = one_object(1, vec![NodeSpec::new(0, "Base", "Base")], Vec::new());
    spec.unity_version = version.to_vec();
    build_serialized(&spec)
}

/// A file record's packed version past 128 bits costs a pass over its
/// limbs for each 19 decimal digits, quadratic work each record of it
/// repeats: the dump counts each pass's limbs before it divides them. Two
/// files whose version strings are as long, one of them packing to a
/// 4,300-digit major and the other to 1, differ in the work their dumps add
/// to their loads by exactly that count: at least a limb for each 19
/// digits, at most every limb for each. Their loads differ as well, since
/// the core counts a version string's packing, over the limbs it holds, as
/// the input's work (its plan 3.4), so each dump's own work is taken past
/// its file's load.
#[test]
fn a_packed_version_past_128_bits_counts_its_conversion_as_work() {
    let big = [vec![b'1'; 4300], b".0.0f1".to_vec()].concat();
    let small = [vec![b'0'; 4299], b"1.0.0f1".to_vec()].concat();
    let (big_load, text_big, big_work, _) = work(&versioned(&big), &DumpOptions::default());
    let (small_load, text_small, small_work, _) = work(&versioned(&small), &DumpOptions::default());
    // The loads differ too: the core counts the big packing over its limbs.
    assert!(big_load > small_load, "{big_load} {small_load}");
    assert!(
        text_small.contains(r#""version_packed":281474976711425,"#),
        "{text_small}"
    );
    // 1...1 (4,300 ones) << 48 | f1: 4,314 digits, in fewer than 225 limbs.
    let digits = text_big
        .split(r#""version_packed":"#)
        .nth(1)
        .unwrap()
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    assert_eq!(digits, 4314);
    let passes = bytes(digits.div_ceil(19));
    let conversion = (big_work - big_load) - (small_work - small_load);
    assert!(
        (passes * 8..=passes * 225 * 8).contains(&conversion),
        "{conversion} counted for {passes} passes"
    );
    // The file record is made before its objects' records, and a dump at
    // L1 reads and re-saves nothing: the work an L1 dump counts ends with
    // the conversion's last pass, so an allowance one short of it refuses
    // that pass, which fails the input, whatever its objects' reads and
    // resaves would have counted after it. (An allowance one short of the
    // whole L0-L3 dump refuses whatever is counted last, here the resave's
    // end: its object's L3 outcome, since a write counts as it goes and
    // checks when it ends, the core's `walk::OpenWork`.)
    let l1 = DumpOptions {
        levels: Levels {
            l1: true,
            l2: false,
            l3: false,
        },
        ..DumpOptions::default()
    };
    let (_, _, l1_work, _) = work_of(&versioned(&big), &l1);
    assert!(l1_work < big_work, "{l1_work} of {big_work}");
    let (_, text, _, _) = work_of(&versioned(&big), &with_work(l1_work - 1));
    let refused = format!(
        r#""kind":"parse","note":"limit DecodeWork: {l1_work} > {}"#,
        l1_work - 1
    );
    assert!(text.contains(&refused), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}
