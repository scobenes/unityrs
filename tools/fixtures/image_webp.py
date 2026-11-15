"""Pillow 9.5.0 / libwebp 1.3.0 still-image oracle (PARITY 15.8, L5b).

Synthetic extractor modes; odd extents, alpha boundaries, defaults and quality
95. Rust rebuilds the pixels and replays every digest and small whole file.
No metadata, animation or lossless option is involved. --check writes nothing.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
from pathlib import Path

import PIL
from PIL import Image, features

OUT = Path(__file__).resolve().parents[2] / "crates/unityrs-imageio/tests/fixtures/webp.json"
MODES = {"L": 1, "LA": 2, "RGB": 3, "RGBA": 4}


def pixels(mode, width, height, kind):
    state = 0x12345678
    out = bytearray()
    for i in range(width * height):
        for c in range(MODES[mode]):
            state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
            value = state >> 24
            if kind == "flat":
                value = 73 + c * 37
            elif kind == "gradient":
                value = (i % width * 7 + i // width * 13 + c * 53) & 255
            if c == MODES[mode] - 1 and mode in ("LA", "RGBA"):
                if kind == "opaque":
                    value = 255
                elif kind == "transparent":
                    value = 0
                elif kind == "alpha":
                    value = (0, 1, 127, 254, 255)[i % 5]
            out.append(value)
    return bytes(out)


def record():
    assert PIL.__version__ == "9.5.0", PIL.__version__
    assert features.version("webp") == "1.3.0", features.version("webp")
    rows = []
    for mode in MODES:
        for width, height in [(1, 1), (3, 5), (15, 17), (17, 15), (31, 33), (257, 129), (16383, 1), (1, 16383)]:
            for kind in ("flat", "gradient", "random", "alpha", "opaque", "transparent"):
                raw = pixels(mode, width, height, kind)
                im = Image.frombytes(mode, (width, height), raw)
                for quality, exact in [(80, False), (95, False), (80, True)]:
                    options = {} if quality == 80 and not exact else {"quality": quality, "exact": exact}
                    fp = io.BytesIO()
                    im.save(fp, "WEBP", **options)
                    data = fp.getvalue()
                    rows.append(dict(mode=mode, width=width, height=height, kind=kind,
                                     quality=quality, exact=exact, pixels_sha256=hashlib.sha256(raw).hexdigest(),
                                     length=len(data), sha256=hashlib.sha256(data).hexdigest(),
                                     head=data[:32].hex(), hex=data.hex() if len(data) <= 256 else None))
    refusals = []
    for width, height in [(0, 0), (0, 1), (1, 0), (16384, 1), (1, 16384)]:
        try:
            Image.new("RGBA", (width, height)).save(io.BytesIO(), "WEBP")
        except Exception as error:
            refusals.append(dict(width=width, height=height, error=type(error).__name__, message=str(error)))
        else:
            raise AssertionError((width, height))
    options_refusals = []
    for width, height, quality, method in [(1, 1, -1, 4), (1, 1, 101, 4), (1, 1, 80, 7), (0, 0, -1, 4), (0, 0, 80, 7)]:
        try:
            Image.new("RGBA", (width, height)).save(io.BytesIO(), "WEBP", quality=quality, method=method)
        except Exception as error:
            options_refusals.append(dict(width=width, height=height, quality=quality, method=method,
                                         error=type(error).__name__, message=str(error)))
        else:
            raise AssertionError((width, height, quality, method))
    return dict(pillow=PIL.__version__, libwebp=features.version("webp"), cases=rows,
                refusals=refusals, options_refusals=options_refusals)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    data = record()
    # One case per line, with a small human-readable wrapper.
    text = '{"pillow": "9.5.0", "libwebp": "1.3.0", "cases": [\n' + ',\n'.join(
        json.dumps(row, sort_keys=True) for row in data["cases"]
    ) + '\n], "refusals": ' + json.dumps(data["refusals"], sort_keys=True) + ', "options_refusals": ' + json.dumps(data["options_refusals"], sort_keys=True) + '}\n'
    if args.check:
        if OUT.read_text() != text:
            raise SystemExit("WebP oracle drift: regenerate tools/fixtures/image_webp.py")
    else:
        OUT.write_text(text)
    print(f"WebP: {len(data['cases'])} images, {len(data['refusals']) + len(data['options_refusals'])} refusals, {len(text)} bytes")


if __name__ == "__main__":
    main()
