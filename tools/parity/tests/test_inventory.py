"""The PARITY.md inventory: the checker's matching rules, and PARITY.md against the oracle.

The rule tests need only the standard library. The others need UnityPy 1.25.2 (the
oracle image) and are skipped elsewhere: they list UnityPy's surface, require
PARITY.md to name all of it, and run every TextureFormat through UnityPy to check
what the matrix says UnityPy does.
"""

import contextlib
import io
import re
import sys
import tempfile
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

SYMBOLS = {
    "modules": ["files/BundleFile.py", "export/Texture2DConverter.py"],
    "classes": ["BundleFile", "SpriteSettings"],
    "functions": ["ImportHelper.check_file_type"],
    "methods": ["Environment.load_file", "Texture2D.set_image"],
    "boost": ["UnityPyBoost.read_typetree"],
    "texture_formats": [["Alpha8", 1], ["RGB24", 3]],
}

COMPLETE = """
| item | note |
|---|---|
| `UnityPy/files/BundleFile.py:188-236` | a path, with its package prefix and a line range |
| `export/Texture2DConverter.py` | a bare path |
| `BundleFile` | a class |
| `SpriteHelper.SpriteSettings` | a class inside a dotted name |
| `UnityPy.helpers.ImportHelper.check_file_type(path)` | a function inside a longer chain and a call |
| `Environment.load_file`, `Texture2D.set_image` | two methods in one cell |
| `UnityPyBoost.read_typetree` | a Boost export |

| value | format | result |
|---|---|---|
| 1 | `Alpha8` | works |
| 3 | `RGB24` | works |
"""


def first_span(cell):
    match = re.search(r"`([^`]+)`", cell)
    return match.group(1) if match else None


class MatchingRules(unittest.TestCase):
    def test_a_complete_document_misses_nothing(self):
        self.assertEqual(inventory.missing(SYMBOLS, COMPLETE), {})

    def test_names_count_only_inside_code_spans(self):
        text = COMPLETE.replace("`SpriteHelper.SpriteSettings`", "SpriteHelper.SpriteSettings")
        self.assertEqual(inventory.missing(SYMBOLS, text), {"classes": ["SpriteSettings"]})

    def test_a_path_names_its_module_and_nothing_else(self):
        text = COMPLETE.replace("| `BundleFile` |", "| the bundle class |")
        self.assertEqual(inventory.missing(SYMBOLS, text), {"classes": ["BundleFile"]})

    def test_a_bare_name_does_not_stand_for_a_dotted_one(self):
        text = COMPLETE.replace("`UnityPy.helpers.ImportHelper.check_file_type(path)`", "`check_file_type`")
        self.assertEqual(inventory.missing(SYMBOLS, text), {"functions": ["ImportHelper.check_file_type"]})

    def test_a_module_needs_its_whole_path(self):
        text = COMPLETE.replace("`export/Texture2DConverter.py`", "`Texture2DConverter.py`")
        self.assertEqual(inventory.missing(SYMBOLS, text), {"modules": ["export/Texture2DConverter.py"]})

    def test_a_texture_format_needs_a_row_with_its_name_and_value(self):
        for text in (
            COMPLETE.replace("| 3 | `RGB24` |", "| 4 | `RGB24` |"),
            COMPLETE.replace("| 3 | `RGB24` |", "| 3 | RGB24 |"),
            COMPLETE.replace("| 3 | `RGB24` | works |", "") + "\n`RGB24` is format 3.\n",
        ):
            with self.subTest(text=text[-60:]):
                self.assertEqual(inventory.missing(SYMBOLS, text), {"texture_formats": ["RGB24 (3)"]})

    def test_an_escaped_pipe_stays_inside_its_cell(self):
        self.assertEqual(inventory.table_cells(r"| `a \| b` | c |"), [r"`a \| b`", "c"])
        self.assertIsNone(inventory.table_cells("a | b"))

    def test_texture_rows_reads_only_the_matrix(self):
        rows = inventory.texture_rows(COMPLETE)
        self.assertEqual(sorted(rows), ["Alpha8", "RGB24"])
        self.assertEqual(rows["RGB24"], {"value": "3", "format": "`RGB24`", "result": "works"})


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
        cls.symbols = inventory.collect()
        cls.text = PARITY.read_text(encoding="utf-8")

    def test_collect_finds_the_surface(self):
        groups = self.symbols
        self.assertEqual(len(groups["texture_formats"]), 78)
        self.assertIn("export/Texture2DConverter.py", groups["modules"])
        self.assertIn("classes/legacy_patch/Texture2D.py", groups["modules"])
        self.assertIn("Texture2DArray", groups["classes"])
        self.assertIn("Texture2DConverter.parse_image_data", groups["functions"])
        self.assertIn("Environment.load_file", groups["methods"])
        self.assertIn("Texture2D.set_image", groups["methods"])
        self.assertIn("UnityPyBoost.read_typetree", groups["boost"])

    def test_parity_md_names_every_symbol(self):
        self.assertEqual(inventory.missing(self.symbols, self.text), {})

    def test_check_exits_1_and_lists_what_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "PARITY.md"
            path.write_text(self.text.replace("`Texture2DConverter.parse_image_data`", "parse_image_data"))
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                status = inventory.main(["check", str(path)])
        self.assertEqual(status, 1)
        self.assertIn("functions: 1 missing\n  Texture2DConverter.parse_image_data\n", stderr.getvalue())

    def test_texture_matrix_matches_the_oracle(self):
        """Each row's mode, UnityPy and `set_image` cells, on 8x8 of 0x30 bytes (section 17)."""
        rows = inventory.texture_rows(self.text)
        data = bytes([0x30]) * 8192
        source = Image.new("RGBA", (8, 8), (10, 20, 30, 40))
        for fmt in TextureFormat:
            row = rows[fmt.name]
            with self.subTest(fmt.name):
                result = re.match(r"(works|wrong|raises|crunch)\b", row["UnityPy 1.25.2"])
                self.assertIsNotNone(result, row["UnityPy 1.25.2"])
                if result.group(1) != "crunch":
                    try:
                        image = Texture2DConverter.parse_image_data(
                            data, 8, 8, fmt, (2021, 3, 1, 3), BuildTarget.StandaloneWindows64
                        )
                        got = ("image", image.mode, image.size)
                    except Exception as error:  # noqa: BLE001 - the exception type is the result
                        got = ("raises", type(error).__name__)
                    if result.group(1) == "raises":
                        self.assertEqual(got, ("raises", first_span(row["UnityPy 1.25.2"])))
                    else:
                        self.assertEqual(got, ("image", row["mode"], (8, 8)))

                cell = row["`set_image` writes"]
                try:
                    written = ("writes", Texture2DConverter.image_to_texture2d(source, fmt)[1].name)
                except Exception as error:  # noqa: BLE001
                    written = ("raises", type(error).__name__)
                self.assertEqual(written, ("raises" if cell.startswith("raises") else "writes", first_span(cell)))


if __name__ == "__main__":
    unittest.main()
