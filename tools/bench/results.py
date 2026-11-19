"""The comparison's schema, pairing and aggregation (docs/plans/bench.md sections 3 and 4.5), shared by bench.py's
`run`, `render` and `check`. Standard library only.

**Pairing** (decision 4). A workload is paired when its PARITY.md row is Tier A for the timed operation, both
implementations completed every measured round with status `ok`, and every round of both produced one digest.
Otherwise it is unpaired, with the first reason that applies, and appears only in BENCHMARKS.md's unityrs-only table.

**Aggregation** (section 3). A workload's figure is the median of its rounds' scope times, with the min and max as
its spread. A category's figure is, for each round, the sum of its paired workloads' times, then the median of those
round totals, the min and max giving its spread. The speedup is the ratio of the two medians; its range runs from
min(UnityPy) / max(unityrs) to max(UnityPy) / min(unityrs), over the round totals.
"""

from __future__ import annotations

import re
import statistics

SCHEMA_VERSION = 1
IMPLS = ("unitypy", "unityrs")
TOP = ("schema_version", "run_id", "run_attempt", "commit", "date", "scale", "rounds", "warmup", "tools", "machine",
       "fixtures_sha256", "workloads")
TOOLS = ("unityrs", "rustc", "unitypy", "unitypyboost_sha256", "pillow", "python")
MACHINE = ("cpu", "cores", "memory_bytes", "kernel", "runner_image", "emulated")
ENTRY = ("name", "impl", "category", "timer", "paired", "reason", "parity_row", "tier", "runs")
RUN = ("round", "scope_s", "process_s", "cpu_s", "peak_pss_bytes", "status", "counts", "digest")
# An absolute path in any string field: a host path (section 4.5 allows none).
HOST_PATH = re.compile(r"(^|[\s=:'\"(])(/(home|Users|root|tmp|private|var|work|mnt|opt)/|[A-Za-z]:\\)")


def strings(value):
    """Every string in a JSON value, keys included."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for k, v in value.items():
            yield k
            yield from strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings(v)


def problems(wall: dict, workloads: dict) -> list[str]:
    """Why `wall` is not a valid wall.json for the workload table `workloads`; empty when it is. The stored
    pairing must be what `pairing` computes from the runs."""
    out = []
    missing = [k for k in TOP if k not in wall]
    if missing:
        return [f"missing fields: {', '.join(missing)}"]
    if wall["schema_version"] != SCHEMA_VERSION:
        out.append(f"schema_version {wall['schema_version']!r}, not {SCHEMA_VERSION}")
    out += [f"tools.{k} missing" for k in TOOLS if k not in wall["tools"]]
    out += [f"machine.{k} missing" for k in MACHINE if k not in wall["machine"]]
    if not isinstance(wall["machine"].get("emulated"), bool):
        out.append("machine.emulated is not a boolean")
    specs = {w["name"]: w for w in workloads["workloads"]}
    seen = set()
    for e in wall["workloads"]:
        lacking = [k for k in ENTRY if k not in e]
        if lacking:
            out.append(f"workload entry lacks {', '.join(lacking)}")
            continue
        key = (e["name"], e["impl"])
        if key in seen:
            out.append(f"{e['name']}/{e['impl']}: listed twice")
        seen.add(key)
        if e["impl"] not in IMPLS:
            out.append(f"{e['name']}: impl {e['impl']!r}")
        spec = specs.get(e["name"])
        if spec is None:
            out.append(f"{e['name']}: not in workloads.json")
            continue
        for k in ("category", "timer", "parity_row", "tier"):
            if e[k] != spec[k]:
                out.append(f"{e['name']}: {k} is not workloads.json's")
        if len(e["runs"]) != wall["rounds"]:
            out.append(f"{e['name']}/{e['impl']}: {len(e['runs'])} runs, not {wall['rounds']}")
        for r in e["runs"]:
            lacking = [k for k in RUN if k not in r]
            if lacking:
                out.append(f"{e['name']}/{e['impl']}: a run lacks {', '.join(lacking)}")
    if out:
        return out
    for name, (paired, reason) in pairings(wall, workloads).items():
        for e in wall["workloads"]:
            if e["name"] == name and (e["paired"], e["reason"]) != (paired, reason):
                out.append(f"{name}/{e['impl']}: stored pairing {e['paired']}, {e['reason']!r}; computed {paired}, "
                           f"{reason!r}")
    out += [f"a host path: {s!r}" for s in strings(wall) if HOST_PATH.search(s)]
    return out


def publish_problems(wall: dict) -> list[str]:
    """Why `wall` may not be published (decision 8): an emulated host, or a scale other than full."""
    out = []
    if wall["machine"].get("emulated") is not False:
        out.append(f"an emulated host ({wall['machine'].get('cpu')!r}) is refused for publishing")
    if wall.get("scale") != "full":
        out.append(f"scale {wall.get('scale')!r}: only the full scale is published")
    return out


def entries(wall: dict, name: str) -> dict:
    """impl -> the entry of workload `name`."""
    return {e["impl"]: e for e in wall["workloads"] if e["name"] == name}


def pairing(spec: dict, by_impl: dict) -> tuple[bool, str | None]:
    """(paired, reason if not) of one workload, from its spec and impl -> entry (decision 4)."""
    if spec["tier"] != "A":
        return False, f"PARITY.md row is Tier {spec['tier']}"
    for impl in IMPLS:
        if impl not in by_impl or not by_impl[impl]["runs"]:
            return False, f"no {impl} runs"
    for impl in IMPLS:
        bad = [r["status"] for r in by_impl[impl]["runs"] if r["status"] != "ok"]
        if bad:
            return False, f"{impl}: {bad[0]}"
    digests = {r["digest"] for impl in IMPLS for r in by_impl[impl]["runs"]}
    if len(digests) != 1 or None in digests:
        return False, "the digests differ"
    return True, None


def pairings(wall: dict, workloads: dict) -> dict:
    """name -> (paired, reason) for every workload `wall` holds, in workloads.json's order."""
    out = {}
    for spec in workloads["workloads"]:
        by_impl = entries(wall, spec["name"])
        if by_impl:
            out[spec["name"]] = pairing(spec, by_impl)
    return out


def times(entry: dict) -> list[float]:
    return [r["scope_s"] for r in sorted(entry["runs"], key=lambda r: r["round"])]


def spread(values: list[float]) -> dict:
    return {"median": statistics.median(values), "min": min(values), "max": max(values)}


def speedup(slow: list[float], fast: list[float]) -> dict:
    """The ratio of the medians, with its range from the extremes (section 3)."""
    return {
        "ratio": statistics.median(slow) / statistics.median(fast),
        "low": min(slow) / max(fast),
        "high": max(slow) / min(fast),
    }


def category_totals(wall: dict, workloads: dict, category: str) -> dict | None:
    """impl -> the per-round totals of the category's paired workloads, or None when none is paired."""
    names = [s["name"] for s in workloads["workloads"] if s["category"] == category]
    paired = [n for n, (ok, _) in pairings(wall, workloads).items() if ok and n in names]
    if not paired:
        return None
    out = {}
    for impl in IMPLS:
        rounds = [times(entries(wall, n)[impl]) for n in paired]
        out[impl] = [sum(r) for r in zip(*rounds, strict=True)]
    return {"workloads": paired, "totals": out}


def summary(wall: dict, workloads: dict) -> list[dict]:
    """One row a category with a paired workload: its figures for both implementations and the speedup."""
    rows = []
    for cat in workloads["categories"]:
        got = category_totals(wall, workloads, cat["name"])
        if got is None:
            continue
        totals = got["totals"]
        rows.append({
            "category": cat["name"],
            "label": cat["label"],
            "workloads": got["workloads"],
            "unitypy": spread(totals["unitypy"]),
            "unityrs": spread(totals["unityrs"]),
            "speedup": speedup(totals["unitypy"], totals["unityrs"]),
        })
    return rows


def workload_figures(wall: dict, name: str) -> dict:
    """impl -> the workload's figures: time spread, median CPU, peak PSS, counts of the last round."""
    out = {}
    for impl, e in entries(wall, name).items():
        ok = [r for r in e["runs"] if r["status"] == "ok"]
        if not ok:
            out[impl] = None
            continue
        pss = [r["peak_pss_bytes"] for r in ok if r["peak_pss_bytes"] is not None]
        out[impl] = {
            **spread([r["scope_s"] for r in ok]),
            "cpu_s": statistics.median([r["cpu_s"] for r in ok]),
            "process_s": statistics.median([r["process_s"] for r in ok]),
            "peak_pss_bytes": max(pss) if pss else None,
            "counts": ok[-1]["counts"],
        }
    return out


# Rendering (decision 9): the README section between the markers, and BENCHMARKS.md, from wall.json, gate.json,
# workloads.json and fixtures.json alone, so a test can hold the committed text to them.

BEGIN, END = "<!-- bench:begin -->", "<!-- bench:end -->"
PENDING = (
    "Results pending. The harness in [tools/bench](tools/bench/README.md) times unityrs against UnityPy 1.25.2 with\n"
    "UnityPyBoost, the same Python script on both, on public synthetic fixtures; the first run published from\n"
    "GitHub's ubuntu-24.04 runner will be summarised here, with the full breakdown in BENCHMARKS.md.\n"
)


def fmt_s(s: float) -> str:
    if s >= 1:
        return f"{s:.2f} s"
    if s >= 1e-3:
        return f"{s * 1e3:.1f} ms"
    return f"{s * 1e6:.0f} µs"


def fmt_x(r: float) -> str:
    return f"×{r:.1f}" if r >= 1 else f"×{r:.2f}"


def fmt_mib(n: int | None) -> str:
    return "—" if n is None else f"{n / 2**20:.1f} MiB"


def stem(fixture: str) -> str:
    return fixture.rsplit(".", 1)[0]


def specs_by_name(workloads: dict) -> dict:
    return {w["name"]: w for w in workloads["workloads"]}


def inventory(row: dict, wall: dict, workloads: dict) -> str:
    """The README's inventory cell of a summary row, from the counts its paired workloads processed."""
    specs = specs_by_name(workloads)
    counts = [(specs[n], workload_figures(wall, n)["unityrs"]["counts"]) for n in row["workloads"]]
    if row["category"] == "typetree":
        n = sum(c.get("objects", 0) for _, c in counts)
        return f"{n:,} objects in {len(counts)} layouts"
    if row["category"] == "texture":
        pixels = sum(c.get("pixels", 0) for _, c in counts)
        return f"{len(counts)} formats, {pixels / 1e6:.1f} Mpixel"
    parts = []
    for spec, c in counts:
        what = f"{c['objects']:,} objects" if "objects" in c else f"{fmt_mib(c.get('entry_bytes'))} entry"
        parts.append(f"{stem(spec['fixture'])} ({what})")
    return ", ".join(parts)


def summary_table(wall: dict, workloads: dict, ranges: bool) -> list[str]:
    lines = ["| operation (timer contains) | inventory | UnityPy | unityrs | speedup |", "|---|---|---|---|---|"]
    for row in summary(wall, workloads):
        x = row["speedup"]
        sx = fmt_x(x["ratio"]) + (f" ({x['low']:.2f}–{x['high']:.2f})" if ranges else "")
        lines.append(f"| {row['label']} | {inventory(row, wall, workloads)} | {fmt_s(row['unitypy']['median'])} | "
                     f"{fmt_s(row['unityrs']['median'])} | {sx} |")
    return lines


def readme_section(wall: dict | None, workloads: dict) -> str:
    """The text between the README's markers."""
    if wall is None:
        return PENDING
    m = wall["machine"]
    head = (
        f"unityrs against UnityPy {wall['tools']['unitypy']} with UnityPyBoost, the same Python script on both, on "
        f"public synthetic fixtures, on GitHub's ubuntu-24.04 runner ({m['cpu']}), commit {wall['commit'][:12]}, "
        f"{wall['date']}. Median of {wall['rounds']} rounds, each a fresh process with a warm file cache. API rows "
        "time the operation after imports; the extraction row times the whole process, imports included."
    )
    body = summary_table(wall, workloads, ranges=False)
    tail = "Breakdown, memory, unityrs-only rows and how to rerun: [BENCHMARKS.md](BENCHMARKS.md)."
    return "\n".join([head, "", *body, "", tail]) + "\n"


def splice(text: str, inner: str) -> str:
    """`text` with the part between the markers replaced by `inner`; the markers must occur once each, in order."""
    if text.count(BEGIN) != 1 or text.count(END) != 1 or text.index(BEGIN) > text.index(END):
        raise ValueError(f"README.md must hold {BEGIN} then {END}, once each")
    before, rest = text.split(BEGIN)
    _, after = rest.split(END)
    return f"{before}{BEGIN}\n{inner}{END}{after}"


def throughput(spec: dict, fig: dict) -> str:
    c, s = fig["counts"], fig["median"]
    mb = c.get("source_bytes", 0) / 1e6 / s
    own = ""
    if "objects" in c:
        own = f", {c['objects'] / s:,.0f} objects/s"
    elif "pixels" in c:
        own = f", {c['pixels'] / 1e6 / s:,.1f} Mpixel/s"
    elif "entry_bytes" in c:
        own = f", {c['entry_bytes'] / 2**20 / s:,.0f} MiB/s decoded"
    return f"{mb:,.1f} MB/s{own}"


def figure_cells(spec: dict, fig: dict | None) -> str:
    if fig is None:
        return "failed | — | — | —"
    return (f"{fmt_s(fig['median'])} ({fmt_s(fig['min'])}–{fmt_s(fig['max'])}) | {fmt_s(fig['cpu_s'])} | "
            f"{fmt_mib(fig['peak_pss_bytes'])} | {throughput(spec, fig)}")


def benchmarks_md(wall: dict, gate: dict | None, workloads: dict, fixtures: dict) -> str:
    """All of BENCHMARKS.md (plan section 6)."""
    t, m = wall["tools"], wall["machine"]
    pairs = pairings(wall, workloads)
    out = [
        "# Benchmarks",
        "",
        "Generated by `python tools/bench/bench.py render` from `benchmarks/wall.json` and `benchmarks/gate.json`;",
        "do not edit by hand. The README's summary is rendered from the same files.",
        "",
        "## 1. How to rerun",
        "",
        "```sh",
        "scripts/dev python tools/bench/bench.py run --out DIR           # the full comparison",
        "scripts/dev python tools/bench/bench.py run --out DIR --quick   # gate scale, 3 rounds",
        "```",
        "",
        "The published numbers come only from the `bench-wall.yml` workflow on GitHub's ubuntu-24.04 runner;",
        "a run elsewhere is for comparison, and an emulated host's is never published.",
        "",
        "## 2. This run",
        "",
        f"- Run {wall['run_id']}, attempt {wall['run_attempt']}, commit {wall['commit']}, {wall['date']}.",
        f"- Machine: {m['cpu']}, {m['cores']} cores, {fmt_mib(m['memory_bytes'])}, kernel {m['kernel']}, "
        f"runner image {m['runner_image']}.",
        f"- unityrs {t['unityrs']}; {t['rustc'].splitlines()[0]}; UnityPy {t['unitypy']} with UnityPyBoost "
        f"(sha256 {t['unitypyboost_sha256'][:16]}…); Pillow {t['pillow']}; Python {t['python']}.",
        f"- Fixtures: the {wall['scale']} scale, manifest sha256 {wall['fixtures_sha256'][:16]}….",
        "",
        "## 3. Inputs",
        "",
        "| file | bytes | sha256 | inventory |",
        "|---|---|---|---|",
    ]
    manifest = fixtures["scales"][wall["scale"]]
    for f in manifest["files"]:
        inv = manifest["inventories"][f["name"]]
        keys = ("objects", "textures", "width", "pixels", "entry_bytes", "blocks")
        text = ", ".join(f"{k} {inv[k]:,}" for k in keys if k in inv)
        mix = inv.get("block_mix_each") or {}
        if mix:
            text += "; blocks each: " + ", ".join(f"{k} {v:,}" for k, v in sorted(mix.items()))
        out.append(f"| `{f['name']}` | {f['bytes']:,} | `{f['sha256'][:16]}` | {text} |")
    out += ["", "## 4. Summary", "", *summary_table(wall, workloads, ranges=True), ""]
    out += ["## 5. By category", ""]
    specs = specs_by_name(workloads)
    for cat in workloads["categories"]:
        names = [n for n, (ok, _) in pairs.items() if ok and specs[n]["category"] == cat["name"]]
        if not names:
            continue
        out += [f"### {cat['label']}", "",
                "| workload | timer | impl | median (min–max) | process CPU | peak PSS | throughput | speedup |",
                "|---|---|---|---|---|---|---|---|"]
        for n in names:
            figs = workload_figures(wall, n)
            x = speedup(times(entries(wall, n)["unitypy"]), times(entries(wall, n)["unityrs"]))
            for impl, label in (("unitypy", "UnityPy"), ("unityrs", "unityrs wheel")):
                sx = f"{fmt_x(x['ratio'])} ({x['low']:.2f}–{x['high']:.2f})" if impl == "unityrs" else ""
                out.append(f"| {n} | {specs[n]['timer']} | {label} | {figure_cells(specs[n], figs[impl])} | {sx} |")
        out.append("")
    out += ["## 6. unityrs-only rows", "",
            "Unpaired workloads: no speedup is published for them (decision 4).", "",
            "| workload | reason | unityrs wheel |", "|---|---|---|"]
    for n, (ok, reason) in pairs.items():
        if not ok:
            fig = workload_figures(wall, n).get("unityrs")
            out.append(f"| {n} | {reason} | {fmt_s(fig['median']) if fig else 'failed'} |")
    out += ["", "## 7. The gate", ""]
    if gate is None:
        out.append("No instruction-count baselines are recorded yet: the gate lands with BN-2.")
    else:
        out += ["| workload | mode | ir_own | ir_libc | digest |", "|---|---|---|---|---|"]
        for n, g in sorted(gate["workloads"].items()):
            out.append(f"| {n} | {g['mode']} | {g['ir_own']:,} | {g['ir_libc']:,} | `{str(g['digest'])[:16]}` |")
        out += ["", "A refresh changes `benchmarks/gate.json` in the same PR; the job's summary shows each delta."]
    out += [
        "",
        "## 8. Method and caveats",
        "",
        f"- Each round runs every workload once per implementation, each run a fresh process; {wall['warmup']} "
        f"warm-up round, then {wall['rounds']} measured rounds, the implementations' order alternating by round.",
        "- The page cache is warm: every input is read once before each round.",
        "- The timer covers only the scope named in each row; setup and the digest are outside it.",
        "- A speedup is published only for a paired workload: Tier A in PARITY.md, every round `ok` on both sides,",
        "  and one digest across all of them.",
        "- Process CPU is the whole run's (imports and setup included); peak PSS is the summed proportional set size",
        "  of the run's process tree, sampled every 50 ms.",
        "- Wall clock on a shared runner is noisy; CI gates on instruction counts instead (section 7).",
        "",
    ]
    return "\n".join(out)
