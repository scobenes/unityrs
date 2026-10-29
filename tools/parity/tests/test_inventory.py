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

# What `collect()` finds in UnityPy 1.25.2, group by group (`inventory.py list --counts`). PARITY.md
# section 21 states the same numbers. A collector that loses a kind of symbol (properties, say)
# shrinks its group, so `check` would ask PARITY.md for less and still pass: these counts stop that.
GROUP_SIZES = {
    "modules": 78,
    "classes": 955,
    "functions": 126,
    "data": 65,
    "methods": 228,
    "fields": 8456,
    "attributes": 145,
    "exports": 924,
    "boost": 15,
    "enum_members": 741,
    "texture_formats": 78,
}


SYMBOLS = {
    "modules": ["files/BundleFile.py", "export/Texture2DConverter.py"],
    "classes": ["BundleFile", "SpriteSettings", "GameObject"],
    "functions": ["ImportHelper.check_file_type"],
    "data": ["CompressionHelper.DECOMPRESSION_MAP"],
    "methods": ["Environment.load_file", "Texture2D.set_image"],
    "fields": ["GameObject.m_Name", "GameObject.m_Layer", "ObjectReader.path_id"],
    "attributes": ["Environment.files", "SerializedFile.objects"],
    "exports": ["UnityPy.files.File", "UnityPy.load"],
    "boost": ["UnityPyBoost.read_typetree"],
    "enum_members": [["BuildTarget.Switch", 38], ["BuildTarget.XBOX360", 11]],
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
        "Environment.files": ["UnityPy.Environment.files", "UnityPy.environment.Environment.files"],
        "SerializedFile.objects": ["UnityPy.files.SerializedFile.SerializedFile.objects"],
        "BuildTarget.Switch": ["UnityPy.enums.BuildTarget.Switch", "UnityPy.enums.BuildTarget.BuildTarget.Switch"],
        "BuildTarget.XBOX360": ["UnityPy.enums.BuildTarget.XBOX360"],
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
| `Environment.files` | an attribute written whole |

| class | bases | fields | attributes |
|---|---|---|---|
| `GameObject` | EditorExtension | `m_Layer`, `m_Name` | — |
| `SerializedFile` | — | — | `objects` |

| enum | values |
|---|---|
| `BuildTarget` | `XBOX360 = 11`, `Switch = 38` |

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
        matrix_row = "| 3 | `RGB24` | works |"
        for text in (
            COMPLETE.replace("| 3 | `RGB24` |", "| 4 | `RGB24` |"),
            COMPLETE.replace("| 3 | `RGB24` |", "| 3 | RGB24 |"),
            COMPLETE.replace("| 3 | `RGB24` |", "| 3 | `RGB24` and more |"),
            COMPLETE.replace("| 3 | `RGB24` |", "| 13 | `RGB24` |"),
            COMPLETE.replace(matrix_row, "") + "\n`RGB24` is format 3.\n",
            # A row of another table names no format: the package table, or a table of the matrix's width.
            COMPLETE.replace(matrix_row, "").replace(
                "| `UnityPy.files` | `File` |", "| `UnityPy.files` | `File` |\n| 3 | `RGB24` |"
            ),
            COMPLETE.replace(matrix_row, "") + "\n| a | b | c |\n|---|---|---|\n" + matrix_row + "\n",
        ):
            with self.subTest(text=text[-60:]):
                self.assertEqual(inventory.missing(SYMBOLS, text), {"texture_formats": ["RGB24 (3)"]})
        # A second table with the matrix's header fails the check, whether it holds the row the matrix lacks, a
        # copy of one the matrix has, or two columns only.
        for text in (
            COMPLETE.replace(matrix_row, "") + "\n| value | format | result |\n|---|---|---|\n" + matrix_row + "\n",
            COMPLETE.replace(matrix_row, "") + "\n| value | format |\n|---|---|\n| 3 | `RGB24` |\n",
            COMPLETE + "\n| value | format |\n|---|---|\n| 3 | `RGB24` |\n",
        ):
            with self.subTest(text=text[-60:]):
                self.assertEqual(
                    inventory.missing(SYMBOLS, text),
                    {"texture_formats": ["2 tables have the matrix's header `value | format`; only the matrix may"]},
                )
            with self.subTest(text=text[-60:]), self.assertRaisesRegex(ValueError, "2 tables"):
                inventory.texture_rows(text)
        # Two matrix rows for one format name, or for one value, fail the check: a dict keyed by name keeps only
        # the later row, so a wrong row before the right one would go unseen.
        same_name = "two matrix rows name the format `RGB24`"
        for rows, error in (
            (["| 3 | `RGB24` | raises |", matrix_row], same_name),
            ([matrix_row, "| 3 | `RGB24` | raises |"], same_name),
            (["| 4 | `RGB24` | works |", matrix_row], same_name),
            ([matrix_row, "| 3 | `RGB24x` | works |"], "two matrix rows give the value 3: `RGB24`, `RGB24x`"),
            (["| 3 | `RGB24x` | works |", matrix_row], "two matrix rows give the value 3: `RGB24x`, `RGB24`"),
        ):
            text = COMPLETE.replace(matrix_row, "\n".join(rows))
            with self.subTest(rows=rows):
                self.assertEqual(inventory.missing(SYMBOLS, text), {"texture_formats": [error]})
            with self.subTest(rows=rows), self.assertRaisesRegex(ValueError, re.escape(error)):
                inventory.texture_rows(text)

    def test_an_enum_member_needs_its_value_in_a_values_column(self):
        for old, new in (
            ("`Switch = 38`", "`Switch = 39`"),  # another value
            ("`Switch = 38`", "`Switch=38`"),  # not the `NAME = value` form
            ("| enum | values |", "| enum | members |"),  # not a values column
            ("| `BuildTarget` |", "| BuildTarget |"),  # no parent
        ):
            with self.subTest(new):
                text = COMPLETE.replace(old, new)
                self.assertIn("BuildTarget.Switch (38)", inventory.missing(SYMBOLS, text).get("enum_members", []))
        # The import path of the enum is a parent too.
        text = COMPLETE.replace("| `BuildTarget` |", "| `UnityPy.enums.BuildTarget` |")
        self.assertEqual(inventory.missing(SYMBOLS, text), {})

    def test_an_enum_value_names_its_member_and_nothing_else(self):
        """FileType's member `BundleFile = 1` does not name the class BundleFile."""
        text = COMPLETE.replace("| `BundleFile` | a class |", "| bundle | a class |")
        text = text.replace("`XBOX360 = 11`", "`XBOX360 = 11`, `BundleFile = 1`")
        self.assertEqual(inventory.missing(SYMBOLS, text), {"classes": ["BundleFile"]})

    def test_attributes_are_what_the_body_and_its_methods_bind(self):
        source = (
            "class C(Base):\n"
            "    A: int\n"
            "    B = 1\n"
            "    _c = 2\n"
            "    def __init__(self, x):\n"
            "        self.d = x\n"
            "        self.e, self._f = 1, 2\n"
            "        other.g = 3\n"
            "        def inner():\n"
            "            self.h = 4\n"
            "    @staticmethod\n"
            "    def make(target):\n"
            "        target.i = 5\n"
            "    @classmethod\n"
            "    def setup(cls):\n"
            "        cls.j = 6\n"
            "try:\n"
            "    import missing\n"
            "except ImportError:\n"
            "    class D:\n"
            "        k = 7\n"
        )
        defs = inventory._class_defs(source)
        self.assertEqual(sorted(defs), ["C", "D"])
        self.assertEqual(inventory._bound_names(defs["C"].body), ["A", "B"])
        self.assertEqual(sorted(inventory._self_attributes(defs["C"])), ["d", "e", "h", "j"])
        self.assertEqual(inventory._bound_names(defs["D"].body), ["k"])

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
            "attributes": ("`Environment.files`", "`File.Environment.files`", "Environment.files"),
            "enum_members": (
                "| `BuildTarget` | `XBOX360 = 11`, `Switch = 38` |",
                "| `BuildTarget` | `XBOX360 = 11` |\n| `TextureFormat` | `Switch = 38` |",
                "BuildTarget.Switch (38)",
            ),
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
        row = "| `GameObject` | EditorExtension | `m_Layer`, `m_Name` | — |"
        moved = COMPLETE.replace(
            row, "| `GameObject` | EditorExtension | `m_Layer` | — |\n| `Texture2D` | Texture | `m_Name` | — |"
        )
        self.assertEqual(inventory.missing(SYMBOLS, moved), {"fields": ["GameObject.m_Name"]})
        # Only a `fields`, `members` or `attributes` column counts.
        other = COMPLETE.replace("| class | bases | fields | attributes |", "| class | bases | notes | more |")
        self.assertEqual(
            inventory.missing(SYMBOLS, other),
            {"fields": ["GameObject.m_Name", "GameObject.m_Layer"], "attributes": ["SerializedFile.objects"]},
        )
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
            "attributes": ("| — | `objects` |", "| — | — |", "SerializedFile.objects"),
            "enum_members": ("`Switch = 38`", "`Switch`", "BuildTarget.Switch (38)"),
            # The whole line: a blank line would end the matrix, and the rows after it would name nothing.
            "texture_formats": ("| 1 | `Alpha8` | works |\n", "", "Alpha8 (1)"),
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
        self.assertEqual(sorted(GROUP_SIZES), sorted(inventory.GROUPS))
        self.assertEqual({group: len(groups[group]) for group in inventory.GROUPS}, GROUP_SIZES)
        # Section 21 states the same sizes, one row per group.
        section = self.text[self.text.index("## 21.") : self.text.index("### Reproducing the numbers")]
        stated = {}
        for header, rows in inventory.tables(section):
            if header[:2] == ["group", "what"]:
                for cells in rows:
                    count = re.search(r"\((\d[\d,]*)[,)]", cells[1])
                    stated[cells[0].strip("`")] = int(count.group(1).replace(",", ""))
        self.assertEqual(stated, GROUP_SIZES)
        # One representative of each kind of symbol the collector must find.
        kinds = {
            "methods": [
                "Environment.objects",  # a property
                "Texture2D.image",  # a property a legacy_patch module adds to a generated class
                "Environment.load_file",  # a method
            ],
            "exports": ["UnityPy.set_assetbundle_decrypt_key"],  # a function a package re-exports
            "fields": ["ObjectReader.path_id", "PPtr.m_PathID", "GameObject.m_Name"],  # own and generated attrs
            "attributes": [
                "Environment.files",  # assigned in __init__ and annotated in the body
                "SerializedFile.objects",
                "BundleFile.dataflags",
                "EndianBinaryReader.BaseOffset",
                "Environment.typetree_generator",  # a class-level default
                "TpkFile.TpkMagicBytes",  # a class constant
                "NodeClass.aliases",  # a dataclass field
                "DirectoryInfo.path",  # a namedtuple field
                "TypeTreeError.nodes",  # set by an exception's __init__
            ],
            "boost": ["UnityPyBoost.read_typetree", "UnityPyBoost.TypeTreeNode.m_Level"],  # a C type's member
        }
        for group, names in kinds.items():
            for name in names:
                self.assertIn(name, groups[group], group)
        # A base class's attribute is named under the base, and a member is not an attribute.
        for name in ("BundleFile.signature", "WebFile.packer", "EndianBinaryReader_Memoryview.Position"):
            self.assertNotIn(name, groups["attributes"])
        self.assertIn("File.signature", groups["attributes"])
        self.assertIn(["BuildTarget.Switch", 38], groups["enum_members"])
        self.assertIn(["ClassIDType.Texture2D", 28], groups["enum_members"])
        self.assertNotIn("TextureFormat", {name.split(".")[0] for name, _ in groups["enum_members"]})
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
            "attributes": (r"^(\| `Environment` \|.*)`local_files`, ", r"\1", "Environment.local_files"),
            "enum_members": (r"^(\| `BuildTarget` \|.*)`Switch = 38`", r"\1`Switch = 39`", "BuildTarget.Switch (38)"),
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
            # An attribute moved to another class's row.
            (
                [
                    (r"^(\| `Environment` \|.*)`cabs`, ", r"\1"),
                    (r"^(\| `BundleFile` \|.*) \|$", r"\1, `cabs` |"),
                ],
                "attributes",
                "Environment.cabs",
            ),
            # An enum member moved to another enum's row.
            (
                [
                    (r"^(\| `BuildTarget` \|.*)`Switch = 38`, ", r"\1"),
                    (r"^(\| `FileType` \|.*) \|$", r"\1, `Switch = 38` |"),
                ],
                "enum_members",
                "BuildTarget.Switch (38)",
            ),
        ]
        for steps, group, name in cases:
            with self.subTest(name):
                text = self.text
                for pattern, repl in steps:
                    text = mutate(self, text, pattern, repl)
                self.assertEqual(inventory.missing(self.symbols, text), {group: [name]})

    def test_appendix_c_is_the_enum_table(self):
        appendix = self.text[self.text.index("## Appendix C.") :]
        rows = [line for line in appendix.splitlines() if line.startswith("| `")]
        self.assertEqual(rows, inventory.enum_table(self.symbols))

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
