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

import lz4
import lz4.block
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


def string_to_null_cases() -> tuple[list[str], list[dict]]:
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
    return [d.hex() for d in datas], out


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


def lz4_length(n: int) -> bytes:
    """An LZ4 length extension: 255s, then the rest."""
    out = bytearray()
    while n >= 255:
        out.append(255)
        n -= 255
    out.append(n)
    return bytes(out)


def lz4_sequence(literals: bytes, match: tuple[int, int] | None) -> bytes:
    """One LZ4 sequence: token, literal length, literals, then `(offset, length)` of a match."""
    lit = len(literals)
    ml = match[1] - 4 if match else 0
    out = bytearray([(min(lit, 15) << 4) | min(ml, 15)])
    if lit >= 15:
        out += lz4_length(lit - 15)
    out += literals
    if match:
        out += match[0].to_bytes(2, "little")
        if ml >= 15:
            out += lz4_length(ml - 15)
    return bytes(out)


def lz4_cases() -> list[dict]:
    """`lz4.block.decompress(data, uncompressed_size)` (helpers/CompressionHelper.py:83-94):
    python-lz4 over liblz4's LZ4_decompress_safe, for a grid of blocks around its end-of-block
    conditions, at declared sizes around each block's real output."""
    text = bytes(range(1, 256))
    blocks = [b""]
    for n in (0, 1, 4, 5, 7, 8, 11, 12, 13, 14, 15, 16, 17, 254, 255, 256, 270):
        blocks.append(lz4_sequence(text[:n] if n <= 255 else (text * 2)[:n], None))
    for l1 in (1, 4, 8, 12, 15, 16):
        for ml in (4, 5, 8, 15, 19, 40):
            for offset in sorted({1, l1}):
                head = lz4_sequence(text[:l1], (offset, ml))
                for l2 in (0, 1, 4, 5, 6, 7, 8, 11, 12, 13, 15, 16):
                    blocks.append(head + (lz4_sequence(text[100 : 100 + l2], None) if l2 else b""))
    # offsets outside what was produced, and offset 0
    for offset in (0, 5, 9):
        blocks.append(lz4_sequence(text[:4], (offset, 4)) + lz4_sequence(text[:6], None))
    out = []
    for block in blocks:
        try:
            real = len(lz4.block.decompress(block, uncompressed_size=1 << 16))
        except lz4.block.LZ4BlockError:
            real = 16
        results, output = [], None
        for declared in sorted({0, 1, real - 1, real, real + 1, real + 4, real + 5, real + 8, real + 12, real + 13, real + 64}):
            if declared < 0:
                continue
            try:
                got = lz4.block.decompress(block, uncompressed_size=declared)
                output = got.hex()
                results.append([declared, len(got)])
            except lz4.block.LZ4BlockError:
                results.append([declared, None])
        out.append({"block": block.hex(), "output": output, "declared": results})
    return out


def fixtures() -> dict[str, object]:
    meta = {"generator": "tools/fixtures/scaffold.py", "unitypy": UnityPy.__version__}
    string_data, string_cases = string_to_null_cases()
    return {
        "reader.json": {
            **meta,
            "read": read_cases(),
            "unpack_from": unpack_cases(),
            "read_u_byte": byte_cases(),
            "align": align_cases(),
            "string_to_null_data": string_data,
            "string_to_null": string_cases,
        },
        "pptr_external.json": {**meta, "cases": external_cases()},
        "lz4_blocks.json": {
            **meta,
            "python_lz4": lz4.__version__,
            "liblz4": lz4.library_version_string(),
            "cases": lz4_cases(),
        },
    }


def dump(body: dict) -> str:
    """JSON with one list element per line, so a regenerated fixture diffs by case."""

    def compact(v: object) -> str:
        return json.dumps(v, sort_keys=True, separators=(",", ":"))

    items = sorted(body.items())
    lines = ["{"]
    for i, (key, value) in enumerate(items):
        comma = "," if i < len(items) - 1 else ""
        if isinstance(value, list):
            lines.append(f" {json.dumps(key)}: [")
            lines += [f"  {compact(e)}" + ("," if j < len(value) - 1 else "") for j, e in enumerate(value)]
            lines.append(" ]" + comma)
        else:
            lines.append(f" {json.dumps(key)}: {compact(value)}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2":
        print(f"needs UnityPy 1.25.2, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    stale = []
    for name, body in fixtures().items():
        text = dump(body)
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
