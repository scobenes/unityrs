"""The canonical dump encoding, shared by the UnityPy dumper and the comparer.

docs/CANONICAL_DUMP.md is the specification; this module is its Python
reference. It needs only the standard library, so compare.py runs anywhere.

The JSON layer is canonical_json.py, the value encoder canonical_encode.py, the
decoder the comparer reads values with canonical_decode.py, and the oracle stack
a reference must come from is pinned in oracle_stack.py; this module names their
public items too, so the tools reach all of it as canonical's.
"""

from __future__ import annotations

import re
from pathlib import Path

from canonical_decode import (  # noqa: F401
    F32,
    F64,
    Blob,
    Dict,
    FormatError,
    Tuple,
    decode,
    decode_key,
)
from canonical_encode import (  # noqa: F401
    FLOAT_WIDTH,
    INLINE_MAX,
    U8_TYPES,
    EncodeError,
    ValueEncoder,
    enc_hex,
    enc_name,
    enc_raw,
    enc_str,
    f32_hex,
    f64_hex,
    u8_bytes,
)
from canonical_json import LONG_LINE, SHARED_NODES, dumps, exact_ints, loads, sharing_hook  # noqa: F401
from oracle_stack import (  # noqa: F401
    INSTALLERS,
    ORACLE_BOOST_SHA256,
    ORACLE_PRODUCER,
    ORACLE_RECORDS,
    ORACLE_REQUIREMENTS,
    ORACLE_UNITYPY,
    installed_packages,
    normalise_package,
    oracle_pins,
    oracle_records,
    stack_problems,
)

FORMAT = "unityrs-canonical-dump"
VERSION = 1
LEVELS = ("L0", "L1", "L2", "L3")


def repo_roots(start: Path) -> list[Path]:
    """Every ancestor of `start` that holds a .git (worktrees nest in the repository).

    Dumps, stats and reports name corpus inputs, so the tools refuse to write
    them under any of these.
    """
    return [p for p in [start, *start.parents] if (p / ".git").exists()]


def inside_repo(path: str) -> Path | None:
    """The repository root `path` resolves into (symlinks followed), or None."""
    resolved = Path(path).resolve()
    return next((r for r in repo_roots(Path(__file__).resolve().parent) if resolved.is_relative_to(r)), None)


# ------------------------------------------------------------ canonical form
#
# The rules a single line can break whatever the other dump holds: section 4's
# member lists and the value encodings of section 3 that the value shows on its
# own. Packing by node type, float width and UInt8-vs-int need the typetree, so
# only the comparison with the reference can check them.

RECORD_MEMBERS = {
    "dump": ("rec", "format", "version", "levels", "producer"),
    "file": ("rec", "file", "l0", "l1", "error"),
    "object": ("rec", "file", "path_id", "l1", "l2", "l3"),
    "end": ("rec", "files", "objects"),
}
L0_MEMBERS = ("type", "parsed_as", "size", "sha256")
# Section 4.2's values for l0: check_file_type's verdicts, and what UnityPy builds
# from each. An entry gets only what its verdict builds. An input may also fail
# ("error", whatever its verdict), and an input detected as a zip is "zip", which
# an entry never is (UnityPy's parser has no zip branch, so it keeps a reader).
L0_TYPES = ("AssetsFile", "BundleFile", "WebFile", "ResourceFile", "ZIP", "error")
L0_PARSED_AS = ("SerializedFile", "BundleFile", "WebFile", "raw", "zip", "error")
L0_ENTRY_BUILDS = {
    "AssetsFile": ("SerializedFile", "raw"),  # raw by the name rule
    "BundleFile": ("BundleFile",),
    "WebFile": ("WebFile",),
    "ResourceFile": ("raw",),
    "ZIP": ("raw",),
    "error": (),  # detection raising fails the input (section 4.2)
}
L0_INPUT_BUILDS = {**L0_ENTRY_BUILDS, "ZIP": ("zip",)}
# Error kinds: a file's (section 4.2) and an object's l2 (section 4.3).
FILE_ERROR_KINDS = ("detect", "parse", "crash", "dumper")
L2_ERROR_KINDS = ("no_typetree", "check_read", "out_of_bounds", "negative_length", "ref_type", "encode", "other")
FILE_L1_MEMBERS = {
    "bundle": (
        "kind",
        "signature",
        "format",
        "version_player",
        "version_engine",
        "engine_version_packed",
        "flags",
        "flags_class",
        "block_alignment",
        "encrypted",
        "hash",
        "crc",
        "blocks_info",
        "blocks",
        "directory",
    ),
    "web": ("kind", "signature", "packer", "directory"),
    "serialized": (
        "kind",
        "header",
        "unity_version",
        "version_packed",
        "target_platform",
        "enable_type_tree",
        "big_id_enabled",
        "types",
        "object_order",
        "script_types",
        "externals",
        "ref_types",
        "user_information",
        "container",
    ),
}
SERIALIZED_HEADER_MEMBERS = ("metadata_size", "file_size", "version", "data_offset", "endian", "reserved", "unknown")
TYPE_MEMBERS = (
    "class_id",
    "is_stripped_type",
    "script_type_index",
    "script_id",
    "old_type_hash",
    "nodes",
    "type_dependencies",
    "class_name",
    "namespace",
    "assembly",
)
FILE_ERROR_MEMBERS = ("kind",)
OBJECT_SECTIONS = (
    ("L1", "l1", ("type_id", "class_id", "byte_start", "byte_size", "is_destroyed", "is_stripped")),
    ("L2", "l2", ("root_type", "source", "error", "value")),
    ("L3", "l3", ("raw_size", "raw_sha256", "resave", "resave_size", "resave_sha256", "first_diff")),
)
L2_ERROR_MEMBERS = ("kind", "bytes_read")
# The l1 kind each parse result carries; a file parsed as raw, zip or error has no l1.
FILE_L1_KIND = {"SerializedFile": "serialized", "BundleFile": "bundle", "WebFile": "web"}
# Name positions (the metadata-string rule) in the l1 of each kind: members, and
# (member, row length, name columns) for lists of rows.
L1_NAMES = {
    "bundle": (("signature", "version_player", "version_engine"), (("directory", 4, (3,)),)),
    "web": (("signature",), (("directory", 4, (3,)),)),
    "serialized": (("unity_version", "user_information"), (("externals", 4, (0, 3)), ("container", 5, (0,)))),
}
TYPE_NAMES = ("class_name", "namespace", "assembly")

_EVEN_HEX = re.compile(r"(?:[0-9a-f]{2})*")
_SHA256 = re.compile(r"[0-9a-f]{64}")
_PACKED = {
    "f32": re.compile(r"[0-9a-f]{8}"),
    "f64": re.compile(r"[0-9a-f]{16}"),
    "f32s": re.compile(r"(?:[0-9a-f]{8})+"),
    "f64s": re.compile(r"(?:[0-9a-f]{16})+"),
}
_RAW_TAGS = ("bytes", "u8s", "str")


def _is_utf8(raw: bytes) -> bool:
    try:
        raw.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return True


def _is_hex_name(v) -> bool:
    """Whether v has the hex form of a name: {"t":"str","hex":H} with lowercase, even-length H."""
    return (
        type(v) is dict
        and list(v) == ["t", "hex"]
        and v["t"] == "str"
        and type(v["hex"]) is str
        and _EVEN_HEX.fullmatch(v["hex"]) is not None
    )


def check_name(v, path, out, nullable=True):
    """The metadata-string rule (section 4.2): a JSON string, or the hex of bytes that are not valid UTF-8."""
    if type(v) is str or (v is None and nullable):
        return
    if not _is_hex_name(v):
        out.append((path, "bad_name", v))
    elif _is_utf8(bytes.fromhex(v["hex"])):
        out.append((path, "hex_name_is_utf8", v))


def _row_names(rows, length, columns, path, out):
    """check_name on the given columns of a list of rows of `length` members."""
    if rows is None:
        return
    if type(rows) is not list:
        out.append((path, "malformed", rows))
        return
    for i, row in enumerate(rows):
        if type(row) is not list or len(row) != length:
            out.append((f"{path}[{i}]", "malformed", row))
            continue
        for col in columns:
            check_name(row[col], f"{path}[{i}][{col}]", out)


def _l1_names(l1, out):
    names, rows = L1_NAMES[l1["kind"]]
    for member in names:
        check_name(l1[member], f"l1.{member}", out)
    for member, length, columns in rows:
        _row_names(l1[member], length, columns, f"l1.{member}", out)
    for member in ("types", "ref_types") if l1["kind"] == "serialized" else ():
        for i, t in enumerate(l1[member] if type(l1[member]) is list else []):
            if type(t) is dict and list(t) == list(TYPE_MEMBERS):
                for name in TYPE_NAMES:
                    check_name(t[name], f"l1.{member}[{i}].{name}", out)
                _row_names(t["nodes"], 10, (1, 2), f"l1.{member}[{i}].nodes", out)


def _members(obj, expected, path, out, note=False) -> bool:
    """Whether obj is an object whose members are exactly `expected`, in order (plus a last `note`)."""
    if type(obj) is not dict:
        out.append((path, "not_object", obj))
        return False
    keys = list(obj)
    if note and keys and keys[-1] == "note":
        keys.pop()
    if keys != list(expected):
        out.append((path, "members", list(obj)))
        return False
    return True


def check_record(rec, levels) -> list:
    """Every section 2-4 rule this record breaks on its own, as (path, kind, detail) triples.

    `levels` is what its dump's header declares: a section of an undeclared
    level must be null, and one of a declared level must be there wherever
    the record has that level.
    """
    out = []
    kind = rec.get("rec")
    if not _members(rec, RECORD_MEMBERS.get(kind, ()) if type(kind) is str else (), "record", out):
        return out
    if kind in ("file", "object") and type(rec["file"]) is list:
        for i, name in enumerate(rec["file"]):
            check_name(name, f"file[{i}]", out, nullable=False)
    if kind == "file":
        l0_ok = _members(rec["l0"], L0_MEMBERS, "l0", out)
        if l0_ok:
            _check_l0(rec["l0"], type(rec["file"]) is list and len(rec["file"]) == 1, out)
        parsed = rec["l0"]["parsed_as"] if l0_ok else None
        # parsed_as says whether the file has an error and which l1 it has.
        if l0_ok and (parsed == "error") != (rec["error"] is not None):
            out.append(("error", "error_mismatch", f"error {rec['error']!r} for parsed_as {parsed!r}"))
        wanted = FILE_L1_KIND.get(parsed) if type(parsed) is str else None
        l1 = rec["l1"]
        if l1 is not None and "L1" not in levels:
            out.append(("l1", "undeclared_level", "L1"))
        elif l1 is None:
            if wanted and "L1" in levels:
                out.append(("l1", "missing_section", f"L1 of a {parsed}"))
        else:
            l1_kind = l1.get("kind") if type(l1) is dict else None
            members = FILE_L1_MEMBERS.get(l1_kind) if type(l1_kind) is str else None
            if l0_ok and l1_kind != wanted:
                out.append(("l1.kind", "kind_mismatch", f"{l1_kind!r} for parsed_as {parsed!r}"))
            if _members(l1, members or (), "l1", out):
                if l1["kind"] == "serialized":
                    _members(l1["header"], SERIALIZED_HEADER_MEMBERS, "l1.header", out)
                    for name in ("types", "ref_types"):
                        if type(l1[name]) is list:
                            for i, t in enumerate(l1[name]):
                                _members(t, TYPE_MEMBERS, f"l1.{name}[{i}]", out)
                _l1_names(l1, out)
        if rec["error"] is not None and _members(rec["error"], FILE_ERROR_MEMBERS, "error", out, note=True):
            error_kind = rec["error"]["kind"]
            if error_kind not in FILE_ERROR_KINDS:
                out.append(("error.kind", "unknown_value", error_kind))
            elif (
                l0_ok
                and (error_kind == "detect") != (rec["l0"]["type"] == "error")
                and error_kind not in ("crash", "dumper")
            ):
                # detection raising is the "detect" error; a crash, or the dumper's own failure, can follow either
                out.append(("error.kind", "error_mismatch", f"{error_kind!r} for type {rec['l0']['type']!r}"))
    elif kind == "object":
        for level, name, members in OBJECT_SECTIONS:
            section = rec[name]
            if section is None:
                if level in levels:
                    out.append((name, "missing_section", level))
                continue
            if level not in levels:
                out.append((name, "undeclared_level", level))
            elif _members(section, members, name, out, note=name == "l3") and name == "l2":
                _check_l2(section, out)
    return out


def _check_l0(l0, is_input, out):
    """Section 4.2's values and pairs for l0's members (`is_input`: the record is an input's, not an entry's)."""
    typ, parsed, size, sha = (l0[k] for k in L0_MEMBERS)
    typ_ok = type(typ) is str and typ in L0_TYPES
    if not typ_ok:
        out.append(("l0.type", "unknown_value", typ))
    if type(parsed) is not str or parsed not in L0_PARSED_AS:
        out.append(("l0.parsed_as", "unknown_value", parsed))
    elif typ_ok and parsed not in (L0_INPUT_BUILDS if is_input else L0_ENTRY_BUILDS)[typ] + (
        ("error",) if is_input else ()
    ):
        where = "an input" if is_input else "an entry"
        out.append(("l0.parsed_as", "l0_mismatch", f"{parsed!r} for type {typ!r} on {where}"))
    if type(size) is not int or size < 0:
        out.append(("l0.size", "bad_value", size))
    if type(sha) is not str or not _SHA256.fullmatch(sha):
        out.append(("l0.sha256", "bad_value", sha))


def _check_l2(l2, out):
    error = l2["error"]
    error_kind = None
    if error is not None and _members(error, L2_ERROR_MEMBERS, "l2.error", out, note=True):
        error_kind = error["kind"]
        if error_kind not in L2_ERROR_KINDS:
            out.append(("l2.error.kind", "unknown_value", error_kind))
    check_name(l2["root_type"], "l2.root_type", out)
    # A parse that finished has a value; so does check_read, which re-reads
    # without the check. Every other error has none.
    if (error is None or error_kind == "check_read") != (l2["value"] is not None):
        out.append(
            (
                "l2.value",
                "value_mismatch",
                f"value {'null' if l2['value'] is None else 'present'} for error {error_kind}",
            )
        )
    check_value(l2["value"], "l2.value", out)


def check_value(tv, path, out):
    """Append (path, kind, detail) for each section 3 rule the tagged value tv breaks, in document order, at
    any depth (a stack, not recursion: a value nests as deep as UnityPyBoost reads)."""
    # Each task is a value and its path, or a dict's entries from the next one on (_DictRest).
    todo = [(tv, path)]
    while todo:
        task = todo.pop()
        if type(task) is _DictRest:
            task.step(todo, out)
            continue
        tv, path = task
        t = type(tv)
        if t is list:
            if any(type(v) is dict and v.get("t") in ("f32", "f64") for v in tv):
                out.append((path, "unpacked", "a list of tagged floats; vectors of float and double pack"))
            todo.extend((tv[i], f"{path}[{i}]") for i in range(len(tv) - 1, -1, -1))
            continue
        if t is float:
            out.append((path, "json_float", tv))
            continue
        if t is not dict:
            continue  # null, bool, int, str
        tag = tv.get("t")
        keys = list(tv)
        if type(tag) is not str:
            out.append((path, "unknown_tag", tag))
        elif tag in _RAW_TAGS:
            _check_raw(tv, tag, keys, path, out)
        elif tag in _PACKED or tag in ("dict", "tuple"):
            if keys != ["t", "v"]:
                out.append((path, "members", keys))
                continue
            v = tv["v"]
            if tag == "dict":
                if type(v) is not list:
                    out.append((path, "malformed", v))
                else:
                    todo.append(_DictRest(v, path))
            elif tag == "tuple":
                if type(v) is not list:
                    out.append((path, "malformed", v))
                    continue
                todo.extend((v[i], f"{path}[{i}]") for i in range(len(v) - 1, -1, -1))
            elif type(v) is not str or not _PACKED[tag].fullmatch(v):
                out.append((path, "empty_packed" if v == "" else "bad_hex", v))
        else:
            out.append((path, "unknown_tag", tag))


class _DictRest:
    """A tagged dict's entries still to check, from `at` on: each entry's own findings, then its value's."""

    __slots__ = ("entries", "path", "at", "seen")

    def __init__(self, entries, path):
        self.entries, self.path, self.at, self.seen = entries, path, 0, set()

    def step(self, todo, out):
        """Check the next well-formed entry's key, queueing its value and then the rest."""
        path = self.path
        while self.at < len(self.entries):
            entry = self.entries[self.at]
            self.at += 1
            if type(entry) is not list or len(entry) != 2:
                out.append((path, "malformed", entry))
                continue
            key, value = entry
            if type(key) is str:
                name, ident = key, ("str", key)
            elif _is_hex_name(key):
                name, ident = key["hex"], ("hex", key["hex"])
                if _is_utf8(bytes.fromhex(name)):
                    out.append((f"{path}.{name}", "hex_name_is_utf8", key))
            else:
                out.append((path, "bad_key", key))
                continue
            if ident in self.seen:
                out.append((f"{path}.{name}", "duplicate_key", key))
            self.seen.add(ident)
            todo.append(self)
            todo.append((value, f"{path}.{name}"))
            return


def _check_raw(tv, tag, keys, path, out):
    if keys == ["t", "hex"]:
        h = tv["hex"]
        if type(h) is not str or not _EVEN_HEX.fullmatch(h):
            out.append((path, "bad_hex", h))
            return
        n, raw = len(h) // 2, bytes.fromhex(h)
        if n > INLINE_MAX:
            out.append((path, "inline_too_long", n))
    elif keys == ["t", "len", "sha256"]:
        n, sha, raw = tv["len"], tv["sha256"], None
        if type(n) is not int or n < 0:
            out.append((path, "bad_len", n))
            return
        if n <= INLINE_MAX:
            out.append((path, "summary_too_short", n))
        if type(sha) is not str or not _SHA256.fullmatch(sha):
            out.append((path, "bad_sha256", sha))
    else:
        out.append((path, "members", keys))
        return
    if tag == "u8s" and n == 0:
        out.append((path, "empty_packed", tv))
    if tag == "str" and raw is not None and _is_utf8(raw):
        out.append((path, "str_is_utf8", tv))
