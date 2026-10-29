"""Oracle fixtures for package C (docs/plans/core.md section 7.4): versions, detection and files.

Runs in the oracle image against UnityPy 1.25.2 and writes small JSON files under
crates/unityrs-core/tests/fixtures/c/, which `cargo test -p unityrs-core` replays
without Python (crates/unityrs-core/tests/files_fixtures.rs). Every input is built
in code: the version strings and the detection inputs here, and the file matrix by
unityrs-testkit (`bundle::matrix`), whose bytes this script asks cargo for and
stores beside UnityPy's L0 and L1 records (tools/parity/unitypy_dump.py's own
functions, so the records are the oracle dump's). No file is read.

    scripts/dev python tools/fixtures/files.py           # write the fixtures
    scripts/dev python tools/fixtures/files.py --check   # regenerate and compare

Integers outside the i64 range are written as strings "big:<n>", and the error
notes of file records are left out (they are messages, not outcomes).
"""

from __future__ import annotations

import argparse
import contextlib
import gzip
import io
import json
import os
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-core/tests/fixtures/c"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import UnityPy  # noqa: E402
from UnityPy.helpers.UnityVersion import UnityVersion  # noqa: E402

import unitypy_dump as D  # noqa: E402
from scaffold import dump  # noqa: E402

I64_MIN, I64_MAX = -(2**63), 2**63 - 1


def norm(v):
    """Ints outside i64 as "big:<n>", notes dropped, recursively."""
    if isinstance(v, bool) or v is None or isinstance(v, (str, float)):
        return v
    if isinstance(v, int):
        return v if I64_MIN <= v <= I64_MAX else f"big:{v}"
    if isinstance(v, list):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        return {k: norm(x) for k, x in v.items() if k != "note"}
    raise TypeError(type(v))


# ----------------------------------------------------------------- versions


def arabic(s: str) -> str:
    """The ASCII digits of s as Arabic-Indic digits (category Nd)."""
    return "".join(chr(0x660 + ord(c) - 48) if c.isascii() and c.isdigit() else c for c in s)


def version_strings() -> list[str]:
    base = [
        "2022.3.0f1",
        "5.6.7",
        "2021.3.2ff1",
        "2019.4.15 f1",
        "2019.4.15f1",
        "2019.4.14f1",
        "2020.3.33f1",
        "2020.3.34f1",
        "2021.3.1f1",
        "2021.3.2f1",
        "2022.1.0f1",
        "2022.1.1f1",
        "2023.1.0a1",
        "2023.1.0b9",
        "2023.1.0c1",
        "2023.1.0p3",
        "2023.1.0x1",
        "2023.1.0A1",
        "2023.1.0F1",
        "2023.1.0Q1",
        "2020.3.34f1024",
        "2020.3.34f255",
        "2020.3.34f256",
        "2020.300.34f1",
        "2020.3.70000f1",
        "133091.1.1f1",
        "65536.1.1f1",
        "0.5.1f1",
        "0.0.0",
        "",
        "abc",
        "1.2",
        "1.2.",
        "1.2.3\n",
        "1.2.3f\n4",
        "1.2.3é" + "7",
        "1.2.3f12abc",
        "1.2.3f",
        "-1.2.3f1",
        "2019.4.15f1\udcff",
        "2019.4.15\udcfff1",
        "12345678901234567890.1.1f1",
    ]
    big = []
    for field in range(4):
        for n in (4300, 4301):
            groups = ["2019", "4", "15", "1"]
            groups[field] = "0" * (n - len(groups[field])) + groups[field]
            big.append(f"{groups[0]}.{groups[1]}.{groups[2]}f{groups[3]}")
    nd = [arabic(s) for s in ("2019.4.15f1", "2020.3.34f1", "5.6.7")] + [arabic(s) for s in big]
    return base + big + nd


def version_case(s: str) -> dict:
    raw = s.encode("utf-8", "surrogateescape")
    case = {"hex": raw.hex(), "ascii_digits": all(not c.isdigit() or c.isascii() for c in s)}
    try:
        v = UnityVersion.from_str(s)
    except Exception:  # noqa: BLE001 - the outcome is that it raises
        case.update(packed=None)
        return case
    case["packed"] = int(v)
    try:
        case["tuple"] = list(v.as_tuple())
    except Exception:  # noqa: BLE001
        case["tuple"] = None
    try:
        old = (
            v < (2020,)
            or (v[0] == 2020 and v < (2020, 3, 34))
            or (v[0] == 2021 and v < (2021, 3, 2))
            or (v[0] == 2022 and v < (2022, 1, 1))
        )
        case["flags"] = "ArchiveFlagsOld" if old else "ArchiveFlags"
    except Exception:  # noqa: BLE001
        case["flags"] = None
    try:
        case["align_f6"] = bool(v[0] == 2019 and v >= (2019, 4, 15))
    except Exception:  # noqa: BLE001
        case["align_f6"] = None
    case["major"] = v.major
    return case


# ---------------------------------------------------------------- detection


def gz(data: bytes, **kw) -> bytes:
    return gzip.compress(data, mtime=0, **kw)


def gz_member(data: bytes, flags: int = 0, extra: bytes = b"", name: bytes = b"", comment: bytes = b"") -> bytes:
    """One gzip member written by hand, with the optional header fields."""
    head = b"\x1f\x8b\x08" + bytes([flags]) + b"\0\0\0\0\0\xff"
    if flags & 4:
        head += struct.pack("<H", len(extra)) + extra
    if flags & 8:
        head += name + b"\0"
    if flags & 16:
        head += comment + b"\0"
    if flags & 2:
        head += struct.pack("<H", zlib.crc32(head) & 0xFFFF)
    c = zlib.compressobj(9, zlib.DEFLATED, -15)
    body = c.compress(data) + c.flush()
    return head + body + struct.pack("<II", zlib.crc32(data), len(data) & 0xFFFFFFFF)


def assets_header(meta: int, size: int, version: int, offset: int, pad: int = 128) -> bytes:
    return struct.pack(">IIII", meta, size, version, offset) + b"\0" * (pad - 16)


def assets22(meta: int, size: int, offset: int, version: int = 22) -> bytes:
    head = struct.pack(">IIII", 0, 0, version, 0) + b"\0\0\0\0" + struct.pack(">Iqqq", meta, size, offset, 0)
    return head + b"\0" * (128 - len(head))


def detect_inputs() -> list[tuple[str, bytes]]:
    web = b"UnityWebData1.0\0" + b"\0" * 200
    plain = b"x" * 300
    cases = [
        ("empty", b""),
        ("short", b"UnityFS\0"),
        ("19", b"a" * 19),
        ("20-no-nul", b"a" * 20),
        ("21-no-nul", b"a" * 21),
        ("20-nul-last", b"a" * 19 + b"\0"),
        ("unityfs", b"UnityFS\0" + b"\0" * 40),
        ("unityweb", b"UnityWeb\0" + b"\0" * 40),
        ("unityraw", b"UnityRaw\0" + b"\0" * 40),
        ("unityarchive", b"UnityArchive\0" + b"\0" * 40),
        ("u00fa-utf8", "ú".encode() * 8 + b"\0" + b"\0" * 40),
        ("fa-bytes", b"\xfa" * 8 + b"\0" + b"\0" * 40),
        ("unityfs-no-nul-20", b"UnityFSUnityFSUnityF" + b"\0" * 40),
        ("webdata", b"UnityWebData1.0\0" + b"\0" * 40),
        ("webdata-20", b"UnityWebDataXXXXXXXX" + b"\0" * 40),
        ("tuanjie", b"TuanjieWebData1.0\0" + b"\0" * 40),
        ("zip", b"PK\x03\x04\0" + b"\0" * 40),
        ("zip-no-nul", b"PK\x03\x04\x14\0" + b"\0" * 40),
        ("127", b"\0" * 127),
        ("128-zeros", b"\0" * 128),
        ("gzip-web", gz(web) + b"\0" * 64),
        ("gzip-web-exact", gz(b"UnityWebData") + b"\0" * 128),
        ("gzip-web-13", gz(b"UnityWebData1") + b"\0" * 128),
        ("gzip-tuanjie", gz(b"TuanjieWebData1.0\0" + b"\0" * 300)),
        ("gzip-plain", gz(plain) + b"\0" * 100),
        ("gzip-two-members", gz(b"UnityWeb") + gz(b"Data1.0\0" + b"\0" * 200)),
        ("gzip-trailing-garbage", gz(web) + b"\x01" + b"\0" * 100),
        ("gzip-truncated", gz(web + bytes(range(256)) * 4)[:-20]),
        ("gzip-bad-crc", gz(web)[:-8] + b"\0\0\0\0" + gz(web)[-4:] + b"\0" * 100),
        ("gzip-bad-size", gz(web)[:-4] + b"\x01\0\0\0" + b"\0" * 100),
        ("gzip-bad-method", b"\x1f\x8b\x07" + gz(web)[3:] + b"\0" * 100),
        ("gzip-corrupt", b"\x1f\x8b\x08\0\0\0\0\0\0\xff" + b"\xff" * 200),
        ("gzip-header-fields", gz_member(web, 2 | 4 | 8 | 16, b"ex", b"name", b"comment") + b"\0" * 64),
        ("gzip-fhcrc-wrong", gz_member(web, 2)[:10] + b"\0\0" + gz_member(web, 2)[12:] + b"\0" * 64),
        ("gzip-empty-stream", gz(b"") + b"\0" * 200),
        ("gzip-one-byte", gz(b"U") + b"\0" * 200),
        ("brotli", b"\0" * 0x20 + b"brotli" + b"\0" * 100),
        ("brotli-short", b"\0" * 0x20 + b"brotl" + b"\0" * 100),
        ("assets-ok", assets_header(100, 128, 17, 120)),
        ("assets-v100", assets_header(100, 128, 100, 120)),
        ("assets-v101", assets_header(100, 128, 101, 120)),
        ("assets-size-past", assets_header(100, 129, 17, 120)),
        ("assets-meta-gt-size", assets_header(120, 100, 17, 50)),
        ("assets-offset-gt-size", assets_header(10, 100, 17, 101)),
        ("assets-v22", assets22(40, 128, 64)),
        ("assets-v22-negative", assets22(40, -1, 64)),
        ("assets-v22-offset-past", assets22(40, 128, 129)),
        ("assets-v21-fields-ignored", assets22(40, -1, 64, version=21)),
        ("le-header", struct.pack("<IIII", 100, 128, 17, 120) + b"\0" * 112),
    ]
    return cases


def detect_case(name: str, data: bytes) -> dict:
    return {"name": name, "hex": data.hex(), "type": D.detect(data)}


# -------------------------------------------------------------------- files


def matrix_inputs() -> list[tuple[str, bytes]]:
    """unityrs-testkit's bundle::matrix(), as the replay test writes it."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "matrix.jsonl"
        env = dict(os.environ, UNITYRS_WRITE_MATRIX=str(out))
        subprocess.run(
            [
                "cargo",
                "test",
                "-q",
                "-j",
                "3",
                "--locked",
                "-p",
                "unityrs-core",
                "--test",
                "files_fixtures",
                "--",
                "--exact",
                "matrix_inputs_are_the_testkit_matrix",
            ],
            cwd=ROOT,
            env=env,
            check=True,
            stdout=subprocess.DEVNULL,
        )
        rows = [json.loads(line) for line in out.read_text().splitlines()]
    return [(r["name"], bytes.fromhex(r["hex"])) for r in rows]


def file_records(name: str, data: bytes) -> list[dict]:
    """tools/parity/unitypy_dump.py's records for one input, at L0 and L1."""
    import hashlib

    from collections import Counter

    stats = Counter()
    detected = D.detect(data)
    l0 = {"type": detected, "parsed_as": None, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    out: list[dict] = []
    with contextlib.redirect_stdout(io.StringIO()):
        if detected in ("ZIP", "error"):
            l0["parsed_as"] = "zip" if detected == "ZIP" else "error"
            error = {"kind": "detect"} if detected == "error" else None
            out.append({"rec": "file", "file": [name], "l0": l0, "l1": None, "error": error})
        else:
            env = UnityPy.Environment()
            try:
                f = env.load_file(data, name=name)
            except Exception:  # noqa: BLE001 - a failed load is a recorded outcome
                l0["parsed_as"] = "error"
                out.append({"rec": "file", "file": [name], "l0": l0, "l1": None, "error": {"kind": "parse"}})
            else:
                l0["parsed_as"] = D.parsed_as(f)
                D.emit_file(f, [name], l0, ("L0", "L1"), out, stats)
    # The file records: emit_file also writes each object's (L1 of objects is
    # package D's).
    return [norm(r) for r in out if r["rec"] == "file"]


def fixtures() -> dict[str, dict]:
    meta = {"generator": "tools/fixtures/files.py", "unitypy": UnityPy.__version__}
    return {
        "versions.json": {**meta, "cases": [norm(version_case(s)) for s in version_strings()]},
        "detect.json": {**meta, "cases": [detect_case(n, d) for n, d in detect_inputs()]},
        "files.json": {
            **meta,
            "cases": [{"name": n, "hex": d.hex(), "records": file_records(n, d)} for n, d in matrix_inputs()],
        },
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
