"""The PARITY.md inventory: the checker's matching rules, and PARITY.md against the oracle.

The rule tests need only the standard library. The others need UnityPy 1.25.2 (the
oracle image) and are skipped elsewhere: they list UnityPy's surface, require
PARITY.md to name all of it, and require `check` to fail when any one entry is
removed or written under a wrong parent. `test_texture_matrix.py` checks what the
TextureFormat matrix says UnityPy does.
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

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False

PARITY = inventory.DEFAULT_PARITY
NEEDS_ORACLE = "needs UnityPy 1.25.2 (the oracle image)"


SYMBOLS = {
    "modules": ["files/BundleFile.py", "export/Texture2DConverter.py"],
    "classes": ["BundleFile", "SpriteSettings", "GameObject"],
    "functions": ["ImportHelper.check_file_type"],
    "data": ["CompressionHelper.DECOMPRESSION_MAP"],
    "methods": ["Environment.load_file", "Texture2D.set_image"],
    "fields": ["GameObject.m_Name", "GameObject.m_Layer", "ObjectReader.path_id"],
    "exports": ["UnityPy.files.File", "UnityPy.load"],
    "boost": ["UnityPyBoost.read_typetree"],
    "texture_formats": [["Alpha8", 1], ["RGB24", 3]],
    # The import paths each short form may also be written as (inventory.collect builds these).
    "paths": {
        "BundleFile": ["UnityPy.files.BundleFile", "UnityPy.files.BundleFile.BundleFile"],
        "SpriteSettings": ["UnityPy.export.SpriteHelper.SpriteSettings"],
        "GameObject": ["UnityPy.classes.GameObject", "UnityPy.classes.generated.GameObject"],
        "ImportHelper.check_file_type": ["UnityPy.helpers.ImportHelper.check_file_type"],
        "CompressionHelper.DECOMPRESSION_MAP": ["UnityPy.helpers.CompressionHelper.DECOMPRESSION_MAP"],
        "Environment.load_file": [
            "UnityPy.Environment.load_file",
            "UnityPy.environment.Environment.load_file",
            "UnityPy.load.load_file",
        ],
        "GameObject.m_Name": ["UnityPy.classes.GameObject.m_Name", "UnityPy.classes.generated.GameObject.m_Name"],
        "ObjectReader.path_id": ["UnityPy.files.ObjectReader.ObjectReader.path_id"],
        "UnityPy.files.File": ["UnityPy.files.File"],
        "UnityPy.load": ["UnityPy.load"],
        "UnityPyBoost.read_typetree": ["UnityPy.UnityPyBoost.read_typetree"],
    },
}

COMPLETE = """
| item | note |
|---|---|
| `UnityPy/files/BundleFile.py:188-236` | a path, with its package prefix and a line range |
| `export/Texture2DConverter.py` | a bare path |
| `BundleFile` | a class |
| `SpriteHelper.SpriteSettings` | a class behind its module |
| `UnityPy.helpers.ImportHelper.check_file_type(path)` | a function behind its import path, called |
| `CompressionHelper.DECOMPRESSION_MAP[4]` | module data, subscripted |
| `Environment.load_file`, `Texture2D.set_image` | two methods in one cell |
| `ObjectReader.path_id` | a field written whole |
| `UnityPy.load` | a package export |
| `UnityPyBoost.read_typetree` | a Boost export |

| class | bases | fields |
|---|---|---|
| `GameObject` | EditorExtension | `m_Layer`, `m_Name` |

| package | members |
|---|---|
| `UnityPy.files` | `File` |

| value | format | result |
|---|---|---|
| 1 | `Alpha8` | works |
| 3 | `RGB24` | works |
"""


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
        text = COMPLETE.replace("`CompressionHelper.DECOMPRESSION_MAP[4]`", "`DECOMPRESSION_MAP[4]`")
        self.assertEqual(inventory.missing(SYMBOLS, text), {"data": ["CompressionHelper.DECOMPRESSION_MAP"]})

    def test_assigned_names_are_the_public_top_level_targets(self):
        source = "A = 1\nB: int = 2\nC, _D = 3, 4\n_E = 5\nif A:\n    F = 6\ndef g():\n    H = 7\n"
        self.assertEqual(inventory._assigned_names(source), ["A", "B", "C"])

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

    def test_a_name_under_a_wrong_parent_names_nothing(self):
        """The chain must start at the symbol or at one of its import paths, never inside another chain."""
        cases = {
            "functions": (
                "`UnityPy.helpers.ImportHelper.check_file_type(path)`",
                "`Wrong.ImportHelper.check_file_type`",
                "ImportHelper.check_file_type",
            ),
            "data": (
                "`CompressionHelper.DECOMPRESSION_MAP[4]`",
                "`helpers.Wrong.CompressionHelper.DECOMPRESSION_MAP`",
                "CompressionHelper.DECOMPRESSION_MAP",
            ),
            "methods": ("`Environment.load_file`,", "`Wrong.Environment.load_file`,", "Environment.load_file"),
            "fields": ("`ObjectReader.path_id`", "`SerializedFile.ObjectReader.path_id`", "ObjectReader.path_id"),
            "exports": ("`UnityPy.load`", "`Wrong.UnityPy.load`", "UnityPy.load"),
            "boost": (
                "`UnityPyBoost.read_typetree`",
                "`UnityPy.helpers.UnityPyBoost.read_typetree`",
                "UnityPyBoost.read_typetree",
            ),
            "classes": ("`SpriteHelper.SpriteSettings`", "`MeshHelper.SpriteSettings`", "SpriteSettings"),
        }
        for group, (old, new, name) in cases.items():
            with self.subTest(group):
                self.assertIn(old, COMPLETE)
                self.assertEqual(inventory.missing(SYMBOLS, COMPLETE.replace(old, new)), {group: [name]})

    def test_an_import_path_and_a_trailing_attribute_name_the_symbol(self):
        for span in (
            "`UnityPy.environment.Environment.load_file`",
            "`UnityPy.Environment.load_file`",
            "`environment.Environment.load_file`",
            "`Environment.load_file.__doc__`",
        ):
            with self.subTest(span):
                text = COMPLETE.replace("`Environment.load_file`,", span + ",")
                self.assertEqual(inventory.missing(SYMBOLS, text), {})

    def test_a_compact_row_names_its_members_under_its_own_parent(self):
        row = "| `GameObject` | EditorExtension | `m_Layer`, `m_Name` |"
        moved = COMPLETE.replace(
            row, "| `GameObject` | EditorExtension | `m_Layer` |\n| `Texture2D` | Texture | `m_Name` |"
        )
        self.assertEqual(inventory.missing(SYMBOLS, moved), {"fields": ["GameObject.m_Name"]})
        # Only a `fields` or `members` column counts.
        other = COMPLETE.replace("| class | bases | fields |", "| class | bases | notes |")
        self.assertEqual(inventory.missing(SYMBOLS, other), {"fields": ["GameObject.m_Name", "GameObject.m_Layer"]})
        # A member written outside a code span, or under another package, does not count.
        for new in ("| `UnityPy.files` | File |", "| `UnityPy.streams` | `File` |"):
            with self.subTest(new):
                text = COMPLETE.replace("| `UnityPy.files` | `File` |", new)
                self.assertEqual(inventory.missing(SYMBOLS, text), {"exports": ["UnityPy.files.File"]})

    def test_removing_any_one_entry_is_reported(self):
        removals = {
            "modules": ("`export/Texture2DConverter.py`", "`export/Converter.py`", "export/Texture2DConverter.py"),
            "classes": ("| `BundleFile` | a class |", "| bundle | a class |", "BundleFile"),
            "functions": (
                "`UnityPy.helpers.ImportHelper.check_file_type(path)`",
                "`check_file_type`",
                "ImportHelper.check_file_type",
            ),
            "data": (
                "`CompressionHelper.DECOMPRESSION_MAP[4]`",
                "`DECOMPRESSION_MAP`",
                "CompressionHelper.DECOMPRESSION_MAP",
            ),
            "methods": ("`Texture2D.set_image` |", "`set_image` |", "Texture2D.set_image"),
            "fields": ("`m_Layer`, `m_Name`", "`m_Name`", "GameObject.m_Layer"),
            "exports": ("| `UnityPy.files` | `File` |", "| `UnityPy.files` | |", "UnityPy.files.File"),
            "boost": ("`UnityPyBoost.read_typetree`", "`read_typetree`", "UnityPyBoost.read_typetree"),
            "texture_formats": ("| 1 | `Alpha8` | works |", "", "Alpha8 (1)"),
        }
        self.assertEqual(sorted(removals), sorted(inventory.GROUPS))
        for group, (old, new, name) in removals.items():
            with self.subTest(group):
                self.assertIn(old, COMPLETE)
                self.assertEqual(inventory.missing(SYMBOLS, COMPLETE.replace(old, new)), {group: [name]})


def mutate(test, text, pattern, repl):
    """`text` with every match of `pattern` replaced; the test fails if nothing matched."""
    new, count = re.subn(pattern, repl, text, flags=re.M)
    test.assertGreater(count, 0, pattern)
    return new


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
        self.assertIn("EndianBinaryReader.read_string_to_null", groups["methods"], "a class outside any fixed list")
        self.assertIn("UnityPyBoost.read_typetree", groups["boost"])
        self.assertIn("CompressionHelper.DECOMPRESSION_MAP", groups["data"])
        self.assertIn("config.FALLBACK_UNITY_VERSION", groups["data"])
        # Not data: an alias of a class (TF = TextureFormat), a TypeVar, a typing alias.
        for name in ("Texture2DConverter.TF", "ObjectReader.T", "CompressionHelper.ByteString"):
            self.assertNotIn(name, groups["data"])
        # Generated-class fields, and the fields of UnityPy's own attrs records.
        for name in ("GameObject.m_Name", "Texture2D.m_Width", "ObjectReader.path_id", "PPtr.m_PathID"):
            self.assertIn(name, groups["fields"])
        # Package re-exports, aliases included; a package's own submodules are modules, not exports.
        for name in ("UnityPy.files.File", "UnityPy.load", "UnityPy.classes.GameObject", "UnityPy.enums.CommonString"):
            self.assertIn(name, groups["exports"])
        for name in ("UnityPy.helpers.ImportHelper", "UnityPy.classes.ABC", "UnityPy.cli.ArgumentParser"):
            self.assertNotIn(name, groups["exports"])
        self.assertIn("UnityPy.classes.GameObject.m_Name", groups["paths"]["GameObject.m_Name"])

    def test_parity_md_names_every_symbol(self):
        self.assertEqual(inventory.missing(self.symbols, self.text), {})

    def test_appendix_a_has_one_row_per_generated_class(self):
        import UnityPy.classes.generated as generated

        defined = {
            name
            for name, value in vars(generated).items()
            if isinstance(value, type) and value.__module__ == generated.__name__ and not name.startswith("_")
        }
        appendix = self.text[self.text.index("## Appendix A.") : self.text.index("## Appendix B.")]
        rows = [cells[0].strip("`") for header, body in inventory.tables(appendix) for cells in body]
        self.assertEqual(len(defined), 869)
        self.assertEqual(sorted(rows), sorted(defined))

    def test_removing_one_entry_of_any_group_fails_the_check(self):
        """One representative entry per group, removed everywhere PARITY.md names it."""
        removals = {
            "modules": (r"files/WebFile\.py", "files/WebFileX.py", "files/WebFile.py"),
            "classes": (r"`AnnotationManager`", "`AnnotationManagerX`", "AnnotationManager"),
            "functions": (
                r"\bImportHelper\.check_file_type\b",
                "ImportHelper.check_file_typeX",
                "ImportHelper.check_file_type",
            ),
            "data": (
                r"\bCompressionHelper\.DECOMPRESSION_MAP\b",
                "CompressionHelper.DECOMPRESSION_MAPX",
                "CompressionHelper.DECOMPRESSION_MAP",
            ),
            "methods": (r"\bEnvironment\.load_file\b", "Environment.load_fileX", "Environment.load_file"),
            "fields": (r"^(\| `GameObject` \|.*)`m_IsActive`, ", r"\1", "GameObject.m_IsActive"),
            "exports": (r"^(\| `UnityPy\.files` \|.*)`DirectoryInfo`, ", r"\1", "UnityPy.files.DirectoryInfo"),
            "boost": (r"\bUnityPyBoost\.decrypt_block\b", "UnityPyBoost.decrypt_blockX", "UnityPyBoost.decrypt_block"),
            "texture_formats": (r"^\| 34 \| `ETC_RGB4` \|", "| 340 | `ETC_RGB4` |", "ETC_RGB4 (34)"),
        }
        self.assertEqual(sorted(removals), sorted(inventory.GROUPS))
        for group, (pattern, repl, name) in removals.items():
            with self.subTest(group):
                absent = inventory.missing(self.symbols, mutate(self, self.text, pattern, repl))
                self.assertIn(name, absent.get(group, []))

    def test_a_name_under_a_wrong_parent_fails_the_check(self):
        cases = [
            # A module-level function behind a wrong module.
            (
                [(r"(?<![\w.])((?:UnityPy\.helpers\.)?ImportHelper\.check_file_type)", r"Wrong.\1")],
                "functions",
                "ImportHelper.check_file_type",
            ),
            # A method behind a wrong class path.
            (
                [(r"(?<![\w.])Environment\.load_file\b", "Wrong.Environment.load_file")],
                "methods",
                "Environment.load_file",
            ),
            # A generated class's field moved to another class's row.
            (
                [
                    (r"^(\| `GameObject` \|.*)`m_IsActive`, ", r"\1"),
                    (r"^(\| `AABB` \|.*) \|$", r"\1, `m_IsActive` |"),
                ],
                "fields",
                "GameObject.m_IsActive",
            ),
            # An export moved to another package's row.
            (
                [
                    (r"^(\| `UnityPy\.files` \|.*)`DirectoryInfo`, ", r"\1"),
                    (r"^(\| `UnityPy\.streams` \|.*) \|$", r"\1, `DirectoryInfo` |"),
                ],
                "exports",
                "UnityPy.files.DirectoryInfo",
            ),
        ]
        for steps, group, name in cases:
            with self.subTest(name):
                text = self.text
                for pattern, repl in steps:
                    text = mutate(self, text, pattern, repl)
                self.assertEqual(inventory.missing(self.symbols, text), {group: [name]})

    def test_check_exits_1_and_lists_what_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "PARITY.md"
            path.write_text(self.text.replace("`Texture2DConverter.parse_image_data`", "parse_image_data"))
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                status = inventory.main(["check", str(path)])
        self.assertEqual(status, 1)
        self.assertIn("functions: 1 missing\n  Texture2DConverter.parse_image_data\n", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
