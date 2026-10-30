"""What the PARITY.md row tests share, not a test module: the oracle check, a TextAsset file and quiet loads.

ORACLE is True in the oracle image (UnityPy 1.25.2, with everything the row tests import from it); without it the
row tests skip. Each row module imports the UnityPy names it uses itself, under `if ORACLE:`.
"""

# The oracle check imports what the row tests' shared import block imported, so ORACLE is False without
# any of it; the row modules import the names they use themselves, so most go unused here.
# ruff: noqa: F401

import contextlib
import io
import sys
import zipfile
from pathlib import Path

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


def zip_with_a_directory():
    """A zip listing a directory entry, `assets/`, as `zip -r` writes, with byte 4 zeroed so the detector sees a ZIP."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("assets/", b"")
        archive.writestr("assets/a.assets", text_asset_file("a"))
    data = bytearray(buffer.getvalue())
    data[4] = 0
    return bytes(data)
