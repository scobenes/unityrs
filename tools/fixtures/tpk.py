"""G7's UnityPy side (docs/plans/core.md sections 6 and 7.2): what UnityPy 1.25.2's
helpers/Tpk.py builds for every class of a TPK at every version it lists.

Runs in the oracle image. Two parts:

- The synthetic set. TPKs built here in code, each a case UnityPy's own TPK cannot show
  (a class for each root-flag combination 0, 64, 128 and 192, each followed by a class
  with both roots; versions whose type byte is 0-6 and 255; a 300-deep chain; entries
  out of order, removed classes, a class listed twice; ids past their tables; counts
  read as UnityPy reads them; LZ4 and LZMA payloads), run through `Tpk.py`
  (`TpkFile(...).GetDataBlob()`, then `get_typetree_node` per class and version with
  the blob in place of the bundled one). The TPKs (hex) and UnityPy's outcomes (JSON)
  are committed under crates/unityrs-tpk/tests/fixtures/tpk/, and
  `cargo test -p unityrs-tpk --test g7` replays them without Python.
- The oracle image's own TPK (`UnityPy/resources/lzma.tpk`). Nothing from it is
  committed (decision 10): its outcomes are written outside the repository and
  compared at test time.

An outcome is UnityPy's node list (every node in pre-order, each with all ten
`TypeTreeNode` fields the dump reads), or "none" where UnityPy raises `KeyError`,
`ValueError` or `AssertionError` (`no_typetree`), or "error" with the exception's name
for anything else (an id past its table).

    scripts/dev python tools/fixtures/tpk.py            # write the synthetic fixtures
    scripts/dev python tools/fixtures/tpk.py --check    # regenerate and compare them, then
                                                        # run G7 on the image's TPK
    scripts/dev python tools/fixtures/tpk.py --outcomes OUT [--tpk PATH]
                                                        # UnityPy's outcomes for a TPK
                                                        # (default: the image's), as JSON lines
    scripts/dev python tools/fixtures/tpk.py --survey   # counts about the image's TPK
"""

from __future__ import annotations

import argparse
import json
import lzma
import os
import struct
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from io import BytesIO
from pathlib import Path

import lz4.block
import UnityPy
from UnityPy.helpers import Tpk
from UnityPy.helpers.UnityVersion import UnityVersion

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-tpk/tests/fixtures/tpk"
UNITYPY = "1.25.2"

MAGIC = 0x2A4B5054  # b"TPK*"
NONE, LZ4, LZMA = 0, 1, 2
I32_MAX = 2**31 - 1


def image_tpk() -> Path:
    return Path(UnityPy.__file__).parent / "resources" / "lzma.tpk"


# --- building TPKs --------------------------------------------------------------------


def ver(major: int, minor: int = 0, build: int = 0, typ: int = 3, number: int = 1) -> int:
    """A version packed as a TPK stores it (`UnityVersion.from_list`)."""
    return (major << 48) | (minor << 32) | (build << 16) | (typ << 8) | number


def node(type_name: int, name: int, subs: list[int], byte_size: int = -1, version: int = 1, meta: int = 0x4000):
    return (type_name, name, byte_size, version, 0, meta, subs)


def cls(flags: int, editor: int | None = None, release: int | None = None):
    return (flags, editor, release)


def both(root: int):
    return cls(192, root, root)


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if not n:
            out.append(byte)
            return bytes(out)
        out.append(byte | 0x80)


def blob(
    versions: list[int],
    classes: list[tuple[int, list]],
    nodes: list[tuple],
    strings: list[str],
    common: list[tuple[int, int]] = (),
    indices: list[int] = (),
    counts: dict[str, int] | None = None,
) -> bytes:
    """A type-tree blob as `TpkTypeTreeBlob` reads it (`Tpk.py:232-240`).

    `counts` replaces a count (`versions`, `classes`, `entries` (each class's),
    `common`, `indices`, `nodes`, `strings`) with another value; for a count of zero or
    less the items are not written.
    """
    counts = counts or {}

    def n(key: str, items) -> tuple[bytes, bool]:
        value = counts.get(key, len(items))
        return struct.pack("<i", value), value > 0

    out = bytearray(struct.pack("<q", 1))
    head, write = n("versions", versions)
    out += head
    if write:
        out += b"".join(struct.pack("<Q", v) for v in versions)
    head, write = n("classes", classes)
    out += head
    for class_id, entries in classes if write else ():
        out += struct.pack("<i", class_id)
        head, write_entries = n("entries", entries)
        out += head
        for version, c in entries if write_entries else ():
            out += struct.pack("<Q", version)
            if c is None:
                out += b"\0"
                continue
            flags, editor, release = c
            out += b"\1" + struct.pack("<HHB", 7, 0, flags)
            out += b"".join(struct.pack("<H", r) for r in (editor, release) if r is not None)
    head, write = n("common", common)
    out += head
    for v, c in common if write else ():
        out += struct.pack("<QB", v, c)
    head, write = n("indices", indices)
    out += head
    if write:
        out += b"".join(struct.pack("<H", i) for i in indices)
    head, write = n("nodes", nodes)
    out += head
    for type_name, name, byte_size, version, type_flags, meta, subs in nodes if write else ():
        out += struct.pack("<HHihBIH", type_name, name, byte_size, version, type_flags, meta, len(subs))
        out += b"".join(struct.pack("<H", s) for s in subs)
    head, write = n("strings", strings)
    out += head
    for s in strings if write else ():
        data = s.encode()
        out += varint(len(data)) + data
    return bytes(out)


def tpk_file(payload: bytes, compression: int = NONE, sizes: tuple[int, int] | None = None) -> bytes:
    """A TPK file around `payload` (`Tpk.py:146-174`), compressed as asked."""
    if compression == LZ4:
        data = lz4.block.compress(payload, store_size=False)
    elif compression == LZMA:
        # `decompress_lzma` (helpers/CompressionHelper.py): 5 bytes of properties, then
        # a raw LZMA1 stream.
        lc, lp, pb, dict_size = 3, 0, 2, 1 << 16
        filters = [{"id": lzma.FILTER_LZMA1, "dict_size": dict_size, "lc": lc, "lp": lp, "pb": pb}]
        data = struct.pack("<BI", (pb * 5 + lp) * 9 + lc, dict_size)
        data += lzma.compress(payload, format=lzma.FORMAT_RAW, filters=filters)
    else:
        data = payload
    compressed, uncompressed = sizes or (len(data), len(payload))
    return struct.pack("<IBBBBIii", MAGIC, 1, compression, 0, 0, 0, compressed, uncompressed) + data


def chain(n: int) -> list[tuple]:
    return [node(0, 1, [i + 1] if i + 1 < n else []) for i in range(n)]


def strings(n: int) -> list[str]:
    return [f"s{i}" for i in range(n)]


def root_flags_blob() -> bytes:
    """Classes 10, 20, 30, 40 with root flags 0, 64, 128, 192, each followed by a class
    with both roots; class 45 carries other flag bits with its release root."""
    nodes = [node(0, 1, [1]), node(2, 3, []), node(4, 5, [1, 1], meta=0)]
    at = ver(2018, 4, 2)
    classes = []
    for class_id, c in [(10, cls(0)), (20, cls(64, editor=2)), (30, cls(128, release=0)), (40, cls(192, 2, 0))]:
        classes.append((class_id, [(at, c)]))
        classes.append((class_id + 1, [(at, both(2))]))
    classes.append((45, [(at, cls(1 | 4 | 16 | 128, release=2))]))
    return blob([ver(5), at, ver(2022, 1)], classes, nodes, strings(6))


def synthetic_cases() -> Iterator[tuple[str, bytes, int]]:
    """`(name, TPK bytes, the tree depth to open it with)`."""
    yield "root_flags", tpk_file(root_flags_blob()), 256

    # Entries: removed and re-added, out of order, empty, a class listed twice.
    nodes = [node(0, 1, [1]), node(2, 3, []), node(4, 5, [])]
    entries = [
        (50, [(ver(2017), both(0)), (ver(2018), None), (ver(2019), both(2))]),
        (51, [(ver(2019), both(0)), (ver(2017), both(2)), (ver(2021), both(1))]),
        (52, []),
        (53, [(ver(2017), both(0))]),
        (54, [(ver(2018), None)]),
        (53, [(ver(2017), both(2))]),
    ]
    versions = [ver(2016), ver(2017), ver(2017, 3), ver(2018), ver(2019), ver(2020), ver(2021), ver(2022)]
    yield "entries", tpk_file(blob(versions, entries, nodes, strings(6))), 256

    # Type bytes 0-5, 6 and 255, in the version list, the entries and the common strings.
    types = [0, 1, 2, 3, 4, 5, 6, 255]
    at = [ver(2, typ=t) for t in types]
    nodes = [node(i % 3, 3, []) for i in range(8)]
    entries = [(60, [(v, both(i)) for i, v in enumerate(at)])]
    versions = sorted({*at, *(v - 1 for v in at), *(v + 1 for v in at), ver(1), ver(3)})
    common = [(v, 1) for v in at]
    yield "version_types", tpk_file(blob(versions, entries, nodes, strings(4), common, [0])), 256

    # A 300-deep chain: UnityPy builds it, levels 0 to 299.
    yield "deep_chain_300", tpk_file(blob([ver(2020)], [(1, [(ver(2020), both(0))])], chain(300), strings(2))), 300

    # Ids past their tables: UnityPy raises IndexError.
    nodes = [node(0, 1, [1]), node(2, 3, []), node(0, 1, [9]), node(0, 9, [])]
    entries = [(70, [(ver(2020), both(9))]), (71, [(ver(2020), both(2))]), (72, [(ver(2020), both(3))])]
    entries.append((73, [(ver(2020), both(0))]))
    yield "ids_past_their_tables", tpk_file(blob([ver(2020)], entries, nodes, strings(4))), 256

    # Counts: a negative count UnityPy reads with range() is no items; one it reads as a
    # struct format raises; a huge one runs out of bytes.
    one = ([ver(2020)], [(1, [(ver(2020), both(0))])], [node(0, 1, [1]), node(2, 3, [])], strings(4))
    for key in ("classes", "entries", "common", "nodes", "strings", "versions", "indices"):
        for value in (-1, I32_MAX):
            yield f"count_{key}_{value}", tpk_file(blob(*one, counts={key: value})), 256

    # No classes at all, and a truncated file.
    yield "no_classes", tpk_file(blob([], [], [], [])), 256
    full = tpk_file(root_flags_blob())
    yield "truncated", full[: len(full) - 5], 256
    yield "negative_compressed_size", tpk_file(root_flags_blob(), sizes=(-1, 0)), 256

    # Compressed payloads.
    yield "lz4", tpk_file(root_flags_blob(), LZ4), 256
    yield "lzma", tpk_file(root_flags_blob(), LZMA), 256


# --- running Tpk.py -------------------------------------------------------------------


def node_rows(tree) -> list[list]:
    return [
        [
            n.m_Level,
            n.m_Type,
            n.m_Name,
            n.m_ByteSize,
            n.m_Version,
            n.m_TypeFlags,
            n.m_VariableCount,
            n.m_Index,
            n.m_MetaFlag,
            n.m_RefTypeHash,
        ]
        for n in tree.traverse()
    ]


def class_outcomes(class_id: int, versions: list[int]) -> dict:
    """UnityPy's outcome for `class_id` at each version, deduplicated: `picks[i]` is the
    index in `outcomes` of the outcome at `versions[i]`."""
    outcomes: list[dict] = []
    seen: dict[object, int] = {}
    picks = []
    for v in versions:
        try:
            tree = Tpk.get_typetree_node(class_id, UnityVersion(v))
            key: object = ("tree", id(tree))
            if key not in seen:
                outcome = {"rows": node_rows(tree)}
        except (KeyError, ValueError, AssertionError) as e:
            key = ("none", type(e).__name__)
            outcome = {"none": type(e).__name__}
        except Exception as e:  # noqa: BLE001 - an IndexError from a walk past a table
            key = ("error", type(e).__name__)
            outcome = {"error": type(e).__name__}
        if key not in seen:
            seen[key] = len(outcomes)
            outcomes.append(outcome)
        picks.append(seen[key])
    return {"class": class_id, "picks": picks, "outcomes": outcomes}


class Installed:
    """Puts `blob` in place of the bundled TPK, with empty caches, for the `with` block."""

    def __init__(self, blob: Tpk.TpkTypeTreeBlob) -> None:
        self.blob = blob

    def __enter__(self) -> None:
        self.saved = Tpk.TPKTYPETREE
        Tpk.TPKTYPETREE = self.blob
        Tpk.CLASSES_CACHE.clear()
        Tpk.NODES_CACHE.clear()

    def __exit__(self, *exc: object) -> None:
        Tpk.TPKTYPETREE = self.saved
        Tpk.CLASSES_CACHE.clear()
        Tpk.NODES_CACHE.clear()


def load(data: bytes) -> Tpk.TpkTypeTreeBlob:
    """`init()`'s load (`Tpk.py:36-39`)."""
    found = Tpk.TpkFile(BytesIO(data)).GetDataBlob()
    assert isinstance(found, Tpk.TpkTypeTreeBlob)
    return found


def outcomes(data: bytes) -> Iterator[dict]:
    """A header, then one record per class (and one for a class the TPK does not hold)."""
    try:
        found = load(data)
    except Exception as e:  # noqa: BLE001 - whatever the reader raises
        yield {"load": type(e).__name__}
        return
    versions = [int(v) for v in found.Versions]
    yield {"load": None, "class_count": len(found.ClassInformation), "versions": [str(v) for v in versions]}
    with Installed(found):
        absent = max(found.ClassInformation, default=0) + 1
        for class_id in [*found.ClassInformation, absent]:
            yield class_outcomes(class_id, versions)


def synthetic() -> list[dict]:
    cases = []
    for name, data, depth in synthetic_cases():
        records = list(outcomes(data))
        head, classes = records[0], records[1:]
        cases.append({"name": name, "tpk": data.hex(), "depth": depth, **head, "classes": classes})
    return cases


# --- output ---------------------------------------------------------------------------


def dump(cases: list[dict]) -> str:
    """One case per line, so a regenerated fixture diffs by case."""
    lines = [json.dumps(c, sort_keys=True, separators=(",", ":")) for c in cases]
    return "{\n \"cases\": [\n  " + ",\n  ".join(lines) + "\n ],\n \"unitypy\": " + json.dumps(UNITYPY) + "\n}\n"


def write_outcomes(tpk: Path, out: Path) -> None:
    with out.open("w") as f:
        for record in outcomes(tpk.read_bytes()):
            f.write(json.dumps(record, separators=(",", ":")) + "\n")


def survey() -> None:
    found = load(image_tpk().read_bytes())
    trees = rows = deepest = largest = 0
    with Installed(found):
        for info in found.ClassInformation.values():
            for _, c in info:
                if c is None or c.ReleaseRootNode is None:
                    continue
                tree = node_rows(Tpk.generate_node(c))
                trees += 1
                rows += len(tree)
                largest = max(largest, len(tree))
                deepest = max(deepest, max(r[0] for r in tree))
    print(
        f"classes {len(found.ClassInformation)}, versions {len(found.Versions)}, "
        f"entries {sum(len(i) for i in found.ClassInformation.values())}, trees {trees}, "
        f"rows {rows}, largest tree {largest} rows, deepest level {deepest}, "
        f"largest meta flag {max(n.MetaFlag for n in found.NodeBuffer)}"
    )


def run_g7(tpk: Path) -> int:
    """G7 on `tpk`: UnityPy's outcomes outside the repository, then unityrs-tpk's test."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "outcomes.jsonl"
        write_outcomes(tpk, out)
        env = dict(os.environ, UNITYRS_ORACLE_TPK=str(tpk), UNITYRS_ORACLE_TPK_OUTCOMES=str(out))
        cmd = ["cargo", "test", "--locked", "-p", "unityrs-tpk", "--test", "g7", "--", "--exact", "image_tpk", "--nocapture"]
        run = subprocess.run(cmd, cwd=ROOT, env=env, check=False, stdout=subprocess.PIPE, text=True)
        sys.stdout.write(run.stdout)
        if run.returncode != 0:
            return run.returncode
        # A filter that matches no test passes with nothing run: G7 cannot skip.
        if "test result: ok. 1 passed;" not in run.stdout:
            print("G7: cargo ran no image_tpk test", file=sys.stderr)
            return 1
        return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="compare the fixtures, then run G7 on the image's TPK")
    ap.add_argument("--outcomes", type=Path, help="write UnityPy's outcomes for --tpk here, as JSON lines")
    ap.add_argument("--tpk", type=Path, help="the TPK for --outcomes (default: the image's)")
    ap.add_argument("--survey", action="store_true", help="print counts about the image's TPK")
    args = ap.parse_args()
    if UnityPy.__version__ != UNITYPY:
        print(f"needs UnityPy {UNITYPY}, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    if args.survey:
        survey()
        return 0
    if args.outcomes:
        write_outcomes(args.tpk or image_tpk(), args.outcomes)
        return 0
    text = dump(synthetic())
    path = OUT / "synthetic.json"
    if args.check:
        if not path.exists() or path.read_text() != text:
            print("stale fixture (run tools/fixtures/tpk.py): " + str(path.relative_to(ROOT)), file=sys.stderr)
            return 1
        return run_g7(image_tpk())
    OUT.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
