//! The bench's JSON: one object for the pass, and a line per input.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use unityrs_core::limits::Limits;

use crate::work::{Load, Sums};
use crate::{Args, Pass};

#[allow(clippy::cast_precision_loss)]
fn f(x: u64) -> f64 {
    x as f64
}

fn secs(ns: u64) -> f64 {
    f(ns) / 1e9
}

/// The value at fraction `p` of `sorted`, rounding the position down
/// (index `floor((n - 1) p)`), or 0 for none.
pub fn at(sorted: &[u64], p: f64) -> u64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0;
    };
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let i = ((last as f64) * p).floor() as usize;
    sorted.get(i.min(last)).copied().unwrap_or(0)
}

/// Max, 99th and 50th percentiles of `xs`.
pub fn spread(mut xs: Vec<u64>) -> Value {
    xs.sort_unstable();
    json!({"n": xs.len(), "max": at(&xs, 1.0), "p99": at(&xs, 0.99), "p50": at(&xs, 0.5)})
}

/// Max, 99th and 50th percentiles of ratios `num / den`, over the pairs with
/// `den > 0`.
pub fn ratios(pairs: impl Iterator<Item = (u64, u64)>) -> Value {
    let mut r: Vec<f64> = pairs
        .filter(|(_, d)| *d > 0)
        .map(|(n, d)| f(n) / f(d))
        .collect();
    r.sort_by(f64::total_cmp);
    let pick = |p: f64| {
        let Some(last) = r.len().checked_sub(1) else {
            return 0.0;
        };
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let i = ((last as f64) * p).floor() as usize;
        r.get(i.min(last)).copied().unwrap_or(0.0)
    };
    json!({"n": r.len(), "max": pick(1.0), "p99": pick(0.99), "p50": pick(0.5)})
}

fn sums_json(s: &Sums) -> Value {
    json!({
        "count": s.bundles,
        "blocks": s.blocks,
        "blocks_max": s.blocks_max,
        "decoded": s.decoded,
        "discarded": s.discarded,
        "redecoded": s.redecoded,
        "runs_peak_max": s.runs_peak_max,
        "runs_peak_sum": s.runs_peak_sum,
        "declared": s.declared,
    })
}

/// The pass's JSON object (the crate docs list what it holds).
pub fn summary(args: &Args, pass: &Pass, input_bytes: u64) -> Value {
    let d = &pass.done;
    let sum = |g: fn(&crate::Done) -> u64| d.iter().map(g).sum::<u64>();
    let objects = sum(|x| x.objects);
    let decode_ns = pass.decode.ns.load(Ordering::Relaxed);
    let mut sums = Sums::default();
    let mut versions: BTreeMap<&str, BTreeMap<String, u64>> = BTreeMap::new();
    for x in d {
        sums.absorb(&x.sums);
        for ((chain, source), n) in &x.versions {
            *versions
                .entry(chain)
                .or_default()
                .entry(source.clone())
                .or_default() += n;
        }
    }
    let wall = pass.wall_s;
    let mut out = json!({
        "bench": "unityrs",
        "inputs": d.len(),
        "input_bytes": input_bytes,
        "jobs": args.jobs,
        "budget": args.budget.name(),
        "load": if args.load == Load::Eager { "eager" } else { "lazy" },
        "counters": cfg!(not(unityrs_no_counters)),
        "clock": pass.clock.kind(),
        "wall_s": wall,
        "objects": objects,
        "objects_per_s": f(objects) / wall,
        "mb_per_s": f(input_bytes) / 1e6 / wall,
        "file_errors": d.iter().filter(|x| x.file_error).count(),
        "value_errors": sum(|x| x.value_errors),
        "cpu_s": {
            "threads": secs(pass.threads_ns),
            "load": secs(sum(|x| x.load_ns)),
            "values": secs(sum(|x| x.values_ns)),
            "decode": secs(decode_ns),
        },
        "decode_calls": pass.decode.calls.load(Ordering::Relaxed),
        "decode_share": if pass.threads_ns > 0 { f(decode_ns) / f(pass.threads_ns) } else { 0.0 },
        "peak_rss_bytes": crate::clock::peak_rss(),
        "bundles": sums_json(&sums),
        "runs_peak_per_bundle_max_over_inputs": spread(d.iter().map(|x| x.sums.runs_peak_max).collect()),
        "versions": versions,
        "leaked_bytes": sum(|x| x.leaked),
    });
    if args.budget == crate::budget::BudgetKind::Tracked {
        let peaks: Vec<u64> = pass.meter_peaks.iter().map(|p| p.unwrap_or(0)).collect();
        let mut top = peaks.clone();
        top.sort_unstable_by(|a, b| b.cmp(a));
        out["live_charges_peak"] = json!(pass.process.peak());
        out["meter_peak"] = spread(peaks.clone());
        out["meter_peak_top8_sum"] = json!(top.iter().take(8).sum::<u64>());
        out["meter_peak_over_decoded"] =
            ratios(peaks.iter().copied().zip(d.iter().map(|x| x.decoded_size)));
        out["decoded_size"] = spread(d.iter().map(|x| x.decoded_size).collect());
        out["max_input_memory"] = json!(Limits::default().max_input_memory);
    }
    if args.pin {
        let pins: Vec<_> = d
            .iter()
            .filter_map(|x| x.pin.map(|p| (p, x.decoded_size)))
            .collect();
        let half = pins
            .iter()
            .filter(|(p, s)| *s > 0 && p.one.saturating_mul(2) >= *s)
            .count();
        out["pin"] = json!({
            "inputs": pins.len(),
            "all": spread(pins.iter().map(|(p, _)| p.all).collect()),
            "one": spread(pins.iter().map(|(p, _)| p.one).collect()),
            "one_own": spread(pins.iter().map(|(p, _)| p.one_own).collect()),
            "one_over_decoded": ratios(pins.iter().map(|(p, s)| (p.one, *s))),
            "one_holds_half_its_input_or_more": half,
            "one_slices": pins.iter().filter(|(p, _)| p.one_slices).count(),
        });
    }
    out
}

/// A JSON line per input, by position.
pub fn per_input(pass: &Pass) -> String {
    let mut text = String::new();
    for (i, (x, peak)) in pass.done.iter().zip(&pass.meter_peaks).enumerate() {
        let line = json!({
            "pos": i,
            "bytes": x.bytes,
            "objects": x.objects,
            "value_errors": x.value_errors,
            "file_error": x.file_error,
            "load_s": secs(x.load_ns),
            "values_s": secs(x.values_ns),
            "decoded_size": x.decoded_size,
            "meter_peak": peak,
            "bundles": sums_json(&x.sums),
            "leaked": x.leaked,
            "pin": x.pin.map(|p| json!({"all": p.all, "one": p.one, "one_own": p.one_own, "one_slices": p.one_slices})),
        });
        text.push_str(&line.to_string());
        text.push('\n');
    }
    text
}
