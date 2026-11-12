//! The cases review round 1 named: number tokens, the sink's kind, headers,
//! dates, ids, the array caps, the views' work budget and the parser's
//! depth (`docs/BATCH_SPEC.md` section 1's conventions, 1.1, 1.2, 1.5,
//! section 10).

mod common;

use common::{accepted, base, parse_text, refused, refused_text, store, with};
use serde_json::{Value, json};
use unityrs_batch::spec::{self, fields};

/// The base spec's text with `member` first.
fn first(member: &str) -> String {
    base().to_string().replacen('{', &format!("{{{member},"), 1)
}

/// The base spec's text with `from` replaced by `to`, once.
fn replaced(from: &str, to: &str) -> String {
    let text = base().to_string();
    assert!(text.contains(from), "{from}");
    text.replacen(from, to, 1)
}

// Numbers: the spec's wording, no arbitrary precision (D2).

#[test]
fn a_negative_zero_is_no_integer() {
    assert_eq!(
        refused_text(&first(r#""max_attempt_evictions":-0"#)),
        "type /max_attempt_evictions"
    );
    assert!(parse_text(&first(r#""max_attempt_evictions":0"#)).is_ok());
}

#[test]
fn a_number_past_an_f64_is_syntax_wherever_it_is() {
    assert_eq!(
        refused_text(&replaced(r#""threads":2"#, r#""threads":1e400"#)),
        "syntax"
    );
    let digits = "9".repeat(400);
    assert_eq!(
        refused_text(&replaced(
            r#""threads":2"#,
            &format!(r#""threads":{digits}"#)
        )),
        "syntax"
    );
    // Even under an unknown member, which would otherwise come first.
    assert_eq!(refused_text(&first(r#""zzz":1e400"#)), "syntax");
}

/// An integer field judges the nearest double (N1): past 2^53 - 1 in
/// magnitude is `range`, any other token not an integer's `type`.
#[test]
fn an_integer_field_judges_the_nearest_double() {
    let size = |token: &str| {
        refused_text(&replaced(
            r#""path":"/in/a""#,
            &format!(r#""path":"/in/a","size":{token}"#),
        ))
    };
    // Correctly rounded (`float_roundtrip`, G1): `.1` to `.4` read as
    // 2^53 - 1, not past the bound and not an integer token; `.5`, a tie,
    // reads as 2^53 (ties to even), as `.6` does.
    for sign in ["", "-"] {
        for d in 1..=6 {
            let want = if d < 5 { "type" } else { "range" };
            let t = format!("{sign}9007199254740991.{d}");
            assert_eq!(size(&t), format!("{want} /inputs/0/size"), "{t}");
        }
        let t = format!("{sign}90071992547409914e-1");
        assert_eq!(size(&t), "type /inputs/0/size", "{t}");
        // f64::MAX, and a token that rounds down to it: finite, so `range`;
        // one past the largest double is no number (`syntax`).
        for t in ["1.7976931348623157e308", "1.7976931348623158e308"] {
            assert_eq!(size(&format!("{sign}{t}")), "range /inputs/0/size", "{t}");
        }
        assert_eq!(size(&format!("{sign}1.8e308")), "syntax");
    }
    assert_eq!(size("9007199254740992"), "range /inputs/0/size");
    assert_eq!(size("1e-400"), "type /inputs/0/size");
    assert_eq!(size("-0"), "type /inputs/0/size");
    assert_eq!(size("2.0"), "type /inputs/0/size");
    assert!(
        parse_text(&replaced(
            r#""path":"/in/a""#,
            r#""path":"/in/a","size":9007199254740991"#
        ))
        .is_ok()
    );
}

#[test]
fn past_2_to_the_53_by_magnitude_is_range() {
    let v = with(|v| v["inputs"][0]["size"] = json!(-1e300));
    assert_eq!(refused(&v), "range /inputs/0/size");
    let v = with(|v| v["inputs"][0]["size"] = json!(-9_007_199_254_740_993_i64));
    assert_eq!(refused(&v), "range /inputs/0/size");
}

#[test]
fn spec_version_by_value() {
    let version = |token: &str| {
        refused_text(&replaced(
            r#""spec_version":1"#,
            &format!(r#""spec_version":{token}"#),
        ))
    };
    // By token (N1): an integer token that fits in 64 bits is a version.
    assert_eq!(version("2"), "version /spec_version");
    assert_eq!(version("-3"), "version /spec_version");
    assert_eq!(version("18446744073709551615"), "version /spec_version");
    // Every other number is `type`, whatever the double it reads as.
    assert_eq!(version("123456789012345678901"), "type /spec_version");
    assert_eq!(version("-0"), "type /spec_version");
    assert_eq!(version("2.0"), "type /spec_version");
    assert_eq!(version("2.0000000000000000001"), "type /spec_version");
    assert_eq!(version("1e-400"), "type /spec_version");
    assert_eq!(version("1e300"), "type /spec_version");
    assert_eq!(version("1.0"), "type /spec_version");
    assert_eq!(version("1e0"), "type /spec_version");
    assert_eq!(version("1.5"), "type /spec_version");
    assert_eq!(version("1e400"), "syntax");
}

// The sink (C1; T2).

fn sink(s: &str) -> String {
    refused_text(&replaced(
        r#""sink":{"kind":"directory","root":"/out"}"#,
        &format!(r#""sink":{s}"#),
    ))
}

#[test]
fn the_sink_s_table_is_its_first_kind_s() {
    assert_eq!(
        sink(r#"{"bucket":1,"kind":null,"kind":"directory","root":"/out"}"#),
        "type /sink/bucket"
    );
    assert_eq!(
        sink(r#"{"root":"/out","kind":"directory","kind":"object_store"}"#),
        "duplicate_member /sink/kind"
    );
    // A store's member before a later `kind` that names a directory.
    assert_eq!(
        sink(r#"{"bucket":"bkt","kind":"directory","root":"/out"}"#),
        "unknown_field /sink/bucket"
    );
}

#[test]
fn a_sink_with_no_kind_or_an_unknown_one() {
    assert_eq!(sink(r#"{"root":"rel"}"#), "format /sink/root");
    assert_eq!(
        sink(r#"{"bogus":1,"kind":"s3"}"#),
        "unknown_field /sink/bogus"
    );
    assert_eq!(
        sink(r#"{"bucket":"bkt","root":"/o"}"#),
        "missing /sink/kind"
    );
    assert_eq!(sink(r#"{"kind":"s3"}"#), "range /sink/kind");
    assert_eq!(sink(r#"{"kind":7}"#), "type /sink/kind");
    assert_eq!(
        sink(r#"{"kind":"directory","root":"/o","root":"/p"}"#),
        "duplicate_member /sink/root"
    );
    assert_eq!(
        sink(r#"{"kind":"directory","root":["/o"]}"#),
        "type /sink/root"
    );
}

// Headers (C2; T2).

#[test]
fn headers_on_a_path_bundle_even_empty() {
    let v = with(|v| v["inputs"][0]["headers"] = json!({}));
    assert_eq!(refused(&v), "headers_without_url /inputs/0/headers");
    let v = with(|v| v["dependencies"][0]["headers"] = json!({}));
    assert!(accepted(&v).spec.bundle(1).is_some_and(|b| b.has_headers()));
}

#[test]
fn headers_own_rules_come_before_their_members() {
    let mut text = String::from(r#"{"Host":"h""#);
    for i in 0..32 {
        text.push_str(r#","X-"#);
        text.push_str(&i.to_string());
        text.push_str(r#"":"v""#);
    }
    text.push('}');
    let doc = replaced(
        r#""url":"https://example.net/d""#,
        &format!(r#""url":"https://example.net/d","headers":{text}"#),
    );
    assert_eq!(refused_text(&doc), "range /dependencies/0/headers");
    // 32 with `Host` first: the refused name.
    let doc = doc.replacen(r#","X-31":"v""#, "", 1);
    assert_eq!(
        refused_text(&doc),
        "refused_header /dependencies/0/headers/Host"
    );
    // The total, kept after a member's error (round 2's T2): the members
    // after it are read for their lengths alone, escaped ones too.
    // Five names (15 bytes) and four values of 16,381 bytes, one escaped:
    // over 64 KiB by 4.
    let big = "v".repeat(16 * 1024 - 3);
    let doc = replaced(
        r#""url":"https://example.net/d""#,
        &format!(
            r#""url":"https://example.net/d","headers":{{"X-0":1,"X-1":"{big}","X-2":"{big}","X-3":"{big}","X-4":"\u0076{big}"}}"#
        ),
    );
    assert_eq!(refused_text(&doc), "range /dependencies/0/headers");
    // Under the total, the member's own error.
    let doc = doc.replacen(&format!(r#","X-4":"\u0076{big}""#), "", 1);
    assert_eq!(refused_text(&doc), "type /dependencies/0/headers/X-0");
}

/// `{}` for a refused name is that name's `refused_header`, before its
/// object's missing `env` (round 2's D2).
#[test]
fn a_refused_name_with_an_empty_object() {
    let doc = replaced(
        r#""url":"https://example.net/d""#,
        r#""url":"https://example.net/d","headers":{"Host":{}}"#,
    );
    assert_eq!(
        refused_text(&doc),
        "refused_header /dependencies/0/headers/Host"
    );
}

#[test]
fn a_header_repeated_exactly_or_by_case() {
    let doc = |h: &str| {
        refused_text(&replaced(
            r#""url":"https://example.net/d""#,
            &format!(r#""url":"https://example.net/d","headers":{h}"#),
        ))
    };
    assert_eq!(
        doc(r#"{"X":"a","X":"b"}"#),
        "duplicate_member /dependencies/0/headers/X"
    );
    assert_eq!(
        doc(r#"{"X":"a","x":"b"}"#),
        "duplicate_header /dependencies/0/headers/x"
    );
}

#[test]
fn an_env_name_s_length_is_range_and_its_form_format() {
    let env = |name: &str| {
        refused(&with(|v| {
            v["dependencies"][0]["headers"] = json!({"X": {"env": name}});
        }))
    };
    assert_eq!(env(""), "range /dependencies/0/headers/X/env");
    assert_eq!(env(&"A".repeat(257)), "range /dependencies/0/headers/X/env");
    assert_eq!(env("9A"), "format /dependencies/0/headers/X/env");
    let v = with(|v| {
        v["dependencies"][0]["headers"] = json!({"X": {"env": "A".repeat(256)}});
    });
    accepted(&v);
    let v = with(|v| v["dependencies"][0]["headers"] = json!({"X": {}}));
    assert_eq!(refused(&v), "missing /dependencies/0/headers/X/env");
}

// Dates (C3).

#[test]
fn a_leap_second_only_at_23_59_60_utc() {
    for ok in [
        "2016-12-31T23:59:60Z",
        "2017-01-01T00:59:60+01:00",
        "2016-12-31T15:59:60.5-08:00",
    ] {
        accepted(&with(|v| v["deadline"] = json!(ok)));
    }
    for bad in [
        "2016-12-31T23:59:60+01:00",
        "2016-12-31T12:00:60Z",
        "2017-01-01T00:00:60Z",
    ] {
        assert_eq!(
            refused(&with(|v| v["deadline"] = json!(bad))),
            "format /deadline",
            "{bad}"
        );
    }
}

// Ids (C6).

#[test]
fn an_id_holds_no_control_character_c1_included() {
    for c in ['\u{0}', '\u{1f}', '\u{7f}', '\u{80}', '\u{85}', '\u{9f}'] {
        let v = with(|v| v["inputs"][0]["id"] = json!(format!("a{c}")));
        assert_eq!(refused(&v), "format /inputs/0/id", "{c:?}");
    }
    accepted(&with(|v| {
        v["inputs"][0]["id"] = json!("a\u{a0}\u{e9}");
    }));
}

// Arrays and their caps (D3; T2).

#[test]
fn an_array_s_count_before_its_items() {
    let v = with(|v| v["archives"] = json!(vec![json!({}); 257]));
    assert_eq!(refused(&v), "range /archives");
    let v = with(|v| v["archives"] = json!(vec![json!({}); 256]));
    assert_eq!(refused(&v), "missing /archives/0/key");
}

fn atlases(classes: Option<Value>, suffixes: Option<Value>) -> Value {
    with(|v| {
        let mut a = json!({"sprite_key": "s", "json_key": "j"});
        if let Some(c) = classes {
            a["script_classes"] = c;
        }
        if let Some(s) = suffixes {
            a["skip_name_suffixes"] = s;
        }
        v["outputs"] = json!({"atlases": a});
    })
}

#[test]
fn script_classes_and_skip_name_suffixes_are_capped() {
    let name = |n: usize| json!("a".repeat(n));
    let list = |n: usize| json!(vec!["A"; n]);
    let at = "/outputs/atlases";
    accepted(&atlases(Some(list(256)), Some(list(256))));
    accepted(&atlases(Some(json!([name(1024)])), Some(json!([]))));
    for (v, want) in [
        (
            atlases(Some(json!([])), None),
            format!("range {at}/script_classes"),
        ),
        (
            atlases(Some(list(257)), None),
            format!("range {at}/script_classes"),
        ),
        (
            atlases(Some(json!([""])), None),
            format!("range {at}/script_classes/0"),
        ),
        (
            atlases(Some(json!([name(1025)])), None),
            format!("range {at}/script_classes/0"),
        ),
        (
            atlases(None, Some(list(257))),
            format!("range {at}/skip_name_suffixes"),
        ),
        (
            atlases(None, Some(json!([""]))),
            format!("range {at}/skip_name_suffixes/0"),
        ),
    ] {
        assert_eq!(refused(&v), want);
    }
    // 257 empty items: the count is the array's own rule.
    assert_eq!(
        refused(&atlases(Some(json!(vec![""; 257])), None)),
        format!("range {at}/script_classes")
    );
}

#[test]
fn an_archive_member_s_bad_kind_and_a_prefer_not_an_object() {
    let v = with(|v| {
        v["archives"] = json!([{"key": "a.zip", "members": [{"kind": "mesh", "path": "p"}]}]);
    });
    assert_eq!(refused(&v), "range /archives/0/members/0/kind");
    let v = with(|v| v["prefer"] = json!({"k": 1}));
    assert_eq!(refused(&v), "type /prefer/k");
    let v = with(|v| v["prefer"] = json!({"k": {"input": "a", "zz": 1}}));
    assert_eq!(refused(&v), "unknown_field /prefer/k/zz");
}

#[test]
fn a_repeated_prefer_key_is_named_before_a_later_entry_s_error() {
    let doc = |p: &str| refused_text(&first(&format!(r#""prefer":{p}"#)));
    assert_eq!(
        doc(r#"{"k":{"input":"a"},"k":{"input":"a"},"m":{"md5":"x"}}"#),
        "duplicate_member /prefer/k"
    );
    // An error in an earlier entry comes first.
    assert_eq!(
        doc(r#"{"m":{"md5":"x"},"k":{"input":"a"},"k":{"input":"a"}}"#),
        "format /prefer/m/md5"
    );
    // The repeat whose own entry fails: the repeat is named (round 2's T2).
    assert_eq!(
        doc(r#"{"k":{"input":"a"},"k":{"md5":"x"}}"#),
        "duplicate_member /prefer/k"
    );
    assert_eq!(
        doc(r#"{"k":{"input":"a"},"k":{"zzz":1}}"#),
        "duplicate_member /prefer/k"
    );
}

#[test]
fn an_unknown_member_s_value_is_never_read_but_for_syntax() {
    assert_eq!(
        refused_text(&first(r#""zzz":[1,"a",{"b":[null]}]"#)),
        "unknown_field /zzz"
    );
    assert_eq!(refused_text(&first(r#""zzz":[1,"#)), "syntax");
}

// The parser's depth (T6): the document's object is the first level, and
// `serde_json` refuses the 128th (its `remaining_depth` reaches 0 there).

#[test]
fn the_depth_limit_is_127_levels() {
    let nested = |levels: usize| {
        first(&format!(
            r#""zzz":{}{}"#,
            "[".repeat(levels),
            "]".repeat(levels)
        ))
    };
    assert_eq!(refused_text(&nested(126)), "unknown_field /zzz");
    assert_eq!(refused_text(&nested(127)), "syntax");
    // After the first error too.
    let after = |levels: usize| {
        replaced(
            r#""threads":2"#,
            &format!(
                r#""threads":0,"zzz":{}{}"#,
                "[".repeat(levels),
                "]".repeat(levels)
            ),
        )
    };
    assert_eq!(refused_text(&after(126)), "range /threads");
    assert_eq!(refused_text(&after(127)), "syntax");
    // A top-level array is the same parser's.
    let top = |levels: usize| format!("{}{}", "[".repeat(levels), "]".repeat(levels));
    assert_eq!(refused_text(&top(127)), "type");
    assert_eq!(refused_text(&top(128)), "syntax");
}

// The views' work budget (D1).

#[test]
fn the_views_budget_is_2_to_the_24_ids() {
    assert_eq!(fields::VIEW_WORK, 1 << 24);
    // a -> d -> e and b -> d: each input's view reads 2 ids, 4 in all.
    let v = with(|v| {
        v["inputs"] = json!([
            {"id": "a", "path": "/a", "deps": ["d"]},
            {"id": "b", "path": "/b", "deps": ["d"]},
        ]);
        v["dependencies"] = json!([
            {"id": "d", "path": "/d", "deps": ["e"]},
            {"id": "e", "path": "/e"},
        ]);
    });
    let text = v.to_string();
    let with_work = |w: u64| match spec::parse_with(text.clone().into_bytes(), w) {
        Ok(_) => "ok".to_owned(),
        Err(e) => e.detail(),
    };
    assert_eq!(with_work(4), "ok");
    assert_eq!(with_work(3), "view_size /inputs/1");
    assert_eq!(with_work(1), "view_size /inputs/0");
    assert!(parse_text(&text).is_ok());
}

#[test]
fn spool_in_sink_compares_by_component() {
    for dir in ["/out/./s", "//out//s", "/out/"] {
        let v = with(|v| v["spool"]["dir"] = json!(dir));
        assert_eq!(refused(&v), "spool_in_sink /spool/dir", "{dir}");
    }
    accepted(&with(|v| v["spool"]["dir"] = json!("/outer")));
}

#[test]
fn an_endpoint_over_1_kib() {
    let host = "a.".repeat(520);
    let v = with(|v| {
        v["deadline"] = json!("2026-01-01T00:00:00Z");
        v["sink"] = store();
        v["sink"]["endpoint"] = json!(format!("https://{host}net"));
    });
    assert_eq!(refused(&v), "range /sink/endpoint");
}

// The schema beside the parser (D4): each difference of
// `docs/plans/extract.md` section 3, item 4, is a fixture of its own, and
// the validator refuses what both refuse.

/// A spec with an object store sink, its endpoint `endpoint`.
fn with_endpoint(endpoint: &str) -> Value {
    with(|v| {
        v["deadline"] = json!("2026-01-01T00:00:00Z");
        v["sink"] = store();
        v["sink"]["endpoint"] = json!(endpoint);
    })
}

#[test]
#[allow(clippy::too_many_lines)]
fn the_validator_refuses_and_differs_only_as_item_4_lists() {
    use common::{Difference, difference, schema_valid};

    assert!(schema_valid(&base()));
    // Refused by both (the agreement cannot pass by a validator that
    // accepts everything).
    let host = "a".repeat(253);
    for v in [
        with(|v| v["threads"] = json!(0)),
        with(|v| v["run_id"] = json!("..")),
        with(|v| v["prefer"] = json!({"k": 1})),
        with(|v| v["sink"] = json!({"kind": "s3"})),
        with(|v| v["inputs"][0]["id"] = json!("a\u{85}")),
        with(|v| v["zzz"] = json!(1)),
        with(|v| v["spec_version"] = json!(2.0)),
        // A URL's ASCII form and its cap as written are the schema's too.
        with(|v| v["dependencies"][0]["url"] = json!("https://\u{e9}.net/")),
        with(|v| {
            v["dependencies"][0]["url"] = json!(format!("https://e.net/{}", "a".repeat(16 * 1024)));
        }),
        with_endpoint(&format!("https://e.net/{}", "a".repeat(1024))),
    ] {
        assert!(!schema_valid(&v), "{v}");
        assert!(spec::parse(v.to_string().into_bytes()).is_err(), "{v}");
    }
    // Accepted by the validator, refused by the parser, as text (a token
    // such as `-0` does not survive a `Value`).
    let t = |v: Value| v.to_string();
    let cases: [(String, Difference); 13] = [
        // Integer tokens (N1): the parser judges the token, the validator
        // the value.
        (
            t(with(|v| v["threads"] = json!(2.0))),
            Difference::IntegerToken,
        ),
        (
            replaced(
                r#""path":"/in/a""#,
                r#""path":"/in/a","size":9007199254740991.1"#,
            ),
            Difference::IntegerToken,
        ),
        (
            first(r#""max_attempt_evictions":-0"#),
            Difference::IntegerToken,
        ),
        (
            first(r#""max_attempt_evictions":1e-400"#),
            Difference::IntegerToken,
        ),
        // Formats are annotations: a date's form and the leap second.
        (
            t(with(|v| v["deadline"] = json!("tomorrow"))),
            Difference::Format,
        ),
        (
            t(with(|v| v["deadline"] = json!("2016-12-31T23:59:60+01:00"))),
            Difference::Format,
        ),
        // A URL's scheme, its host as written, an endpoint's path, query
        // and fragment.
        (
            t(with(|v| {
                v["dependencies"][0]["url"] = json!("ftp://e.net/");
            })),
            Difference::Format,
        ),
        (
            t(with(|v| {
                v["dependencies"][0]["url"] = json!(format!("https://{host}a/"));
            })),
            Difference::Format,
        ),
        (
            t(with_endpoint("https://s3.example.net/x")),
            Difference::Format,
        ),
        (
            t(with_endpoint("https://s3.example.net/?q")),
            Difference::Format,
        ),
        (
            t(with_endpoint("https://s3.example.net/#f")),
            Difference::Format,
        ),
        // `maxLength` counts code points, the parser bytes.
        (
            t(with(|v| v["inputs"][0]["id"] = json!("\u{e9}".repeat(600)))),
            Difference::ByteLength,
        ),
        (
            t(with(|v| {
                let big: serde_json::Map<String, Value> = (0..5)
                    .map(|i| (format!("X-{i}"), json!("v".repeat(16 * 1024 - 3))))
                    .collect();
                v["dependencies"][0]["headers"] = json!(big);
            })),
            Difference::HeaderTotal,
        ),
    ];
    for (text, want) in cases {
        let v: Value = serde_json::from_str(&text).expect("JSON");
        let e = spec::parse(text.clone().into_bytes()).expect_err("refused");
        assert_eq!(difference(&v, &e), Some(want), "{e}");
        assert!(schema_valid(&v), "{want:?}: the schema accepts {text:.200}");
    }
    // Names compared case-insensitively: a cross-field rule.
    let v = with(|v| v["dependencies"][0]["headers"] = json!({"X": "1", "x": "2"}));
    assert_eq!(refused(&v), "duplicate_header /dependencies/0/headers/x");
    assert!(schema_valid(&v));
    // A lone surrogate escape is JSON (RFC 8259 section 8.2) that no
    // UTF-8 string holds: `syntax`. A validator that reads JSON into
    // UTF-16 strings accepts it; `serde_json`, and so this one, cannot.
    assert_eq!(
        refused_text(&replaced(r#""run_id":"r1""#, r#""run_id":"\ud800""#)),
        "syntax"
    );
}

/// ECMA-262's `.` stops at a line terminator, so the schema's prefix
/// pattern spells "any character" `[\s\S]`; both readers take these.
#[test]
fn a_prefix_may_hold_a_line_terminator() {
    for prefix in ["a\n/", "a\r/", "a\u{2028}/", "a\u{2029}/", "\n/"] {
        accepted(&with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            v["sink"] = store();
            v["sink"]["prefix"] = json!(prefix);
        }));
    }
    let v = with(|v| {
        v["deadline"] = json!("2026-01-01T00:00:00Z");
        v["sink"] = store();
        v["sink"]["prefix"] = json!("a/\n");
    });
    assert_eq!(refused(&v), "format /sink/prefix");
}

/// A `url` or `endpoint` as written (U1): ASCII (`format`), then its cap
/// and its host's (`range`), all before it is parsed; an escaped string is
/// judged by the string it holds.
#[test]
fn a_url_is_checked_as_written_before_it_is_parsed() {
    let url = |u: &str| {
        refused_text(&replaced(
            r#""url":"https://example.net/d""#,
            &format!(r#""url":"{u}""#),
        ))
    };
    assert_eq!(url("https://e.net/\u{e9}"), "format /dependencies/0/url");
    // A JSON escape, so the parser hands the string over unescaped (T1).
    assert_eq!(url(r"https://e.net/\u00e9"), "format /dependencies/0/url");
    assert_eq!(url("https://\u{4e2d}.net/"), "format /dependencies/0/url");
    // Escaped slashes: the string holds `/`, 16 KiB + 1 of it.
    let long = format!(r"https:\/\/e.net\/{}", "a".repeat(16 * 1024 - 13));
    assert_eq!(url(&long), "range /dependencies/0/url");
    let fits = format!(r"https:\/\/e.net\/{}", "a".repeat(16 * 1024 - 14));
    assert!(
        parse_text(&replaced(
            r#""url":"https://example.net/d""#,
            &format!(r#""url":"{fits}""#),
        ))
        .is_ok()
    );
    let host = "a".repeat(253);
    assert_eq!(
        url(&format!("https://{host}a/")),
        "range /dependencies/0/url"
    );
    assert_eq!(
        url(&format!("https://{host}A/")),
        "range /dependencies/0/url"
    );
    assert!(
        parse_text(&replaced(
            r#""url":"https://example.net/d""#,
            &format!(r#""url":"https://{host}/""#),
        ))
        .is_ok()
    );
    // The form comes before the serialized cap (G2): userinfo, its spaces
    // percent-encoded past 16 KiB.
    let spaces = " ".repeat(5462);
    assert_eq!(
        url(&format!("https://u@e.net/{spaces}x")),
        "format /dependencies/0/url"
    );
    // ASCII comes first: a long non-ASCII url is `format`.
    let wide = format!("https://e.net/{}\u{e9}", "a".repeat(16 * 1024));
    assert_eq!(url(&wide), "format /dependencies/0/url");
    // The endpoint, gathered with its sink, escaped or not.
    let endpoint = |e: &str| {
        let v = with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            v["sink"] = store();
            v["sink"]["endpoint"] = json!("E");
        });
        refused_text(&v.to_string().replacen(r#""E""#, &format!(r#""{e}""#), 1))
    };
    assert_eq!(endpoint("https://s3.\u{e9}.net"), "format /sink/endpoint");
    // A JSON escape, so the endpoint reaches the sink's stand-in (T1).
    assert_eq!(endpoint(r"https://s3.\u00e9.net"), "format /sink/endpoint");
    // A path, then a serialization past 1 KiB: the form first (G2).
    let spaces = " ".repeat(340);
    assert_eq!(
        endpoint(&format!("https://e.net/{spaces}x")),
        "format /sink/endpoint"
    );
    assert_eq!(
        endpoint(&format!(r"https:\/\/e.net\/{spaces}x")),
        "format /sink/endpoint"
    );
    let long = format!(r"https:\/\/{}", "a".repeat(1024));
    assert_eq!(endpoint(&long), "range /sink/endpoint");
    let host = "a".repeat(254);
    assert_eq!(endpoint(&format!("https://{host}")), "range /sink/endpoint");
    assert_eq!(
        endpoint(&format!(r"https:\/\/{host}")),
        "range /sink/endpoint"
    );
}
