"""The TextureFormat matrix of PARITY.md section 17, against the oracle.

`ThisParityFile` reads the matrix with the standard library alone. `AgainstTheOracle`
needs UnityPy 1.25.2 (the oracle image) and is skipped elsewhere: it runs every
TextureFormat through `parse_image_data` and `image_to_texture2d` and checks what the
matrix says UnityPy returns, down to the pixels.

The pixel cases are built here from each format's layout, not from UnityPy: raw
formats from a byte pattern, block formats from solid-colour blocks whose decoded
colour the format's specification fixes (one colour per block, so block order and
UnityPy's vertical flip show too). A row marked `works` must decode to that
reference; a row marked `wrong` must decode to the wrong pixels the row states and
not to the reference; a row marked `unverified` has no reference, and its pixels are
pinned as UnityPy returns them.
"""

import re
import struct
import sys
import unittest
from collections import namedtuple
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import inventory  # noqa: E402

try:
    import UnityPy
    from PIL import Image
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.export import Texture2DConverter

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False

PARITY = inventory.DEFAULT_PARITY
NEEDS_ORACLE = "needs UnityPy 1.25.2 (the oracle image)"

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
EIGHT_BYTE_BLOCKS = set(
    "DXT1 BC4 ETC_RGB4 ETC_RGB4_3DS ETC2_RGB ETC2_RGBA1 EAC_R EAC_R_SIGNED ATC_RGB4"
    " PVRTC_RGB2 PVRTC_RGBA2 PVRTC_RGB4 PVRTC_RGBA4".split()
)


def stored_size(fmt, width, height):
    """The bytes a real texture of `fmt` holds for its top level: what `parse_image_data` gets from Unity."""
    if fmt.name in BYTES_PER_PIXEL:
        return BYTES_PER_PIXEL[fmt.name] * width * height
    # UnityPy's own block table; ATC is absent from it (it pads nothing) and has 4x4 blocks.
    block_w, block_h = Texture2DConverter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE[fmt] or (4, 4)
    blocks = -(-width // block_w) * -(-height // block_h)
    return blocks * (8 if fmt.name in EIGHT_BYTE_BLOCKS else 16)


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
        self.rows = inventory.texture_rows(PARITY.read_text(encoding="utf-8"))

    def test_the_matrix_has_one_row_per_texture_format(self):
        self.assertEqual(len(self.rows), 78)
        self.assertEqual(len({row["value"] for row in self.rows.values()}), 78)

    def test_tier_b_has_the_22_formats_design_names(self):
        tier_b = sorted(name for name, row in self.rows.items() if row["tier"] == "B")
        signed = [name for name in self.rows if name.endswith("_SIGNED") and not name.startswith("EAC")]
        expected = ["BC6H", "RHalf", "RGHalf", "RFloat", "RGFloat", "RGBAFloat", "ARGBFloat", "RGBFloat"]
        expected += signed + ["RG16", "RG32", "R16", "RGBA64", "RGB9e5Float", "YUY2"]
        self.assertEqual(len(signed), 8)
        self.assertEqual(tier_b, sorted(expected))
        self.assertEqual(len(tier_b), 22)

    def test_p1_formats_are_the_five_design_names(self):
        p1 = sorted(name for name, row in self.rows.items() if row["status"] == "planned (P1)")
        self.assertEqual(p1, ["BC7", "DXT1", "DXT5", "RGB24", "RGBA32"])



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
    reference = flipped(lambda x, y: tuple(round(h * 255) for h in halves[4 * (y * W + x) :][:4]))
    # Three halves read per pixel of four stored, each int(h * 256): alpha dropped, pixels misaligned.
    unitypy = flipped(lambda x, y: tuple(int(h * 256) for h in halves[3 * (y * W + x) :][:3]))
    return Case(data, "RGBA", reference, ("RGB", unitypy))


def rgb48():
    data = struct.pack("<3H", 0xFFFF, 0, 0) * (W * H)  # pure red, 16 bits a channel
    reference = flipped(lambda x, y: (255, 0, 0))
    # Pillow's RGB;16 is packed 5-6-5, two bytes a pixel: the u16 stream FFFF 0000 0000 ... gives
    # white, black, black, white, ... in data order.
    unitypy = flipped(lambda x, y: (255, 255, 255) if (y * W + x) % 3 == 0 else (0, 0, 0))
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


PIXEL_CASES = {
    "Alpha8": lambda: raw(1, "RGBA", lambda b: (0, 0, 0, b[0])),
    "ARGB4444": lambda: raw(2, "RGBA", lambda b: nibbles(u16(b), 8, 4, 0, 12)),
    "RGB24": lambda: raw(3, "RGB", tuple),
    "RGBA32": lambda: raw(4, "RGBA", tuple),
    "ARGB32": lambda: raw(4, "RGBA", lambda b: (b[1], b[2], b[3], b[0])),
    # Each field widened as v * 255 / 31 (green / 63), truncated: red 16 (the first pixel) gives 131.
    "RGB565": lambda: raw(
        2,
        "RGB",
        lambda b: ((u16(b) >> 11) * 255 // 31, (u16(b) >> 5 & 63) * 255 // 63, (u16(b) & 31) * 255 // 31),
        start=b"\x00\x80",
    ),
    "BGR24": lambda: raw(3, "RGB", lambda b: (b[2], b[1], b[0])),
    "RGBA4444": lambda: raw(2, "RGBA", lambda b: nibbles(u16(b), 12, 8, 4, 0)),
    "BGRA32": lambda: raw(4, "RGBA", lambda b: (b[2], b[1], b[0], b[3])),
    "R8": lambda: raw(1, "RGB", lambda b: (b[0], 0, 0)),
    "RGBAHalf": rgba_half,
    "RGB48": rgb48,
    "DXT1": lambda: blocks(
        4, 4, "RGBA", [(bc1(c), px) for c, px in RGB565_SOLIDS] + [(bc1(0, 0xFFFF, 3), (0, 0, 0, 0))], 8
    ),
    "DXT3": lambda: blocks(
        4,
        4,
        "RGBA",
        [
            (bytes([a << 4 | a]) * 8 + bc1(c), px[:3] + (a * 17,))
            for a, (c, px) in zip((8, 15, 0, 4), RGB565_SOLIDS + [(0xFFFF, (255, 255, 255, 255))], strict=True)
        ],
        16,
    ),
    "DXT5": lambda: blocks(
        4,
        4,
        "RGBA",
        [
            (bc4(a) + bc1(c), px[:3] + (a,))
            for a, (c, px) in zip((200, 100, 0, 255), RGB565_SOLIDS + [(0xFFFF, (255, 255, 255, 255))], strict=True)
        ],
        16,
    ),
    "BC4": lambda: blocks(4, 4, "L", [(bc4(v), v) for v in (0x10, 0x80, 0xC0, 0xFF)], 8),
    "BC5": lambda: blocks(4, 4, "RGB", [(bc4(r) + bc4(g), (r, g, 0)) for r, g in BC5_PAIRS], 16),
    "BC7": lambda: blocks(4, 4, "RGBA", [(bc7_mode6(*c), c) for c in BC7_COLOURS], 16),
    "PVRTC_RGB2": lambda: blocks(8, 4, "RGBA", [pvrtc(31, 16, 8)], 8),
    "PVRTC_RGBA2": lambda: blocks(8, 4, "RGBA", [pvrtc(31, 16, 8)], 8),
    "PVRTC_RGB4": lambda: blocks(4, 4, "RGBA", [pvrtc(31, 16, 8)], 8),
    "PVRTC_RGBA4": lambda: blocks(4, 4, "RGBA", [pvrtc(31, 16, 8)], 8),
    "ETC_RGB4": lambda: blocks(4, 4, "RGBA", ETC1_SOLIDS, 8),
    "ETC2_RGB": lambda: blocks(4, 4, "RGBA", ETC1_SOLIDS, 8),
    "ETC2_RGBA1": etc2_rgba1,
    "ETC2_RGBA8": lambda: blocks(
        4,
        4,
        "RGBA",
        [(eac(a) + block, px[:3] + (a,)) for a, (block, px) in zip((200, 100, 0, 255), ETC1_SOLIDS, strict=True)],
        16,
    ),
    "EAC_R": lambda: blocks(4, 4, "RGBA", [(eac(v), (v, 0, 0, 255)) for v in EAC_BASES], 8),
    "EAC_R_SIGNED": lambda: blocks(
        4, 4, "RGBA", [(eac(v), (eac_signed_value(v), 0, 0, 255)) for v in EAC_SIGNED_BASES], 8
    ),
    "EAC_RG": lambda: blocks(
        4, 4, "RGBA", [(eac(r) + eac(g), (r, g, 0, 255)) for r, g in zip(EAC_BASES, reversed(EAC_BASES))], 16
    ),
    "EAC_RG_SIGNED": lambda: blocks(
        4,
        4,
        "RGBA",
        [
            (eac(r) + eac(g), (eac_signed_value(r), eac_signed_value(g), 0, 255))
            for r, g in zip(EAC_SIGNED_BASES, reversed(EAC_SIGNED_BASES))
        ],
        16,
    ),
    "ATC_RGB4": lambda: blocks(4, 4, "RGBA", [(atc(c), px) for c, px in ATC_SOLIDS], 8),
    "ATC_RGBA8": lambda: blocks(
        4,
        4,
        "RGBA",
        [(bc4(a) + atc(c), px[:3] + (a,)) for a, (c, px) in zip((200, 0, 255, 64), ATC_SOLIDS, strict=True)],
        16,
    ),
    **{f"ASTC_RGB_{n}x{n}": (lambda n=n: astc(n)) for n in (4, 5, 6, 8, 10, 12)},
    **{f"ASTC_RGBA_{n}x{n}": (lambda n=n: astc(n)) for n in (4, 5, 6, 8, 10, 12)},
    **{f"ASTC_HDR_{n}x{n}": (lambda n=n: astc_hdr(n)) for n in (4, 5, 6, 8, 10, 12)},
    "ETC_RGB4_3DS": etc_rgb4_3ds,
    "ETC_RGBA8_3DS": etc_rgba8_3ds,
}


@unittest.skipUnless(ORACLE, NEEDS_ORACLE)
class AgainstTheOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = PARITY.read_text(encoding="utf-8")

    def test_texture_matrix_matches_the_oracle(self):
        """Each row's mode, UnityPy and `set_image` cells, on 8x8 of 0x30 bytes, real-sized (section 17)."""
        rows = inventory.texture_rows(self.text)
        source = Image.new("RGBA", (8, 8), (10, 20, 30, 40))
        for fmt in TextureFormat:
            row = rows[fmt.name]
            with self.subTest(fmt.name):
                result = re.match(r"(works|wrong|unverified|raises|crunch)\b", row["UnityPy 1.25.2"])
                self.assertIsNotNone(result, row["UnityPy 1.25.2"])
                if result.group(1) != "crunch":
                    # As many bytes as a real 8x8 texture holds: a converter that reads past them
                    # (RHalf reads three halves a pixel) fails as it does on real data.
                    data = bytes([0x30]) * stored_size(fmt, 8, 8)
                    try:
                        image = Texture2DConverter.parse_image_data(
                            data, 8, 8, fmt, (2021, 3, 1, 3), BuildTarget.StandaloneWindows64
                        )
                        got = ("image", image.mode, image.size)
                    except Exception as error:  # noqa: BLE001 - the exception type is the result
                        got = ("raises", exception_name(error))
                    if result.group(1) == "raises":
                        self.assertEqual(got, ("raises", first_span(row["UnityPy 1.25.2"])))
                    else:
                        self.assertEqual(got, ("image", row["mode"], (8, 8)))

                cell = row["`set_image` writes"]
                try:
                    written = ("writes", Texture2DConverter.image_to_texture2d(source, fmt)[1].name)
                except Exception as error:  # noqa: BLE001
                    written = ("raises", exception_name(error))
                self.assertEqual(written, ("raises" if cell.startswith("raises") else "writes", first_span(cell)))

    def test_pixels_match_the_matrix(self):
        """Each `works` row decodes to the layout's reference, each `wrong` row to its stated wrong pixels."""
        rows = inventory.texture_rows(self.text)
        for fmt in TextureFormat:
            row = rows[fmt.name]
            result = re.match(r"(works|wrong|unverified|raises|crunch)\b", row["UnityPy 1.25.2"]).group(1)
            with self.subTest(fmt.name, result=result):
                if result in ("raises", "crunch"):
                    self.assertNotIn(fmt.name, PIXEL_CASES)
                    continue
                self.assertIn(fmt.name, PIXEL_CASES, "every format that returns an image has a pixel case")
                case = PIXEL_CASES[fmt.name]()
                image = Texture2DConverter.parse_image_data(
                    case.data, W, H, fmt, (2021, 3, 1, 3), BuildTarget.StandaloneWindows64
                )
                got = (image.mode, list(image.getdata()))
                if result == "works":
                    self.assertIsNone(case.unitypy, "a row with a stated wrong result is not `works`")
                    self.assertEqual(case.mode, row["mode"])
                    self.assertEqual(got, (case.mode, case.pixels))
                elif result == "wrong":
                    self.assertIsNotNone(case.pixels, "`wrong` needs a reference to be wrong against")
                    self.assertEqual(case.unitypy[0], row["mode"])
                    self.assertEqual(got, case.unitypy)
                    self.assertNotEqual(got, (case.mode, case.pixels))
                else:
                    self.assertIsNone(case.pixels, "an `unverified` row has no reference")
                    self.assertEqual(case.unitypy[0], row["mode"])
                    self.assertEqual(got, case.unitypy)


if __name__ == "__main__":
    unittest.main()
