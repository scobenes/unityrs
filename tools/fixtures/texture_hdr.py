"""Record synthetic ASTC HDR blocks against astcenc's full HDR profile.

Run in the pinned amd64 oracle image; --check regenerates without writing.
The scalar build decodes into F16 with DECOMPRESS_ONLY, without
USE_DECODE_UNORM8. Raw binary16 hashes pin interpolation before byte
conversion. PARITY.md section 17.1's closed form then scales each half;
UnityPy's unchanged LDR-profile result is recorded beside it. AVX2 raw
half hashes are also kept: its F16 conversion mishandles signed void
colours, whereas scalar preserves them. Neither build's U8 store supplies
the reference. No input comes from an external file.

All six footprints cover all HDR endpoint modes, 48 independently seeded
blocks per mode, LDR endpoints under HDR, partitions, mixed endpoints,
dual planes, void extents and IEEE specials. Structural errors are named
InvalidBlock, while astcenc's NaN output and UnityPy's error colour remain
recorded. Image cases additionally cover crop, flip, block order, trailing
data, short/empty data, zero and negative sides, and an invalid later block.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import sys
from functools import lru_cache
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import texture  # noqa: E402
import texture_astc as astc  # noqa: E402

OUT = texture.OUT / "astc_hdr.json"
SCALAR = astc.build("_encoder_none")
AVX2 = astc.build("_encoder_avx2")


def digest(data):
    return hashlib.sha256(data).hexdigest()


@lru_cache(maxsize=None)
def context(side, avx2=False):
    backend = AVX2 if avx2 else SCALAR
    config = backend.ASTCConfig(astc.ASTCProfile.HDR, side, side, 1, 0,
                                astc.ASTCConfigFlags.DECOMPRESS_ONLY)
    return backend.ASTCContext(config)


def half_decode(data, width, height, side, avx2=False):
    backend = AVX2 if avx2 else SCALAR
    image = backend.ASTCImage(astc.ASTCType.F16, width, height)
    context(side, avx2).decompress(data, image, backend.ASTCSwizzle())
    return bytes(image.data)


def scaled(raw, width, height, flip=True):
    def byte(value):
        if math.isnan(value) or value <= 0:
            return 0
        if value >= 1:
            return 255
        return math.floor(value * 255 + 0.5)

    pixels = bytes(byte(v[0]) for v in struct.iter_unpack("<e", raw))
    if not flip:
        return pixels
    stride = width * 4
    return b"".join(pixels[y * stride:(y + 1) * stride] for y in reversed(range(height)))


def valid(tags):
    return not any(t.startswith("error:") and t != "error:void_hdr" for t in tags)


def sample(side, label, cem, parts=1, plane=None):
    rng = astc.Rng(texture.seed_of(f"astc-hdr-{side}-{label}"))
    modes = [m for m in astc.legal(side, side) if bool(astc.mode_fields(m)[3]) == (plane is not None)]
    for _ in range(20000):
        block = rng.take(16)
        mode = modes[rng.below(len(modes))]
        astc.put(block, 0, 11, mode)
        astc.put(block, 11, 2, parts - 1)
        if parts == 1:
            astc.put(block, 13, 4, cem)
        else:
            astc.put(block, 23, 6, cem << 2)
        if plane is not None:
            astc.put(block, 128 - astc.weight_bits(mode) - 2, 2, plane)
        tags = astc.classify(bytes(block), side, side)
        if "valid" in tags and f"cem:{cem}" in tags:
            return bytes(block)
    raise AssertionError((side, label, cem, parts, plane))


def blocks(side):
    for i, block in enumerate(astc.crafted(side, side)):
        yield f"crafted-{i}", block
    for cem in astc.HDR_CEMS:
        for i in range(48):
            label = f"cem-{cem}-{i}"
            yield label, sample(side, label, cem)
        for parts in (2, 3, 4):
            if (((cem >> 2) + 1) * 2) * parts <= 18:
                label = f"cem-{cem}-partitions-{parts}"
                yield label, sample(side, label, cem, parts=parts)
        for plane in range(4):
            label = f"cem-{cem}-plane-{plane}"
            yield label, sample(side, label, cem, plane=plane)
    # Four different channels expose placement as well as the half special.
    specials = [0, 0x8000, 1, 0x3ff, 0x400, 0x3800, 0x3c00, 0x7bff,
                0x7c00, 0xfc00, 0x7e00, 0x7c01, 0xfe55, 0xbc00]
    for i, value in enumerate(specials):
        yield f"void-half-{value:04x}", (bytes.fromhex("fcffffffffffffff") +
              struct.pack("<4H", value, specials[(i + 1) % len(specials)], 0x3400, 0x3c00))


def reference(data, width, height, side):
    if width == 0 or height == 0:
        return {"mode": "RGBA", "size": [0, 0], "sha256": digest(b"")}
    if not data:
        return {"raises": "NoImageData"}
    if width < 0 or height < 0:
        return {"raises": "NegativeSize"}
    pw, ph = width + (-width) % side, height + (-height) % side
    needed = pw // side * (ph // side) * 16
    if len(data) < needed:
        return {"raises": "NotEnoughData", "needed": needed}
    for at in range(0, needed, 16):
        if not valid(astc.classify(data[at:at + 16], side, side)):
            return {"raises": "InvalidBlock"}
    raw = half_decode(data[:needed], pw, ph, side)
    row = pw * 8
    cropped = b"".join(raw[y * row:y * row + width * 8] for y in range(height))
    pixels = scaled(cropped, width, height)
    return {"mode": "RGBA", "size": [width, height], "sha256": digest(pixels)}


def fixture():
    block_cases, image_cases, coverage = [], [], {}
    with astc.decoding_with(AVX2):
        for side in astc.SIDES:
            fmt = astc.TF[f"ASTC_HDR_{side}x{side}"]
            good = []
            bad = None
            tags_seen = set()
            for label, data in blocks(side):
                tags = astc.classify(data, side, side)
                tags_seen.update(tags)
                raw = half_decode(data, side, side, side)
                c = {"id": f"{fmt.name}-{label}", "side": side, "format": int(fmt),
                     "data": data.hex(), "tags": tags,
                     "unitypy_sha256": texture.outcome(data, side, side, fmt, texture.WINDOWS, None)["sha256"]}
                if valid(tags):
                    other = half_decode(data, side, side, side, avx2=True)
                    if "error:void_hdr" not in tags:
                        assert raw == other, (c["id"], "scalar/AVX2 raw halves differ")
                    c.update(half_sha256=digest(raw), avx2_half_sha256=digest(other),
                             sha256=digest(scaled(raw, side, side)))
                    good.append(data)
                else:
                    assert all(math.isnan(v[0]) for v in struct.iter_unpack("<e", raw)), c["id"]
                    c["raises"] = "InvalidBlock"
                    bad = data
                block_cases.append(c)
            coverage[str(side)] = sorted(tags_seen)
            # Distinct blocks make cropping, block order and row flip observable.
            data = b"".join(good[-18:-14])
            for label, width, height, payload in (
                ("one", 1, 1, data), ("crop", side + 1, side - 1, data),
                ("four", 2 * side, 2 * side, data),
                ("long", side + 1, side + 1, data + b"trailing mip"),
                ("invalid-later", 2 * side, side, good[0] + bad),
                ("short", side, side, data[:15]), ("empty", side, side, b""),
                ("zero-width", 0, side, b""), ("zero-height", side, 0, b"x"),
                ("negative", -side, side, data), ("negative-empty", side, -1, b""),
            ):
                c = texture.case(f"{fmt.name}-{label}", fmt, width, height, {"hex": payload.hex()})
                c["reference"] = reference(payload, width, height, side)
                image_cases.append(c)
    return {"generator": "tools/fixtures/texture_hdr.py", "unitypy": texture.UnityPy.__version__,
            "pillow": texture.PIL.__version__, "astc_encoder": astc.astc_encoder.__version__,
            "astcenc_commit": "631fb04e1c4bca002a8b653f34e044dbb35b32b4",
            "half_backend": "scalar", "profile": "HDR", "flags": "DECOMPRESS_ONLY",
            "coverage": coverage, "blocks": block_cases, "cases": image_cases}


def dump(body):
    """Five records per line keeps this complete matrix below 800 lines."""
    def compact(value):
        return json.dumps(value, sort_keys=True, separators=(",", ":"))

    lines = ["{"]
    items = sorted(body.items())
    for i, (key, value) in enumerate(items):
        comma = "," if i + 1 < len(items) else ""
        if isinstance(value, list):
            lines.append(f" {json.dumps(key)}: [")
            for at in range(0, len(value), 5):
                records = value[at:at + 5]
                tail = "," if at + 5 < len(value) else ""
                lines.append("  " + ",".join(compact(record) for record in records) + tail)
            lines.append(" ]" + comma)
        else:
            lines.append(f" {json.dumps(key)}: {compact(value)}{comma}")
    return "\n".join([*lines, "}"]) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    versions = (texture.UnityPy.__version__, texture.PIL.__version__, astc.astc_encoder.__version__)
    if versions != ("1.25.2", "9.5.0", "0.1.12") or AVX2 is None:
        raise SystemExit(f"needs pinned amd64 oracle with AVX2, found {versions}")
    text = dump(fixture())
    if len(text.encode()) >= 1_500_000:
        raise SystemExit("ASTC HDR fixture exceeds 1.5 MB")
    if args.check:
        if not OUT.exists() or OUT.read_text() != text:
            raise SystemExit("stale fixture: astc_hdr.json")
    else:
        OUT.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
