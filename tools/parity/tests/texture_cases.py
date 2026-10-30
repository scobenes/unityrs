"""The pixel cases of `test_texture_matrix.py`, not a test module: the reference each format's layout gives, built
without UnityPy (that module's docstring says how), and in `PIXEL_CASES` each format's conversion and case.
"""

import struct
import sys
from collections import namedtuple
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import references  # noqa: E402

# ---------------------------------------------------------------------------------------------
# Pixel cases: the reference each format's layout gives, independent of UnityPy.

W = H = 8
Case = namedtuple("Case", "data mode pixels unitypy", defaults=(None,))


def flipped(pixel_at):
    """Every pixel UnityPy returns, top row first, for data whose pixel (x, y) in data order is
    pixel_at(x, y): UnityPy flips each image top to bottom."""
    return [pixel_at(x, H - 1 - y) for y in range(H) for x in range(W)]


def pattern(size, start=b""):
    """A deterministic byte pattern that reaches most byte values."""
    return start + bytes((i * 47 + 13) % 256 for i in range(size - len(start)))


def raw(bpp, mode, decode, start=b""):
    """A raw format: one pixel of `bpp` bytes, `decode` giving the reference pixel."""
    data = pattern(bpp * W * H, start)

    def at(x, y):
        i = (y * W + x) * bpp
        return decode(data[i : i + bpp])

    return Case(data, mode, flipped(at))


def u16(b):
    return b[0] | b[1] << 8


def nibbles(v, *shifts):
    return tuple((v >> s & 15) * 17 for s in shifts)


def widen(v, bits):
    """A field of `bits` bits widened to 8 as Pillow does: `v * 255 / (2**bits - 1)`, truncated."""
    return v * 255 // ((1 << bits) - 1)


def rgb565_red_low(v):
    """Pillow's `RGB;16`: 5-6-5 with red in the low five bits."""
    return widen(v & 31, 5), widen(v >> 5 & 63, 6), widen(v >> 11, 5)


def blocks(block_w, block_h, mode, solids, block_bytes=None):
    """A block format, block i holding solids[i % len(solids)] = (encoded block, its reference pixel)."""
    across, down = -(-W // block_w), -(-H // block_h)
    order = [solids[i % len(solids)] for i in range(across * down)]
    data = b"".join(encoded for encoded, _ in order)
    if block_bytes is not None:
        assert all(len(encoded) == block_bytes for encoded, _ in order)

    def at(x, y):
        return order[(y // block_h) * across + x // block_w][1]

    return Case(data, mode, flipped(at))


def indices(index, bits, byteorder="little"):
    """16 pixel indices of `bits` bits, all `index`."""
    value = 0
    for k in range(16):
        value |= index << (bits * k)
    return value.to_bytes(bits * 2, byteorder)


def bc1(c0, c1=0, index=0):
    """A BC1 colour block: two RGB565 endpoints, every pixel at `index`."""
    return struct.pack("<HH", c0, c1) + indices(index, 2)


def bc4(value):
    """A BC4 (and BC3 alpha) block: both endpoints `value`, every index 0, so every pixel is `value`."""
    return bytes([value, value]) + indices(0, 3)


def bc7_mode6(r, g, b, a):
    """A BC7 mode 6 block of one colour: 7-bit endpoints plus a shared low bit, so all channels odd or all even."""
    parity = r & 1
    assert all(c & 1 == parity for c in (g, b, a))
    bits, pos = 1 << 6, 7
    for channel in (r, g, b, a):
        bits |= (channel >> 1) << pos | (channel >> 1) << (pos + 7)
        pos += 14
    bits |= parity << 63 | parity << 64  # every index 0: endpoint 0 exactly
    return bits.to_bytes(16, "little")


def clamp8(v):
    return max(0, min(255, v))


def etc1(r4, g4, b4):
    """An ETC1 individual-mode block, both halves the 4-bit colour, table 0, every pixel index 0 (+2)."""
    head = bytes([r4 << 4 | r4, g4 << 4 | g4, b4 << 4 | b4, 0])
    return head + bytes(4), tuple(clamp8(c * 17 + 2) for c in (r4, g4, b4)) + (255,)


def expand5(v):
    return v << 3 | v >> 2


def etc2_punchthrough(r5, g5, b5, opaque=True):
    """An ETC2 RGBA1 block in differential mode with no delta, table 0. Opaque: every pixel index 0 (+2).
    Not opaque: every pixel index 2 (msb 1, lsb 0), which is transparent, and the Khronos Data Format
    Specification 1.3 section 22.9 sets its RGB to 0 too."""
    head = bytes([r5 << 3, g5 << 3, b5 << 3, int(opaque) << 1])
    if opaque:
        return head + bytes(4), tuple(clamp8(expand5(c) + 2) for c in (r5, g5, b5)) + (255,)
    return head + b"\xff\xff\x00\x00", (0, 0, 0, 0)


def etc2_rgba1():
    solids = [etc2_punchthrough(31, 0, 0), etc2_punchthrough(0, 31, 0), etc2_punchthrough(16, 16, 16)]
    transparent, _ = etc2_punchthrough(0, 0, 31, opaque=False)
    case = blocks(4, 4, "RGBA", solids + [(transparent, (0, 0, 0, 0))], 8)
    # texture2ddecoder keeps the transparent pixel's colour (the base, modifier 0) and zeroes only alpha.
    kept = blocks(4, 4, "RGBA", solids + [(transparent, (0, 0, 255, 0))], 8)
    return case._replace(unitypy=("RGBA", kept.pixels))


def eac(base):
    """An EAC block (R11 or the ETC2 alpha block) of table 13, index 4, whose modifier is 0: every value is the base."""
    return bytes([base & 0xFF, 13]) + indices(4, 3, byteorder="big")


def atc(c0):
    """An ATC block, colour 0 an RGB555 word (method bit clear), every pixel index 0."""
    return struct.pack("<HH", c0, 0) + indices(0, 2)


def pvrtc(r5, g5, b4):
    """A PVRTC block whose opaque colours A and B are equal, so every pixel is that colour whatever the
    modulation and interpolation. Colour A's 4-bit blue widens by repeating its top bit."""
    b5 = b4 << 1 | b4 >> 3
    a = 1 << 15 | r5 << 10 | g5 << 5 | b4 << 1
    b = 1 << 15 | r5 << 10 | g5 << 5 | b5
    return struct.pack("<II", 0, a | b << 16), (expand5(r5), expand5(g5), expand5(b5), 255)


def astc_void_extent(r, g, b, a, hdr=False):
    """An ASTC constant-colour (void-extent) block: UNORM16 channels, or FP16 ones when `hdr`."""
    return bytes([0xFC, 0xFF if hdr else 0xFD]) + b"\xff" * 6 + struct.pack("<4H", r, g, b, a)


def half_bits(value):
    return struct.unpack("<H", struct.pack("<e", value))[0]


def astc(block):
    colours = [(255, 0, 0, 255), (0, 255, 0, 128), (0, 0, 255, 255), (64, 128, 192, 32)]
    solids = [(astc_void_extent(*(c * 257 for c in colour)), colour) for colour in colours]
    return blocks(block, block, "RGBA", solids, 16)


BC5_PAIRS = [(255, 0), (0, 255), (16, 200), (128, 64)]
BC7_COLOURS = [(201, 101, 51, 255), (1, 255, 1, 127), (0, 0, 254, 2), (64, 128, 192, 32)]
ATC_SOLIDS = [
    (0x7C00, (255, 0, 0, 255)),
    (0x03E0, (0, 255, 0, 255)),
    (0x001F, (0, 0, 255, 255)),
    (0x7FFF, (255, 255, 255, 255)),
]
RGB565_SOLIDS = [(0xF800, (255, 0, 0, 255)), (0x07E0, (0, 255, 0, 255)), (0x001F, (0, 0, 255, 255))]
ETC1_SOLIDS = [etc1(15, 0, 0), etc1(0, 15, 0), etc1(0, 0, 15), etc1(8, 4, 12)]
EAC_BASES = [16, 100, 200, 255]
EAC_SIGNED_BASES = [100, -100, 0, -128]  # -128 is clamped to -127, so 0 after the +127


def eac_signed_value(base):
    return max(base, -127) + 127


def rgba_half():
    halves = (0.5, 0.25, 0.125, 0.75) * (W * H)
    data = struct.pack(f"<{len(halves)}e", *halves)
    reference = flipped(lambda x, y: tuple(references.float_to_byte(h) for h in halves[4 * (y * W + x) :][:4]))
    # Three halves read per pixel of four stored, each int(h * 256): alpha dropped, pixels misaligned.
    unitypy = flipped(lambda x, y: tuple(int(h * 256) for h in halves[3 * (y * W + x) :][:3]))
    return Case(data, "RGBA", reference, ("RGB", unitypy))


RGB48_PIXELS = [(0xFFFF, 0, 0), (0xFF00, 0x00FF, 0x8080)]  # stored 16-bit channels, repeated


def rgb48():
    stored = [RGB48_PIXELS[i % 2] for i in range(W * H)]
    data = b"".join(struct.pack("<3H", *pixel) for pixel in stored)
    # The reference is each channel's high byte, `v >> 8`: 0xFF00 gives 255 and 0x00FF 0, where
    # rounding (`round(v * 255 / 65535)`) gives 254 and 1.
    reference = flipped(lambda x, y: tuple(v >> 8 for v in stored[y * W + x]))
    # Pillow's RGB;16 is 5-6-5 with red in the low bits, two bytes a pixel: the first W * H words.
    words = struct.unpack(f"<{W * H}H", data[: 2 * W * H])
    unitypy = flipped(lambda x, y: rgb565_red_low(words[y * W + x]))
    return Case(data, "RGB", reference, ("RGB", unitypy))


def astc_hdr(block):
    # (0.5, 0.25, 1, 1) in FP16. astcenc's LDR profile decodes every FP16 constant block as 0xFFFF.
    encoded = astc_void_extent(*(half_bits(v) for v in (0.5, 0.25, 1.0, 1.0)), hdr=True)
    case = blocks(block, block, "RGBA", [(encoded, (128, 64, 255, 255))], 16)
    return case._replace(unitypy=("RGBA", [(255, 255, 255, 255)] * (W * H)))


def etc_rgb4_3ds():
    # Decoded as plain ETC1 in plain block order: what UnityPy returns, with no 3DS reference to check it.
    plain = blocks(4, 4, "RGBA", ETC1_SOLIDS, 8)
    return Case(plain.data, None, None, ("RGBA", plain.pixels))


def etc_rgba8_3ds():
    # 16 bytes a 4x4 block: UnityPy hands the data to the ETC1 decoder, which reads its first half
    # as four plain 8-byte blocks and never sees the rest; alpha is 255.
    plain = blocks(4, 4, "RGBA", ETC1_SOLIDS, 8)
    data = plain.data + b"\xa5" * len(plain.data)
    return Case(data, None, None, ("RGBA", plain.pixels))


HDR_AS_4X4 = "HDR blocks decoded with the LDR profile, as `ASTC_HDR_4x4`"

# Each format that returns an image: (its conversion cell, the case that shows it).
PIXEL_CASES = {
    "Alpha8": ("(0, 0, 0, A)", lambda: raw(1, "RGBA", lambda b: (0, 0, 0, b[0]))),
    "ARGB4444": ("bands reordered (2, 1, 0, 3)", lambda: raw(2, "RGBA", lambda b: nibbles(u16(b), 8, 4, 0, 12))),
    "RGB24": ("as stored", lambda: raw(3, "RGB", tuple)),
    "RGBA32": ("as stored", lambda: raw(4, "RGBA", tuple)),
    "ARGB32": ("alpha moved last", lambda: raw(4, "RGBA", lambda b: (b[1], b[2], b[3], b[0]))),
    # The first pixel's red is 16: widened by truncation it gives 131.
    "RGB565": (
        "red from the top 5 bits; each field widened as `v * 255 / 31` (green `/ 63`), truncated: red 16"
        " gives 131, where bit replication gives 132",
        lambda: raw(
            2,
            "RGB",
            lambda b: (widen(u16(b) >> 11, 5), widen(u16(b) >> 5 & 63, 6), widen(u16(b) & 31, 5)),
            start=b"\x00\x80",
        ),
    ),
    "BGR24": ("blue and red swapped", lambda: raw(3, "RGB", lambda b: (b[2], b[1], b[0]))),
    "RGBA4444": ("bands reordered (3, 2, 1, 0)", lambda: raw(2, "RGBA", lambda b: nibbles(u16(b), 12, 8, 4, 0))),
    "BGRA32": ("blue and red swapped", lambda: raw(4, "RGBA", lambda b: (b[2], b[1], b[0], b[3]))),
    "R8": ("(R, 0, 0)", lambda: raw(1, "RGB", lambda b: (b[0], 0, 0))),
    "RGBAHalf": (
        "three halves per pixel for four stored, each `int(h * 256)`: alpha dropped, pixels misaligned"
        " (`(.5, .25, .125, .75)` repeated gives `(128, 64, 32)`, `(192, 128, 64)`, `(32, 192, 128)`, …)",
        rgba_half,
    ),
    "RGB48": (
        "Pillow reads `RGB;16` as packed 5-6-5 with red in the low five bits, each field widened as"
        " `v * 255 / 31` (green `/ 63`) and truncated, two bytes per pixel for six stored",
        rgb48,
    ),
    "DXT1": (
        "1-bit alpha",
        lambda: blocks(
            4, 4, "RGBA", [(bc1(c), px) for c, px in RGB565_SOLIDS] + [(bc1(0, 0xFFFF, 3), (0, 0, 0, 0))], 8
        ),
    ),
    "DXT3": (
        "explicit 4-bit alpha",
        lambda: blocks(
            4,
            4,
            "RGBA",
            [
                (bytes([a << 4 | a]) * 8 + bc1(c), px[:3] + (a * 17,))
                for a, (c, px) in zip((8, 15, 0, 4), RGB565_SOLIDS + [(0xFFFF, (255, 255, 255, 255))], strict=True)
            ],
            16,
        ),
    ),
    "DXT5": (
        "interpolated alpha",
        lambda: blocks(
            4,
            4,
            "RGBA",
            [
                (bc4(a) + bc1(c), px[:3] + (a,))
                for a, (c, px) in zip((200, 100, 0, 255), RGB565_SOLIDS + [(0xFFFF, (255, 255, 255, 255))], strict=True)
            ],
            16,
        ),
    ),
    "BC4": ("one channel", lambda: blocks(4, 4, "L", [(bc4(v), v) for v in (0x10, 0x80, 0xC0, 0xFF)], 8)),
    "BC5": ("(R, G, 0)", lambda: blocks(4, 4, "RGB", [(bc4(r) + bc4(g), (r, g, 0)) for r, g in BC5_PAIRS], 16)),
    "BC7": ("as decoded", lambda: blocks(4, 4, "RGBA", [(bc7_mode6(*c), c) for c in BC7_COLOURS], 16)),
    "PVRTC_RGB2": ("BGRA to RGBA", lambda: blocks(8, 4, "RGBA", [pvrtc(31, 16, 8)], 8)),
    "PVRTC_RGBA2": ("BGRA to RGBA", lambda: blocks(8, 4, "RGBA", [pvrtc(31, 16, 8)], 8)),
    "PVRTC_RGB4": ("BGRA to RGBA", lambda: blocks(4, 4, "RGBA", [pvrtc(31, 16, 8)], 8)),
    "PVRTC_RGBA4": ("BGRA to RGBA", lambda: blocks(4, 4, "RGBA", [pvrtc(31, 16, 8)], 8)),
    "ETC_RGB4": ("alpha 255", lambda: blocks(4, 4, "RGBA", ETC1_SOLIDS, 8)),
    "ETC2_RGB": ("alpha 255", lambda: blocks(4, 4, "RGBA", ETC1_SOLIDS, 8)),
    "ETC2_RGBA1": (
        "BGRA to RGBA; a transparent punch-through pixel keeps its colour and gets alpha 0",
        etc2_rgba1,
    ),
    "ETC2_RGBA8": (
        "BGRA to RGBA",
        lambda: blocks(
            4,
            4,
            "RGBA",
            [(eac(a) + block, px[:3] + (a,)) for a, (block, px) in zip((200, 100, 0, 255), ETC1_SOLIDS, strict=True)],
            16,
        ),
    ),
    "EAC_R": ("(R, 0, 0, 255)", lambda: blocks(4, 4, "RGBA", [(eac(v), (v, 0, 0, 255)) for v in EAC_BASES], 8)),
    "EAC_R_SIGNED": (
        "(R + 127, 0, 0, 255), -128 clamped to -127: base 100 gives 227, -100 gives 27",
        lambda: blocks(4, 4, "RGBA", [(eac(v), (eac_signed_value(v), 0, 0, 255)) for v in EAC_SIGNED_BASES], 8),
    ),
    "EAC_RG": (
        "(R, G, 0, 255)",
        lambda: blocks(
            4,
            4,
            "RGBA",
            [(eac(r) + eac(g), (r, g, 0, 255)) for r, g in zip(EAC_BASES, reversed(EAC_BASES), strict=True)],
            16,
        ),
    ),
    "EAC_RG_SIGNED": (
        "(R + 127, G + 127, 0, 255)",
        lambda: blocks(
            4,
            4,
            "RGBA",
            [
                (eac(r) + eac(g), (eac_signed_value(r), eac_signed_value(g), 0, 255))
                for r, g in zip(EAC_SIGNED_BASES, reversed(EAC_SIGNED_BASES), strict=True)
            ],
            16,
        ),
    ),
    "ATC_RGB4": ("alpha 255", lambda: blocks(4, 4, "RGBA", [(atc(c), px) for c, px in ATC_SOLIDS], 8)),
    "ATC_RGBA8": (
        "BGRA to RGBA",
        lambda: blocks(
            4,
            4,
            "RGBA",
            [(bc4(a) + atc(c), px[:3] + (a,)) for a, (c, px) in zip((200, 0, 255, 64), ATC_SOLIDS, strict=True)],
            16,
        ),
    ),
    **{f"ASTC_RGB_{n}x{n}": ("as decoded", lambda n=n: astc(n)) for n in (4, 5, 6, 8, 10, 12)},
    **{f"ASTC_RGBA_{n}x{n}": ("as decoded", lambda n=n: astc(n)) for n in (4, 5, 6, 8, 10, 12)},
    "ASTC_HDR_4x4": (
        "HDR blocks decoded with the LDR profile: an FP16 constant-colour block decodes as white, because"
        " astcenc's LDR profile writes 0xFFFF for one",
        lambda: astc_hdr(4),
    ),
    **{f"ASTC_HDR_{n}x{n}": (HDR_AS_4X4, lambda n=n: astc_hdr(n)) for n in (5, 6, 8, 10, 12)},
    "ETC_RGB4_3DS": ("plain ETC1 blocks in plain order; any 3DS tiling or byte order is ignored", etc_rgb4_3ds),
    "ETC_RGBA8_3DS": (
        "the first half of the 16-byte-per-block data read as ETC1 blocks, alpha 255; the alpha is never decoded",
        etc_rgba8_3ds,
    ),
}
