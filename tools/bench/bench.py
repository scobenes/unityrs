#!/usr/bin/env python3
"""The benchmark suite's driver: unityrs against UnityPy (docs/plans/bench.md section 4.4).

    python tools/bench/bench.py gen --scale gate|full --out DIR [--update]
    scripts/dev python tools/bench/bench.py run --out DIR [--quick] [--workload NAME ...]
    python tools/bench/bench.py render [--check]
    python tools/bench/bench.py check --wall-run [RUN_ID] --base SHA --repo OWNER/REPO

- `gen` writes the fixtures with `unityrs-bench gen` (built with cargo, or `$UNITYRS_BENCH_BIN`) and holds them to
  `benchmarks/fixtures.json`, re-hashing every file; `--update` records both scales there instead.
- `run` times both implementations on the fixtures (cached under `$UNITYRS_BENCH_CACHE`, else under DIR), round by
  round, each run a fresh process (`py_workloads.py`), and writes DIR/wall.json. It refuses any stack but UnityPy
  1.25.2 with the pinned UnityPyBoost, and marks an emulated host, which `render` then refuses.
- `render` writes the README's section between its markers and BENCHMARKS.md from `benchmarks/wall.json` and
  `benchmarks/gate.json`; without wall.json the section reads "Results pending" and BENCHMARKS.md does not exist.
  `--check` fails when the committed text differs.
- `check --wall-run` (CI) verifies a committed wall.json the PR changes against the named `bench-wall.yml` run:
  its workflow, event, branch (main) and conclusion, no change to the measured paths (`measured_paths`: the
  workflow, the build configuration, the inputs and every repository module the measurement imports at the run's
  commit or in the checkout) between its commit and the base or the head, and the run's artifact byte-equal to
  the file. A PR that leaves wall.json alone has nothing to verify.

`run` fails, writing no wall.json, when a child crashes or is killed, its setup fails or it prints no valid
result; children's stderr goes only to DIR/logs, never to the console or wall.json (which `run` uploads alone).

Standard library only; children run through tools/parity/child.py, scratch through tools/parity/scratch.py.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import io
import json
import os
import platform
import posixpath
import resource
import sys
import tarfile
import time
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
WORKFLOW = ".github/workflows/bench-wall.yml"
sys.path.insert(0, str(ROOT / "tools" / "parity"))
sys.path.insert(0, str(ROOT / "tools" / "perf"))
sys.path.insert(0, str(HERE))

import child  # noqa: E402
import results  # noqa: E402
import scratch  # noqa: E402

FIXTURES = ROOT / "benchmarks" / "fixtures.json"
WALL = ROOT / "benchmarks" / "wall.json"
GATE = ROOT / "benchmarks" / "gate.json"
README = ROOT / "README.md"
BENCHMARKS = ROOT / "BENCHMARKS.md"
WORKLOADS = HERE / "workloads.json"
# The paths a published run measured (plan section 5.2), beside the modules `measured_paths` derives: a change to
# any between the run's commit and the PR's base or head means the numbers are not this code's. `crates/` holds
# the wheel's build configuration (its Cargo.toml and pyproject.toml); `.cargo/` would change any build.
MEASURED = ("crates/", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/", "tools/bench/",
            "tools/parity/requirements-oracle.txt", "benchmarks/fixtures.json", WORKFLOW)
# The measurement's entry points, and where its own modules are found: the folders bench.py and py_workloads.py
# put on sys.path (repository-relative).
ENTRIES = ("tools/bench/bench.py", "tools/bench/py_workloads.py")
IMPORT_DIRS = ("tools/bench", "tools/parity", "tools/perf")


def local_imports(entries, sources: dict, dirs=IMPORT_DIRS) -> set[str]:
    """Every repository file `entries` import, transitively, read from `sources` (repository-relative path ->
    Python source): each `import` or absolute `from` at any depth (a function's included) whose top name is a
    module beside the importer or in `dirs`; a package counts whole, as its folder (`name/`). Names found nowhere
    there (the standard library, UnityPy, unityrs) are not ours."""
    seen, todo = set(), [e for e in entries if e in sources]
    while todo:
        path = todo.pop()
        if path in seen:
            continue
        seen.add(path)
        if path.endswith("/"):
            todo += sorted(f for f in sources if f.startswith(path))
            continue
        for node in ast.walk(ast.parse(sources[path])):
            if isinstance(node, ast.Import):
                names = [a.name for a in node.names]
            elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                names = [node.module]
            else:
                continue
            for top in {n.split(".")[0] for n in names}:
                for d in (posixpath.dirname(path), *dirs):
                    if f"{d}/{top}.py" in sources:
                        todo.append(f"{d}/{top}.py")
                    elif f"{d}/{top}/__init__.py" in sources:
                        todo.append(f"{d}/{top}/")
                    else:
                        continue
                    break
    return seen


def checkout_sources(root: Path = ROOT) -> dict:
    """The checkout's Python sources under tools/, by repository-relative path."""
    return {p.relative_to(root).as_posix(): p.read_text(encoding="utf-8")
            for p in sorted((root / "tools").rglob("*.py")) if "__pycache__" not in p.parts}


def commit_sources(sha: str) -> dict:
    """Commit `sha`'s Python sources under tools/, read from `git archive` without writing them out."""
    data = child.run(["git", "archive", "--format=tar", sha, "tools"], cwd=ROOT, capture_output=True,
                     check=True).stdout
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        return {m.name: tar.extractfile(m).read().decode("utf-8") for m in tar.getmembers()
                if m.isfile() and m.name.endswith(".py")}


def measured_paths(*trees: dict) -> list[str]:
    """`MEASURED` and every repository module the measurement imports in any of `trees` (the checkout's sources
    when none), unioned, as pathspecs: a module under a listed folder is already covered. The run's commit and
    the checkout are both read, so a module only one of them imports (or holds) is measured either way."""
    found = set().union(*(local_imports(ENTRIES, t) for t in (trees or (checkout_sources(),))))
    covered = [m for m in MEASURED if m.endswith("/")]
    return [*MEASURED, *sorted(r for r in found if not any(r.startswith(m) for m in covered))]


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def dump_json(value) -> str:
    return json.dumps(value, indent=1, ensure_ascii=False) + "\n"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


# gen


def bench_binary() -> Path:
    """`unityrs-bench`: `$UNITYRS_BENCH_BIN`, else built (release, locked) into the cargo target folder."""
    given = os.environ.get("UNITYRS_BENCH_BIN")
    if given:
        return Path(given)
    child.run(["cargo", "build", "--release", "--locked", "-p", "unityrs-bench"], cwd=ROOT, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
    return target / "release" / "unityrs-bench"


def manifest_problems(folder: Path, recorded: dict) -> list[str]:
    """How `folder`'s fixtures and manifest.json differ from the recorded manifest, every file re-hashed."""
    try:
        made = load_json(folder / "manifest.json")
    except (OSError, ValueError) as e:
        return [f"manifest.json: {e}"]
    out = []
    if made != recorded:
        names = {f["name"]: f for f in recorded["files"]}
        for f in made.get("files", []):
            if names.get(f["name"]) != f:
                out.append(f"{f['name']}: {f['bytes']} bytes, sha256 {f['sha256'][:16]}… is not the recorded file")
        gone = set(names) - {f["name"] for f in made.get("files", [])}
        out += [f"{n}: recorded, not made" for n in sorted(gone)]
        out = out or ["manifest.json differs from the recorded inventories"]
    for f in recorded["files"]:
        path = folder / f["name"]
        if not path.is_file() or path.stat().st_size != f["bytes"] or sha256_file(path) != f["sha256"]:
            out.append(f"{f['name']}: the file on disk is not the recorded one")
    return out


def generate(scale: str, dest: Path) -> dict:
    """Writes the fixtures at `scale` into `dest`: their manifest."""
    dest.mkdir(parents=True, exist_ok=True)
    child.run([str(bench_binary()), "gen", "--scale", scale, str(dest)], check=True)
    return load_json(dest / "manifest.json")


def fixtures_at(scale: str, out: Path) -> Path:
    """The fixtures at `scale`, checked: the cached set when it matches the manifest, else made again."""
    cache = os.environ.get("UNITYRS_BENCH_CACHE")
    folder = (Path(cache) if cache else out / "fixtures") / scale
    recorded = load_json(FIXTURES)["scales"][scale]
    if manifest_problems(folder, recorded):
        generate(scale, folder)
    bad = manifest_problems(folder, recorded)
    if bad:
        raise SystemExit("bench: the fixtures are not benchmarks/fixtures.json's:\n  " + "\n  ".join(bad))
    return folder


def cmd_gen(args) -> int:
    if args.update:
        with scratch.folder("unityrs-bench-") as tmp:
            scales = {s: generate(s, Path(tmp) / s) for s in ("full", "gate")}
        FIXTURES.parent.mkdir(exist_ok=True)
        FIXTURES.write_text(dump_json({"schema_version": 1, "scales": scales}), encoding="utf-8")
        print(f"bench: recorded both scales in {FIXTURES.relative_to(ROOT)}")
        return 0
    if not args.out:
        raise SystemExit("bench gen: --out DIR, or --update")
    generate(args.scale, args.out)
    bad = manifest_problems(args.out, load_json(FIXTURES)["scales"][args.scale])
    for b in bad:
        print(f"bench gen: {b}", file=sys.stderr)
    return 1 if bad else 0


# run


def stack_info() -> tuple[dict, str | None]:
    """The tool versions wall.json records, and why this is not the pinned stack (or None)."""
    import importlib.metadata

    import bench_unitypy  # imports UnityPy, with UnityPyBoost when it is installed
    import PIL
    import unitypy_dump

    rustc = child.run(["rustc", "-vV"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    try:
        unityrs = importlib.metadata.version("unityrs")
    except importlib.metadata.PackageNotFoundError:
        unityrs = None
    tools = {
        "unityrs": unityrs,
        "rustc": rustc.strip(),
        "unitypy": bench_unitypy.UnityPy.__version__,
        "unitypyboost_sha256": unitypy_dump.BOOST_SHA256,
        "pillow": PIL.__version__,
        "python": platform.python_version(),
    }
    problem = bench_unitypy.stack_problem() or (None if unityrs else "the unityrs wheel is not installed")
    return tools, problem


def machine_info() -> dict:
    cpu = platform.processor() or platform.machine()
    memory = None
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                memory = int(line.split()[1]) * 1024
    except OSError:
        pass
    emulated = platform.machine() != "x86_64" or any(s in cpu for s in ("VirtualApple", "QEMU"))
    return {"cpu": cpu, "cores": os.cpu_count(), "memory_bytes": memory, "kernel": platform.release(),
            "runner_image": os.environ.get("ImageVersion"), "emulated": emulated}


class ChildFailed(Exception):
    """A child crashed, failed its setup or printed no result: the run stops (its stderr is in the log file)."""


def one_run(impl: str, spec: dict, fixture: Path, log: Path) -> dict:
    """One fresh process of `py_workloads.py`: its result, with whole-process wall and CPU time and peak PSS
    (setup and warm-up included). It runs once (`child.run`'s `once`): a kill, even from outside, is the run's
    result and fails it, as no second run may be measured in its place."""
    cmd = [sys.executable, str(HERE / "py_workloads.py"), "--impl", impl, spec["name"], str(fixture)]
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.perf_counter()
    pss = None
    if Path("/proc/self/smaps_rollup").exists():
        import mem_peak

        proc, report = mem_peak.measure(cmd, capture_output=True, text=True, once=True)
        pss = report["peak_pss_sum_bytes"]
    else:
        proc = child.run(cmd, capture_output=True, text=True, once=True)
    process_s = time.perf_counter() - t0
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = (after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)
    got = child_result(proc, f"{spec['name']} {impl}", log)
    return {**got, "process_s": process_s, "cpu_s": cpu, "peak_pss_bytes": pss}


def child_result(proc, what: str, log: Path) -> dict:
    """A finished child's result line, held to `results.result_problems` (its keys, their types, finite
    numbers); `ChildFailed` when it exited non-zero, a kill included, or printed no valid result. Its stderr,
    when any, is written to `log` and nowhere else, and nothing it printed reaches the console."""
    if proc.stderr:
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text(proc.stderr, encoding="utf-8")
    lines = (proc.stdout or "").strip().splitlines()
    try:
        got = json.loads(lines[-1], parse_constant=reject) if proc.returncode == 0 and lines else None
    except ValueError:
        got = None
    if got is None or results.result_problems(got):
        code = "no valid result" if proc.returncode == 0 else f"exit {proc.returncode}"
        raise ChildFailed(f"{what}: {code} (stderr, if any, in {log.name})")
    return got


def reject(name: str):
    raise ValueError(f"{name} is not a JSON number")


def commit_sha() -> str | None:
    if os.environ.get("GITHUB_SHA"):
        return os.environ["GITHUB_SHA"]
    proc = child.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return proc.stdout.strip() if proc.returncode == 0 else None


def env_int(name: str) -> int | None:
    value = os.environ.get(name, "")
    return int(value) if value.isdigit() else None


def cmd_run(args) -> int:
    tools, problem = stack_info()
    if problem:
        print(f"bench run: not the pinned oracle stack: {problem}", file=sys.stderr)
        return 2
    table = load_json(WORKLOADS)
    scale = "gate" if args.quick else "full"
    rounds = table["quick_rounds"] if args.quick else table["rounds"]
    specs = [w for w in table["workloads"] if not args.workload or w["name"] in args.workload]
    if not specs:
        raise SystemExit("bench run: no workload selected")
    args.out.mkdir(parents=True, exist_ok=True)
    folder = fixtures_at(scale, args.out)
    runs = {(s["name"], impl): [] for s in specs for impl in results.IMPLS}
    for rnd in range(table["warmup"] + rounds):
        for spec in specs:  # warm the page cache: every input read once before the round
            (folder / spec["fixture"]).read_bytes()
        order = results.IMPLS if rnd % 2 == 0 else tuple(reversed(results.IMPLS))
        for spec in specs:
            for impl in order:
                log = args.out / "logs" / f"{rnd}-{spec['name']}-{impl}.stderr"
                try:
                    got = one_run(impl, spec, folder / spec["fixture"], log)
                except ChildFailed as e:
                    print(f"bench run: round {rnd}: {e}; stopping, no wall.json", file=sys.stderr)
                    return 1
                print(f"bench: round {rnd} {spec['name']} {impl}: {got['status']} {got['scope_s']}", flush=True)
                if rnd >= table["warmup"]:
                    runs[(spec["name"], impl)].append({"round": rnd - table["warmup"], **got})
    wall = {
        "schema_version": results.SCHEMA_VERSION,
        "run_id": env_int("GITHUB_RUN_ID"),
        "run_attempt": env_int("GITHUB_RUN_ATTEMPT"),
        "commit": commit_sha(),
        "date": time.strftime("%Y-%m-%d", time.gmtime()),
        "scale": scale,
        "rounds": rounds,
        "warmup": table["warmup"],
        "tools": tools,
        "machine": machine_info(),
        "fixtures_sha256": hashlib.sha256((folder / "manifest.json").read_bytes()).hexdigest(),
        "workloads": [],
    }
    for spec in specs:
        for impl in results.IMPLS:
            wall["workloads"].append({
                "name": spec["name"], "impl": impl, "category": spec["category"], "timer": spec["timer"],
                "paired": None, "reason": None, "parity_row": spec["parity_row"], "tier": spec["tier"],
                "runs": runs[(spec["name"], impl)],
            })
    for name, (paired, reason) in results.pairings(wall, table).items():
        for e in wall["workloads"]:
            if e["name"] == name:
                e["paired"], e["reason"] = paired, reason
    (args.out / "wall.json").write_text(dump_json(wall), encoding="utf-8")
    bad = results.problems(wall, table)
    for b in bad:
        print(f"bench run: {b}", file=sys.stderr)
    for name, (paired, reason) in results.pairings(wall, table).items():
        print(f"bench: {name}: {'paired' if paired else 'unpaired: ' + reason}")
    return 1 if bad else 0


# render


def rendered(root: Path) -> tuple[str, str | None, list[str]]:
    """(README.md, BENCHMARKS.md or None, problems) as `render` makes them under `root`."""
    table = load_json(root / WORKLOADS.relative_to(ROOT))
    wall_path, gate_path = root / WALL.relative_to(ROOT), root / GATE.relative_to(ROOT)
    wall = load_json(wall_path) if wall_path.exists() else None
    gate = load_json(gate_path) if gate_path.exists() else None
    bad = []
    if wall is not None:
        bad = results.problems(wall, table) or results.publish_problems(wall, table)
        if bad:
            return "", None, [f"benchmarks/wall.json: {b}" for b in bad]
    readme = results.splice((root / "README.md").read_text(encoding="utf-8"), results.readme_section(wall, table))
    if wall is None:
        return readme, None, []
    fixtures = load_json(root / FIXTURES.relative_to(ROOT))
    return readme, results.benchmarks_md(wall, gate, table, fixtures), []


def render(root: Path, check: bool) -> list[str]:
    readme, benchmarks, bad = rendered(root)
    if bad:
        return bad
    target = root / "BENCHMARKS.md"
    if check:
        if readme != (root / "README.md").read_text(encoding="utf-8"):
            bad.append("README.md's benchmark section is stale: run `python tools/bench/bench.py render`")
        if benchmarks is None and target.exists():
            bad.append("BENCHMARKS.md exists without benchmarks/wall.json")
        elif benchmarks is not None and (not target.exists() or target.read_text(encoding="utf-8") != benchmarks):
            bad.append("BENCHMARKS.md is stale: run `python tools/bench/bench.py render`")
        return bad
    (root / "README.md").write_text(readme, encoding="utf-8")
    if benchmarks is None:
        target.unlink(missing_ok=True)
    else:
        target.write_text(benchmarks, encoding="utf-8")
    return []


def cmd_render(args) -> int:
    bad = render(ROOT, args.check)
    for b in bad:
        print(f"bench render: {b}", file=sys.stderr)
    return 1 if bad else 0


# check --wall-run


def git(*args: str) -> tuple[int, str]:
    proc = child.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    return proc.returncode, proc.stdout


def gh_api(path: str) -> bytes:
    return child.run(["gh", "api", path], cwd=ROOT, capture_output=True, check=True).stdout


def check_wall_run(run_id, base: str, repo: str, gh=gh_api, git=git, wall_path: Path = WALL, say=print,
                   sources_at=commit_sources) -> list[str]:
    """Why the committed wall.json is not the named run's published result; empty when it is, or when the change
    under test (`base`..HEAD) leaves wall.json alone."""
    rel = str(wall_path.relative_to(ROOT)) if wall_path.is_relative_to(ROOT) else "benchmarks/wall.json"
    rc, _ = git("diff", "--quiet", base, "HEAD", "--", rel)
    if rc == 0:
        say("bench check: wall.json is unchanged; nothing to verify")
        return []
    if not wall_path.exists():
        say("bench check: wall.json is removed; nothing to verify")
        return []
    data = wall_path.read_bytes()
    wall = json.loads(data)
    table = load_json(WORKLOADS)
    bad = results.problems(wall, table) or results.publish_problems(wall, table)
    if run_id is not None and int(run_id) != wall["run_id"]:
        bad.append(f"run {run_id} is not wall.json's run {wall['run_id']}")
    rid, attempt = wall["run_id"], wall["run_attempt"]
    run = json.loads(gh(f"repos/{repo}/actions/runs/{rid}/attempts/{attempt}"))
    if run.get("path", "").split("@")[0] != WORKFLOW:
        bad.append(f"run {rid}: workflow {run.get('path')!r}, not {WORKFLOW}")
    for key, want in (("event", "workflow_dispatch"), ("head_branch", "main"), ("conclusion", "success"),
                      ("head_sha", wall["commit"])):
        if run.get(key) != want:
            bad.append(f"run {rid}: {key} {run.get(key)!r}, not {want!r}")
    try:
        at_run = sources_at(str(run.get("head_sha")))
    except child.CalledProcessError:
        return [*bad, f"run {rid}: its commit {run.get('head_sha')!r} cannot be read"]
    paths = measured_paths(checkout_sources(), at_run)
    for ref in (base, "HEAD"):
        rc, names = git("diff", "--name-only", str(run.get("head_sha")), ref, "--", *paths)
        if rc != 0 or names.strip():
            changed = ", ".join(names.split()) or f"git exited {rc}"
            bad.append(f"the measured paths changed between the run's commit and {ref}: {changed}")
    listing = json.loads(gh(f"repos/{repo}/actions/runs/{rid}/artifacts?per_page=100"))
    name = f"bench-wall-{rid}-{attempt}"
    found = [a for a in listing.get("artifacts", []) if a.get("name") == name]
    if len(found) != 1:
        return [*bad, f"run {rid}: {len(found)} artifacts named {name}"]
    with zipfile.ZipFile(io.BytesIO(gh(f"repos/{repo}/actions/artifacts/{found[0]['id']}/zip"))) as z:
        published = z.read("wall.json") if "wall.json" in z.namelist() else None
    if published != data:
        bad.append(f"benchmarks/wall.json is not byte-equal to run {rid}'s artifact {name}")
    return bad


def cmd_check(args) -> int:
    bad = check_wall_run(args.wall_run or None, args.base, args.repo)
    for b in bad:
        print(f"bench check: {b}", file=sys.stderr)
    return 1 if bad else 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("gen")
    g.add_argument("--scale", choices=("full", "gate"), default="gate")
    g.add_argument("--out", type=Path)
    g.add_argument("--update", action="store_true")
    r = sub.add_parser("run")
    r.add_argument("--out", type=Path, required=True)
    r.add_argument("--quick", action="store_true")
    r.add_argument("--workload", action="append")
    d = sub.add_parser("render")
    d.add_argument("--check", action="store_true")
    c = sub.add_parser("check")
    c.add_argument("--wall-run", nargs="?", const="", required=True, metavar="RUN_ID")
    c.add_argument("--base", required=True)
    c.add_argument("--repo", required=True)
    args = ap.parse_args(argv)
    return {"gen": cmd_gen, "run": cmd_run, "render": cmd_render, "check": cmd_check}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
