#!/usr/bin/env python3
"""One run of one workload, in a fresh process, by either implementation (docs/plans/bench.md section 4.2).

    python tools/bench/py_workloads.py --impl unitypy|unityrs WORKLOAD FIXTURE

The same script for both sides: `--impl unitypy` imports UnityPy, `--impl unityrs` does `import unityrs as UnityPy`
(the wheel), and every line after the import is shared. Each run has three phases:

1. setup, neither timed nor counted: the imports, the fixture's bytes read, a warm-up (`warm`), and for the API
   rows after `load` the environment loaded and the objects listed;
2. the scope, the only part timed, whose outputs are held in memory;
3. the digest, after the timer stops: the outputs hashed, so both sides' work can be compared.

| op | scope | digest |
|---|---|---|
| `load` | load from bytes; list every file's object table | each object's path id, class id and byte size |
| `load_entries` | load from bytes; read every non-serialized entry's bytes | the entries' bytes |
| `typetree` | `parse_as_dict()` on every object | canonical JSON of each value |
| `texture` | `obj.read().image.tobytes()` for each Texture2D | mode, size and pixels of each, in the mode UnityPy returns |

It prints one JSON line: `status` (`ok`, or the exception's class and message), `scope_s`, `counts` (what the scope
processed, for throughput) and `digest`. A failing scope is a result, not a crash: the exit status is 0 whenever
the line is written. Standard library only (plus the implementation under test).
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORKLOADS = HERE / "workloads.json"


def implementation(impl: str):
    """The module under test, imported by the name the workload code uses."""
    if impl == "unitypy":
        import UnityPy
    else:
        import unityrs as UnityPy
    return UnityPy


def files_of(env) -> list:
    """Every file the environment loaded, nested bundle entries included, in load order."""
    out, todo = [], list(env.files.values())
    while todo:
        f = todo.pop(0)
        out.append(f)
        inner = getattr(f, "files", None)
        if isinstance(inner, dict) and type(f).__name__ != "SerializedFile":
            todo[0:0] = list(inner.values())
    return out


def serialized_files(env) -> list:
    return [f for f in files_of(env) if type(f).__name__ == "SerializedFile"]


def objects_of(env) -> list:
    return [obj for sf in serialized_files(env) for obj in sf.objects.values()]


def canonical(value):
    """A JSON-able form of a `parse_as_dict()` value that keeps what JSON would lose: dict order and non-string
    keys (as pairs), tuples apart from lists, and bytes (as hex)."""
    if isinstance(value, dict):
        return {"d": [[canonical(k), canonical(v)] for k, v in value.items()]}
    if isinstance(value, tuple):
        return {"t": [canonical(v) for v in value]}
    if isinstance(value, list):
        return [canonical(v) for v in value]
    if isinstance(value, (bytes, bytearray, memoryview)):
        return {"b": bytes(value).hex()}
    if isinstance(value, float):
        return {"f": value.hex()}
    return value


def op_load(UnityPy, data: bytes):
    env = UnityPy.load(data)
    table = [(obj.path_id, obj.class_id, obj.byte_size) for obj in objects_of(env)]
    return (env, table)


def digest_load(out) -> tuple[str, dict]:
    _env, table = out
    h = hashlib.sha256()
    for row in table:
        h.update(("%d,%d,%d\n" % row).encode())
    return h.hexdigest(), {"objects": len(table)}


def op_load_entries(UnityPy, data: bytes):
    env = UnityPy.load(data)
    files = [f for f in files_of(env) if hasattr(f, "bytes") and type(f).__name__ != "SerializedFile"]
    return [f.bytes for f in files]


def digest_load_entries(out) -> tuple[str, dict]:
    h = hashlib.sha256()
    for entry in out:
        h.update(entry)
    return h.hexdigest(), {"entries": len(out), "entry_bytes": sum(len(e) for e in out)}


def op_typetree(objs):
    return [obj.parse_as_dict() for obj in objs]


def digest_typetree(out) -> tuple[str, dict]:
    h = hashlib.sha256()
    for value in out:
        h.update(json.dumps(canonical(value), ensure_ascii=False, separators=(",", ":")).encode())
        h.update(b"\n")
    return h.hexdigest(), {"objects": len(out)}


def op_texture(objs):
    out = []
    for obj in objs:
        image = obj.read().image
        out.append((image.mode, image.size, image.tobytes()))
    return out


def digest_texture(out) -> tuple[str, dict]:
    h = hashlib.sha256()
    for mode, (w, hh), pixels in out:
        h.update(f"{mode} {w}x{hh} {len(pixels)}\n".encode())
        h.update(pixels)
    return h.hexdigest(), {"textures": len(out), "pixels": sum(w * hh for _, (w, hh), _ in out)}


def warm(UnityPy, data: bytes) -> None:
    """Each side's one-time set-up, done before the timer as `import` would do it, so the scope holds the operation
    and not a first call's initialisation (the plan's "after imports"). Both sides defer some to a first use, and
    differently: UnityPy imports its `export` package (fmod_toolkit, numpy) on the first `.image`; the wheel builds
    its `classes` on the first `read()`, imports Pillow on the first `.image`, and reads its config, TPK and file
    system on the first `load`. So both import `classes`, `export` and `PIL.Image`, and load the fixture once,
    dropping the result: no output is reused."""
    for name in (f"{UnityPy.__name__}.classes", f"{UnityPy.__name__}.export", "PIL.Image"):
        importlib.import_module(name)
    UnityPy.load(data)


def run(impl: str, op: str, fixture: Path) -> dict:
    UnityPy = implementation(impl)
    data = fixture.read_bytes()
    warm(UnityPy, data)
    counts = {"source_bytes": len(data)}
    if op in ("load", "load_entries"):
        work = (op_load if op == "load" else op_load_entries, (UnityPy, data))
    else:
        env = UnityPy.load(data)  # held until the process ends, as the objects read through it
        objs = objects_of(env)
        if op == "texture":
            objs = [o for o in objs if o.type.name == "Texture2D"]
        work = (op_typetree if op == "typetree" else op_texture, (objs,))
    fn, args = work
    t0 = time.perf_counter()
    try:
        out = fn(*args)
    except Exception as e:  # noqa: BLE001  (a side that raises is a result: unpaired, with its reason)
        scope = time.perf_counter() - t0
        why = f"{type(e).__name__}: {e}".splitlines()[0][:200]
        return {"status": why, "scope_s": scope, "counts": counts, "digest": None}
    scope = time.perf_counter() - t0
    digest, more = {"load": digest_load, "load_entries": digest_load_entries, "typetree": digest_typetree,
                    "texture": digest_texture}[op](out)
    return {"status": "ok", "scope_s": scope, "counts": {**counts, **more}, "digest": digest}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--impl", choices=("unitypy", "unityrs"), required=True)
    ap.add_argument("workload")
    ap.add_argument("fixture", type=Path)
    args = ap.parse_args(argv)
    table = json.loads(WORKLOADS.read_text(encoding="utf-8"))
    spec = next((w for w in table["workloads"] if w["name"] == args.workload), None)
    if spec is None:
        ap.error(f"no workload {args.workload!r} in {WORKLOADS.name}")
    result = run(args.impl, spec["op"], args.fixture)
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
