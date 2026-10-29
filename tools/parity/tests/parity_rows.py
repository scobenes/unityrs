"""What the PARITY.md row tests share: the oracle check, a TextAsset file, quiet loads and a scratch folder.

test_parity_rows_files.py, test_parity_rows_objects.py and
test_parity_rows_media.py hold the row tests; test_parity_rows.py checks
PARITY.md's test column against them. ORACLE is False outside the oracle image
(UnityPy 1.25.2), where the row tests skip.
"""

import contextlib
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import inventory  # noqa: E402
import synth as S  # noqa: E402

try:
    # Every package the row tests import: without one of them they skip.
    import attrs  # noqa: F401
    import etcpak  # noqa: F401
    import lz4.block  # noqa: F401
    import UnityPy
    from PIL import Image  # noqa: F401

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


class RowsCase(unittest.TestCase):
    """A scratch folder per test, and a working directory a test can change for its own duration."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    def chdir(self, path):
        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)
