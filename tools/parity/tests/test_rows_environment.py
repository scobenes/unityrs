"""PARITY.md rows on the Environment, run against the oracle: one CAB registry for the whole Environment, how `load`
and `load_file` key and find files, `Environment.file`, `objects`, `assets` and `find_file`, and split files,
folders and `Environment.save`.

Sections 3, 5.2 and 14.1. Every input is built here (`synth.py`), so nothing needs the corpus, and each test names
the rows it pins; a row's test column names the test back. Its class is `Rows`, as in `test_parity_rows.py`, the
module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import io
import os
import re
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE, TEXT_ASSET, load, quietly, text_asset_file, zip_with_a_directory  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import UnityPy
    from UnityPy.classes import PPtr
    from UnityPy.helpers import ResourceReader
    from UnityPy.helpers.ContainerHelper import ContainerHelper


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


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    def chdir(self, path):
        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)

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
        # load tests and opens through fs, which strips a URL-style prefix (section 3), and keys by the path as
        # given and its basename: `file:asset` is its own basename, `file:/d/asset` is not.
        for given, keys in (("file:asset", ["file:asset"]), (f"file:{path}", [f"file:{path}", "asset"])):
            env = UnityPy.Environment()
            quietly(env.load, [given])
            with self.subTest(given):
                self.assertEqual((list(env.files), len(env.objects)), (keys, len(keys)))
        # The basename is ntpath's, and so is the constructor's dirname: a backslash and a drive colon split a name,
        # so a file so named in the cwd is keyed twice, and `path` is the part before it (`.` for a bare name).
        for name, base, folder in (
            ("a\\b.assets", "b.assets", "a"),
            ("C:c.assets", "c.assets", "C:"),
            ("p.assets", None, "."),
        ):
            (self.tmp / "sub" / name).write_bytes(text_asset_file())
            env = UnityPy.Environment()
            quietly(env.load, [name])
            with self.subTest(name):
                keys = [name] if base is None else [name, base]
                self.assertEqual((list(env.files), len(env.objects)), (keys, len(keys)))
                self.assertEqual(quietly(UnityPy.Environment, name).path, folder)

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
            with (
                self.subTest(record),
                self.assertRaisesRegex(
                    FileNotFoundError, f"^Failed to resolve pointer - {re.escape(record)} not found!$"
                ),
            ):
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
        with self.assertRaisesRegex(FileNotFoundError, "^Failed to resolve pointer - Shared.Assets not found!$"):
            deref("Shared.Assets", preloaded)
        # The loop compares each key lower-cased (`key.lower() ==`), so a key in any case is found by name.
        keyed = UnityPy.Environment(path=str(self.tmp))
        load(text_asset_file("keyed"), "Shared.Assets", keyed)
        keyed.cabs.pop("shared.assets")
        self.assertEqual(deref("archive:/CAB-x/SHARED.assets", keyed), "keyed")
        # find_file loads what it finds on disk as a dependency (is_dependency=True, its default), by path/name or by
        # the walk, and so through deref: the file joins `files` and the registry, never `objects` or `assets`, and a
        # bundle's entries inherit the flag. A file on disk detected as a ZIP (fifth byte NUL, section 4) raises
        # load_file's TypeError; a zip as zipfile writes it (fifth byte 20) loads as a raw reader, which has no
        # `objects`.
        (d / "dep.assets").write_bytes(text_asset_file("dep"))
        (d / "b.bundle").write_bytes(S.fs_bundle([(b"CAB-b", text_asset_file("in bundle"), 4)]))
        (d / "z.zip").write_bytes(zip_with_a_directory())
        dependency = UnityPy.Environment(path=str(d))
        self.assertEqual(deref("archive:/CAB-x/dep.assets", dependency), "dep")  # path/name
        self.assertEqual(deref("Other.Assets", dependency), "other")  # the walk
        bundle = quietly(dependency.find_file, "b.bundle")
        own = dependency.files["main.assets"]
        disk = [str(d / "dep.assets"), str(d / "sub" / "other.assets"), str(d / "b.bundle")]
        self.assertEqual(list(dependency.files), ["main.assets", *disk])
        self.assertEqual([dependency.files[k].is_dependency for k in disk], [True, True, True])
        self.assertEqual(
            (bundle.files["CAB-b"].is_dependency, bundle.files["CAB-b"].objects[1].parse_as_dict()["m_Name"]),
            (True, "in bundle"),
        )
        self.assertEqual(sorted(dependency.cabs), ["cab-b", "dep.assets", "main.assets", "other.assets"])
        self.assertEqual(
            ([o.parse_as_dict()["m_Name"] for o in dependency.objects], dependency.assets), (["main"], [own])
        )
        with self.assertRaisesRegex(TypeError, "^Unsupported type for loading zip file$"):
            deref("archive:/CAB-x/z.zip", dependency)
        plain = io.BytesIO()
        with zipfile.ZipFile(plain, "w") as archive:
            archive.writestr("in.assets", text_asset_file("in"))
        self.assertEqual(plain.getvalue()[4], 20)
        (d / "p.zip").write_bytes(plain.getvalue())
        with self.assertRaisesRegex(
            AttributeError, "^'EndianBinaryReader_Streamable' object has no attribute 'objects'$"
        ):
            deref("archive:/CAB-x/p.zip", UnityPy.Environment(path=str(d)))
        for name in ("dep.assets", "b.bundle", "z.zip", "p.zip"):
            (d / name).unlink()
        # A broken link the walk matches: the path/name step rejects it (fs.exists follows links), load_file's
        # os.path.exists rejects the walked path, and find_sensitive_path finds nothing for it, so find_file returns
        # None, loading nothing, and deref raises "Failed to resolve pointer" with the record's whole path.
        broken = self.tmp / "broken"
        (broken / "sub").mkdir(parents=True)
        (broken / "gone.assets").symlink_to(broken / "nothing")
        (broken / "sub" / "gone2.assets").symlink_to(broken / "nothing")
        for name in ("gone.assets", "sub/gone2.assets", "gone2.assets"):
            env = UnityPy.Environment(path=str(broken))
            with self.subTest(name):
                self.assertEqual((quietly(env.find_file, name), env.files), (None, {}))
                self.assertEqual(
                    sorted(env.local_files), [str(broken / "gone.assets"), str(broken / "sub" / "gone2.assets")]
                )
        for record in ("archive:/CAB-x/gone.assets", "archive:/CAB-x/sub/gone2.assets"):
            with (
                self.subTest(record),
                self.assertRaisesRegex(
                    FileNotFoundError, f"^Failed to resolve pointer - {re.escape(record)} not found!$"
                ),
            ):
                deref(record, UnityPy.Environment(path=str(broken)))
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
        # The miss names the name as given, in its own case.
        with self.assertRaisesRegex(FileNotFoundError, f"^File Missing.Assets not found in {re.escape(str(d))}$"):
            found(UnityPy.Environment(path=str(d)), "Missing.Assets")
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
        # Resolution goes the same way, find_file handing the walked path to load_file: a record of the name
        # `a.assets.split0.bak` resolves to the merged file, and one whose file sits in `dir.split1/` raises.
        main = load(text_asset_file("main"), "main.assets", UnityPy.Environment(path=str(d)))[0]
        for record, expected in (
            ("a.assets.split0.bak", "merged"),
            ("inner.assets", IsADirectoryError),
            ("archive:/CAB-q/inner.assets", IsADirectoryError),
            ("plain.assets", "plain"),
        ):
            main.externals = [SimpleNamespace(path=record)]
            pointer = PPtr(m_FileID=1, m_PathID=1, assetsfile=main)
            with self.subTest(record=record):
                if isinstance(expected, str):
                    self.assertEqual(quietly(pointer.deref).parse_as_dict()["m_Name"], expected)
                else:
                    with self.assertRaisesRegex(expected, re.escape(str(d / "dir.split1"))):
                        quietly(pointer.deref)
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


if __name__ == "__main__":
    unittest.main()
