"""Oracle fixtures for package E: how unitypy_dump.py reads a corpus index (docs/plans/core.md 7.6).

Runs in the oracle image and writes crates/unityrs-dump/tests/fixtures/dump/index.json, which
crates/unityrs-dump/tests/index_json.rs replays without Python. The oracle reads an index with
`json.loads(fh.read().decode("utf-8"))` and then compares `e.get("kind")` with `--kind`,
`str(e.get("build"))` with `--build`, encodes `e["path"]` with `os.fsencode`, and prints `str()` of an
entry's `sha256` pin (`collect_inputs`, `main`). Recorded here:

- "str": Python's `str(json.loads(text))` for a table of JSON texts of every kind: integers of any
  sign and size, floats at and past every boundary (and seeded random ones, in more digits than round
  trip; seeded doubles whose shortest digits break a tie, and powers of two whose nearest digits of
  that length do not round-trip), NaN and the infinities, strings with every escape and lone surrogates, lists and dicts of all
  of these (their `repr`s; seeded random code points, so `str.isprintable` and the quote rule are
  read), a repeated key;
- "select": `unitypy_dump.collect_inputs` itself over an index this script writes, with a `--kind` and
  `--build` (as argv bytes): the names it selects, in order, and each one's pin as the run's message
  prints it, or that it raised;
- "runs": the oracle's whole run (`unitypy_dump.py --index`, a subprocess, so its argv is bytes and its
  call stack is the real one) on index texts json.loads takes or refuses: its exit status and the
  names it dumped;
- "depth": the deepest nesting of arrays the oracle's json.loads decodes in `collect_inputs`, found by
  running it (Python's recursion limit less the frames below the call), and its runs at that depth and
  one past it, nested arrays and nested objects.

Every input is built here in code; no file outside the temporary folders is read.

    scripts/dev python tools/fixtures/index.py           # write the fixture
    scripts/dev python tools/fixtures/index.py --check   # regenerate and compare
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import struct
import subprocess
import sys
import tempfile
from decimal import ROUND_FLOOR, ROUND_HALF_EVEN, Context, Decimal, localcontext
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-dump/tests/fixtures/dump/index.json"
ORACLE = ROOT / "tools/parity/unitypy_dump.py"
sys.path.insert(0, str(ROOT / "tools" / "parity"))

import unitypy_dump  # noqa: E402
import UnityPy  # noqa: E402

SEED = 7


def wtf8(s: str) -> str:
    """A `str` as the hex of its UTF-8, each lone surrogate's three bytes as `surrogatepass` writes them."""
    return s.encode("utf-8", "surrogatepass").hex()


# ------------------------------------------------------------------------ str()


def esc(cp: int) -> str:
    """A JSON escape for one code point (a pair for one past the BMP)."""
    if cp < 0x10000:
        return f"\\u{cp:04x}"
    cp -= 0x10000
    return f"\\u{0xD800 + (cp >> 10):04x}\\u{0xDC00 + (cp & 0x3FF):04x}"


def random_code_point(rng: random.Random) -> int:
    pools = [
        (0x00, 0x7F), (0x80, 0xFF), (0x100, 0x2FFF), (0x2000, 0x206F), (0xD800, 0xDFFF), (0xE000, 0xFFFF),
        (0x10000, 0x1FFFF), (0x20000, 0x10FFFF),
    ]
    lo, hi = rng.choice(pools)
    return rng.randint(lo, hi)


def random_string(rng: random.Random) -> str:
    """A JSON string literal of random code points, each escaped or (when JSON lets it be) written as is."""
    out = []
    for _ in range(rng.randint(0, 6)):
        cp = rng.choice([random_code_point(rng), ord("'"), ord('"'), ord("\\")])
        raw = cp >= 0x20 and not 0xD800 <= cp <= 0xDFFF and chr(cp) not in '"\\'
        out.append(chr(cp) if raw and rng.random() < 0.5 else esc(cp))
    return '"' + "".join(out) + '"'


def random_float(rng: random.Random) -> str:
    """A finite double's decimal text, in more (or fewer) digits than its shortest form."""
    while True:
        x = struct.unpack("<d", rng.getrandbits(64).to_bytes(8, "little"))[0]
        if x == x and abs(x) != float("inf"):
            break
    if rng.random() < 0.5:
        x = rng.uniform(-1e7, 1e7)
    return f"{x:.{rng.randint(0, 25)}e}"


def shortest_digits(x: float) -> tuple[Decimal, int]:
    """`abs(x)`'s exact value, and the exponent of the last digit `repr(x)` gives."""
    digits = repr(abs(x)).split("e")[0].replace(".", "").strip("0") or "0"
    exact = Decimal(abs(x))
    return exact, exact.adjusted() - len(digits) + 1


def shortest_tie(x: float) -> bool:
    """Whether `repr(x)`'s digits are one of two of that length that round-trip and lie equally near."""
    exact, q = shortest_digits(x)
    with localcontext(Context(prec=1200)):
        lo = exact.scaleb(-q).to_integral_value(rounding=ROUND_FLOOR).scaleb(q)
        hi = lo + Decimal(1).scaleb(q)
        return exact - lo == hi - exact and float(lo) == float(hi) == abs(x)


def nearest_misses(x: float) -> bool:
    """Whether the decimal of `repr(x)`'s length nearest `x` fails to round-trip."""
    exact, q = shortest_digits(x)
    with localcontext(Context(prec=1200)):
        return float(exact.scaleb(-q).to_integral_value(rounding=ROUND_HALF_EVEN).scaleb(q)) != abs(x)


def tie_floats(rng: random.Random) -> list[str]:
    """Exact decimal texts of doubles whose shortest digits break a tie, which random doubles almost never
    do (review round 8: Rust's shortest digits break it upwards, Python's `repr` to the even digit), and
    of powers of two whose nearest decimal of that length does not round-trip. A tie's exact decimal has
    at most 18 digits, so its double is an odd number over at most 2^25, or an odd multiple of 5^(t+1)
    times 2^t: a decimal ending in 5 and t zeros."""
    ties: list[float] = []
    while len(ties) < 160:
        if rng.random() < 0.6:
            x = (rng.getrandbits(rng.randint(1, 53)) | 1) / 2 ** rng.randint(1, 25)
        else:
            t = rng.randint(0, 21)
            x = float((rng.randrange(1, max(2, 2**53 // 5 ** (t + 1))) | 1) * 5 ** (t + 1) * 2**t)
        x = -x if rng.random() < 0.25 else x
        if shortest_tie(x) and x not in ties:
            ties.append(x)
    misses = [x for x in (math.ldexp(1.0, e) for e in range(-1074, 1024)) if nearest_misses(x)]
    # A float's text, `.0` after an integer's digits so json.loads reads a float.
    texts = [str(Decimal(x)) for x in ties + misses[:4] + rng.sample(misses[4:], 12)]
    return [t if "." in t or "E" in t else t + ".0" for t in texts]


STR_TEXTS = [
    "null", "true", "false",
    "0", "-0", "1", "-1", "4100", "100056", "9223372036854775807", "9223372036854775808",
    "-9223372036854775809", "18446744073709551616", "1" + "0" * 30, "9" * 4300, "-" + "9" * 4300,
    "0.0", "-0.0", "0e5", "-0e-5", "1.0", "1E5", "1e+5", "1e-5", "1.5e-4", "0.0001", "0.00001", "1e15",
    "1e16", "1234567890123456.0", "12345678901234567.0", "1e22", "1e23", "968494.1733294675", "0.1",
    "0.30000000000000004", "2.5", "2.50", "4100.0", "1e400", "-1e400", "1e-400", "-1e-400", "4.9e-324",
    "2.4703282292062327e-324", "2.4703282292062328e-324", "2.2250738585072011e-308",
    "2.2250738585072014e-308", "1.7976931348623157e308", "1.7976931348623158e308",
    "1.7976931348623159e308", "9007199254740993.0", "9007199254740993", "123456789012345678901234567890.5",
    "0." + "0" * 400 + "1", "1" + "0" * 400 + ".0", "1" + "0" * 1000 + "e-1000", "1e99999999999999999999",
    "1e-99999999999999999999",
    "NaN", "Infinity", "-Infinity",
    '""', '"4100"', '"a,b"', '"\u00e9"', '"\\u00e9"', '"\\udcff"', '"\\ud800"', '"\\ud83d\\ude00"',
    '"\\ud800\\u0041"', '"\\ud800\\ud800\\udc00"', '"\\uDBFF\\uDFFF"', '"\\u0000\\t\\n\\"\\\\\\/\\b\\f\\r"',
    '"\u2028\u007f\u0085"',
    "[]", "{}", "[1]", "[[]]", "[{}]", "[true, false, null]", '["a", "it\'s", "a\\"b", "x\'\\""]',
    '[1, {"k": true}, 2.50, null, "s"]', '[-0, -0.0, NaN, Infinity, -Infinity, 1e400, 1e16]',
    '["\\u0378\\u00a0\\u2028\\u007f\\udcff\\ud83d\\ude00\\\\\\t\\n\\r\\u0000\\u001f\\u0085\\u00ad"]',
    '{"x": 1}', '{"a": 1, "b": 2, "a": 3}', '{"k": [1, {"k": true}]}', '{"\\u0000": "\'", "\'\\"": ""}',
    '[[[["deep"]]]]', '{"a": {"b": {"c": [null]}}}',
    # Review round 8: shortest digits that break a tie (Python `...320.2`, `...806.62`).
    "1551563132437320.25", "79686618578806.625", "1125899906842624.25", "22620844907483.3125",
    "-1551563132437320.25", "[79686618578806.625]", "2.98023223876953125E-8", "5.9604644775390625E-7",
]


def str_cases() -> list[dict]:
    rng = random.Random(SEED)
    texts = list(STR_TEXTS)
    texts += [random_float(rng) for _ in range(300)]
    texts += ["[" + ", ".join(random_string(rng) for _ in range(rng.randint(1, 3))) + "]" for _ in range(200)]
    texts += ["{" + random_string(rng) + ": " + random_string(rng) + "}" for _ in range(50)]
    texts += tie_floats(random.Random(SEED + 1))
    out = []
    for text in texts:
        try:
            value = json.loads(text)
        except json.JSONDecodeError as e:
            raise AssertionError(f"{text!r}: {e}") from e
        out.append({"json": text, "str": wtf8(str(value))})
    return out


# ----------------------------------------------------------------- collect_inputs

# (name, index text, --kind bytes or None, --build bytes or None): collect_inputs reads only the index.
FFF = "\\udcff"
BUILDS = (
    '{"files":['
    '{"path":"b0","kind":"bundle","build":-0},'
    '{"path":"b1","kind":"bundle","build":968494.1733294675},'
    '{"path":"b2","kind":"bundle","build":{"x": 1}},'
    '{"path":"b3","kind":"bundle","build":[true]},'
    '{"path":"b4","kind":"bundle","build":"' + FFF + '"},'
    '{"path":"b5","kind":"bundle","build":NaN},'
    '{"path":"b6","kind":"bundle","build":1e400},'
    '{"path":"b7","kind":"bundle"},'
    '{"path":"b8","kind":"bundle","build":"4100"},'
    '{"path":"b9","kind":"bundle","build":4100},'
    '{"path":"ba","kind":"bundle","build":4100.0},'
    '{"path":"bb","kind":"bundle","build":{"a":1,"a":2}},'
    '{"path":"bc","kind":"bundle","build":-0.0},'
    '{"path":"bd","kind":"bundle","build":["\\u00e9\\u0378", "it\'s"]},'
    '{"path":"be","kind":"bundle","build":["\\u00e9\\u0378\\u00a0it\'s\\ud83d\\ude00\\udbff\\udfff"]},'
    '{"path":"bf","kind":"bundle","build":1551563132437320.25}'
    "]}"
)
BUILD_ARGS = [
    b"0", b"968494.1733294675", b"968494.1733294677", b"-0.0", b"{'x': 1}", b'{"x":1}', b"[True]", b"[true]",
    b"\xff", b"nan", b"inf", b"None", b"4100", b"4100.0", b"{'a': 2}", b"x,0,inf", b"[True],,None",
    "['\u00e9\\u0378', \"it's\"]".encode(),
    "[\"\u00e9\\u0378\\xa0it's\U0001f600\\U0010ffff\"]".encode(),
    b"1551563132437320.2", b"1551563132437320.3",  # review round 8: a tie, `repr` takes the even digit
]
KINDS = (
    '{"files":['
    '{"path":"k0","kind":"bundle"},'
    '{"path":"k1","kind":"' + FFF + '"},'
    '{"path":"k2","kind":1},'
    '{"path":"k3","kind":["bundle"]},'
    '{"path":"k4","kind":"Bundle"},'
    '{"path":"k5","kind":"unitypy-sample"},'
    '{"path":"k6","kind":"\\u00e9"},'
    '{"path":"k7","kind":""}'
    "]}"
)
PATHS = (
    '{"files":['
    '{"path":"' + FFF + '","kind":"bundle","sha256":"' + FFF + '"},'
    '{"path":"a\\u00e9","kind":"bundle","sha256":[1, {"k": true}, 2.50, null, "s", "' + FFF + '"]},'
    '{"path":"\\ud83d\\ude00","kind":"bundle","sha256":{"x": NaN}},'
    '{"path":"x/../q","kind":"bundle","sha256":1e400},'
    '{"path":"\\ud800","kind":"other"},'
    '{"path":"p","kind":"bundle","sha256":-0.0},'
    '{"path":"p","kind":"bundle","sha256":NaN}'
    "]}"
)


def select_case_list() -> list[tuple[str, str, bytes | None, bytes | None]]:
    cases = [(f"build_{i}", BUILDS, None, b) for i, b in enumerate(BUILD_ARGS)]
    cases += [
        ("kind_default", KINDS, None, None),
        ("kind_bundle", KINDS, b"bundle", None),
        ("kind_not_utf8", KINDS, b"\xff", None),
        ("kind_int", KINDS, b"1", None),
        ("kind_several", KINDS, "\u00e9,,Bundle".encode(), None),
        ("kind_latin1_byte", KINDS, b"\xe9", None),
        ("paths_and_pins", PATHS, None, None),
        ("path_lone_high_surrogate", PATHS, b"bundle,other", None),
        ("path_number", '{"files":[{"path":5,"kind":"bundle"}]}', None, None),
        ("path_missing_unselected", '{"files":[{"kind":"other"},{"path":"p","kind":"bundle"}]}', None, None),
        ("files_repeated", '{"files":[{"path":"p","kind":"bundle"}],"files":[]}', None, None),
        ("kind_repeated", '{"files":[{"path":"p","kind":"other","kind":"bundle","path":"q"}]}', None, None),
    ]
    return cases


def select_cases(root: Path) -> list[dict]:
    out = []
    for n, (name, text, kind, build) in enumerate(select_case_list()):
        folder = root / str(n)
        folder.mkdir()
        (folder / "index.json").write_text(text)
        kinds = list(unitypy_dump.INDEX_KINDS) if kind is None else os.fsdecode(kind).split(",")
        builds = os.fsdecode(build).split(",") if build else None
        args = SimpleNamespace(index=True, kind=kinds, build=builds, name=None, sample=None, limit=None,
                               input=str(folder / "index.json"))
        try:
            items = unitypy_dump.collect_inputs(args)
        except Exception:  # noqa: BLE001 - the oracle's run fails on any of them
            rows = None
        else:
            # The pin as `main`'s message prints it (`stderr` writes lone surrogates backslashed);
            # one Python reads as false pins nothing.
            rows = [[unitypy_dump.name_bytes(rel).hex(),
                     str(sha).encode("utf-8", "backslashreplace").hex() if sha else None]
                    for _, rel, sha in items]
        out.append({"name": name, "index": text,
                    "kind": None if kind is None else kind.hex(), "build": None if build is None else build.hex(),
                    "inputs": rows})
    return out


# ---------------------------------------------------------------------- runs

ENTRY = '{"path":"f","kind":"bundle"}'


def with_x(x: str) -> str:
    return '{"files":[' + ENTRY + '],"x":' + x + "}"


RUN_TEXTS = [
    ("nan", with_x("NaN").encode()),
    ("infinities", with_x("[Infinity, -Infinity]").encode()),
    ("overflow", with_x("1e400").encode()),
    ("int_4300_digits", with_x("9" * 4300).encode()),
    ("int_4301_digits", with_x("9" * 4301).encode()),
    ("negative_int_4300_digits", with_x("-" + "9" * 4300).encode()),
    ("negative_int_4301_digits", with_x("-" + "9" * 4301).encode()),
    ("float_of_5000_digits", with_x("9" * 5000 + ".5").encode()),
    ("lone_surrogate", with_x('"\\udcff\\ud800"').encode()),
    ("raw_control", with_x('"\t"').encode()),
    ("raw_del_and_line_separator", with_x('"\x7f\u2028"').encode()),
    ("bad_escape", with_x('"\\x"').encode()),
    ("bad_unicode_escape", with_x('"\\u00g0"').encode()),
    ("bad_low_half", with_x('"\\ud800\\uzzzz"').encode()),
    ("bom", b"\xef\xbb\xbf" + with_x("0").encode()),
    ("not_utf8", with_x('"\xff"').encode("latin-1")),
    ("encoded_surrogate", with_x('"').encode() + b"\xed\xb3\xbf" + b'"}'),
    ("trailing_comma", b'{"files":[' + ENTRY.encode() + b",]}"),
    ("extra_data", with_x("0").encode() + b" 0"),
    ("whitespace", b" \r\n\t" + with_x("0").encode() + b" \r\n\t"),
    ("leading_zero", with_x("01").encode()),
    ("plus_sign", with_x("+1").encode()),
    ("bare_point", with_x("1.").encode()),
    ("negative_nan", with_x("-NaN").encode()),
    ("single_quotes", with_x("'a'").encode()),
    ("files_repeated", b'{"files":[' + ENTRY.encode() + b'],"files":[]}'),
    ("path_lone_surrogate_byte", b'{"files":[{"path":"\\udcff","kind":"bundle"}]}'),
    ("path_lone_high_surrogate", b'{"files":[{"path":"\\ud800","kind":"bundle"}]}'),
    ("path_lone_high_surrogate_unselected", b'{"files":[' + ENTRY.encode() + b',{"path":"\\ud800","kind":"x"}]}'),
    ("kind_lone_surrogate", b'{"files":[{"path":"f","kind":"\\udcff"}]}'),
    ("build_object", b'{"files":[{"path":"f","kind":"bundle","build":{"x": 1}}]}'),
    ("build_negative_zero", b'{"files":[{"path":"f","kind":"bundle","build":-0}]}'),
    ("build_tie", b'{"files":[{"path":"f","kind":"bundle","build":79686618578806.625}]}'),
]
RUN_ARGV = {
    "kind_lone_surrogate": [b"--kind", b"bundle,\xff"],
    "build_object": [b"--build", b"{'x': 1}"],
    "build_negative_zero": [b"--build", b"0,\xfe"],
    "build_tie": [b"--build", b"79686618578806.62"],
}
RUN_FILES = ["66", "ff"]  # "f", and the byte 0xFF


def run_oracle(folder: Path, text: bytes, extra: list[bytes]) -> tuple[int, list[str] | None, str]:
    """The oracle's exit status on the index `text`, the names it dumped (hex) when it wrote a dump, its stderr."""
    index = folder / "index.json"
    index.write_bytes(text)
    out = folder / "out.jsonl"
    argv = [sys.executable.encode(), str(ORACLE).encode(), b"--index", str(index).encode(), b"-q",
            b"-o", str(out).encode(), *extra]
    proc = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, check=False)
    names = None
    if out.exists():
        names = []
        for line in out.read_text().splitlines():
            rec = json.loads(line)
            if rec.get("rec") == "file":
                name = rec["file"][0]
                names.append(bytes.fromhex(name["hex"]).hex() if isinstance(name, dict) else name.encode().hex())
        out.unlink()
    return proc.returncode, names, proc.stderr.decode("utf-8", "replace")


def run_cases(root: Path) -> list[dict]:
    folder = root / "runs"
    folder.mkdir()
    for f in RUN_FILES:
        with open(os.path.join(os.fsencode(folder), bytes.fromhex(f)), "wb") as fh:
            fh.write(b"f")
    out = []
    for name, text in RUN_TEXTS:
        extra = RUN_ARGV.get(name, [])
        status, names, _ = run_oracle(folder, text, extra)
        out.append({"name": name, "index_hex": text.hex(), "argv": [a.hex() for a in extra], "status": status,
                    "names": names})
    return out


# --------------------------------------------------------------------- depth


def nested(levels: int, kind: str) -> bytes:
    """An index whose member `x` nests `levels - 1` arrays (or objects) in its root object: `levels` deep."""
    n = levels - 1
    inner = "[" * n + "]" * n if kind == "array" else '{"a":' * n + "0" + "}" * n
    return with_x(inner).encode()


def depth_cases(root: Path) -> dict:
    folder = root / "depth"
    folder.mkdir()
    (folder / "f").write_bytes(b"f")
    # The oracle exits 2 (no inputs matched) once json.loads has decoded the index, 1 when it raised.
    lo, hi = 2, 4096
    statuses = {}
    for at in (lo, hi):
        statuses[at] = run_oracle(folder, nested(at, "array"), [b"--kind", b"none"])[0]
    if statuses != {lo: 2, hi: 1}:
        raise AssertionError(f"the oracle's depth bounds moved: {statuses}")
    while hi - lo > 1:
        mid = (lo + hi) // 2
        status, _, err = run_oracle(folder, nested(mid, "array"), [b"--kind", b"none"])
        if status == 2:
            lo = mid
        elif status == 1 and "RecursionError" in err:
            hi = mid
        else:
            raise AssertionError(f"depth {mid}: exit {status}: {err}")
    runs = []
    for kind in ("array", "object"):
        for levels in (lo, lo + 1):
            status, names, _ = run_oracle(folder, nested(levels, kind), [])
            runs.append({"kind": kind, "levels": levels, "status": status, "names": names})
    return {"max": lo, "runs": runs}


# ---------------------------------------------------------------------- main


def fixture() -> dict:
    with tempfile.TemporaryDirectory(prefix="unityrs-fixtures-") as tmp:
        select = select_cases(Path(tmp))
    with tempfile.TemporaryDirectory(prefix="unityrs-fixtures-") as tmp:
        runs = run_cases(Path(tmp))
        depth = depth_cases(Path(tmp))
    return {
        "unitypy": UnityPy.__version__,
        "python": sys.version.split()[0],
        "str": str_cases(),
        "select": select,
        "runs": runs,
        "depth": depth,
    }


def dump(body: dict) -> str:
    """JSON with one list element per line, so a regenerated fixture diffs by case."""

    def compact(v: object) -> str:
        return json.dumps(v, sort_keys=True, separators=(",", ":"))

    items = sorted(body.items())
    lines = ["{"]
    for n, (key, value) in enumerate(items):
        comma = "," if n < len(items) - 1 else ""
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
    if sys.get_int_max_str_digits() != sys.int_info.default_max_str_digits:
        print("needs Python's default int digit limit, which the oracle runs with", file=sys.stderr)
        return 2
    text = dump(fixture())
    if args.check:
        if not OUT.exists() or OUT.read_text() != text:
            print(f"stale fixture: {OUT.name}", file=sys.stderr)
            return 1
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
