//! X13, the squeeze (`docs/plans/extract.md` X13; `docs/plans/batch.md`
//! 5.8): every testkit scene's every input through the sealed attempt API
//! below validation (`run_attempt`, whose limits a spec's 1 MiB floors and
//! `M <= D` rule would keep above a small input's needs), one pass per
//! limit with the others nonbinding:
//!
//! - **`M`**, its four meanings together: meter memory and input bytes at
//!   `m`, decode work at `4m`, the Input budget at `m` (`run_attempt`'s
//!   context). Its need is the least `m` with no CR8 refusal of
//!   `InputMemory`, `InputBytes`, `DecodeWork` or `Budget(Input)`, absorbed
//!   or surfaced, and no record naming one.
//! - **`D`**: the least `d` with no `Budget(Dependency)` refusal and no
//!   eviction.
//! - **`max_export_work`**: the baseline's `export_work`.
//! - **`max_attempt_evictions`**: where the run at `M` = `D` = `M`'s need
//!   completes with evictions, their count.
//!
//! Each input runs as it is and as its `+members` variant, its outputs
//! expanded into archive members. Each need is bisected in a child of its
//! own and checked monotone at its adjacent and sampled values
//! (`unityrs_testkit::search`), each sampled value with no refusal giving
//! the baseline's result; a counterexample fails the test as one that
//! needs a spec ruling. The need, one above it and one below it then each
//! run twice, each time in a fresh child: the two agree, the need's and the
//! one above's runs are the baseline's, and the one below's is one of
//! X13's outcomes, for `M` exactly the records a reference model derives
//! from the baseline (`squeeze_support::compare`). The scenes CR7 keeps out are exactly
//! `squeeze_support::inventory`'s.
//!
//! It reads the engine's test tap, so it is built with `test-hooks`.
#![cfg(feature = "test-hooks")]

mod squeeze_support;

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value as J, json};
use squeeze_support::compare::{below_d, below_m};
use squeeze_support::exclusions::{UNTIL, detect, inventory};
use squeeze_support::{
    BIG, BIG_EVICTIONS, MEMBERS, Outcome, catalogue, limits, refused_d, refused_m, run, run_failure,
};
use unityrs_batch::reason::{Limit, LimitKind};
use unityrs_testkit::ngui::AtlasScene;
use unityrs_testkit::search::least;
use unityrs_testkit::{child, guard};

const VAR: &str = "UNITYRS_SQUEEZE";
/// Values sampled on each side of a need, beyond its neighbours and the
/// powers of two away from it.
const SAMPLES: u64 = 4;

/// A catalogue scene by name, and whether the name is its `+members`
/// variant's.
fn scene(name: &str) -> (AtlasScene, bool) {
    let (base, members) = name
        .strip_suffix(MEMBERS)
        .map_or((name, false), |b| (b, true));
    let s = catalogue().into_iter().find(|(n, _)| *n == base).unwrap().1;
    (s, members)
}

/// A need's search, in this process (a child's), the other limits
/// nonbinding: `Err` is a counterexample's text. Each sampled value with
/// no refusal must give the baseline's result.
fn need(pass: &str, s: &AtlasScene, input: &str, members: bool) -> Result<u64, String> {
    let base = run(s, input, limits(BIG, BIG, BIG_EVICTIONS, BIG), members);
    let ask = |v: u64| {
        let (o, refused): (Outcome, fn(&Outcome) -> bool) = match pass {
            "m" => (
                run(s, input, limits(v, BIG, BIG_EVICTIONS, BIG), members),
                refused_m,
            ),
            _ => (
                run(s, input, limits(BIG, v, BIG_EVICTIONS, BIG), members),
                refused_d,
            ),
        };
        assert!(
            o.allocator == 0 && !run_failure(&o),
            "{pass}={v}: no squeeze outcome: {o:?}"
        );
        let r = refused(&o);
        assert!(
            r || o.same_result(&base),
            "{pass}={v}: clear, but not the baseline's result: {o:?}\nbaseline: {base:?}"
        );
        r
    };
    least(0, BIG, SAMPLES, ask)
        .map(|l| l.need)
        .map_err(|e| format!("{pass}: {e}"))
}

/// The child: `search\t<scene>\t<input>` or
/// `run\t<scene>\t<input>\t<m>\t<d>\t<evictions>\t<work>`; one `SQUEEZE`
/// line with its result.
#[test]
fn squeeze_child() {
    let Ok(spec) = std::env::var(VAR) else {
        return;
    };
    let fields: Vec<&str> = spec.split('\t').collect();
    let (mode, name, id) = (fields[0], fields[1], fields[2]);
    let (scene, members) = scene(name);
    let report = match mode {
        "peek" | "peeksearch" => {
            let data = &scene.bundles.iter().find(|b| b.id == id).unwrap().data;
            if mode == "peek" {
                squeeze_support::peek(data, fields[3].parse().unwrap())
            } else {
                peek_need(data)
            }
        }
        "search" => search(name, &scene, id, members),
        _ => {
            let value = |i: usize| fields[i].parse::<u64>().unwrap();
            let l = limits(value(3), value(4), value(5), value(6));
            run(&scene, id, l, members).to_json()
        }
    };
    println!("\nSQUEEZE {report}");
}

/// A peek's `M` need, or its counterexample's text.
fn peek_need(data: &[u8]) -> J {
    let refused = |m: u64| {
        let o = squeeze_support::peek(data, m);
        assert_eq!(o["allocator"], 0, "m={m}: {o}");
        peek_refused(&o)
    };
    match least(0, BIG, SAMPLES, refused) {
        Ok(found) => json!(found.need),
        Err(e) => json!(format!("peek: {e}")),
    }
}

/// The `M` and `D` needs of `input`, each null where an exclusion keeps
/// its pass out (a `+members` variant's as its scene's), or a
/// counterexample's text.
fn search(name: &str, scene: &AtlasScene, input: &str, members: bool) -> J {
    let base = name.strip_suffix(MEMBERS).unwrap_or(name);
    let excluded: Vec<_> = inventory()
        .into_iter()
        .filter(|e| e.scene == base && e.input == input)
        .collect();
    let pass = |p: &str, out: bool| match (out, need(p, scene, input, members)) {
        (true, _) => J::Null,
        (false, Ok(found)) => json!(found),
        (false, Err(e)) => json!(e),
    };
    json!({
        "m": pass("m", excluded.iter().any(|e| e.m())),
        "d": pass("d", excluded.iter().any(|e| e.d())),
    })
}

/// Runs the child with `spec` in a fresh process.
fn child(spec: &str) -> J {
    let out = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "squeeze_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(VAR, spec),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{spec}: {}\n{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = text
        .lines()
        .find_map(|l| l.split_once("SQUEEZE ").map(|(_, r)| r))
        .unwrap_or_else(|| panic!("{spec}: no report\n{text}"));
    serde_json::from_str(line).unwrap()
}

/// One run under `l` = `[m, d, evictions, work]`, twice, each in a fresh
/// child: the two must agree.
fn twice(scene: &str, input: &str, l: [u64; 4]) -> Outcome {
    let spec = format!(
        "run\t{scene}\t{input}\t{}\t{}\t{}\t{}",
        l[0], l[1], l[2], l[3]
    );
    let a = Outcome::from_json(&child(&spec));
    let b = Outcome::from_json(&child(&spec));
    assert_eq!(a, b, "{spec}: two fresh runs differ");
    a
}

/// One input's passes; each line of the result names a pass, its need and
/// what its run one below the need was, or a failure.
fn squeeze(name: &str, input: &str) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let found = child(&format!("search\t{name}\t{input}"));
    let base = twice(name, input, [BIG, BIG, BIG_EVICTIONS, BIG]);
    if base.m_refusals + base.d_refusals + base.evictions > 0 || base.state != "Done" {
        return vec![Err(format!(
            "{name}/{input}: baseline is not unbounded: {base:?}"
        ))];
    }
    // The limit at index `i` of `[m, d, evictions, work]` at its need and
    // one above it (each the baseline's), and one below it.
    let boundary = |pass: &str, need: u64, i: usize| {
        let with = |v: u64| {
            let mut l = [BIG, BIG, BIG_EVICTIONS, BIG];
            l[i] = v;
            l
        };
        for v in [need, need + 1] {
            let o = twice(name, input, with(v));
            if !o.same_result(&base) || refused_m(&o) || (pass != "m" && refused_d(&o)) {
                return Err(format!(
                    "{name}/{input} {pass}={v}: at or above the need, not the baseline's: {o:?}"
                ));
            }
        }
        if need == 0 {
            return Ok(format!("{name}/{input} {pass} need 0"));
        }
        let o = twice(name, input, with(need - 1));
        match pass {
            "m" => below_m(&o, &base),
            "d" => below_d(&o, &base),
            // The engine's own limit always fails X, at the value set.
            _ => (o.state == "Failed" && o.reason == engine_limit(LimitKind::ExportWork, need - 1))
                .then_some("failed")
                .ok_or_else(|| format!("{o:?}")),
        }
        .map(|how| format!("{name}/{input} {pass} need {need}: {how}"))
        .map_err(|e| format!("{name}/{input} {pass} below its need: {e}"))
    };
    for (pass, at) in [("m", 0), ("d", 1)] {
        match &found[pass] {
            J::Null => out.push(Ok(format!("{name}/{input} {pass} excluded ({UNTIL})"))),
            J::String(e) => out.push(Err(format!("{name}/{input} {e}"))),
            n => out.push(boundary(pass, n.as_u64().unwrap(), at)),
        }
    }
    out.push(boundary("ExportWork", base.export_work, 3));
    if let Some(m) = found["m"].as_u64() {
        let reference = twice(name, input, [m, m, BIG_EVICTIONS, BIG]);
        let e = reference.evictions;
        if reference.state == "Done" && e > 0 {
            let base = reference;
            let at = twice(name, input, [m, m, e, BIG]);
            let below = twice(name, input, [m, m, e - 1, BIG]);
            out.push(
                if at.same_result(&base)
                    && below.state == "Failed"
                    && below.reason == engine_limit(LimitKind::AttemptEvictions, e - 1)
                {
                    Ok(format!("{name}/{input} AttemptEvictions need {e}: failed"))
                } else {
                    Err(format!(
                        "{name}/{input} AttemptEvictions at {e}: {at:?}; below: {below:?}"
                    ))
                },
            );
        }
    }
    out
}

/// An engine limit's failure at `value`, in its records' spelling.
fn engine_limit(kind: LimitKind, value: u64) -> String {
    Limit { kind, value }.to_string()
}

/// The next input, while any is left.
fn next<T>(work: &Mutex<Vec<T>>) -> Option<T> {
    work.lock().unwrap_or_else(PoisonError::into_inner).pop()
}

/// Every scene's every input, each in children of its own, at most as many
/// at once as the host has threads.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn x13_every_limit_of_every_scene_at_its_need() {
    // Each input as it is and as its `+members` variant, whose outputs
    // expand into archive members (its own export work).
    let mut work: Vec<(String, String)> = catalogue()
        .into_iter()
        .flat_map(|(n, s)| s.inputs.into_iter().map(move |i| (n, i)))
        .flat_map(|(n, i)| [(n.to_owned(), i.clone()), (format!("{n}{MEMBERS}"), i)])
        .collect();
    work.reverse();
    let work = Mutex::new(work);
    let results = Mutex::new(BTreeMap::new());
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let () = guard::finishes("the squeeze's children", move || {
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    while let Some((n, i)) = next(&work) {
                        let r = squeeze(&n, &i);
                        results
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert((n, i), r);
                    }
                });
            }
        });
        let results = results.into_inner().unwrap_or_else(PoisonError::into_inner);
        let mut failed = Vec::new();
        for line in results.values().flatten() {
            match line {
                Ok(l) => println!("X13 {l}"),
                Err(e) => failed.push(e.clone()),
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
        assert_eq!(results.len(), 84, "the catalogue's inputs, each twice");
        let evicted = results.values().flatten().flatten();
        assert!(
            evicted.into_iter().any(|l| l.contains("AttemptEvictions")),
            "no scene evicts at M = D = M's need"
        );
    });
}

/// The exclusions are exactly what the catalogue's structure calls for.
#[test]
fn the_exclusions_are_exactly_the_catalogues_cr7_gaps() {
    assert_eq!(detect(&catalogue()), inventory());
}

/// A peek met an `M` refusal: counted, or as its failed outcome (typed).
fn peek_refused(o: &J) -> bool {
    o["m_refusals"].as_u64().unwrap() > 0 || o["m_limit"].as_bool().unwrap()
}

/// Runs a peek child twice, each fresh: the two agree.
fn peek_twice(scene: &str, bundle: &str, m: u64) -> J {
    let spec = format!("peek\t{scene}\t{bundle}\t{m}");
    let (a, b) = (child(&spec), child(&spec));
    assert_eq!(a, b, "{spec}: two fresh peeks differ");
    a
}

/// The once-per-run peek's companion `M` boundary (X13): a peek's failure
/// precedes its views' attempts, so every bundle of the catalogue is peeked
/// at its own `M` need, which must give the unbounded peek's outcome and
/// file, and one below it, which must fail with an `M` limit or give that
/// outcome with an `M` refusal counted.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn x13_every_bundles_peek_at_its_m_need() {
    let mut work: Vec<(&'static str, String)> = catalogue()
        .into_iter()
        .flat_map(|(n, s)| s.bundles.into_iter().map(move |b| (n, b.id)))
        .collect();
    let total = work.len();
    work.reverse();
    let work = Mutex::new(work);
    let failed = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let () = guard::finishes("the peeks' children", move || {
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    while let Some((n, b)) = next(&work) {
                        if let Err(e) = peek_boundary(n, &b) {
                            failed
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push(e);
                        }
                    }
                });
            }
        });
        let failed = failed.into_inner().unwrap_or_else(PoisonError::into_inner);
        assert!(failed.is_empty(), "{}", failed.join("\n"));
        println!("X13 peeks: {total} bundles at their M needs");
    });
}

fn peek_boundary(scene: &str, bundle: &str) -> Result<(), String> {
    let found = child(&format!("peeksearch\t{scene}\t{bundle}"));
    let need = found
        .as_u64()
        .ok_or_else(|| format!("{scene}/{bundle} {found}"))?;
    let base = peek_twice(scene, bundle, BIG);
    let at = peek_twice(scene, bundle, need);
    let same = |o: &J| {
        o["state"] == base["state"] && o["outcome"] == base["outcome"] && o["file"] == base["file"]
    };
    if !same(&at) || peek_refused(&at) || at["clear"] != true {
        return Err(format!(
            "{scene}/{bundle} peek at its need {need}: {at} against {base}"
        ));
    }
    if need == 0 {
        return Ok(());
    }
    let below = peek_twice(scene, bundle, need - 1);
    let ok = below["allocator"] == 0
        && below["clear"] == true
        && below["state"] == "Done"
        && (below["m_limit"] == true || (same(&below) && below["m_refusals"].as_u64() > Some(0)));
    if ok {
        println!("X13 peek {scene}/{bundle} M need {need}");
        Ok(())
    } else {
        Err(format!(
            "{scene}/{bundle} peek below its need {need}: {below}"
        ))
    }
}
