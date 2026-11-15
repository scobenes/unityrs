#!/usr/bin/env python3
"""L6 corpus JSONL: one digest record per Mesh, with exact masked comparison.

Dump: mesh_dump.py INPUT -o OUT.jsonl[.gz] [--index] [--corrected]
Compare: mesh_dump.py compare REFERENCE CANDIDATE [--mask MASK.json]
Only aggregate counts are printed. Dumps and masks remain outside the repository.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import ntpath
import os
import struct
import sys
from pathlib import Path
from unittest import mock

ATTRS = (
    "m_Vertices",
    "m_Normals",
    "m_Tangents",
    "m_Colors",
    "m_BoneWeights",
    "m_BoneIndices",
    *(f"m_UV{i}" for i in range(8)),
)


def dumps(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def name(raw):
    try:
        return raw.decode("utf8")
    except UnicodeDecodeError:
        return {"t": "str", "hex": raw.hex()}


def u64(n):
    return struct.pack(">Q", n)


def array_digest(rows):
    """Framed tuples, each component tagged i/i64 or f/IEEE binary64, big endian."""
    h = hashlib.sha256(b"array-v1\0" + u64(len(rows)))
    for row in rows:
        h.update(u64(len(row)))
        for x in row:
            h.update(b"f" + struct.pack(">d", x) if isinstance(x, float) else b"i" + struct.pack(">q", x))
    return h.hexdigest()


def index_digest(indices):
    if indices is None:
        return None
    h = hashlib.sha256(b"indices-v1\0" + u64(len(indices)))
    for x in indices:
        h.update(struct.pack(">I", x))
    return h.hexdigest()


def arrays(handler):
    attrs = {key: array_digest(getattr(handler, key)) for key in ATTRS if getattr(handler, key) is not None}
    bind = getattr(handler, "m_BindPose", None)
    if bind is not None:
        attrs["m_BindPose"] = array_digest([[x for row in matrix for x in row] for matrix in bind])
    return {
        "vertex_count": handler.m_VertexCount,
        "use16": handler.m_Use16BitIndices,
        "index_buffer": index_digest(handler.m_IndexBuffer),
        "attrs": attrs,
    }


def triangles_digest(submeshes, counts):
    """Full tuples, partial tuple, empty-tuple count, then mutated indexCount."""
    h = hashlib.sha256(b"triangles-v1\0" + u64(len(submeshes)))
    for sub, count in zip(submeshes, counts, strict=True):
        whole = [tri for tri in sub if len(tri) == 3]
        cut = next((tri for tri in sub if 0 < len(tri) < 3), ())
        h.update(u64(len(whole)))
        for tri in whole:
            for x in tri:
                h.update(struct.pack(">I", x))
        h.update(u64(len(cut)))
        for x in cut:
            h.update(struct.pack(">I", x))
        h.update(u64(sum(not tri for tri in sub)))
        h.update(u64(count))
    return h.hexdigest()


def records(path, relative, data=None):
    import UnityPy
    from UnityPy.export.MeshExporter import export_mesh_obj
    from UnityPy.helpers.MeshHelper import MeshHandler

    # Parse exactly the bytes that main hashes; retain the source directory
    # for the oracle's normal sibling-resource fallback.
    if data is None:
        data = Path(os.fsdecode(path)).read_bytes()
    env = UnityPy.Environment()
    env.path = os.path.dirname(os.path.abspath(os.fsdecode(path)))
    env.load_file(data, name=relative)
    result = []
    for obj in env.objects:
        if obj.type.value != 43:
            continue
        try:
            mesh = obj.parse_as_object()
            handler = MeshHandler(mesh)
            handler.process()
            values = arrays(handler)
            subs = handler.get_triangles() if handler.m_VertexCount > 0 and handler.m_Vertices else []
            counts = [s.indexCount for s in mesh.m_SubMeshes] if subs else []
            triangles = triangles_digest(subs, counts)
            # Triangle strips mutate indexCount: export a fresh Mesh.
            text = export_mesh_obj(obj.parse_as_object())
            answer = {
                "arrays": values,
                "triangles": triangles,
                "obj": hashlib.sha256(text.encode("utf8", "surrogateescape")).hexdigest()
                if text is not False
                else None,
            }
        except Exception as error:
            if isinstance(error, (MemoryError, OSError)):
                raise
            answer = {"error": type(error).__name__}
        file = ntpath.basename(obj.assets_file.name).encode("utf8", "surrogateescape").hex()
        result.append({"input": name(os.fsencode(relative)), "file": file, "path_id": obj.path_id, "result": answer})
    return sorted(result, key=lambda r: (r["file"], r["path_id"]))


def key(record):
    return dumps(record["input"]), record["file"], record["path_id"]


def exact(left, right):
    """JSON equality retaining numeric/boolean types, independent of member order."""
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(exact(left[k], right[k]) for k in left)
    if isinstance(left, list):
        return len(left) == len(right) and all(exact(a, b) for a, b in zip(left, right, strict=True))
    return left == right


def compare(reference, candidate, mask=()):
    """Masks are exact record keys and member paths; stale/duplicate masks fail."""

    def indexed(rows):
        out = {}
        for row in rows:
            k = key(row)
            if k in out:
                raise ValueError("duplicate mesh key")
            out[k] = row
        return out

    a, b = indexed(reference), indexed(candidate)
    masked = set()
    for entry in mask:
        k = (dumps(entry["key"][0]), *entry["key"][1:])
        path = tuple(entry["path"])
        identity = (k, path)
        if identity in masked or k not in a or k not in b or not path or path[0] != "result":
            raise ValueError("duplicate, missing or invalid mask")
        left, right = a[k], b[k]
        try:
            for part in path[:-1]:
                left, right = left[part], right[part]
            if exact(left[path[-1]], right[path[-1]]):
                raise ValueError("stale mask")
            # Clone only records being masked; caller-owned records remain unchanged.
            a[k], b[k] = json.loads(dumps(a[k])), json.loads(dumps(b[k]))
            left, right = a[k], b[k]
            for part in path[:-1]:
                left, right = left[part], right[part]
            left[path[-1]] = right[path[-1]] = None
        except (KeyError, TypeError) as error:
            raise ValueError("mask path is absent") from error
        masked.add(identity)
    return {
        "meshes": len(a),
        "masked": len(masked),
        "mismatches": sum(not exact(a.get(k), b.get(k)) for k in a.keys() | b.keys()),
    }


def read_rows(path):
    opener = gzip.open if str(path).endswith(".gz") else open
    with opener(path, "rt", encoding="utf8") as fh:
        return [json.loads(line) for line in fh]


def compare_main(argv):
    p = argparse.ArgumentParser(description="Compare Mesh JSONL, printing aggregate counts only.")
    p.add_argument("reference")
    p.add_argument("candidate")
    p.add_argument("--mask", type=Path)
    args = p.parse_args(argv)
    try:
        mask = json.loads(args.mask.read_text()) if args.mask else []
        summary = compare(read_rows(args.reference), read_rows(args.candidate), mask)
    except (OSError, ValueError, KeyError, TypeError):
        p.exit(2, "mesh_dump: unreadable dump or invalid mask (details suppressed)\n")
    print(dumps(summary))
    return int(summary["mismatches"] != 0)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv and argv[0] == "compare":
        return compare_main(argv[1:])
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("input")
    p.add_argument("-o", "--output", required=True)
    p.add_argument("--index", action="store_true")
    p.add_argument("--kind", default="bundle,unitypy-sample")
    p.add_argument("--build")
    p.add_argument("--name", action="append")
    p.add_argument("--sample", type=int)
    p.add_argument("--limit", type=int)
    p.add_argument("--allow-index-mismatch", action="store_true")
    p.add_argument("--corrected", action="store_true", help="use section 19.1 corrected MeshHandler")
    args = p.parse_args(argv)
    if args.allow_index_mismatch and not args.index:
        p.error("--allow-index-mismatch needs --index")
    import unitypy_dump as U

    problem = U.oracle_stack_problem()
    if problem:
        p.error("requires the pinned oracle stack")
    U.pin_none()
    args.kind = args.kind.split(",")
    args.build = args.build.split(",") if args.build else None
    inputs = U.collect_inputs(args)
    out = U.output_path(args.output)
    tmp = out.with_name(out.name + ".partial")
    if U.C.inside_repo(out):
        p.error("output must lie outside the repository")
    reads = [Path(os.fsdecode(path)).resolve() for path, _, _ in inputs]
    if args.index:
        reads.append(Path(args.input).resolve())
    for path in (out, tmp):
        if (
            path.is_symlink()
            or path.resolve() in reads
            or any(
                U.file_identity(path) is not None and U.file_identity(path) == U.file_identity(r, True) for r in reads
            )
        ):
            p.error("output or partial path aliases an input or is a symlink")
    if U.file_identity(out) is not None and U.file_identity(out) == U.file_identity(tmp):
        p.error("output aliases its partial path")
    if not inputs:
        p.error("no inputs matched")
    patch = {}
    if args.corrected:
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
        with mock.patch.object(sys, "argv", sys.argv[:1]):
            import meshdecode
        patch = meshdecode.CORRECTED
    from UnityPy.helpers.MeshHelper import MeshHandler

    out.parent.mkdir(parents=True, exist_ok=True)
    tmp.unlink(missing_ok=True)
    total = 0
    try:
        fh, raw = U.open_output(tmp, out.suffix == ".gz")
        try:
            with mock.patch.multiple(MeshHandler, **patch) if patch else mock.patch.dict({}, {}):
                for path, relative, expected in inputs:
                    with open(path, "rb") as source:
                        data = source.read()
                        digest = hashlib.sha256(data).hexdigest()
                    if expected and expected != digest and not args.allow_index_mismatch:
                        raise ValueError("index mismatch")
                    for row in records(path, relative, data):
                        fh.write((dumps(row) + "\n").encode("utf8"))
                        total += 1
        finally:
            fh.close()
            if raw is not None:
                raw.close()
        os.replace(tmp, out)
    except Exception:
        tmp.unlink(missing_ok=True)
        p.exit(1, "mesh_dump: input failed; no dump written (details suppressed)\n")
    print(dumps({"inputs": len(inputs), "meshes": total, "corrected": args.corrected}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
