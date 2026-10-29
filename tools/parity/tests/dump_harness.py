"""What the dumper's test modules share: the pinned samples, synthetic inputs, and running the dumper.

test_dump.py, test_dump_crashes.py, test_dump_oracle.py and test_dump_cli.py
run tools/parity/unitypy_dump.py end to end. The samples are read at run time
from $UNITYRS_UNITYPY_SAMPLES (UnityPy's tests/samples at commit 417d595), else
from wherever the corpus index ($UNITYRS_CORPUS_INDEX, default
/corpus/index.json) lists its unitypy-sample entries. They are game bundles and
are never copied into this repository. unitypy_dump and dump_records are None
outside the oracle image, where the tests skip.
"""

import contextlib
import gzip
import hashlib
import io
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

try:
    import dump_records  # noqa: F401 - for the tests that patch a record function
    import unitypy_dump
except ImportError:  # pragma: no cover - outside the oracle image
    dump_records = unitypy_dump = None

import compare  # noqa: E402
import synth as S  # noqa: E402

SAMPLES = {
    "atlas_test": "61ee5afb5f5c64f0a4f65f039d09b2509198363e713423289c895067ac215177",
    "banner_1": "6bd0c3bc36c97e5f1cd2de71bb58477b6c32ac2c254a5705884306c56b94417f",
    "char_118_yuki.ab": "6198cb71c7a820256208332bbd375d21b18ed1a36a12ad04be1eef2111b014a1",
    "xinzexi_2_n_tex": "a9f6e6bba7110cce647ef752d66655d13ce5850b013a60d80d4e8b08953e02ea",
    "xinzexi_2_n_tex_mesh": "8bb2082b9586b3f8f3bad2d22d33346ec0492053f2a76323aa8b4b4047368ca8",
}


def samples_dir():
    """The pinned samples' folder: $UNITYRS_UNITYPY_SAMPLES, else the one the corpus index names."""
    candidates = [os.environ.get("UNITYRS_UNITYPY_SAMPLES")]
    index = Path(os.environ.get("UNITYRS_CORPUS_INDEX", "/corpus/index.json"))
    try:
        entries = json.loads(index.read_text())["files"]
        candidates += sorted(
            {str((index.parent / e["path"]).parent) for e in entries if e.get("kind") == "unitypy-sample"}
        )
    except (OSError, ValueError, KeyError, TypeError):
        pass
    for candidate in candidates:
        if candidate and all((Path(candidate) / name).is_file() for name in SAMPLES):
            return Path(candidate)
    return None


def unityraw(fmt, entries, player=b"5.x.x", engine=b"5.6.7f1"):
    """A synthetic UnityRaw bundle holding uncompressed entries [(name bytes, data)].

    Format 6 takes UnityPy's UnityFS layout (read_fs, plus one byte after the
    flags); every other format the web/raw layout, with a hash and crc from 4.
    """
    head = b"UnityRaw\0" + struct.pack(">I", fmt) + player + b"\0" + engine + b"\0"
    data = b"".join(p for _, p in entries)
    if fmt == 6:
        rows, off = b"", 0
        for name, payload in entries:
            rows += struct.pack(">qqI", off, len(payload), 4) + name + b"\0"
            off += len(payload)
        info = bytes(16) + struct.pack(">iIIHi", 1, len(data), len(data), 0, len(entries)) + rows
        return head + struct.pack(">qIII", 0, len(info), len(info), 0x40) + b"\0" + info + data
    rows, off = b"", 4 + sum(len(n) + 9 for n, _ in entries)
    for name, payload in entries:
        rows += name + b"\0" + struct.pack(">II", off, len(payload))
        off += len(payload)
    block = struct.pack(">i", len(entries)) + rows + data
    size = len(head) + 52
    fields = struct.pack(">IIIIiIIII", 0xC0FFEE, 0, size, 1, 1, len(block), len(block), size + len(block), 0)
    return head + bytes(range(16)) + fields + block


def records(path):
    opener = gzip.open if str(path).endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as fh:
        return [json.loads(line) for line in fh]


def dump(*argv):
    """Run the dumper in-process, quietly (UnityPy prints load errors); returns its exit status."""
    with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
        return unitypy_dump.main(["-q", *map(str, argv)])


def dump_process(*argv, env=None, flags=(), first=(), preload=()):
    """Run the dumper as its own process, for inputs that could take the test runner down with it.

    `flags` go to the interpreter. The `first` folders are put before the
    rest of sys.path, and the `preload` modules imported, before the script
    starts, as PYTHONPATH and a sitecustomize on it would (the dumper refuses
    PYTHONPATH itself, so this is how a test puts a copy of a module first).
    """
    args = [str(HERE / "unitypy_dump.py"), "-q", *map(str, argv)]
    if first or preload:
        boot = f"import runpy, sys; sys.path[:0] = {list(map(str, first))!r}; "
        boot += "".join(f"import {name}; " for name in preload)
        boot += f"sys.argv = {args!r}; runpy.run_path(sys.argv[0], run_name='__main__')"
        args = ["-c", boot]
    return subprocess.run(
        [sys.executable, "-B", *flags, *args],
        capture_output=True,
        env={**os.environ, **(env or {})},
        timeout=300,
    )


def ints_value(**fields):
    """The encoded value of a struct of ints."""
    return {"t": "dict", "v": [[k, v] for k, v in fields.items()]}


def assets(tree, data, **kw):
    """A format-22 SerializedFile with one MonoBehaviour-class object (path_id 1) typed by `tree`."""
    return S.serialized_file([S.Type(114, tree)], [S.Obj(1, 0, data)], **kw)


class DumpCase(unittest.TestCase):
    """A scratch folder per test, and the checks every dump test shares."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-parity-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def assert_canonical(self, path):
        """Every line the oracle wrote is in the canonical form, and the dump compares clean with itself."""
        report, *_ = compare.run(str(path), str(path))
        self.assertEqual(report.as_dict()["by_path"], [])
        self.assertEqual(report.counts["identical"], len(records(path)) - 1)

    def sample_args(self):
        src = samples_dir()
        if src is None:
            self.skipTest("UnityPy samples not found; set UNITYRS_UNITYPY_SAMPLES")
        for name, sha in SAMPLES.items():
            self.assertEqual(
                hashlib.sha256((src / name).read_bytes()).hexdigest(), sha, f"{name} is not the pinned sample"
            )
        args = [src]
        for name in SAMPLES:
            args += ["--name", name]
        return args
