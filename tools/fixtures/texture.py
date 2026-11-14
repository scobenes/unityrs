"""Oracle fixtures for unityrs-texture: the five decoders of docs/plans/extract.md E15, the selection of E66, and the
ETC and EAC decoders and the rest of Pillow's raw and bcn formats of P3.

Runs in the oracle image against UnityPy 1.25.2, Pillow 9.5.0 and texture2ddecoder 1.0.5 and writes four JSON files
under
crates/unityrs-texture/tests/fixtures/, one case a line, which `cargo test -p unityrs-texture` replays without
Python (crates/unityrs-texture/tests/oracle.rs). Every input is built here in code; no file is read.

    scripts/dev python -E -s tools/fixtures/texture.py           # write the fixtures
    scripts/dev python -E -s tools/fixtures/texture.py --check   # regenerate and compare

**decode.json.** Each case is a texture of one of the five formats (BC7, DXT1, DXT5, RGBA32, RGB24), decoded by
UnityPy's own converter on raw bytes: `Texture2DConverter.get_image_from_texture2d`
(export/Texture2DConverter.py:237-251), the function `Texture2D.image` calls, given a minimal Texture2D-like object
that holds the case's data, size, format, platform and platform blob (`fake_texture`). It runs `parse_image_data`
(export/Texture2DConverter.py:254-320), which pads a block format to whole 4x4 blocks, calls Pillow's `bcn` or
`raw` decoder through `Image.frombytes` with UnityPy's arguments (`CONV_TABLE`, export/Texture2DConverter.py:491-516),
crops back to size and flips top to bottom. A case records its parameters, its data and the data's sha256
(`data_sha256`, which the replay checks before it decodes), then either the image (its mode, size, the sha256 of
`tobytes()`, and those bytes as hex for an image of at most 64 pixels) or the exception (its type and message).

A case's data is `{"seed": S, "len": N}`, the first N bytes of SplitMix64 seeded with S (each output little-endian,
as `unityrs_testkit::rng::Rng` makes them), with `"bc7_mode": M` when every 16-byte block's first byte is then set
so the block is BC7 mode M (its lowest set bit is bit M; 8 is the all-zero byte, Pillow's degenerate block),
`{"hex": ...}` for a crafted block, or `{"zeros": N}`, N zero bytes. The cases cover each format at every size class
(1x1, 2x2, 3x5, 4x4, 7x9, non-power-of-two, wide and tall), data that is exact, short, empty and long (a mip chain
after mip 0), zero and negative sides (a negative one with empty data, one byte and more), every BC7 mode, the BC1
and BC3 endpoint orders, the platforms whose textures UnityPy neither deswizzles nor byte-swaps (an Xbox 360 RGBA32
or RGB24 texture, a Switch texture with one GOB per block), and two limits of Pillow's: its raw decoder's width cap
(`MemoryError` before the data is read, one byte given, at the cap and one past it) and its crop's
decompression-bomb check. The bomb's cases, DXT1 from zeros: the fewest pixels past it, 59 x 3,033,169, with mip 0's
91 MB (Pillow decodes the padded image, about 728 MB, before the crop raises) and with one byte (the data check
speaks first); the most pixels it lets through, 12,470 x 14,351 (178,956,970, a `DecompressionBombWarning` only);
and an aligned texture past it, 4 x 44,739,244, which no crop checks. Recorded in the oracle container, the three
past the bomb's own take about 12 s together and at most about 2.5 GB.

**etc.json.** The cases of tools/fixtures/texture_etc.py (its docstring lists them), each the eight ETC and EAC
formats' texture decoded by `get_image_from_texture2d` as decode.json's are, so through texture2ddecoder 1.0.5 and
Pillow's raw `BGRA` mode (export/Texture2DConverter.py:405-432), and recorded as decode.json's are. The image of a
format with a Tier C patch (PARITY.md section 19.1) also records `reference_sha256`, the sha256 of the Khronos Data
Format Specification 1.3's answer, and `diverging`, how many pixels UnityPy's differs in: for ETC2_RGBA1 its bytes
with every pixel of alpha 0 set to (0, 0, 0, 0), as section 22.9 makes a transparent punch-through pixel; for
EAC_R_SIGNED and EAC_RG_SIGNED UnityPy's decode of the data with every signed base of -128 (each block's first byte
0x80) made -127 (0x81), as sections 22.7 and 22.8 read it. The two limit cases take about 1.5 GB and a few seconds in the oracle container.

**raw.json.** The cases of tools/fixtures/texture_raw.py (its docstring lists them), each a texture of Alpha8,
ARGB4444, ARGB32, RGB565, BGR24, DXT3, RGBA4444, BGRA32, BC4, BC5 or R8 decoded by `get_image_from_texture2d` as
decode.json's are, so through Pillow's raw decoder with each raw mode's unpacker and UnityPy's band order, or its
`bcn` decoder with 2, 4 or 5 (export/Texture2DConverter.py:488-566), and recorded as decode.json's are. The width cap
cases take up to about 1 GB each and the bomb's about 0.3 GB, a few seconds together in the oracle container.

**select.json.** The selection of docs/BATCH_SPEC.md section 3 (E66), computed with UnityPy's own predicates:
`TextureFormat(value).name` (or the decimal value where the enum has none), `TextureSwizzler.is_switch_swizzled`
(helpers/TextureSwizzler.py:121-127) and `Texture2DConverter.XBOX_SWAP_FORMATS` (export/Texture2DConverter.py:569),
in the order a skip takes: a side of 0 (`parse_image_data` returns its empty image before it reads the format,
export/Texture2DConverter.py:265-266), then the format, then the platform. The predicates are measured; the order
between the skips is this script's rule (`selected`), BATCH_SPEC's, which the replay checks the crate keeps.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import PIL
import texture2ddecoder
import UnityPy
from UnityPy.enums import BuildTarget, TextureFormat
from UnityPy.export import Texture2DConverter
from UnityPy.helpers import TextureSwizzler

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-texture/tests/fixtures"
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import texture_etc  # noqa: E402
import texture_raw  # noqa: E402

M64 = (1 << 64) - 1
VERSION = (2021, 3, 1, 1)
WINDOWS = int(BuildTarget.StandaloneWindows64)
XBOX360 = int(BuildTarget.XBOX360)
SWITCH = int(BuildTarget.Switch)
TF = TextureFormat
FIVE = (TF.RGB24, TF.RGBA32, TF.DXT1, TF.DXT5, TF.BC7)
# Bytes per 4x4 block for the block formats, per pixel for the raw ones.
BLOCK = {TF.DXT1: 8, TF.DXT5: 16, TF.BC7: 16}
PIXEL = {TF.RGB24: 3, TF.RGBA32: 4}
HEX_PIXELS = 64


def splitmix(seed: int, n: int) -> bytes:
    """The first `n` bytes SplitMix64 gives from `seed`, each output little-endian (unityrs_testkit::rng::Rng)."""
    out = bytearray()
    state = seed & M64
    while len(out) < n:
        state = (state + 0x9E3779B97F4A7C15) & M64
        z = state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
        z ^= z >> 31
        out += z.to_bytes(8, "little")
    return bytes(out[:n])


def bc7_mode(data: bytes, mode: int) -> bytes:
    """Each 16-byte block's first byte set so the block is BC7 mode `mode` (8: the all-zero byte)."""
    out = bytearray(data)
    for at in range(0, len(out) - len(out) % 16, 16):
        out[at] = 0 if mode == 8 else (out[at] & ~((2 << mode) - 1) & 0xFF) | (1 << mode)
    return bytes(out)


def build(spec: dict) -> bytes:
    if "hex" in spec:
        return bytes.fromhex(spec["hex"])
    if "zeros" in spec:
        return bytes(spec["zeros"])
    if "ramp" in spec:
        return texture_raw.ramp(spec["ramp"], spec["len"])
    data = splitmix(spec["seed"], spec["len"])
    return bc7_mode(data, spec["bc7_mode"]) if "bc7_mode" in spec else data


def seed_of(name: str) -> int:
    return int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "little") & ((1 << 63) - 1)


def needed(fmt: TextureFormat, width: int, height: int) -> int:
    """The bytes mip 0 of a `width` x `height` texture holds (both sides positive)."""
    if fmt in BLOCK:
        return ((width + 3) // 4) * ((height + 3) // 4) * BLOCK[fmt]
    return width * height * PIXEL[fmt]


def mip_chain(fmt: TextureFormat, width: int, height: int) -> int:
    """The bytes of every mip after mip 0, each side halved down to 1, as Unity stores a texture with mips."""
    total = 0
    while width > 1 or height > 1:
        width, height = max(1, width // 2), max(1, height // 2)
        total += needed(fmt, width, height)
    return total


def fake_texture(data: bytes, width: int, height: int, fmt: int, platform: int, blob) -> SimpleNamespace:
    """What `get_image_from_texture2d` reads of a Texture2D: its data (`get_image_data`), size and format, its
    reader's version and platform, and `m_PlatformBlob`."""
    return SimpleNamespace(
        get_image_data=lambda: data,
        m_Width=width,
        m_Height=height,
        m_TextureFormat=fmt,
        object_reader=SimpleNamespace(version=VERSION, platform=platform),
        m_PlatformBlob=blob,
    )


def outcome(data: bytes, width: int, height: int, fmt: int, platform: int, blob) -> dict:
    texture = fake_texture(data, width, height, fmt, platform, blob)
    try:
        image = Texture2DConverter.get_image_from_texture2d(texture)
    except Exception as e:  # noqa: BLE001 - every exception is an outcome to record
        return {"raises": type(e).__name__, "message": str(e)}
    pixels = image.tobytes()
    out = {"mode": image.mode, "size": list(image.size), "sha256": hashlib.sha256(pixels).hexdigest()}
    if image.size[0] * image.size[1] <= HEX_PIXELS:
        out["pixels"] = pixels.hex()
    return out


def case(name: str, fmt: TextureFormat, width: int, height: int, spec: dict, platform=WINDOWS, blob=None) -> dict:
    data = build(spec)
    return {
        "id": name,
        "format": int(fmt),
        "width": width,
        "height": height,
        "platform": platform,
        "blob": blob,
        "data": spec,
        "data_sha256": hashlib.sha256(data).hexdigest(),
        **outcome(data, width, height, int(fmt), platform, blob),
    }


def seeded(name: str, n: int, **extra) -> dict:
    return {"seed": seed_of(name), "len": n, **extra}


SIZES = [
    (1, 1), (2, 2), (3, 5), (4, 4), (7, 9), (5, 3), (8, 8), (6, 13), (12, 12), (17, 1), (1, 17),
    (33, 2), (2, 33), (31, 33), (64, 4), (4, 64), (100, 60), (128, 128), (257, 3),
]  # fmt: skip


def size_cases() -> list[dict]:
    """Every format at every size class, with exactly the bytes mip 0 holds."""
    out = []
    for fmt in FIVE:
        for w, h in SIZES:
            name = f"{fmt.name}-{w}x{h}-exact"
            out.append(case(name, fmt, w, h, seeded(name, needed(fmt, w, h))))
    return out


def length_cases() -> list[dict]:
    """Data longer than mip 0 (one byte more, a mip chain, twice and more), shorter (one byte, half, one byte
    left), and empty."""
    out = []
    for fmt in FIVE:
        for w, h in ((3, 5), (8, 8), (7, 9), (16, 4)):
            need = needed(fmt, w, h)
            lengths = {
                "long1": need + 1,
                "mips": need + mip_chain(fmt, w, h),
                "double": 2 * need + 5,
                "short1": need - 1,
                "half": need // 2,
                "one": 1,
            }
            for label, n in lengths.items():
                name = f"{fmt.name}-{w}x{h}-{label}"
                out.append(case(name, fmt, w, h, seeded(name, n)))
        out.append(case(f"{fmt.name}-4x4-empty", fmt, 4, 4, {"hex": ""}))
    return out


def side_cases() -> list[dict]:
    """A side of 0 (the empty RGBA image, whatever the data, empty included) and negative sides, with empty data
    (UnityPy's empty-data check comes first), one byte (Pillow's size checks come before its data check) and more."""
    out = []
    for fmt in FIVE:
        for w, h in ((0, 0), (0, 4), (4, 0), (0, -3), (-3, 0), (0, 1)):
            for label, spec in (("empty", {"hex": ""}), ("data", seeded(f"{fmt.name}-{w}x{h}-zero", 64))):
                out.append(case(f"{fmt.name}-{w}x{h}-zero-{label}", fmt, w, h, spec))
        for w, h in ((-1, 4), (4, -1), (-3, -3), (-4, 4), (4, -4), (-1, -1), (-8, 8), (-2, 1), (1, -3)):
            name = f"{fmt.name}-{w}x{h}-negative"
            out.append(case(name, fmt, w, h, seeded(name, 1024)))
            out.append(case(f"{name}-empty", fmt, w, h, {"hex": ""}))
            out.append(case(f"{name}-one", fmt, w, h, seeded(f"{name}-one", 1)))
    return out


def limit_cases() -> list[dict]:
    """Pillow's raw decoder's width cap (`MemoryError` past `INT_MAX / bits - 7`, before it reads the data, so one
    byte is given: at the cap the data check speaks), and its crop's decompression-bomb check, as DXT1: at the fewest
    pixels past twice `MAX_IMAGE_PIXELS`, 59 x 3,033,169 (178,956,971), with all of mip 0's bytes, zero, and with one
    byte (the data check comes before the bomb's); at the most it lets through, 12,470 x 14,351 (178,956,970, padded
    to 12,472 x 14,352: a warning, and the image); and past it at a size no crop changes, 4 x 44,739,244
    (178,956,976), which decodes. Each image is recorded by its sha256 only, as every image past `HEX_PIXELS` is."""
    out = []
    for fmt, cap in ((TF.RGBA32, 67_108_856), (TF.RGB24, 89_478_478)):
        for w in (cap, cap + 1):
            out.append(case(f"{fmt.name}-{w}x1-one", fmt, w, 1, {"hex": "00"}))
    w, h = 59, 3_033_169
    out.append(case(f"DXT1-{w}x{h}-zeros", TF.DXT1, w, h, {"zeros": needed(TF.DXT1, w, h)}))
    out.append(case(f"DXT1-{w}x{h}-one", TF.DXT1, w, h, {"hex": "00"}))
    for w, h in ((12_470, 14_351), (4, 44_739_244)):
        out.append(case(f"DXT1-{w}x{h}-zeros", TF.DXT1, w, h, {"zeros": needed(TF.DXT1, w, h)}))
    return out


def bc7_cases() -> list[dict]:
    """Every BC7 mode, and Pillow's degenerate all-zero first byte (8), at one block, a partial one and many."""
    out = []
    for mode in range(9):
        for w, h in ((4, 4), (5, 7), (64, 64), (30, 18)):
            name = f"BC7-mode{mode}-{w}x{h}"
            out.append(case(name, TF.BC7, w, h, seeded(name, needed(TF.BC7, w, h), bc7_mode=mode)))
    return out


def block(c0: int, c1: int, lut: int) -> bytes:
    return c0.to_bytes(2, "little") + c1.to_bytes(2, "little") + lut.to_bytes(4, "little")


def alpha(a0: int, a1: int, lut: int) -> bytes:
    return bytes([a0, a1]) + lut.to_bytes(6, "little")


def crafted_cases() -> list[dict]:
    """BC1's endpoint orders (c0 > c1, c0 == c1 and c0 < c1, the last two its 3-colour mode), BC3's alpha orders,
    and blocks of all zeros and all ones, each as one 4x4 block and cut to 3x3."""
    lut = 0xE4E4_E4E4  # indices 0, 1, 2, 3 in every row
    alut = 0xFAC688_FAC688  # indices 0 to 7 in each half
    colors = {"gt": block(0xF81F, 0x07E0, lut), "eq": block(0x1234, 0x1234, lut), "lt": block(0x07E0, 0xF81F, lut)}
    alphas = {"gt": alpha(200, 20, alut), "eq": alpha(77, 77, alut), "lt": alpha(20, 200, alut)}
    blocks = {f"DXT1-{k}": (TF.DXT1, v) for k, v in colors.items()}
    for ka, va in alphas.items():
        for kc, vc in colors.items():
            blocks[f"DXT5-a{ka}-c{kc}"] = (TF.DXT5, va + vc)
    for fmt in (TF.DXT1, TF.DXT5, TF.BC7):
        blocks[f"{fmt.name}-zeros"] = (fmt, bytes(BLOCK[fmt]))
        blocks[f"{fmt.name}-ones"] = (fmt, b"\xff" * BLOCK[fmt])
    out = []
    for name, (fmt, data) in blocks.items():
        for w, h in ((4, 4), (3, 3)):
            out.append(case(f"{name}-{w}x{h}", fmt, w, h, {"hex": data.hex()}))
    return out


def platform_cases() -> list[dict]:
    """Textures on a platform UnityPy treats specially that it neither deswizzles nor byte-swaps: RGBA32 and RGB24
    on Xbox 360 (`XBOX_SWAP_FORMATS` holds neither), BC7 there too, and every format on Switch with a platform
    blob that is missing, short, or names one GOB per block (`is_switch_swizzled` false)."""
    blobs = {"none": None, "empty": [], "short": [1] * 11, "one-gob": [0] * 12, "one-gob-long": [7] * 8 + [0] * 8}
    out = []
    for fmt in FIVE:
        for label, blob in blobs.items():
            name = f"{fmt.name}-switch-{label}"
            out.append(case(name, fmt, 7, 9, seeded(name, needed(fmt, 7, 9)), platform=SWITCH, blob=blob))
    for fmt in (TF.RGB24, TF.RGBA32, TF.BC7):
        name = f"{fmt.name}-xbox360"
        out.append(case(name, fmt, 7, 9, seeded(name, needed(fmt, 7, 9)), platform=XBOX360))
    return out


def cleared(pixels: bytes) -> bytes:
    """RGBA bytes with every pixel of alpha 0 set to (0, 0, 0, 0)."""
    out = bytearray(pixels)
    for at in range(0, len(out), 4):
        if out[at + 3] == 0:
            out[at : at + 4] = bytes(4)
    return bytes(out)


def decoded(data: bytes, w: int, h: int, fmt: int, platform: int, blob) -> bytes:
    """The bytes of UnityPy's image of a texture."""
    return Texture2DConverter.get_image_from_texture2d(fake_texture(data, w, h, fmt, platform, blob)).tobytes()


def etc_cases() -> list[dict]:
    """texture_etc.py's cases, recorded as decode.json's; the image of a Tier C format also with the reference's
    sha256 (`reference_sha256`) and the count of pixels UnityPy's differs from it in (`diverging`)."""
    out = []
    for name, fmt, w, h, spec, *where in texture_etc.cases():
        platform, blob = where or (WINDOWS, None)
        c = case(name, TF(fmt), w, h, spec, platform=platform, blob=blob)
        if fmt in (TF.ETC2_RGBA1, TF.EAC_R_SIGNED, TF.EAC_RG_SIGNED) and "mode" in c:
            data = build(spec)
            pixels = decoded(data, w, h, fmt, platform, blob)
            if fmt == TF.ETC2_RGBA1:
                reference = cleared(pixels)
            else:
                # Every block (8 bytes, an EAC_RG_SIGNED block two of them) begins with its signed base.
                patched = bytes(0x81 if at % 8 == 0 and v == 0x80 else v for at, v in enumerate(data))
                reference = decoded(patched, w, h, fmt, platform, blob)
            c["reference_sha256"] = hashlib.sha256(reference).hexdigest()
            c["diverging"] = sum(1 for at in range(0, len(pixels), 4) if pixels[at : at + 4] != reference[at : at + 4])
        out.append(c)
    return out


def raw_cases() -> list[dict]:
    """texture_raw.py's cases, recorded as decode.json's."""
    out = []
    for name, fmt, w, h, spec, *where in texture_raw.cases():
        platform, blob = where or (WINDOWS, None)
        out.append(case(name, TF(fmt), w, h, spec, platform=platform, blob=blob))
    return out


def format_name(value: int) -> str:
    try:
        return TextureFormat(value).name
    except ValueError:
        return str(value)


def selected(fmt: int, width: int, height: int, platform: int, blob) -> str:
    """BATCH_SPEC section 3 with UnityPy's predicates: `export(<name>)` or the skip's reason."""
    if not width or not height:
        return "empty_image"
    name = format_name(fmt)
    if name not in {f.name for f in FIVE}:
        return f"unsupported_format({name})"
    if TextureSwizzler.is_switch_swizzled(platform, blob):
        return "unsupported_platform(switch)"
    if platform == BuildTarget.XBOX360 and TextureFormat(fmt) in Texture2DConverter.XBOX_SWAP_FORMATS:
        return "unsupported_platform(xbox360)"
    return f"export({name})"


SWIZZLING = [0] * 8 + [1, 0, 0, 0]


def select_cases() -> list[dict]:
    """Every format value from -2 to 100 and the ends of i32 on Windows; a few formats (the five, some others,
    a value with no name) on each platform with each platform blob; and sides of 0 and below 0. A blob names a
    GOB exponent of at most 2^24 (`1 << n` is a Python integer)."""
    blobs = [
        None, [], [0] * 11, [0] * 12, SWIZZLING, [0] * 8 + [0, 0, 0, 1], [9] * 8 + [4, 0, 0, 0, 5],
        [0] * 8 + [0, 0, 0, 0, 1], [0] * 8 + [0, 1, 0, 0],
    ]  # fmt: skip
    platforms = (WINDOWS, XBOX360, SWITCH, 0, int(BuildTarget.UnknownPlatform))
    some = [int(f) for f in FIVE] + [int(TF.Alpha8), int(TF.RGB565), int(TF.DXT1Crunched), 100]
    rows = [(f, w, h, WINDOWS, None) for f in [*range(-2, 101), -(2**31), 2**31 - 1] for w, h in ((4, 4), (0, 4))]
    rows += [(f, 4, 4, p, b) for p in platforms for b in blobs for f in some]
    rows += [
        (f, w, h, p, b)
        for p, b in ((WINDOWS, None), (XBOX360, None), (SWITCH, SWIZZLING))
        for f in some
        for w, h in ((4, 0), (0, 0), (-1, 4), (3, -7))
    ]
    return [
        {"format": f, "width": w, "height": h, "platform": p, "blob": b, "selected": selected(f, w, h, p, b)}
        for f, w, h, p, b in rows
    ]


def names() -> list[dict]:
    """UnityPy 1.25.2's TextureFormat members, value and name (enums/TextureFormat.py)."""
    return [{"value": int(f), "name": f.name} for f in TextureFormat]


def fixtures() -> dict[str, dict]:
    meta = {"generator": "tools/fixtures/texture.py", "unitypy": UnityPy.__version__, "pillow": PIL.__version__}
    etc = {**meta, "texture2ddecoder": texture2ddecoder.__version__, "hex_pixels": HEX_PIXELS, "cases": etc_cases()}
    decode = size_cases() + length_cases() + side_cases() + bc7_cases() + crafted_cases() + platform_cases()
    decode += limit_cases()
    return {
        "decode.json": {**meta, "hex_pixels": HEX_PIXELS, "cases": decode},
        "etc.json": etc,
        "raw.json": {**meta, "hex_pixels": HEX_PIXELS, "cases": raw_cases()},
        "select.json": {
            **meta,
            "xbox_swap_formats": [int(f) for f in Texture2DConverter.XBOX_SWAP_FORMATS],
            "names": names(),
            "cases": select_cases(),
        },
    }


def dump(body: dict) -> str:
    """JSON with one list element per line, so a regenerated fixture diffs by case (tools/fixtures/scaffold.py)."""

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
    found = (UnityPy.__version__, PIL.__version__, texture2ddecoder.__version__)
    if found != ("1.25.2", "9.5.0", "1.0.5"):
        print(f"needs UnityPy 1.25.2, Pillow 9.5.0 and texture2ddecoder 1.0.5, found {found}", file=sys.stderr)
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
