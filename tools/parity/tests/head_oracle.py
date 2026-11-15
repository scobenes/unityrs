"""The HEAD oracle's readers and bundle writer (UnityPy git 417d595, PARITY.md section 20), for the HEAD-tier tests.

Neither the oracle image nor CI installs UnityPy at 417d595, so `head_417d595/` holds the five source files that
the HEAD-tier save and synthetic reading rules need, byte for byte as the commit has them (MIT, `head_417d595/LICENSE`):
`UnityPy/helpers/CompressionHelper.py` (`BlockInfo`, `chunk_based_compress`), `UnityPy/files/BundleFile.py`
(`save`, `save_fs`, `read_fs`, `decompress_block`), `UnityPy/files/SerializedFile.py`,
`UnityPy/helpers/TypeTreeNode.py` and `UnityPy/helpers/TypeTreeHelper.py`. `HEAD_BLOBS` pins each one's git blob id, which
`git rev-parse 417d595:PATH` prints in a UnityPy checkout.

`load()` runs them inside UnityPy 1.25.2's package: each is executed as a module of the package it comes from, so
its relative imports take 1.25.2's modules, except that HEAD's BundleFile imports HEAD's CompressionHelper. Of the
modules they import, 417d595 has 1.25.2's `config.py`, `enums/BundleFile.py`, `files/File.py`,
`helpers/ArchiveStorageManager.py`, `helpers/UnityVersion.py` and `streams/EndianBinaryReader.py` unchanged; its
`streams/EndianBinaryWriter.py` adds a `tell` method, unused by the bundle writer; the SerializedFile envelope
writer tests adapt it to the installed writer's Position. Its `enums/__init__.py` no
longer re-exports `CommonString`, which neither writer file uses.
`load()` keeps 1.25.2 entry dispatch for existing writer tests; `readers()`
selects HEAD's SerializedFile and TypeTreeNode plus TypeTreeHelper for H1's
stored-tree fixtures. All five reader/writer sources are blob-pinned. The sixth pinned source,
`tools/TpkClassGenerator.py`, is retained for Y2 generation provenance; this
loader does not execute it.
"""

from __future__ import annotations

import hashlib
import sys
import types
from pathlib import Path
from unittest import mock
from contextlib import contextmanager

HEAD_DIR = Path(__file__).resolve().parent / "head_417d595"
HEAD_COMMIT = "417d59522a998fdee9a9c2b29fe35f3063222457"
# file -> (its path in the UnityPy repository, its git blob id at HEAD_COMMIT)
HEAD_BLOBS = {
    "TpkClassGenerator.py": ("UnityPy/tools/TpkClassGenerator.py", "2536644e856d1b9d85da0171ff93ff83924f8ffd"),
    "CompressionHelper.py": ("UnityPy/helpers/CompressionHelper.py", "922df19a5cb83a1913725da376a942606ad3fc62"),
    "BundleFile.py": ("UnityPy/files/BundleFile.py", "e589be5f5cabedeb24ab20c15ea98ed8a1885320"),
    "SerializedFile.py": ("UnityPy/files/SerializedFile.py", "309dd1062b5dbd309cdf428c5977b67df96eab4e"),
    "TypeTreeNode.py": ("UnityPy/helpers/TypeTreeNode.py", "3f4c82a4f4f0abba9eb983761fc4d3b5ab053104"),
    "TypeTreeHelper.py": ("UnityPy/helpers/TypeTreeHelper.py", "7eb916eb6da8606bf2cde2afdf8e7dcaf9de117d"),
    "LICENSE": ("LICENSE", "bf224f947cfa854312e17fc9927452f75a2b9c6d"),
}


def blob_id(data: bytes) -> str:
    """The git blob id of `data` (`git hash-object`)."""
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def _module(name: str, package: str, file: str) -> types.ModuleType:
    path = HEAD_DIR / file
    module = types.ModuleType(name)
    module.__package__, module.__file__ = package, str(path)
    sys.modules[name] = module
    exec(compile(path.read_bytes(), str(path), "exec"), module.__dict__)
    return module


def load():
    """(CompressionHelper, BundleFile): HEAD's two modules, loaded once per process inside UnityPy 1.25.2."""
    import UnityPy.files
    import UnityPy.helpers

    helper_name, bundle_name = "UnityPy.helpers._head_CompressionHelper", "UnityPy.files._head_BundleFile"
    if bundle_name in sys.modules:
        return sys.modules[helper_name], sys.modules[bundle_name]
    for file, (_, blob) in HEAD_BLOBS.items():
        got = blob_id((HEAD_DIR / file).read_bytes())
        if got != blob:
            raise RuntimeError(f"{HEAD_DIR / file} is not 417d595's (blob {got}, pinned {blob})")
    helper = _module(helper_name, "UnityPy.helpers", "CompressionHelper.py")
    # `from ..helpers import ArchiveStorageManager, CompressionHelper` and `from . import File` take the packages'
    # attributes. While UnityPy imports, `UnityPy.files.File` is still the module; `files/__init__.py` then
    # rebinds it to the class.
    with (
        mock.patch.object(UnityPy.helpers, "CompressionHelper", helper),
        mock.patch.object(UnityPy.files, "File", sys.modules["UnityPy.files.File"]),
    ):
        bundle = _module(bundle_name, "UnityPy.files", "BundleFile.py")
    assert bundle.CompressionHelper is helper
    return helper, bundle


@contextmanager
def readers():
    """HEAD SF/node/value readers plus the bundle reader, restored on exit.

    ObjectReader, ContainerHelper and EndianBinaryReader are unchanged at
    HEAD. Local-string, stored-tree fixtures need no HEAD TPK (which moved
    to tpk_ar); its provider and bundled resource remain outside this oracle.
    The value helper is HEAD's too, including its pure-Python array fix.
    HEAD's new TPK accessor is adapted to the installed common-string map;
    these fixtures encode all names locally and never consult that map.
    """
    import UnityPy.helpers
    import UnityPy.files

    _, bundle = load()
    node_name = "UnityPy.helpers.TypeTreeNode"
    import UnityPy.helpers.Tpk as tpk
    from UnityPy.enums import CommonString
    with mock.patch.dict(sys.modules), mock.patch.object(tpk, "get_common_strings", lambda: CommonString, create=True):
        node = _module(node_name, "UnityPy.helpers", "TypeTreeNode.py")
        helper = _module("UnityPy.helpers._head_TypeTreeHelper", "UnityPy.helpers", "TypeTreeHelper.py")
        with (
            mock.patch.object(UnityPy.files, "File", sys.modules["UnityPy.files.File"]),
            mock.patch.object(UnityPy.files, "BundleFile", bundle),
        ):
            serialized = _module("UnityPy.files._head_SerializedFile", "UnityPy.files", "SerializedFile.py")
        with (
            mock.patch.object(sys.modules["UnityPy.files.ObjectReader"], "TypeTreeHelper", helper),
            mock.patch.object(UnityPy.files, "SerializedFile", serialized.SerializedFile),
            mock.patch.object(UnityPy.files, "BundleFile", bundle.BundleFile),
            mock.patch.object(sys.modules["UnityPy.files.File"], "SerializedFile", serialized),
        ):
            yield serialized, bundle, node
