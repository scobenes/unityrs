//! The cross-field rules `template` and `key(<rule>)` (`docs/BATCH_SPEC.md`
//! 1.1, 2.1, 2.3), each with its exact `<rule> <pointer>`: one test per
//! rule and per place a template or a literal key stands, and the document
//! order that picks one of two breaks of a rule. Their place among the
//! other rules is `precedence.rs`'s.

mod common;

use common::{accepted, refused, refused_text, store, with};
use serde_json::{Value, json};
use unityrs_batch::reason::KeyRule;
use unityrs_batch::spec::{BadSpec, Rule, push_token};

/// `key(<rule>) <pointer>` for a literal key of `prefer`, cut to fit as
/// every detail is.
fn prefer_detail(rule: &str, key: &str) -> String {
    let rule = KeyRule::ALL
        .into_iter()
        .find(|r| r.name() == rule)
        .expect("a rule");
    let mut pointer = "/prefer".to_owned();
    push_token(&mut pointer, key);
    BadSpec {
        rule: Rule::Key(rule),
        pointer,
    }
    .detail()
}

fn deadline(v: &mut Value) {
    v["deadline"] = json!("2026-01-01T00:00:00Z");
}

/// The base spec with an object store sink whose prefix is `prefix`.
fn prefixed(prefix: &str, f: impl FnOnce(&mut Value)) -> Value {
    with(|v| {
        deadline(v);
        v["sink"] = store();
        v["sink"]["prefix"] = json!(prefix);
        f(v);
    })
}

#[test]
fn every_template_place_and_kind_is_accepted_when_valid() {
    accepted(&with(|v| {
        v["outputs"] = json!({
            "textures": {"key": "t/{input}/{type}/{name}_{path_id}.png"},
            "sprites": {"key": "s/{safe_input}/{safe_name}.png"},
            "atlases": {"sprite_key": "a/{atlas}/{safe_sprite}.png", "json_key": "a/{safe_atlas}.json"},
        });
        v["archives"] = json!([{"key": "all.zip", "members": [
            {"kind": "texture", "path": "_unityrs/{name}.png"},
            {"kind": "sprite", "path": ".unityrs-tmp-{name}"},
            {"kind": "atlas_sprite", "path": "{atlas}/{sprite}.png"},
            {"kind": "atlas_json", "path": "{atlas}.json"},
        ]}]);
        v["prefer"] = json!({"t/a/Texture2D/x_1.png": {"input": "a"}});
    }));
}

/// A template that does not parse for its kind: an unknown variable, one
/// its kind does not allow, a lone brace, an empty template.
#[test]
fn template_a_template_that_does_not_parse() {
    let out = |key: &str, text: &str| {
        refused(&with(|v| {
            v["outputs"] = json!({key: {"key": text}});
        }))
    };
    assert_eq!(out("textures", "{nope}"), "template /outputs/textures/key");
    assert_eq!(out("textures", "{atlas}"), "template /outputs/textures/key");
    assert_eq!(out("textures", "a{name"), "template /outputs/textures/key");
    assert_eq!(out("textures", "a}b"), "template /outputs/textures/key");
    assert_eq!(out("textures", ""), "template /outputs/textures/key");
    assert_eq!(out("sprites", "{sprite}"), "template /outputs/sprites/key");
    let atlases = |sprite_key: &str, json_key: &str| {
        refused(&with(|v| {
            v["outputs"] = json!({"atlases": {"sprite_key": sprite_key, "json_key": json_key}});
        }))
    };
    assert_eq!(
        atlases("{name}", "{atlas}"),
        "template /outputs/atlases/sprite_key"
    );
    assert_eq!(
        atlases("{sprite}", "{sprite}"),
        "template /outputs/atlases/json_key"
    );
}

/// An archive member's path parses for its member's kind.
#[test]
fn template_a_member_path_parses_for_its_kind() {
    for (kind, bad) in [
        ("texture", "{atlas}"),
        ("sprite", "{sprite}"),
        ("atlas_sprite", "{name}"),
        ("atlas_json", "{sprite}"),
    ] {
        let v = with(|v| {
            v["archives"] = json!([
                {"key": "ok.zip", "members": [{"kind": "texture", "path": "{name}"}]},
                {"key": "bad.zip", "members": [
                    {"kind": "atlas_json", "path": "{atlas}"},
                    {"kind": kind, "path": bad},
                ]},
            ]);
        });
        assert_eq!(refused(&v), "template /archives/1/members/1/path", "{kind}");
    }
}

/// What a template's literals fix breaks a key rule in every expansion
/// (2.1's literal checks), one case per rule.
#[test]
fn template_a_literal_part_that_breaks_a_key_rule() {
    let long = "x".repeat(256);
    for text in [
        "/{name}",
        "{name}/",
        "a//{name}",
        "{input}/./{name}",
        "{input}/..",
        &format!("{{input}}/{long}"),
        "_unityrs",
        "_unityrs/{name}",
        "{input}/.unityrs-tmp-x",
    ] {
        let v = with(|v| v["outputs"]["textures"]["key"] = json!(text));
        assert_eq!(refused(&v), "template /outputs/textures/key", "{text}");
    }
    // What only a value can break is each expansion's.
    for text in [
        "{input}/.{name}",
        "_unityrs{name}",
        "{name}\u{1}",
        "{input}/{name}/x",
    ] {
        accepted(&with(|v| v["outputs"]["textures"]["key"] = json!(text)));
    }
    // A member path takes no reserved rule (4.7), in its literals either.
    let member = |path: &str| {
        with(|v| {
            v["archives"] =
                json!([{"key": "z.zip", "members": [{"kind": "texture", "path": path}]}]);
        })
    };
    accepted(&member("_unityrs/{name}"));
    accepted(&member("{input}/.unityrs-tmp-x"));
    assert_eq!(
        refused(&member("{input}//x")),
        "template /archives/0/members/0/path"
    );
}

/// Of two bad templates, the first in document order is named: the
/// outputs' and the archives' by where each stands, and an atlas's two
/// keys by theirs.
#[test]
fn template_the_first_bad_template_in_document_order() {
    let head = r#""spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":67108864,"max_decoded_bytes":1048576,"dep_cache_bytes":1048576,
        "spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0},
        "inputs":[{"id":"a","path":"/a"}],"sink":{"kind":"directory","root":"/o"}"#;
    let outputs = r#""outputs":{"atlases":{"json_key":"{name}","sprite_key":"{name}"}}"#;
    let archives = r#""archives":[{"key":"z","members":[{"path":"{nope}","kind":"texture"}]}]"#;
    let doc = |a: &str, b: &str| format!("{{{head},{a},{b}}}");
    assert_eq!(
        refused_text(&doc(outputs, archives)),
        "template /outputs/atlases/json_key"
    );
    assert_eq!(
        refused_text(&doc(archives, outputs)),
        "template /archives/0/members/0/path"
    );
}

/// A `prefer` key breaking each rule of 2.3 a literal key can break here
/// (`not_utf8` cannot: a JSON string is Unicode), checked as given.
#[test]
fn key_a_prefer_key_breaks_each_rule() {
    let prefer = |key: &str| refused(&with(|v| v["prefer"] = json!({key: {"input": "a"}})));
    let long_segment = "x".repeat(256);
    let long_key = "ab/".repeat(342);
    for (key, rule) in [
        ("", "empty"),
        ("a\u{7f}", "control_char"),
        ("a\tb", "control_char"),
        ("/a", "leading_slash"),
        ("a/", "trailing_slash"),
        ("a//b", "empty_segment"),
        ("a/./b", "dot_segment"),
        ("..", "dot_segment"),
        (&long_segment, "segment_too_long"),
        (&long_key, "too_long"),
        ("_unityrs", "reserved_prefix"),
        ("_unityrs/head.json", "reserved_prefix"),
        ("a/.unityrs-tmp-1", "reserved_name"),
    ] {
        assert_eq!(prefer(key), prefer_detail(rule, key), "{key:?}");
    }
}

/// An archive's key is a literal key too.
#[test]
fn key_an_archive_key() {
    let v = with(|v| {
        v["archives"] = json!([
            {"key": "ok.zip", "members": [{"kind": "texture", "path": "{name}"}]},
            {"key": "a//b.zip", "members": [{"kind": "texture", "path": "{name}"}]},
        ]);
    });
    assert_eq!(refused(&v), "key(empty_segment) /archives/1/key");
}

/// A key is checked again after the sink's prefix (2.3): a key that fits
/// alone and passes `too_long` with it.
#[test]
fn key_a_key_is_checked_after_the_prefix() {
    let prefix = format!("{}/", "p".repeat(255));
    let key = format!("{}/{}", "k".repeat(255), "k".repeat(255));
    accepted(&prefixed(&prefix, |_| {}));
    accepted(&with(|v| {
        v["prefer"] = json!({key.clone(): {"input": "a"}});
    }));
    let long = format!("{key}/{}/{}", "k".repeat(255), "k".repeat(255));
    assert_eq!(long.len(), 1023);
    accepted(&with(|v| {
        v["prefer"] = json!({long.clone(): {"input": "a"}});
    }));
    let v = prefixed(&prefix, |v| {
        v["prefer"] = json!({long.clone(): {"input": "a"}});
    });
    assert_eq!(refused(&v), prefer_detail("too_long", &long));
    // The key's own refusal comes before its prefixed one (`prefer` comes
    // before `sink` in `Value`'s sorted text).
    let v = prefixed("x/", |v| {
        v["prefer"] = json!({"_unityrs/a//b": {"input": "a"}});
    });
    assert_eq!(refused(&v), "key(empty_segment) /prefer/_unityrs~1a~1~1b");
    // A bad prefix is named itself, before the keys it prefixes.
    let v = prefixed("_unityrs/", |v| v["prefer"] = json!({"a": {"input": "a"}}));
    assert_eq!(refused(&v), "key(reserved_prefix) /sink/prefix");
}

/// A non-empty prefix passes the rules without its final `/` (1.1).
#[test]
fn key_the_sink_prefix_without_its_final_slash() {
    accepted(&prefixed("", |_| {}));
    accepted(&prefixed("a/b/", |_| {}));
    for (prefix, rule) in [
        ("/", "empty"),
        ("a//", "trailing_slash"),
        ("/a/", "leading_slash"),
        ("a//b/", "empty_segment"),
        ("./", "dot_segment"),
        ("_unityrs/", "reserved_prefix"),
        ("a/.unityrs-tmp-x/", "reserved_name"),
    ] {
        assert_eq!(
            refused(&prefixed(prefix, |_| {})),
            format!("key({rule}) /sink/prefix"),
            "{prefix}"
        );
    }
}

/// Of two bad literal keys, the first in document order is named: `prefer`'s
/// and the archives' by where each stands, in the document's own text (not
/// a sorted `Value`'s); a bad sink prefix before any of them, wherever it
/// stands, since they are checked with it.
#[test]
fn key_the_first_bad_key_in_document_order() {
    let head = r#""spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":67108864,"max_decoded_bytes":1048576,"dep_cache_bytes":1048576,
        "deadline":"2026-01-01T00:00:00Z",
        "spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0},
        "inputs":[{"id":"a","path":"/a"}],"outputs":{"textures":{"key":"{name}"}}"#;
    let prefer = r#""prefer":{"ok":{"input":"a"},"x/":{"input":"a"}}"#;
    let archives = r#""archives":[{"key":"/z","members":[{"kind":"texture","path":"{name}"}]}]"#;
    let sink = r#""sink":{"kind":"object_store","bucket":"b-1","region":"r","prefix":"./"}"#;
    let ok_sink = r#""sink":{"kind":"object_store","bucket":"b-1","region":"r"}"#;
    let doc = |m: &[&str]| format!("{{{head},{}}}", m.join(","));
    assert_eq!(
        refused_text(&doc(&[prefer, archives, ok_sink])),
        "key(trailing_slash) /prefer/x~1"
    );
    assert_eq!(
        refused_text(&doc(&[archives, prefer, ok_sink])),
        "key(leading_slash) /archives/0/key"
    );
    assert_eq!(
        refused_text(&doc(&[sink, archives, prefer])),
        "key(dot_segment) /sink/prefix"
    );
    // With the prefix after them, the prefix is still named first.
    assert_eq!(
        refused_text(&doc(&[prefer, sink])),
        "key(dot_segment) /sink/prefix"
    );
    assert_eq!(
        refused_text(&doc(&[archives, prefer, sink])),
        "key(dot_segment) /sink/prefix"
    );
    // Within `prefer`, the first bad key in the text: `z/` before `a/`,
    // and the reverse.
    let za = r#""prefer":{"z/":{"input":"a"},"a/":{"input":"a"}}"#;
    let az = r#""prefer":{"a/":{"input":"a"},"z/":{"input":"a"}}"#;
    assert_eq!(
        refused_text(&doc(&[za, ok_sink])),
        "key(trailing_slash) /prefer/z~1"
    );
    assert_eq!(
        refused_text(&doc(&[az, ok_sink])),
        "key(trailing_slash) /prefer/a~1"
    );
}
