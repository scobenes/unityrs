//! One test per `bad_spec` rule B1 implements, each asserting the exact
//! `<rule> <pointer>` (`docs/BATCH_SPEC.md` section 10), the document's
//! own rules aside (`job_file.rs`).

mod common;

use common::{accepted, base, refused, refused_text, remove, store, with};
use serde_json::json;
use unityrs_batch::spec::model::{
    Fencing, HeaderValue, MemberKind, Prefer, Redirects, RetryOn, Sink, TryClass,
};

#[test]
fn the_base_spec_is_valid_with_its_defaults() {
    let v = accepted(&base());
    let s = &v.spec;
    assert_eq!(s.run_id, "r1");
    assert_eq!(s.deadline, None);
    assert_eq!(s.shutdown_grace_ms, 20_000);
    assert!(!s.stop_on_stdin_eof);
    assert_eq!(s.parent_pid, None);
    assert_eq!(s.max_view_bundles, 1024);
    assert_eq!(s.max_attempt_evictions, 1024);
    assert_eq!(s.max_export_work, 2 * 16_777_216);
    assert_eq!(s.http.idle_timeout_ms, 30_000);
    assert_eq!(s.http.min_rate_bytes, 1024);
    assert_eq!(s.http.connect_timeout_ms, 10_000);
    assert_eq!(s.http.attempts, 3);
    assert_eq!(s.http.backoff_initial_ms, 500);
    assert_eq!(s.http.backoff_max_ms, 30_000);
    assert_eq!(s.http.retry_on, RetryOn::ALL);
    assert_eq!(s.http.redirects, Redirects::SameOrigin);
    assert_eq!(s.prefer_count(), 0);
    assert!(s.publish_incomplete);
    assert!(!s.resume);
    assert_eq!(s.archives, Vec::new());
    assert_eq!(s.manifest_fencing, Fencing::Conditional);
    assert_eq!(
        s.sink,
        Sink::Directory {
            root: "/out".into()
        }
    );
    assert_eq!(v.graph.len(), 2);
    assert_eq!(v.graph.deps(0), [1]);
}

#[test]
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn every_field_is_read() {
    let v = accepted(&with(|v| {
        v["deadline"] = json!("2026-11-11T12:00:00.25+01:00");
        v["shutdown_grace_ms"] = json!(10_000);
        v["stop_on_stdin_eof"] = json!(true);
        v["parent_pid"] = json!(4_194_304);
        v["max_view_bundles"] = json!(2);
        v["max_attempt_evictions"] = json!(0);
        v["max_export_work"] = json!(1_048_576);
        v["http"] = json!({
            "idle_timeout_ms": 1000, "min_rate_bytes": 1, "connect_timeout_ms": 100,
            "attempts": 10, "backoff_initial_ms": 0, "backoff_max_ms": 0,
            "retry_on": ["timeout", "status_429"], "redirects": "none",
        });
        v["inputs"][0]["size"] = json!(0);
        v["dependencies"][0]["headers"] = json!({"Authorization": {"env": "TOKEN"}, "X-A": "b"});
        v["dependencies"][0]["size"] = json!(9_007_199_254_740_991_u64);
        v["dependencies"][0]["sha256"] = json!("0".repeat(64));
        v["outputs"] = json!({
            "sprites": {"key": "s"},
            "atlases": {
                "sprite_key": "a", "json_key": "j",
                "script_classes": ["UIAtlas", "NGUIAtlas"], "skip_name_suffixes": ["Ref"],
            },
        });
        v["prefer"] = json!({"k": {"input": "a"}, "m": {"md5": "f".repeat(32), "input": null}});
        v["publish_incomplete"] = json!(false);
        v["resume"] = json!(true);
        v["sink"] = json!({
            "kind": "object_store", "bucket": "b-1", "region": "eu-west-1",
            "endpoint": "http://[::1]:9000", "path_style": true, "prefix": "p/",
            "timeout_ms": 1000, "attempts": 10,
        });
        v["archives"] =
            json!([{"key": "x.zip", "members": [{"kind": "sprite", "path": "{name}"}]}]);
        v["manifest_fencing"] = json!("none");
    }));
    let s = &v.spec;
    assert_eq!(
        (s.spec_version, s.run_id.as_str(), s.attempt, s.threads),
        (1, "r1", 1, 2)
    );
    assert_eq!(
        (
            s.download_concurrency,
            s.max_inflight_uncompressed_bytes,
            s.max_decoded_bytes,
            s.dep_cache_bytes
        ),
        (2, 268_435_456, 16_777_216, 33_554_432)
    );
    let d = s.deadline.as_ref().expect("a deadline");
    assert_eq!(d.text, "2026-11-11T12:00:00.25+01:00");
    assert_eq!(
        (
            d.at.year,
            d.at.month,
            d.at.day,
            d.at.hour,
            d.at.nanos,
            d.at.offset_minutes
        ),
        (2026, 11, 11, 12, 250_000_000, 60)
    );
    assert_eq!(
        (s.shutdown_grace_ms, s.stop_on_stdin_eof, s.parent_pid),
        (10_000, true, Some(4_194_304))
    );
    assert_eq!(
        (
            s.max_view_bundles,
            s.max_attempt_evictions,
            s.max_export_work
        ),
        (2, 0, 1_048_576)
    );
    assert_eq!(
        (
            s.spool.dir.as_str(),
            s.spool.size_bytes,
            s.spool.reserve_bytes
        ),
        ("/spool", 1_073_741_824, 0)
    );
    let h = &s.http;
    assert_eq!(
        (
            h.idle_timeout_ms,
            h.min_rate_bytes,
            h.connect_timeout_ms,
            h.attempts,
            h.backoff_initial_ms,
            h.backoff_max_ms
        ),
        (1000, 1, 100, 10, 0, 0)
    );
    assert_eq!(
        h.retry_on,
        RetryOn::NONE
            .with(TryClass::Timeout)
            .with(TryClass::Status429)
    );
    assert_eq!(h.redirects, Redirects::None);
    // The bundles, `dependencies` first in this document's order.
    assert!(!s.inputs_first());
    let a = s.bundle(0).expect("the input");
    assert_eq!(
        (
            a.id(),
            a.path(),
            a.url(),
            a.size(),
            a.sha256(),
            a.has_headers()
        ),
        ("a", Some("/in/a"), None, Some(0), None, false)
    );
    let dep = s.bundle(1).expect("the dependency");
    assert_eq!(
        (dep.id(), dep.path(), dep.url(), dep.size()),
        (
            "d",
            None,
            Some("https://example.net/d"),
            Some(9_007_199_254_740_991)
        )
    );
    assert_eq!(dep.sha256(), Some("0".repeat(64).as_str()));
    assert_eq!(
        dep.headers().collect::<Vec<_>>(),
        [
            ("Authorization", HeaderValue::Env("TOKEN")),
            ("X-A", HeaderValue::Literal("b"))
        ]
    );
    assert_eq!(v.graph.deps(0), [1]);
    assert_eq!(s.outputs.textures, None);
    assert_eq!(s.outputs.sprites.as_deref(), Some("s"));
    let at = s.outputs.atlases.as_ref().expect("atlases");
    assert_eq!((at.sprite_key.as_str(), at.json_key.as_str()), ("a", "j"));
    assert_eq!(at.script_classes, ["UIAtlas", "NGUIAtlas"]);
    assert_eq!(at.skip_name_suffixes, ["Ref"]);
    let prefer: Vec<_> = s.prefer().collect();
    let md5 = "f".repeat(32);
    assert_eq!(
        prefer,
        [
            (
                "k",
                Prefer {
                    input: Some("a"),
                    md5: None
                }
            ),
            (
                "m",
                Prefer {
                    input: None,
                    md5: Some(md5.as_str())
                }
            ),
        ]
    );
    assert_eq!((s.publish_incomplete, s.resume), (false, true));
    let Sink::ObjectStore(o) = &s.sink else {
        panic!("an object store")
    };
    assert_eq!(
        (o.bucket.as_str(), o.region.as_str(), o.prefix.as_str()),
        ("b-1", "eu-west-1", "p/")
    );
    assert_eq!(
        (
            o.endpoint.as_deref(),
            o.path_style,
            o.timeout_ms,
            o.attempts
        ),
        (Some("http://[::1]:9000"), true, 1000, 10)
    );
    assert_eq!(s.archives.len(), 1);
    assert_eq!(s.archives[0].key, "x.zip");
    assert_eq!(
        (
            s.archives[0].members[0].kind,
            s.archives[0].members[0].path.as_str()
        ),
        (MemberKind::Sprite, "{name}")
    );
    assert_eq!(s.manifest_fencing, Fencing::None);
}

// The document itself and `spec_version`.

#[test]
fn type_of_the_document() {
    assert_eq!(refused_text("[]"), "type");
    assert_eq!(refused_text("1"), "type");
}

#[test]
fn spec_version_rules() {
    assert_eq!(
        refused(&with(|v| remove(v, "spec_version"))),
        "missing /spec_version"
    );
    assert_eq!(
        refused(&with(|v| v["spec_version"] = json!("1"))),
        "type /spec_version"
    );
    assert_eq!(
        refused(&with(|v| v["spec_version"] = json!(1.0))),
        "type /spec_version"
    );
    assert_eq!(
        refused(&with(|v| v["spec_version"] = json!(3))),
        "version /spec_version"
    );
    assert_eq!(
        refused(&with(|v| v["spec_version"] = json!(0))),
        "version /spec_version"
    );
    assert_eq!(
        refused(&with(|v| v["spec_version"] = json!(-1))),
        "version /spec_version"
    );
    let twice = base().to_string().replacen('{', r#"{"spec_version":1,"#, 1);
    assert_eq!(refused_text(&twice), "duplicate_member /spec_version");
}

// The per-value rules.

#[test]
fn duplicate_member() {
    let text = base().to_string().replacen('{', r#"{"run_id":"x","#, 1);
    assert_eq!(refused_text(&text), "duplicate_member /run_id");
    let text = base()
        .to_string()
        .replace(r#""dir":"/spool""#, r#""dir":"/spool","dir":"/s2""#);
    assert_eq!(refused_text(&text), "duplicate_member /spool/dir");
    let text = with(|v| v["prefer"] = json!({"k": {"md5": null, "input": "a"}}))
        .to_string()
        .replace(r#""prefer":{"k""#, r#""prefer":{"k":{"input":"a"},"k""#);
    assert_eq!(refused_text(&text), "duplicate_member /prefer/k");
}

#[test]
fn unknown_field() {
    assert_eq!(
        refused(&with(|v| v["threadz"] = json!(1))),
        "unknown_field /threadz"
    );
    assert_eq!(
        refused(&with(|v| v["spool"]["x"] = json!(1))),
        "unknown_field /spool/x"
    );
    assert_eq!(
        refused(&with(|v| v["inputs"][0]["a/b~"] = json!(1))),
        "unknown_field /inputs/0/a~1b~0"
    );
    // A store's field under a directory sink.
    assert_eq!(
        refused(&with(|v| v["sink"]["bucket"] = json!("b"))),
        "unknown_field /sink/bucket"
    );
    assert_eq!(
        refused(&with(
            |v| v["dependencies"][0]["headers"] = json!({"X": {"env": "A", "e": 1}})
        )),
        "unknown_field /dependencies/0/headers/X/e"
    );
}

#[test]
fn missing() {
    assert_eq!(refused(&with(|v| remove(v, "threads"))), "missing /threads");
    assert_eq!(
        refused(&with(|v| remove(&mut v["spool"], "dir"))),
        "missing /spool/dir"
    );
    assert_eq!(
        refused(&with(|v| remove(&mut v["inputs"][0], "id"))),
        "missing /inputs/0/id"
    );
    assert_eq!(
        refused(&with(|v| remove(&mut v["sink"], "kind"))),
        "missing /sink/kind"
    );
    assert_eq!(
        refused(&with(|v| remove(&mut v["sink"], "root"))),
        "missing /sink/root"
    );
    assert_eq!(
        refused(&with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            v["sink"] = json!({"kind": "object_store", "region": "r"});
        })),
        "missing /sink/bucket"
    );
    assert_eq!(
        refused(&with(
            |v| v["outputs"] = json!({"atlases": {"sprite_key": "a"}})
        )),
        "missing /outputs/atlases/json_key"
    );
}

#[test]
fn type_() {
    assert_eq!(
        refused(&with(|v| v["threads"] = json!("2"))),
        "type /threads"
    );
    assert_eq!(
        refused(&with(|v| v["threads"] = json!(2.5))),
        "type /threads"
    );
    assert_eq!(
        refused(&with(|v| v["threads"] = json!(2.0))),
        "type /threads"
    );
    assert_eq!(
        refused(&with(|v| v["threads"] = json!(null))),
        "type /threads"
    );
    assert_eq!(
        refused(&with(|v| v["deadline"] = json!(5))),
        "type /deadline"
    );
    assert_eq!(refused(&with(|v| v["inputs"] = json!({}))), "type /inputs");
    assert_eq!(
        refused(&with(|v| v["inputs"][0]["deps"][0] = json!(1))),
        "type /inputs/0/deps/0"
    );
    assert_eq!(refused(&with(|v| v["sink"] = json!([]))), "type /sink");
    assert_eq!(
        refused(&with(|v| v["dependencies"][0]["headers"] = json!({"X": 1}))),
        "type /dependencies/0/headers/X"
    );
}

#[test]
fn range() {
    let r = |f: fn(&mut serde_json::Value)| refused(&with(f));
    assert_eq!(r(|v| v["threads"] = json!(0)), "range /threads");
    assert_eq!(r(|v| v["threads"] = json!(1025)), "range /threads");
    assert_eq!(r(|v| v["threads"] = json!(-1)), "range /threads");
    // Past 2^53 - 1 whatever the token.
    assert_eq!(
        r(|v| v["inputs"][0]["size"] = json!(9_007_199_254_740_992_u64)),
        "range /inputs/0/size"
    );
    assert_eq!(
        r(|v| v["inputs"][0]["size"] = json!(1e300)),
        "range /inputs/0/size"
    );
    let huge = base().to_string().replace(
        r#""threads":2"#,
        r#""threads":100000000000000000000000000000"#,
    );
    assert_eq!(refused_text(&huge), "range /threads");
    assert_eq!(r(|v| v["run_id"] = json!("")), "range /run_id");
    assert_eq!(r(|v| v["run_id"] = json!("a".repeat(65))), "range /run_id");
    assert_eq!(r(|v| v["inputs"] = json!([])), "range /inputs");
    assert_eq!(
        r(|v| v["manifest_fencing"] = json!("x")),
        "range /manifest_fencing"
    );
    assert_eq!(
        r(|v| v["http"] = json!({"retry_on": ["bogus"]})),
        "range /http/retry_on/0"
    );
    assert_eq!(
        r(|v| v["http"] = json!({"retry_on": ["connect", "timeout", "connect"]})),
        "range /http/retry_on/2"
    );
    assert_eq!(r(|v| v["sink"]["kind"] = json!("s3")), "range /sink/kind");
    assert_eq!(
        r(|v| v["spool"]["dir"] = json!(format!("/{}", "a".repeat(256)))),
        "range /spool/dir"
    );
    assert_eq!(
        r(|v| v["archives"] = json!([{"key": "k", "members": []}])),
        "range /archives/0/members"
    );
    assert_eq!(
        r(|v| v["inputs"][0]["id"] = json!("")),
        "range /inputs/0/id"
    );
    // Headers: count, byte total, one value's length.
    let many: serde_json::Map<String, serde_json::Value> =
        (0..33).map(|i| (format!("X-{i}"), json!("v"))).collect();
    assert_eq!(
        refused(&with(|v| v["dependencies"][0]["headers"] = json!(many))),
        "range /dependencies/0/headers"
    );
    let big: serde_json::Map<String, serde_json::Value> = (0..5)
        .map(|i| (format!("X-{i}"), json!("v".repeat(16 * 1024 - 3))))
        .collect();
    assert_eq!(
        refused(&with(|v| v["dependencies"][0]["headers"] = json!(big))),
        "range /dependencies/0/headers"
    );
    assert_eq!(
        r(|v| v["dependencies"][0]["headers"] = json!({"X": "v".repeat(16 * 1024 + 1)})),
        "range /dependencies/0/headers/X"
    );
    // A URL's cap is on its serialization.
    assert_eq!(
        r(|v| v["dependencies"][0]["url"] = json!(format!("https://e.net/{}x", " ".repeat(5462)))),
        "range /dependencies/0/url"
    );
}

#[test]
fn format() {
    let r = |f: fn(&mut serde_json::Value)| refused(&with(f));
    assert_eq!(r(|v| v["run_id"] = json!("a/b")), "format /run_id");
    assert_eq!(r(|v| v["run_id"] = json!("..")), "format /run_id");
    assert_eq!(r(|v| v["deadline"] = json!("tomorrow")), "format /deadline");
    assert_eq!(
        r(|v| v["inputs"][0]["path"] = json!("in/a")),
        "format /inputs/0/path"
    );
    assert_eq!(
        r(|v| v["inputs"][0]["id"] = json!("a\nb")),
        "format /inputs/0/id"
    );
    assert_eq!(
        r(|v| v["inputs"][0]["sha256"] = json!("A".repeat(64))),
        "format /inputs/0/sha256"
    );
    assert_eq!(
        r(|v| v["dependencies"][0]["url"] = json!("ftp://e.net/")),
        "format /dependencies/0/url"
    );
    assert_eq!(
        r(|v| v["dependencies"][0]["url"] = json!("https://u:p@e.net/")),
        "format /dependencies/0/url"
    );
    assert_eq!(
        r(|v| v["dependencies"][0]["headers"] = json!({"X Y": "v"})),
        "format /dependencies/0/headers/X Y"
    );
    assert_eq!(
        r(|v| v["dependencies"][0]["headers"] = json!({"X": "a\rb"})),
        "format /dependencies/0/headers/X"
    );
    assert_eq!(
        r(|v| v["dependencies"][0]["headers"] = json!({"X": {"env": "A=B"}})),
        "format /dependencies/0/headers/X/env"
    );
    assert_eq!(
        r(|v| v["prefer"] = json!({"k": {"md5": "x"}})),
        "format /prefer/k/md5"
    );
    let sink = |s: serde_json::Value| {
        refused(&with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            v["sink"] = s;
        }))
    };
    let mut s = store();
    s["prefix"] = json!("p");
    assert_eq!(sink(s), "format /sink/prefix");
    let mut s = store();
    s["bucket"] = json!("b/c");
    assert_eq!(sink(s), "format /sink/bucket");
    let mut s = store();
    s["endpoint"] = json!("https://s3.example.net/path");
    assert_eq!(sink(s), "format /sink/endpoint");
    assert_eq!(r(|v| v["sink"]["root"] = json!("out")), "format /sink/root");
}

#[test]
fn refused_header() {
    for name in [
        "Host",
        "content-length",
        "TRANSFER-ENCODING",
        "Connection",
        "range",
        "Accept-Encoding",
    ] {
        let v = with(|v| v["dependencies"][0]["headers"] = json!({name: "x"}));
        assert_eq!(
            refused(&v),
            format!("refused_header /dependencies/0/headers/{name}")
        );
    }
}

// The cross-field rules, in section 10's order.

#[test]
fn budgets() {
    assert_eq!(
        refused(&with(|v| v["max_decoded_bytes"] = json!(33_554_433))),
        "budgets /max_decoded_bytes"
    );
    assert_eq!(
        refused(&with(|v| {
            v["dep_cache_bytes"] = json!(60 << 20);
            v["max_inflight_uncompressed_bytes"] = json!(64 << 20);
        })),
        "budgets /max_decoded_bytes"
    );
    let v = with(|v| {
        v["max_decoded_bytes"] = json!(33_554_432);
        v["max_inflight_uncompressed_bytes"] = json!(67_108_864);
    });
    accepted(&v);
}

#[test]
fn duplicate_id() {
    let v = with(|v| {
        v["dependencies"] = json!([{"id": "d", "path": "/d"}, {"id": "a", "path": "/a2"}]);
    });
    // `dependencies` sorts first in the text `Value` writes, so `a` there
    // is the first occurrence and the input's the second.
    assert_eq!(refused(&v), "duplicate_id /inputs/0/id");
    let text = r#"{"spec_version":1,"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":67108864,"max_decoded_bytes":1048576,"dep_cache_bytes":1048576,
        "spool":{"dir":"/s","size_bytes":1048576,"reserve_bytes":0},
        "inputs":[{"id":"a","path":"/a"},{"id":"b","path":"/b"}],
        "dependencies":[{"id":"b","path":"/b2"},{"id":"a","path":"/a2"}],
        "outputs":{"textures":{"key":"k"}},"sink":{"kind":"directory","root":"/o"}}"#;
    assert_eq!(refused_text(text), "duplicate_id /dependencies/0/id");
}

#[test]
fn duplicate_header() {
    let v = with(|v| v["dependencies"][0]["headers"] = json!({"X-A": "1", "x-a": {"env": "B"}}));
    assert_eq!(refused(&v), "duplicate_header /dependencies/0/headers/x-a");
}

#[test]
fn backoff() {
    assert_eq!(
        refused(&with(|v| v["http"] = json!({"backoff_initial_ms": 40_000}))),
        "backoff /http/backoff_max_ms"
    );
    assert_eq!(
        refused(&with(|v| v["http"] = json!({"backoff_max_ms": 499}))),
        "backoff /http/backoff_max_ms"
    );
}

#[test]
fn deps() {
    assert_eq!(
        refused(&with(|v| v["inputs"][0]["deps"] = json!(["zz"]))),
        "deps /inputs/0/deps"
    );
    assert_eq!(
        refused(&with(|v| v["inputs"][0]["deps"] = json!(["d", "d"]))),
        "deps /inputs/0/deps"
    );
    assert_eq!(
        refused(&with(|v| {
            v["max_view_bundles"] = json!(1);
            v["inputs"][0]["deps"] = json!(["d", "a"]);
        })),
        "deps /inputs/0/deps"
    );
    // A list may name its own bundle.
    accepted(&with(|v| v["inputs"][0]["deps"] = json!(["a", "d"])));
}

#[test]
fn view_size() {
    let v = with(|v| {
        v["max_view_bundles"] = json!(2);
        v["inputs"] = json!([
            {"id": "a", "path": "/a", "deps": ["d"]},
            {"id": "b", "path": "/b", "deps": ["e"]},
        ]);
        v["dependencies"] = json!([
            {"id": "d", "path": "/d"},
            {"id": "e", "path": "/e", "deps": ["f"]},
            {"id": "f", "path": "/f"},
        ]);
    });
    assert_eq!(refused(&v), "view_size /inputs/1");
}

#[test]
fn spool_in_sink() {
    for (spool, root) in [
        ("/out/spool", "/out"),
        ("/out", "/out/"),
        ("/data", "/"),
        ("/a/b", "/a/b/c"),
    ] {
        let v = with(|v| {
            v["spool"]["dir"] = json!(spool);
            v["sink"]["root"] = json!(root);
        });
        assert_eq!(refused(&v), "spool_in_sink /spool/dir", "{spool} {root}");
    }
    accepted(&with(|v| v["spool"]["dir"] = json!("/outx")));
}

#[test]
fn path_style() {
    let s = |sink: serde_json::Value| {
        with(|v| {
            v["deadline"] = json!("2026-01-01T00:00:00Z");
            v["sink"] = sink;
        })
    };
    let mut st = store();
    st["endpoint"] = json!("http://10.0.0.1:9000");
    assert_eq!(refused(&s(st.clone())), "path_style /sink/endpoint");
    st["bucket"] = json!("Upper");
    assert_eq!(refused(&s(st.clone())), "path_style /sink/endpoint");
    st["path_style"] = json!(true);
    accepted(&s(st));
    let mut st = store();
    st["bucket"] = json!("a_b");
    assert_eq!(refused(&s(st)), "path_style /sink/bucket");
}

#[test]
fn deadline_required() {
    assert_eq!(
        refused(&with(|v| v["sink"] = store())),
        "deadline_required /deadline"
    );
    assert_eq!(
        refused(&with(|v| {
            v["sink"] = store();
            v["deadline"] = json!(null);
        })),
        "deadline_required /deadline"
    );
}

#[test]
fn unknown_input() {
    assert_eq!(
        refused(&with(|v| v["prefer"] = json!({"k/1": {"input": "d"}}))),
        "unknown_input /prefer/k~11/input"
    );
}

#[test]
fn archive_key() {
    let a = |keys: &[&str]| {
        let archives: Vec<serde_json::Value> = keys
            .iter()
            .map(|k| json!({"key": k, "members": [{"kind": "texture", "path": "p"}]}))
            .collect();
        refused(&with(|v| v["archives"] = json!(archives)))
    };
    assert_eq!(
        a(&["x.zip", "y.zip", "x.zip"]),
        "archive_key /archives/2/key"
    );
    assert_eq!(a(&["a/b.zip", "a"]), "archive_key /archives/1/key");
    assert_eq!(a(&["a", "a/b.zip"]), "archive_key /archives/1/key");
    accepted(&with(|v| {
        v["archives"] = json!([
            {"key": "a", "members": [{"kind": "texture", "path": "p"}]},
            {"key": "ab.zip", "members": [{"kind": "texture", "path": "p"}]},
        ]);
    }));
}

#[test]
fn archive_kind() {
    let v = with(|v| {
        v["archives"] = json!([
            {"key": "a.zip", "members": [{"kind": "texture", "path": "p"}]},
            {"key": "b.zip", "members": [
                {"kind": "sprite", "path": "p"},
                {"kind": "texture", "path": "q"},
                {"kind": "sprite", "path": "r"},
            ]},
        ]);
    });
    assert_eq!(refused(&v), "archive_kind /archives/1/members/2/kind");
}

#[test]
fn source() {
    assert_eq!(
        refused(&with(|v| v["inputs"][0]["url"] = json!("https://e.net/a"))),
        "source /inputs/0"
    );
    assert_eq!(
        refused(&with(|v| remove(&mut v["dependencies"][0], "url"))),
        "source /dependencies/0"
    );
}

#[test]
fn headers_without_url() {
    assert_eq!(
        refused(&with(|v| v["inputs"][0]["headers"] = json!({"X": "y"}))),
        "headers_without_url /inputs/0/headers"
    );
}

#[test]
fn outputs_empty() {
    assert_eq!(
        refused(&with(|v| v["outputs"] = json!({}))),
        "outputs_empty /outputs"
    );
}

#[test]
fn prefer_empty() {
    assert_eq!(
        refused(&with(
            |v| v["prefer"] = json!({"k": {"input": "a"}, "j": {"input": null}})
        )),
        "prefer_empty /prefer/j"
    );
    assert_eq!(
        refused(&with(|v| v["prefer"] = json!({"k": {}}))),
        "prefer_empty /prefer/k"
    );
}
