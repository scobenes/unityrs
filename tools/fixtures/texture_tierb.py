"""Record Tier B/C raw textures beside their independent closed-form references.

UnityPy 1.25.2's actual image or exception is kept unchanged beside PARITY.md section 17.1's
reference (RGBAHalf: section 19.1; RGB48: section 19.2). No Pillow path supplies a reference.
Cases cover channel placement, row flip, lengths, zero/negative sides, IEEE specials and
rounding neighbours. Every binary16 pattern is also decoded in each half format; integer
ramps exercise every 8/16-bit channel value. ARGBFloat and odd YUY2 are recorded as Tier U.

Run in the pinned oracle container; --check regenerates and compares without writing.
"""
from __future__ import annotations

import argparse
import hashlib
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "parity"))

import references  # noqa: E402
import texture  # noqa: E402
from UnityPy.enums import TextureFormat as TF  # noqa: E402

OUT = texture.OUT / "tierb.json"
NAMES = sorted((set(references.TIER_B) - {"BC6H"}) | set(references.TIER_C))
HALVES = [0, 0x8000, 1, 0x3FF, 0x400, 0x37FF, 0x3800, 0x3801, 0x3C00, 0x7BFF,
          0x7C00, 0xFC00, 0x7E00, 0x7C01, 0xBC00]
FLOATS = [0, 0x80000000, 1, 0x7FFFFF, 0x800000, 0x3B008080, 0x3EFFFFFF,
          0x3F000000, 0x3F000001, 0x3F800000, 0x7F7FFFFF, 0x7F800000,
          0xFF800000, 0x7FC00000, 0x7F800001, 0xBF800000]


def cases():
    """Synthetic inputs only, shared with native replay and the texture fuzz seeds."""
    for name in NAMES:
        fmt = TF[name]
        for w, h in ((1, 1), (2, 2), (3, 5), (8, 8), (17, 1), (2, 9)):
            if name == "YUY2":
                w += w % 2
            size = references.needed_bytes(name, w, h)
            yield f"{name}-{w}x{h}-random", fmt, w, h, texture.seeded(f"tierb-{name}-{w}x{h}", size)
        w, h = (4, 3) if name == "YUY2" else (3, 2)
        size = references.needed_bytes(name, w, h)
        for label, length in (("empty", 0), ("one", 1), ("short", size - 1), ("exact", size),
                              ("long", size + 1), ("mips", size * 2 + 16)):
            yield f"{name}-{label}", fmt, w, h, texture.seeded(f"tierb-{name}-{label}", length)
        for w, h, length in ((0, 0, 0), (0, 4, 1), (4, 0, 1), (0, -3, 0),
                              (-2, 4, 64), (4, -1, 0), (4, -1, 64)):
            yield f"{name}-{w}x{h}-side-{length}", fmt, w, h, {"zeros": length}
        # Matrix input: the exact UnityPy failure class/message for all Tier B rows.
        size = references.needed_bytes(name, 8, 8)
        yield f"{name}-matrix", fmt, 8, 8, {"hex": (b"\x30" * size).hex()}
        if "Half" in name or "Float" in name and name != "RGB9e5Float":
            bits, code = (HALVES, "H") if "Half" in name else (FLOATS, "I")
            per_pixel = references.needed_bytes(name, 1, 1) // struct.calcsize(code)
            for bit in bits:
                data = struct.pack("<" + code * per_pixel, *([bit] * per_pixel))
                yield f"{name}-ieee-{bit:08x}", fmt, 1, 1, {"hex": data.hex()}
            if "Half" in name:
                yield f"{name}-every-half", fmt, 256, 256, {"ramp": 2, "len": 65536 * per_pixel * 2}
        elif name not in ("YUY2", "RGB9e5Float"):
            bits = 8 if name in ("RG16", "R8_SIGNED", "RG16_SIGNED", "RGB24_SIGNED", "RGBA32_SIGNED") else 16
            side = 16 if bits == 8 else 256
            yield f"{name}-every-integer", fmt, side, side, {"ramp": bits // 8,
                "len": references.needed_bytes(name, side, side)}
    # Both Tier C paths return wrong images on these finite inputs.
    yield "RGBAHalf-channels", TF.RGBAHalf, 2, 2, {"hex": struct.pack("<16e", *([.5, .25, .125, .75] * 4)).hex()}
    yield "RGB48-channels", TF.RGB48, 2, 2, {"hex": struct.pack("<12H", *([0xFFFF, 0, 0] * 4)).hex()}
    yield "YUY2-colours", TF.YUY2, 2, 2, {"hex": bytes([16, 128, 235, 128, 81, 90, 81, 240]).hex()}
    for exponent in range(32):
        words = [exponent << 27 | r | g << 9 | b << 18 for r, g, b in ((0, 1, 511), (256, 128, 1))]
        yield f"RGB9e5Float-exponent-{exponent}", TF.RGB9e5Float, 2, 1, {"hex": struct.pack("<2I", *words).hex()}
    for w, h, length in ((1, 1, 2), (3, 2, 12), (1, 0, 0), (-1, 2, 4)):
        yield f"YUY2-unsupported-{w}x{h}", TF.YUY2, w, h, {"zeros": length}
    for w, h, length in ((1, 1, 16), (0, 0, 0), (1, 1, 0)):
        yield f"ARGBFloat-unsupported-{w}x{h}-{length}", TF.ARGBFloat, w, h, {"zeros": length}


def reference(name, data, w, h):
    if w == 0 or h == 0:
        mode, size, pixels = "RGBA", [0, 0], b""
    elif name == "ARGBFloat" or name == "YUY2" and w % 2:
        return {"raises": "Unsupported"}
    elif not data:
        return {"raises": "NoImageData"}
    elif w < 0 or h < 0:
        return {"raises": "NegativeSize"}
    elif len(data) < references.needed_bytes(name, w, h):
        return {"raises": "NotEnoughData", "needed": references.needed_bytes(name, w, h)}
    else:
        mode, values = references.decode(name, data, w, h)
        size, pixels = [w, h], bytes(v for pixel in values for v in pixel)
    result = {"mode": mode, "size": size, "sha256": hashlib.sha256(pixels).hexdigest()}
    if w * h <= texture.HEX_PIXELS:
        result["pixels"] = pixels.hex()
    return result


def fixture():
    result = []
    for label, fmt, w, h, spec in cases():
        c = texture.case(label, fmt, w, h, spec)
        c["reference"] = reference(fmt.name, texture.build(spec), w, h)
        result.append(c)
    return {"generator": "tools/fixtures/texture_tierb.py", "unitypy": texture.UnityPy.__version__,
            "pillow": texture.PIL.__version__, "cases": result}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    if (texture.UnityPy.__version__, texture.PIL.__version__) != ("1.25.2", "9.5.0"):
        raise SystemExit("needs UnityPy 1.25.2 and Pillow 9.5.0")
    text = texture.dump(fixture())
    if args.check:
        if not OUT.exists() or OUT.read_text() != text:
            raise SystemExit("stale fixture: tierb.json")
    else:
        OUT.write_text(text)


if __name__ == "__main__":
    main()
