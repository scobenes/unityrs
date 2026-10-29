"""Oracle fixtures for the rules the core scaffold implements (docs/plans/core.md section 7.1).

Runs in the oracle image against UnityPy 1.25.2 and writes small JSON files under
crates/unityrs-core/tests/fixtures/scaffold/, which `cargo test -p unityrs-core`
replays without Python (crates/unityrs-core/tests/scaffold_fixtures.rs). Every input
is built here in code; no file is read.

    scripts/dev python tools/fixtures/scaffold.py           # write the fixtures
    scripts/dev python tools/fixtures/scaffold.py --check   # regenerate and compare

Integers that can leave the i64 range (positions, sizes) are written as strings.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path
from types import SimpleNamespace

import UnityPy
from UnityPy.classes import PPtr
from UnityPy.streams import EndianBinaryReader

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-core/tests/fixtures/scaffold"

I64_MIN, I64_MAX = -(2**63), 2**63 - 1
I32_MIN, I32_MAX = -(2**31), 2**31 - 1


def reader(data: bytes, pos: int) -> EndianBinaryReader:
    r = EndianBinaryReader(data, "<")
    r.Position = pos
    return r


def positions(n: int) -> list[int]:
    return list(range(-n - 4, n + 5)) + [I64_MIN, -(2**62), 2**62, I64_MAX]


def read_cases() -> list[dict]:
    """`read(size)` (streams/EndianBinaryReader.py:248-255): a Python slice of the view."""
    data = bytes(range(10))
    sizes = [-14, -11, -10, -3, -1, 0, 1, 3, 10, 11, I32_MIN, I32_MAX, I64_MIN, I64_MAX]
    out = []
    for pos in positions(len(data)):
        for size in sizes:
            r = reader(data, pos)
            got = r.read(size)
            # The bytes are distinct, so the first one names the start.
            out.append(
                {
                    "pos": str(pos),
                    "size": str(size),
                    "start": got[0] if got else None,
                    "n": len(got),
                    "new_pos": str(r.Position),
                }
            )
    return out


def unpack_cases() -> list[dict]:
    """Fixed-width reads: `struct.unpack_from(view, Position)` (EndianBinaryReader.py:327-331)."""
    data = bytes(range(10))
    funcs = {2: "read_u_short", 4: "read_u_int", 8: "read_u_long"}
    out = []
    for pos in positions(len(data)):
        for n, name in funcs.items():
            r = reader(data, pos)
            try:
                value = getattr(r, name)()
                start = value & 0xFF  # little-endian: the low byte is the first byte read
                out.append({"pos": str(pos), "n": n, "start": start, "new_pos": str(r.Position)})
            except struct.error:
                out.append({"pos": str(pos), "n": n, "start": None, "new_pos": str(r.Position)})
    return out


def byte_cases() -> list[dict]:
    """One-byte reads: `unpack(fmt, self.read(1))` (EndianBinaryReader.py:91-95), so a slice
    that yields no byte raises after the position moved."""
    data = bytes(range(10))
    out = []
    for pos in positions(len(data)):
        r = reader(data, pos)
        try:
            value = r.read_u_byte()
        except struct.error:
            value = None
        out.append({"pos": str(pos), "value": value, "new_pos": str(r.Position)})
    return out


def align_cases() -> list[dict]:
    """`align_stream(n)` (EndianBinaryReader.py:186-187): Python's floor modulo."""
    out = []
    for n in (4, 16):
        for pos in list(range(-20, 21)) + [I64_MIN, I64_MAX]:
            r = reader(b"", pos)
            r.align_stream(n)
            out.append({"pos": str(pos), "n": n, "new_pos": str(r.Position)})
    return out


def string_to_null_cases() -> list[dict]:
    """`read_string_to_null(max_length)` (EndianBinaryReader.py:275-284): `re.search` clamps
    its start and end to the buffer; a miss raises when `pos + max_length >= len` and
    otherwise reads `max_length` bytes as a slice; a hit sets the absolute end."""
    datas = [b"ab\0cdef\0gh", b"abcdefghij", b"\0\0x"]
    out = []
    for di, data in enumerate(datas):
        for pos in positions(len(data)):
            for max_length in (1, 3, 5, 32767):
                r = reader(data, pos)
                try:
                    s = r.read_string_to_null(max_length).encode("utf8", "surrogateescape")
                    out.append(
                        {
                            "data": di,
                            "pos": str(pos),
                            "max": max_length,
                            "hex": s.hex(),
                            "new_pos": str(r.Position),
                        }
                    )
                except Exception as e:  # noqa: BLE001 - UnityPy raises a bare Exception here
                    out.append({"data": di, "pos": str(pos), "max": max_length, "error": type(e).__name__})
    return {"data": [d.hex() for d in datas], "cases": out}


def external_cases() -> list[dict]:
    """`PPtr.deref`'s external lookup (classes/PPtr.py:56-60): `m_FileID - 1`, a `>= len`
    guard (FileNotFoundError), then Python indexing (a negative id counts from the end,
    below `-len` is IndexError). Run on UnityPy's own `deref` with a stand-in file whose
    `parent` is None, so a found external stops at the next rule ("no container")."""
    out = []
    for n in (0, 1, 3):
        externals = [SimpleNamespace(path=f"ext{i}") for i in range(n)]
        sf = SimpleNamespace(externals=externals, parent=None)
        for file_id in sorted({I32_MIN, -n - 1, -n, -1, 1, n, n + 1, I32_MAX}):
            if file_id == 0:
                continue
            try:
                PPtr(assetsfile=sf, m_FileID=file_id, m_PathID=1).deref()
                outcome = "resolved"
            except IndexError:
                outcome = "below_start"
            except FileNotFoundError as e:
                text = str(e)
                if "invalid m_FileID" in text:
                    outcome = "past_end"
                else:
                    # The external was found: the message names its path.
                    outcome = "index:" + text.split("PPtr points to ext", 1)[1].split(" ", 1)[0]
            out.append({"len": n, "file_id": file_id, "outcome": outcome})
    return out


def fixtures() -> dict[str, object]:
    meta = {"generator": "tools/fixtures/scaffold.py", "unitypy": UnityPy.__version__}
    return {
        "reader.json": {
            **meta,
            "read": read_cases(),
            "unpack_from": unpack_cases(),
            "read_u_byte": byte_cases(),
            "align": align_cases(),
            "string_to_null": string_to_null_cases(),
        },
        "pptr_external.json": {**meta, "cases": external_cases()},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2":
        print(f"needs UnityPy 1.25.2, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    stale = []
    for name, body in fixtures().items():
        text = json.dumps(body, indent=1, sort_keys=True) + "\n"
        path = OUT / name
        if args.check:
            if not path.exists() or path.read_text() != text:
                stale.append(name)
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    if stale:
        print("stale fixtures: " + ", ".join(stale), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
