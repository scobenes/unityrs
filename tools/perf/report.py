"""The perf report's aggregates (perf.py): ranges over repeated passes, ratios, and the counts of plan section 8.

`summarise(records, meta)` turns perf.py's pass records ({phase, round, bench, mem}) into one JSON-able dict;
`markdown(summary)` renders it as the tables the plan quotes. Aggregates only: no input is named. Standard
library only.
"""

from __future__ import annotations

import json
import statistics


def rng(xs):
    xs = [x for x in xs if x is not None]
    return [min(xs), max(xs)] if xs else None


def median(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def by_phase(records) -> dict[str, list[dict]]:
    out: dict[str, list[dict]] = {}
    for r in records:
        out.setdefault(r["phase"], []).append(r)
    return out


def phase_stats(recs: list[dict]) -> dict:
    """Ranges over a phase's runs, and each run's timing beside its load."""
    b = [r["bench"] for r in recs]
    m = [r["mem"] for r in recs]
    out = {
        "runs": len(recs),
        "wall_s": rng([x["wall_s"] for x in b]),
        "objects_per_s": rng([x["objects_per_s"] for x in b]),
        "mb_per_s": rng([x["mb_per_s"] for x in b]),
        "peak_pss_sum_bytes": rng([x["peak_pss_sum_bytes"] for x in m]),
        "max_processes": max(x["max_processes"] for x in m),
        "peak_rss_bytes": rng([x.get("peak_rss_bytes") for x in b]),
        "load": {
            "loadavg_1m_before": rng([x["load"]["loadavg_before"][0] for x in m]),
            "others_cores": rng([x["load"]["others_cores"] for x in m]),
            "box_busy_cores": rng([x["load"]["box_busy_cores"] for x in m]),
        },
        "per_run": [
            {
                "round": r["round"],
                "wall_s": r["bench"]["wall_s"],
                "mb_per_s": r["bench"]["mb_per_s"],
                "objects_per_s": r["bench"]["objects_per_s"],
                "peak_pss_sum_bytes": r["mem"]["peak_pss_sum_bytes"],
                "loadavg_before": r["mem"]["load"]["loadavg_before"],
                "loadavg_after": r["mem"]["load"]["loadavg_after"],
                "others_cores": r["mem"]["load"]["others_cores"],
                "box_busy_cores": r["mem"]["load"]["box_busy_cores"],
            }
            for r in recs
        ],
    }
    if b[0].get("bench") == "unityrs":
        out["decode_share"] = rng([x["decode_share"] for x in b])
        out["cpu_threads_s"] = rng([x["cpu_s"]["threads"] for x in b])
        out["cpu_decode_s"] = rng([x["cpu_s"]["decode"] for x in b])
        out["clock"] = sorted({x["clock"] for x in b})
        out["counters"] = sorted({x["counters"] for x in b})
    return out


def ratio(num: dict | None, den: dict | None, key: str = "objects_per_s") -> dict | None:
    """num's throughput over den's: the range (worst over best, best over worst) and the medians' ratio."""
    if not num or not den:
        return None
    n = [r["bench"][key] for r in num["_recs"]]
    d = [r["bench"][key] for r in den["_recs"]]
    return {"range": [min(n) / max(d), max(n) / min(d)], "median": median(n) / median(d)}


def change(new: dict | None, base: dict | None) -> dict | None:
    """How much longer `new`'s passes took than `base`'s: medians' relative difference, and both wall ranges."""
    if not new or not base:
        return None
    nw = [r["bench"]["wall_s"] for r in new["_recs"]]
    bw = [r["bench"]["wall_s"] for r in base["_recs"]]
    return {"wall_s": rng(nw), "base_wall_s": rng(bw), "median_change": median(nw) / median(bw) - 1}


def consistency(records) -> dict:
    """Whether every pass over the whole selection saw the same inputs, bytes and objects."""
    full = [r for r in records if r["phase"] != "unityrs-lazy-r2-sample"]
    seen = {(r["bench"]["inputs"], r["bench"]["input_bytes"], r["bench"]["objects"]) for r in full}
    errors = {(r["bench"]["file_errors"], r["bench"]["value_errors"]) for r in full}
    return {
        "same_inputs_bytes_objects": len(seen) == 1,
        "seen": sorted(seen),
        "errors_file_value": sorted(errors),
        "leaked_bytes": sum(r["bench"].get("leaked_bytes", 0) for r in records),
    }


def once(phases, name):
    recs = phases.get(name)
    return recs[0]["bench"] if recs else None


def counts(phases) -> dict:
    eager = once(phases, "unityrs-tracked-serial")
    par = once(phases, "unityrs-tracked-parallel")
    sample = once(phases, "unityrs-lazy-r2-sample")
    lazy = once(phases, "unityrs-lazy")
    pss = {n: phases[n][0]["mem"]["peak_pss_sum_bytes"] for n in phases if n.startswith("unityrs-tracked")}
    out = {}
    if sample or lazy or eager:
        out["r2"] = {
            "sample_lazy": sample and {"inputs": sample["inputs"], **sample["bundles"]},
            "all_lazy": lazy and {"inputs": lazy["inputs"], **lazy["bundles"]},
            "all_eager": eager and {"inputs": eager["inputs"], **eager["bundles"]},
        }
    if eager:
        out["r3"] = {
            "serial": {
                "peak_rss_bytes": eager["peak_rss_bytes"],
                "live_charges_peak": eager["live_charges_peak"],
                "peak_pss_sum_bytes": pss.get("unityrs-tracked-serial"),
            },
            "parallel": par
            and {
                "jobs": par["jobs"],
                "peak_rss_bytes": par["peak_rss_bytes"],
                "live_charges_peak": par["live_charges_peak"],
                "peak_pss_sum_bytes": pss.get("unityrs-tracked-parallel"),
            },
            "runs_peak_per_bundle": eager["runs_peak_per_bundle_max_over_inputs"],
            "runs_peak_per_bundle_lazy": lazy and lazy["runs_peak_per_bundle_max_over_inputs"],
            "pin": eager.get("pin"),
        }
        out["r5"] = eager["versions"]
        out["r10"] = {
            "meter_peak": eager["meter_peak"],
            "meter_peak_over_decoded": eager["meter_peak_over_decoded"],
            "meter_peak_over_decoded_1mib_up": eager["meter_peak_over_decoded_1mib_up"],
            "decoded_size": eager["decoded_size"],
            "max_input_memory": eager["max_input_memory"],
            "lazy_meter_peak": lazy and lazy["meter_peak"],
            "lazy_meter_peak_over_decoded": lazy and lazy["meter_peak_over_decoded"],
            "lazy_meter_peak_over_decoded_1mib_up": lazy and lazy["meter_peak_over_decoded_1mib_up"],
        }
    return out


def summarise(records, meta) -> dict:
    phases = by_phase(records)
    stats = {name: {**phase_stats(recs), "_recs": recs} for name, recs in phases.items()}
    get = stats.get
    serial = ratio(get("unityrs-serial"), get("unitypy-serial"))
    out = {
        **meta,
        "phases": {n: {k: v for k, v in s.items() if k != "_recs"} for n, s in stats.items()},
        "r4": {
            "serial_unityrs_over_unitypy": serial,
            "parallel_unityrs_over_unitypy": ratio(get("unityrs-parallel"), get("unitypy-parallel")),
            "blocker_slower_serially": bool(serial and serial["median"] <= 1),
            "serial_ranges_overlap": bool(serial and serial["range"][0] <= 1),
        },
        "counters_cost_serial": change(get("unityrs-serial"), get("unityrs-serial-nocounters")),
        "r10_counting_over_unbounded": {
            "serial": change(get("unityrs-serial-counting"), get("unityrs-serial")),
            "parallel": change(get("unityrs-parallel-counting"), get("unityrs-parallel")),
        },
        "consistency": consistency(records),
        "counts": counts(phases),
    }
    return out


def span(r, fmt="{:,.2f}"):
    if not r:
        return "-"
    lo, hi = (fmt.format(v) for v in r)
    return lo if lo == hi else f"{lo}-{hi}"


def markdown(s: dict) -> str:
    trial = f", every pass on a sample of {s['sample']} inputs" if s.get("sample") else ""
    lines = [
        f"# Perf report ({s['started_utc']}, {s['repeat']} rounds, -j {s['jobs']}{trial})",
        "",
        "| pass | runs | wall s | objects/s | MB/s | peak PSS MB | decode share | loadavg 1m | others' cores | busy CPUs |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for name, p in s["phases"].items():
        pss = [x / 1e6 for x in p["peak_pss_sum_bytes"]]
        lines.append(
            f"| {name} | {p['runs']} | {span(p['wall_s'])} | {span(p['objects_per_s'], '{:,.0f}')} "
            f"| {span(p['mb_per_s'], '{:,.1f}')} | {span(pss, '{:,.0f}')} "
            f"| {span(p.get('decode_share'), '{:.3f}')} | {span(p['load']['loadavg_1m_before'], '{:.1f}')} "
            f"| {span(p['load']['others_cores'], '{:.1f}')} | {span(p['load']['box_busy_cores'], '{:.1f}')} |"
        )
    lines += [
        "",
        "Each run, its load beside it:",
        "",
        "| pass | round | wall s | MB/s | loadavg before/after | others' cores | busy CPUs |",
        "|---|---|---|---|---|---|---|",
    ]
    for name, p in s["phases"].items():
        for r in p["per_run"]:
            lines.append(
                f"| {name} | {r['round'] or '-'} | {r['wall_s']:.2f} | {r['mb_per_s']:.1f} "
                f"| {r['loadavg_before'][0]:.1f} / {r['loadavg_after'][0]:.1f} | {r['others_cores']:.1f} "
                f"| {r['box_busy_cores']:.1f} |"
            )
    r4 = s["r4"]
    head = {k: s[k] for k in ("r4", "counters_cost_serial", "r10_counting_over_unbounded", "consistency")}
    lines += ["", "```json", json_block(head), "```"]
    lines += ["", "Counts (plan section 8):", "", "```json", json_block(s["counts"]), "```", ""]
    if r4["blocker_slower_serially"]:
        lines.insert(2, "**Blocker: unityrs is slower than UnityPyBoost serially (median over median).**\n")
    return "\n".join(lines) + "\n"


def json_block(x) -> str:
    return json.dumps(x, indent=1, sort_keys=True)
