"""What the bench self-tests share: the harness's modules on the path, and a synthetic wall.json made from given
times, statuses and digests. Standard library only."""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import bench  # noqa: E402
import results  # noqa: E402

WORKLOADS = json.loads((HERE / "workloads.json").read_text(encoding="utf-8"))
SPECS = {w["name"]: w for w in WORKLOADS["workloads"]}
FIXTURES = json.loads((ROOT / "benchmarks" / "fixtures.json").read_text(encoding="utf-8"))
NAMES = ("load-objects-lz4", "typetree-objects-lz4", "typetree-objects-2019", "texture-DXT1", "texture-BC6H")


def counts(spec: dict) -> dict:
    """What a run of `spec` would count, from the full scale's inventory of its fixture."""
    inv = FIXTURES["scales"]["full"]["inventories"][spec["fixture"]]
    out = {"source_bytes": 1_000_000}
    if spec["op"] in ("load", "typetree"):
        out["objects"] = inv["objects"]
    elif spec["op"] == "texture":
        out.update(textures=inv["textures"], pixels=inv["pixels"])
    else:
        out.update(entries=1, entry_bytes=inv["entry_bytes"])
    return out


def wall(names=NAMES, times=None, status=None, digest=None, rounds=7, emulated=False) -> dict:
    """A wall.json of `names`: run r of (name, impl) takes `times[(name, impl)][r]` seconds (by default UnityPy
    1.0 + r/100 and unityrs a tenth of it), with status `status[(name, impl, r)]` (default ok) and digest
    `digest[(name, impl, r)]` (default one per name). Its stored pairing is the computed one."""
    times, status, digest = times or {}, status or {}, digest or {}
    doc = {
        "schema_version": 1, "run_id": 12345, "run_attempt": 1, "commit": "0123456789abcdef" * 2 + "01234567",
        "date": "2026-11-19", "scale": "full", "rounds": rounds, "warmup": 1,
        "tools": {"unityrs": "0.0.1", "rustc": "rustc 1.99.1 (synthetic)\nhost: x86_64-unknown-linux-gnu",
                  "unitypy": "1.25.2", "unitypyboost_sha256": "ab" * 32, "pillow": "9.5.0", "python": "3.11.9"},
        "machine": {"cpu": "Synthetic CPU", "cores": 4, "memory_bytes": 16 * 2**30, "kernel": "6.8.0",
                    "runner_image": "20261101.1", "emulated": emulated},
        "fixtures_sha256": "cd" * 32, "workloads": [],
    }
    for name in names:
        spec = SPECS[name]
        for impl in results.IMPLS:
            base = [1.0 + r / 100 for r in range(rounds)]
            got = times.get((name, impl)) or (base if impl == "unitypy" else [t / 10 for t in base])
            runs = [{
                "round": r, "scope_s": got[r], "process_s": got[r] + 0.5, "cpu_s": got[r] + 0.4,
                "peak_pss_bytes": 64 * 2**20, "status": status.get((name, impl, r), "ok"),
                "counts": counts(spec), "digest": digest.get((name, impl, r), f"digest-{name}"),
            } for r in range(rounds)]
            doc["workloads"].append({
                "name": name, "impl": impl, "category": spec["category"], "timer": spec["timer"], "paired": None,
                "reason": None, "parity_row": spec["parity_row"], "tier": spec["tier"], "runs": runs,
            })
    for name, (paired, reason) in results.pairings(doc, WORKLOADS).items():
        for e in doc["workloads"]:
            if e["name"] == name:
                e["paired"], e["reason"] = paired, reason
    return doc


def tree(folder: Path, wall_doc: dict | None = None, gate: dict | None = None) -> Path:
    """A copy of what `render` reads, under `folder`: README.md, workloads.json, fixtures.json, and the given
    wall.json and gate.json."""
    (folder / "tools" / "bench").mkdir(parents=True)
    (folder / "benchmarks").mkdir()
    shutil.copy(ROOT / "README.md", folder / "README.md")
    shutil.copy(HERE / "workloads.json", folder / "tools" / "bench" / "workloads.json")
    shutil.copy(ROOT / "benchmarks" / "fixtures.json", folder / "benchmarks" / "fixtures.json")
    for name, doc in (("wall.json", wall_doc), ("gate.json", gate)):
        if doc is not None:
            (folder / "benchmarks" / name).write_text(bench.dump_json(doc), encoding="utf-8")
    return folder


def scratch_tree(case, wall_doc: dict | None = None, gate: dict | None = None) -> Path:
    """`tree` in a scratch folder the test case owns."""
    import scratch

    return tree(scratch.for_case(case, "unityrs-bench-"), wall_doc, gate)
