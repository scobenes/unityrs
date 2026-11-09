use super::*;

fn args(s: &[&str]) -> Vec<Vec<u8>> {
    s.iter().map(|a| a.as_bytes().to_vec()).collect()
}

fn dump(s: &[&str]) -> DumpArgs {
    match parse(&args(s)) {
        Ok(Parsed::Dump(a)) => *a,
        other => panic!("{s:?}: {other:?}"),
    }
}

fn error(s: &[&str]) -> String {
    match parse(&args(s)) {
        Err(Usage(m)) => m,
        other => panic!("{s:?}: {other:?}"),
    }
}

#[test]
fn defaults_are_the_oracles() {
    let a = dump(&["corpus", "-o", "out.jsonl.gz"]);
    assert_eq!(a.inputs, [PathBuf::from("corpus")]);
    assert_eq!(a.output, PathBuf::from("out.jsonl.gz"));
    assert_eq!(a.kind, INDEX_KINDS.map(|k| k.as_bytes().to_vec()));
    assert_eq!(a.build, None);
    assert_eq!(a.levels, Levels::ALL);
    assert_eq!(a.jobs, 1);
    assert!(!a.index && !a.quiet && !a.allow_index_mismatch && !a.lazy_reverse);
    assert_eq!(a.unbounded, Vec::<LimitKind>::new());
    assert!(dump(&["c", "-o", "o", "--rev"]).lazy_reverse);
    assert_eq!(
        (a.sample, a.limit, a.stats, a.tpk),
        (None, None, None, None)
    );
    assert_eq!(a.name, Vec::<Vec<u8>>::new());
}

/// Each case's outcome is Python 3.11.17's argparse's for the oracle's
/// parser (recorded by hand from a probe of `unitypy_dump.py`'s option
/// set; `tools/fixtures` holds no argparse fixture).
#[test]
fn values_are_taken_as_argparse_takes_them() {
    assert_eq!(dump(&["x", "-o=a"]).output, PathBuf::from("a"));
    assert_eq!(dump(&["x", "-oq"]).output, PathBuf::from("q"));
    assert_eq!(dump(&["x", "-o", "-"]).output, PathBuf::from("-"));
    assert_eq!(dump(&["x", "-o", "-1"]).output, PathBuf::from("-1"));
    assert_eq!(dump(&["x", "-o", "y", "-o y"]).output, PathBuf::from(" y"));
    assert_eq!(dump(&["x", "-o", "y", "--out="]).output, PathBuf::from(""));
    assert_eq!(dump(&["x", "-o", "y", "-qj4"]).jobs, 4);
    assert_eq!(dump(&["x", "-o", "y", "-qj", "4"]).jobs, 4);
    assert_eq!(dump(&["x", "-o", "y", "-qj=4"]).jobs, 4);
    assert_eq!(
        dump(&["x", "-o", "y", "--sample", " +1_0 "]).sample,
        Some(10)
    );
    assert_eq!(
        dump(&["x", "-o", "y", "--name", "-a b"]).name,
        [b"-a b".to_vec()]
    );
    assert_eq!(dump(&["-x y", "-o", "y"]).inputs, [PathBuf::from("-x y")]);
    let a = dump(&[
        "--index",
        "--out=o",
        "--kind",
        "bundle",
        "--build=4100,100056",
        "--name",
        "a*",
        "--name=-b*",
        "--sample",
        "-1",
        "--lim",
        "-2",
        "--levels",
        "L0,L1",
        "--allow",
        "idx.json",
        "--stats",
        "s.json",
        "--qu",
    ]);
    assert!(a.index && a.quiet && a.allow_index_mismatch);
    assert_eq!(a.output, PathBuf::from("o"));
    assert_eq!(a.kind, [b"bundle".to_vec()]);
    assert_eq!(a.build, Some(vec![b"4100".to_vec(), b"100056".to_vec()]));
    assert_eq!(a.name, [b"a*".to_vec(), b"-b*".to_vec()]);
    assert_eq!((a.sample, a.limit), (Some(-1), Some(-2)));
    assert_eq!(a.levels.names(), ["L0", "L1"]);
    assert_eq!(a.inputs, [PathBuf::from("idx.json")]);
    assert_eq!(a.stats, Some(PathBuf::from("s.json")));
    // The last of a repeated option wins; an empty --build is none.
    let a = dump(&["x", "-oa", "-o", "b", "-j", "2", "--jobs", "-3", "--build="]);
    assert_eq!((a.output, a.jobs, a.build), (PathBuf::from("b"), -3, None));
    // So is an empty --stats, the last one given included: the oracle
    // writes the dump and no stats, and exits 0 (review round 16; pinned on
    // its side by test_dump_cli.py's
    // test_an_empty_stats_path_writes_no_stats).
    for empty in [
        &["--stats", ""][..],
        &["--stats="],
        &["--stats", "s", "--stats="],
    ] {
        let a = dump(&[&["x", "-o", "y"][..], empty].concat());
        assert_eq!(a.stats, None, "{empty:?}");
    }
    // `--` ends the options; past i64 a count saturates.
    let a = dump(&[
        "-o",
        "y",
        "--limit",
        "99999999999999999999",
        "--",
        "-q",
        "--",
        "x",
    ]);
    assert_eq!(
        a.inputs,
        [PathBuf::from("-q"), PathBuf::from("--"), PathBuf::from("x")]
    );
    assert!(!a.quiet);
    assert_eq!(a.limit, Some(i64::MAX));
    // Several inputs, wherever they stand (the extension).
    assert_eq!(
        dump(&["a", "-o", "y", "b"]).inputs,
        [PathBuf::from("a"), PathBuf::from("b")]
    );
    // Bytes that are not UTF-8 reach the run unchanged.
    let raw = parse(&[
        b"in\xff".to_vec(),
        b"-o".to_vec(),
        b"y".to_vec(),
        b"--name".to_vec(),
        b"*\xfe".to_vec(),
    ]);
    let Ok(Parsed::Dump(a)) = raw else {
        panic!("{raw:?}")
    };
    assert_eq!(a.name, [b"*\xfe".to_vec()]);
    assert_eq!(a.inputs, [path(b"in\xff")]);
}

/// Review round 7: a `--kind` or `--build` that is not UTF-8 was a usage
/// error (status 2), with or without `--index`, where the oracle keeps the
/// surrogate-escaped `str`, splits it and dumps. The bytes reach the run.
#[test]
fn kinds_and_builds_that_are_not_utf8_reach_the_run() {
    for index in [false, true] {
        let mut argv = vec![
            b"in".to_vec(),
            b"-o".to_vec(),
            b"y".to_vec(),
            b"--kind".to_vec(),
            b"bundle,\xff".to_vec(),
            b"--build".to_vec(),
            b"4100,\xfe\xff,".to_vec(),
        ];
        if index {
            argv.push(b"--index".to_vec());
        }
        let parsed = parse(&argv);
        let Ok(Parsed::Dump(a)) = parsed else {
            panic!("{parsed:?}")
        };
        assert_eq!(a.kind, [b"bundle".to_vec(), b"\xff".to_vec()]);
        assert_eq!(
            a.build,
            Some(vec![b"4100".to_vec(), b"\xfe\xff".to_vec(), Vec::new()])
        );
    }
}

#[test]
fn bad_arguments_are_refused_as_argparse_refuses_them() {
    let req = "the following arguments are required: ";
    assert_eq!(error(&[]), format!("{req}input, -o/--output"));
    assert_eq!(error(&["x"]), format!("{req}-o/--output"));
    assert_eq!(error(&["-o", "y"]), format!("{req}input"));
    assert_eq!(error(&["--bogus", "x"]), format!("{req}-o/--output"));
    assert_eq!(
        error(&["x", "-o"]),
        "argument -o/--output: expected one argument"
    );
    assert_eq!(
        error(&["x", "-o", "--index"]),
        "argument -o/--output: expected one argument"
    );
    assert_eq!(
        error(&["x", "-o", "--", "y"]),
        "argument -o/--output: expected one argument"
    );
    assert_eq!(
        error(&["x", "-o", "y", "--bogus", "z"]),
        "unrecognized arguments: --bogus"
    );
    assert_eq!(
        error(&["x", "-o", "y", "-qzz"]),
        "unrecognized arguments: -zz"
    );
    assert_eq!(
        error(&["x", "-o", "y", "--l", "1"]),
        "ambiguous option: --l could match --limit, --levels"
    );
    // Ambiguity is found before anything is taken, `-h` included.
    assert_eq!(
        error(&["-h", "--l"]),
        "ambiguous option: --l could match --limit, --levels"
    );
    assert!(
        error(&["x", "-o", "y", "--=a"])
            .starts_with("ambiguous option: --=a could match --help, --output,")
    );
    assert_eq!(
        error(&["x", "-o", "y", "-q=z"]),
        "argument -q/--quiet: ignored explicit argument 'z'"
    );
    assert_eq!(
        error(&["x", "-o", "y", "-q-j"]),
        "argument -q/--quiet: ignored explicit argument '-j'"
    );
    assert_eq!(
        error(&["x", "-o", "y", "--index="]),
        "argument --index: ignored explicit argument ''"
    );
    assert_eq!(
        error(&["x", "-o", "y", "--sample", "1.5"]),
        "argument --sample: invalid int value: '1.5'"
    );
    assert_eq!(
        error(&["x", "-o", "y", "--sample", "-1.5"]),
        "argument --sample: invalid int value: '-1.5'"
    );
    assert_eq!(
        error(&["x", "-o", "y", "--sample", "it's"]),
        "argument --sample: invalid int value: \"it's\""
    );
    assert_eq!(
        error(&["x", "-o", "y", "-j", "1__0"]),
        "argument -j/--jobs: invalid int value: '1__0'"
    );
    // An int converts as it is taken, so before a later -h.
    assert_eq!(
        error(&["x", "-o", "y", "--sample", "x", "-h"]),
        "argument --sample: invalid int value: 'x'"
    );
    assert_eq!(
        parse(&args(&["x", "-h", "--sample", "x"])),
        Ok(Parsed::Help)
    );
    assert_eq!(parse(&args(&["-hz"])), Ok(Parsed::Help));
    assert_eq!(
        error(&["x", "-o", "y", "--allow-index-mismatch"]),
        "--allow-index-mismatch needs --index"
    );
    assert_eq!(
        error(&["x", "y", "-o", "z", "--index"]),
        "--index takes one INPUT, the corpus index.json"
    );
    assert!(error(&["x", "-o", "y", "--levels", "L1,L2"]).contains("holds L0"));
    assert!(error(&["x", "-o", "y", "--levels", "L0,L3"]).contains("L3 needs L2"));
    assert!(error(&["x", "-o", "y", "--levels", "L0,L4"]).contains("'L4'"));
}

#[test]
fn unbounded_takes_a_comma_list_of_limits() {
    use LimitKind as K;
    let a = dump(&[
        "c",
        "-o",
        "o",
        "--unbounded",
        "ZeroSizeElements,TreeDepth,TreeDepth",
    ]);
    assert_eq!(a.unbounded, [K::TreeDepth, K::ZeroSizeElements]);
    assert_eq!(
        dump(&["c", "-o", "o", "--unb=Inflate"]).unbounded,
        [K::Inflate]
    );
    let all = LIFTABLE.map(|k| format!("{k:?}")).join(",");
    assert_eq!(
        dump(&["c", "-o", "o", "--unbounded", &all]).unbounded,
        LIFTABLE
    );
    for bad in ["Count", "treedepth", "", "TreeDepth,"] {
        let e = error(&["c", "-o", "o", "--unbounded", bad]);
        assert!(
            e.starts_with("argument --unbounded: invalid limit: "),
            "{bad:?}: {e}"
        );
    }
    // No abbreviation the oracle takes changed meaning.
    for o in STRINGS
        .map(|(o, _)| o)
        .iter()
        .filter(|o| o.starts_with("--"))
    {
        for n in 3..o.len() {
            let matches = STRINGS
                .iter()
                .filter(|(s, _)| {
                    s.starts_with(&o[..n]) && !matches!(*s, "--unbounded" | "--reverse-prepass")
                })
                .count();
            let with = STRINGS
                .iter()
                .filter(|(s, _)| s.starts_with(&o[..n]))
                .count();
            if !matches!(*o, "--unbounded" | "--reverse-prepass") {
                assert_eq!(matches, with, "{}", &o[..n]);
            }
        }
    }
}

/// Each outcome is Python 3.11's `int()`'s, recorded from a probe of it
/// (`PARITY.md`'s `ShaderProgram` row states the same rules, which
/// `test_rows_exports.py` pins in the oracle image).
#[test]
fn numbers_parse_as_python_int() {
    let ones = |n: usize| "1".repeat(n);
    for (s, v) in [
        ("0".to_owned(), Some(0)),
        ("-0".into(), Some(0)),
        (" 7\n".into(), Some(7)),
        ("\x0b7\x0c".into(), Some(7)),
        ("\r7\r".into(), Some(7)),
        ("+3".into(), Some(3)),
        ("1_000".into(), Some(1000)),
        ("-12".into(), Some(-12)),
        ("-99999999999999999999".into(), Some(-i64::MAX)),
        // Review round 16: the decimal digits of every script, and the
        // whitespace past ASCII, which `int` maps to ASCII first.
        ("\u{663}".into(), Some(3)),
        ("-\u{663}".into(), Some(-3)),
        ("\u{661}_\u{662}".into(), Some(12)),
        ("\u{661}1\u{662}".into(), Some(112)),
        ("\u{ff11}".into(), Some(1)),
        ("\u{1d7ce}".into(), Some(0)),
        ("\u{3000}-\u{663}\u{3000}".into(), Some(-3)),
        ("\u{2028}7\u{2029}".into(), Some(7)),
        // At most 4,300 digits, underscores not counted.
        (ones(4300), Some(i64::MAX)),
        (format!("-{}", ones(4300)), Some(-i64::MAX)),
        (format!(" {} ", ones(4300)), Some(i64::MAX)),
        (vec!["1"; 4300].join("_"), Some(i64::MAX)),
        ("\u{661}".repeat(4300), Some(i64::MAX)),
    ] {
        assert_eq!(py_int(s.as_bytes()), v, "{s:?}");
    }
    for s in [
        "", " ", "_1", "1_", "1__0", "0x10", "1e3", "--1", "+-1", "1 2", "- 1", "+_1", "+",
        "1\x7f", // Not whitespace to `int`, though `str.isspace()` takes them.
        "\x1c1", "1\x1f", // Numeric, but no decimal digit.
        "\u{b2}", "\u{2167}", "\u{f33}",
    ] {
        assert_eq!(py_int(s.as_bytes()), None, "{s:?}");
    }
    for s in [
        ones(4301),
        "0".repeat(4301),
        vec!["1"; 4301].join("_"),
        format!("+{}", ones(4301)),
        format!("-{}", ones(4301)),
        format!(" -{} ", ones(4301)),
        "\u{661}".repeat(4301),
    ] {
        assert_eq!(py_int(s.as_bytes()), None, "{} characters", s.len());
    }
    assert_eq!(py_int(b"1\xff"), None);
    // The whitespace `int` takes around a number: these, and no other
    // character that is not a digit (Python 3.11, every code point).
    let spaces: Vec<char> =
        "\t\n\x0b\x0c\r \u{85}\u{a0}\u{1680}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}"
            .chars()
            .chain('\u{2000}'..='\u{200a}')
            .collect();
    for c in ('\0'..=char::MAX).filter(|c| nd_digit(*c).is_none()) {
        let padded = format!("{c}1{c}");
        assert_eq!(
            py_int(padded.as_bytes()).is_some(),
            spaces.contains(&c),
            "U+{:04X}",
            u32::from(c)
        );
    }
}

/// Each outcome is Python 3.11's `_negative_number_matcher`'s, whose `\d`
/// takes the decimal digits of every script.
#[test]
fn negative_numbers_are_values() {
    for s in [
        "-1",
        "-12",
        "-.5",
        "-1.5",
        "-1\n",
        "-\u{663}",
        "-\u{663}.\u{665}",
        "-.\u{665}",
        "-\u{ff11}",
        "-\u{663}\n",
    ] {
        assert!(negative_number(s.as_bytes()), "{s:?}");
    }
    for s in [
        "-", "-.", "-1.", "--1", "-a", "1", "-1e3", "-1\n\n", "-\u{b2}",
    ] {
        assert!(!negative_number(s.as_bytes()), "{s:?}");
    }
    assert!(!negative_number(b"-1\xff"));
    // So `--limit -٣` is the value -3, as the oracle takes it (review
    // round 16's sweep: it was "expected one argument").
    assert_eq!(
        dump(&["x", "-o", "y", "--limit", "-\u{663}"]).limit,
        Some(-3)
    );
}

#[test]
fn repr_quotes_as_python() {
    assert_eq!(py_repr(b"a"), "'a'");
    assert_eq!(py_repr(b"it's"), "\"it's\"");
    assert_eq!(py_repr(b"'\""), "'\\'\"'");
    assert_eq!(py_repr(b"a\\b\n\x01\x7f"), "'a\\\\b\\n\\x01\\x7f'");
    assert_eq!(py_repr(b"\xffz"), "'\\udcffz'");
}
