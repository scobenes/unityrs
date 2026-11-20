"""The instruction-count gate's measurement and verdict (docs/plans/bench.md decisions 6 and 7, sections 4.2 and 5.1).

**A run.** `measure` runs `unityrs-bench run WORKLOAD FIXTURE` once under callgrind (valgrind 3.19.0 from
`install_valgrind.sh`), with `env -i` and the counting only inside the workload's scope:
`--collect-atstart=no --toggle-collect='*unityrs_bench::work::run_<name>*'`. It reads the output file (`parse`) and
rejects the run (`scope_problems`) when the scope was entered other than exactly once, its inclusive `Ir` is under
10,000, another `run_*` function was entered, or collection was on outside the scope (the file's total exceeds the
scope's inclusive cost). Of an accepted run it records (`entry`):

- `ir_own`: the `Ir` of the bench binary's own object (the object holding the scope), the gated count;
- `ir_libc`: libc's, recorded, never gated (glibc's malloc varies with addresses, section 8);
- `allocs`: the calls of `malloc`, `calloc`, `realloc` and `free` from the bench binary's frames;
- `alloc_bytes`: what the binary's counting allocator says the scope asked for;
- `digest` and `calls` (the scope's entries, 1).

**The verdict.** `compare` holds the measured counts to the PR's own `gate.json` under the policy of the base
branch's: each workload's mode (`calibrating` or `enforcing`; one the base lacks is `calibrating`) and the tolerance
(decision 7, so a PR cannot loosen its own gate). An enforcing workload fails on any gated count (`ir_own`, each of
`allocs`, `alloc_bytes`) outside the two-sided band or on a digest change, with "out of band: refresh the
baseline"; a calibrating one only reports. A workload missing from the PR's `gate.json`, a run rejected or failed,
and a fingerprint differing from the PR's fail; while every workload is calibrating, a fingerprint difference is
reported instead, as there is no enforced count for it to guard. `summary` is the job summary: base, PR and
measured counts for every workload, with each delta.

**ASLR.** `aslr_probe` runs one workload a few times with `setarch -R` and without, and reports whether libc's count
repeats with ASLR off (section 8's open question); where `setarch -R` is refused (a container's seccomp profile), it
says so. It never fails.

Standard library only; children run through tools/parity/child.py.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "tools" / "parity"))

import child  # noqa: E402
import scratch  # noqa: E402

SCHEMA_VERSION = 1
TOLERANCE_PCT = 1.0
MODES = ("calibrating", "enforcing")
ALLOCS = ("malloc", "calloc", "realloc", "free")
FINGERPRINT = ("valgrind_deb_sha256", "container", "rustc", "gcc", "ld", "libc", "target", "guest_cpu")
ENTRY = ("mode", "ir_own", "ir_libc", "allocs", "alloc_bytes", "digest", "calls")
MIN_IR = 10_000
SCOPE_PREFIX = "unityrs_bench::work::run_"
RUN_FN = re.compile(r"unityrs_bench::work::run_[A-Za-z0-9_]+")
HASH = re.compile(r"::h[0-9a-f]{16}$")
REFRESH = "out of band: refresh the baseline"


def scope_name(workload: str) -> str:
    """The scope's function name, as `unityrs_bench::work::scope_name` makes it."""
    return "run_" + re.sub(r"[^a-z0-9]", "_", workload.lower())


def toggle(workload: str) -> str:
    return f"*unityrs_bench::work::{scope_name(workload)}*"


# Parsing


class Profile:
    """What `parse` reads of a callgrind output file: each function's own cost, by (object, name); each call's
    count and inclusive cost, by caller and callee; and the file's total."""

    def __init__(self):
        self.own: dict[tuple[str, str], int] = {}
        self.calls: list[tuple[str, str, str, str, int, int]] = []  # caller ob, fn, callee ob, fn, count, cost
        self.totals: int | None = None
        self.names: set[str] = set()

    def objects(self) -> dict[str, int]:
        """Own `Ir` by object."""
        out: dict[str, int] = {}
        for (ob, _), ir in self.own.items():
            out[ob] = out.get(ob, 0) + ir
        return out

    def entered(self, fn: str) -> int:
        return sum(n for _, _, _, callee, n, _ in self.calls if callee == fn)

    def inclusive(self, fn: str) -> int:
        own = sum(ir for (_, name), ir in self.own.items() if name == fn)
        return own + sum(cost for _, caller, _, callee, _, cost in self.calls if caller == fn and callee != fn)

    def object_of(self, fn: str) -> str | None:
        found = {ob for ob, name in self.own if name == fn}
        found |= {ob for ob, caller, *_ in self.calls if caller == fn}
        return found.pop() if len(found) == 1 else None


def clean(name: str) -> str:
    return HASH.sub("", name)


class ParseError(ValueError):
    pass


def parse(text: str) -> Profile:
    """A callgrind output file (format version 1, as callgrind 3.19 writes it): name compression (`(id) name`
    defines, `(id)` refers), `ob`/`fn` and their call forms, `calls=` lines whose next cost line is the call's
    inclusive cost, relative positions, and `Ir` wherever `events:` puts it. A cost line without its `Ir` field, or
own costs that do not sum to `totals:`, refuse the file: a count is never read as zero."""
    prof = Profile()
    tables: dict[str, dict[str, str]] = {"ob": {}, "fl": {}, "fn": {}}
    kind = {"ob": "ob", "cob": "ob", "fl": "fl", "fi": "fl", "fe": "fl", "cfi": "fl", "cfl": "fl",
            "fn": "fn", "cfn": "fn"}
    ir_at = 0
    ob = fn = None
    cob = cfn = None
    pending = None  # the count of a `calls=` line, until its cost line
    for lineno, line in enumerate(text.splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        head = line[0]
        if head.isdigit() or head in "+-*":
            fields = line.split()
            costs = fields[1:]
            if len(costs) <= ir_at:
                raise ParseError(f"line {lineno}: a cost line with no Ir field")
            ir = int(costs[ir_at])
            if fn is None:
                raise ParseError(f"line {lineno}: a cost before any fn=")
            if pending is not None:
                if cfn is None:
                    raise ParseError(f"line {lineno}: calls= without cfn=")
                prof.calls.append((ob, fn, cob or ob, cfn, pending, ir))
                pending = cob = cfn = None
            else:
                prof.own[(ob, fn)] = prof.own.get((ob, fn), 0) + ir
            continue
        key, sep, value = line.partition("=")
        if sep and key in kind:
            table = tables[kind[key]]
            m = re.fullmatch(r"\((\d+)\)(?: (.*))?", value)
            if m:
                if m.group(2) is not None:
                    table[m.group(1)] = m.group(2)
                elif m.group(1) not in table:
                    raise ParseError(f"line {lineno}: {key}=({m.group(1)}) before its name")
                value = table[m.group(1)]
            if kind[key] == "fn":
                value = clean(value)
                prof.names.add(value)
            if key == "ob":
                ob = value
            elif key == "fn":
                fn = value
            elif key == "cob":
                cob = value
            elif key == "cfn":
                cfn = value
            continue
        if sep and key == "calls":
            pending = int(value.split()[0])
            continue
        if sep and key in ("jump", "jcnd"):
            raise ParseError(f"line {lineno}: jump records (--collect-jumps) are not read")
        key, colon, value = line.partition(":")
        if colon and key == "events":
            names = value.split()
            if "Ir" not in names:
                raise ParseError(f"line {lineno}: no Ir event")
            ir_at = names.index("Ir")
        elif colon and key == "totals":
            prof.totals = int(value.split()[ir_at])
    if prof.totals is None:
        raise ParseError("no totals: line (an unfinished file)")
    own = sum(prof.own.values())
    if own != prof.totals:
        raise ParseError(f"the functions' own costs sum to {own:,}, not the file's total {prof.totals:,}")
    return prof


# Scope checks and the entry


def scope_problems(prof: Profile, workload: str) -> list[str]:
    """Why the run of `workload` is not its scope alone (plan section 4.2); empty when it is."""
    scope = SCOPE_PREFIX + scope_name(workload)[len("run_"):]
    out = []
    if scope not in prof.names:
        return [f"{scope} was never entered"]
    entered = prof.entered(scope)
    if entered != 1:
        out.append(f"{scope} was entered {entered} times, not once")
    inclusive = prof.inclusive(scope)
    if inclusive < MIN_IR:
        out.append(f"{scope}'s inclusive Ir is {inclusive:,}, under {MIN_IR:,}")
    others = sorted(n for n in prof.names if RUN_FN.fullmatch(n) and n != scope)
    if others:
        out.append(f"another scope was entered: {', '.join(others)}")
    if prof.totals != inclusive:
        out.append(f"collection was on outside the scope: the file's total is {prof.totals:,}, "
                   f"the scope's {inclusive:,}")
    return out


def is_libc(ob: str) -> bool:
    return re.fullmatch(r"libc(-[0-9.]+)?\.so(\.[0-9]+)*", ob.rsplit("/", 1)[-1]) is not None


def counts(prof: Profile, workload: str) -> dict:
    """`ir_own`, `ir_libc` and `allocs` of an accepted run, and `calls`."""
    scope = SCOPE_PREFIX + scope_name(workload)[len("run_"):]
    own = prof.object_of(scope)
    by_object = prof.objects()
    allocs = dict.fromkeys(ALLOCS, 0)
    for caller_ob, _, _, callee, n, _ in prof.calls:
        if caller_ob == own and callee in allocs:
            allocs[callee] += n
    return {
        "ir_own": by_object.get(own, 0),
        "ir_libc": sum(ir for ob, ir in by_object.items() if is_libc(ob)),
        "allocs": allocs,
        "calls": prof.entered(scope),
    }


# Running


def valgrind_cmd(prefix: Path, workload: str, out: Path) -> list[str]:
    return [str(prefix / "usr" / "bin" / "valgrind"), "--tool=callgrind", "--collect-atstart=no",
            f"--toggle-collect={toggle(workload)}", f"--callgrind-out-file={out}"]


def run_once(binary: Path, prefix: Path, workload: str, fixture: Path, folder: Path, setarch: bool = False) -> dict:
    """One callgrind run of `workload`: its counts and the binary's result line, or `problems`."""
    out = folder / f"callgrind.{scope_name(workload)}.out"
    out.unlink(missing_ok=True)
    cmd = [*valgrind_cmd(prefix, workload, out), str(binary), "run", workload, str(fixture)]
    if setarch:
        cmd = ["setarch", "-R", *cmd]
    env = {"VALGRIND_LIB": str(prefix / "usr" / "libexec" / "valgrind"), scratch.VAR: ""}
    proc = child.run(cmd, env=env, capture_output=True, text=True, once=True)
    if proc.returncode != 0:
        tail = " | ".join(proc.stderr.strip().splitlines()[-3:])
        return {"problems": [f"exit {proc.returncode}: {tail}"]}
    try:
        line = json.loads(proc.stdout.strip().splitlines()[-1])
        prof = parse(out.read_text(encoding="utf-8"))
    except (IndexError, ValueError, OSError) as e:
        return {"problems": [f"unreadable result: {e}"]}
    bad = scope_problems(prof, workload)
    if line.get("status") != "ok":
        bad.append(f"status {line.get('status')!r}")
    if line.get("scopes") != 1:
        bad.append(f"the process entered {line.get('scopes')!r} scopes, not 1 (setup and digest included)")
    got = {"problems": bad, "digest": line.get("digest"), "alloc_bytes": line.get("alloc_bytes"),
           "guest_cpu": line.get("cpu")}
    if not bad:
        got.update(counts(prof, workload))
        if got["ir_own"] == 0:
            got["problems"] = ["ir_own is 0: no instruction of the bench binary's own was counted"]
    return got


def entry(got: dict, mode: str) -> dict:
    """A gate.json workload entry from an accepted run."""
    return {"mode": mode, "ir_own": got["ir_own"], "ir_libc": got["ir_libc"], "allocs": got["allocs"],
            "alloc_bytes": got["alloc_bytes"], "digest": got["digest"], "calls": got["calls"]}


def rustc_vv() -> str:
    """`rustc -vV`, the toolchain the fingerprint names."""
    return child.run(["rustc", "-vV"], cwd=HERE.parents[1], capture_output=True, text=True,
                     check=True).stdout.strip()


def first_line(cmd: list[str]) -> str | None:
    """The first line `cmd` prints, or None when it cannot run."""
    try:
        proc = child.run(cmd, capture_output=True, text=True)
    except OSError:
        return None
    lines = (proc.stdout or "").strip().splitlines()
    return lines[0].strip() if proc.returncode == 0 and lines else None


def toolchain() -> dict:
    """The C toolchain and libc the bench binary is built and run with, exactly: gcc, which compiles the C the
    binary links (liblz4, liblzma) and links it; the linker; libc. Each from its Debian package's version where
    dpkg knows it, else the tool's own banner."""
    def pkg(name: str) -> str | None:
        return first_line(["dpkg-query", "-W", "-f=${Version}\n", name])

    gcc, ld, libc = pkg("gcc-12"), pkg("binutils"), pkg("libc6")
    return {
        "gcc": f"gcc-12 {gcc}" if gcc else first_line(["cc", "--version"]),
        "ld": f"binutils {ld}" if ld else first_line(["ld", "--version"]),
        "libc": f"libc6 {libc}" if libc else first_line(["ldd", "--version"]),
    }


def fingerprint(prefix: Path, container: str, guest_cpu: str | None, rustc: str, tools: dict) -> dict:
    """What the counts depend on beside the code (plan section 4.5): the valgrind deb (`install_valgrind.sh`
    records its sha256 in PREFIX), the container, the toolchains (`rustc -vV` and `toolchain`) and the target,
    and the CPU class the guest sees (valgrind presents its own CPUID, by the host's capabilities)."""
    target = next((ln.split(":", 1)[1].strip() for ln in rustc.splitlines() if ln.startswith("host:")), None)
    deb = prefix / ".deb-sha256"
    return {"valgrind_deb_sha256": deb.read_text(encoding="utf-8").strip() if deb.exists() else None,
            "container": container, "rustc": rustc, "gcc": tools.get("gcc"), "ld": tools.get("ld"),
            "libc": tools.get("libc"), "target": target, "guest_cpu": guest_cpu}


def measure(binary: Path, prefix: Path, fixtures: Path, specs: list, modes: dict, container: str,
            folder: Path, rustc: str, tools: dict, say=print) -> tuple[dict, dict]:
    """Every workload of `specs` once: (measured gate.json, problems by workload). A run's mode is `modes`'s
    (the PR's own gate.json's), else calibrating."""
    workloads, problems, cpu = {}, {}, set()
    for spec in specs:
        got = run_once(binary, prefix, spec["name"], fixtures / spec["fixture"], folder)
        cpu.add(got.get("guest_cpu"))
        if got["problems"]:
            problems[spec["name"]] = got["problems"]
            say(f"gate: {spec['name']}: rejected: {'; '.join(got['problems'])}")
            continue
        workloads[spec["name"]] = entry(got, modes.get(spec["name"], "calibrating"))
        say(f"gate: {spec['name']}: ir_own {got['ir_own']:,}, ir_libc {got['ir_libc']:,}, "
            f"allocs {sum(got['allocs'].values()):,}, alloc_bytes {got['alloc_bytes']:,}")
    if len(cpu) > 1:
        problems["(fingerprint)"] = [f"the guest CPU differed between runs: {sorted(map(str, cpu))}"]
    doc = {"schema_version": SCHEMA_VERSION, "tolerance_pct": TOLERANCE_PCT,
           "fingerprint": fingerprint(prefix, container, next(iter(cpu)) if len(cpu) == 1 else None, rustc,
                                  tools),
           "workloads": dict(sorted(workloads.items()))}
    return doc, problems


def aslr_probe(binary: Path, prefix: Path, fixtures: Path, spec: dict, runs: int, folder: Path) -> list[str]:
    """Markdown lines: `ir_libc` and `ir_own` over `runs` runs with ASLR on and with it off (`setarch -R`), and
    whether libc's count repeats with it off."""
    out = [f"### libc under ASLR off ({spec['name']}, {runs} runs each)", ""]
    probe = child.run(["setarch", "-R", "true"], capture_output=True, text=True)
    if probe.returncode != 0:
        why = (probe.stderr.strip().splitlines() or ["refused"])[-1]
        return [*out, f"`setarch -R` is refused here ({why}): not measured.", ""]
    out += ["| ASLR | ir_libc per run | repeats | ir_own per run |", "|---|---|---|---|"]
    for label, off in (("on", False), ("off (setarch -R)", True)):
        got = [run_once(binary, prefix, spec["name"], fixtures / spec["fixture"], folder, off) for _ in range(runs)]
        bad = [p for g in got for p in g["problems"]]
        if bad:
            out.append(f"| {label} | rejected: {bad[0]} | — | — |")
            continue
        libc, own = [g["ir_libc"] for g in got], [g["ir_own"] for g in got]
        out.append(f"| {label} | {', '.join(f'{v:,}' for v in libc)} | {'yes' if len(set(libc)) == 1 else 'no'} | "
                   f"{', '.join(f'{v:,}' for v in own)} |")
    return [*out, ""]


# Validation and the verdict


def gate_problems(doc) -> list[str]:
    """Why `doc` is not a gate.json (plan section 4.5); empty when it is."""
    if not isinstance(doc, dict) or set(doc) != {"schema_version", "tolerance_pct", "fingerprint", "workloads"}:
        return ["not exactly schema_version, tolerance_pct, fingerprint and workloads"]
    out = []
    if doc["schema_version"] != SCHEMA_VERSION:
        out.append(f"schema_version {doc['schema_version']!r}, not {SCHEMA_VERSION}")
    tol = doc["tolerance_pct"]
    if not isinstance(tol, (int, float)) or isinstance(tol, bool) or not 0 <= tol <= 100:
        out.append("tolerance_pct is not a percentage")
    if not isinstance(doc["fingerprint"], dict) or set(doc["fingerprint"]) != set(FINGERPRINT):
        out.append(f"fingerprint is not exactly {', '.join(FINGERPRINT)}")
    if not isinstance(doc["workloads"], dict):
        return [*out, "workloads is not a map"]
    for name, e in doc["workloads"].items():
        if not isinstance(e, dict) or set(e) != set(ENTRY):
            out.append(f"{name}: not exactly {', '.join(ENTRY)}")
            continue
        if e["mode"] not in MODES:
            out.append(f"{name}: mode {e['mode']!r}")
        ints = [e["ir_own"], e["ir_libc"], e["alloc_bytes"], e["calls"]]
        allocs = e["allocs"]
        if not isinstance(allocs, dict) or set(allocs) != set(ALLOCS):
            out.append(f"{name}: allocs is not exactly {', '.join(ALLOCS)}")
        else:
            ints += list(allocs.values())
        if not all(isinstance(v, int) and not isinstance(v, bool) and v >= 0 for v in ints):
            out.append(f"{name}: a count is not a non-negative int")
        elif e["ir_own"] == 0:
            out.append(f"{name}: ir_own is 0")
        if not (isinstance(e["digest"], str) and re.fullmatch(r"[0-9a-f]{64}", e["digest"])):
            out.append(f"{name}: digest is not 64 hex digits")
    return out


def metrics(e: dict | None) -> dict:
    """The gated counts of an entry, by label."""
    if e is None:
        return {}
    return {"ir_own": e["ir_own"], **{f"allocs.{k}": e["allocs"][k] for k in ALLOCS}, "alloc_bytes": e["alloc_bytes"]}


def in_band(recorded: int, measured: int, tolerance_pct: float) -> bool:
    """Two-sided: |measured - recorded| within tolerance_pct of recorded (exact for a recorded 0)."""
    return abs(measured - recorded) * 100 <= tolerance_pct * recorded


def delta(recorded: int | None, measured: int | None) -> str:
    if recorded is None or measured is None:
        return "—"
    if recorded == 0:
        return "0" if measured == 0 else "new"
    return f"{(measured - recorded) * 100 / recorded:+.3f}%"


def compare(base: dict | None, pr: dict, measured: dict, run_problems: dict, names: list[str]) -> tuple[list, list]:
    """(rows, failures): every workload's verdict under the base branch's policy (module docs). `names` is the
    workload table's order; `run_problems` maps a workload (or "(fingerprint)") to why its run was rejected. The
    verdict covers the base's enforcing workloads too: one the PR drops from its table or its gate.json fails, and
    so does an expected workload with no measured row."""
    base_w = (base or {}).get("workloads", {})
    tol = (base or {}).get("tolerance_pct", TOLERANCE_PCT)
    rows, fails = [], []
    modes = {n: base_w.get(n, {}).get("mode", "calibrating") for n in names}
    fails += [f"{n}: enforcing in the base's gate.json but not a workload" for n, e in base_w.items()
              if e.get("mode") == "enforcing" and n not in names]
    fails += [f"{n}: in the PR's gate.json but not a workload" for n in pr["workloads"] if n not in names]
    fails += [f"(fingerprint): {p}" for p in run_problems.get("(fingerprint)", [])]
    if pr["fingerprint"] != measured["fingerprint"]:
        differ = [k for k in FINGERPRINT if pr["fingerprint"].get(k) != measured["fingerprint"].get(k)]
        what = f"the fingerprint differs from the PR's gate.json in {', '.join(differ)}"
        if any(m == "enforcing" for m in modes.values()):
            fails.append(f"{what}: refresh the baseline")
        else:
            rows.append(("(fingerprint)", "calibrating", "fingerprint", "", "", "", "", "", f"reported: {what}"))
    for n in names:
        mode, want, got = modes[n], pr["workloads"].get(n), measured["workloads"].get(n)
        if want is None:
            fails.append(f"{n}: missing from the PR's gate.json")
        if n in run_problems:
            fails += [f"{n}: {p}" for p in run_problems[n]]
            rows.append((n, mode, "run", "", "", "", "", "", "rejected: " + "; ".join(run_problems[n])))
            continue
        if got is None:
            fails.append(f"{n}: not measured")
            rows.append((n, mode, "run", "", "", "", "", "", "not measured"))
        if want is None or got is None:
            continue
        old = metrics(base_w.get(n))
        for label, value in metrics(got).items():
            recorded = metrics(want)[label]
            ok = in_band(recorded, value, tol)
            verdict = "ok" if ok else (REFRESH if mode == "enforcing" else "out of band (calibrating: reported)")
            if not ok and mode == "enforcing":
                fails.append(f"{n}: {label} {value:,} against {recorded:,} (±{tol}%): {REFRESH}")
            rows.append((n, mode, label, fmt(old.get(label)), fmt(recorded), fmt(value), delta(recorded, value),
                         delta(old.get(label), value), verdict))
        if got["digest"] != want["digest"]:
            if mode == "enforcing":
                fails.append(f"{n}: the digest changed: {REFRESH}")
            rows.append((n, mode, "digest", "", want["digest"][:16], got["digest"][:16], "", "",
                         REFRESH if mode == "enforcing" else "changed (calibrating: reported)"))
    return rows, fails


def fmt(v: int | None) -> str:
    return "—" if v is None else f"{v:,}"


def summary(rows: list, fails: list, tol: float, base_found: bool, warnings: list | None = None) -> str:
    head = ["## Instruction-count gate", ""]
    if warnings:
        head += ["> [!WARNING]",
                 "> **This change edits the gate's own checker or policy.** The job runs the PR's code, so the base",
                 "> branch's policy is applied by a checker the PR controls: review these edits as the gate itself.",
                 *[f"> - {w}" for w in warnings], ""]
    head += [f"Policy from the base branch's gate.json{'' if base_found else ' (none there: every workload calibrating)'}"
             f"; tolerance ±{tol}%. Counts compared against the PR's own gate.json; the measured file is this run's "
             "artifact.", ""]
    head += ["**Failed:**", "", *[f"- {f}" for f in fails], ""] if fails else ["**Passed.**", ""]
    table = ["| workload | mode | count | base | PR | measured | Δ vs PR | Δ vs base | verdict |",
             "|---|---|---|---|---|---|---|---|---|"]
    table += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join([*head, *table, ""])


def spread(values: list[int]) -> float:
    """(max - min) / min, in percent."""
    return (max(values) - min(values)) * 100 / min(values) if values and min(values) else 0.0


def load(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))
