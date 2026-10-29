"""The TextureFormat matrix of PARITY.md section 17, against the oracle.

`ThisParityFile` reads the matrix with the standard library alone. `AgainstTheOracle`
needs UnityPy 1.25.2 (the oracle image) and is skipped elsewhere: it runs every
TextureFormat through `parse_image_data` and `image_to_texture2d` and checks what the
matrix says UnityPy returns.
"""

import re
import sys
import unittest
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
                result = re.match(r"(works|wrong|raises|crunch)\b", row["UnityPy 1.25.2"])
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


if __name__ == "__main__":
    unittest.main()
