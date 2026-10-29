"""PARITY.md rows a review disputed, run against the oracle on the smallest input that shows each.

Every input is built here (`synth.py`), so nothing needs the corpus. Each test names the
PARITY.md rows it keeps honest; a row's test column names the test back. Needs UnityPy
1.25.2 (the oracle image); skipped elsewhere.
"""

import contextlib
import gzip
import io
import os
import random
import re
import struct
import sys
import tempfile
import unittest
import zipfile
from abc import ABC
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import inventory  # noqa: E402
import synth as S  # noqa: E402

try:
    import attrs
    import etcpak
    import lz4.block
    import UnityPy
    from PIL import Image
    from UnityPy import classes
    from UnityPy.classes import (
        GameObject,
        PPtr,
        StreamedResource,
        StreamingInfo,
        Texture2D,
        Texture2DArray,
        UnknownObject,
    )
    from UnityPy.enums import ArchiveFlags, ArchiveFlagsOld, BuildTarget, ClassIDType, FileType, TextureFormat
    from UnityPy.enums.GraphicsFormat import GraphicsFormat
    from UnityPy.export import AudioClipConverter, Texture2DConverter
    from UnityPy.files import BundleFile, WebFile
    from UnityPy.helpers import ImportHelper, PackedBitVector, ResourceReader, TextureSwizzler, Tpk, TypeTreeHelper
    from UnityPy.helpers.ContainerHelper import ContainerHelper
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.helpers.TypeTreeNode import clean_name as node_clean_name
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader
    from UnityPy.tools import extractor

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False

TEXT_ASSET = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])


def text_asset_file(name="hello", script="world", **kw):
    """A format-22 SerializedFile holding one TextAsset (path id 1)."""
    return S.serialized_file([S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32(name) + S.s32(script))], **kw)


def quietly(call, *args, **kw):
    """UnityPy prints a failing parse to stdout before re-raising it."""
    with contextlib.redirect_stdout(io.StringIO()):
        return call(*args, **kw)


def load(data, name="f.assets", env=None):
    env = env or UnityPy.Environment()
    return quietly(env.load_file, data, name=name), env


def scandir_walk(root):
    """PARITY.md section 3's walk, built from `os.scandir` and not from fsspec: a folder's files in the order
    `os.scandir` lists them, then each subfolder in that same order, depth-first."""
    entries = list(os.scandir(root))
    files = [entry.path for entry in entries if not entry.is_dir(follow_symlinks=False)]
    for entry in entries:
        if entry.is_dir(follow_symlinks=False):
            files += scandir_walk(entry.path)
    return files


def walk_tree(root):
    """A folder, under `root`, whose `os.scandir` listing tells section 3's walk from a sorted one and from one that
    enters a subfolder where the listing meets it: two subfolders holding the same name, one of them nested, made
    between the files, so that a file system listing in creation order or in its reverse (tmpfs lists the newest
    first) interleaves them. The listing has to be unsorted, with a subfolder before a file; a file system that
    lists the folders last, or in sorted order, gets a new folder, up to 32 times, before the precondition fails."""
    for attempt in range(32):
        tree = root / f"tree{attempt}"
        tree.mkdir(parents=True)
        for name in ("m", "s2/", "c", "s1/inner/", "x", "a", "s0/", "q", "f"):
            if name.endswith("/"):
                (tree / name).mkdir(parents=True)
            else:
                (tree / f"{name}.assets").write_bytes(text_asset_file(name))
        for name in ("s2/ab", "s1/ab", "s1/inner/z", "s0/k"):
            (tree / f"{name}.assets").write_bytes(text_asset_file(name.replace("/", "-")))
        listing = [entry.name for entry in os.scandir(tree)]
        kinds = [(tree / name).is_dir() for name in listing]
        if listing != sorted(listing) and kinds.index(True) < len(kinds) - 1 - kinds[::-1].index(False):
            return tree
    raise AssertionError(f"{root}: every listing sorted, or its folders last")


def zip_with_a_directory():
    """A zip listing a directory entry, `assets/`, as `zip -r` writes, with byte 4 zeroed so the detector sees a ZIP."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("assets/", b"")
        archive.writestr("assets/a.assets", text_asset_file("a"))
    data = bytearray(buffer.getvalue())
    data[4] = 0
    return bytes(data)


class TestColumn(unittest.TestCase):
    """PARITY.md's test column and the tests it names agree (standard library)."""

    CITED = re.compile(r"`(test_[a-z_]+\.py)` ([a-z0-9_]+)")

    def test_every_cited_test_exists_and_every_row_test_is_cited(self):
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        cited = set(self.CITED.findall(text))
        for module, name in sorted(cited):
            source = (HERE / "tests" / module).read_text(encoding="utf-8")
            self.assertIn(f"def test_{name}(", source, f"PARITY.md cites {module} {name}, which does not exist")
        # The row modules hold only tests a row names.
        modules = [Path(__file__), *sorted((HERE / "tests").glob("test_rows_*.py"))]
        self.assertGreater(len(modules), 1)
        for path in modules:
            defined = set(re.findall(r"def test_([a-z0-9_]+)\(", path.read_text(encoding="utf-8")))
            defined -= {
                "every_cited_test_exists_and_every_row_test_is_cited",
                "every_status_agrees_with_its_test_column",
                "rows_are_cited_by_item_not_by_line",
                "head_dicts_are_a_target_while_their_fixtures_are_planned",
                "every_resolution_exception_is_listed",
                "an_empty_path_raises_only_where_no_cab_answers_it",
                "the_gpu_program_index_rule_is_int_in_base_10",
            }
            named = {name for module, name in cited if module == path.name}
            self.assertEqual(sorted(defined - named), [], f"tests no PARITY.md row names, in {path.name}")

    @classmethod
    def probed(cls, test_cell):
        """Whether the test column names a test, or the corpus dump, outside a `planned` clause."""
        for clause in test_cell.split(";"):
            if "planned" in clause:
                continue
            if cls.CITED.search(clause) or re.search(r"`test_[a-z_]+\.py` \(all\)", clause) or "corpus dump" in clause:
                return True
        return False

    def test_every_status_agrees_with_its_test_column(self):
        """Section 0's status rules: a row that names a test is pinned or verified, one that names none is
        unverified (source reading), n/a (tier —) or a pointer to another row; `planned` is section 17.1's. A test
        or the corpus dump named inside a `planned` clause probes nothing, every row states its own status, and
        every test a test column names carries its module."""
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        seen = 0
        for header, rows in inventory.tables(text):
            if "status" not in header or "test" not in header:
                continue
            test, status = header.index("test"), header.index("status")
            for cells in rows:
                seen += 1
                probed = self.probed(cells[test])
                state = cells[status]
                with self.subTest(row=cells[0][:60], status=state):
                    # Every test is named with its module, so a renamed test fails the check above.
                    self.assertIsNone(re.search(r"`test_[a-z_]+\.py` [a-z0-9_]+, [a-z]", cells[test]), "a bare name")
                    if state.startswith(("pinned P0", "verified P0")):
                        self.assertTrue(probed, "a probed row names what probes it")
                    elif state.startswith("unverified (source reading); impl P"):
                        self.assertFalse(probed, "a row a test runs is not unverified")
                    elif state.startswith("planned ("):
                        self.assertIn("reference", header, "only section 17.1's references are planned")
                    elif state == "n/a":
                        self.assertEqual(cells[header.index("tier")], "—")
                    else:
                        self.assertEqual(state, "", "an unknown status")
                        self.assertEqual(set(cells[2:]), {""}, "a row with no status points at another section")
        self.assertGreater(seen, 500)

    def test_rows_are_cited_by_item_not_by_line(self):
        """A row points at another by its item and section ("the `.splitN` merging row, section 3"), never by a
        line number of PARITY.md, which every edit above it moves; nor do the docs and the parity tools and tests.
        So none says "row N" in any case, "line N of this file", or `PARITY.md:N`, and a "line N" stands only right
        after the source file it is a line of (`fmod.py` lines 129-137)."""
        root = HERE.parent.parent
        paths = [root / "PARITY.md", *sorted((root / "docs").glob("*.md")), *sorted(HERE.glob("*.md"))]
        paths += [*sorted(HERE.glob("*.py")), *sorted((HERE / "tests").glob("*.py"))]
        for path in paths:
            text = path.read_text(encoding="utf-8")
            with self.subTest(path.name):
                self.assertEqual(re.findall(r"(?i)\brows? \d+(?![\d%])", text), [])
                self.assertEqual(re.findall(r"(?i)\b(?:PARITY|DESIGN|README|CANONICAL_DUMP)\.md:\d+", text), [])
                self.assertEqual(
                    re.findall(r"(?i)\blines? \d+\S* of (?:this|that|the) (?:file|table|section)", text), []
                )
                bare = [
                    m.group(0)
                    for m in re.finditer(r"(?i)\blines? \d+", text)
                    if not re.search(r"`[^`\n]+` $", text[: m.start()])
                ]
                self.assertEqual(bare, [], "a line number with no source file before it")

    # DESIGN.md section 3's resolution clause names the Tier B and C exceptions to "`obj.container` and dependency
    # resolution match". Each bullet of those lists that touches resolution (it names one of RESOLUTION) maps here
    # to the phrase the clause names it by.
    RESOLUTION = ("find_file", "PPtr.deref", "load_file", "load_dependencies", "load_assets", "path_dict", "registry")
    NAMED_BY = {
        "a stream whose `name` is not a `str`": ("B", "`PPtr.deref` reaching an `int` or `None` `files` key"),
        "a record that `PPtr.deref` resolves": ("B", "a record whose file's basename holds an upper-case letter"),
        "a name read through a non-local fsspec filesystem, which": (
            "B",
            "a name read through a non-local fsspec filesystem where UnityPy raises",
        ),
        "two bundles holding a CAB of one name": ("C", "two bundles' same-named CABs"),
        "`find_file`'s suffix match": ("C", "`find_file`'s suffix match with no separator boundary"),
        "split members of a zip": ("C", "split members of a zip"),
        "a name read through a non-local fsspec filesystem where": (
            "C",
            "a name read through a non-local fsspec filesystem where `load_file` returns `None`",
        ),
        "`load_dependencies` loads dependencies": ("C", "`load_dependencies`"),
        "`ContainerHelper.path_dict` ignores": ("C", "`ContainerHelper.path_dict`'s file id"),
    }

    def test_every_resolution_exception_is_listed(self):
        """Every Tier B or C bullet of DESIGN.md section 3 that touches dependency resolution is named, under its
        tier, in the clause that lists the exceptions to "dependency resolution match"."""
        design = (HERE.parent.parent / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        reading = design.split("**Reading**", 1)[1].split("**Writing**", 1)[0]
        clause = next(line for line in reading.splitlines() if "dependency resolution match" in line)
        listed = clause.split("lists below say:", 1)[1]
        named = {"B": listed.split("(B)", 1)[0], "C": listed.split("(B)", 1)[1].split("(C)", 1)[0]}
        tier, seen = None, 0
        for line in reading.splitlines():
            if line.startswith("- **"):
                tier = "B" if line.startswith("- **Tier B") else "C" if line.startswith("- **Tier C") else None
            elif tier and line.startswith("  - ") and any(word in line for word in self.RESOLUTION):
                bullet = line[4:]
                with self.subTest(tier=tier, bullet=bullet[:70]):
                    lead = [key for key in self.NAMED_BY if bullet.startswith(key)]
                    self.assertEqual(len(lead), 1, "a resolution exception no clause names: list it, and map it here")
                    in_tier, phrase = self.NAMED_BY[lead[0]]
                    self.assertEqual(in_tier, tier)
                    self.assertIn(phrase, named[tier], "missing from the resolution clause")
                    seen += 1
        self.assertEqual(seen, len(self.NAMED_BY))

    def test_an_empty_path_raises_only_where_no_cab_answers_it(self):
        """An empty resource path is joined to `Environment.path` and raises `IsADirectoryError` only where no CAB
        `""` is registered: a zip's directory entry registers one, which answers the path with `b""` first (section
        3). So every PARITY.md row and DESIGN.md line that has an empty path, or a texture with no data, raise there
        names CAB `""` too."""
        root = HERE.parent.parent
        lines = [("PARITY.md", line) for line in (root / "PARITY.md").read_text("utf-8").splitlines()]
        lines += [("DESIGN.md", line) for line in (root / "docs" / "DESIGN.md").read_text("utf-8").splitlines()]
        seen = 0
        for name, line in lines:
            raises = "IsADirectoryError" in line or "raises as `Texture2D.get_image_data` does" in line
            empty = re.search(r"empty (?:stream |resource )?path|empty `m_Source`|empty name|with no data", line)
            if raises and empty:
                seen += 1
                with self.subTest(name, line=line[:80]):
                    self.assertIn('CAB `""`', line)
        self.assertGreaterEqual(seen, 6)

    def test_the_gpu_program_index_rule_is_int_in_base_10(self):
        """`GpuProgramIndex N` fills where `int()` in base 10 takes the rest of the line, which is not Python's
        integer-literal grammar (`+0` and `٠` fill, `0x0` raises; `test_rows_exports.py` shader_text): no line of
        PARITY.md or DESIGN.md that speaks of such a line calls the rule a literal, and each that states the refusal
        of a line the table cannot fill names `int()` in base 10."""
        root = HERE.parent.parent
        refusals = 0
        for name, path in (("PARITY.md", root / "PARITY.md"), ("DESIGN.md", root / "docs" / "DESIGN.md")):
            for line in path.read_text(encoding="utf-8").splitlines():
                if "GpuProgramIndex" not in line:
                    continue
                with self.subTest(name, line=line[:80]):
                    self.assertNotRegex(line, r"(?i)\bliteral")
                    if "cannot fill, where" in line:  # the refusal, not a tier cell that names it
                        refusals += 1
                        self.assertIn("`int()` in base 10", line)
        self.assertGreaterEqual(refusals, 2)

    def test_head_dicts_are_a_target_while_their_fixtures_are_planned(self):
        """Section 20's reading rows name the Unity 6000.4/6000.5 fixtures as planned, so no comparison with HEAD's
        dicts exists: every sentence or table cell of PARITY.md and DESIGN.md that speaks of dicts and HEAD (or those
        fixtures) calls that equality a target, or says no comparison has been run, and none states it as done."""
        root = HERE.parent.parent
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        section = parity.split("\n## 20. HEAD tier\n", 1)[1].split("\n## ", 1)[0]
        planned = [
            cells[0]
            for header, rows in inventory.tables(section)
            for cells in rows
            if "planned 6000.4/6000.5 fixtures" in cells[header.index("test")]
        ]
        if not planned:
            self.skipTest("section 20 names no planned HEAD fixture: the reading rows are measured")
        speaking = 0
        for name, text in (("PARITY.md", parity), ("DESIGN.md", (root / "docs" / "DESIGN.md").read_text("utf-8"))):
            for sentence in re.split(r"(?<=\.)\s+|\s*\|\s*|\n", text):
                if "dicts" in sentence and ("HEAD" in sentence or "6000.4/6000.5" in sentence):
                    speaking += 1
                    with self.subTest(name, sentence=sentence[:80]):
                        self.assertTrue(
                            "target" in sentence or "No comparison" in sentence, "HEAD's dicts stated as measured"
                        )
        self.assertGreaterEqual(speaking, 3)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    def chdir(self, path):
        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)

    # Sections 4, 5.2, 6: WebFile gzip.
    def test_gzip_webfile_is_detected_and_never_parsed(self):
        plain = S.web_file([(b"a.txt", bytes(range(256)) * 2), (b"b.txt", b"world" * 40)])
        packed = gzip.compress(plain, mtime=0)
        self.assertEqual(list(load(plain)[0].files), ["a.txt", "b.txt"], "a plain WebFile loads")

        # Detection reads the gzip stream through the shared reader and leaves it where it stopped.
        kind, reader = ImportHelper.check_file_type(packed)
        self.assertEqual(kind, FileType.WebFile)
        self.assertNotEqual(reader.Position, 0)
        # So WebFile reads its magic past the start, takes the file for plain, and the signature fails.
        with self.assertRaisesRegex(ValueError, r"^Invalid WebFile signature: '\\x1f\\udc8b\\x08'"):
            load(packed)
        path = self.tmp / "web.data.gz"
        path.write_bytes(packed)
        with self.assertRaisesRegex(ValueError, "Invalid WebFile signature"):
            quietly(UnityPy.load, str(path))
        # Inside a bundle too.
        with self.assertRaisesRegex(ValueError, "Invalid WebFile signature"):
            load(S.fs_bundle([(b"inner.data.gz", packed, 0)]))
        # The decoder itself works on a fresh reader, and agrees with gzip.decompress plus the plain layout.
        self.assertEqual(list(WebFile(EndianBinaryReader(packed), None).files), ["a.txt", "b.txt"])
        self.assertEqual(gzip.decompress(packed), plain)

    # Sections 5.1, 18: UnityArchive.
    def test_unityarchive_loads_as_a_raw_file(self):
        data = S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], signature=b"UnityArchive")
        self.assertEqual(ImportHelper.check_file_type(data)[0], FileType.ResourceFile)
        loaded, env = load(data, name="archive")
        self.assertIsInstance(loaded, EndianBinaryReader)
        self.assertIs(env.get_cab("archive"), loaded)
        with self.assertRaisesRegex(NotImplementedError, "UnityArchive"):
            BundleFile(EndianBinaryReader(data), env)
        # A BundleFile of any other unknown signature raises too (detection never builds one).
        unknown = S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], signature=b"UnityXYZ")
        with self.assertRaisesRegex(NotImplementedError, "^Unknown Bundle signature: UnityXYZ$"):
            BundleFile(EndianBinaryReader(unknown), env)

    # Sections 8 and 10: an object with no tree anywhere.
    def test_no_tree_raises_the_tpk_error(self):
        def parse(class_id, **kw):
            sf = S.serialized_file(
                [S.Type(class_id, TEXT_ASSET)],
                [S.Obj(1, 0, S.s32("a") + S.s32("b"), class_id=class_id)],
                enable_type_tree=False,
                **kw,
            )
            return next(iter(load(sf)[0].objects.values())).parse_as_dict()

        self.assertEqual(parse(49), {"m_Name": "a", "m_Script": "b"}, "the TPK's TextAsset")
        with self.assertRaises(KeyError):
            parse(987654)  # not in the TPK
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            parse(49, unity_version=b"2.0.0f1")  # before the TPK's first TextAsset
        self.assertIsNone(Tpk.TPKTYPETREE.ClassInformation[12][-1][1], "class 12 is gone from 2018.3")
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            parse(12)
        with self.assertRaisesRegex(AssertionError, "has no ReleaseRootNode"):
            parse(18)  # EditorExtension, abstract: no release tree
        # TypeTreeError is unreachable: a node is always truthy, and the TPK returns one or raises.
        self.assertTrue(TypeTreeNode(m_Level=0, m_Type="X", m_Name="Base", m_ByteSize=0, m_Version=0))

    # Sections 3, 5.2, 14.1: one CAB registry for the whole Environment.
    def test_same_named_cabs_share_one_registry_entry(self):
        env = UnityPy.Environment()

        def bundle(name, fill):
            return S.fs_bundle([(b"CAB-same", text_asset_file(name), 4), (b"CAB-same.resS", fill * 64, 0)])

        a, _ = load(bundle("A", b"\x11"), "a", env)
        b, _ = load(bundle("B", b"\x22"), "b", env)
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["A", "B"])
        self.assertEqual(list(env.cabs), ["cab-same", "cab-same.ress"])
        self.assertIs(env.cabs["cab-same"], b.files["CAB-same"], "the last one loaded wins")
        # A's texture data is read from B's .resS.
        got = ResourceReader.get_resource_data("archive:/CAB-same/CAB-same.resS", a.files["CAB-same"], 0, 4)
        self.assertEqual(got, b"\x22" * 4)
        # Environment.container joins only the registered copy's container.
        info = SimpleNamespace(asset=SimpleNamespace(path_id=1), preloadIndex=0, preloadSize=0)
        a.files["CAB-same"]._container = ContainerHelper([("a/path", info)])
        b.files["CAB-same"]._container = ContainerHelper([("b/path", info)])
        self.assertEqual(list(env.container.keys()), ["b/path"])
        # A PPtr looks in its own parent's entries first (PPtr.deref's loop over `files`), so from inside either
        # bundle it reads that bundle's copy; only one from outside both reaches the registry, and reads B's.
        env = UnityPy.Environment()

        def holder(tag, cab):
            pointing = S.serialized_file(
                [S.Type(49, TEXT_ASSET)],
                [S.Obj(1, 0, S.s32(f"main {tag}") + S.s32("s"))],
                externals=[S.External(b"archive:/CAB-same/CAB-same")],
            )
            entries = [(cab, pointing, 4)]
            if tag != "C":
                entries += [(b"CAB-same", text_asset_file(tag), 4), (b"CAB-same.resS", tag.encode() * 64, 0)]
            return load(S.fs_bundle(entries), tag, env)[0]

        pointing = {tag: holder(tag, cab).files[cab.decode()] for tag, cab in (("A", b"CAB-main"), ("B", b"CAB-main"))}
        pointing["C"] = holder("C", b"CAB-c").files["CAB-c"]
        self.assertEqual(list(env.cabs), ["cab-main", "cab-same", "cab-same.ress", "cab-c"])
        read = {
            tag: PPtr(m_FileID=1, m_PathID=1, assetsfile=sf).deref().parse_as_dict()["m_Name"]
            for tag, sf in pointing.items()
        }
        self.assertEqual(read, {"A": "A", "B": "B", "C": "B"})
        got = ResourceReader.get_resource_data("archive:/CAB-same/CAB-same.resS", pointing["A"], 0, 4)
        self.assertEqual(got, b"BBBB", "a .resS read from A still reads B's")

    # Section 7.1: type dependencies sit inside the tree flag.
    def test_type_dependencies_need_the_tree_flag(self):
        with_tree = S.serialized_file([S.Type(49, TEXT_ASSET, deps=[7, 8])], [S.Obj(1, 0, S.s32("a") + S.s32("b"))])
        self.assertEqual(load(with_tree)[0].types[0].type_dependencies, (7, 8))
        stripped = text_asset_file(enable_type_tree=False)
        loaded = load(stripped)[0]
        self.assertIsNone(loaded.types[0].type_dependencies)
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["hello"])

        class WithDependencies(S.Type):
            def encode(self, version, enable_type_tree, is_ref, E):
                return super().encode(version, enable_type_tree, is_ref, E) + struct.pack(E + "i", 0)

        misread = S.serialized_file(
            [WithDependencies(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))], enable_type_tree=False
        )
        with self.assertRaises(struct.error):
            load(misread)

    # Section 6: LZ4 blocks.
    def test_lz4_stored_size_is_a_capacity(self):
        entries = [(b"CAB-test", text_asset_file(), 4), (b"CAB-test.resS", b"\x11" * 64, 0)]
        size = sum(len(data) for _, data, _ in entries)
        loaded = load(S.fs_bundle(entries, stored_sizes=[size + 100]))[0]
        self.assertEqual(list(loaded.files), ["CAB-test", "CAB-test.resS"], "a block that decodes short loads")
        for stored in (size - 10, 0):
            with self.subTest(stored=stored), self.assertRaises(lz4.block.LZ4BlockError):
                load(S.fs_bundle(entries, stored_sizes=[stored]))

    # Sections 5.2, 13: reads past the end.
    def test_reads_past_the_end_return_short_data(self):
        sf = text_asset_file()
        data = sf + b"\x11" * 64
        rows = [(b"CAB-test", 0, len(sf), 4), (b"CAB-test.resS", len(sf), 64 + 1000, 0)]
        loaded = load(S.fs_bundle(rows, blocks=[data], codec=0))[0]
        self.assertEqual(loaded.files["CAB-test.resS"].Length, 64, "the entry is cut to the bytes that exist")
        got = ResourceReader.get_resource_data("CAB-test.resS", loaded.files["CAB-test"], 60, 500)
        self.assertEqual(got, b"\x11" * 4)
        reader = EndianBinaryReader(b"abcd")
        self.assertEqual((reader.read_bytes(10), reader.Position), (b"abcd", 10))
        with self.assertRaises(struct.error):
            reader.read_int()
        # Section 13, negative sizes and positions. The memory reader's Position takes any value, and its reads
        # index the view with it: read(-1) reads to the end, any other size is the slice view[P:P + n].
        data = bytes(range(20))
        reader = EndianBinaryReader(data)

        def read_at(position, size):
            reader.Position = position
            return reader.read(size), reader.Position

        self.assertEqual(read_at(5, 0), (b"", 5))
        self.assertEqual(read_at(5, -1), (data[5:], 20))
        self.assertEqual(read_at(-3, -1), (data[-3:], 20))
        self.assertEqual(read_at(5, -2), (b"", 3))
        self.assertEqual(read_at(2, -5), (data[2:17], -3))
        self.assertEqual(read_at(-3, 2), (data[17:19], -1))
        self.assertEqual(read_at(-2, 4), (b"", 2))
        minus_one = EndianBinaryReader(struct.pack(">i", -1) + data)
        self.assertEqual(minus_one.read_byte_array(), data)
        # A fixed-width primitive at a negative Position counts from the end, and raises when fewer bytes than its
        # size lie from there to the end, or the Position is before the start.
        reader.Position = -4
        self.assertEqual((reader.read_int(), reader.Position), (struct.unpack(">i", data[-4:])[0], 0))
        for position, message in ((-2, "^not enough data to unpack 4 bytes at offset -2$"), (-24, "out of range")):
            reader.Position = position
            with self.subTest(position=position), self.assertRaisesRegex(struct.error, message):
                reader.read_int()
        # The stream reader refuses a negative Position, and its read is the stream's own: -1 reads to the end;
        # below -1 a buffered file raises, while a BytesIO reads to the end.
        stream = EndianBinaryReader(io.BytesIO(data))
        stream.Position = 5
        self.assertEqual((stream.read(-2), stream.Position), (data[5:], 20))
        with self.assertRaisesRegex(ValueError, "^Position cannot be negative$"):
            stream.Position = -1
        path = self.tmp / "data.bin"
        path.write_bytes(data)
        from fsspec.implementations.local import LocalFileSystem

        for opener in (open, LocalFileSystem().open):
            with opener(str(path), "rb") as opened:
                stream = EndianBinaryReader(opened)
                stream.Position = 18
                self.assertEqual(stream.read(-1), data[18:])
                stream.Position = 18
                with self.assertRaisesRegex(ValueError, "^read length must be non-negative or -1$"):
                    stream.read(-7)

    # Sections 9.1, 18: a null managed reference.
    def test_an_empty_managed_reference_does_not_resave(self):
        root = S.Node("MonoBehaviour", "Base", [S.Node("int", "x"), S.ref_object("ref")])
        foo = S.Node("Foo", "Base", [S.Node("float", "v")])
        empty = struct.pack("<iq", 7, 1) + S.s32("") * 3
        typed = struct.pack("<iq", 7, 2) + S.s32("Foo") + S.s32("") + S.s32("Asm") + struct.pack("<f", 1.5)
        refs = [S.Type(-1, foo, ref_names=["Foo", "", "Asm"])]
        sf = S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, empty), S.Obj(2, 0, typed)], ref_types=refs)
        objects = load(sf)[0].objects
        value = objects[1].parse_as_dict()
        self.assertEqual(value["ref"], {"rid": 1, "type": {"class": "", "ns": "", "asm": ""}})
        with self.assertRaisesRegex(KeyError, "'data'"):
            objects[1].save_typetree(value)
        self.assertEqual(objects[2].save_typetree(objects[2].parse_as_dict()), objects[2].get_raw_data())

    # Sections 9.1: get_ref_type_node on an empty ref_types list.
    def test_ref_type_lookup_differs_on_an_empty_list(self):
        root = S.Node("MonoBehaviour", "Base", [S.Node("int", "x"), S.ref_object("ref")])
        empty = struct.pack("<iq", 7, 1) + S.s32("") * 3
        loaded = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, empty)], ref_types=[]))[0]
        self.assertEqual(loaded.ref_types, [])
        obj = loaded.objects[1]
        self.assertEqual(obj.parse_as_dict()["ref"], {"rid": 1, "type": {"class": "", "ns": "", "asm": ""}})
        reader = EndianBinaryReader(empty, endian="<")
        with self.assertRaisesRegex(ValueError, "^SerializedFile has no ref_types$"):
            TypeTreeHelper.read_typetree(
                obj._get_typetree_node(), reader, as_dict=True, byte_size=0, check_read=False, assetsfile=loaded
            )

    # Sections 5.2, 14.2, 17: a texture with no pixel data.
    def test_an_empty_texture_follows_an_empty_stream_path(self):
        env = UnityPy.Environment(path=str(self.tmp))
        sf = load(text_asset_file(), env=env)[0]

        def texture(stream, width=4):
            return SimpleNamespace(
                image_data=b"",
                m_StreamData=stream,
                object_reader=SimpleNamespace(assets_file=sf, version=(2021, 3, 2, 0), platform=0),
                m_Width=width,
                m_Height=width,
                m_TextureFormat=4,
                m_PlatformBlob=None,
                get_image_data=lambda: Texture2D.get_image_data(fake),
            )

        fake = texture(StreamingInfo(offset=0, size=0, path=""))
        self.assertTrue(fake.m_StreamData, "a StreamingInfo is truthy, even empty")
        # The empty path is looked up, misses, is loaded relative to env.path, and opens the directory.
        with self.assertRaises(IsADirectoryError):
            Texture2D.get_image_data(fake)
        with self.assertRaises(IsADirectoryError):
            ResourceReader.get_resource_data("", sf, 0, 0)
        # The names tried, in order: the basename, then `{name}.resource`, `{name}.assets.resS`, `{name}.resS`; a
        # miss after load_dependencies raises FileNotFoundError.
        names = ["x.bin", "x.resource", "x.assets.resS", "x.resS"]
        for first in range(4):
            own = UnityPy.Environment(path=str(self.tmp))
            asking = load(text_asset_file(), env=own)[0]
            for n, name in enumerate(names[first:]):
                load(bytes([65 + first + n]) * 10, name=name, env=own)
            with self.subTest(names[first]):
                self.assertEqual(
                    ResourceReader.get_resource_data("archive:/CAB/x.bin", asking, 2, 3), bytes([65 + first]) * 3
                )
        with self.assertRaisesRegex(FileNotFoundError, "^Resource file x.bin not found$"):
            ResourceReader.get_resource_data("archive:/CAB/x.bin", sf, 0, 1)
        # A 0x0 texture fetches its data first too, so it never reaches the empty-image shortcut.
        fake = texture(StreamingInfo(offset=0, size=0, path=""), width=0)
        with self.assertRaises(IsADirectoryError):
            Texture2DConverter.get_image_from_texture2d(fake)
        # Only a texture with no m_StreamData at all (pre-5.3 trees) gets the ValueError.
        fake = texture(None)
        with self.assertRaisesRegex(ValueError, "^No image data found$"):
            Texture2D.get_image_data(fake)
        image = Texture2DConverter.parse_image_data(b"", 0, 0, 4, (2021, 3, 2, 0), 0)
        self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)), "parse_image_data's own shortcut")
        # A zip's directory entry registers CAB "" (section 3), which answers the empty path first: it reads b"", so
        # a 0x0 texture returns the empty image and one of non-zero size raises ValueError.
        quietly(env.load_file, zip_with_a_directory(), name="z.bin")
        self.assertEqual(ResourceReader.get_resource_data("", sf, 0, 8), b"")
        fake = texture(StreamingInfo(offset=0, size=0, path=""), width=0)  # `texture` reads `fake` when called
        image = Texture2DConverter.get_image_from_texture2d(fake)
        self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)))
        fake = texture(StreamingInfo(offset=0, size=0, path=""))
        with self.assertRaisesRegex(ValueError, "^Texture2D has no image data$"):
            Texture2DConverter.get_image_from_texture2d(fake)

    # Sections 3, 4, 13: what reader a path gets.
    def test_a_path_reads_strings_with_the_stream_rule(self):
        path = self.tmp / "twenty"
        path.write_bytes(b"A" * 20)
        kind, reader = ImportHelper.check_file_type(str(path))  # opens the file and leaves it open
        reader.dispose()
        self.assertEqual(kind, FileType.ResourceFile)
        with open(path, "rb") as stream:
            self.assertEqual(ImportHelper.check_file_type(stream)[0], FileType.ResourceFile)
        with self.assertRaisesRegex(Exception, "^String not terminated$"):
            ImportHelper.check_file_type(b"A" * 20)
        # Only an input of exactly 20 bytes reaches the memory reader's end: a longer one takes its first 20
        # bytes as the signature, and detection goes on.
        for size in (21, 127, 200):
            with self.subTest(size=size):
                self.assertEqual(ImportHelper.check_file_type(b"A" * size)[0], FileType.ResourceFile)
        env = quietly(UnityPy.load, str(path))
        reader = env.files[str(path)]
        self.addCleanup(reader.dispose)
        self.assertEqual(type(reader).__name__, "EndianBinaryReader_Streamable")
        # With no NUL in reach the stream reader never raises and drops the last byte it read.
        stream = EndianBinaryReader(io.BytesIO(b"C" * 10))
        self.assertEqual((stream.read_string_to_null(), stream.Position), ("C" * 9, 10))
        stream = EndianBinaryReader(io.BytesIO(b"B" * 40000))
        self.assertEqual((len(stream.read_string_to_null()), stream.Position), (32766, 32767))
        memory = EndianBinaryReader(b"B" * 40000)
        self.assertEqual((len(memory.read_string_to_null()), memory.Position), (32767, 32767))
        # A string with its NUL in reach reads the same from both.
        for reader in (EndianBinaryReader(io.BytesIO(b"2021.3.2f1\0rest")), EndianBinaryReader(b"2021.3.2f1\0rest")):
            self.assertEqual((reader.read_string_to_null(), reader.Position), ("2021.3.2f1", 11))

    # Section 3: Environment.load and load_file.
    def test_load_keys_each_file_by_path_and_basename(self):
        (self.tmp / "sub").mkdir()
        path = self.tmp / "sub" / "asset"
        path.write_bytes(text_asset_file())
        env = UnityPy.Environment()
        quietly(env.load, [str(path)])
        self.assertEqual(list(env.files), [str(path), "asset"])
        self.assertEqual(len(env.objects), 2)
        self.chdir(self.tmp / "sub")
        env = UnityPy.Environment()
        quietly(env.load, ["asset"])
        self.assertEqual((list(env.files), len(env.objects)), (["asset"], 1))

    def test_load_file_tries_the_cwd_first(self):
        (self.tmp / "cwd").mkdir()
        (self.tmp / "env").mkdir()
        (self.tmp / "cwd" / "x").write_bytes(text_asset_file("from the cwd"))
        (self.tmp / "env" / "x").write_bytes(text_asset_file("from env.path"))
        self.chdir(self.tmp / "cwd")
        env = UnityPy.Environment(path=str(self.tmp / "env"))
        loaded = quietly(env.load_file, "x")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["from the cwd"])
        (self.tmp / "cwd" / "x").unlink()
        loaded = quietly(env.load_file, "x")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["from env.path"])

    # Sections 3, 4, 19.2: the case-insensitive walk takes `os.path.split`'s head and tail.
    def test_the_case_insensitive_walk_takes_head_and_tail(self):
        d = self.tmp / "d"
        (d / "Resources" / "Sub").mkdir(parents=True)
        for name in ("Resources/Foo.asset", "Resources/Sub/Bar.asset", "Top.asset"):
            (d / name).write_bytes(text_asset_file(name))
        walk = ImportHelper.find_sensitive_path
        found = str(d / "Resources" / "Foo.asset")
        # Exactly two components resolve; leading and trailing separators are stripped first.
        for path in ("resources/foo.asset", "RESOURCES/FOO.ASSET", "/resources/foo.asset", "resources/foo.asset/"):
            self.assertEqual(walk(str(d), path), found, path)
        # A single name (an empty head) or a deeper path does not, even in exact case; nor a full path.
        for path in ("Top.asset", "top.asset", "Resources/Sub/Bar.asset", f"{d}/resources/foo.asset"):
            self.assertIsNone(walk(str(d), path), path)
        # A directory the local disk does not have raises, `''` included.
        for missing in ("", str(d / "nowhere")):
            with self.assertRaises(FileNotFoundError):
                walk(missing, "resources/foo.asset")

        def names(path, name):
            env = UnityPy.Environment(path=path)
            loaded = quietly(env.load_file, name)
            return None if loaded is None else [o.parse_as_dict()["m_Name"] for o in loaded.objects.values()]

        self.chdir(self.tmp)
        # load_file hands the walk the name joined to `path`, so a record in another case is not found.
        self.assertIsNone(names(str(d), "resources/foo.asset"))
        self.assertIsNone(names(str(d), "top.asset"))
        self.assertEqual(names(str(d), "Resources/Foo.asset"), ["Resources/Foo.asset"])
        # An absolute name is handed over as given: `/a/b` is looked up as `path/A/B`.
        self.assertEqual(names(str(d), "/resources/foo.asset"), ["Resources/Foo.asset"])
        # A relative one-component `path` P and a bare name n are looked up as P/P/n.
        self.chdir(d)
        self.assertIsNone(names("Resources", "foo.asset"))
        (d / "Resources" / "resources").mkdir()
        (d / "Resources" / "resources" / "FOO.asset").write_bytes(text_asset_file("doubled"))
        self.assertEqual(names("Resources", "foo.Asset"), ["doubled"])
        # A `path` the walk cannot list raises instead of returning None.
        for path in ("", "nowhere"):
            with self.subTest(path=path), self.assertRaises(FileNotFoundError):
                names(path, "x.asset")

    # Sections 3, 19.2: a filesystem other than the local one.
    def test_a_non_local_fs_loads_only_through_load_and_streams(self):
        from fsspec.implementations.memory import MemoryFileSystem

        fs = MemoryFileSystem()  # one store per process: a folder of this test's own, removed after
        root = f"/{self.tmp.name}"
        fs.pipe(f"{root}/x.assets", text_asset_file("in memory"))
        self.addCleanup(fs.rm, root, recursive=True)
        self.assertFalse(os.path.exists(root), "nothing of that name on the local disk")
        self.assertEqual(UnityPy.Environment(fs=fs).path, "")
        # load_file tests a name with os.path and walks `path` with os.listdir, both on the local disk.
        for name in (f"{root}/x.assets", "x.assets", "missing"):
            with self.subTest(name), self.assertRaises(FileNotFoundError):
                quietly(UnityPy.Environment(fs=fs).load_file, name)
        with self.assertRaises(FileNotFoundError):
            quietly(UnityPy.Environment(fs=fs, path=root).load_file, "x.assets")
        # Where the local disk can list `path`, the walk finds nothing there and the name is None, with no error.
        self.assertIsNone(quietly(UnityPy.Environment(fs=fs, path=str(self.tmp)).load_file, "x.assets"))
        self.assertIsNone(quietly(UnityPy.Environment(fs=fs, path="/").load_file, f"{root[1:]}/x.assets"))
        # The constructor takes a file through load_file, a folder through load_files' builtin open.
        for arg in (f"{root}/x.assets", root):
            with self.subTest(arg), self.assertRaises(FileNotFoundError):
                quietly(UnityPy.Environment, arg, fs=fs)
        with self.assertRaises(FileNotFoundError):
            quietly(UnityPy.Environment(fs=fs).load_folder, root)
        # load (fs.exists, fs.open) and a stream do read through the filesystem.
        env = UnityPy.Environment(fs=fs)
        quietly(env.load, [f"{root}/x.assets"])
        self.assertEqual(list(env.files), [f"{root}/x.assets", "x.assets"])
        with fs.open(f"{root}/x.assets", "rb") as stream:
            loaded = quietly(UnityPy.Environment(fs=fs).load_file, stream, name="x.assets")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.objects.values()], ["in memory"])

    # Sections 14.1, 14.3: file ids and the preload pass.
    def test_negative_file_ids_index_from_the_end(self):
        sf = load(text_asset_file(), env=UnityPy.Environment(path=str(self.tmp)))[0]
        self.assertEqual(sf.externals, [])
        with self.assertRaises(IndexError):
            PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf).deref()
        sf.externals = [SimpleNamespace(path="archive:/CAB-first/CAB-first"), SimpleNamespace(path="archive:/x/y")]
        with self.assertRaisesRegex(FileNotFoundError, "cab-first"):
            PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf).deref()  # externals[-2]
        sf.externals = []

        def preload(pptr):
            info = SimpleNamespace(asset=SimpleNamespace(path_id=1), preloadIndex=0, preloadSize=1)
            helper = ContainerHelper(SimpleNamespace(m_Container=[("p", info)], m_PreloadTable=[pptr]))
            helper.parse_preload_table()
            return helper

        preload(PPtr(m_FileID=5, m_PathID=1, assetsfile=sf))  # FileNotFoundError: skipped
        preload(PPtr(m_FileID=0, m_PathID=99, assetsfile=sf))  # KeyError: skipped
        with self.assertRaises(IndexError):
            preload(PPtr(m_FileID=-1, m_PathID=1, assetsfile=sf))
        # deref: no assetsfile raises ValueError before the path id is looked at; path id 0 raises ValueError; file
        # id 0 is the pointer's own file; a missing path id is KeyError.
        with self.assertRaisesRegex(ValueError, "^PPtr can't deref without an assetsfile!$"):
            PPtr(m_FileID=0, m_PathID=0).deref()
        with self.assertRaisesRegex(ValueError, "^PPtr can't deref with m_PathID == 0!$"):
            PPtr(m_FileID=0, m_PathID=0, assetsfile=sf).deref()
        # A file with no parent raises for an external pointer, naming the record.
        orphan = load(text_asset_file(), env=UnityPy.Environment(path=str(self.tmp)))[0]
        orphan.externals, orphan.parent = [SimpleNamespace(path="archive:/CAB-x/CAB-x")], None
        with self.assertRaisesRegex(
            FileNotFoundError, "^PPtr points to archive:/CAB-x/CAB-x but no container is set!$"
        ):
            PPtr(m_FileID=1, m_PathID=1, assetsfile=orphan).deref()
        self.assertIs(PPtr(m_FileID=0, m_PathID=1, assetsfile=sf).deref(), sf.objects[1])
        with self.assertRaisesRegex(KeyError, "^99$"):
            PPtr(m_FileID=0, m_PathID=99, assetsfile=sf).deref()

        # parse_preload_table: an entry whose range starts below 0, has a size of 0 or less, or runs past the table
        # is skipped, and so is a null PPtr; a target's path is set only where its path_dict has none; the table is
        # read once.
        def table(entries, pptrs):
            rows = [
                (p, SimpleNamespace(asset=SimpleNamespace(path_id=9), preloadIndex=i, preloadSize=n))
                for p, i, n in entries
            ]
            return ContainerHelper(SimpleNamespace(m_Container=rows, m_PreloadTable=pptrs))

        own = PPtr(m_FileID=0, m_PathID=1, assetsfile=sf)
        paths = sf._container.path_dict
        paths.pop(1, None)
        helper = table(
            [("neg", -1, 1), ("zero", 0, 0), ("past", 1, 2), ("null", 1, 1), ("ok", 0, 1), ("later", 0, 1)],
            [own, PPtr(m_FileID=0, m_PathID=0, assetsfile=sf)],
        )
        helper.parse_preload_table()
        self.assertEqual((paths.get(1), helper._preload_table), ("ok", None))
        paths[1] = "kept"
        table([("x", 0, 1)], [own]).parse_preload_table()
        self.assertEqual(paths.pop(1), "kept")
        # Any other failure escapes: a target that is a raw reader (AttributeError), a loader OSError.
        load(b"\x01" * 30, name="raw.bin", env=sf.environment)
        (self.tmp / "dir.assets").mkdir()
        for external, error in (("raw.bin", AttributeError), ("dir.assets", IsADirectoryError)):
            sf.externals = [SimpleNamespace(path=external)]
            with self.subTest(external), self.assertRaises(error):
                table([("x", 0, 1)], [PPtr(m_FileID=1, m_PathID=1, assetsfile=sf)]).parse_preload_table()
        sf.externals = []
        # deref's upper guard: a file id past the externals raises FileNotFoundError, before the parent is looked at.
        sf.externals = [SimpleNamespace(path="a"), SimpleNamespace(path="b")]
        for file_id, file in ((3, sf), (2, orphan)):
            with (
                self.subTest(file_id=file_id),
                self.assertRaisesRegex(FileNotFoundError, "^Failed to resolve pointer - invalid m_FileID!$"),
            ):
                PPtr(m_FileID=file_id, m_PathID=1, assetsfile=file).deref()
        # What the loop or find_file finds is used whatever it holds: a None (a zip's own key) raises
        # FileNotFoundError naming the file id; a raw reader (by key, or a registry hit on a .resS or on CAB ""), a
        # BundleFile or a WebFile has no `objects` and raises AttributeError.
        env = sf.environment
        load(b"\x02" * 30, name="dir/CAB-r.resS", env=env)
        load(S.fs_bundle([(b"CAB-b", text_asset_file(), 4)]), name="b.bundle", env=env)
        load(S.web_file([(b"w.txt", b"web")]), name="w.data", env=env)
        quietly(env.load_file, zip_with_a_directory(), name="z.zip")
        self.assertEqual(env.files["z.zip"], None)
        for record, error, message in (
            ("archive:/CAB-x/RAW.bin", AttributeError, "'EndianBinaryReader_Memoryview' object has no attribute"),
            ("archive:/CAB-r/CAB-r.resS", AttributeError, "'EndianBinaryReader_Memoryview' object has no attribute"),
            ("assets/", AttributeError, "'EndianBinaryReader_Streamable' object has no attribute 'objects'$"),
            ("b.bundle", AttributeError, "^'BundleFile' object has no attribute 'objects'$"),
            ("w.data", AttributeError, "^'WebFile' object has no attribute 'objects'$"),
            ("z.zip", FileNotFoundError, "^Failed to resolve pointer - 1 not found!$"),
        ):
            sf.externals = [SimpleNamespace(path=record)]
            with self.subTest(record), self.assertRaisesRegex(error, message):
                quietly(PPtr(m_FileID=1, m_PathID=1, assetsfile=sf).deref)
        sf.externals = []

    # Section 9: object-mode attribute names.
    def test_object_mode_names_use_the_boost_rule(self):
        root = S.Node("Base", "Base", [S.Node("int", "²ab"), S.Node("int", "2cd")])
        sf = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, struct.pack("<ii", 1, 2))]))[0]
        value = sf.objects[1].parse_as_object()
        self.assertEqual((getattr(value, "²ab"), value.x2cd), (1, 2))
        node = TypeTreeNode(m_Level=0, m_Type="int", m_Name="²ab", m_ByteSize=4, m_Version=1)
        self.assertEqual(node._clean_name, "²ab", "UnityPyBoost: ASCII digits only")
        self.assertEqual((node_clean_name("²ab"), TypeTreeHelper.clean_name("²ab")), ("x²ab", "x²ab"))

    # Sections 9.2, 14.1, 19.2: how a class node becomes an object, on UnityPyBoost and on the pure-Python path.
    def test_object_mode_builds_classes_as_parse_class_does(self):
        def tree(root_type, fields):
            nodes = [{"m_Level": 0, "m_Type": root_type, "m_Name": "Base", "m_ByteSize": -1, "m_Version": 1}]
            nodes += [
                {"m_Level": 1, "m_Type": "int", "m_Name": name, "m_ByteSize": 4, "m_Version": 1} for name in fields
            ]
            return TypeTreeNode.from_list(nodes)

        def parse(root_type, fields, boost):
            data = struct.pack(f"<{len(fields)}i", *range(1, len(fields) + 1))
            size = len(data) if boost else None  # no byte_size: the pure-Python path
            reader = EndianBinaryReader(data, "<")
            return TypeTreeHelper.read_typetree(tree(root_type, fields), reader, False, size, check_read=boost)

        # One class of each kind section 9.2.1 lists: a non-empty `__slots__` and no __dict__; a non-empty
        # `__slots__` and the __dict__ Object gives; and `()` (found through the MRO) with a __dict__, which holds
        # both the classes attrs builds with slots=False and those it builds with slots=True and no fields on
        # Object (`GameManager`). test_object_mode.py checks the rule over every class.
        self.assertEqual(classes.AssetDatabaseMetrics.__bases__, (object,))
        self.assertTrue(classes.AssetDatabaseMetrics.__slots__)
        self.assertEqual(classes.MarshallingTestObject.__mro__[1:3], (classes.Object, ABC))
        self.assertTrue(classes.MarshallingTestObject.__slots__)
        for slotted in (classes.EditorSettings, classes.StreamingInfo, classes.Vector3f):
            self.assertTrue(slotted.__slots__, slotted)
        for unslotted in (classes.MultiplayerManager, classes.TextAsset, classes.Texture2D, classes.GameManager):
            self.assertEqual(unslotted.__slots__, (), unslotted)
        self.assertEqual((classes.GameManager.__bases__[0], attrs.fields(classes.GameManager)), (classes.Object, ()))

        metrics = ["totalAssetCount", "nonProAssetCount", "nonProAssetsCreatedAfterProLicense"]  # 1 required
        roles = ["m_ActiveMultiplayerRole", "m_ActiveMultiplayerRoles"]  # both optional
        self.assertEqual([f.default is attrs.NOTHING for f in attrs.fields(classes.AssetDatabaseMetrics)], [1, 0, 0])
        cases = {
            # (class, fields): (UnityPyBoost's result, the pure-Python path's)
            ("AssetDatabaseMetrics", (*metrics,)): ("AssetDatabaseMetrics", "AssetDatabaseMetrics"),
            # A missing optional field takes its default on both paths, a missing required one gives UnknownObject.
            ("AssetDatabaseMetrics", (metrics[0],)): ("AssetDatabaseMetrics", "AssetDatabaseMetrics"),
            ("AssetDatabaseMetrics", (*metrics[1:],)): ("UnknownObject", "UnknownObject"),
            # An extra field: Boost gives up where `__slots__` is a non-empty tuple; Python's setattr fails without a
            # __dict__.
            ("AssetDatabaseMetrics", (*metrics, "m_Extra")): ("UnknownObject", AttributeError),
            ("MarshallingTestObject", ("m_Prop", "m_Extra")): ("UnknownObject", "MarshallingTestObject"),
            # Where `__slots__` is `()`, Boost keeps the extra as an attribute; Python wants every annotation present.
            ("MultiplayerManager", (*roles, "m_Extra")): ("MultiplayerManager", "MultiplayerManager"),
            ("MultiplayerManager", ("m_Extra",)): ("MultiplayerManager", "UnknownObject"),
            # A field-less class attrs built with slots=True on Object has `()` too, so it keeps the extra.
            ("GameManager", ("m_Extra",)): ("GameManager", "GameManager"),
            ("AssetDatabaseMetrics", (metrics[0], "m_Extra")): ("UnknownObject", "UnknownObject"),
            # A type the module lacks.
            ("NoSuchClass", ("a",)): ("UnknownObject", "UnknownObject"),
        }
        for (root, fields), results in cases.items():
            for boost, expected in zip((True, False), results, strict=True):
                with self.subTest(root, fields=fields, boost=boost):
                    if expected is AttributeError:
                        with self.assertRaisesRegex(AttributeError, "object has no attribute 'm_Extra'"):
                            parse(root, fields, boost)
                        continue
                    value = parse(root, fields, boost)
                    self.assertEqual(type(value).__name__, expected)
                    self.assertEqual(
                        {name: getattr(value, name) for name in fields},
                        dict(zip(fields, range(1, len(fields) + 1), strict=True)),
                    )
                    if expected == "UnknownObject":
                        # Boost always keeps the node; the pure-Python path drops it for a type the module lacks.
                        kept = boost or root != "NoSuchClass"
                        self.assertEqual(value.get_type(), root if kept else None)
        # ObjectReader passes byte_size, so every object-mode parse of a loaded file goes through Boost.
        root = S.Node("AssetDatabaseMetrics", "Base", [S.Node("int", name) for name in [*metrics, "m_Extra"]])
        sf = load(S.serialized_file([S.Type(114, root)], [S.Obj(1, 0, struct.pack("<4i", 1, 2, 3, 4))]))[0]
        parsed = sf.objects[1].parse_as_object()
        self.assertEqual((type(parsed), parsed.get_type(), parsed.m_Extra), (UnknownObject, "AssetDatabaseMetrics", 4))

    # Section 18: BundleFile.save flags.
    def test_bundle_save_flags(self):
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        saved = bundle.save()
        again = load(saved)[0]
        self.assertEqual((int(again.dataflags), again._block_info_flags), (0x40, 0x40))
        empty = load(S.fs_bundle([]))[0]
        self.assertEqual(empty.files, {})
        with self.assertRaisesRegex(AttributeError, "_block_info_flags"):
            empty.save(packer="original")
        # A bundle with no blocks can still list entries: it holds no data, so each reads b"", whatever size the
        # directory states (an entry past the data reads short, section 13), and `original` raises for it too.
        listed = load(S.fs_bundle([(b"e.resS", b"", 0), (b"f.resS", 0, 16, 0)]))[0]
        self.assertEqual({name: bytes(f.bytes) for name, f in listed.files.items()}, {"e.resS": b"", "f.resS": b""})
        with self.assertRaisesRegex(AttributeError, "_block_info_flags"):
            listed.save(packer="original")

    # Sections 12, 18, 19.2: save_fs XORs the flag set's whole encryption mask out when any bit of it is set.
    def test_bundle_save_xors_the_whole_encryption_mask(self):
        def header_flags(data):
            end = data.index(b"\0") + 1 + 4  # signature, format
            end = data.index(b"\0", data.index(b"\0", end) + 1) + 1  # player and engine versions
            return struct.unpack_from(">I", data, end + 16)[0]  # after the size and the two blocks-info sizes

        def saved(engine, extra):
            bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], engine=engine))[0]
            bundle.dataflags = type(bundle.dataflags)(int(bundle.dataflags) | extra)
            return type(bundle.dataflags), bundle.save(packer="original")

        new, old = b"2021.3.2f1", b"2019.4.1f1"
        cases = {
            # ArchiveFlags' mask is 0x1400: one bit of it comes out as the other, which reads back as encrypted.
            (new, 0x1000): (ArchiveFlags, 0x0443, LookupError),
            (new, 0x0400): (ArchiveFlags, 0x1043, LookupError),
            (new, 0x1400): (ArchiveFlags, 0x0043, None),
            (new, 0x0200): (ArchiveFlags, 0x0243, None),  # BlockInfoNeedPaddingAtStart, kept
            # ArchiveFlagsOld's mask is 0x200 alone, which is cleared; its other bits are not encryption.
            (old, 0x0200): (ArchiveFlagsOld, 0x0043, None),
            (old, 0x1000): (ArchiveFlagsOld, 0x1043, None),
        }
        for (engine, extra), (flag_set, flags, error) in cases.items():
            with self.subTest(engine=engine, extra=hex(extra)):
                kind, data = saved(engine, extra)
                self.assertEqual((kind, hex(header_flags(data))), (flag_set, hex(flags)))
                if error is None:
                    self.assertEqual(list(load(data)[0].files), ["CAB-test"])
                else:
                    with self.assertRaisesRegex(error, "^The BundleFile is encrypted, but no key was provided"):
                        load(data)
        # The data blocks' own 0x100 bit is not in the mask, so `original` writes it back.
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        bundle._block_info_flags |= 0x100
        again = load(bundle.save(packer="original"))[0]
        self.assertEqual((again._block_info_flags, list(again.files)), (0x102, ["CAB-test"]))

    # Section 19.2: `GameObject.m_Components` raises on an empty list and on the old pair form.
    def test_game_object_components_raise(self):
        game_object = GameObject.__new__(GameObject)
        pointer = PPtr(m_FileID=0, m_PathID=5)
        object.__setattr__(game_object, "m_Component", [])
        with self.assertRaises(IndexError):
            _ = game_object.m_Components
        object.__setattr__(game_object, "m_Component", [(4, pointer)])
        with self.assertRaisesRegex(AttributeError, "m_GameObject"):
            _ = game_object.m_Components
        object.__setattr__(game_object, "m_Component", None)
        self.assertEqual(game_object.m_Components, [])

    # Sections 9.3, 10: the TPK's version lookup and TypeTreeNode.from_list.
    def test_tpk_version_lookup_and_from_list(self):
        items = [(UnityVersion.from_str("5.0.0f1"), "a"), (UnityVersion.from_str("2020.1.0f1"), "b")]
        self.assertEqual(Tpk.get_item_for_version(UnityVersion.from_str("2021.3.2f1"), items), "b")
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            Tpk.get_item_for_version(UnityVersion.from_str("4.0.0f1"), items)
        with self.assertRaisesRegex(ValueError, "^Could not find exact version$"):
            removed = [*items, (UnityVersion.from_str("2021.1.0f1"), None)]
            Tpk.get_item_for_version(UnityVersion.from_str("2021.3.2f1"), removed)
        rows = [{"m_Level": 0, "m_Type": "Base", "m_Name": "Base"}, {"m_Level": 1, "m_Type": "int", "m_Name": "a"}]
        node = TypeTreeNode.from_list(rows)
        self.assertEqual((node.m_ByteSize, node.m_Children[0].m_ByteSize), (0, 0))
        with self.assertRaises(TypeError):
            TypeTreeNode.from_list(
                [
                    {"m_Level": 0, "m_Type": "Base", "m_Name": "Base", "m_ByteSize": -1, "m_Version": 1},
                    {"m_Level": 1, "m_Type": "int", "m_Name": "a"},
                ]
            )

    # Sections 15.1, 18, 19.2: the BC7 bytes `set_image` should write.
    def test_bc7_set_image_reference_is_etcpak_default(self):
        image = Image.new("RGBA", (6, 5))
        image.putdata([(x * 40, y * 50, (x * y * 13) % 256, 255 - x * 30) for y in range(5) for x in range(6)])
        with self.assertRaisesRegex(ValueError, "^params must be an instance of BC7CompressBlockParams$"):
            Texture2DConverter.image_to_texture2d(image, TextureFormat.BC7)
        # The call UnityPy means to make: default parameters for its None.
        compress = etcpak.compress_bc7

        def with_defaults(data, width, height, params):
            self.assertIsNone(params)
            return compress(data, width, height, etcpak.BC7CompressBlockParams())

        with mock.patch.object(etcpak, "compress_bc7", with_defaults):
            written, tag = Texture2DConverter.image_to_texture2d(image, TextureFormat.BC7)
        self.assertEqual(tag, TextureFormat.BC7)
        # The data it hands etcpak: the image flipped, padded to whole 4x4 blocks, as RGBA.
        padded = Texture2DConverter.pad_image(image.transpose(Image.Transpose.FLIP_TOP_BOTTOM), 8, 8)
        reference = compress(padded.tobytes("raw", "RGBA"), 8, 8, etcpak.BC7CompressBlockParams())
        self.assertEqual(written, reference)
        self.assertEqual(compress(padded.tobytes("raw", "RGBA"), 8, 8, etcpak.BC7CompressBlockParams()), reference)
        decoded = Texture2DConverter.parse_image_data(written, 6, 5, TextureFormat.BC7, (2021, 3, 1, 3), 0)
        self.assertEqual((decoded.mode, decoded.size), ("RGBA", (6, 5)))
        # On a Switch-swizzled texture the image is padded to the swizzle's size, not to whole 4x4 blocks,
        # compressed, and the compressed bytes swizzled with BC7's 4x4 block and the blob's GOBs.
        blob = [0] * 8 + list((4).to_bytes(4, "little"))  # 1 << 4 = 16 GOBs per block
        with mock.patch.object(etcpak, "compress_bc7", with_defaults):
            switch, tag = Texture2DConverter.image_to_texture2d(image, TextureFormat.BC7, BuildTarget.Switch, blob)
        self.assertEqual(tag, TextureFormat.BC7)
        self.assertEqual(TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP[TextureFormat.BC7], (4, 4))
        width, height = TextureSwizzler.get_padded_texture_size(6, 5, 4, 4, 16)
        self.assertEqual((width, height), (16, 512))
        flipped = image.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
        linear = compress(
            Texture2DConverter.pad_image(flipped, width, height).tobytes("raw", "RGBA"),
            width,
            height,
            etcpak.BC7CompressBlockParams(),
        )
        self.assertEqual(switch, bytes(TextureSwizzler.swizzle(linear, width, height, 4, 4, 16)))
        self.assertEqual((len(switch), len(written)), (8192, 64))
        # The reader deswizzles it back: the same pixels as the linear write (its blocks hold the same texels).
        version = (2021, 3, 1, 3)
        back = Texture2DConverter.parse_image_data(switch, 6, 5, TextureFormat.BC7, version, BuildTarget.Switch, blob)
        self.assertEqual(list(back.getdata()), list(decoded.getdata()))

    # Sections 18, 19.2: `set_image` on a swizzled or byte-swapped platform.
    def test_set_image_writes_linear_mips_and_never_swaps_for_xbox(self):
        image = Image.new("RGBA", (64, 64))
        image.putdata([(x * 4, y * 4, 128, 255) for y in range(64) for x in range(64)])
        blob = [0] * 8 + list((4).to_bytes(4, "little"))  # 16 GOBs per block

        def texture(platform, blob=None):
            return SimpleNamespace(
                m_TextureFormat=TextureFormat.RGBA32,
                object_reader=SimpleNamespace(platform=platform),
                m_PlatformBlob=blob,
                m_MipMap=False,
                m_MipCount=1,
                m_StreamData=None,
            )

        switch = texture(BuildTarget.Switch, blob)
        Texture2D.set_image(switch, image, TextureFormat.RGBA32, 2)
        top, _ = Texture2DConverter.image_to_texture2d(image, TextureFormat.RGBA32, BuildTarget.Switch, blob)
        mip, _ = Texture2DConverter.image_to_texture2d(
            image.resize((32, 32), Image.Resampling.BICUBIC), TextureFormat.RGBA32
        )
        self.assertNotEqual(top, Texture2DConverter.image_to_texture2d(image, TextureFormat.RGBA32)[0], "swizzled")
        self.assertEqual(switch.image_data, top + mip, "a swizzled top level, then a linear mip")
        self.assertEqual((switch.m_MipCount, switch.m_MipMap), (2, True))
        # A swizzled mip would be padded to whole GOB blocks: four times the linear one here.
        swizzled_mip, _ = Texture2DConverter.image_to_texture2d(
            image.resize((32, 32), Image.Resampling.BICUBIC), TextureFormat.RGBA32, BuildTarget.Switch, blob
        )
        self.assertEqual((len(mip), len(swizzled_mip)), (4096, 16384))
        self.assertTrue(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, blob))
        # XBOX360: the reader swaps DXT1's byte pairs, the writer never does, so a written texture reads back wrong.
        xbox = texture(BuildTarget.XBOX360)
        Texture2D.set_image(xbox, image, TextureFormat.DXT1)
        self.assertEqual(xbox.image_data, Texture2DConverter.image_to_texture2d(image, TextureFormat.DXT1)[0])
        args = (xbox.image_data, 64, 64, TextureFormat.DXT1, (2021, 3, 1, 3))
        plain = Texture2DConverter.parse_image_data(*args, BuildTarget.StandaloneWindows64)
        swapped = Texture2DConverter.parse_image_data(*args, BuildTarget.XBOX360)
        self.assertNotEqual(list(swapped.getdata()), list(plain.getdata()))

    # Sections 15.1, 18, 18.1, 19.2: `set_image` packs the image in the mode it opened in.
    def test_set_image_packs_only_what_pillow_packs(self):
        image = Image.new("RGBA", (8, 8))
        image.putdata([(x * 30, y * 30, 7, 255 - x) for y in range(8) for x in range(8)])

        def texture():
            return SimpleNamespace(
                m_TextureFormat=TextureFormat.DXT1,
                object_reader=None,
                m_PlatformBlob=None,
                m_MipMap=False,
                m_MipCount=1,
                m_StreamData=None,
            )

        targets = {
            TextureFormat.DXT1: "RGBA",
            TextureFormat.RGB24: "RGB",
            TextureFormat.R8: "R",
            TextureFormat.Alpha8: "A",
        }
        # An indexed and a grey PNG, set from a path, keep the mode Image.open gives and raise for every target.
        for mode in ("P", "L"):
            path = self.tmp / f"{mode}.png"
            image.convert(mode).save(path)
            with Image.open(path) as opened:
                self.assertEqual(opened.mode, mode)
            for fmt, raw in targets.items():
                with self.subTest(mode, target=fmt.name):
                    with self.assertRaisesRegex(ValueError, f"^No packer found from {mode} to {raw}$"):
                        Texture2D.set_image(texture(), str(path), fmt)
                    # The reference: UnityPy's set_image of the image converted to RGBA, which packs.
                    converted = texture()
                    Texture2D.set_image(converted, Image.open(path).convert("RGBA"), fmt)
                    reference = Texture2DConverter.image_to_texture2d(Image.open(path).convert("RGBA"), fmt)[0]
                    self.assertEqual(converted.image_data, reference)
        # RGB raises for Alpha8 alone; converted, its alpha is 255.
        rgb = image.convert("RGB")
        with self.assertRaisesRegex(ValueError, "^No packer found from RGB to A$"):
            Texture2DConverter.image_to_texture2d(rgb, TextureFormat.Alpha8)
        for fmt in set(targets) - {TextureFormat.Alpha8}:
            Texture2DConverter.image_to_texture2d(rgb, fmt)
        self.assertEqual(
            Texture2DConverter.image_to_texture2d(rgb.convert("RGBA"), TextureFormat.Alpha8)[0], b"\xff" * 64
        )
        # LAB packs into Alpha8: Pillow's `A` band of LAB is its a* channel.
        lab = Image.new("LAB", (8, 8), (10, 20, 30))
        self.assertEqual(Texture2DConverter.image_to_texture2d(lab, TextureFormat.Alpha8)[0], bytes([20]) * 64)
        # Modes Pillow cannot convert to RGBA either: Tier U.
        for mode in ("BGR;15", "BGR;16", "BGR;24", "La"):
            with self.subTest(mode), self.assertRaisesRegex(ValueError, "^conversion from .* not supported$"):
                Image.new(mode, (2, 2)).convert("RGBA")

    # Sections 14.2, 19.2: Texture2DArray decodes every slice with platform 0 and no blob.
    def test_texture2darray_ignores_the_platform(self):
        self.assertIn("m_PlatformBlob", attrs.fields_dict(Texture2D))
        self.assertNotIn("m_PlatformBlob", attrs.fields_dict(Texture2DArray))
        data = bytes(range(64)) * 2  # two 4x4 RGBA32 slices
        seen = []
        parse = Texture2DConverter.parse_image_data

        def record(image_data, width, height, fmt, version, platform, blob=None, flip=True):
            seen.append((platform, blob))
            return parse(image_data, width, height, fmt, version, platform, blob, flip)

        for platform in (BuildTarget.Switch, BuildTarget.XBOX360):
            array = self.texture_array(GraphicsFormat.R8G8B8A8_UNorm, data, 4, 2, platform)
            with mock.patch.object(Texture2DConverter, "parse_image_data", record):
                images = Texture2DArray.images.fget(array)
            self.assertEqual(
                [image.tobytes() for image in images], [self.flipped_rgba(data[:64]), self.flipped_rgba(data[64:])]
            )
        self.assertEqual(seen, [(0, None)] * 4)

    @staticmethod
    def flipped_rgba(data):
        return Image.frombytes("RGBA", (4, 4), data).transpose(Image.Transpose.FLIP_TOP_BOTTOM).tobytes()

    def texture_array(self, graphics_format, data, width, depth, platform=0):
        return SimpleNamespace(
            m_Format=int(graphics_format),
            image_data=data,
            m_StreamData=None,
            m_DataSize=len(data),
            m_Depth=depth,
            m_Width=width,
            m_Height=width,
            object_reader=SimpleNamespace(version=(2021, 3, 2, 0), platform=platform, assets_file=None),
        )

    # Sections 14.2, 19.2: a streamed array's data is never read, and the reference is the same bytes held inline.
    def test_streamed_texture2darray_never_reads_its_resource(self):
        data = bytes(range(64)) * 2  # two 4x4 RGBA32 slices, as the .resS would hold them
        reads = []

        def get_resource_data(path, assets_file, offset, size):
            reads.append((path, offset, size))
            return data[offset : offset + size]

        streamed = self.texture_array(GraphicsFormat.R8G8B8A8_UNorm, b"", 4, 2)  # a parsed array's empty data
        streamed.m_StreamData = StreamingInfo(path="archive:/CAB-x/CAB-x.resS", offset=0, size=len(data))
        streamed.m_DataSize = len(data)
        with mock.patch.object(ResourceReader, "get_resource_data", get_resource_data):
            with self.assertRaisesRegex(ValueError, "^Texture2D has no image data$"):
                Texture2DArray.images.fget(streamed)
            self.assertEqual(reads, [], "`image_data` is b'', not None, so the .resS is never read")
            # The reference: the bytes the .resS holds, decoded as an array holding them inline decodes them.
            inline = Texture2DArray.images.fget(self.texture_array(GraphicsFormat.R8G8B8A8_UNorm, data, 4, 2))
            streamed.image_data = None  # what the fallback needs to fire
            read = Texture2DArray.images.fget(streamed)
        self.assertEqual(reads, [("archive:/CAB-x/CAB-x.resS", 0, len(data))])
        self.assertEqual([image.tobytes() for image in read], [image.tobytes() for image in inline])
        self.assertEqual(
            [image.tobytes() for image in inline], [self.flipped_rgba(data[:64]), self.flipped_rgba(data[64:])]
        )

    # Sections 14.2, 19.2: the 8-bit integer graphics formats map to 16-bit texture formats.
    def test_texture2darray_8bit_integer_formats(self):
        raises = {
            "R8_UInt": ValueError,  # R16
            "R8G8_UInt": KeyError,  # RG32
            "R8G8B8A8_UInt": ValueError,  # RGBA64
            "R8_SInt": ValueError,  # R16_SIGNED
            "R8G8_SInt": KeyError,  # RG32_SIGNED
            "R8G8B8_SInt": ValueError,  # RGB48_SIGNED
            "R8G8B8A8_SInt": ValueError,  # RGBA64_SIGNED
        }
        for name, error in raises.items():
            channels = name.count("8")
            with self.subTest(name), self.assertRaises(error):
                Texture2DArray.images.fget(self.texture_array(GraphicsFormat[name], bytes(16 * channels), 4, 1))
        # A value GraphicsFormat lacks raises ValueError; a graphics format the map lacks, NotImplementedError.
        from UnityPy.enums.GraphicsFormat import GRAPHICS_TO_TEXTURE_MAP

        unmapped = next(g for g in GraphicsFormat if g not in GRAPHICS_TO_TEXTURE_MAP)
        for value, error in ((99999, ValueError), (unmapped, NotImplementedError)):
            with self.subTest(value=value), self.assertRaises(error):
                Texture2DArray.images.fget(self.texture_array(value, bytes(64), 4, 1))
        # R8G8B8_UInt decodes as RGB48, Pillow's 5-6-5 `RGB;16`: not the stored bytes.
        data = bytes(range(48))
        image = Texture2DArray.images.fget(self.texture_array(GraphicsFormat.R8G8B8_UInt, data, 4, 1))[0]
        self.assertEqual(image.mode, "RGB")
        self.assertNotEqual(
            image.tobytes(), Image.frombytes("RGB", (4, 4), data).transpose(Image.Transpose.FLIP_TOP_BOTTOM).tobytes()
        )

    # Sections 15.7, 19.2: what Pillow saves under the container's extension.
    def test_pillow_saves_by_extension_and_mode(self):
        image = Image.new("RGBA", (4, 4), (1, 2, 3, 4))
        cases = [
            (".png", "RGBA", None),
            (".tga", "RGBA", None),
            (".jpg", "RGB", None),
            (".jpg", "RGBA", OSError),
            (".jpeg", "LA", OSError),
            (".psd", "RGBA", KeyError),
            (".xyz", "RGBA", ValueError),
            ("", "RGBA", ValueError),
        ]
        for extension, mode, error in cases:
            with self.subTest(extension=extension, mode=mode):
                path = self.tmp / f"image{extension}"
                if error is None:
                    image.convert(mode).save(path)
                    self.assertTrue(path.exists())
                else:
                    with self.assertRaises(error):
                        image.convert(mode).save(path)

    # Sections 15.7, 19.2: the names `export_obj(ref, fp, True, True)` gives, as exportGameObject means to call it.
    def test_export_obj_appends_none_for_the_path_id(self):
        sf = load(text_asset_file("hello", "world"), env=UnityPy.Environment(path=str(self.tmp)))[0]
        (self.tmp / "go").mkdir()  # exportGameObject makes its folder first
        exported = extractor.export_obj(PPtr(m_FileID=0, m_PathID=1, assetsfile=sf), str(self.tmp / "go"), True, True)
        self.assertEqual(exported, [(sf, 1)])
        self.assertEqual((self.tmp / "go" / "hello_None.txt").read_bytes(), b"world")

    # Section 19.2: with `_None` in place of the path id, objects of one name share one file in the crawl's folder.
    def test_export_obj_names_collide_without_the_path_id(self):
        objects = [S.Obj(1, 0, S.s32("") + S.s32("first")), S.Obj(2, 0, S.s32("") + S.s32("second"))]
        data = S.serialized_file([S.Type(49, TEXT_ASSET)], objects)
        sf = load(data, env=UnityPy.Environment(path=str(self.tmp)))[0]
        folder = self.tmp / "go"
        folder.mkdir()
        for path_id in (1, 2):  # two components with an empty m_Name, as most MonoBehaviours have
            pointer = PPtr(m_FileID=0, m_PathID=path_id, assetsfile=sf)
            self.assertEqual(extractor.export_obj(pointer, str(folder), True, True), [(sf, path_id)])
        # Both land on `{fp}/_None.txt`; the later overwrites the earlier.
        self.assertEqual(sorted(p.name for p in folder.iterdir()), ["_None.txt"])
        self.assertEqual((folder / "_None.txt").read_bytes(), b"second")
        # The names exportGameObject means to give them, with the path id appended, are distinct.
        self.assertEqual(len({f"_{reader.path_id}.txt" for reader in sf.objects.values()}), 2)

    # Sections 16, 19.2: an AudioClip with no data.
    def test_an_empty_audio_clip_follows_an_empty_resource_path(self):
        env = UnityPy.Environment(path=str(self.tmp))
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4), (b"CAB-a.resS", b"\x11" * 16, 0)]), env=env)[0]
        sf = bundle.files["CAB-a"]

        def clip(resource, audio_data=b""):
            return SimpleNamespace(
                m_AudioData=audio_data,
                m_Resource=resource,
                object_reader=SimpleNamespace(assets_file=sf),
                m_Name="clip",
                m_Channels=1,
                m_Frequency=44100,
            )

        empty = StreamedResource(m_Source="", m_Offset=0, m_Size=0)
        self.assertTrue(empty, "a StreamedResource is truthy, even empty")
        # The empty path is looked up, misses, is loaded relative to env.path, and opens the directory.
        for size in (0, 16):
            with self.subTest(size=size), self.assertRaises(IsADirectoryError):
                AudioClipConverter.extract_audioclip_samples(
                    clip(StreamedResource(m_Source="", m_Offset=0, m_Size=size))
                )
        # A real resource with no bytes reaches FMOD, which refuses it.
        source = StreamedResource(m_Source="archive:/CAB-a/CAB-a.resS", m_Offset=0, m_Size=0)
        with self.assertRaises(Exception) as raised:
            AudioClipConverter.extract_audioclip_samples(clip(source))
        self.assertEqual(type(raised.exception).__name__, "FmodError")
        # A zip's directory entry registers CAB "" (section 3): the empty path reads b"" from it, which FMOD refuses.
        quietly(env.load_file, zip_with_a_directory(), name="z.bin")
        for size in (0, 16):
            with self.subTest(cab="", size=size), self.assertRaises(Exception) as raised:
                AudioClipConverter.extract_audioclip_samples(
                    clip(StreamedResource(m_Source="", m_Offset=0, m_Size=size))
                )
            self.assertEqual(type(raised.exception).__name__, "FmodError")
        # Only a clip with no m_Resource at all gets the ValueError.
        with self.assertRaisesRegex(ValueError, "^AudioClip with neither m_AudioData nor m_Resource$"):
            AudioClipConverter.extract_audioclip_samples(clip(None))
        ogg = b"OggS" + bytes(12)
        self.assertEqual(AudioClipConverter.extract_audioclip_samples(clip(None, ogg)), {"clip.ogg": ogg})

    # Sections 3, 4, 19.2: a ZIP is found by content, and only bytes and buffered streams load.
    def test_a_zip_loads_from_bytes_and_raises_from_a_path(self):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("inner.assets", text_asset_file())
        data = bytearray(buffer.getvalue())
        self.assertEqual(
            ImportHelper.check_file_type(bytes(data))[0], FileType.ResourceFile, "zipfile's fifth byte is 20"
        )
        data[4] = 0  # "version needed to extract" 0: the NUL ends the signature at four bytes
        data = bytes(data)
        self.assertEqual(ImportHelper.check_file_type(data)[0], FileType.ZIP)
        env = UnityPy.Environment()
        self.assertIsNone(quietly(env.load_file, data, name="z.bin"))
        self.assertEqual(
            {name: type(f).__name__ for name, f in env.files.items()},
            {"inner.assets": "SerializedFile", "z.bin": "NoneType"},
        )
        path = self.tmp / "z.bin"
        path.write_bytes(data)
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.Environment().load_file, str(path))
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.load, str(path))
        with open(path, "rb") as stream:
            env = quietly(UnityPy.load, stream)
        self.assertEqual(sorted(type(f).__name__ for f in env.files.values()), ["NoneType", "SerializedFile"])
        # Every name namelist() gives loads, directory entries included (zip -r and shutil.make_archive write them):
        # each is an empty raw reader keyed by its name and registered as CAB "", the basename of a name ending in
        # "/", so the last one registered holds it.
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("assets/", b"")
            archive.writestr("assets/bin/", b"")
            archive.writestr("assets/bin/a.assets", text_asset_file("a"))
        data = bytearray(buffer.getvalue())
        data[4] = 0
        env = UnityPy.Environment()
        quietly(env.load_file, bytes(data), name="dirs.bin")
        self.assertEqual(
            zipfile.ZipFile(io.BytesIO(bytes(data))).namelist(), ["assets/", "assets/bin/", "assets/bin/a.assets"]
        )
        self.assertEqual(
            {name: type(f).__name__ for name, f in env.files.items()},
            {
                "assets/": "EndianBinaryReader_Streamable",
                "assets/bin/": "EndianBinaryReader_Streamable",
                "assets/bin/a.assets": "SerializedFile",
                "dirs.bin": "NoneType",
            },
        )
        self.assertEqual((len(env.files["assets/"].bytes), len(env.files["assets/bin/"].bytes)), (0, 0))
        self.assertEqual(list(env.cabs), ["", "a.assets"])
        self.assertIs(env.cabs[""], env.files["assets/bin/"])
        # load_zip_file takes bytes, a bytearray or a memoryview, read-only or writable (each in an io.BytesIO), an
        # io.BufferedIOBase (an io.BytesIO, the builtin open's io.BufferedReader) or a path string fs finds; anything
        # else raises TypeError: a path fs does not find, fsspec's LocalFileOpener, an int. Through load_file each of
        # the first five loads, with name= (a bytearray and a writable memoryview need it, the load_file row).
        zipped = zip_with_a_directory()
        (self.tmp / "dirs.zip").write_bytes(zipped)
        members = ["assets/", "assets/a.assets"]

        def given():
            yield from (zipped, bytearray(zipped), memoryview(zipped), memoryview(bytearray(zipped)))
            yield io.BytesIO(zipped)

        with open(self.tmp / "dirs.zip", "rb") as opened:
            for value in (*given(), opened, str(self.tmp / "dirs.zip")):
                with self.subTest(type(value).__name__):
                    env = UnityPy.Environment()
                    quietly(env.load_zip_file, value)
                    self.assertEqual(list(env.files), members)
        for value in given():
            with self.subTest(type(value).__name__, through="load_file"):
                env = UnityPy.Environment()
                self.assertIsNone(quietly(env.load_file, value, name="z.bin"))
                self.assertEqual(list(env.files), [*members, "z.bin"])
        with UnityPy.Environment().fs.open(str(self.tmp / "dirs.zip"), "rb") as local_opener:
            for value in (str(self.tmp / "missing.zip"), local_opener, 3):
                with self.subTest(type(value).__name__), self.assertRaisesRegex(TypeError, "^Unsupported type"):
                    UnityPy.Environment().load_zip_file(value)
        # A zip in a folder goes through detection from the stream the builtin open gives (load_files), which
        # load_zip_file accepts: one whose fifth byte is NUL loads its members, registers CAB "" and holds None under
        # its own path, where load_file of that path raises; Python's own zip (fifth byte 20) is a raw file.
        folder = self.tmp / "zips"
        folder.mkdir()
        (folder / "z.bin").write_bytes(zipped)
        (folder / "plain.zip").write_bytes(buffer.getvalue())
        self.assertEqual(buffer.getvalue()[4], 20)
        opened, real_open = [], open  # load_files never closes what it opens

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            opened.append(stream)
            return stream

        loaded = {
            "assets/": "EndianBinaryReader_Streamable",
            "assets/a.assets": "SerializedFile",
            str(folder / "z.bin"): "NoneType",
            str(folder / "plain.zip"): "EndianBinaryReader_Streamable",
        }
        for how in ("constructor", "load_folder", "load_files"):
            with self.subTest(how), mock.patch("builtins.open", tracked):
                if how == "constructor":
                    env = quietly(UnityPy.Environment, str(folder))
                else:
                    env = UnityPy.Environment()
                    if how == "load_folder":
                        quietly(env.load_folder, str(folder))
                    else:
                        quietly(env.load_files, [str(folder / "z.bin"), str(folder / "plain.zip")])
                self.assertEqual({name: type(f).__name__ for name, f in env.files.items()}, loaded)
                self.assertEqual(sorted(env.cabs), ["", "a.assets", "plain.zip"])
        for stream in opened:
            stream.close()
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            quietly(UnityPy.Environment().load_file, str(folder / "z.bin"))

    # Sections 2, 2.1: the package's aliases and every re-export section 2.1 lists.
    def test_package_aliases_and_reexports(self):
        from UnityPy.helpers import ArchiveStorageManager

        self.assertIs(UnityPy.load, UnityPy.Environment)
        self.assertIs(UnityPy.AssetsManager, UnityPy.Environment)
        self.assertIs(UnityPy.set_assetbundle_decrypt_key, ArchiveStorageManager.set_assetbundle_decrypt_key)
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        rows = next(body for header, body in inventory.tables(text) if header == ["package", "source", "members"])
        self.assertEqual(len(rows), 31)
        for package, _, members in rows:
            module = __import__(package.strip("`"), fromlist=["_"])
            for member in re.findall(r"`([A-Za-z_0-9]+)`", members):
                with self.subTest(package, member=member):
                    value = getattr(module, member)
                    self.assertFalse(isinstance(value, type(module)), "a re-export, not a submodule")
        self.assertIsInstance(UnityPy.files.BundleFile, type)
        self.assertTrue(callable(UnityPy.cli.update_tpk))
        # helpers and export re-export only their own submodules; export leaves ShaderConverter out.
        self.assertNotIn("ShaderConverter", UnityPy.export.__all__)
        self.assertEqual(len(UnityPy.helpers.__all__), 6)

    # Section 3: Environment.file, objects, assets, the CAB registry and find_file.
    def test_environment_registry_and_find_file(self):
        self.assertIsNone(UnityPy.Environment().typetree_generator)
        one = quietly(UnityPy.Environment, text_asset_file())
        self.assertIs(one.file, next(iter(one.files.values())), "set by the constructor for exactly one file")
        for env in (quietly(UnityPy.Environment, text_asset_file(), text_asset_file("b")), UnityPy.Environment()):
            self.assertFalse(hasattr(env, "file"))
        later = UnityPy.Environment()
        load(text_asset_file(), env=later)
        self.assertFalse(hasattr(later, "file"), "load_file never sets it")
        # Once set, it is never changed or removed: a later load_file leaves it on the first file.
        first_file = one.file
        load(text_asset_file("other"), env=one)
        self.assertEqual((one.file, len(one.files)), (first_file, 2))
        # It counts `files` entries, and two equal byte arguments share one key (their hash).
        twice = quietly(UnityPy.Environment, text_asset_file(), text_asset_file())
        self.assertEqual((len(twice.files), hasattr(twice, "file")), (1, True))
        # objects and assets: depth-first in `files` order, dependencies left out.
        env = UnityPy.Environment()
        plain = load(text_asset_file("plain"), "plain.assets", env)[0]
        quietly(env.load_file, text_asset_file("dependency"), name="dep.assets", is_dependency=True)
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file("in bundle"), 4), (b"CAB-a.resS", b"\x11" * 8, 0)]))[0]
        quietly(env.load_file, S.fs_bundle([(b"CAB-a", text_asset_file("in bundle"), 4)]), name="b")
        inner = env.files["b"].files["CAB-a"]
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["plain", "in bundle"])
        self.assertEqual(env.assets, [plain, inner])
        self.assertEqual(len(bundle.files), 2)
        # simplify_name is the lower-cased basename, extension kept; the registry is keyed by it.
        from UnityPy.environment import simplify_name

        self.assertEqual(simplify_name("dir\\Sub/CAB-AbC.resS"), "cab-abc.ress")
        env = UnityPy.Environment()
        first, second = object(), object()
        env._container_index_built = True
        env.register_cab("dir/Sub/CAB-AbC.resS", first)
        self.assertFalse(env._container_index_built, "registering invalidates the container index")
        self.assertIs(env.get_cab("other\\cab-abc.RESS"), first)
        env.register_cab("elsewhere/cab-abc.ress", second)
        self.assertEqual((list(env.cabs), env.get_cab("CAB-ABC.RESS")), (["cab-abc.ress"], second))
        self.assertIsNone(env.get_cab("missing"))
        # find_file: the registry, then path/name, then a walk of `path` built once.
        d = self.tmp / "d"
        (d / "sub").mkdir(parents=True)
        (d / "Shared.Assets").write_bytes(text_asset_file("shared"))
        (d / "sub" / "other.assets").write_bytes(text_asset_file("other"))
        (d / "sub" / "xab.assets").write_bytes(text_asset_file("xab"))

        def found(env, name):
            got = quietly(env.find_file, name)
            return got if got is None or got is first else [o.parse_as_dict()["m_Name"] for o in got.objects.values()]

        env = UnityPy.Environment(path=str(d))
        env.register_cab("CAB-registered", first)
        self.assertIs(found(env, "cab-REGISTERED"), first)
        self.assertEqual(found(env, "Shared.Assets"), ["shared"], "path/name")
        self.assertEqual(found(env, "other.assets"), ["other"], "a suffix of a walked path")
        self.assertEqual(found(env, "ab.assets"), ["xab"], "a suffix with no separator boundary")
        # A name in another case misses the exact matches (the simplified one compares a bare name with full
        # paths) and matches a simplified path by suffix: that path names no file, so load_file gives None.
        self.assertIsNone(found(UnityPy.Environment(path=str(d)), "shared.assets"))
        # When the basename on disk is all lower case, that simplified path is the file, so any case finds it.
        (d / "sub" / "lower.assets").write_bytes(text_asset_file("lower"))
        for name in ("LOWER.assets", "Lower.Assets", "sub/LOWER.ASSETS"):
            with self.subTest(name):
                self.assertEqual(found(UnityPy.Environment(path=str(d)), name), ["lower"])

        # PPtr.deref hands find_file the record's basename lower-cased, so a file whose basename on disk has an
        # upper-case letter is never found from disk, even by a record in its own case (section 19.2), while one
        # in lower case is found by a record in any case. A file already loaded is found through the registry.
        def deref(record, env):
            main = load(text_asset_file("main"), "main.assets", env)[0]
            main.externals = [SimpleNamespace(path=record)]
            return PPtr(m_FileID=1, m_PathID=1, assetsfile=main).deref().parse_as_dict()["m_Name"]

        for record in ("Shared.Assets", "archive:/x/Shared.Assets"):
            with self.subTest(record), self.assertRaisesRegex(FileNotFoundError, f"{re.escape(record)} not found!$"):
                deref(record, UnityPy.Environment(path=str(d)))
        for record in ("lower.assets", "assets/sub/LOWER.Assets", "archive:/CAB-x/Lower.ASSETS"):
            with self.subTest(record):
                self.assertEqual(deref(record, UnityPy.Environment(path=str(d))), "lower")
        # A file loaded by path is keyed by its full path, so deref's loop over `files` misses it and the registry
        # (find_file's first step, by CAB name) finds it; with its registry entry removed, deref raises.
        preloaded = UnityPy.Environment(path=str(d))
        quietly(preloaded.load_file, str(d / "Shared.Assets"))
        self.assertEqual((list(preloaded.files), list(preloaded.cabs)), ([str(d / "Shared.Assets")], ["shared.assets"]))
        self.assertEqual(deref("Shared.Assets", preloaded), "shared")
        preloaded.cabs.pop("shared.assets")
        with self.assertRaisesRegex(FileNotFoundError, "Shared.Assets not found!$"):
            deref("Shared.Assets", preloaded)
        # The loop compares each key lower-cased (`key.lower() ==`), so a key in any case is found by name.
        keyed = UnityPy.Environment(path=str(self.tmp))
        load(text_asset_file("keyed"), "Shared.Assets", keyed)
        keyed.cabs.pop("shared.assets")
        self.assertEqual(deref("archive:/CAB-x/SHARED.assets", keyed), "keyed")
        # The suffix steps answer a name even where the file it names exists: beside sub/ab.assets, a root
        # xab.assets answers ab.assets (the walk lists a folder's files before its subfolders), so a record in any
        # case reads the wrong file (Tier C, section 19.2). As `path/name`, sub/ab.assets is found.
        f = self.tmp / "f"
        (f / "sub").mkdir(parents=True)
        (f / "xab.assets").write_bytes(text_asset_file("xab"))
        (f / "sub" / "ab.assets").write_bytes(text_asset_file("ab"))
        self.assertEqual(found(UnityPy.Environment(path=str(f)), "ab.assets"), ["xab"])
        self.assertEqual(found(UnityPy.Environment(path=str(f)), "sub/ab.assets"), ["ab"])
        for record in ("ab.assets", "archive:/CAB-x/ab.assets", "AB.assets"):
            with self.subTest(record):
                self.assertEqual(deref(record, UnityPy.Environment(path=str(f))), "xab")
        # Without xab.assets these records read sub/ab.assets: deref hands find_file the last component lower-cased,
        # the name the Tier C reference matches at a separator, so `AB.assets`'s reference is sub/ab.assets.
        (f / "xab.assets").unlink()
        for record in ("ab.assets", "archive:/CAB-x/ab.assets", "AB.assets"):
            with self.subTest(record, without="xab.assets"):
                self.assertEqual(deref(record, UnityPy.Environment(path=str(f))), "ab")
        # The Tier B retry matches the exact basename, not find_file's suffix match: beside sub/Shared.Assets,
        # find_file("Shared.Assets") loads MyShared.Assets, the walk listing a folder's files first.
        e = self.tmp / "e"
        (e / "sub").mkdir(parents=True)
        (e / "MyShared.Assets").write_bytes(text_asset_file("my-shared"))
        (e / "sub" / "Shared.Assets").write_bytes(text_asset_file("sub-shared"))
        self.assertEqual(found(UnityPy.Environment(path=str(e)), "Shared.Assets"), ["my-shared"])
        with self.assertRaisesRegex(FileNotFoundError, "Shared.Assets not found!$"):
            deref("Shared.Assets", UnityPy.Environment(path=str(e)))
        # find_file's walk is section 3's (os.scandir order, each folder's files first): its list of paths, and the
        # first of two same-named files in it answers the name.
        tree = walk_tree(self.tmp)
        walking = UnityPy.Environment(path=str(tree))
        self.assertEqual(found(walking, "z.assets"), ["s1-inner-z"])
        order = scandir_walk(tree)
        self.assertEqual(walking.local_files, order)
        first_walked = min(("s1", "s2"), key=lambda s: order.index(str(tree / s / "ab.assets")))
        self.assertEqual(found(UnityPy.Environment(path=str(tree)), "ab.assets"), [f"{first_walked}-ab"])
        (d / "sub" / "lower.assets").unlink()
        with self.assertRaisesRegex(FileNotFoundError, "File missing.assets not found in"):
            found(env, "missing.assets")
        (d / "sub" / "late.assets").write_bytes(text_asset_file("late"))
        with self.assertRaises(FileNotFoundError):
            found(env, "late.assets")  # the walk is not built again
        self.assertEqual(found(UnityPy.Environment(path=str(d)), "late.assets"), ["late"])

    # Sections 3, 5.2: split files, folders, Environment.save and File's own attributes.
    def test_split_files_folders_and_save(self):
        from fsspec.implementations.memory import MemoryFileSystem
        from UnityPy.environment import reSplit
        from UnityPy.files import File

        self.assertEqual(reSplit.pattern, r"(.*?([^\/\\]+?))\.split\d+")
        self.assertEqual(reSplit.match("a/b.c.split12").groups(), ("a/b.c", "b.c"))
        # re.match anchors only the start: `.split` and digits anywhere make the path a split part.
        self.assertEqual(reSplit.match("d/a.assets.split0.bak").groups(), ("d/a.assets", "a.assets"))
        self.assertEqual(reSplit.match("d/dir.split1/inner.assets").groups(), ("d/dir", "dir"))
        d = self.tmp / "d"
        (d / "sub").mkdir(parents=True)
        data = text_asset_file("split")
        half = len(data) // 2
        (d / "big.split0").write_bytes(data[:half])
        (d / "big.split1").write_bytes(data[half:])
        (d / "big.split3").write_bytes(b"after a gap")
        (d / "nozero.split1").write_bytes(b"one")
        (d / "nozero.split2").write_bytes(b"two")
        env = UnityPy.Environment(path=str(d))
        # Up to the first missing part after split0; a missing split0 is skipped.
        self.assertEqual(env._load_split_file(str(d / "big")), data)
        self.assertEqual(env._load_split_file(str(d / "nozero")), b"onetwo")
        loaded = quietly(env.load_file, str(d / "big.split1"))
        self.assertEqual(
            (list(env.files), [o.parse_as_dict()["m_Name"] for o in loaded.objects.values()]),
            ([str(d / "big")], ["split"]),
        )
        # At most 999 parts, split0 to split998.
        fs = MemoryFileSystem()
        root = f"/{self.tmp.name}-parts"
        self.addCleanup(fs.rm, root, recursive=True)
        for i in range(1001):
            fs.pipe(f"{root}/p.split{i}", b"x")
        self.assertEqual(len(UnityPy.Environment(fs=fs)._load_split_file(f"{root}/p")), 999)
        # load_files (load_assets with the builtin open) merges split members once per base path. UnityPy never
        # closes what it opens there, so the files are closed here.
        (d / "sub" / "plain.assets").write_bytes(text_asset_file("plain"))
        opened, real_open = [], open  # the real one, taken before builtins.open is patched to `tracked`

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            opened.append(stream)
            return stream

        env = UnityPy.Environment()
        with mock.patch("builtins.open", tracked):
            quietly(env.load_files, [str(d / "big.split0"), str(d / "big.split1"), str(d / "sub" / "plain.assets")])
        self.assertEqual(list(env.files), [str(d / "big"), str(d / "sub" / "plain.assets")])
        # A path holding `.splitN` before its end: `a.assets.split0.bak` loads the merged parts and never reads
        # itself, and `dir.split1/inner.assets` opens the folder `dir.split1` as a part (section 19.2).
        whole = text_asset_file("merged")
        (d / "a.assets.split0").write_bytes(whole[:40])
        (d / "a.assets.split1").write_bytes(whole[40:])
        (d / "a.assets.split0.bak").write_bytes(text_asset_file("bak"))
        (d / "dir.split1").mkdir()
        (d / "dir.split1" / "inner.assets").write_bytes(text_asset_file("inner"))
        env = UnityPy.Environment()
        loaded = quietly(env.load_file, str(d / "a.assets.split0.bak"))
        self.assertEqual(
            (list(env.files), [o.parse_as_dict()["m_Name"] for o in loaded.objects.values()]),
            ([str(d / "a.assets")], ["merged"]),
        )
        with self.assertRaisesRegex(IsADirectoryError, re.escape(str(d / "dir.split1"))):
            quietly(UnityPy.Environment().load_file, str(d / "dir.split1" / "inner.assets"))
        # Split members of a zip: load_assets reads the parts through Environment.fs, relative to the cwd, never
        # from the zip. With no such files the member loads as an empty raw reader, a CAB, and its objects are
        # lost; with files at those paths, their bytes load in its place (section 19.2).
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("assets/bin/Data/s.assets.split0", whole[:40])
            archive.writestr("assets/bin/Data/s.assets.split1", whole[40:])
            archive.writestr("assets/bin/Data/plain.assets", text_asset_file("plain"))
        apk = self.tmp / "app.apk"
        apk.write_bytes(buffer.getvalue())
        cwd = self.tmp / "cwd"
        cwd.mkdir()
        self.chdir(cwd)

        def from_zip():
            from_bytes = UnityPy.Environment()
            quietly(from_bytes.load_zip_file, buffer.getvalue())
            for env in (from_bytes, quietly(UnityPy.Environment, str(apk))):
                member = env.files["assets/bin/Data/s.assets"]
                yield (
                    list(env.files),
                    sorted(env.cabs),
                    type(member).__name__,
                    sorted(o.parse_as_dict()["m_Name"] for o in env.objects),
                )

        keys = ["assets/bin/Data/s.assets", "assets/bin/Data/plain.assets"]
        for got in from_zip():
            self.assertEqual(got[:2], (keys, ["plain.assets", "s.assets"]))
            self.assertEqual(got[2:], ("EndianBinaryReader_Memoryview", ["plain"]))
        self.assertEqual(len(UnityPy.Environment()._load_split_file("assets/bin/Data/s.assets")), 0)
        (cwd / "assets" / "bin" / "Data").mkdir(parents=True)
        other = text_asset_file("from the disk")
        (cwd / "assets" / "bin" / "Data" / "s.assets.split0").write_bytes(other[:30])
        (cwd / "assets" / "bin" / "Data" / "s.assets.split1").write_bytes(other[30:])
        for got in from_zip():
            self.assertEqual(got[2:], ("SerializedFile", ["from the disk", "plain"]))
        # load_folder: every file fs.walk finds, in its order, through load_files.
        folder = self.tmp / "folder"
        (folder / "b").mkdir(parents=True)
        for name in ("z.assets", "a.assets", "b/m.assets"):
            (folder / name).write_bytes(text_asset_file(name))
        env = UnityPy.Environment()
        with mock.patch("builtins.open", tracked):
            quietly(env.load_folder, str(folder))
        for stream in opened:
            stream.close()
        walked = ["/".join([root, f]) for root, _, files in env.fs.walk(str(folder)) for f in files]
        self.assertEqual(list(env.files), walked)
        self.assertEqual(sorted(walked), sorted(str(folder / n) for n in ("z.assets", "a.assets", "b/m.assets")))
        # The walk's order is os.scandir's, unsorted, each folder's files before its subfolders, depth-first
        # (section 3, the walk): `files` follows it, and of two same-named files the later one holds the CAB.
        tree = walk_tree(self.tmp)
        env = UnityPy.Environment()
        with mock.patch("builtins.open", tracked):
            quietly(env.load_folder, str(tree))
        for stream in opened:
            stream.close()
        order = scandir_walk(tree)
        self.assertEqual(list(env.files), order)
        later = max(str(tree / "s1" / "ab.assets"), str(tree / "s2" / "ab.assets"), key=order.index)
        self.assertIs(env.cabs["ab.assets"], env.files[later])
        # File: its attributes; mark_changed marks every File above; save writes the changed ones by basename.
        env = UnityPy.Environment()
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4)]), "dir/b.bundle", env)[0]
        sf = bundle.files["CAB-a"]
        self.assertEqual(
            (bundle.name, bundle.cab_file, bundle.is_changed, bundle.parent, bundle.environment),
            ("b.bundle", "CAB-UnityPy_Mod.resS", False, env, env),
        )
        self.assertEqual((sf.name, sf.parent, sf.environment), ("CAB-a", bundle, env))
        self.assertTrue(isinstance(bundle, File) and isinstance(sf, File))
        sf.mark_changed()
        self.assertEqual((sf.is_changed, bundle.is_changed), (True, True))
        out = self.tmp / "out"
        out.mkdir()
        load(text_asset_file("unchanged"), "u.assets", env)
        quietly(env.save, out_path=str(out))
        self.assertEqual(sorted(p.name for p in out.iterdir()), ["b.bundle"])
        self.assertEqual((out / "b.bundle").read_bytes(), bundle.save(packer="none"))
        # File.container: the union of its File children's containers.
        info = SimpleNamespace(asset=PPtr(m_FileID=0, m_PathID=7), preloadIndex=0, preloadSize=0)
        sf._container = ContainerHelper([("a/path", info), ("b/path", info), ("a/path", info)])
        self.assertEqual(bundle.container, {"a/path": info.asset, "b/path": info.asset})
        self.assertEqual((sorted(sf.container.keys()), sf.container.values()), (["a/path", "b/path"], [info.asset]))

    # Section 3, the walk: the paths it yields, links, and folders it cannot list.
    def test_the_walk_yields_absolute_paths_and_takes_links_for_files(self):
        opened, real_open = [], open  # load_files never closes what it opens

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            opened.append(stream)
            return stream

        self.addCleanup(lambda: [stream.close() for stream in opened])
        patched = mock.patch("builtins.open", tracked)
        patched.start()
        self.addCleanup(patched.stop)

        def keys(folder):
            env = UnityPy.Environment()
            quietly(env.load_folder, folder)
            return list(env.files)

        # Absolute posix paths, whatever form the folder takes: joined to the cwd, `./` and a trailing `/` dropped,
        # `~` expanded, `..` kept. `files` holds them, find_file's walk lists them, and `path` stays as given.
        work = self.tmp / "work"
        (work / "rel" / "sub").mkdir(parents=True)
        (work / "rel" / "a.bin").write_bytes(text_asset_file("a"))
        (work / "rel" / "sub" / "b.bin").write_bytes(text_asset_file("b"))
        self.chdir(work)
        cwd = os.getcwd()
        absolute = [f"{cwd}/rel/a.bin", f"{cwd}/rel/sub/b.bin"]
        for folder in ("rel", "./rel", "rel/", f"{cwd}/rel"):
            with self.subTest(folder):
                self.assertEqual(keys(folder), absolute)
        self.assertEqual(keys("rel/../rel"), [f"{cwd}/rel/../rel/a.bin", f"{cwd}/rel/../rel/sub/b.bin"])
        with mock.patch.dict(os.environ, {"HOME": cwd}):
            self.assertEqual(keys("~/rel"), absolute)
        env = quietly(UnityPy.Environment, "rel")
        self.assertEqual((env.path, list(env.files), sorted(env.cabs)), ("rel", absolute, ["a.bin", "b.bin"]))
        env = UnityPy.Environment(path="rel")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in quietly(env.find_file, "b.bin").objects.values()], ["b"])
        self.assertEqual(env.local_files, absolute)
        # Links are files: a link to a folder is never entered and raises when opened, a broken one raises, a link to
        # a file loads under its own path and name. find_file reaches a file under a linked folder only as path/name.
        real = self.tmp / "real"
        real.mkdir()
        (real / "r.assets").write_bytes(text_asset_file("r"))
        (self.tmp / "target.assets").write_bytes(text_asset_file("t"))
        linked = {}
        for case, target in (("folder", real), ("broken", self.tmp / "nothing"), ("file", self.tmp / "target.assets")):
            linked[case] = self.tmp / f"{case}-link"
            linked[case].mkdir()
            (linked[case] / "Link.assets").symlink_to(target)
            walk = list(UnityPy.Environment().fs.walk(str(linked[case])))
            self.assertEqual(walk, [(str(linked[case]), [], ["Link.assets"])], case)
        with self.assertRaisesRegex(IsADirectoryError, re.escape(str(linked["folder"] / "Link.assets"))):
            keys(str(linked["folder"]))
        with self.assertRaisesRegex(FileNotFoundError, re.escape(str(linked["broken"] / "Link.assets"))):
            keys(str(linked["broken"]))
        env = UnityPy.Environment()
        quietly(env.load_folder, str(linked["file"]))
        self.assertEqual((list(env.files), list(env.cabs)), ([str(linked["file"] / "Link.assets")], ["link.assets"]))
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["t"])
        with self.assertRaisesRegex(FileNotFoundError, "^File r.assets not found in "):
            quietly(UnityPy.Environment(path=str(linked["folder"])).find_file, "r.assets")
        found = quietly(UnityPy.Environment(path=str(linked["folder"])).find_file, "Link.assets/r.assets")
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in found.objects.values()], ["r"])
        # A folder the walk cannot list yields nothing, with no error: a missing one, an unreadable subfolder (which
        # only a user other than root cannot read). A file walks as itself under "", and opening path/ raises.
        self.assertEqual(keys(str(self.tmp / "missing")), [])
        perm = self.tmp / "perm"
        (perm / "locked").mkdir(parents=True)
        (perm / "ok.bin").write_bytes(text_asset_file("ok"))
        (perm / "locked" / "x.bin").write_bytes(text_asset_file("x"))
        os.chmod(perm / "locked", 0)
        self.addCleanup(os.chmod, perm / "locked", 0o755)
        readable = os.access(perm / "locked", os.R_OK)
        self.assertEqual(keys(str(perm)), [str(perm / "ok.bin"), *([str(perm / "locked" / "x.bin")] * readable)])
        with self.assertRaisesRegex(NotADirectoryError, re.escape(str(perm / "ok.bin") + "/")):
            keys(str(perm / "ok.bin"))

    # Sections 5.2, 19.2: File's iteration and dict-style methods.
    def test_file_iteration_and_dict_access(self):
        def two(name):
            objects = [S.Obj(1, 0, S.s32(name) + S.s32("s")), S.Obj(2, 0, S.s32(f"{name}2") + S.s32("s"))]
            return S.serialized_file([S.Type(49, TEXT_ASSET)], objects)

        bundle = load(S.fs_bundle([(b"CAB-a", two("a"), 4), (b"CAB-a.resS", b"\x11" * 16, 0)]), "b")[0]
        sf, raw = bundle.files["CAB-a"], bundle.files["CAB-a.resS"]
        self.assertIsInstance(raw, EndianBinaryReader)
        text_asset = [ClassIDType.TextAsset]
        # A raw reader is skipped: the bundle's objects are the SerializedFile's two.
        self.assertEqual(list(bundle.get_objects()), list(sf.objects.values()))
        self.assertEqual(list(bundle.get_assets()), [sf])
        self.assertEqual([o.path_id for o in bundle.get_filtered_objects(text_asset)], [1, 2])
        self.assertEqual(list(bundle.get_filtered_objects([ClassIDType.Texture2D])), [])
        # With no types, `return self.get_objects()` ends the generator: nothing.
        self.assertEqual((list(bundle.get_filtered_objects()), list(bundle.get_filtered_objects([]))), ([], []))
        # A SerializedFile's `files` is its objects: get_objects yields them through the ObjectReader branch,
        # get_filtered_objects takes neither branch, and get_assets' `return self` ends its generator.
        self.assertIs(sf.files, sf.objects)
        self.assertEqual(list(sf.get_objects()), list(sf.objects.values()))
        self.assertEqual((list(sf.get_filtered_objects(text_asset)), list(sf.get_assets())), ([], []))
        # A nested container is iterated as `f.objects`, which BundleFile and WebFile lack.
        inner = S.fs_bundle([(b"CAB-i", two("i"), 4)])
        nested = load(S.fs_bundle([(b"inner", inner, 4), (b"CAB-o", two("o"), 4)]), "n")[0]
        web = load(S.web_file([(b"inner", inner)]), "w")[0]
        self.assertIsInstance(web.files["inner"], BundleFile)
        for container in (nested, web):
            for call in (container.get_objects, lambda c=container: c.get_filtered_objects(text_asset)):
                with self.assertRaisesRegex(AttributeError, "'BundleFile' object has no attribute 'objects'"):
                    list(call())
        # get_assets recurses: the reference those raises are checked against.
        self.assertEqual([f.name for f in nested.get_assets()], ["CAB-i", "CAB-o"])
        # `get` is an attribute lookup; keys, items, values and [] are those of `files`.
        self.assertEqual((bundle.get("CAB-a"), bundle.get("nope", 5)), (None, 5))
        self.assertIs(bundle.get("files"), bundle.files)
        self.assertEqual(list(bundle.keys()), ["CAB-a", "CAB-a.resS"])
        self.assertEqual(list(bundle.values()), [sf, raw])
        self.assertEqual(list(bundle.items()), [("CAB-a", sf), ("CAB-a.resS", raw)])
        self.assertIs(bundle["CAB-a"], sf)
        self.assertEqual((list(sf.keys()), sf[2]), ([1, 2], sf.objects[2]))

    # Section 6: the codec helpers and tables.
    def test_compression_helpers(self):
        import lzma

        import brotli
        from UnityPy.enums import CompressionFlags as C
        from UnityPy.helpers import CompressionHelper as H

        data = bytes(range(256)) * 700  # 175 KiB, compressible
        self.assertEqual(sorted(H.DECOMPRESSION_MAP), [C.NONE, C.LZMA, C.LZ4, C.LZ4HC, C.LZHAM])
        self.assertEqual(sorted(H.COMPRESSION_MAP), [C.NONE, C.LZMA, C.LZ4, C.LZ4HC])
        self.assertEqual(H.COMPRESSION_MAP[C.LZ4HC], H.compress_lz4)
        self.assertEqual(H.DECOMPRESSION_MAP[C.LZ4HC], H.decompress_lz4)
        self.assertEqual(
            H.COMPRESSION_CHUNK_SIZE_MAP, {C.NONE: 0xFFFFFFFF, C.LZMA: 0xFFFFFFFF, C.LZ4: 0x20000, C.LZ4HC: 0x20000}
        )
        self.assertEqual((H.GZIP_MAGIC, H.BROTLI_MAGIC), (b"\x1f\x8b", b"brotli"))
        # LZHAM and any other codec value.
        with self.assertRaisesRegex(NotImplementedError, "LZHAM"):
            H.decompress_lzham(b"x", 1)
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        with self.assertRaisesRegex(NotImplementedError, "LZHAM"):
            bundle.decompress_data(b"x", 1, 4)
        for codec in (5, 6, 7, 8, 63, 0x40 | 9):
            with self.subTest(codec=codec), self.assertRaisesRegex(ValueError, "^Unknown compression"):
                bundle.decompress_data(b"x", 1, codec)
        # LZMA: props 0x5D, an 8 MiB dictionary, lc 3 lp 0 pb 2, BT4, nice_len 123, an optional u64 size.
        filters = [
            {"id": lzma.FILTER_LZMA1, "dict_size": 1 << 23, "lc": 3, "lp": 0, "pb": 2, "mf": lzma.MF_BT4}
            | {"mode": lzma.MODE_NORMAL, "nice_len": 123}
        ]
        raw = lzma.compress(data, format=lzma.FORMAT_RAW, filters=filters)
        self.assertEqual(H.compress_lzma(data), b"\x5d" + (1 << 23).to_bytes(4, "little") + raw)
        self.assertEqual(H.compress_lzma(data, True), H.compress_lzma(data)[:5] + struct.pack("<Q", len(data)) + raw)
        self.assertEqual(H.decompress_lzma(H.compress_lzma(data)), data)
        self.assertEqual(H.decompress_lzma(H.compress_lzma(data, True), True), data)
        # LZ4: high compression at level 9, no size prefix.
        self.assertEqual(
            H.compress_lz4(data), lz4.block.compress(data, mode="high_compression", compression=9, store_size=False)
        )
        # brotli at the library's default quality, 11; gzip at level 9 with the time as the header's mtime.
        self.assertEqual(H.compress_brotli(data), brotli.compress(data, quality=11))
        with mock.patch("time.time", return_value=1000.0):
            first = H.compress_gzip(data)
        with mock.patch("time.time", return_value=2000.0):
            second = H.compress_gzip(data)
        self.assertEqual((first[4:8], second[4:8]), (struct.pack("<I", 1000), struct.pack("<I", 2000)))
        self.assertEqual((first[8], first[10:], H.decompress_gzip(first)), (2, second[10:], data))  # XFL 2: level 9
        # chunk_based_compress: LZ4 in 128 KiB chunks, LZMA and none in one; a chunk that grows keeps its bytes
        # with the codec bits cleared.
        packed, blocks = H.chunk_based_compress(data, 0x42)
        self.assertEqual([(size, flag) for size, _, flag in blocks], [(0x20000, 0x42), (len(data) - 0x20000, 0x42)])
        self.assertEqual(lz4.block.decompress(packed[: blocks[0][1]], 0x20000), data[:0x20000])
        self.assertEqual([(u, f) for u, _, f in H.chunk_based_compress(data, 1)[1]], [(len(data), 1)])
        self.assertEqual(H.chunk_based_compress(data, 0x40), (data, [(len(data), len(data), 0x40)]))
        noise = random.Random(3).randbytes(1000)
        self.assertEqual(H.chunk_based_compress(noise, 0x42), (noise, [(1000, 1000, 0x40)]))
        # `none` is one block whatever the payload, an empty one included; a compressing codec writes no block for
        # an empty payload. Only a chunk that grows is stored: one that compresses to exactly its own size stays
        # compressed, as a whole chunk and as the last part.
        self.assertEqual(H.chunk_based_compress(b"", 0x40), (b"", [(0, 0, 0x40)]))
        for flag in (1, 2, 3):
            self.assertEqual(H.chunk_based_compress(b"", flag), (b"", []))
        exact = random.Random(5).randbytes(130548) + bytes(0x20000 - 130548)
        self.assertEqual(len(H.compress_lz4(exact)), 0x20000)
        self.assertEqual([f for _, _, f in H.chunk_based_compress(exact, 3)[1]], [3])
        self.assertEqual([f for _, _, f in H.chunk_based_compress(exact + b"x", 3)[1]], [3, 0])
        with self.assertRaisesRegex(NotImplementedError, "No compression function"):
            H.chunk_based_compress(data, 4)

    # Sections 5.2, 6: WebFile sniffs gzip at offset 0 and brotli at 0x20, given a reader at its start.
    def test_webfile_sniffs_its_codec(self):
        from UnityPy.helpers import CompressionHelper as H

        plain = S.web_file([(b"a.txt", b"hello")])
        gz = WebFile(EndianBinaryReader(gzip.compress(plain)), None)
        self.assertEqual((gz.packer, list(gz.files)), ("gzip", ["a.txt"]))
        marked = bytes(0x20) + b"brotli" + bytes(10)
        with mock.patch.object(H, "decompress_brotli", lambda data: plain) as stub:
            br = WebFile(EndianBinaryReader(marked), None)
        self.assertEqual((br.packer, list(br.files), stub(b"")), ("brotli", ["a.txt"], plain))
        self.assertEqual(WebFile(EndianBinaryReader(plain), None).packer, "none")

    # Sections 10, 11: the fallback version, the version exceptions and UnityVersion's string form.
    def test_fallback_version_and_version_strings(self):
        from UnityPy import config
        from UnityPy.exceptions import UnityVersionFallbackError, UnityVersionFallbackWarning

        self.assertTrue(issubclass(UnityVersionFallbackError, Exception))
        self.assertTrue(issubclass(UnityVersionFallbackWarning, UserWarning))
        for value in (None, 2019):
            with (
                self.subTest(value=value),
                mock.patch.object(config, "FALLBACK_UNITY_VERSION", value),
                self.assertRaises(UnityVersionFallbackError),
            ):
                config.get_fallback_version()
        # The `once` filter UnityPy installs at import: one warning for three calls from three places, in a process
        # of its own because a test runner replaces the warning filters.
        code = (
            "import UnityPy.config as c\n"
            "c.FALLBACK_UNITY_VERSION = '2019.4.1f1'\n"
            "a = c.get_fallback_version()\n"
            "b = c.get_fallback_version()\n"
            "d = c.get_fallback_version()\n"
            "print(a, b, d)\n"
        )
        run = __import__("subprocess").run([sys.executable, "-B", "-c", code], capture_output=True, text=True)
        self.assertEqual(run.stdout, "2019.4.1f1 2019.4.1f1 2019.4.1f1\n")
        self.assertEqual(run.stderr.count("UnityVersionFallbackWarning"), 1, run.stderr)
        # str(): above 5.x the build and its dot go; 5.x and below the type and its number; the postfix stays.
        cases = {
            "2019.4.15f1": ("2019.4f1", ""),
            "2018.4.36": ("2018.4f0", ""),
            "5.6.7f1": ("5.6.7", ""),
            "2019.4.15f1-custom": ("2019.4f1-custom", "-custom"),
            "2022.3.1f1c1": ("2022.3f1c1", "c1"),
        }
        for text, (printed, postfix) in cases.items():
            with self.subTest(text):
                version = UnityVersion.from_str(text)
                self.assertEqual((str(version), version.postfix), (printed, postfix))

    # Sections 15.1, 15.6, 23: texture helpers of Texture2DConverter and TextureSwizzler.
    def test_texture_helpers(self):
        import threading

        size = Texture2DConverter.get_compressed_image_size
        for fmt, padded in (
            (TextureFormat.DXT1, (8, 4)),
            (TextureFormat.BC7, (8, 4)),
            (TextureFormat.ETC2_RGBA8, (8, 4)),
            (TextureFormat.EAC_R, (8, 4)),
            (TextureFormat.PVRTC_RGB2, (8, 4)),
            (TextureFormat.PVRTC_RGBA4, (8, 4)),
            (TextureFormat.ASTC_RGB_6x6, (6, 6)),
            (TextureFormat.ASTC_RGBA_10x10, (10, 10)),
            (TextureFormat.ATC_RGBA8, (5, 3)),
            (TextureFormat.RGBA32, (5, 3)),
        ):
            with self.subTest(fmt.name):
                self.assertEqual(size(5, 3, fmt), padded)
        self.assertEqual(size(9, 3, TextureFormat.PVRTC_RGB2), (16, 4), "PVRTC 2 bpp: 8x4 blocks")
        # parse_image_data's checks before any format: a zero side, no data, a number outside the enum.
        parse, version = Texture2DConverter.parse_image_data, (2021, 3, 2, 0)
        for width, height in ((0, 4), (4, 0)):
            empty = parse(b"\x01" * 64, width, height, TextureFormat.RGBA32, version, 0)
            self.assertEqual((empty.mode, empty.size), ("RGBA", (0, 0)))
        with self.assertRaisesRegex(ValueError, "^Texture2D has no image data$"):
            parse(b"", 4, 4, TextureFormat.RGBA32, version, 0)
        with self.assertRaisesRegex(ValueError, "is not a valid TextureFormat"):
            parse(b"\x01" * 64, 4, 4, 999, version, 0)
        # pad_image: the last column and row, then the corner pixel.
        image = Image.new("L", (2, 2))
        image.putdata([1, 2, 3, 4])
        self.assertEqual(
            list(Texture2DConverter.pad_image(image, 4, 3).getdata()), [1, 2, 2, 2, 3, 4, 4, 4, 3, 4, 4, 4]
        )
        self.assertIs(Texture2DConverter.pad_image(image, 2, 2), image)
        # The Xbox swap: byte pairs, a last odd byte kept.
        self.assertEqual(
            Texture2DConverter.swap_bytes_for_xbox(b"\x01\x02\x03\x04\x05"), bytearray(b"\x02\x01\x04\x03\x05")
        )
        self.assertEqual(
            Texture2DConverter.XBOX_SWAP_FORMATS,
            [
                TextureFormat.RGB565,
                TextureFormat.DXT1,
                TextureFormat.DXT1Crunched,
                TextureFormat.DXT5,
                TextureFormat.DXT5Crunched,
            ],
        )
        # ASTC: 16 bytes a block; data short of the blocks raises; one LDR context per thread and block size.
        self.assertEqual(Texture2DConverter.calculate_astc_compressed_size(9, 5, (4, 4)), 3 * 2 * 16)
        with self.assertRaisesRegex(ValueError, "Invalid ASTC data size"):
            Texture2DConverter.astc(bytes(15), 4, 4, (4, 4))
        context = Texture2DConverter.get_astc_context((4, 4))
        self.assertIs(Texture2DConverter.get_astc_context((4, 4)), context)
        self.assertIsNot(Texture2DConverter.get_astc_context((6, 6)), context)
        other = []
        thread = threading.Thread(target=lambda: other.append(Texture2DConverter.get_astc_context((4, 4))))
        thread.start()
        thread.join()
        self.assertIsNot(other[0], context)
        import astc_encoder

        self.assertEqual((context.config.block_x, context.config.block_y), (4, 4))
        self.assertEqual(context.config.profile, astc_encoder.ASTCProfile.LDR)
        self.assertEqual(context.config.flags, astc_encoder.ASTCConfigFlags.USE_DECODE_UNORM8)
        made = []
        with (
            mock.patch.object(astc_encoder, "ASTCConfig", lambda *a, **kw: made.append((a, kw)) or a[0]),
            mock.patch.object(astc_encoder, "ASTCContext", lambda config: config),
        ):
            Texture2DConverter._get_astc_context.__wrapped__(0, (5, 5))
        self.assertEqual(made[0][1]["quality"], 100)
        # TextureSwizzler: the GOB, the blob, the padded size and the walk.
        self.assertEqual(
            (TextureSwizzler.GOB_X_TEXEL_COUNT, TextureSwizzler.GOB_Y_TEXEL_COUNT, TextureSwizzler.TEXEL_BYTE_SIZE),
            (4, 8, 16),
        )
        self.assertEqual(
            (TextureSwizzler.TEXELS_IN_GOB, sorted(TextureSwizzler.GOB_MAP)),
            (32, [(x, y) for x in range(4) for y in range(8)]),
        )
        blob = [0] * 8 + list((3).to_bytes(4, "little"))
        self.assertEqual(TextureSwizzler.get_switch_gobs_per_block(blob), 8)
        self.assertTrue(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, blob))
        self.assertFalse(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, [0] * 12), "one GOB per block")
        self.assertFalse(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, blob[:11]), "under 12 bytes")
        self.assertFalse(TextureSwizzler.is_switch_swizzled(BuildTarget.Android, blob))
        self.assertEqual(TextureSwizzler.get_padded_texture_size(5, 9, 4, 4, 2), (16, 64))
        self.assertEqual(len(TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP), 29)
        # set_image on a Switch-swizzled texture pads and swizzles through that table too, so a target whose written
        # format the table lacks raises KeyError: the seven ETC targets (section 19.2). BC7 raises its own error.
        raised = {}
        for fmt in TextureFormat:
            try:
                Texture2DConverter.image_to_texture2d(Image.new("RGBA", (8, 8)), fmt, BuildTarget.Switch, blob)
            except KeyError as e:
                raised[fmt.name] = e.args[0].name
            except Exception as e:  # noqa: BLE001 - every other raise is named below
                raised[fmt.name] = type(e).__name__
        written = {
            "ETC_RGB4": "ETC_RGB4",
            "ETC2_RGB": "ETC2_RGB",
            "ETC2_RGBA1": "ETC2_RGBA8",
            "ETC2_RGBA8": "ETC2_RGBA8",
        }
        written |= {"ETC_RGB4_3DS": "ETC_RGB4_3DS", "ETC_RGB4Crunched": "ETC_RGB4Crunched"}
        self.assertEqual(raised, written | {"ETC2_RGBA8Crunched": "ETC2_RGBA8", "BC7": "ValueError"})
        data = bytes(range(256)) * 4  # one block of 2 GOBs of RGBA32: 16 x 16 pixels, 1024 bytes
        swizzled = TextureSwizzler.swizzle(data, 16, 16, 4, 1, 2)
        self.assertNotEqual(swizzled, data)
        self.assertEqual(TextureSwizzler.deswizzle(swizzled, 16, 16, 4, 1, 2), data)
        # Only whole blocks of GOBs are walked: the rest of the output stays zero.
        self.assertEqual(TextureSwizzler.deswizzle(data, 16, 8, 4, 1, 2), bytearray(len(data)))

    # Section 16: what the dispatcher does before FMOD.
    def test_audio_dispatch(self):
        def clip(data, channels=0, frequency=0):
            return SimpleNamespace(
                m_AudioData=data,
                m_Resource=None,
                m_Name="c",
                m_Channels=channels,
                m_Frequency=frequency,
                object_reader=None,
            )

        for data, name in (
            (b"OggS" + bytes(8), "c.ogg"),
            (b"RIFF" + bytes(8), "c.wav"),
            (b"\0\0\0\x20ftypM4A ", "c.m4a"),
        ):
            with self.subTest(name):
                self.assertEqual(AudioClipConverter.extract_audioclip_samples(clip(data)), {name: data})
        calls = []
        with mock.patch.object(
            AudioClipConverter.fmod_toolkit, "raw_to_wav", lambda *a, **kw: calls.append((a, kw)) or {"done": b""}
        ):
            self.assertEqual(AudioClipConverter.extract_audioclip_samples(clip(b"FSB5" + bytes(8))), {"done": b""})
            AudioClipConverter.extract_audioclip_samples(clip(b"FSB5", 1, 22050), convert_pcm_float=False)
        self.assertEqual(calls[0], ((b"FSB5" + bytes(8), "c", 2, 44100), {"convert_pcm_float": True}))
        self.assertEqual(calls[1], ((b"FSB5", "c", 1, 22050), {"convert_pcm_float": False}))
        from UnityPy.enums.Audio import AUDIO_TYPE_EXTEMSION, AudioCompressionFormat, AudioType

        # 22 entries written, 20 kept: the two IntEnums share values 1 and 2, and the later entry wins.
        self.assertEqual(len(AUDIO_TYPE_EXTEMSION), 20)
        self.assertEqual(
            (AUDIO_TYPE_EXTEMSION[AudioType.MPEG], AUDIO_TYPE_EXTEMSION[AudioCompressionFormat.MP3]), (".mp3", ".fsb")
        )
        from UnityPy.classes import AudioClip

        extension = {
            fmt.name: AudioClip.extension.fget(SimpleNamespace(m_CompressionFormat=fmt))
            for fmt in AudioCompressionFormat
        }
        self.assertEqual(
            extension,
            {
                "PCM": ".fsb",
                "Vorbis": ".fsb",
                "ADPCM": ".fsb",
                "MP3": ".fsb",
                "VAG": ".vag",
                "HEVAG": ".vag",
                "XMA": ".wav",
                "AAC": ".m4a",
                "GCADPCM": ".fsb",
                "ATRAC9": ".at9",
            },
        )
        self.assertEqual(AudioClip.extension.fget(SimpleNamespace(m_CompressionFormat=11)), ".audioclip")

    # Sections 14.1, 14.3: the base classes, the math records and PPtr.
    def test_base_classes_and_records(self):
        import copy

        from UnityPy.classes import (
            ColorRGBA,
            Matrix3x4f,
            Matrix4x4f,
            Quaternionf,
            TextAsset,
            Vector2f,
            Vector3f,
            Vector4f,
            float3,
            float4,
        )

        self.assertEqual(
            (Vector2f(), Vector3f(), Vector4f()), (Vector2f(0, 0), Vector3f(0, 0, 0), Vector4f(0, 0, 0, 0))
        )
        self.assertEqual((float3, float4), (Vector3f, Vector4f))
        self.assertTrue(issubclass(Quaternionf, Vector4f))
        self.assertEqual([f.name for f in attrs.fields(Matrix3x4f)], [f"e{r}{c}" for r in range(3) for c in range(4)])
        self.assertEqual([f.name for f in attrs.fields(Matrix4x4f)], [f"e{r}{c}" for r in range(4) for c in range(4)])
        self.assertEqual(ColorRGBA(), ColorRGBA(0, 0, 0, 1))
        self.assertEqual(ColorRGBA(rgba=0x80FF0040), ColorRGBA(128 / 255, 1.0, 0.0, 64 / 255))
        self.assertEqual(ColorRGBA(rgba=0x80FF0040).rgba, 0x80FF0040)
        # PPtr: truthy when the path id is not 0; equality and hash on the two ids alone.
        a, b = PPtr(m_FileID=1, m_PathID=5), PPtr(m_FileID=1, m_PathID=5, assetsfile=object())
        self.assertEqual(
            (bool(a), bool(PPtr(m_FileID=1, m_PathID=0)), a == b, hash(a) == hash(b)), (True, False, True, True)
        )
        self.assertNotEqual(a, PPtr(m_FileID=2, m_PathID=5))
        self.assertEqual((a.file_id, a.path_id), (1, 5))
        # Object: keyword fields, no reader, save raises, copy.copy rebuilds; UnknownObject.get_type.
        text = TextAsset(m_Name="n", m_Script="s")
        self.assertIsNone((text.object_reader, text.assets_file)[1])
        with self.assertRaisesRegex(ValueError, "^ObjectReader not set$"):
            text.save()
        text.set_object_reader("reader")
        self.assertEqual(text.object_reader, "reader")
        extra = TextAsset(m_Name="n", m_Script="s")
        extra.m_Extra = 3
        copied = copy.copy(extra)
        self.assertEqual((type(copied), copied.m_Name, copied.m_Extra), (TextAsset, "n", 3))
        self.assertIsNone(UnknownObject(a=1).get_type())
        node = TypeTreeNode(m_Level=0, m_Type="Thing", m_Name="Base", m_ByteSize=0, m_Version=0)
        self.assertEqual(UnknownObject(node, a=1).get_type(), "Thing")

    # Section 11: the type character.
    def test_version_type_str_is_lower_case_for_known_types(self):
        upper = UnityVersion.from_str("2019.4.15F1")
        self.assertEqual((upper.type_str, str(upper)), ("f", "2019.4f1"))
        unknown = UnityVersion.from_str("2019.4.15q1")
        self.assertEqual((int(unknown.type), unknown.type_str, str(unknown)), (255, "q", "2019.4q1"))

    # Section 15.3: PackedBitVector.unpack_floats divides before it multiplies.
    def test_unpack_floats_divides_first(self):
        def f32(value):
            return struct.unpack("<f", struct.pack("<f", value))[0]

        divided = multiplied = 0
        rng = random.Random(7)
        for _ in range(50):
            bits = rng.choice([8, 10, 11, 12, 16, 20, 24])
            values = [rng.randrange(1 << bits) for _ in range(32)]
            packed = sum(v << (bits * i) for i, v in enumerate(values)).to_bytes((bits * 32 + 7) // 8, "little")
            vector = SimpleNamespace(
                m_NumItems=32,
                m_BitSize=bits,
                m_Range=f32(rng.uniform(0.01, 50)),
                m_Start=f32(rng.uniform(-20, 20)),
                m_Data=list(packed),
            )
            got = PackedBitVector.unpack_floats(vector)
            scale = vector.m_Range / ((1 << bits) - 1)
            self.assertEqual(got, [x * scale + vector.m_Start for x in values])
            left_to_right = [x * vector.m_Range / ((1 << bits) - 1) + vector.m_Start for x in values]
            divided += len(values)
            multiplied += sum(a != b for a, b in zip(got, left_to_right, strict=True))
        # Multiplying first rounds differently for a fair share of values.
        self.assertGreater(multiplied, divided // 20)
        # Bit size 0 repeats the start.
        empty = SimpleNamespace(m_NumItems=3, m_BitSize=0, m_Range=1.0, m_Start=2.5, m_Data=[])
        self.assertEqual(PackedBitVector.unpack_floats(empty), [2.5, 2.5, 2.5])

    # Sections 8, 14.3: defaults that are never used.
    def test_object_reader_get_and_container_attributes(self):
        sf = load(text_asset_file())[0]
        reader = sf.objects[1]
        self.assertIsNone(reader.get("nope", 5))
        self.assertIsNone(reader.nope)
        info = SimpleNamespace(asset=SimpleNamespace(path_id=1))
        container = ContainerHelper([("a/path", info)])
        self.assertIs(container.__getattr__("a/path"), info.asset)
        with self.assertRaisesRegex(KeyError, "'x'"):
            hasattr(container, "x")
        with self.assertRaisesRegex(KeyError, "'x'"):
            getattr(container, "x", None)
        # The mapping protocol: `[]` and `in` on the last entry of a path, `len` counting repeats, iteration over
        # each path once; assignment and deletion raise.
        first, last = (SimpleNamespace(asset=SimpleNamespace(path_id=i)) for i in (2, 3))
        repeated = ContainerHelper([("a/path", info), ("b", first), ("b", last)])
        self.assertEqual(
            (len(repeated), sorted(repeated), "b" in repeated, "c" in repeated), (3, ["a/path", "b"], True, False)
        )
        self.assertIs(repeated["b"], last.asset)
        with self.assertRaisesRegex(NotImplementedError, "^Assigning to container is not allowed!$"):
            repeated["c"] = info.asset
        with self.assertRaisesRegex(NotImplementedError, "^Deleting from the container is not allowed!$"):
            del repeated["b"]


if __name__ == "__main__":
    unittest.main()
