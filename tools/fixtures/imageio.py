"""Oracle fixtures for unityrs-imageio: Pillow 9.5.0's PNG bytes and the visible-pixel hash (docs/plans/extract.md
E19, E20; gate rows X3 and X4).

Runs in the oracle image and writes crates/unityrs-imageio/tests/fixtures/pillow_png.json, one case a line, which
crates/unityrs-imageio/tests/pillow_png.rs replays without Python. Every image is built here in code, in each mode
the PNG writer takes from the extractor (`L`, `LA`, `RGB`, `RGBA`), at sizes from 1x1 through odd, wide and tall ones
to ones whose PNG needs several IDAT chunks, among them widths past 16384, where Pillow's encoder buffer, and so each
IDAT chunk, grows to `4 x width` bytes. Its pixels come from one of the kinds below, drawn from splitmix64 seeded with
the case's seed, a generator the Rust test runs too:

- "flat": one pixel's bytes, drawn once, everywhere;
- "gradient": byte `(x * dx + y * dy + c * dc + base) & 255` of channel `c`, the four parameters drawn first;
- "random": every byte drawn;
- "noisy": the gradient, plus a drawn byte's low two bits, mod 256;
- "smooth": byte `(((x * x * a + y * y * b) >> 5) + c * dc + base) & 255`, the four parameters drawn first, a surface
  Paeth predicts well;
- "sparse": for each byte, two draws: the second where the first is below 32, else 0;
- "stripes": one drawn row, then row `y` as that row plus `y // 2`, mod 256, so rows repeat in pairs;
- "alpha" (`LA`, `RGBA`): each pixel's colour drawn, then its alpha: a draw below 96 is 0, else the draw.

The draws are the bytes of successive splitmix64 outputs, low byte first. Each case records its parameters and seed,
the sha256 of its pixels (`tobytes()`), of its filtered rows (the IDAT stream inflated) and of the PNG
`Image.save(fp, "PNG")` writes, the PNG's length, its IDAT chunks' data sizes, how many rows took each filter, the
whole PNG as hex when it is at most 256 bytes, and the visible-pixel hash computed here from Pillow's
`convert("RGBA")`.

The PNG bytes depend on the zlib Pillow loads; the reference is zlib 1.2.13, the oracle image's
(`tools/parity/oracle_env.py --image`, PARITY.md section 1). Writing refuses any other. `--check` under another zlib
(a runner's own) compares everything but the PNG's sha256, length, hex and IDAT sizes, and says so on stderr; CI runs
it over a zlib 1.2.13 it builds from the release tarball, so every field is compared (the parity-py job's X3 and X4
step).

    scripts/dev python -E -s tools/fixtures/imageio.py           # write the fixture
    scripts/dev python -E -s tools/fixtures/imageio.py --check   # regenerate and compare
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import struct
import sys
import zlib
from pathlib import Path

import PIL
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-imageio/tests/fixtures/pillow_png.json"

PILLOW = "9.5.0"
REFERENCE_ZLIB = "1.2.13"
CHANNELS = {"L": 1, "LA": 2, "RGB": 3, "RGBA": 4}
# The PNG fields that depend on the zlib Pillow loads.
ZLIB_FIELDS = ("png_sha256", "png_len", "png_hex", "idat")
HEX_MAX = 256
MASK = (1 << 64) - 1

SMALL = [(1, 1), (2, 1), (1, 2), (2, 2), (3, 3), (5, 1), (1, 5), (7, 5), (13, 17), (31, 2), (64, 64), (100, 37)]
KINDS = ["flat", "gradient", "random", "noisy", "smooth", "sparse", "stripes"]
# Wide, tall and several-chunk sizes, each with one kind: 16384 is the widest whose encoder buffer is 65536 bytes.
LARGE = [
    (257, 129, "random"),
    (4096, 3, "gradient"),
    (3, 4096, "noisy"),
    (1, 70000, "random"),
    (16384, 2, "random"),
    (16385, 2, "random"),
    (20000, 5, "random"),
]


class Draws:
    """splitmix64's outputs as bytes, low byte first."""

    def __init__(self, seed: int):
        self.state = seed & MASK
        self.buf = b""
        self.pos = 0

    def _next64(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
        return z ^ (z >> 31)

    def take(self, n: int) -> bytes:
        while len(self.buf) - self.pos < n:
            more = (n - (len(self.buf) - self.pos) + 7) // 8
            self.buf = self.buf[self.pos :] + b"".join(struct.pack("<Q", self._next64()) for _ in range(more))
            self.pos = 0
        out = self.buf[self.pos : self.pos + n]
        self.pos += n
        return out

    def byte(self) -> int:
        return self.take(1)[0]


def pixels(kind: str, mode: str, width: int, height: int, seed: int) -> bytes:
    ch = CHANNELS[mode]
    d = Draws(seed)
    n = width * height * ch
    if kind == "flat":
        return d.take(ch) * (width * height)
    if kind == "random":
        return d.take(n)
    if kind in ("gradient", "noisy"):
        dx, dy, dc, base = d.take(4)
        out = bytearray(n)
        i = 0
        for y in range(height):
            for x in range(width):
                for c in range(ch):
                    out[i] = (x * dx + y * dy + c * dc + base) & 255
                    i += 1
        if kind == "noisy":
            noise = d.take(n)
            out = bytearray((v + (e & 3)) & 255 for v, e in zip(out, noise, strict=True))
        return bytes(out)
    if kind == "smooth":
        a, b, dc, base = d.take(4)
        return bytes(
            (((x * x * a + y * y * b) >> 5) + c * dc + base) & 255
            for y in range(height)
            for x in range(width)
            for c in range(ch)
        )
    if kind == "sparse":
        pairs = d.take(2 * n)
        return bytes(pairs[2 * i + 1] if pairs[2 * i] < 32 else 0 for i in range(n))
    if kind == "stripes":
        row = d.take(width * ch)
        return b"".join(bytes((v + y // 2) & 255 for v in row) for y in range(height))
    if kind == "alpha":
        out = bytearray()
        for _ in range(width * height):
            out += d.take(ch - 1)
            a = d.byte()
            out.append(0 if a < 96 else a)
        return bytes(out)
    raise ValueError(kind)


def seed_of(case_id: str) -> int:
    return int.from_bytes(hashlib.sha256(case_id.encode()).digest()[:4], "little")


def case_list() -> list[tuple[str, int, int, str]]:
    out = []
    for mode in CHANNELS:
        kinds = KINDS + (["alpha"] if mode in ("LA", "RGBA") else [])
        out += [(mode, w, h, kind) for (w, h) in SMALL for kind in kinds]
        out += [(mode, w, h, kind) for (w, h, kind) in LARGE]
    return out


def chunks(png: bytes) -> list[tuple[bytes, bytes]]:
    pos, out = 8, []
    while pos < len(png):
        (n,) = struct.unpack(">I", png[pos : pos + 4])
        out.append((png[pos + 4 : pos + 8], png[pos + 8 : pos + 8 + n]))
        pos += 12 + n
    return out


def visible_hash(image: Image.Image) -> str:
    """E20: sha256 of "{w}x{h}:" and convert("RGBA")'s bytes, each pixel of alpha 0 as four zeros."""
    rgba = bytearray(image.convert("RGBA").tobytes())
    for i in range(3, len(rgba), 4):
        if rgba[i] == 0:
            rgba[i - 3 : i + 1] = b"\0\0\0\0"
    width, height = image.size
    return hashlib.sha256(f"{width}x{height}:".encode() + bytes(rgba)).hexdigest()


def record(mode: str, width: int, height: int, kind: str) -> dict:
    case_id = f"{mode}-{width}x{height}-{kind}"
    seed = seed_of(case_id)
    data = pixels(kind, mode, width, height, seed)
    image = Image.frombytes(mode, (width, height), data)
    saved = io.BytesIO()
    image.save(saved, "PNG")
    png = saved.getvalue()
    parts = chunks(png)
    idat = [body for kind_, body in parts if kind_ == b"IDAT"]
    if [k for k, _ in parts] != [b"IHDR"] + [b"IDAT"] * len(idat) + [b"IEND"]:
        raise AssertionError(f"{case_id}: chunks {[k for k, _ in parts]}")
    filtered = zlib.decompress(b"".join(idat))
    stride = width * CHANNELS[mode] + 1
    filters = [0] * 5
    for y in range(height):
        filters[filtered[y * stride]] += 1
    case = {
        "id": case_id,
        "mode": mode,
        "width": width,
        "height": height,
        "kind": kind,
        "seed": seed,
        "pixels_sha256": hashlib.sha256(data).hexdigest(),
        "filtered_sha256": hashlib.sha256(filtered).hexdigest(),
        "filters": filters,
        "png_sha256": hashlib.sha256(png).hexdigest(),
        "png_len": len(png),
        "idat": [len(b) for b in idat],
        "visible": visible_hash(image),
    }
    if len(png) <= HEX_MAX:
        case["png_hex"] = png.hex()
    return case


def fixture() -> dict:
    return {
        "generator": "tools/fixtures/imageio.py",
        "pillow": PIL.__version__,
        "zlib": Image.core.zlib_version,
        "cases": [record(*c) for c in case_list()],
    }


def dump(body: dict) -> str:
    """JSON with one case per line, so a regenerated fixture diffs by case (scaffold.py's layout)."""

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


def without_zlib(body: dict) -> dict:
    """The fixture less what depends on the zlib Pillow loads."""
    cases = [{k: v for k, v in c.items() if k not in ZLIB_FIELDS} for c in body["cases"]]
    return {**{k: v for k, v in body.items() if k not in ("cases", "zlib")}, "cases": cases}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if PIL.__version__ != PILLOW:
        print(f"needs Pillow {PILLOW}, found {PIL.__version__}", file=sys.stderr)
        return 2
    loaded = Image.core.zlib_version
    if not args.check and loaded != REFERENCE_ZLIB:
        print(f"Pillow loads zlib {loaded}; the fixture is written with zlib {REFERENCE_ZLIB}", file=sys.stderr)
        return 2
    body = fixture()
    if not args.check:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(dump(body))
        return 0
    if not OUT.exists():
        print(f"stale fixture: {OUT.name} is missing", file=sys.stderr)
        return 1
    stored = json.loads(OUT.read_text())
    if loaded == REFERENCE_ZLIB:
        same = OUT.read_text() == dump(body)
    else:
        print(
            f"imageio.py: Pillow loads zlib {loaded}, not {REFERENCE_ZLIB}: the PNGs' sha256, length, hex and IDAT "
            "sizes are not compared (the oracle image compares them)",
            file=sys.stderr,
        )
        same = stored.get("zlib") == REFERENCE_ZLIB and without_zlib(stored) == without_zlib(body)
    if not same:
        print(f"stale fixture: {OUT.name}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
