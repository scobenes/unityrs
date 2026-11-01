"""The oracle's records of one input: its file and object records, or its crash record.

docs/CANONICAL_DUMP.md specifies them. unitypy_dump.py runs dump_input on each
input in a process of its own (its run_jobs and _child, which the comments here
name) and writes what it returns. Importing this module installs two thin hooks
on UnityPy's File and BundleFile that keep what the L1 records need; both call
straight through, so parsing is unchanged.
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import re
import time
from collections import Counter

import UnityPy
from UnityPy.files import BundleFile, File, SerializedFile, WebFile
from UnityPy.helpers import ImportHelper, TypeTreeHelper
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter

import canonical as C

NOTE_MAX = 300


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
    except MemoryError:
        raise  # the machine's (--memory-cap), not the input's: _child's crash record
    except Exception:  # noqa: BLE001 - UnityPy raises bare Exception here
        return "error"


def note(e: BaseException | str) -> str:
    """A message as a note: valid Unicode, whatever bytes it quotes, and at most NOTE_MAX characters.

    UnityPy puts file bytes into messages raw ("Invalid version string: ..."
    with surrogate escapes); those are written as backslash escapes.
    """
    text = e if isinstance(e, str) else f"{type(e).__name__}: {e}"
    text = text.encode("utf-8", "backslashreplace").decode("utf-8")
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
    except MemoryError:
        raise
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
        # FileIdentifier's own __init__ sets temp_empty from format 6 and guid
        # and type from format 5 only (files/SerializedFile.py:78-83), and its
        # slots leave the others unset (an AttributeError, PARITY.md 7.1): each
        # is recorded as null, as the spec has a member its format lacks.
        "externals": [
            [
                C.enc_name(getattr(e, "temp_empty", None)),
                C.enc_hex(getattr(e, "guid", None)),
                getattr(e, "type", None),
                C.enc_name(e.path),
            ]
            for e in sf.externals
        ],
        "ref_types": None if ref_types is None else [type_record(t) for t in ref_types],
        "user_information": C.enc_name(user_info),
        "container": [container_row(path, info) for path, info in sf._container.container],
    }


def _int(v):
    """An int as itself; anything else (a bool, a float, a string, a class) as None."""
    return v if type(v) is int else None


def container_row(path, info):
    """One `m_Container` item as the spec's row (4.2).

    The load needs only `info.asset` and its `path_id` (helpers/ContainerHelper.py:21-29),
    so on a hostile tree the other members can be missing (an UnknownObject AssetInfo)
    or of another type: a path that is not a str, and a member that is not an int, are
    recorded as null.
    """
    return [
        C.enc_name(path) if isinstance(path, str) else None,
        _int(getattr(info, "preloadIndex", None)),
        _int(getattr(info, "preloadSize", None)),
        _int(getattr(info.asset, "m_FileID", None)),
        _int(getattr(info.asset, "m_PathID", None)),
    ]


# ------------------------------------------------------------ object records

# The messages each L2 kind is raised with, by exception class, matched whole (re.fullmatch). A message can
# carry the file's own strings: the pure-Python reader's "Referenced type not found" names the class, namespace
# and assembly the object holds (helpers/TypeTreeHelper.py:109), so a substring found anywhere in a message says
# nothing about its kind (review round 9: a class named "out of bounds" was out_of_bounds, and one named like
# the check_read message stopped the whole run).
_CHECK_READ = re.compile(r"Expected to read (\d+) bytes, but only read (\d+) bytes")  # TypeTreeHelper.py:156
_KINDS = (
    # UnityPyBoost's bounds checks (UnityPyBoost/TypeTreeHelper.cpp): read_bool, read_u8, read_s8, their
    # arrays and read_bytes raise ValueError; every other read raises EOFError, which only UnityPyBoost raises.
    (ValueError, re.compile(r"read_\w+ out of bounds"), "out_of_bounds"),
    # Both readers (TypeTreeHelper.py:215, TypeTreeHelper.cpp:281).
    (ValueError, re.compile(r"Negative length read from TypeTree"), "negative_length"),
    # The pure-Python reader (TypeTreeHelper.py:99-109): no ref types, then a class it does not hold. A file
    # below format 20 has no `ref_types` attribute, so reading it raises first (PARITY.md, get_ref_type_node).
    (ValueError, re.compile(r"SerializedFile has no ref_types|Referenced type not found: .*", re.S), "ref_type"),
    (AttributeError, re.compile(r"'[\w.]+' object has no attribute 'ref_types'"), "ref_type"),
    # UnityPyBoost: every failure of its get_ref_type_node is reported as this one (TypeTreeHelper.cpp:763).
    (ValueError, re.compile(r"Failed to get ref type node"), "ref_type"),
)


def error_kind(e: BaseException) -> tuple[str, int | None]:
    """Map a parse exception to the spec's L2 error kinds (and bytes read, if known), by its class and its
    whole message: one of the forms above, else `other`."""
    msg = str(e)
    m = _CHECK_READ.fullmatch(msg) if type(e) is ValueError else None
    if m:
        return "check_read", int(m.group(2))
    if type(e) is EOFError:
        return "out_of_bounds", None
    for cls, form, kind in _KINDS:
        if type(e) is cls and form.fullmatch(msg):
            return kind, None
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
        except MemoryError:
            raise
        except Exception as e:  # noqa: BLE001 - TypeTreeError, or the TPK's KeyError/ValueError/AssertionError
            error = {"kind": "no_typetree", "bytes_read": None, "note": note(e)}
        if error is None:
            try:
                value = obj.parse_as_dict()
                parsed_ok = True
            except MemoryError:
                raise  # the machine's (--memory-cap), not an L2 outcome: _child's crash record
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
                # UnityPy read the value; the oracle's own encoder refusing it is a
                # bug in the dumper, not an outcome of the input, so the run stops
                # (spec 4.3: the oracle never writes an `encode` error).
                raise DumperError(
                    f"file {C.dumps(path)} path_id {obj.path_id}: the canonical encoder refused the value "
                    f"UnityPy read ({e})"
                ) from e
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
            except MemoryError:
                raise
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
    """Dump one input file to canonical lines; run_jobs runs it in a process of its own."""
    # UnityPy prints each load error to stdout, quoting the file's bytes; with a
    # strict stdout encoding that print raises and replaces the error.
    with contextlib.redirect_stdout(io.StringIO()):
        return _dump_input(job)


def _dump_input(job):
    path, rel, levels = job
    stats = Counter()
    t0 = time.perf_counter()
    with open(path, "rb") as fh:
        data = fh.read()
    detected = detect(data)
    l0 = {"type": detected, "parsed_as": None, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    out = []
    if detected in ("ZIP", "error"):
        # Format v1 does not expand zip/apk members (see the spec); a file whose
        # detection raises would raise again inside Environment.load_file.
        l0["parsed_as"] = "zip" if detected == "ZIP" else "error"
        error = {"kind": "detect"} if detected == "error" else None
        out.append({"rec": "file", "file": [C.enc_name(rel)], "l0": l0, "l1": None, "error": error})
        stats["files"] += 1
    else:
        env = UnityPy.Environment()
        try:
            # The input's name, for the name rule (spec 4.2).
            f = env.load_file(data, name=rel)
        except MemoryError:
            raise
        except Exception as e:  # noqa: BLE001 - a failed load is a recorded outcome
            l0["parsed_as"] = "error"
            out.append(
                {
                    "rec": "file",
                    "file": [C.enc_name(rel)],
                    "l0": l0,
                    "l1": None,
                    "error": {"kind": "parse", "note": note(e)},
                }
            )
            stats["files"] += 1
            stats["load_errors"] += 1
        else:
            stats["t_load"] += time.perf_counter() - t0
            l0["parsed_as"] = parsed_as(f)
            emit_file(f, [C.enc_name(rel)], l0, levels, out, stats)
    text = "".join(C.dumps(r) + "\n" for r in out)
    stats["bytes_in"] += len(data)
    stats["bytes_out"] += len(text.encode("utf-8"))
    stats["t_total"] += time.perf_counter() - t0
    return rel, text, stats, l0["sha256"]


def crash_result(job, why):
    """The one file record of an input whose process died: error kind "crash"."""
    path, rel, _ = job
    with open(path, "rb") as fh:
        data = fh.read()
    sha = hashlib.sha256(data).hexdigest()
    l0 = {"type": detect(data), "parsed_as": "error", "size": len(data), "sha256": sha}
    rec = {
        "rec": "file",
        "file": [C.enc_name(rel)],
        "l0": l0,
        "l1": None,
        "error": {"kind": "crash", "note": note(why)},
    }
    text = C.dumps(rec) + "\n"
    stats = Counter(files=1, crashes=1, bytes_in=len(data), bytes_out=len(text.encode("utf-8")))
    return rel, text, stats, sha


class DumperError(RuntimeError):
    """The dumper failed on an input in its own code, which no input explains, so the run stops."""
