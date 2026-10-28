#!/usr/bin/env python3
"""Dump Unity files to the canonical JSON Lines format, with UnityPy as the oracle.

    python tools/parity/unitypy_dump.py INPUT -o OUT.jsonl[.gz] [--jobs N]
    python tools/parity/unitypy_dump.py --index /corpus/index.json -o OUT.jsonl --sample 20

INPUT is a file or a folder (walked recursively); with --index it is the
corpus index.json and its bundle and sample entries are dumped. The output
path must lie outside the repository. docs/CANONICAL_DUMP.md specifies the
format; this script needs only the standard library and UnityPy (the pinned
oracle stack in the unityrs-dev container).
"""

from __future__ import annotations

import argparse
import fnmatch
import gzip
import hashlib
import json
import os
import re
import sys
import time
from collections import Counter
from multiprocessing import get_context
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import UnityPy  # noqa: E402
from UnityPy.files import BundleFile, File, SerializedFile, WebFile  # noqa: E402
from UnityPy.helpers import ImportHelper, TypeTreeHelper  # noqa: E402
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter  # noqa: E402

import canonical as C  # noqa: E402

# What routes parse_as_dict through UnityPyBoost; without it the oracle is not the pinned one.
HAVE_BOOST = TypeTreeHelper.read_typetree_boost is not None

NOTE_MAX = 300
INDEX_KINDS = ("bundle", "unitypy-sample")


# ---------------------------------------------------------------------- hooks
#
# UnityPy reads a bundle's blocks and directory into locals and keeps neither.
# Two thin wrappers record them on the instance as UnityPy itself sees them;
# both call straight through, so parsing is unchanged.

_orig_read_files = File.read_files
_orig_decompress = BundleFile.decompress_data


def _read_files(self, reader, files):
    nodes = []
    for node in files:
        reader.Position = node.offset
        data = bytes(reader.read(node.size))
        nodes.append((node, len(data), hashlib.sha256(data).hexdigest(), detect(data)))
    self._unityrs_nodes = nodes
    return _orig_read_files(self, reader, files)


def _decompress_data(self, compressed_data, uncompressed_size, flags, *args, **kwargs):
    self.__dict__.setdefault("_unityrs_decompress", []).append(
        [len(compressed_data), int(uncompressed_size), int(flags)]
    )
    return _orig_decompress(self, compressed_data, uncompressed_size, flags, *args, **kwargs)


File.read_files = _read_files
BundleFile.decompress_data = _decompress_data


# ----------------------------------------------------------------- utilities


def detect(data: bytes) -> str:
    """check_file_type's verdict as a FileType name, or "error" when it raises."""
    try:
        return ImportHelper.check_file_type(EndianBinaryReader(data))[0].name
    except Exception:  # noqa: BLE001 - UnityPy raises bare Exception here
        return "error"


def note(e: BaseException) -> str:
    text = f"{type(e).__name__}: {e}"
    return text if len(text) <= NOTE_MAX else text[: NOTE_MAX - 3] + "..."


def node_rows(root):
    """A typetree as flat pre-order rows (see the spec's 'Typetree nodes')."""
    if root is None:
        return None
    return [
        [
            n.m_Level,
            C.enc_name(n.m_Type),
            C.enc_name(n.m_Name),
            n.m_ByteSize,
            n.m_Version,
            n.m_TypeFlags,
            n.m_VariableCount,
            n.m_Index,
            n.m_MetaFlag,
            n.m_RefTypeHash,
        ]
        for n in root.traverse()
    ]


def type_record(t):
    return {
        "class_id": t.class_id,
        "is_stripped_type": t.is_stripped_type,
        "script_type_index": t.script_type_index,
        "script_id": C.enc_hex(t.script_id),
        "old_type_hash": C.enc_hex(t.old_type_hash),
        "nodes": node_rows(t.node),
        "type_dependencies": None if t.type_dependencies is None else list(t.type_dependencies),
        "class_name": C.enc_name(t.m_ClassName),
        "namespace": C.enc_name(t.m_NameSpace),
        "assembly": C.enc_name(t.m_AssemblyName),
    }


def directory_rows(f):
    rows = []
    for node, _, _, _ in getattr(f, "_unityrs_nodes", []):
        rows.append([node.offset, node.size, getattr(node, "flags", None), C.enc_name(node.path)])
    return rows


# ---------------------------------------------------------------- L1 records


def bundle_l1(b):
    calls = b.__dict__.get("_unityrs_decompress")
    flags = getattr(b, "dataflags", None)
    return {
        "kind": "bundle",
        "signature": C.enc_name(b.signature),
        "format": b.version,
        "version_player": C.enc_name(b.version_player),
        "version_engine": C.enc_name(b.version_engine),
        "engine_version_packed": engine_version(b),
        "flags": None if flags is None else int(flags),
        "flags_class": None if flags is None else type(flags).__name__,
        "block_alignment": bool(b._uses_block_alignment),
        "encrypted": b.decryptor is not None,
        "hash": C.enc_hex(getattr(b, "_hash", None)),
        "crc": getattr(b, "crc", None),
        # Recorded calls are [compressed, uncompressed, flags]; rows follow
        # UnityPy's BlockInfo order: [uncompressed, compressed, flags].
        "blocks_info": [calls[0][1], calls[0][0], calls[0][2]] if calls else None,
        "blocks": [[u, c, fl] for c, u, fl in calls[1:]] if calls else None,
        "directory": directory_rows(b),
    }


def engine_version(b):
    """BundleFile.parse_version() packed, or None where it raises (only UnityFS needs it to load)."""
    try:
        return int(b.parse_version())
    except Exception:  # noqa: BLE001
        return None


def web_l1(w):
    return {
        "kind": "web",
        "signature": C.enc_name(w.signature),
        "packer": w.packer,
        "directory": directory_rows(w),
    }


def serialized_l1(sf):
    h = sf.header
    ref_types = getattr(sf, "ref_types", None)
    user_info = getattr(sf, "userInformation", None)
    return {
        "kind": "serialized",
        "header": {
            "metadata_size": h.metadata_size,
            "file_size": h.file_size,
            "version": h.version,
            "data_offset": h.data_offset,
            "endian": h.endian,
            "reserved": C.enc_hex(getattr(h, "reserved", None)),
            "unknown": sf.unknown,
        },
        "unity_version": C.enc_name(sf.unity_version),
        # SerializedFile leaves .version unset below format 7.
        "version_packed": int(sf.version) if hasattr(sf, "version") else None,
        "target_platform": getattr(sf, "_m_target_platform", None),
        "enable_type_tree": bool(sf._enable_type_tree),
        "big_id_enabled": sf.big_id_enabled,
        "types": [type_record(t) for t in sf.types],
        "object_order": list(sf.objects.keys()),
        "script_types": [[s.local_serialized_file_index, s.local_identifier_in_file] for s in sf.script_types],
        "externals": [[C.enc_name(e.temp_empty), C.enc_hex(e.guid), e.type, C.enc_name(e.path)] for e in sf.externals],
        "ref_types": None if ref_types is None else [type_record(t) for t in ref_types],
        "user_information": C.enc_name(user_info),
        "container": [
            [C.enc_name(path), info.preloadIndex, info.preloadSize, info.asset.m_FileID, info.asset.m_PathID]
            for path, info in sf._container.container
        ],
    }


# ------------------------------------------------------------ object records

_CHECK_READ = re.compile(r"Expected to read (\d+) bytes, but only read (\d+) bytes")


def error_kind(e: BaseException) -> tuple[str, int | None]:
    """Map a parse exception to the spec's L2 error kinds (and bytes read, if known)."""
    msg = str(e)
    m = _CHECK_READ.search(msg)
    if m:
        return "check_read", int(m.group(2))
    if isinstance(e, EOFError) or "out of bounds" in msg:
        return "out_of_bounds", None
    if "Negative length" in msg:
        return "negative_length", None
    if "ref type" in msg.lower() or "ref_types" in msg or "Referenced type" in msg or "Reference Type" in msg:
        return "ref_type", None
    return "other", None


def ref_resolver(sf):
    cache = {}

    def resolve(type_value):
        key = (type_value["class"], type_value["ns"], type_value["asm"])
        if key not in cache:
            cache[key] = TypeTreeHelper.get_ref_type_node({"type": {"class": key[0], "ns": key[1], "asm": key[2]}}, sf)
        return cache[key]

    return resolve


def first_difference(a: bytes, b: bytes) -> int:
    """The first offset where a and b differ, or the shorter length if one is a prefix."""
    return next((i for i, (x, y) in enumerate(zip(a, b, strict=False)) if x != y), min(len(a), len(b)))


def object_record(obj, path, encoder, levels, stats):
    l2 = l3 = None
    value = node = None
    parsed_ok = False
    if "L2" in levels:
        t0 = time.perf_counter()
        error = None
        root_type = source = None
        try:
            node = obj._get_typetree_node()
            root_type = C.enc_name(node.m_Type)
            st = obj.serialized_type
            source = "embedded" if st is not None and st.node is node else "tpk"
        except Exception as e:  # noqa: BLE001 - TypeTreeError, or the TPK's KeyError/ValueError/AssertionError
            error = {"kind": "no_typetree", "bytes_read": None, "note": note(e)}
        if error is None:
            try:
                value = obj.parse_as_dict()
                parsed_ok = True
            except Exception as e:  # noqa: BLE001 - every failure is a recorded outcome
                kind, bytes_read = error_kind(e)
                error = {"kind": kind, "bytes_read": bytes_read, "note": note(e)}
                if kind == "check_read":
                    value = obj.parse_as_dict(check_read=False)
        t1 = time.perf_counter()
        encoded = None
        if value is not None:
            try:
                encoded = encoder.encode(value, node)
            except C.EncodeError as e:
                parsed_ok = False
                error = {"kind": "encode", "bytes_read": None, "note": note(e)}
                stats["encode_errors"] += 1
        stats["t_parse"] += t1 - t0
        stats["t_encode"] += time.perf_counter() - t1
        if error is not None:
            stats["l2_" + error["kind"]] += 1
        l2 = {"root_type": root_type, "source": source, "error": error, "value": encoded}
    if "L3" in levels:
        t0 = time.perf_counter()
        raw = bytes(obj.get_raw_data())
        l3 = {"raw_size": len(raw), "raw_sha256": hashlib.sha256(raw).hexdigest()}
        if not parsed_ok:
            l3.update(resave="skipped", resave_size=None, resave_sha256=None, first_diff=None)
        else:
            try:
                w = EndianBinaryWriter(endian=obj.reader.endian)
                TypeTreeHelper.write_typetree(value, node, w, obj.assets_file)
                out = bytes(w.bytes)
            except Exception as e:  # noqa: BLE001
                l3.update(resave="error", resave_size=None, resave_sha256=None, first_diff=None, note=note(e))
            else:
                if out == raw:
                    l3.update(resave="equal", resave_size=None, resave_sha256=None, first_diff=None)
                else:
                    l3.update(
                        resave="differs",
                        resave_size=len(out),
                        resave_sha256=hashlib.sha256(out).hexdigest(),
                        first_diff=first_difference(out, raw),
                    )
        stats["l3_" + l3["resave"]] += 1
        stats["t_resave"] += time.perf_counter() - t0
    l1 = None
    if "L1" in levels:
        l1 = {
            "type_id": obj.type_id,
            "class_id": obj.class_id,
            "byte_start": obj.byte_start,
            "byte_size": obj.byte_size,
            "is_destroyed": obj.is_destroyed,
            "is_stripped": obj.is_stripped,
        }
    return {"rec": "object", "file": path, "path_id": obj.path_id, "l1": l1, "l2": l2, "l3": l3}


# -------------------------------------------------------------- file records


def parsed_as(f) -> str:
    if isinstance(f, SerializedFile):
        return "SerializedFile"
    if isinstance(f, BundleFile):
        return "BundleFile"
    if isinstance(f, WebFile):
        return "WebFile"
    return "raw"


def emit_file(f, path, l0, levels, out, stats):
    """One file record, its objects by path_id, then its children in UnityPy's order."""
    kind = l0["parsed_as"]
    l1 = None
    if "L1" in levels:
        if kind == "SerializedFile":
            l1 = serialized_l1(f)
        elif kind == "BundleFile":
            l1 = bundle_l1(f)
        elif kind == "WebFile":
            l1 = web_l1(f)
    out.append({"rec": "file", "file": path, "l0": l0, "l1": l1, "error": None})
    stats["files"] += 1
    if kind == "SerializedFile":
        encoder = C.ValueEncoder(ref_resolver(f))
        for obj in sorted(f.objects.values(), key=lambda o: o.path_id):
            out.append(object_record(obj, path, encoder, levels, stats))
            stats["objects"] += 1
    elif kind in ("BundleFile", "WebFile"):
        info = {node.path: (size, sha, detected) for node, size, sha, detected in f._unityrs_nodes}
        for name, child in f.files.items():
            size, sha, detected = info[name]
            child_l0 = {"type": detected, "parsed_as": parsed_as(child), "size": size, "sha256": sha}
            emit_file(child, [*path, C.enc_name(name)], child_l0, levels, out, stats)


def dump_input(job):
    """Dump one input file to canonical lines; runs in a worker process."""
    abs_path, rel, levels = job
    stats = Counter()
    t0 = time.perf_counter()
    data = Path(abs_path).read_bytes()
    detected = detect(data)
    l0 = {"type": detected, "parsed_as": None, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    out = []
    if detected in ("ZIP", "error"):
        # Format v1 does not expand zip/apk members (see the spec); a file whose
        # detection raises would raise again inside Environment.load_file.
        l0["parsed_as"] = "zip" if detected == "ZIP" else "error"
        error = {"kind": "detect"} if detected == "error" else None
        out.append({"rec": "file", "file": [rel], "l0": l0, "l1": None, "error": error})
        stats["files"] += 1
    else:
        env = UnityPy.Environment()
        try:
            f = env.load_file(data, name=abs_path)
        except Exception as e:  # noqa: BLE001 - a failed load is a recorded outcome
            l0["parsed_as"] = "error"
            out.append(
                {"rec": "file", "file": [rel], "l0": l0, "l1": None, "error": {"kind": "parse", "note": note(e)}}
            )
            stats["files"] += 1
            stats["load_errors"] += 1
        else:
            stats["t_load"] += time.perf_counter() - t0
            l0["parsed_as"] = parsed_as(f)
            emit_file(f, [rel], l0, levels, out, stats)
    text = "".join(C.dumps(r) + "\n" for r in out)
    stats["bytes_in"] += len(data)
    stats["bytes_out"] += len(text.encode("utf-8"))
    stats["t_total"] += time.perf_counter() - t0
    return rel, text, stats, l0["sha256"]


# ------------------------------------------------------------------- inputs


def repo_roots(start: Path) -> list[Path]:
    """Every ancestor of this script that holds a .git (worktrees nest in the repo)."""
    return [p for p in [start, *start.parents] if (p / ".git").exists()]


def collect_inputs(args) -> list[tuple[str, str, str | None]]:
    """(absolute path, dump-relative path, expected sha256) in canonical order."""
    src = Path(args.input).resolve()
    items = []
    if args.index:
        index = json.loads(src.read_text())
        root = src.parent
        for e in index["files"]:
            if e.get("kind") not in args.kind:
                continue
            if args.build and str(e.get("build")) not in args.build:
                continue
            items.append((str(root / e["path"]), e["path"], e.get("sha256")))
    elif src.is_dir():
        for dirpath, dirnames, filenames in os.walk(src):
            dirnames.sort()
            for name in filenames:
                p = Path(dirpath) / name
                if p.is_file():
                    items.append((str(p), p.relative_to(src).as_posix(), None))
    elif src.is_file():
        items.append((str(src), src.name, None))
    else:
        raise SystemExit(f"no such input: {src}")
    if args.name:
        items = [i for i in items if any(fnmatch.fnmatchcase(i[1], pat) for pat in args.name)]
    # Paths order by their UTF-8 (filesystem) bytes, as Rust's OsStr does on Unix.
    items.sort(key=lambda i: os.fsencode(i[1]))
    if args.sample and len(items) > args.sample:
        n = len(items)
        items = [items[(k * n) // args.sample] for k in range(args.sample)]
    if args.limit:
        items = items[: args.limit]
    return items


def open_output(path: Path, compress: bool):
    """The output stream; gzip with mtime 0 so equal dumps compress to equal bytes."""
    if compress:
        raw = open(path, "wb")  # noqa: SIM115 - closed with the gzip wrapper
        return gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0), raw
    return open(path, "wb"), None  # noqa: SIM115


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input", help="file, folder, or (with --index) a corpus index.json")
    ap.add_argument("-o", "--output", required=True, help="output .jsonl or .jsonl.gz, outside the repository")
    ap.add_argument("--index", action="store_true", help="INPUT is a corpus index.json")
    ap.add_argument("--kind", default=",".join(INDEX_KINDS), help="index entry kinds to dump (comma list)")
    ap.add_argument("--build", help="index builds to dump (comma list)")
    ap.add_argument("--name", action="append", help="glob on the dump-relative path (repeatable)")
    ap.add_argument("--sample", type=int, help="keep N inputs spread evenly over the sorted list")
    ap.add_argument("--limit", type=int, help="keep the first N inputs")
    ap.add_argument("--levels", default=",".join(C.LEVELS), help="levels to dump (default: L0,L1,L2,L3)")
    ap.add_argument("-j", "--jobs", type=int, default=1, help="worker processes (default 1)")
    ap.add_argument("--stats", help="write per-input timings and totals as JSON to this path")
    ap.add_argument("-q", "--quiet", action="store_true", help="no per-input progress on stderr")
    args = ap.parse_args(argv)
    args.kind = args.kind.split(",")
    args.build = args.build.split(",") if args.build else None
    levels = [lv for lv in C.LEVELS if lv in args.levels.split(",")]
    if "L3" in levels and "L2" not in levels:
        ap.error("L3 needs L2 (the resave writes the parsed value)")

    out_path = Path(args.output).resolve()
    for root in repo_roots(Path(__file__).resolve().parent):
        if out_path.is_relative_to(root):
            ap.error(f"refusing to write into the repository ({root}); pick a path outside it")

    inputs = collect_inputs(args)
    if not inputs:
        ap.error("no inputs matched")

    header = {
        "rec": "dump",
        "format": C.FORMAT,
        "version": C.VERSION,
        "levels": levels,
        "producer": {
            "name": "unitypy_dump.py",
            "unitypy": UnityPy.__version__,
            "boost": HAVE_BOOST,
            "python": sys.version.split()[0],
        },
    }
    totals = Counter()
    per_input = []
    t0 = time.perf_counter()
    # Written under a .partial name and renamed when complete, so a dump at the
    # requested path is always whole.
    out_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = out_path.with_name(out_path.name + ".partial")
    fh, raw = open_output(tmp, out_path.suffix == ".gz")
    pool = None
    done = False
    try:
        fh.write((C.dumps(header) + "\n").encode())
        jobs = [(a, r, levels) for a, r, _ in inputs]
        expected = {r: sha for _, r, sha in inputs}
        if args.jobs > 1:
            # imap keeps input order, so the output is the same as a serial run.
            pool = get_context("fork").Pool(args.jobs, maxtasksperchild=50)
            results = pool.imap(dump_input, jobs, chunksize=1)
        else:
            results = map(dump_input, jobs)
        for i, (rel, text, stats, sha) in enumerate(results, 1):
            fh.write(text.encode("utf-8"))
            totals.update(stats)
            if expected[rel] and expected[rel] != sha:
                totals["index_sha256_mismatch"] += 1
                print(f"warning: {rel}: sha256 {sha} differs from the index", file=sys.stderr)
            per_input.append({"path": rel, **{k: round(v, 6) if isinstance(v, float) else v for k, v in stats.items()}})
            if not args.quiet:
                print(
                    f"[{i}/{len(jobs)}] {rel}: {stats['objects']} objects, {stats['bytes_in']} B in, "
                    f"{stats['bytes_out']} B out, {stats['t_total']:.2f}s",
                    file=sys.stderr,
                )
        fh.write((C.dumps({"rec": "end", "files": totals["files"], "objects": totals["objects"]}) + "\n").encode())
        done = True
    finally:
        if pool is not None:
            if done:
                pool.close()
            else:
                pool.terminate()
            pool.join()
        fh.close()
        if raw is not None:
            raw.close()
        if not done:
            tmp.unlink(missing_ok=True)
    tmp.replace(out_path)
    wall = time.perf_counter() - t0
    summary = {
        "inputs": len(inputs),
        "wall_s": round(wall, 3),
        "output_bytes": out_path.stat().st_size,
        **{k: round(v, 3) if isinstance(v, float) else v for k, v in sorted(totals.items())},
    }
    print(json.dumps(summary, indent=1), file=sys.stderr)
    if args.stats:
        Path(args.stats).write_text(json.dumps({"summary": summary, "inputs": per_input}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
