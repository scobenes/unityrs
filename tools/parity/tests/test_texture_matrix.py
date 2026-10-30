"""The TextureFormat matrix of PARITY.md section 17, against the oracle.

`ThisParityFile` reads the matrix with the standard library alone. `AgainstTheOracle`
needs UnityPy 1.25.2 (the oracle image) and is skipped elsewhere.

`AgainstTheOracle` builds every cell of every row itself and requires the matrix to hold
exactly that, so a change to any cell fails a test. No builder reads PARITY.md:
`test_no_cell_is_built_from_this_file` builds the rows again in a new process, from a copy
of it with every cell changed. The cells come:
- from UnityPy: the value; the decoder (`CONV_TABLE`); the mode and the result of
  `parse_image_data` on real-sized data, with the source lines the result cites; the
  padding, Switch, Xbox and crunch gates, observed by running `parse_image_data` with
  stand-ins for the decoders, the deswizzle and the crunch unpackers; and what
  `image_to_texture2d` writes;
- from the pixel cases below: the conversion, and whether a row works, is wrong or is
  unverified;
- from `RESULT_NOTES` and `RAISE_REASONS`: the words a row adds to its result, each
  shown by a test here;
- the tier from the result, and the status: pinned by this test, implemented in the phase
  `DESIGN.md`'s P1 list gives.

The pixel cases are built here from each format's layout, not from UnityPy: raw
formats from a byte pattern, block formats from solid-colour blocks whose decoded
colour the format's specification fixes (one colour per block, so block order and
UnityPy's vertical flip show too). A row marked `works` must decode to that
reference; a row marked `wrong` must decode to the wrong pixels the case states and
not to the reference; a row marked `unverified` has no reference, and its pixels are
pinned as UnityPy returns them. A crunched row is decoded with the crunch unpacker
replaced by one that returns its base format's case, so its conversion is checked
and the crunch stream itself is not.
"""

import inspect
import json
import math
import random
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from collections import namedtuple
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import inventory  # noqa: E402
import references  # noqa: E402

try:
    import texture2ddecoder
    import UnityPy
    from PIL import Image
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.helpers import TextureSwizzler

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False

PARITY = inventory.DEFAULT_PARITY
NEEDS_ORACLE = "needs UnityPy 1.25.2 (the oracle image)"

COLUMNS = [
    "value",
    "format",
    "decoder",
    "mode",
    "conversion",
    "gates",
    "UnityPy 1.25.2",
    "`set_image` writes",
    "tier",
    "status",
]
RESULT = re.compile(r"(works|wrong|unverified|raises|crunch)\b")
# The tier each result gives (DESIGN.md section 3): a crunched row is Tier A like its base format.
KIND_TIER = {"works": "A", "crunch": "A", "raises": "B", "wrong": "C", "unverified": "U"}


def tier_of(name, kind):
    """A raise is Tier B where section 17.1 names a reference (`references.TIER_B`), else Tier U."""
    if kind == "raises" and name not in references.TIER_B:
        return "U"
    return KIND_TIER[kind]


# DESIGN.md section 5: the formats P1 implements; every other format is P3.
P1 = ["BC7", "DXT1", "DXT5", "RGB24", "RGBA32"]
VERSION = (2021, 3, 1, 3)  # the matrix's 2021.3.1f3
# A platform blob that makes TextureSwizzler.is_switch_swizzled hold: 1 << 1 = 2 GOBs per block.
GOBS = 2
SWITCH_BLOB = bytes(8) + (1).to_bytes(4, "little")
# Versions either side of the 2017.3 crunch boundary.
CRUNCH_VERSIONS = [(5, 6, 0, 0), (2017, 2, 9, 0), (2017, 3, 0, 0), (2021, 3, 1, 3)]

# What Unity stores for one pixel of each uncompressed TextureFormat, by name.
BYTES_PER_PIXEL = {
    name: size
    for size, names in {
        1: "Alpha8 R8 R8_SIGNED",
        2: "ARGB4444 RGBA4444 RGB565 R16 RHalf RG16 RG16_SIGNED R16_SIGNED YUY2",
        3: "RGB24 BGR24 RGB24_SIGNED",
        4: "RGBA32 ARGB32 BGRA32 RFloat RGHalf RGB9e5Float RG32 RGBA32_SIGNED RG32_SIGNED",
        6: "RGB48 RGB48_SIGNED",
        8: "RGBAHalf RGFloat RGBA64 RGBA64_SIGNED",
        12: "RGBFloat",
        16: "ARGBFloat RGBAFloat",
    }.items()
    for name in names.split()
}
# Block formats whose blocks are 8 bytes; every other block format has 16-byte blocks.
EIGHT_BYTE_BLOCKS = {
    "DXT1",
    "BC4",
    "ETC_RGB4",
    "ETC_RGB4_3DS",
    "ETC2_RGB",
    "ETC2_RGBA1",
    "EAC_R",
    "EAC_R_SIGNED",
    "ATC_RGB4",
    "PVRTC_RGB2",
    "PVRTC_RGBA2",
    "PVRTC_RGB4",
    "PVRTC_RGBA4",
}


def base_name(name):
    """The plain format a crunched one unpacks to: `DXT1` for `DXT1Crunched`."""
    return name.removesuffix("Crunched")


def unit(fmt):
    """(width, height, bytes) of the smallest unit the format stores: a pixel, or a block."""
    name = base_name(fmt.name)
    if name in BYTES_PER_PIXEL:
        return 1, 1, BYTES_PER_PIXEL[name]
    # UnityPy's own block table; ATC is absent from it (it pads nothing) and has 4x4 blocks.
    block_w, block_h = Texture2DConverter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE[fmt] or (4, 4)
    return block_w, block_h, 8 if name in EIGHT_BYTE_BLOCKS else 16


def stored_size(fmt, width, height):
    """The bytes a real texture of `fmt` holds for its top level: what `parse_image_data` gets from Unity."""
    unit_w, unit_h, size = unit(fmt)
    return -(-width // unit_w) * -(-height // unit_h) * size


def exception_name(error):
    """`ValueError` for a builtin, `struct.error` for anything else: how the matrix names a raise."""
    kind = type(error)
    return kind.__qualname__ if kind.__module__ == "builtins" else f"{kind.__module__}.{kind.__qualname__}"


def first_span(cell):
    match = re.search(r"`([^`]+)`", cell)
    return match.group(1) if match else None


class ThisParityFile(unittest.TestCase):
    """What PARITY.md says, read without the oracle."""

    def setUp(self):
        text = PARITY.read_text(encoding="utf-8")
        self.rows = inventory.texture_rows(text)
        self.header = next(header for header, _ in inventory.tables(text) if header[:2] == ["value", "format"])

    def test_the_matrix_has_one_row_per_texture_format(self):
        self.assertEqual(self.header, COLUMNS)
        self.assertEqual(len(self.rows), 78)
        self.assertEqual(len({row["value"] for row in self.rows.values()}), 78)

    def test_tier_b_has_the_21_formats_design_names(self):
        tier_b = sorted(name for name, row in self.rows.items() if row["tier"] == "B")
        signed = [name for name in self.rows if name.endswith("_SIGNED") and not name.startswith("EAC")]
        expected = ["BC6H", "RHalf", "RGHalf", "RFloat", "RGFloat", "RGBAFloat", "RGBFloat"]
        expected += signed + ["RG16", "RG32", "R16", "RGBA64", "RGB9e5Float", "YUY2"]
        self.assertEqual(len(signed), 8)
        self.assertEqual(tier_b, sorted(expected))
        self.assertEqual(len(tier_b), 21)
        self.assertEqual(self.rows["ARGBFloat"]["tier"], "U", "raises, and no reference can be named")

    def test_every_tier_b_row_names_its_reference(self):
        """Section 17.1 has one row per Tier B format, in the mode `references` returns, and nothing else."""
        text = PARITY.read_text(encoding="utf-8")
        header, body = next(
            (h, b) for h, b in inventory.tables(text) if h[:3] == ["format", "stored per pixel (little-endian)", "mode"]
        )
        self.assertEqual(header, ["format", "stored per pixel (little-endian)", "mode", "reference", "test", "status"])
        listed = {first_span(cells[0]): cells for cells in body}
        tier_b = {name for name, row in self.rows.items() if row["tier"] == "B"}
        self.assertEqual(sorted(listed), sorted(tier_b))
        self.assertEqual(sorted(listed), sorted(references.TIER_B))
        for name, cells in listed.items():
            with self.subTest(name):
                self.assertEqual(cells[2], references.TIER_B[name])
                self.assertTrue(cells[3].strip() and cells[3] != "—")
                self.assertEqual(cells[4], "planned: divergence test against `references.decode`")
        self.assertEqual(
            sorted(references.UNSUPPORTED),
            [
                n
                for n, r in self.rows.items()
                if r["tier"] == "U" and RESULT.match(r["UnityPy 1.25.2"]).group(1) == "raises"
            ],
        )

    def test_p1_formats_are_the_five_design_names(self):
        p1 = sorted(name for name, row in self.rows.items() if row["status"] == "pinned P0; impl P1")
        self.assertEqual(p1, P1)

    def test_tier_and_status_follow_the_result(self):
        for name, row in self.rows.items():
            with self.subTest(name):
                kind = RESULT.match(row["UnityPy 1.25.2"]).group(1)
                self.assertEqual(row["tier"], tier_of(name, kind))
                self.assertEqual(row["status"], "pinned P0; impl P1" if name in P1 else "pinned P0; impl P3")


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

# Words a row's result adds after `wrong`, each shown by a test of AgainstTheOracle (named), and
# the source it cites after the CONV_TABLE entry: ("function", name) for a function of
# Texture2DConverter.py, ("call", regex) for a call from the line the regex matches to its `)` line.
RESULT_NOTES = {
    # half_values_outside_a_byte_raise
    "RGBAHalf": (
        "; a value at 1.0 or above, at -1/256 or below, or NaN raises `ValueError`, an infinity `OverflowError`",
        [("function", "half")],
    ),
    # pixels_match_the_matrix (the case's reference)
    "ETC2_RGBA1": (
        ": the Khronos Data Format Specification 1.3, section 22.9, makes a transparent punch-through pixel"
        " (0, 0, 0, 0)",
        [],
    ),
    **{f"ASTC_HDR_{n}x{n}": ("", [("call", r"config = astc_encoder\.ASTCConfig\(")]) for n in (4, 5, 6, 8, 10, 12)},
}
# The words after `raises `E`` for each exception and message the oracle gives on real-sized data.
RAISE_REASONS = {
    ("ValueError", "unknown raw mode for given image mode"): ", unknown raw mode",
    ("ValueError", "unrecognized image mode"): ", unrecognized image mode",
    ("ValueError", "bad image mode"): ", bad image mode",
    ("KeyError", "'raw'"): " `'raw'`",
    ("TypeError", "'float' object cannot be interpreted as an integer"): ", floats into a bytearray",
    ("NotImplementedError", "Not implemented texture format: YUY2"): "",
    ("NotImplementedError", "Not implemented texture format: RGBFloat"): "",
    # RHalf; half_values_outside_a_byte_raise shows the rest.
    ("struct.error", "unpack requires a buffer of 2 bytes"): (
        ": `half` reads three halves a pixel for the one stored, so real-sized data runs out; a value at 1.0"
        " or above, at -1/256 or below, or NaN raises `ValueError` first, an infinity `OverflowError`, and"
        " Pillow's missing mode `R` (`ValueError` too) is reached only with three times the data"
    ),
}
# texture2ddecoder's names for the modes `etc` and `eac` take.
ETC_MODES = {"ETC1": "ETC1", "ETC2_RGB": "ETC2", "ETC2_A1": "ETC2 A1", "ETC2_A8": "ETC2 A8"}
EAC_MODES = {"EAC_R": "EAC R", "EAC_R_SIGNED": "EAC R signed", "EAC_RG": "EAC RG", "EAC_RG_SIGNED": "EAC RG signed"}
# `set_image` writes plain ETC1 data under these tags.
PLAIN_ETC1 = ("ETC_RGB4_3DS", "ETC_RGB4Crunched")


# ---------------------------------------------------------------------------------------------
# The oracle's side of each cell.


def source_lines():
    return inspect.getsource(Texture2DConverter).splitlines()


def line_of(pattern):
    """The 1-based line of Texture2DConverter.py whose text matches `pattern`; exactly one must."""
    found = [i for i, text in enumerate(source_lines(), 1) if re.search(pattern, text)]
    assert len(found) == 1, (pattern, found)
    return found[0]


def cite(first, last=None):
    return f"`:{first}`" if last in (None, first) else f"`:{first}-{last}`"


def cite_source(anchor):
    """The lines of a function of Texture2DConverter.py, or of a call from the line a regex matches to its `)`."""
    kind, name = anchor
    if kind == "function":
        lines, start = inspect.getsourcelines(getattr(Texture2DConverter, name))
        return cite(start, start + len(lines) - 1)
    start = line_of(name)
    end = next(i for i, text in enumerate(source_lines(), 1) if i > start and text.strip() == ")")
    return cite(start, end)


def entry_line(fmt):
    return line_of(rf"^\s*TF\.{fmt.name}: \(")


def raise_line(error):
    """The line of Texture2DConverter.py the exception passed through last."""
    lines = [frame.lineno for frame in _frames(error.__traceback__) if frame.filename == Texture2DConverter.__file__]
    return lines[-1]


def _frames(tb):
    while tb is not None:
        yield inspect.getframeinfo(tb, context=0)
        tb = tb.tb_next


def decode(data, fmt, width=W, height=H, platform=None, blob=None, version=VERSION):
    platform = BuildTarget.StandaloneWindows64 if platform is None else platform
    return Texture2DConverter.parse_image_data(data, width, height, fmt, version, platform, blob)


def stand_in_unpackers():
    """Patches for both crunch unpackers: they record what they get and return it unchanged, so a
    crunched format's data reaches its base format's decoder as it was given."""
    calls = []

    def unpacker(name):
        def unpack(data):
            calls.append((name, bytes(data)))
            return bytes(data)

        return unpack

    patches = [
        mock.patch.object(texture2ddecoder, "unpack_unity_crunch", unpacker("unpack_unity_crunch")),
        mock.patch.object(texture2ddecoder, "unpack_crunch", unpacker("unpack_crunch")),
    ]
    return calls, patches


def with_stand_ins(fmt, width=1, height=1, platform=None, version=VERSION, data=None):
    """Run parse_image_data with every decoder, the deswizzle and the crunch unpackers replaced by
    recorders. Returns (calls, image size or None, exception name or None)."""
    calls, patches = stand_in_unpackers()

    def converter(key):
        def convert(image_data, w, h, *args):
            calls.append(("convert", key, bytes(image_data), w, h))
            return Image.new("RGBA", (w, h))

        return convert

    def deswizzle(image_data, w, h, block_w, block_h, gobs):
        calls.append(("deswizzle", w, h, block_w, block_h, gobs))
        return bytearray(image_data)

    table = {key: (converter(key), ()) for key in TextureFormat}
    patches += [
        mock.patch.dict(Texture2DConverter.CONV_TABLE, table),
        mock.patch.object(TextureSwizzler, "deswizzle", deswizzle),
    ]
    blob = SWITCH_BLOB if platform == BuildTarget.Switch else None
    data = bytes(range(256)) * 64 if data is None else data
    for patch in patches:
        patch.start()
    try:
        image = decode(data, fmt, width, height, platform, blob, version)
        return calls, image.size, None
    except Exception as error:  # noqa: BLE001 - the exception type is the result
        return calls, None, exception_name(error)
    finally:
        for patch in reversed(patches):
            patch.stop()


def gates_cell(fmt):
    """The gates cell, from what parse_image_data does on a 1x1 texture."""
    calls, size, error = with_stand_ins(fmt)
    assert error is None and size == (1, 1), (fmt, error, size)  # padded, decoded, cropped back
    (_, _, _, width, height) = next(call for call in calls if call[0] == "convert")
    cells = ["no pad" if (width, height) == (1, 1) else f"pad {width}×{height}"]

    calls, size, error = with_stand_ins(fmt, platform=BuildTarget.Switch)
    if error is not None:
        assert error == "KeyError", (fmt, error)
        cells.append("sw —")
    else:
        _, swizzled_w, swizzled_h, block_w, block_h, gobs = next(call for call in calls if call[0] == "deswizzle")
        _, read_as, _, width, height = next(call for call in calls if call[0] == "convert")
        assert gobs == GOBS and (width, height) == (swizzled_w, swizzled_h), (fmt, calls)  # in place of padding
        cells.append(f"sw {block_w}×{block_h}" + ("" if read_as == fmt else f", read as `{read_as.name}`"))

    data = bytes(range(256)) * 64
    calls, size, error = with_stand_ins(fmt, platform=BuildTarget.XBOX360, data=data)
    first = calls[0][1] if calls[0][0] != "convert" else calls[0][2]  # what the first stage got
    if first[:4] == bytes([1, 0, 3, 2]):
        cells.append("xbox")
    else:
        assert first[:4] == data[:4], (fmt, first[:4])

    unpackers = []
    for version in CRUNCH_VERSIONS:
        calls, size, error = with_stand_ins(fmt, version=version)
        unpackers.append(tuple(call[0] for call in calls if call[0].startswith("unpack")))
    if unpackers == [("unpack_unity_crunch",)] * 4:
        cells.append("crunch: `unpack_unity_crunch` at any version")
    elif unpackers == [("unpack_crunch",)] * 2 + [("unpack_unity_crunch",)] * 2:
        cells.append("crunch: `unpack_crunch` before 2017.3, `unpack_unity_crunch` from 2017.3")
    else:
        assert unpackers == [()] * 4, (fmt, unpackers)
    return "; ".join(cells)


def decoder_cell(fmt):
    entry = Texture2DConverter.CONV_TABLE.get(fmt)
    if entry is None:
        commented = [i for i, text in enumerate(source_lines(), 1) if re.match(rf"\s*# TF\.{fmt.name}:", text)]
        return f"none: the entry is commented out ({cite(commented[0])})" if commented else "none: no entry"
    func, args = entry
    T = Texture2DConverter
    if func is T.pillow:
        mode, codec, rawmode, *swap = args
        text = f"Pillow raw `{rawmode}`" if codec == "raw" else f"Pillow `{codec}` {rawmode}"
        text += f" into `{mode}`" + (f", bands {swap[0]}" if swap else "")
    elif func is T.half:
        mode, codec, rawmode = args
        text = f"`half`, then Pillow {codec} `{rawmode}` into `{mode}`"
    elif func is T.rg:
        text = "`rg` (" + ", ".join(f"`{arg}`" for arg in args) + ")"
    elif func is T.rgb9e5float:
        text = "`rgb9e5float`"
    elif func is T.pvrtc:
        text = f"texture2ddecoder PVRTC {2 if args[0] else 4} bpp"
    elif func is T.etc:
        text = f"texture2ddecoder {ETC_MODES[args[0]]}"
    elif func is T.eac:
        text = f"texture2ddecoder {EAC_MODES[args[0]]}"
    elif func is T.atc:
        text = "texture2ddecoder ATC RGBA" if args[0] else "texture2ddecoder ATC RGB"
    elif func is T.astc:
        text = "astcenc LDR, {}×{} blocks".format(*args[0])
    else:
        raise AssertionError(f"no rendering for {func.__name__}")
    return ("crunch, then " if "Crunched" in fmt.name else "") + text


def real_sized(fmt):
    return bytes([0x30]) * stored_size(fmt, W, H)


def run_on_real_sized_data(fmt):
    """(image, None) or (None, (exception name, message, raising line)), on 0x30 bytes, as many as a real
    8x8 texture holds; a crunched format gets as many as its base format and keeps them through unpacking."""
    calls, patches = stand_in_unpackers()
    for patch in patches:
        patch.start()
    try:
        return decode(real_sized(fmt), fmt), None
    except Exception as error:  # noqa: BLE001 - the exception is the result
        message = str(error)
        return None, (exception_name(error), message, raise_line(error))
    finally:
        for patch in reversed(patches):
            patch.stop()


def kind_of(fmt, raised):
    if raised:
        return "raises"
    if "Crunched" in fmt.name:
        return "crunch"
    case = PIXEL_CASES[fmt.name][1]()
    if case.pixels is None:
        return "unverified"
    return "works" if case.unitypy is None else "wrong"


def result_cell(fmt, kind, raised):
    cites = [cite(entry_line(fmt))] if fmt in Texture2DConverter.CONV_TABLE else []
    if kind == "raises":
        name, message, line = raised
        return f"raises `{name}`{RAISE_REASONS[name, message]} ({', '.join([*cites, cite(line)])})"
    note, anchors = RESULT_NOTES.get(fmt.name, ("", []))
    return f"{kind}{note} ({', '.join(cites + [cite_source(anchor) for anchor in anchors])})"


def set_image_cell(fmt):
    source = Image.new("RGBA", (W, H), (10, 20, 30, 40))
    try:
        data, written = Texture2DConverter.image_to_texture2d(source, fmt)
    except Exception as error:  # noqa: BLE001 - the exception is the result
        return f"raises `{exception_name(error)}` ({cite(raise_line(error))}); Tier C (writing), section 19.2"
    cell = f"`{written.name}`"
    if written.name in PLAIN_ETC1 and data == Texture2DConverter.image_to_texture2d(source, TextureFormat.ETC_RGB4)[0]:
        cell += " holding plain ETC1"
    try:
        from_rgb = Texture2DConverter.image_to_texture2d(source.convert("RGB"), fmt)[1]
    except Exception as error:  # noqa: BLE001 - the exception is the result
        cell += f" (an RGB image with no zero side raises `{exception_name(error)}`, section 18.1)"
    else:
        if from_rgb != written:
            cell += f" (`{from_rgb.name}` from an RGB image)"
    try:
        decode(data, written)
    except Exception:  # noqa: BLE001 - written data its own reader cannot read
        cell += "; Tier C (writing), section 19.2"
    return cell


def expected_row(fmt):
    """Every cell of the format's row, as the oracle and the pixel cases make it."""
    image, raised = run_on_real_sized_data(fmt)
    kind = kind_of(fmt, raised)
    conversion = "—" if raised else PIXEL_CASES[base_name(fmt.name)][0]
    return {
        "value": str(fmt.value),
        "format": f"`{fmt.name}`",
        "decoder": decoder_cell(fmt),
        "mode": "—" if raised else image.mode,
        "conversion": conversion,
        "gates": gates_cell(fmt),
        "UnityPy 1.25.2": result_cell(fmt, kind, raised),
        "`set_image` writes": set_image_cell(fmt),
        "tier": tier_of(fmt.name, kind),
        "status": "pinned P0; impl P1" if fmt.name in P1 else "pinned P0; impl P3",
    }


def matrix_errors(rows, expected):
    """(format, column) for every cell of `rows` that differs from `expected`."""
    return [
        (name, column) for name, cells in expected.items() for column in cells if rows[name][column] != cells[column]
    ]


def change_cells(text, columns):
    """`text` with " changed" added to the named columns' cells in every row of the matrix."""
    lines = text.splitlines(keepends=True)
    header = next(i for i, line in enumerate(lines) if line.startswith("| value | format |"))
    end = next(i for i in range(header, len(lines)) if not lines[i].startswith("|"))
    for i in range(header + 2, end):
        cells = [cell.strip() for cell in lines[i].rstrip("\n").strip("|").split(" | ")]
        cells = [cell + " changed" if column in columns else cell for column, cell in zip(COLUMNS, cells, strict=True)]
        lines[i] = "| " + " | ".join(cells) + " |\n"
    return "".join(lines)


def rebuilt_elsewhere(parity_text):
    """Every row as `expected_row` builds it in a new process, from a copy of tools/parity whose PARITY.md
    is `parity_text`, so a builder that read PARITY.md, even at import, would build from that text."""
    with tempfile.TemporaryDirectory(prefix="unityrs-matrix-") as root:
        copy = Path(root) / "tools" / "parity"
        shutil.copytree(HERE, copy, ignore=shutil.ignore_patterns("__pycache__"))
        (Path(root) / "PARITY.md").write_text(parity_text, encoding="utf-8")
        script = (
            "import json, test_texture_matrix as t\n"
            "print(json.dumps({f.name: t.expected_row(f) for f in t.TextureFormat}))\n"
        )
        result = subprocess.run(
            [sys.executable, "-B", "-c", script], cwd=copy / "tests", capture_output=True, text=True, check=True
        )
    return json.loads(result.stdout)


@unittest.skipUnless(ORACLE, NEEDS_ORACLE)
class AgainstTheOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rows = inventory.texture_rows(PARITY.read_text(encoding="utf-8"))
        cls.expected = {fmt.name: expected_row(fmt) for fmt in TextureFormat}

    def test_texture_matrix_matches_the_oracle(self):
        """Every cell of every row equals what the oracle and the pixel cases make it (section 17)."""
        for name, cells in self.expected.items():
            for column, cell in cells.items():
                with self.subTest(name, column=column):
                    self.assertEqual(self.rows[name][column], cell)

    def test_no_cell_is_built_from_this_file(self):
        """Every cell comes from the oracle and the pixel cases, never from PARITY.md: in a fresh process whose
        PARITY.md has every cell of the matrix changed but the format names, the builders build the same rows,
        and every changed cell then differs from them."""
        self.assertEqual(sorted(next(iter(self.expected.values()))), sorted(COLUMNS))
        self.assertEqual(matrix_errors(self.rows, self.expected), [])
        text = PARITY.read_text(encoding="utf-8")
        others = [column for column in COLUMNS if column != "format"]
        changed = change_cells(text, others)
        everything = sorted((name, column) for name in self.rows for column in others)
        self.assertEqual(sorted(matrix_errors(inventory.texture_rows(changed), self.expected)), everything)
        self.assertEqual(rebuilt_elsewhere(changed), self.expected)
        # The format cells are compared too (keyed by the original names, since the keys change with them).
        renamed = inventory.texture_rows(change_cells(text, ["format"]))
        rows = dict(zip(self.rows, renamed.values(), strict=True))
        self.assertEqual(sorted(matrix_errors(rows, self.expected)), sorted((name, "format") for name in self.rows))
        # The change Codex's review made by hand: DXT1's Switch block size.
        rows = {key: dict(cells) for key, cells in self.rows.items()}
        rows["DXT1"]["gates"] = rows["DXT1"]["gates"].replace("sw 8×4", "sw —")
        self.assertEqual(matrix_errors(rows, self.expected), [("DXT1", "gates")])

    def test_pixels_match_the_matrix(self):
        """Each `works` row decodes to the layout's reference, each `wrong` row to its stated wrong pixels."""
        for fmt in TextureFormat:
            row = self.rows[fmt.name]
            result = RESULT.match(row["UnityPy 1.25.2"]).group(1)
            with self.subTest(fmt.name, result=result):
                if result == "raises":
                    self.assertNotIn(fmt.name, PIXEL_CASES)
                    continue
                self.assertIn(base_name(fmt.name), PIXEL_CASES, "every format that returns an image has a pixel case")
                case = PIXEL_CASES[base_name(fmt.name)][1]()
                got = self.decode_case(fmt, case.data)
                self.check_case(result, case, got, row["mode"])

    def decode_case(self, fmt, data, platform=None, blob=None):
        """Decode `data`; for a crunched format, the unpacker hands it on as the base format's data."""
        calls, patches = stand_in_unpackers()
        for patch in patches:
            patch.start()
        try:
            image = decode(data, fmt, platform=platform, blob=blob)
        finally:
            for patch in reversed(patches):
                patch.stop()
        return image.mode, list(image.getdata())

    def check_case(self, result, case, got, mode):
        if result in ("works", "crunch"):
            self.assertIsNone(case.unitypy, "a row with a stated wrong result is not `works`")
            self.assertEqual(case.mode, mode)
            self.assertEqual(got, (case.mode, case.pixels))
        elif result == "wrong":
            self.assertIsNotNone(case.pixels, "`wrong` needs a reference to be wrong against")
            self.assertEqual(case.unitypy[0], mode)
            self.assertEqual(got, case.unitypy)
            self.assertNotEqual(got, (case.mode, case.pixels))
        else:
            self.assertIsNone(case.pixels, "an `unverified` row has no reference")
            self.assertEqual(case.unitypy[0], mode)
            self.assertEqual(got, case.unitypy)

    def test_switch_and_xbox_data_decode_as_the_gates_say(self):
        """Each format with a pixel case, its data swizzled with the gates cell's block size (Switch) or
        byte-swapped (XBOX360), decodes on that platform to what it decodes to elsewhere."""
        for fmt in TextureFormat:
            row = self.rows[fmt.name]
            result = RESULT.match(row["UnityPy 1.25.2"]).group(1)
            if result == "raises":
                continue
            gates = row["gates"]
            sw = re.search(r"sw (\d+)×(\d+)(?:, read as `(\w+)`)?", gates)
            with self.subTest(fmt.name, platform="Switch"):
                if sw is None:
                    self.assertIn("sw —", gates)
                    with self.assertRaises(KeyError):
                        decode(real_sized(fmt), fmt, platform=BuildTarget.Switch, blob=SWITCH_BLOB)
                else:
                    read_as = TextureFormat[sw.group(3)] if sw.group(3) else fmt
                    case = PIXEL_CASES[read_as.name][1]()
                    swizzled = swizzle(case.data, read_as, int(sw.group(1)), int(sw.group(2)))
                    got = self.decode_case(fmt, swizzled, BuildTarget.Switch, SWITCH_BLOB)
                    read_as_result = RESULT.match(self.rows[read_as.name]["UnityPy 1.25.2"]).group(1)
                    self.check_case(read_as_result, case, got, self.rows[read_as.name]["mode"])
            with self.subTest(fmt.name, platform="XBOX360"):
                case = PIXEL_CASES[base_name(fmt.name)][1]()
                data = case.data
                if "xbox" in gates.split("; "):
                    data = bytes(b for i in range(0, len(data), 2) for b in data[i : i + 2][::-1])
                got = self.decode_case(fmt, data, BuildTarget.XBOX360)
                self.check_case(result, case, got, row["mode"])

    def test_half_values_outside_a_byte_raise(self):
        """RHalf and RGBAHalf: what RESULT_NOTES and RAISE_REASONS say beyond the 0x30 run."""

        def halves(fmt, values, pixels=W * H):
            per_pixel = {"RHalf": 1, "RGBAHalf": 4}[fmt.name]
            data = struct.pack(f"<{pixels * per_pixel}e", *(values * (pixels * per_pixel))[: pixels * per_pixel])
            return decode(data, fmt)

        for fmt in (TextureFormat.RHalf, TextureFormat.RGBAHalf):
            # Three times RHalf's data reaches Pillow's missing mode R; RGBAHalf has enough of its own.
            enough = 3 * W * H if fmt == TextureFormat.RHalf else W * H
            for value, error in ((1.0, ValueError), (-1 / 256, ValueError), (float("nan"), ValueError)):
                with self.subTest(fmt.name, value=value), self.assertRaises(error):
                    halves(fmt, [value], enough)
            with self.subTest(fmt.name, value="inf"), self.assertRaises(OverflowError):
                halves(fmt, [float("inf")], enough)
            for value in (255 / 256, -1 / 512):  # int(h * 256) still 0-255
                with self.subTest(fmt.name, value=value):
                    if fmt == TextureFormat.RHalf:
                        with self.assertRaisesRegex(ValueError, "^unrecognized image mode$"):
                            halves(fmt, [value], enough)
                    else:
                        self.assertEqual(halves(fmt, [value]).mode, "RGB")
        # Real-sized RHalf data with a 1.0 first: the ValueError comes before the data runs out.
        with self.assertRaises(ValueError):
            halves(TextureFormat.RHalf, [1.0])

    def test_array_8bit_targets_have_references(self):
        """Section 19.2's 8-bit integer arrays decode as the 8-bit format UnityPy maps the UNorm or SNorm graphics
        format of the same channels to, and each of those is Tier A or has a section 17.1 reference."""
        from UnityPy.enums.GraphicsFormat import GRAPHICS_TO_TEXTURE_MAP, GraphicsFormat

        targets = {}
        for graphics in GraphicsFormat:
            if graphics.name.startswith("R8") and graphics.name.endswith(("_UInt", "_SInt")):
                norm = graphics.name.replace("_UInt", "_UNorm").replace("_SInt", "_SNorm")
                targets[graphics.name] = GRAPHICS_TO_TEXTURE_MAP[GraphicsFormat[norm]].name
        self.assertEqual(
            targets,
            {
                "R8_UInt": "R8",
                "R8G8_UInt": "RG16",
                "R8G8B8_UInt": "RGB24",
                "R8G8B8A8_UInt": "RGBA32",
                "R8_SInt": "R8_SIGNED",
                "R8G8_SInt": "RG16_SIGNED",
                "R8G8B8_SInt": "RGB24_SIGNED",
                "R8G8B8A8_SInt": "RGBA32_SIGNED",
            },
        )
        for target in targets.values():
            with self.subTest(target):
                row = self.rows[target]
                self.assertTrue(row["tier"] == "A" or target in references.TIER_B, row["tier"])

    def test_rgb48_reference_is_the_high_byte(self):
        """Section 19.2's RGB48 reference: each channel's high byte, what Pillow's `RGB;16L` gives."""
        case = rgb48()
        image = Image.frombytes("RGB", (W, H), case.data, "raw", "RGB;16L").transpose(Image.Transpose.FLIP_TOP_BOTTOM)
        self.assertEqual(list(image.getdata()), case.pixels)
        rounded = [tuple(round(v * 255 / 65535) for v in pixel) for pixel in RGB48_PIXELS]
        self.assertNotEqual(rounded, [tuple(v >> 8 for v in pixel) for pixel in RGB48_PIXELS])


class TierBReferences(unittest.TestCase):
    """Section 17.1's references hold the values it states (standard library; the BC6H libraries need the oracle)."""

    def one(self, name, data, width=1, height=1):
        mode, pixels = references.decode(name, data, width, height)
        return mode, pixels[0] if width * height == 1 else pixels

    def test_tier_b_references_hold_their_stated_values(self):
        # Half and float: clamped to [0, 1], times 255, rounded half up; NaN gives 0.
        cases = {0.5: 128, 1.0: 255, 2.0: 255, float("inf"): 255, -1.0: 0, -0.0: 0, float("nan"): 0, 1 / 255: 1}
        for value, byte in cases.items():
            with self.subTest(value=value):
                self.assertEqual(self.one("RHalf", struct.pack("<e", value)), ("RGB", (byte, 0, 0)))
                self.assertEqual(self.one("RFloat", struct.pack("<f", value)), ("RGB", (byte, 0, 0)))
        self.assertEqual(self.one("RGHalf", struct.pack("<2e", 0.25, 0.75)), ("RGB", (64, 191, 0)))
        self.assertEqual(self.one("RGBFloat", struct.pack("<3f", 0, 0.5, 1)), ("RGB", (0, 128, 255)))
        self.assertEqual(self.one("RGBAFloat", struct.pack("<4f", 1, 0, 0.5, 0.25)), ("RGBA", (255, 0, 128, 64)))
        # Unsigned: the high byte. Signed: offset by 2^(n-1), then the high byte.
        self.assertEqual(self.one("R16", struct.pack("<H", 0xFF00)), ("RGB", (255, 0, 0)))
        self.assertEqual(self.one("R16", struct.pack("<H", 0x00FF)), ("RGB", (0, 0, 0)))
        self.assertEqual(self.one("RG16", bytes([7, 200])), ("RGB", (7, 200, 0)))
        self.assertEqual(
            self.one("RGBA64", struct.pack("<4H", 0xFFFF, 0x8000, 0x7FFF, 0)), ("RGBA", (255, 128, 127, 0))
        )
        self.assertEqual(self.one("RGB24_SIGNED", struct.pack("<3b", -128, 0, 127)), ("RGB", (0, 128, 255)))
        self.assertEqual(self.one("RGBA32_SIGNED", struct.pack("<4b", -1, 1, -127, 64)), ("RGBA", (127, 129, 1, 192)))
        self.assertEqual(self.one("R8_SIGNED", struct.pack("<b", -128)), ("RGB", (0, 0, 0)))
        self.assertEqual(self.one("RG16_SIGNED", struct.pack("<2b", 0, 127)), ("RGB", (128, 255, 0)))
        self.assertEqual(self.one("RGB48_SIGNED", struct.pack("<3h", -32768, 0, 32767)), ("RGB", (0, 128, 255)))
        self.assertEqual(self.one("R16_SIGNED", struct.pack("<h", -1)), ("RGB", (127, 0, 0)))
        self.assertEqual(self.one("RG32_SIGNED", struct.pack("<2h", 256, -256)), ("RGB", (129, 127, 0)))
        self.assertEqual(self.one("RG32", struct.pack("<2H", 0x1234, 0xABCD)), ("RGB", (0x12, 0xAB, 0)))
        self.assertEqual(self.one("RGBA64_SIGNED", struct.pack("<4h", 0, 0, 0, 32767)), ("RGBA", (128, 128, 128, 255)))
        # RGB9e5Float: exponent 15, red 256: 256 * 2^(15 - 24) = 0.5.
        self.assertEqual(self.one("RGB9e5Float", struct.pack("<I", 15 << 27 | 256)), ("RGB", (128, 0, 0)))
        self.assertEqual(references.rgb9e5_to_floats(16 << 27 | 256 << 18 | 128 << 9 | 1), (2**-8, 0.5, 1.0))
        # YUY2: BT.601 limited range; one pair of pixels shares U and V.
        pair = self.one("YUY2", bytes([16, 128, 235, 128]), width=2)
        self.assertEqual(pair, ("RGB", [(0, 0, 0), (255, 255, 255)]))
        self.assertEqual(references.yuv_to_rgb(81, 90, 240), (255, 0, 0))  # BT.601 red
        with self.assertRaises(NotImplementedError):
            references.decode("YUY2", bytes(4), 1, 1)
        with self.assertRaises(NotImplementedError):
            references.decode("ARGBFloat", bytes(16), 1, 1)
        # Every image comes back flipped top to bottom.
        mode, pixels = references.decode("R16", struct.pack("<2H", 0x0100, 0x0200), 1, 2)
        self.assertEqual(pixels, [(2, 0, 0), (1, 0, 0)])

    def test_short_data_raises_for_every_reference(self):
        # What a texture holds: its pixels, or BC6H's whole 4x4 blocks.
        self.assertEqual(references.needed_bytes("RGBAFloat", 3, 2), 96)
        self.assertEqual(references.needed_bytes("YUY2", 4, 2), 16)
        self.assertEqual(references.needed_bytes("RGB9e5Float", 2, 2), 16)
        self.assertEqual(references.needed_bytes("BC6H", 5, 4), 32)
        for name in sorted(references.TIER_B):
            width, height = (4, 2) if name == "YUY2" else (5, 3)  # an odd width is Tier U for YUY2
            needed = references.needed_bytes(name, width, height)
            with self.subTest(name):
                # Exactly enough decodes, and bytes past that are ignored.
                full = references.decode(name, bytes(needed), width, height)
                self.assertEqual(len(full[1]), width * height)
                self.assertEqual(references.decode(name, bytes(needed + 16), width, height), full)
                # One byte short, or none: an error, never pixels for bytes that do not exist.
                for short in (needed - 1, 0):
                    with self.assertRaisesRegex(ValueError, "^not enough image data"):
                        references.decode(name, bytes(short), width, height)

    def test_bc6h_reference_is_the_specification(self):
        """BC6H's reference decodes as the Khronos Data Format Specification 1.3, section 20.2, says."""

        def block(e0, e1, index=0):
            # Mode 3 (mode bits 00011): one subset, 10-bit endpoints stored as they are; texel 0, the anchor, keeps
            # 3 index bits and the others 4, and every texel takes `index`.
            bits, shift = 0b00011, 5
            for channel in (*e0, *e1):
                bits |= channel << shift
                shift += 10
            bits |= (index & 7) << shift
            for texel in range(1, 16):
                bits |= index << (shift + 3 + 4 * (texel - 1))
            return bits.to_bytes(16, "little")

        def half(endpoint):  # the unsigned unquantize of a 10-bit endpoint, and the final 31/64
            return ((((endpoint << 16) + 0x8000) >> 10) * 31) >> 6

        self.assertEqual([hex(half(e)) for e in (400, 488, 500)], ["0x307f", "0x3b27", "0x3c9b"])
        mode, pixels = references.decode("BC6H", block((400, 488, 500), (0, 0, 0)), 4, 4)
        self.assertEqual((mode, set(pixels)), ("RGB", {(36, 228, 255)}))
        # Cropped from whole 4x4 blocks, as UnityPy pads every BC format.
        self.assertEqual(
            references.decode("BC6H", block((400, 488, 500), (0, 0, 0)), 3, 2), ("RGB", [(36, 228, 255)] * 6)
        )

        # An endpoint equal to 2^(bits-1) unquantizes by the formula: 512 of 10 bits to 32800, not 0xFFFF. With
        # weight 34 (index 8) towards 0: (32800 * 30 + 32) >> 6 = 15375, times 31/64 = 0x1d17, 0.00477, red 1.
        self.assertEqual(references.bc6h_unquantize(512, 10), 32800)
        self.assertEqual(references.bc6h_unquantize(1023, 10), 0xFFFF)
        reviewer = block((512, 100, 100), (0, 0, 0), 8)
        texels = references.bc6h_block(reviewer)
        self.assertEqual(texels[1:], [(0x1D17, 0x05B4, 0x05B4)] * 15)
        self.assertEqual(texels[0], (0x3E0F, 0x0C2B, 0x0C2B))  # the anchor's 3 bits hold 0: endpoint 0
        mode, pixels = references.decode("BC6H", reviewer, 4, 4)
        self.assertEqual(pixels[12:], [(255, 0, 0), (1, 0, 0), (1, 0, 0), (1, 0, 0)])  # flipped: texel 0 is last
        # Equation 2 rounds (+ 32): weight 4 (index 1) from 0 towards 1023 (0xFFFF) gives (0xFFFF * 4 + 32) >> 6
        # = 4096, times 31/64 = 0x7c0; without the + 32, 4095 and 0x7bf.
        ramp = block((0, 0, 0), (1023, 1023, 1023), 1)
        self.assertEqual(references.bc6h_block(ramp)[1], (0x7C0,) * 3)
        self.assertEqual(references.bc6h_block(ramp, rounding=0)[1], (0x7BF,) * 3)

    def test_bc6h_libraries_depart_from_the_specification(self):
        """Section 19.2: texture2ddecoder 1.0.5 and Pillow 9.5.0 each equal section 20.2's decoding with one
        departure, on random blocks of every mode, and the departure shows on the reviewer's block."""
        if not ORACLE:
            self.skipTest("needs texture2ddecoder 1.0.5 and Pillow 9.5.0 (the oracle image)")

        def bimg_unquantize(value, bits):  # texture2ddecoder's src/Texture2DDecoder/bcn.cpp, unsigned
            if bits >= 15:
                return value
            if value == 0:
                return 0
            if value == 1 << (bits - 1):  # section 20.2 tests (1 << bits) - 1
                return 0xFFFF
            return ((value << 15) + 0x4000) >> (bits - 1)

        def as_bytes(texels, convert):
            return [tuple(convert(references.half_bits_to_float(h)) for h in texel) for texel in texels]

        def truncate(value):  # Pillow's bc6_clamp: clamped to [0, 1], then (UINT8)(value * 255.0f)
            return 255 if value > 1 else math.floor(value * 255)

        def t2d(data):
            bgra = texture2ddecoder.decode_bc6(data, 4, 4)
            return [(bgra[i + 2], bgra[i + 1], bgra[i]) for i in range(0, 64, 4)]

        def pillow(data):
            return list(Image.frombytes("RGB", (4, 4), data, "bcn", 6).getdata())

        rng = random.Random(29)
        modes = sorted(references.BC6H_MODES) + [19, 23, 27, 31]  # the last four are reserved
        differs = {"texture2ddecoder": 0, "Pillow": 0}
        for mode in modes:
            for _ in range(60):
                width = 2 if mode < 2 else 5
                data = (rng.getrandbits(128) >> width << width | mode).to_bytes(16, "little")
                spec = as_bytes(references.bc6h_block(data), references.float_to_byte)
                with self.subTest(mode=mode, data=data.hex()):
                    self.assertEqual(
                        t2d(data), as_bytes(references.bc6h_block(data, bimg_unquantize), references.float_to_byte)
                    )
                    self.assertEqual(pillow(data), as_bytes(references.bc6h_block(data, rounding=0), truncate))
                differs["texture2ddecoder"] += t2d(data) != spec
                differs["Pillow"] += pillow(data) != spec
        self.assertTrue(all(differs.values()), differs)
        # The reviewer's block: red endpoint 0 at 512 of 10 bits, endpoint 1 at 0, index 8 (texel 1 onwards).
        bits = 0b00011 | 512 << 5 | 100 << 15 | 100 << 25
        for texel in range(1, 16):
            bits |= 8 << (65 + 3 + 4 * (texel - 1))
        data = bits.to_bytes(16, "little")
        self.assertEqual(as_bytes(references.bc6h_block(data), references.float_to_byte)[1], (1, 0, 0))
        self.assertEqual(t2d(data)[1], (195, 0, 0))
        self.assertEqual(pillow(data)[1], (1, 0, 0))


def swizzle(data, fmt, block_w, block_h):
    """`data`, an 8x8 texture of `fmt`, laid out at the size the Switch path pads to and swizzled."""
    unit_w, unit_h, size = unit(fmt)
    padded_w, padded_h = TextureSwizzler.get_padded_texture_size(W, H, block_w, block_h, GOBS)
    across, down = -(-W // unit_w), -(-H // unit_h)
    padded_across, padded_down = padded_w // unit_w, padded_h // unit_h
    linear = bytearray(padded_across * padded_down * size)
    for row in range(down):
        source = data[row * across * size : (row + 1) * across * size]
        linear[row * padded_across * size : row * padded_across * size + len(source)] = source
    return bytes(TextureSwizzler.swizzle(linear, padded_w, padded_h, block_w, block_h, GOBS))


if __name__ == "__main__":
    unittest.main()
