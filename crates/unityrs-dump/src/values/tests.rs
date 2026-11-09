use super::*;
use serde_json::json;
use unityrs_core::budget::{Counting, Unbounded};

fn lines(out: &[u8]) -> Vec<Json> {
    out.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect()
}

#[test]
fn cases_parse_as_the_script_writes_them() {
    let text = br#"[{"name":"c","endian":">","nodes":[[0,"Base","Base",0],[1,"int","a",16384]],"hex":"0001ff"}]"#;
    let cases = parse_cases(text).unwrap();
    assert_eq!(
        cases,
        [Case {
            name: "c".into(),
            big_endian: true,
            nodes: vec![
                (0, "Base".into(), "Base".into(), 0),
                (1, "int".into(), "a".into(), 16384)
            ],
            data: vec![0, 1, 0xff],
        }]
    );
    assert!(parse_cases(br#"[{"name":"c","nodes":[[0,"x"]],"hex":""}]"#).is_err());
    assert!(parse_cases(br#"[{"name":"c","nodes":[],"hex":"0"}]"#).is_err());
}

/// A case as deep as a tree may nest (`Limits::max_tree_depth`), run
/// from a caller's thread far smaller than reading it needs: the cases
/// run on a stack of their own, and it reads and re-saves.
#[test]
fn a_deep_case_runs_whatever_the_callers_stack() {
    let mut nodes = vec![json!([0, "Base", "Base", 0])];
    nodes.extend((1..255).map(|level| json!([level, "C", "c", 0])));
    nodes.push(json!([255, "int", "x", 0]));
    let text = json!([{"name": "deep", "nodes": nodes, "hex": "07000000"}]).to_string();
    let line = std::thread::Builder::new()
        .stack_size(64 << 10)
        .spawn(move || {
            let mut out = Vec::new();
            run(text.as_bytes(), &mut out).map(|()| out)
        })
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
    let rec = &lines(&line)[0];
    assert_eq!(rec["kind"], Json::Null, "{rec}");
    assert_eq!(rec["resave"], "equal", "{rec}");
}

/// The line holds every member the script reads, each as JSON: strings
/// escaped, the re-saved bytes as hex, a missing member `null`.
#[test]
fn a_line_holds_every_member() {
    let text = json!([
        {"name": "a\"\n\u{2028}", "nodes": [[0, "Base", "Base", 0], [1, "string", "s", 0]],
         "hex": "02000000227a0000"},
        {"name": "short", "nodes": [[0, "Base", "Base", 0], [1, "int", "x", 0]], "hex": "0100"},
    ])
    .to_string();
    let mut out = Vec::new();
    run(text.as_bytes(), &mut out).unwrap();
    assert!(out.ends_with(b"\n"));
    let recs = lines(&out);
    assert_eq!(
        recs[0],
        json!({"name": "a\"\n\u{2028}", "kind": null, "bytes_read": null,
               "value": r#"{"t":"dict","v":[["s","\"z"]]}"#, "resave": "equal",
               "resave_hex": "02000000227a0000", "note": null})
    );
    assert_eq!(recs[1]["name"], "short");
    assert_eq!(recs[1]["kind"], "out_of_bounds", "{}", recs[1]);
    assert_eq!(recs[1]["resave_hex"], Json::Null);
    assert!(recs[1]["note"].is_string());
}

/// Review round 15's case: an unaligned `TypelessData` puts the vector's
/// count at byte 5, where it reads `0x03000000`, so 16 bytes read as
/// 50,331,648 empty structs.
fn wide_pptrs() -> Case {
    parse_cases(
        json!([{"name": "r15", "nodes": [
            [0, "Base", "Base", 0],
            [1, "TypelessData", "f0", 0],
            [2, "int", "size", 0],
            [2, "UInt8", "data", 0],
            [1, "vector", "f1", 0],
            [2, "Array", "Array", 0],
            [3, "int", "size", 0],
            [3, "PPtr<Object>", "data", 0],
            [1, "unsigned short", "f2", 16384]
        ], "hex": "01000000e500000003000000ff7f0000"}])
        .to_string()
        .as_bytes(),
    )
    .unwrap()
    .remove(0)
}

/// `count` structs of one field, an empty struct named `name_len` bytes
/// long, in 4 bytes: a value far smaller than its text.
fn wide_names(count: i32, name_len: usize) -> Case {
    Case {
        name: "wide".into(),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "vector".into(), "f".into(), 0),
            (2, "Array".into(), "Array".into(), 0),
            (3, "int".into(), "size".into(), 0),
            (3, "S".into(), "data".into(), 0),
            (4, "E".into(), "n".repeat(name_len), 0),
        ],
        data: count.to_le_bytes().to_vec(),
    }
}

fn small() -> Case {
    Case {
        name: "small".into(),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "int".into(), "x".into(), 0),
        ],
        data: vec![7, 0, 0, 0],
    }
}

fn bounded(max_input_memory: u64) -> Limits {
    let mut limits = Limits::default();
    limits.max_input_memory = max_input_memory;
    limits
}

fn unbounded() -> Arc<dyn MemoryBudget> {
    Arc::new(Unbounded)
}

/// Review round 15: the read ran on an unlimited meter, so nothing bounded
/// what a case's bytes read as. On the case's own meter, round 15's case
/// is refused at `max_input_memory` while it reads (kind `other`, the
/// limit's text in the note), and the next case still runs.
#[test]
fn a_read_past_the_cases_memory_bound_is_its_outcome() {
    let limits = bounded(64 << 20);
    let mut out = Vec::new();
    run_cases(&[wide_pptrs(), small()], &limits, &unbounded(), &mut out).unwrap();
    let recs = lines(&out);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0]["kind"], "other", "{}", recs[0]);
    // The read's refusal, at the vector's first element (byte 9).
    let note = recs[0]["note"].as_str().unwrap();
    assert!(
        note.starts_with("Other at byte 9: limit InputMemory: "),
        "{note}"
    );
    assert_eq!(recs[0]["value"], Json::Null);
    assert_eq!(recs[1]["kind"], Json::Null, "{}", recs[1]);
    assert_eq!(recs[1]["value"], r#"{"t":"dict","v":[["x",7]]}"#);
}

/// Review round 15: the value's text grew in a plain `String`, charged to
/// nothing. A value of 16,384 structs read in 1.4 MiB writes 16 MiB of
/// text, which the case's 8 MiB bound refuses as it grows: the case's
/// outcome is that refusal, with no value.
#[test]
fn a_value_whose_text_passes_the_cases_memory_bound_is_its_outcome() {
    let wide = wide_names(1 << 14, 1000);
    let o = run_case_on(&wide, &bounded(8 << 20), &unbounded());
    assert_eq!(o.kind, Some("other"), "{o:?}");
    let note = o.note.as_deref().unwrap();
    assert!(note.starts_with("limit InputMemory: "), "{note}");
    assert!(o.value.is_none() && o.resaved.is_none(), "{o:?}");
    // Its read alone fits: at 64 MiB it has its value, 16 MiB of text.
    let o = run_case_on(&wide, &bounded(64 << 20), &unbounded());
    assert_eq!(o.kind, None, "{o:?}");
    assert!(o.value_text().unwrap().len() > 16 << 20);
    assert_eq!(o.resave, "equal");
}

/// A refusal of memory by the budget (or the allocator) is the case's
/// `crash`, never an abort: the read's, the text's and the resave's.
#[test]
fn a_refusal_of_memory_is_the_cases_crash() {
    for (case, budget) in [
        // The read: the bound is far above, the budget refuses.
        (wide_pptrs(), 32 << 20),
        // The text: the read fits in the budget, the text does not.
        (wide_names(1 << 14, 1000), 8 << 20),
    ] {
        let budget: Arc<dyn MemoryBudget> = Arc::new(Counting::new(budget));
        let o = run_case_on(&case, &Limits::default(), &budget);
        assert_eq!(o.kind, Some(CRASH), "{o:?}");
        let note = o.note.as_deref().unwrap();
        assert!(
            note.starts_with("unityrs ran out of memory on this case (memory budget refused"),
            "{note}"
        );
        assert!(o.value.is_none() && o.resaved.is_none(), "{o:?}");
    }
    // The resave: a `vector<UInt8>` of 1 MiB reads as a slice of the
    // case's bytes (1 MiB, charged), and its resave needs a buffer as big
    // again, which a budget of 1.5 MiB refuses.
    let mut data = (1_i32 << 20).to_le_bytes().to_vec();
    data.resize(4 + (1 << 20), 3);
    let bytes = Case {
        name: "bytes".into(),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "vector".into(), "v".into(), 0),
            (2, "Array".into(), "Array".into(), 0),
            (3, "int".into(), "size".into(), 0),
            (3, "UInt8".into(), "data".into(), 0),
        ],
        data,
    };
    let fits = run_case_on(&bytes, &Limits::default(), &unbounded());
    assert_eq!(
        (fits.kind, fits.resave.as_str()),
        (None, "equal"),
        "{fits:?}"
    );
    let budget: Arc<dyn MemoryBudget> = Arc::new(Counting::new(3 << 19));
    let o = run_case_on(&bytes, &Limits::default(), &budget);
    assert_eq!(o.kind, Some(CRASH), "{o:?}");
}

/// A `vector<UInt8>` holding `body`.
fn byte_vector(body: &[u8]) -> Case {
    let mut data = i32::try_from(body.len()).unwrap().to_le_bytes().to_vec();
    data.extend_from_slice(body);
    Case {
        name: "bytes".into(),
        big_endian: false,
        nodes: vec![
            (0, "Base".into(), "Base".into(), 0),
            (1, "vector".into(), "v".into(), 0),
            (2, "Array".into(), "Array".into(), 0),
            (3, "int".into(), "size".into(), 0),
            (3, "UInt8".into(), "data".into(), 0),
        ],
        data,
    }
}

/// Review round 16: a resave refused at a bound of the limits is the
/// resave's error, as any write error is (`value_cases.py`'s `oracle`
/// keeps UnityPy's value and writes `error: <exception>`), never the case's
/// `other`: no kind, the value kept, `resave` `error: ` and the limit's
/// text, no bytes, no note. A `vector<UInt8>` of 1 MiB reads as a slice of
/// the case's bytes (1 MiB, charged), and its resave needs a buffer as big
/// again: a bound of 1.5 MiB holds the read and the text and refuses the
/// buffer.
#[test]
fn a_resave_refused_at_a_bound_is_its_resave_error() {
    let case = byte_vector(&vec![3; 1 << 20]);
    let mut out = Vec::new();
    run_cases(&[case], &bounded(3 << 19), &unbounded(), &mut out).unwrap();
    let rec = &lines(&out)[0];
    assert_eq!(rec["kind"], Json::Null, "{rec}");
    assert!(rec["value"].is_string(), "{rec}");
    let resave = rec["resave"].as_str().unwrap();
    assert!(resave.starts_with("error: limit InputMemory: "), "{resave}");
    assert_eq!(
        (&rec["resave_hex"], &rec["note"]),
        (&Json::Null, &Json::Null),
        "{rec}"
    );
}

/// Each case runs on a meter of its own (module docs, "Memory"), so the
/// work a case does (`max_decode_work`) is its own, as an input's is, and no
/// case is refused for what the cases before it did. Under an allowance of
/// one and a half times one case's work, the same case twice is answered
/// the same twice (review round 16: on one meter for the run, the second
/// was refused, `limit DecodeWork`); under one byte less than its work, it
/// is refused.
#[test]
fn each_case_runs_on_a_meter_of_its_own() {
    let body: Vec<u8> = (0..16 << 10)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let case = byte_vector(&body);
    let meter = Meter::for_input(&Limits::default(), unbounded(), Reservation::Input);
    let first = outcome_on(&case, &Limits::default(), &meter);
    assert!(matches!(&first, Ok(o) if o.kind.is_none() && o.resave == "equal"));
    drop(first);
    let work = meter.decoded();
    let mut limits = Limits::default();
    limits.max_decode_work = work - 1;
    let o = run_case_on(&case, &limits, &unbounded());
    let said = o.note.as_deref().unwrap_or(&o.resave);
    assert!(said.contains("limit DecodeWork: "), "{said}");
    limits.max_decode_work = work + work / 2;
    let mut out = Vec::new();
    run_cases(&[case.clone(), case], &limits, &unbounded(), &mut out).unwrap();
    let recs = lines(&out);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0]["resave"], "equal", "{}", recs[0]);
    assert_eq!(recs[1], recs[0]);
}

/// A base of `rows - 1` empty structs, each of type `ty` and named `name`:
/// no byte to read.
fn tree_case(rows: usize, ty: &str, name: &str) -> Case {
    let mut nodes = vec![(0, "Base".to_owned(), "Base".to_owned(), 0)];
    nodes.extend((1..rows).map(|_| (1, ty.to_owned(), name.to_owned(), 0)));
    Case {
        name: "tree".into(),
        big_endian: false,
        nodes,
        data: Vec::new(),
    }
}

/// The tree's rows and their names are charged to the case's meter, as an
/// input's typetree rows are (review round 15's sweep: they were built on
/// an unlimited meter, `NodeList::trusted`): 100,000 rows whose strings are
/// empty and charge nothing, so only the list of rows meets the bound, or
/// one type or one name of 4 MiB, pass a bound of 1 MiB while the tree is
/// built; at 64 MiB each builds, charged while it lives.
#[test]
fn a_cases_tree_is_charged_to_its_meter() {
    use unityrs_core::LimitKind;
    for case in [
        tree_case(100_000, "", ""),
        tree_case(2, &"T".repeat(4 << 20), "t"),
        tree_case(2, "E", &"n".repeat(4 << 20)),
    ] {
        let meter = Meter::for_input(&bounded(1 << 20), unbounded(), Reservation::Input);
        let refused = case_tree(&case, &bounded(1 << 20), &meter).map(|_| ());
        assert!(
            matches!(&refused, Err(TreeError::Charge(ChargeError::Limit(l))) if l.kind == LimitKind::InputMemory),
            "{refused:?}"
        );
        assert_eq!(meter.used(), 0, "a refused tree holds nothing");
        let meter = Meter::for_input(&bounded(64 << 20), unbounded(), Reservation::Input);
        let tree = case_tree(&case, &bounded(64 << 20), &meter).unwrap();
        assert!(meter.used() > 1 << 20, "{}", meter.used());
        drop(tree);
        assert_eq!(meter.used(), 0);
        // Through the protocol: the case's `other`, the limit's text its note.
        let o = run_case_on(&case, &bounded(1 << 20), &unbounded());
        assert_eq!(o.kind, Some("other"), "{o:?}");
        assert!(
            o.note.as_deref().unwrap().contains("limit InputMemory: "),
            "{o:?}"
        );
    }
    // A budget that refuses the tree makes it the case's `crash`.
    let budget: Arc<dyn MemoryBudget> = Arc::new(Counting::new(1 << 20));
    let o = run_case_on(
        &tree_case(2, &"T".repeat(4 << 20), "t"),
        &Limits::default(),
        &budget,
    );
    assert_eq!(o.kind, Some(CRASH), "{o:?}");
}

/// The re-saved bytes as hex, each byte in its place, past the buffer on the
/// stack the hex goes through (512 bytes at a time) and at its edges.
#[test]
fn a_long_resave_is_written_whole() {
    for len in [511_usize, 512, 513, 1024, 3000] {
        let body: Vec<u8> = (0..len)
            .map(|i| u8::try_from((i * 7) % 251).unwrap())
            .collect();
        let mut data = i32::try_from(len).unwrap().to_le_bytes().to_vec();
        data.extend_from_slice(&body);
        let want: String = data
            .iter()
            .flat_map(|b| [b >> 4, b & 15])
            .map(|digit| char::from_digit(u32::from(digit), 16).unwrap())
            .collect();
        let case = Case {
            name: "long".into(),
            big_endian: false,
            nodes: vec![
                (0, "Base".into(), "Base".into(), 0),
                (1, "vector".into(), "v".into(), 0),
                (2, "Array".into(), "Array".into(), 0),
                (3, "int".into(), "size".into(), 0),
                (3, "UInt8".into(), "data".into(), 0),
            ],
            data,
        };
        let mut out = Vec::new();
        run_cases(&[case], &Limits::default(), &unbounded(), &mut out).unwrap();
        let rec = &lines(&out)[0];
        assert_eq!(rec["resave"], "equal", "{len}");
        assert_eq!(rec["resave_hex"], Json::String(want), "{len}");
    }
}

/// Each line is flushed as its case ends, and nothing is held back to the
/// end of the run: a process killed from outside on a later case (the
/// container's memory cap, the OOM killer) has answered the earlier ones.
#[test]
fn each_line_is_flushed_as_its_case_ends() {
    #[derive(Default)]
    struct Flushes {
        written: Vec<u8>,
        at_flush: Vec<usize>,
    }
    impl Write for Flushes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.at_flush.push(self.written.len());
            Ok(())
        }
    }
    let mut out = Flushes::default();
    run_cases(
        &[small(), wide_names(4, 8), small()],
        &Limits::default(),
        &unbounded(),
        &mut out,
    )
    .unwrap();
    let ends: Vec<usize> = out
        .written
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .map(|(at, _)| at + 1)
        .collect();
    assert_eq!(ends.len(), 3);
    assert_eq!(out.at_flush, ends);
}
