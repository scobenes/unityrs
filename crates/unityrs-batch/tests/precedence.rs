//! Documents that break two rules: the first by section 10's order is
//! named (`docs/BATCH_SPEC.md` section 10).

mod common;

use common::{refused, refused_text, remove, store, with};
use serde_json::{Value, json};

/// A spec's text with its members in the order written.
fn doc(members: &[&str]) -> String {
    format!("{{{}}}", members.join(","))
}

const REST: &[&str] = &[
    r#""attempt":1"#,
    r#""threads":1"#,
    r#""download_concurrency":1"#,
    r#""max_inflight_uncompressed_bytes":67108864"#,
    r#""max_decoded_bytes":1048576"#,
    r#""dep_cache_bytes":1048576"#,
    r#""inputs":[{"id":"a","path":"/a"}]"#,
    r#""outputs":{"textures":{"key":"k"}}"#,
    r#""sink":{"kind":"directory","root":"/o"}"#,
];

fn spec_with(first: &[&str], last: &[&str]) -> String {
    let mut m: Vec<&str> = first.to_vec();
    m.extend_from_slice(REST);
    m.extend_from_slice(last);
    doc(&m)
}

#[test]
fn the_text_is_valid_as_built() {
    let ok = spec_with(
        &[r#""spec_version":1"#, r#""run_id":"r""#],
        &[r#""spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0}"#],
    );
    common::parse_text(&ok).expect("valid");
}

#[test]
fn spec_version_comes_before_any_other_value() {
    let text = spec_with(
        &[r#""bogus":1"#, r#""run_id":"r""#],
        &[
            r#""spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0}"#,
            r#""spec_version":7"#,
        ],
    );
    assert_eq!(refused_text(&text), "version /spec_version");
}

#[test]
fn values_are_checked_in_document_order() {
    // An unknown field before a type error.
    let text = spec_with(
        &[r#""spec_version":1"#, r#""zzz":1"#, r#""run_id":5"#],
        &[r#""spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0}"#],
    );
    assert_eq!(refused_text(&text), "unknown_field /zzz");
    // A type error before an unknown field.
    let text = spec_with(
        &[r#""spec_version":1"#, r#""run_id":5"#, r#""zzz":1"#],
        &[r#""spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0}"#],
    );
    assert_eq!(refused_text(&text), "type /run_id");
}

#[test]
fn an_absent_member_is_at_its_object_s_end() {
    // `spool` lacks `dir`: its missing member comes at `spool`'s end, before
    // the later `run_id`'s type error.
    let text = spec_with(
        &[
            r#""spec_version":1"#,
            r#""spool":{"size_bytes":0,"reserve_bytes":0}"#,
        ],
        &[r#""run_id":5"#],
    );
    // `size_bytes` 0 is out of range before `dir` is missed.
    assert_eq!(refused_text(&text), "range /spool/size_bytes");
    let text = spec_with(
        &[
            r#""spec_version":1"#,
            r#""spool":{"size_bytes":1048576,"reserve_bytes":0}"#,
        ],
        &[r#""run_id":5"#],
    );
    assert_eq!(refused_text(&text), "missing /spool/dir");
    // A top-level member missed comes after every value.
    let text = doc(&[r#""spec_version":1"#, r#""threads":0"#]);
    assert_eq!(refused_text(&text), "range /threads");
}

#[test]
fn one_value_s_rules_go_in_order() {
    // Too long and of the wrong form: `range` first.
    assert_eq!(
        refused(&with(|v| v["run_id"] = json!("/".repeat(65)))),
        "range /run_id"
    );
    // A header both refused and of the wrong form: `format` first.
    assert_eq!(
        refused(&with(
            |v| v["dependencies"][0]["headers"] = json!({"Host": "a\rb"})
        )),
        "format /dependencies/0/headers/Host"
    );
    // A duplicate member before its unknown name's refusal: the first
    // occurrence is unknown already.
    let text = r#"{"spec_version":1,"x":1,"x":2}"#;
    assert_eq!(refused_text(text), "unknown_field /x");
}

#[test]
fn per_value_rules_come_before_cross_field_rules() {
    let v = with(|v| {
        v["max_decoded_bytes"] = json!(1 << 30); // budgets
        v["threads"] = json!(0); // range
    });
    assert_eq!(refused(&v), "range /threads");
    let v = with(|v| {
        remove(&mut v["inputs"][0], "path"); // source
        v["zzz"] = json!(1);
    });
    assert_eq!(refused(&v), "unknown_field /zzz");
}

#[test]
fn cross_field_rules_go_in_section_10_s_order() {
    // deps before budgets? No: budgets first.
    let v = with(|v| {
        v["inputs"][0]["deps"] = json!(["nope"]);
        v["max_decoded_bytes"] = json!(1 << 30);
    });
    assert_eq!(refused(&v), "budgets /max_decoded_bytes");
    // view_size before source.
    let v = with(|v| {
        v["max_view_bundles"] = json!(1);
        v["dependencies"][0]["path"] = json!("/d");
    });
    assert_eq!(refused(&v), "view_size /inputs/0");
    // deadline_required before prefer_empty and outputs_empty.
    let v = with(|v| {
        v["sink"] = store();
        v["outputs"] = json!({});
        v["prefer"] = json!({"k": {}});
    });
    assert_eq!(refused(&v), "deadline_required /deadline");
    // archive_kind before source, source before headers_without_url.
    let v = with(|v| {
        v["inputs"][0]["headers"] = json!({"X": "y"});
        v["dependencies"][0]["path"] = json!("/d");
        v["archives"] = json!([{"key": "a", "members": [
            {"kind": "texture", "path": "p"}, {"kind": "texture", "path": "q"}]}]);
    });
    assert_eq!(refused(&v), "archive_kind /archives/0/members/1/kind");
    let v = with(|v| {
        v["inputs"][0]["headers"] = json!({"X": "y"});
        v["dependencies"][0]["path"] = json!("/d");
    });
    assert_eq!(refused(&v), "source /dependencies/0");
    // duplicate_id before deps.
    let v = with(|v| {
        v["dependencies"][0]["id"] = json!("a");
        v["inputs"][0]["deps"] = json!(["zz"]);
    });
    assert_eq!(refused(&v), "duplicate_id /inputs/0/id");
    // spool_in_sink before unknown_input.
    let v = with(|v| {
        v["spool"]["dir"] = json!("/out/s");
        v["prefer"] = json!({"k": {"input": "zz"}});
    });
    assert_eq!(refused(&v), "spool_in_sink /spool/dir");
    // outputs_empty before prefer_empty.
    let v = with(|v| {
        v["outputs"] = json!({});
        v["prefer"] = json!({"k": {}});
    });
    assert_eq!(refused(&v), "outputs_empty /outputs");
}

#[test]
fn of_two_breaks_of_one_rule_the_first_in_document_order_is_named() {
    let text = r#"{"spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":67108864,"max_decoded_bytes":1048576,"dep_cache_bytes":1048576,
        "spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0},
        "dependencies":[{"id":"d","path":"/d","deps":["x"]}],
        "inputs":[{"id":"a","path":"/a","deps":["y"]}],
        "outputs":{"textures":{"key":"k"}},"sink":{"kind":"directory","root":"/o"}}"#;
    assert_eq!(refused_text(text), "deps /dependencies/0/deps");
    let text = text.replace(r#""deps":["x"]"#, r#""deps":[]"#);
    assert_eq!(refused_text(&text), "deps /inputs/0/deps");
}

/// Breaks one cross-field rule of the base spec.
type Break = fn(&mut Value);

/// Each cross-field rule B1 checks, in section 10's order, with a change
/// that breaks it alone and the detail it gives.
const RULES: &[(&str, Break, &str)] = &[
    (
        "budgets",
        |v| v["max_decoded_bytes"] = json!(1 << 30),
        "budgets /max_decoded_bytes",
    ),
    (
        "duplicate_id",
        |v| v["dependencies"][0]["id"] = json!("a"),
        "duplicate_id /inputs/0/id",
    ),
    (
        "duplicate_header",
        |v| v["dependencies"][0]["headers"] = json!({"X": "1", "x": "2"}),
        "duplicate_header /dependencies/0/headers/x",
    ),
    (
        "backoff",
        |v| v["http"] = json!({"backoff_initial_ms": 10, "backoff_max_ms": 5}),
        "backoff /http/backoff_max_ms",
    ),
    (
        "deps",
        |v| v["inputs"][0]["deps"] = json!(["d", "nope"]),
        "deps /inputs/0/deps",
    ),
    (
        "view_size",
        |v| v["max_view_bundles"] = json!(1),
        "view_size /inputs/0",
    ),
    (
        "spool_in_sink",
        |v| v["spool"]["dir"] = json!("/out/s"),
        "spool_in_sink /spool/dir",
    ),
    (
        "path_style",
        |v| {
            v["sink"] = store();
            v["sink"]["bucket"] = json!("B-1");
        },
        "path_style /sink/bucket",
    ),
    (
        "deadline_required",
        |v| {
            v["sink"] = store();
            remove(v, "deadline");
        },
        "deadline_required /deadline",
    ),
    (
        "unknown_input",
        |v| v["prefer"] = json!({"k": {"input": "zz"}}),
        "unknown_input /prefer/k/input",
    ),
    (
        "archive_key",
        |v| {
            let a = json!({"key": "a", "members": [{"kind": "sprite", "path": "p"}]});
            let b = json!({"key": "a/b", "members": [{"kind": "sprite", "path": "p"}]});
            push_archives(v, &[a, b], true);
        },
        "archive_key /archives/1/key",
    ),
    (
        "archive_kind",
        |v| {
            let a = json!({"key": "z", "members": [
                {"kind": "texture", "path": "p"}, {"kind": "texture", "path": "q"}]});
            push_archives(v, &[a], false);
        },
        "archive_kind /archives/0/members/1/kind",
    ),
    (
        "source",
        |v| v["dependencies"][0]["path"] = json!("/d"),
        "source /dependencies/0",
    ),
    (
        "headers_without_url",
        |v| v["inputs"][0]["headers"] = json!({}),
        "headers_without_url /inputs/0/headers",
    ),
    (
        "outputs_empty",
        |v| v["outputs"] = json!({}),
        "outputs_empty /outputs",
    ),
    (
        "prefer_empty",
        |v| v["prefer"] = json!({"e": {}}),
        "prefer_empty /prefer/e",
    ),
];

/// Puts `archives` before (`front`) or after any the spec has, so two
/// breaks of archives keep both.
fn push_archives(v: &mut Value, archives: &[Value], front: bool) {
    if !v["archives"].is_array() {
        v["archives"] = json!([]);
    }
    if let Some(a) = v["archives"].as_array_mut() {
        let at = if front { 0 } else { a.len() };
        a.splice(at..at, archives.iter().cloned());
    }
}

/// Section 10's order, pair by adjacent pair (T1): with both rules broken,
/// the earlier is named. One adjacent pair cannot meet: `spool_in_sink`
/// needs a directory sink, `path_style` an object store. Two pairs stand
/// for it: `view_size` before `path_style`, and `spool_in_sink` before
/// `unknown_input` (`deadline_required` between them needs an object store
/// too).
#[test]
fn each_cross_field_rule_comes_before_the_next() {
    let alone = |f: Break| {
        refused(&with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            f(v);
        }))
    };
    for (name, f, want) in RULES {
        assert_eq!(&alone(*f), want, "{name} alone");
    }
    let both = |a: Break, b: Break| {
        refused(&with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            b(v);
            a(v);
        }))
    };
    let at = |name: &str| RULES.iter().position(|r| r.0 == name).expect(name);
    let unmet = (at("spool_in_sink"), at("path_style"));
    let mut pairs: Vec<(usize, usize)> = (0..RULES.len() - 1)
        .map(|i| (i, i + 1))
        .filter(|&p| p != unmet)
        .collect();
    assert_eq!(pairs.len(), RULES.len() - 2, "one pair cannot meet");
    pairs.push((at("view_size"), at("path_style")));
    pairs.push((at("spool_in_sink"), at("unknown_input")));
    for (a, b) in pairs {
        let (name_a, fa, want) = RULES[a];
        let (name_b, fb, _) = RULES[b];
        assert_eq!(both(fa, fb), want, "{name_a} before {name_b}");
    }
}
