"""Synthetic platform and Texture2DArray oracle; UnityPy 1.25.2 (MIT).

Records the actual helpers, converter, and patched Texture2DArray property.
Switch data is built with UnityPy's swizzler after NEAREST padding; Xbox
CRNs come from the original synthetic builder in texture_crunch.py.
Array integer references follow PARITY.md 19.2 and 17.1, including signed
bytes biased by 128. No external data or patched decoder is used.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import PIL
import UnityPy
from PIL import Image
from UnityPy.classes import Texture2DArray
from UnityPy.enums import TextureFormat as TF
from UnityPy.enums.GraphicsFormat import GRAPHICS_TO_TEXTURE_MAP, GraphicsFormat
from UnityPy.export import Texture2DConverter as C
from UnityPy.helpers import TextureSwizzler as S

import texture_crunch

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-texture/tests/fixtures/platform.json"
VERSION = (2017, 2, 0, 0)


def ramp(n):
    return bytes((i * 37 + i // 17 + 11) % 256 for i in range(n))


def result(call):
    try:
        image = call()
        if isinstance(image, list):
            return {"images": [pixels(item) for item in image]}
        return pixels(image)
    except Exception as error:  # the oracle exception is fixture data
        return {"raises": type(error).__name__, "message": str(error)}


def pixels(image):
    data = image.tobytes()
    return {"mode": image.mode, "size": list(image.size), "sha256": hashlib.sha256(data).hexdigest()}


def converted(data, width, height, fmt, platform, blob=None):
    """Exercise the same object getter as Texture2D.image, including its fields."""
    obj = SimpleNamespace(m_Width=width, m_Height=height, m_TextureFormat=fmt, m_PlatformBlob=blob,
                          object_reader=SimpleNamespace(version=VERSION, platform=platform), get_image_data=lambda: data)
    return result(lambda: C.get_image_from_texture2d(obj))


def texture_cases():
    rows = []
    # Every texel block, several GOB heights, edges and multiple GOB columns.
    for fmt, (bw, bh) in S.TEXTURE_FORMAT_BLOCK_SIZE_MAP.items():
        for exponent, w, h in ((1, 1, 1), (2, bw * 4 + 1, bh * 9 + 1), (3, bw * 9 - 1, bh * 65 - 1)):
            gobs = 1 << exponent
            pw, ph = S.get_padded_texture_size(w, h, bw, bh, gobs)
            linear = ramp(pw // bw * (ph // bh) * 16)
            if fmt.name.startswith("ASTC"):
                # Valid UNORM16 void extents avoid astcenc's documented
                # AVX2/scalar difference for invalid blocks. Platform layout
                # is the subject here; texture_astc.py covers error colours.
                linear = b"".join(bytes.fromhex("fcfdffffffffffff") + linear[i + 8:i + 16]
                                  for i in range(0, len(linear), 16))
            data = bytes(S.swizzle(linear, pw, ph, bw, bh, gobs))
            blob = bytes(8) + exponent.to_bytes(4, "little")
            assert S.deswizzle(data, pw, ph, bw, bh, gobs) == linear
            rows.append(dict(id=f"switch-{fmt.name}-{w}x{h}-g{gobs}", format=int(fmt), width=w, height=h,
                             platform=38, blob=blob.hex(), data=data.hex(), padded=[pw, ph], block=[bw, bh],
                             linear_sha256=hashlib.sha256(linear).hexdigest(),
                             **converted(data, w, h, fmt, 38, list(blob))))
    # RGB24/BGR24 are promoted to four channels before the table lookup.
    for fmt in (TF.RGB24, TF.BGR24):
        w, h = 19, 13
        pw, ph = S.get_padded_texture_size(w, h, 4, 1, 2)
        img = Image.frombytes("RGBA", (w, h), ramp(w * h * 4))
        linear = C.pad_image(img, pw, ph).tobytes()
        data = S.swizzle(linear, pw, ph, 4, 1, 2)
        blob = bytes(8) + bytes([1, 0, 0, 0])
        rows.append(dict(id=f"switch-promote-{fmt.name}", format=int(fmt), width=w, height=h,
                         platform=38, blob=blob.hex(), data=data.hex(),
                         **converted(data, w, h, fmt, 38, list(blob))))
    for fmt in TF:
        if fmt in S.TEXTURE_FORMAT_BLOCK_SIZE_MAP or fmt in (TF.RGB24, TF.BGR24):
            continue
        blob = bytes(8) + bytes([1, 0, 0, 0])
        data = bytes(1024)
        rows.append(dict(id=f"switch-unsupported-{fmt.name}", format=int(fmt), width=4, height=4,
                         platform=38, blob=blob.hex(), data=data.hex(),
                         **converted(data, 4, 4, fmt, 38, list(blob))))
    for fmt in (TF.RGB565, TF.DXT1, TF.DXT5, TF.RGBA32):
        for w, h in ((1, 1), (7, 9), (32, 4)):
            n = w * h * (2 if fmt == TF.RGB565 else 4)
            if fmt in (TF.DXT1, TF.DXT5):
                n = -(-w // 4) * -(-h // 4) * (8 if fmt == TF.DXT1 else 16)
            for extra in (0, 1, 17):
                data = C.swap_bytes_for_xbox(ramp(n + extra))
                rows.append(dict(id=f"xbox-{fmt.name}-{w}x{h}-tail{extra}", format=int(fmt), width=w, height=h,
                                 platform=11, blob=None, data=data.hex(),
                                 **converted(data, w, h, fmt, 11)))
    for alpha in (False, True):
        for w, h in ((1, 1), (7, 9), (17, 33)):
            fmt = TF.DXT5Crunched if alpha else TF.DXT1Crunched
            crn, _ = texture_crunch.build(w, h, alpha=alpha)
            data = C.swap_bytes_for_xbox(crn)
            rows.append(dict(id=f"xbox-{fmt.name}-{w}x{h}", format=int(fmt), width=w, height=h,
                             platform=11, blob=None, data=data.hex(),
                             **converted(data, w, h, fmt, 11)))
    for platform, blob in ((38, None), (38, "00" * 11), (38, "00" * 12), (13, "00" * 8 + "01" * 4)):
        data = ramp(24)
        raw_blob = None if blob is None else list(bytes.fromhex(blob))
        rows.append(dict(id=f"gate-{platform}-{blob}", format=4, width=2, height=3, platform=platform,
                         blob=blob, data=data.hex(),
                         **converted(data, 2, 3, TF.RGBA32, platform, raw_blob)))
    return rows


def array_case(name, fmt, width=3, height=2, depth=3, data=None, data_size=None, platform=13, streamed=False):
    channels = ((fmt - 13) % 4 + 1) if 13 <= fmt <= 20 else {3: 3, 4: 4, 96: 0, 102: 0}.get(fmt, 4)
    n = width * height * channels
    if fmt == 96:
        n = -(-width // 4) * -(-height // 4) * 8
    if fmt == 102:
        n = -(-width // 4) * -(-height // 4) * 8
    data = ramp(n * depth) if data is None else data
    size = len(data) if data_size is None else data_size
    obj = SimpleNamespace(m_Format=fmt, m_Width=width, m_Height=height, m_Depth=depth, m_DataSize=size,
                          image_data=b"" if streamed else data,
                          object_reader=SimpleNamespace(version=VERSION, platform=platform))
    read = lambda: Texture2DArray.images.fget(obj)
    row = dict(id=name, graphics_format=fmt, width=width, height=height, depth=depth, data_size=size,
               platform=platform, data=data.hex(), streamed=streamed, oracle=result(read))
    if streamed:
        obj.image_data = data
        row["reference"] = result(read)
    if 13 <= fmt <= 20 and width and height:
        images = []
        for offset in range(0, size, size // depth):
            stored = data[offset:offset + size // depth]
            signed = fmt >= 17
            converted = bytes(b ^ 128 if signed else b for b in stored)
            # Section 17.1: absent RGB channels are zero, alpha absent is 255.
            mode = "RGBA" if channels == 4 else "RGB"
            values = b"".join(converted[i:i + channels] + bytes(max(0, 3 - channels))
                              for i in range(0, len(converted), channels))
            images.append(pixels(Image.frombytes(mode, (width, height), values).transpose(Image.Transpose.FLIP_TOP_BOTTOM)))
        row["reference"] = {"images": images}
    return row


def fixture():
    assert (UnityPy.__version__, PIL.__version__) == ("1.25.2", "9.5.0")
    arrays = [array_case(f"array-{fmt}", fmt) for fmt in (3, 4, 96, 102, *range(13, 21))]
    arrays += [array_case("array-streamed", 4, streamed=True),
               array_case("array-switch", 4, platform=38), array_case("array-xbox", 4, platform=11),
               array_case("array-unknown", 999), array_case("array-unmapped", 0),
               array_case("array-depth-zero", 4, depth=0),
               array_case("array-step-zero", 4, data=b"x", depth=2),
               array_case("array-remainder", 4, width=1, height=1, data=ramp(13), depth=3),
               array_case("array-remainder-trailing", 4, width=1, height=1, data=ramp(16), data_size=13, depth=3),
               array_case("array-mips", 4, width=2, height=2, data=ramp(60), depth=3),
               array_case("array-short", 4, data=b"x" * 7),
               array_case("array-zero-side", 4, width=0, data=b"x" * 12)]
    # Mapped formats reach parse_image_data's zero-side return before any
    # decoder or input-data check, including the corrected integer formats.
    for fmt in (3, 4, 9, 13, 17, 21, 22, 96, 102):
        for w, h in ((0, 2), (3, 0), (0, 0)):
            for depth in (1, 2, 5):
                arrays.append(array_case(f"array-zero-{fmt}-{w}x{h}-d{depth}", fmt,
                                         width=w, height=h, depth=depth, data=b"x" * depth))
    arrays += [array_case("array-zero-empty", 21, width=0, data=b"", data_size=3),
               array_case("array-zero-remainder", 22, width=0, data=b"", data_size=13),
               array_case("array-zero-unknown", 999, width=0, data=b"xxx"),
               array_case("array-zero-unmapped", 0, width=0, data=b"xxx"),
               array_case("array-zero-depth-zero", 4, width=0, depth=0, data=b"xxx"),
               array_case("array-zero-step-zero", 4, width=0, data=b"")]
    pads = []
    for mode, channels in (("L", 1), ("RGB", 3), ("RGBA", 4)):
        for w, h, pw, ph in ((1, 1, 5, 7), (3, 2, 3, 2), (3, 2, 8, 2), (3, 2, 3, 9), (3, 2, 8, 9), (0, 2, 0, 3), (2, 0, 3, 0), (0, 2, 3, 2), (2, 0, 2, 3)):
            data = ramp(w * h * channels)
            img = Image.frombytes(mode, (w, h), data) if w and h else Image.new(mode, (w, h))
            padded = C.pad_image(img, pw, ph)
            pads.append(dict(mode=mode, width=w, height=h, padded=[pw, ph], data=data.hex(),
                             same=padded is img, image=pixels(padded)))
    # Preserve the original records, then capture zero-extent successes and
    # failures through the actual helper, including its corner-pixel access.
    for mode in ("L", "RGB", "RGBA"):
        for w, h, pw, ph in ((0, 0, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1), (0, 0, 1, 1),
                             (0, 2, 0, 2), (2, 0, 2, 0), (0, 2, 3, 3), (2, 0, 3, 3),
                             (0, 2, 0, 1), (2, 0, 1, 0), (0, 0, 3, 0), (0, 0, 0, 3),
                             (0, 0, 3, 3), (0, 2, 0, 4), (2, 0, 4, 0)):
            img = Image.new(mode, (w, h))
            pads.append(dict(mode=mode, width=w, height=h, padded=[pw, ph], data="",
                             same=(w, h) == (pw, ph), image=result(lambda: C.pad_image(img, pw, ph))))
    return dict(generator="tools/fixtures/texture_platform.py", unitypy=UnityPy.__version__, pillow=PIL.__version__,
                textures=texture_cases(), arrays=arrays, pads=pads,
                swaps=[dict(data=ramp(n).hex(), swapped=C.swap_bytes_for_xbox(ramp(n)).hex()) for n in (0, 1, 2, 3, 17)],
                graphics_map=[[int(g), int(t)] for g, t in GRAPHICS_TO_TEXTURE_MAP.items()])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    doc = fixture()
    lines = ['{']
    for key, value in doc.items():
        if isinstance(value, list):
            lines.append(f'  {json.dumps(key)}: [')
            lines.extend('    ' + json.dumps(row, separators=(",", ":")) + (',' if i + 1 < len(value) else '')
                         for i, row in enumerate(value))
            lines.append('  ],')
        else:
            lines.append(f'  {json.dumps(key)}: {json.dumps(value)},')
    lines[-1] = lines[-1].rstrip(',')
    text = '\n'.join(lines + ['}']) + '\n'
    if args.check:
        if OUT.read_text() != text:
            raise SystemExit(f"stale {OUT.relative_to(ROOT)}")
    else:
        OUT.write_text(text)
    print(f"platform: {len(doc['textures'])} textures, {len(doc['arrays'])} arrays, {len(doc['pads'])} pads")


if __name__ == "__main__":
    main()
