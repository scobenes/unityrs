//! Phase 1's cost on the largest legal specs (`docs/plans/batch.md` 5.3's
//! `C_spec` and `p`, 2.3 rule 4's walk): the time `spec::load` takes, the
//! heap it peaks at above its start (the bytes, the tree and the typed
//! spec, `p`), and the heap the validated spec keeps (`C_spec`), on the
//! test's thread through a counting allocator, in glibc chunks
//! (`heap_cost`). Ignored: run it with
//!
//! ```text
//! cargo test --release -p unityrs-batch --test measure -- --ignored --nocapture
//! ```
//!
//! Three shapes, each within 64 MiB:
//! - `wide`: 100,000 inputs, each with a URL, a digest, a size and a deps
//!   list of shared dependencies, filled to the cap;
//! - `nodes`: the most JSON values, 65,536 bundles whose ids are one to
//!   three bytes and whose `deps` lists fill the cap;
//! - `walk`: the slowest view check, inputs whose views each reach one
//!   strongly connected set of dependencies whose lists fill the cap, so
//!   each input's walk reads every list: as many inputs as the views'
//!   2^24-id budget accepts (`docs/BATCH_SPEC.md` 1.1), and the 100,000
//!   the spec allows, which the budget refuses (`view_size`).
//!
//! `phase_1_on_the_worst_documents` measures the documents, legal or not,
//! that hold the most for their bytes (review round 1's D3, round 2's U1
//! and D4), and fails when one peaks at 512 MiB or more (round 2's T1).
//! Each prints a `ROW` line per shape; `slowest.rs` measures phase 1's
//! time where it is slowest.

mod measuring;

use std::fmt::Write as _;
use std::sync::atomic::Ordering;
use std::time::Instant;

use measuring::{HEAD, LIVE, Measure, PEAK, fill, row, short_id};
use unityrs_batch::spec::fields::VIEW_WORK;
use unityrs_batch::spec::{self, MAX_SPEC_BYTES};
use unityrs_testkit::counted::{count_this_thread, others_are_not_counted};
use unityrs_testkit::scratch::Scratch;

/// `wide`: `inputs` inputs of `per` deps each among `deps` dependencies.
fn wide(inputs: usize, deps: usize, per: usize) -> String {
    let mut s = String::from(HEAD);
    s.push_str(r#""dependencies":["#);
    for d in 0..deps {
        if d > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            r#"{{"id":"dep/{d:05}","url":"https://cdn.example.net/bundles/dep/{d:05}.bundle","sha256":"{:064x}","size":{}}}"#,
            d,
            1000 + d
        );
    }
    s.push_str(r#"],"inputs":["#);
    for i in 0..inputs {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            r#"{{"id":"input/{i:06}","url":"https://cdn.example.net/bundles/{i:06}.bundle","sha256":"{:064x}","size":{},"deps":["#,
            i,
            1000 + i
        );
        for k in 0..per {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, r#""dep/{:05}""#, (i * 7 + k * 13) % deps);
        }
        s.push_str("]}");
    }
    s.push_str("]}");
    s
}

/// `nodes`: one input and 65,535 dependencies with short ids, each
/// dependency's list `per` ids long.
fn nodes(per: usize) -> String {
    let n = 65_535;
    let mut s = String::from(HEAD);
    let _ = write!(
        s,
        r#""inputs":[{{"id":"input","path":"/i","deps":["{}"]}}],"#,
        short_id(0)
    );
    s.push_str(r#""dependencies":["#);
    for d in 0..n {
        if d > 0 {
            s.push(',');
        }
        let _ = write!(s, r#"{{"id":"{}","path":"/d","deps":["#, short_id(d));
        for k in 0..per {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, r#""{}""#, short_id((d + 1 + k) % n));
        }
        s.push_str("]}");
    }
    s.push_str("]}");
    s
}

/// `walk`: `inputs` inputs, each listing one of `set` dependencies, each
/// of which lists every other.
fn walk(inputs: usize, set: usize) -> String {
    let mut s = String::from(HEAD);
    s.push_str(r#""inputs":["#);
    for i in 0..inputs {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            r#"{{"id":"in/{i}","path":"/i","deps":["{}"]}}"#,
            short_id(i % set)
        );
    }
    s.push_str(r#"],"dependencies":["#);
    for d in 0..set {
        if d > 0 {
            s.push(',');
        }
        let _ = write!(s, r#"{{"id":"{}","path":"/d","deps":["#, short_id(d));
        let mut first = true;
        for k in 0..set {
            if k != d {
                if !first {
                    s.push(',');
                }
                first = false;
                let _ = write!(s, r#""{}""#, short_id(k));
            }
        }
        s.push_str("]}");
    }
    s.push_str("]}");
    s
}

fn measure(dir: &Scratch, name: &str, text: &str) -> Measure {
    assert!(
        text.len() as u64 <= MAX_SPEC_BYTES,
        "{name}: {} bytes",
        text.len()
    );
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, text).unwrap();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let v = spec::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let secs = t.elapsed().as_secs_f64();
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let kept = LIVE.load(Ordering::Relaxed) - base;
    drop(v);
    Measure {
        bytes: text.len(),
        secs,
        peak,
        kept,
    }
}

/// Grows `make(n)` while it fits 64 MiB, by doubling then bisecting.
fn largest(make: impl Fn(usize) -> String, start: usize) -> String {
    let fits = |n: usize| make(n).len() as u64 <= MAX_SPEC_BYTES;
    let (mut lo, mut hi) = (start, start * 2);
    while fits(hi) {
        lo = hi;
        hi *= 2;
    }
    while hi - lo > 1 {
        let mid = usize::midpoint(lo, hi);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    make(lo)
}

#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn phase_1_on_the_largest_specs() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let dir = Scratch::new("batch-measure");

    let text = largest(|per| wide(100_000, 12_000, per), 1);
    let m = measure(&dir, "wide", &text);
    row(
        "wide (100,000 inputs, 12,000 dependencies)",
        VIEW_WORK,
        "accepted",
        &m,
    );
    drop(text);

    let text = largest(nodes, 1);
    let m = measure(&dir, "nodes", &text);
    row(
        "nodes (65,536 bundles, short ids)",
        VIEW_WORK,
        "accepted",
        &m,
    );
    drop(text);

    // The set whose lists fill what 100,000 inputs leave.
    // An input's id in the set is up to two bytes longer than with a set
    // of one.
    let room = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits")
        - walk(100_000, 1).len()
        - 2 * 100_000;
    // The set's lists take about `set^2` ids of up to four bytes.
    let (mut lo, mut hi) = (1, 8192);
    while hi - lo > 1 {
        let mid = usize::midpoint(lo, hi);
        if walk(0, mid).len() <= room {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let set = lo;
    // Each input's view reads its own list and every list of the set.
    let per = (set * (set - 1) + 1) as u64;
    let accepted = usize::try_from(VIEW_WORK / per).expect("fits");
    let text = walk(accepted, set);
    let m = measure(&dir, "walk", &text);
    row(
        &format!("walk ({accepted} inputs, a set of {set}: the budget's most)"),
        VIEW_WORK,
        "accepted",
        &m,
    );
    drop(text);
    let text = walk(100_000, set);
    let (m, outcome) = measure_any(&dir, "walk", &text);
    assert_eq!(outcome, format!("view_size /inputs/{accepted}"));
    row(
        &format!("walk (100,000 inputs, a set of {set})"),
        VIEW_WORK,
        &outcome,
        &m,
    );
}

/// Like [`measure`], for a document refused or accepted.
fn measure_any(dir: &Scratch, name: &str, text: &str) -> (Measure, String) {
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, text).unwrap();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let r = spec::load(&path);
    let secs = t.elapsed().as_secs_f64();
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let kept = LIVE.load(Ordering::Relaxed) - base;
    let outcome = match &r {
        Ok(_) => "accepted".to_owned(),
        Err(e) => e.detail(),
    };
    drop(r);
    let m = Measure {
        bytes: text.len(),
        secs,
        peak,
        kept,
    };
    (m, outcome)
}

/// The adversarial documents review round 1 named, legal or not, each
/// filled to 64 MiB: phase 1's peak `p` must stay under 512 MiB on every
/// one (`docs/plans/batch.md` section 8).
#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
#[allow(clippy::too_many_lines)] // one table of shapes
fn phase_1_on_the_worst_documents() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let dir = Scratch::new("batch-measure-worst");
    // `_` is no short id.
    let one = r#""inputs":[{"id":"_","path":"/i"}],"#;
    let deps = format!(r#"{HEAD}"inputs":[{{"id":"_","path":"/i","deps":["#);
    let full_list = |i: usize| {
        let ids = vec![r#""""#; 65_536].join(",");
        format!(r#"{{"id":"{}","path":"/","deps":[{ids}]}}"#, short_id(i))
    };
    let no_outputs = HEAD.replace(
        r#""outputs":{"textures":{"key":"{input}/{name}.png"}},"#,
        "",
    );
    let shapes: Vec<(&str, String)> = vec![
        ("zeros at the top", fill("[", |_| "0".into(), "]")),
        (
            "zeros under a known field",
            fill(&deps, |_| "0".into(), "]}]}"),
        ),
        (
            "zeros under an unknown member",
            fill(&format!(r#"{HEAD}{one}"zzz":["#), |_| "0".into(), "]}"),
        ),
        (
            "escaped one-character deps",
            fill(&deps, |_| r#""\u0061""#.into(), "]}]}"),
        ),
        ("empty deps", fill(&deps, |_| r#""""#.into(), "]}]}")),
        (
            "empty deps in full lists",
            fill(&format!(r#"{HEAD}{one}"dependencies":["#), full_list, "]}"),
        ),
        (
            "escaped ids",
            fill(
                &format!(r#"{HEAD}{one}"dependencies":["#),
                |_| r#"{"id":"\u0061"}"#.into(),
                "]}",
            ),
        ),
        (
            "empty script_classes",
            fill(
                &format!(
                    r#"{no_outputs}{one}"outputs":{{"atlases":{{"sprite_key":"a","json_key":"b","script_classes":["#
                ),
                |_| r#""""#.into(),
                "]}}}",
            ),
        ),
        (
            "a non-ASCII url",
            format!(
                r#"{HEAD}"inputs":[{{"id":"i","url":"https://e.net/{}"}}]}}"#,
                "\u{e9}".repeat(30 << 20)
            ),
        ),
        (
            "a url of spaces",
            format!(
                r#"{HEAD}"inputs":[{{"id":"i","url":"https://e.net/{}x"}}]}}"#,
                " ".repeat(60 << 20)
            ),
        ),
        (
            "a url of spaces, its slashes escaped",
            format!(
                r#"{HEAD}"inputs":[{{"id":"i","url":"https:\/\/e.net\/{}x"}}]}}"#,
                " ".repeat(60 << 20)
            ),
        ),
        (
            "a url's long host after an uppercase letter",
            format!(
                r#"{HEAD}"inputs":[{{"id":"i","url":"https://A{}/"}}]}}"#,
                "a".repeat(60 << 20)
            ),
        ),
        (
            "an escaped endpoint",
            format!(
                r#"{HEAD}{one}"deadline":"2026-01-01T00:00:00Z","zz":0}}"#
            )
            .replace(
                r#""sink":{"kind":"directory","root":"/out"}"#,
                &format!(
                    r#""sink":{{"kind":"object_store","bucket":"bkt","region":"r","endpoint":"https:\/\/{}"}}"#,
                    "a".repeat(60 << 20)
                ),
            )
            .replace(r#","zz":0"#, ""),
        ),
        (
            "id-only dependencies",
            fill(
                &format!(r#"{HEAD}{one}"dependencies":["#),
                |_| r#"{"id":"a"}"#.into(),
                "]}",
            ),
        ),
        (
            "id-only unique dependencies",
            fill(
                &format!(r#"{HEAD}{one}"dependencies":["#),
                |i| format!(r#"{{"id":"{}"}}"#, short_id(i)),
                "]}",
            ),
        ),
        (
            "unique dependencies",
            fill(
                &format!(r#"{HEAD}{one}"dependencies":["#),
                |i| format!(r#"{{"id":"{}","path":"/"}}"#, short_id(i)),
                "]}",
            ),
        ),
        (
            "unique dependencies sharing five bytes",
            fill(
                &format!(r#"{HEAD}{one}"dependencies":["#),
                |i| format!(r#"{{"id":"ppppp{}","path":"/"}}"#, short_id(i)),
                "]}",
            ),
        ),
        (
            "empty prefers sharing five bytes",
            fill(
                &format!(r#"{HEAD}{one}"prefer":{{"#),
                |i| format!(r#""ppppp{}":{{}}"#, short_id(i)),
                "}}",
            ),
        ),
        (
            "empty prefers",
            fill(
                &format!(r#"{HEAD}{one}"prefer":{{"#),
                |i| format!(r#""{}":{{}}"#, short_id(i)),
                "}}",
            ),
        ),
    ];
    for (name, text) in shapes {
        let (m, outcome) = measure_any(&dir, "worst", &text);
        drop(text);
        row(name, VIEW_WORK, &outcome, &m);
    }
}
