"""Synthetic P3 T5 oracle: UnityPy 1.25.2, texture2ddecoder 1.0.5, Pillow 9.5.0.

    scripts/dev python -E -s tools/fixtures/texture_pvrtc_atc.py
    scripts/dev python -E -s tools/fixtures/texture_pvrtc_atc.py --check

Reuses texture.py's get_image_from_texture2d wrapper, SplitMix64, data hashes,
image hashes, hex pixels and one-case-per-line JSON. Each format covers tiny,
odd, non-square, below-minimum and non-power-of-two sizes, exact and long data,
zero sides and empty data. PVRTC grids that the native decoder refuses record
its SystemError; short data and negative sides are never passed to native code
(it checks neither and can read/write outside its buffers).

PVRTC: 8x4 or 4x4 padding, no minimum-size expansion, RGB names' retained alpha,
Morton order and wrap at unequal power-of-two block counts, both endpoint modes,
all modulation codes, punch-through and all three checkerboard interpolation
modes. ATC: no UnityPy padding, both colour modes, every palette index, BC3 alpha
endpoint orders and every alpha index. The C++ subtractive entry clamps at zero;
its byte narrowing is retained (docs/plans/rest.md decision 20).

No source tree is imported or executed. Run only with the installed pinned oracle.
The coordinator generates pvrtc_atc.json and runs the Rust replay test.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import PIL
import texture2ddecoder
import UnityPy
from UnityPy.enums import TextureFormat

import texture

OUT = Path(__file__).resolve().parents[2] / "crates/unityrs-texture/tests/fixtures/pvrtc_atc.json"
FORMATS = [TextureFormat(v) for v in (30, 31, 32, 33, 35, 36)]
SIZES = [
    (1, 1), (1, 7), (2, 2), (3, 5), (4, 4), (5, 3), (7, 9), (6, 7),
    (8, 8), (15, 7), (16, 8), (17, 1), (1, 17), (31, 15), (32, 16),
    (16, 32), (63, 31), (64, 32), (32, 64), (257, 3), (12, 12),
    (24, 8), (8, 12), (48, 16), (512, 4),
]


def needed(fmt: TextureFormat, w: int, h: int) -> int:
    bw = 8 if int(fmt) in (30, 31) else 4
    return ((w + bw - 1) // bw) * ((h + 3) // 4) * (16 if int(fmt) == 36 else 8)


def crafted(fmt: TextureFormat):
    """Named bytes and sizes; each input's fields independently enumerate modes."""
    if int(fmt) < 34:
        bw = 8 if int(fmt) in (30, 31) else 4
        for ca, cb in [(0, 0), (0x7FFE, 0x7FFF), (0x8000, 0xFFFF), (0xFFFF, 0), (0, 0xFFFF)]:
            for mod in [0, 0x55555555, 0xAAAAAAAA, 0xFFFFFFFF, 1, 0x00100001]:
                for mode in [0, 1]:
                    block = mod.to_bytes(4, "little") + ((ca & ~1) | mode).to_bytes(2, "little") + cb.to_bytes(2, "little")
                    yield f"endpoints-{ca:x}-{cb:x}-mod-{mod:x}-mode-{mode}", bw, 4, block
        # Mixed endpoint/modulation blocks at wrap boundaries, including a
        # single block on one axis and different long-side Morton tails.
        for nx, ny in [(1, 4), (4, 1), (2, 4), (4, 2), (4, 4)]:
            data = texture.splitmix(0x5056525443 + nx * 16 + ny, nx * ny * 8)
            yield f"wrap-{nx}-{ny}", nx * bw - 1, ny * 4 - 1, data
    else:
        for c0, c1 in [(0, 0xFFFF), (0x7FFF, 0), (0x8000, 0xFFFF), (0xFFFF, 0), (0x9234, 0x5678)]:
            colour = c0.to_bytes(2, "little") + c1.to_bytes(2, "little") + bytes.fromhex("e4e4e4e4")
            if int(fmt) == 35:
                yield f"palette-{c0:x}-{c1:x}", 4, 4, colour
            else:
                for a0, a1 in [(0, 0), (0, 255), (255, 0), (17, 119)]:
                    indices = sum((i % 8) << (3 * i) for i in range(16))
                    alpha = bytes([a0, a1]) + indices.to_bytes(6, "little")
                    yield f"palette-{c0:x}-{c1:x}-alpha-{a0}-{a1}", 4, 4, alpha + colour


def fixtures() -> dict:
    rows = []
    for fmt in FORMATS:
        for w, h in SIZES:
            n = needed(fmt, w, h)
            for trial in range(3):
                name = f"{fmt.name}-{w}x{h}-random-{trial}"
                rows.append(texture.case(name, fmt, w, h, texture.seeded(name, n)))
        # Ignore extra mip/storage bytes, including the usual 2x2-block
        # minimum storage for PVRTC, which this decoder does not enforce.
        for w, h in [(1, 1), (8, 8), (16, 8), (31, 15)]:
            name = f"{fmt.name}-{w}x{h}-long"
            rows.append(texture.case(name, fmt, w, h, texture.seeded(name, needed(fmt, w, h) + 64)))
        for w, h in [(0, 0), (0, -1), (1, 0), (1, 1), (-1, 1)]:
            rows.append(texture.case(f"{fmt.name}-{w}x{h}-empty", fmt, w, h, {"hex": ""}))
        for tag, w, h, block in crafted(fmt):
            rows.append(texture.case(f"{fmt.name}-{tag}", fmt, w, h, {"hex": block.hex()}))
    return {
        "generator": "tools/fixtures/texture_pvrtc_atc.py", "unitypy": UnityPy.__version__,
        "pillow": PIL.__version__, "texture2ddecoder": texture2ddecoder.__version__, "cases": rows,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare without writing")
    args = ap.parse_args()
    found = (UnityPy.__version__, PIL.__version__, texture2ddecoder.__version__)
    if found != ("1.25.2", "9.5.0", "1.0.5"):
        print(f"needs pinned oracle versions, found {found}", file=sys.stderr)
        return 2
    result = texture.dump(fixtures())
    if args.check:
        if not OUT.exists() or OUT.read_text() != result:
            print("stale fixture: pvrtc_atc.json", file=sys.stderr)
            return 1
    else:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
