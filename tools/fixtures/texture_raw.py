"""The cases of unityrs-texture's oracle fixtures for the rest of the formats UnityPy 1.25.2 decodes with Pillow 9.5.0,
which tools/fixtures/texture.py records into crates/unityrs-texture/tests/fixtures/raw.json (tests/raw_oracle.rs
replays them).

UnityPy decodes Alpha8, ARGB4444, ARGB32, RGB565, BGR24, RGBA4444, BGRA32 and R8 with Pillow's raw decoder, and DXT3,
BC4 and BC5 with its `bcn` decoder (`CONV_TABLE`, export/Texture2DConverter.py:488-566), as it does the five formats
of decode.json. The cases:

- **sizes**: each format at every size class of texture.py, from SplitMix64 data of exactly mip 0's bytes;
- **lengths**: data one byte long, a mip chain long, and twice as long (ignored past mip 0), one byte short, half,
  one byte, and empty;
- **sides**: a side of 0 (the empty image, whatever the data), and negative sides, with data, empty data and one byte;
- **fields**: every value of each packed field: Alpha8's and R8's 256 bytes, and every 16-bit pixel of ARGB4444,
  RGBA4444 and RGB565 (256 x 256, `{"ramp": 2}`), and BGR24's, BGRA32's and ARGB32's bytes counted up, so each
  band's place shows; DXT3's alpha at each of its 16 values in each place, over BC1 colour endpoints in each order
  (BC2 always reads its colour in BC1's four-colour form, even where `c0 <= c1`); BC4's and BC5's alpha endpoints in
  each order (eight values where `a0 > a1`, else six with 0 and 255) with every index, and BC5's two channels in
  different orders; blocks of all zeros and all ones; each at one block (4x4) and cut to 3x3;
- **platforms**: an Xbox 360 texture of each format but RGB565 (which UnityPy byte-swaps there; unityrs's
  selection skips it, PARITY.md section 17's `xbox` gate) and a Switch one whose blob names one GOB per block (not
  swizzled): decoded as on any other platform;
- **limits**: Pillow's raw decoder's width cap for each raw format, `INT_MAX / bits - 7` (`MemoryError` before it
  reads the data, so one byte is given: at the cap the data check speaks); and its crop's decompression-bomb check at
  59 x 3,033,169, as texture.py's DXT1 case, for BC4 from all of mip 0's zeros (the image raises) and for each block
  format from one byte (the data check comes first).

A case's data spec is texture.py's (`seed` and `len`, `hex`, `zeros`), or `{"ramp": K, "len": N}`, the values 0, 1,
2, ... each K bytes little-endian (modulo 2^(8K)), cut to N bytes. Standard library only: a module texture.py
imports.
"""

from __future__ import annotations

import hashlib

# The formats, by TextureFormat value: the raw ones with the bytes a pixel of each takes, the block ones with the
# bytes a 4x4 block of each holds.
ALPHA8, ARGB4444, ARGB32, RGB565, BGR24, DXT3, RGBA4444, BGRA32, BC4, BC5, R8 = 1, 2, 5, 7, 8, 11, 13, 14, 26, 27, 63
NAMES = {
    ALPHA8: "Alpha8",
    ARGB4444: "ARGB4444",
    ARGB32: "ARGB32",
    RGB565: "RGB565",
    BGR24: "BGR24",
    DXT3: "DXT3",
    RGBA4444: "RGBA4444",
    BGRA32: "BGRA32",
    BC4: "BC4",
    BC5: "BC5",
    R8: "R8",
}
PIXEL = {ALPHA8: 1, ARGB4444: 2, ARGB32: 4, RGB565: 2, BGR24: 3, RGBA4444: 2, BGRA32: 4, R8: 1}
BLOCK = {DXT3: 16, BC4: 8, BC5: 16}
SIZES = [
    (1, 1), (2, 2), (3, 5), (4, 4), (7, 9), (5, 3), (8, 8), (6, 13), (12, 12), (17, 1), (1, 17),
    (33, 2), (2, 33), (31, 33), (64, 4), (4, 64), (100, 60), (128, 128), (257, 3),
]  # fmt: skip
XBOX360, SWITCH = 11, 38
INT_MAX = 2**31 - 1


def seed_of(name: str) -> int:
    """texture.py's seed for a case name."""
    return int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "little") & ((1 << 63) - 1)


def seeded(name: str, n: int) -> dict:
    return {"seed": seed_of(name), "len": n}


def ramp(width: int, n: int) -> bytes:
    """The values 0, 1, 2, ... each `width` bytes little-endian (modulo 2^(8 width)), cut to `n` bytes."""
    count = -(-n // width)
    return b"".join((i % (1 << (8 * width))).to_bytes(width, "little") for i in range(count))[:n]


def needed(fmt: int, width: int, height: int) -> int:
    """The bytes mip 0 of a `width` x `height` texture holds (both sides positive)."""
    if fmt in BLOCK:
        return ((width + 3) // 4) * ((height + 3) // 4) * BLOCK[fmt]
    return width * height * PIXEL[fmt]


def mip_chain(fmt: int, width: int, height: int) -> int:
    total = 0
    while width > 1 or height > 1:
        width, height = max(1, width // 2), max(1, height // 2)
        total += needed(fmt, width, height)
    return total


def size_cases() -> list[tuple]:
    return [
        (f"{n}-{w}x{h}-exact", f, w, h, seeded(f"{n}-{w}x{h}-exact", needed(f, w, h)))
        for f, n in NAMES.items()
        for w, h in SIZES
    ]


def length_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        for w, h in ((3, 5), (16, 4)):
            need = needed(f, w, h)
            lengths = {
                "long1": need + 1,
                "mips": need + mip_chain(f, w, h),
                "double": 2 * need + 5,
                "short1": need - 1,
                "half": need // 2,
                "one": 1,
            }
            for label, size in lengths.items():
                out.append((f"{n}-{w}x{h}-{label}", f, w, h, seeded(f"{n}-{w}x{h}-{label}", size)))
        out.append((f"{n}-4x4-empty", f, 4, 4, {"hex": ""}))
    return out


def side_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        for w, h in ((0, 0), (0, 4), (4, 0), (0, -3)):
            for label, spec in (("empty", {"hex": ""}), ("data", seeded(f"{n}-{w}x{h}-zero", 64))):
                out.append((f"{n}-{w}x{h}-zero-{label}", f, w, h, spec))
        for w, h in ((-1, 4), (4, -1), (-3, -3), (-4, 4), (-8, 8), (1, -3)):
            name = f"{n}-{w}x{h}-negative"
            out.append((name, f, w, h, seeded(name, 1024)))
            out.append((f"{name}-empty", f, w, h, {"hex": ""}))
            out.append((f"{name}-one", f, w, h, seeded(f"{name}-one", 1)))
    return out


def colour(c0: int, c1: int, lut: int) -> bytes:
    """A BC1 colour block."""
    return c0.to_bytes(2, "little") + c1.to_bytes(2, "little") + lut.to_bytes(4, "little")


def alpha(a0: int, a1: int, lut: int) -> bytes:
    """A BC3 alpha block."""
    return bytes([a0, a1]) + lut.to_bytes(6, "little")


LUT = 0xE4E4_E4E4  # colour indices 0, 1, 2, 3 in every row
ALUT = 0xFAC688_FAC688  # alpha indices 0 to 7 in each half
COLOURS = {"gt": colour(0xF81F, 0x07E0, LUT), "eq": colour(0x1234, 0x1234, LUT), "lt": colour(0x07E0, 0xF81F, LUT)}
ALPHAS = {
    "gt": alpha(200, 20, ALUT),
    "eq": alpha(77, 77, ALUT),
    "lt": alpha(20, 200, ALUT),
    "full": alpha(255, 0, ALUT),
    "empty": alpha(0, 255, ALUT),
}
# DXT3's explicit alpha: pixel n's 4 bits at bit 4n, here n itself, then 15 - n.
NIBBLES = {"up": bytes(range(0x10, 0x100, 0x22)), "down": bytes(0xFF - b for b in range(0x10, 0x100, 0x22))}


def field_cases() -> list[tuple]:
    out = [
        ("Alpha8-16x16-every-byte", ALPHA8, 16, 16, {"ramp": 1, "len": 256}),
        ("R8-16x16-every-byte", R8, 16, 16, {"ramp": 1, "len": 256}),
    ]
    for f in (ARGB4444, RGBA4444, RGB565):
        out.append((f"{NAMES[f]}-256x256-every-pixel", f, 256, 256, {"ramp": 2, "len": 2 * 65536}))
        out.append((f"{NAMES[f]}-16x16-every-low-byte", f, 16, 16, {"ramp": 1, "len": 512}))
    for f in (BGR24, BGRA32, ARGB32):
        out.append((f"{NAMES[f]}-7x5-counted", f, 7, 5, {"ramp": 1, "len": needed(f, 7, 5)}))
    blocks = {}
    for ka, va in NIBBLES.items():
        for kc, vc in COLOURS.items():
            blocks[f"DXT3-a{ka}-c{kc}"] = (DXT3, va + vc)
    for ka, va in ALPHAS.items():
        blocks[f"BC4-{ka}"] = (BC4, va)
    for kr, kg in (("gt", "lt"), ("lt", "gt"), ("eq", "full"), ("empty", "gt")):
        blocks[f"BC5-r{kr}-g{kg}"] = (BC5, ALPHAS[kr] + ALPHAS[kg])
    for f in BLOCK:
        blocks[f"{NAMES[f]}-zeros"] = (f, bytes(BLOCK[f]))
        blocks[f"{NAMES[f]}-ones"] = (f, b"\xff" * BLOCK[f])
    for name, (f, data) in blocks.items():
        for w, h in ((4, 4), (3, 3)):
            out.append((f"{name}-{w}x{h}", f, w, h, {"hex": data.hex()}))
    return out


def platform_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        where = [("switch-one-gob", SWITCH, [0] * 12), ("switch-no-blob", SWITCH, None)]
        if f != RGB565:
            where.append(("xbox360", XBOX360, None))
        for label, platform, blob in where:
            name = f"{n}-{label}"
            out.append((name, f, 7, 9, seeded(name, needed(f, 7, 9)), platform, blob))
    return out


def limit_cases() -> list[tuple]:
    out = []
    for f, n in NAMES.items():
        if f in PIXEL:
            cap = INT_MAX // (8 * PIXEL[f]) - 7
            for w in (cap, cap + 1):
                out.append((f"{n}-{w}x1-one", f, w, 1, {"hex": "00"}))
    w, h = 59, 3_033_169
    out.append((f"BC4-{w}x{h}-zeros", BC4, w, h, {"zeros": needed(BC4, w, h)}))
    for f in BLOCK:
        out.append((f"{NAMES[f]}-{w}x{h}-one", f, w, h, {"hex": "00"}))
    return out


def cases() -> list[tuple]:
    """Every case: (name, format, width, height, data spec[, platform, blob])."""
    return size_cases() + length_cases() + side_cases() + field_cases() + platform_cases() + limit_cases()
