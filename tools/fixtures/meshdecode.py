"""Oracle fixtures for unityrs-meshdecode: UnityPy 1.25.2's MeshHandler (docs/plans/extract.md E16, PARITY.md
section 15.3's rows marked P1b).

Runs in the oracle image and writes three JSON files under crates/unityrs-meshdecode/tests/fixtures/, one case a
line, which `cargo test -p unityrs-meshdecode` replays without Python (crates/unityrs-meshdecode/tests/oracle.rs).
Every input is built here in code, its trees by hand (meshdecode_trees.py); no file is read.

    scripts/dev python -E -s tools/fixtures/meshdecode.py           # write the fixtures
    scripts/dev python -E -s tools/fixtures/meshdecode.py --check   # regenerate and compare
    scripts/dev python -E -s tools/fixtures/meshdecode.py --tpk [--no-cargo] [--verbose]
                                                                    # the cases on the TPK's trees (below)

**formats.json.** `MeshHandler.get_channel_dtype` and `get_channel_component_size` (helpers/MeshHelper.py:451-466)
for every format byte, 0 to 255, at versions on each side of the tables' boundaries (2017, 2019) and in each table
(below 5, at 5), with a type byte no `UnityVersionType` has; and the members of `VertexChannelFormat`,
`VertexFormat2017`, `VertexFormat`, `MeshTopology` and `GfxPrimitiveType`, name and value.

**unpack.json.** `UnityPyBoost.unpack_vertexdata` called directly (UnityPyBoost/Mesh.cpp:23-210) over a grid of
component sizes (1 to 4, and 8, which it never swaps), counts, offsets, strides (0, and one that wraps to a step
back), dimensions (0 to 4) and swaps, with data of exact length, one byte short and one long: the bytes, or the
exception. A case whose wrapped offsets pass UnityPyBoost's check but lie outside the data, or whose buffer length
wraps, is undefined behaviour there; it is not run, and records `"wrapped": true`, what the crate refuses
(`MeshError::WrappedAccess`, PARITY.md section 19.3).

**process.json.** The cases of meshdecode_cases.py: each object's tree (node rows, in `trees` by digest) and bytes,
written by UnityPy's `TypeTreeHelper.write_typetree` from a dict-mode value, read back in object mode by
UnityPyBoost (`read_typetree` with `byte_size`, as `ObjectReader.read` reads), handed to `MeshHandler(src,
version, endianess)`, then `process()`, `get_triangles()` when it returns, and `process()` again. A case records,
for `process`, the vertex count, the index size, the index buffer and every array the handler set (a float as its
f64 bits in hex, an int as itself), with the `get_resource_data` call `m_StreamData` made; for `get_triangles`,
each submesh's list (its whole tuples, the one a buffer cut short and the count of empty ones, which is the shape
a triangle list's slices take, checked here) and each submesh's `indexCount` after the call; for the second
`process` (`again`), whether it left what the first did (`same`, `differs`), or its exception; or the exception,
its class's `__name__` and message. A `process` that sets bind poses records them too (`m_BindPose`, each matrix
its rows). Three parts of `MeshHandler` are later phases', and the crate returns `Unsupported` where UnityPy
begins them; the script makes each raise a marker there and records `{"p2a": kind}`: `get_channels` (below 4, P4),
and the reads of a Mesh's own arrays and of a render data's `vertices` that begin `copy_from_mesh`'s and
`copy_from_spriterenderdata`'s copies (each field read through a property that raises where the field is non-empty
and the handler's own is unset); every other line is UnityPy's own. Each case's `process` is run once more with
`decompress_compressed_mesh` corrected: UnityPy's own source with PARITY.md section 19.1's two Tier C rows fixed in
it (`FIXES`, the UV offset added, not assigned, and the fourth bone weight `(31 - sum) / 31`, not `1 - sum`), each
line replaced exactly once, compiled in memory; where that answer differs, the case records it as `corrected`, the
answer unityrs gives, and the crate's replay names each such case in its expected-divergence mask.

**--tpk.** The same cases on UnityPy's own TPK trees (`Tpk.get_typetree_node`) in place of the hand-built ones, at
run time. Nothing derived from a TPK may be committed (docs/plans/core.md section 9, Q2), so the TPK's trees and
rows exist only in memory and in a pipe, and `--tpk` refuses any write to a path in its own process. Its guard is
against accidental writes and leaks, by this repository's code or a dependency's (caches, temp files, bytecode, core
dumps, debug dumps, logs, CI's public log); it is no sandbox against code written to evade it, which is a review
matter like any other code here. Before any module of this repository but scratch.py is imported, and so before
UnityPy (which loads its TPK) and before any value derived from the TPK exists, it stops bytecode writes, sets its
core-dump limit to 0, which cargo and the test binary inherit (under a pipe `core_pattern` the limit only reaches
the helper, and a core stays on the machine, never in a log or the repository), registers its exit check as the
first `atexit` callback (checked), and installs an audit hook (`NoWrites`) that raises `PermissionError("--tpk
writes to no path: <event>")` at: an `open` of a path with a write, create, append or truncate flag; `os.mkdir`,
`os.rename`, `os.replace`, `os.link`, `os.symlink`, `os.remove`, `os.rmdir`, `os.truncate`, `os.setxattr`,
`os.removexattr`, `os.utime`, `os.chmod`, `os.chown`, `os.chflags` and `os.lchflags`; every `shutil` and `syslog`
event, `tempfile.mkstemp` and `tempfile.mkdtemp`; every `sqlite3.connect`; `socket.bind`, `socket.connect`,
`socket.sendto` and `socket.sendmsg` on an AF_UNIX socket; every spawn that raises an audit event (`os.system`,
`os.exec*`, `os.posix_spawn*`, `os.spawn*`, `os.fork`, `os.forkpty`, `subprocess.Popen`) but one `subprocess.Popen`
of the replay's argv (`REPLAY`) in the checkout, with no other executable; and any event whose arguments it cannot
read. Its one exemption is child.run's scratch for that Popen: the making, mode change and removal of its per-run
folder (`unityrs-child-` and tempfile's 8 random characters, directly in the scratch root) and, when no parent names
a root, of the root it makes (`scratch.own_root()`), each by its path, never relative to a `dir_fd`; a file in
either is refused, and the replay's `UNITYRS_TEST_SCRATCH` is that per-run folder. An audit hook cannot be removed,
so the rule holds to the process's end. The exit check runs after every other `atexit` callback and after the
non-daemon threads end, and when anything was refused, even where the `PermissionError` was caught, it exits 1
whatever its own messages do (`finally: os._exit(1)`); a refusal later still (finalization, a daemon thread) is
refused but does not fail the run. The hook's reach ends where audit events do: `os.mkfifo`, `os.mknod`,
`_posixsubprocess.fork_exec` (which `subprocess` calls after its audited Popen), POSIX shared memory, a write by
native code (a C extension's `fopen`) and the replay's own process raise none. For those the backstops are
tools/parity/tests/test_meshdecode_tpk.py's owned working folder, `TMPDIR` and scratch, empty after, with `git
status` unchanged; CI's checks around its real step (the checkout's `git status --porcelain -uall --ignored` the
same after as before, its own `TMPDIR` empty after, and no file in the checkout newer than the run's start), which
print counts and fixed lines only, their commands' own errors discarded; and that test's allowlist scan of the
replay's source (`tests/oracle.rs`, its fixtures compiled in). Under `--tpk` an OOM-killed replay fails the run, and
is not run again: child.run's run after a SIGKILL is a second Popen, refused, and the run says the replay was killed
only then; a replay that cannot start at all fails it with the error's class. The run prints the number of cases and
of outcomes that differ from the hand-built trees', and the names of those cases, then hands the cases to the
crate's replay on its standard input, a pipe (`cargo test -p unityrs-meshdecode --test oracle`, its
`the_cases_on_the_tpk_s_trees_answer_as_unitypy_does`, with `UNITYRS_MESHDECODE_TPK_STDIN` set), unless
`--no-cargo`; the replay's failures name only the cases and the stage, and the replay prints `read <n> cases from
standard input`, which the script requires with every case, so a skipped replay is no pass. It exits 1 on any
difference. CI runs it, and its log is public, so only `--verbose`, for a local run, prints, on standard output, the
outcomes that differ, every field whose node type the two trees give differently (and always `topology`,
`m_IndexFormat` and `m_Use16BitIndices`), the replay's own messages, and an exception's message and traceback where
building or running a case on the TPK's trees raises; without it such a run prints the builder or case and the
exception's class only, and an inherited `UNITYRS_MESHDECODE_TPK_VERBOSE` is cleared.
`UNITYRS_MESHDECODE_TPK_PLANT`, for that test only, names one write or spawn `plant()` attempts after the TPK pass,
which the hook must refuse.
"""

from __future__ import annotations

# Only these, and scratch.py, come before --tpk's hook (test_meshdecode_tpk.py checks the order).
import os
import re
import resource
import socket
import sys
from pathlib import Path

TPK = "--tpk" in sys.argv[1:]
if TPK:
    sys.dont_write_bytecode = True  # no .pyc of this repository's modules (module docs)

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-meshdecode/tests/fixtures"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import scratch  # noqa: E402

# --- --tpk's rule: no write to any path (module docs), in force before UnityPy is imported

REPLAY = ["cargo", "test", "--locked", "-p", "unityrs-meshdecode", "--test", "oracle", "--", "--exact",
          "the_cases_on_the_tpk_s_trees_answer_as_unitypy_does", "--nocapture"]  # fmt: skip
PROBE = "unityrs.meshdecode.no_writes"  # meshdecode_build.under_no_writes's event: whether the hook is live
WRITE_FLAGS = os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND
PATH_EVENTS = {"os.mkdir", "os.rename", "os.replace", "os.link", "os.symlink", "os.remove", "os.rmdir", "os.truncate",
               "os.setxattr", "os.removexattr", "os.utime", "os.chmod", "os.chown", "os.chflags", "os.lchflags",
               "tempfile.mkstemp", "tempfile.mkdtemp"}  # fmt: skip
SPAWNS = {"os.system", "os.exec", "os.posix_spawn", "os.spawn", "os.fork", "os.forkpty"}
UNIX_SOCKET_EVENTS = {"socket.bind", "socket.connect", "socket.sendto", "socket.sendmsg"}
# The exemption's events, each with the place of its `dir_fd` among the event's arguments (None: it takes none).
FOLDER_EVENTS = {"os.mkdir": 2, "os.chmod": 2, "os.rmdir": 1, "tempfile.mkdtemp": None}
# child.run's per-run folder: scratch.folder's prefix, then tempfile's 8 random characters.
CHILD_FOLDER = re.compile(r"unityrs-child-[a-z0-9_]{8}")


class NoWrites:
    """The audit hook `--tpk` runs under (module docs): it raises `PermissionError` before any write to a path and
    any spawn but the replay's one `subprocess.Popen`; `refused` keeps what it refused, which fails the run."""

    def __init__(self) -> None:
        self.refused: list[str] = []
        self.popen = False
        # Taken before the hook is in: the temp folder's first lookup may probe it with a write, which it refuses.
        self.own_root = os.path.abspath(scratch.own_root())

    def exempt(self, event: str, args: tuple) -> bool:
        """The one exemption, child.run's scratch: making, changing the mode of and removing its per-run folder,
        directly in the scratch root, and the root it makes when no parent names one; by path, never by dir_fd."""
        if event not in FOLDER_EVENTS or not args or not isinstance(args[0], (str, bytes, os.PathLike)):
            return False
        at = FOLDER_EVENTS[event]
        if at is not None and (len(args) <= at or args[at] not in (None, -1)):
            return False
        path = os.path.abspath(os.fsdecode(args[0]))
        root = scratch.root()
        root = os.path.abspath(root) if root is not None else None
        if path == self.own_root:
            return root in (None, self.own_root)
        named = CHILD_FOLDER.fullmatch(os.path.basename(path)) is not None
        return named and os.path.dirname(path) == (root or self.own_root)

    def why(self, event: str, args: tuple) -> bool:
        """Whether `event` is refused."""
        if event == "open":
            where, mode, flags = args
            named = isinstance(where, (str, bytes, os.PathLike))  # an open descriptor (the pipe's end) is no path
            return named and (any(c in (mode or "") for c in "wax+") or bool((flags or 0) & WRITE_FLAGS))
        if event in PATH_EVENTS:
            return not self.exempt(event, args)
        if event.startswith(("shutil.", "syslog.")) or event == "sqlite3.connect":
            return True
        if event in UNIX_SOCKET_EVENTS:
            return args[0].family == socket.AF_UNIX
        if event == "subprocess.Popen":
            executable, argv, cwd, _ = args
            first = not self.popen and executable in (None, "cargo") and isinstance(argv, (list, tuple))
            first = first and list(argv) == REPLAY and cwd is not None and os.path.abspath(cwd) == str(ROOT)
            self.popen = self.popen or first
            return not first
        return event in SPAWNS

    def __call__(self, event: str, args: tuple) -> None:
        try:
            if event == PROBE:
                args[0].append(True)
                return
            refuse = self.why(event, args)
        except Exception:  # noqa: BLE001 -- arguments it cannot read: refused (module docs)
            refuse = True
        if refuse:
            self.refused.append(event)
            raise PermissionError(f"--tpk writes to no path: {event}")

    def at_exit(self) -> None:
        """The first `atexit` callback, so run last (after every other one and after the non-daemon threads end):
        when anything was refused, even where the `PermissionError` was caught, the run exits 1, whatever its
        messages do."""
        if not self.refused:
            return
        try:
            names = ", ".join(sorted(set(self.refused)))
            message = f"--tpk: refused {len(self.refused)} writes to a path or spawns ({names}); the run fails\n"
            for step in (sys.stdout.flush, lambda: sys.stderr.write(message), sys.stderr.flush):
                try:
                    step()
                except Exception:  # noqa: BLE001, S110 -- best effort: the exit below is what counts
                    pass
        finally:
            os._exit(1)


if TPK:
    import atexit

    # No core dump, of this process or of cargo and the test binary, which inherit the limit: one would hold the
    # TPK's trees in a file. Set before UnityPy is imported, which loads its TPK.
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    scratch.sweep()  # a dead process's root, which scratch.make would otherwise remove under the hook
    NO_WRITES = NoWrites()
    atexit.register(NO_WRITES.at_exit)
    if atexit._ncallbacks() != 1:
        raise SystemExit("--tpk: its exit check is not the first atexit callback")
    sys.addaudithook(NO_WRITES)

import argparse  # noqa: E402
import hashlib  # noqa: E402
import inspect  # noqa: E402
import json  # noqa: E402
import struct  # noqa: E402
import textwrap  # noqa: E402
import traceback  # noqa: E402
from contextlib import ExitStack  # noqa: E402
from types import SimpleNamespace  # noqa: E402
from unittest import mock  # noqa: E402

import child  # noqa: E402
import UnityPy  # noqa: E402
from UnityPy import UnityPyBoost  # noqa: E402
from UnityPy.classes import ChannelInfo, Mesh, SpriteRenderData  # noqa: E402
from UnityPy.enums.GfxPrimitiveType import GfxPrimitiveType  # noqa: E402
from UnityPy.enums.MeshTopology import MeshTopology  # noqa: E402
from UnityPy.enums.VertexFormat import VertexChannelFormat, VertexFormat, VertexFormat2017  # noqa: E402
from UnityPy.helpers import MeshHelper, TypeTreeHelper  # noqa: E402
from UnityPy.helpers.UnityVersion import UnityVersion  # noqa: E402
from UnityPy.streams import EndianBinaryReader  # noqa: E402

import meshdecode_cases  # noqa: E402
from meshdecode_build import splitmix, to_node, tpk_rows, under_no_writes  # noqa: E402
from meshdecode_cases import cases  # noqa: E402

ATTRS = ["m_Vertices", "m_Normals", "m_Tangents", "m_Colors", *[f"m_UV{i}" for i in range(8)], "m_BoneWeights",
         "m_BoneIndices"]  # fmt: skip
# PARITY.md section 19.1's Tier C rows on decompress_compressed_mesh, as UnityPy's line and its correction.
FIXES = (
    ("uvSrcOffset = uvDim * m_VertexCount", "uvSrcOffset += uvDim * m_VertexCount"),
    ("boneWeights[vertexIndex][j] = 1 - sum", "boneWeights[vertexIndex][j] = (31 - sum) / 31"),
)
# A Mesh's own arrays, and the handler field copy_from_mesh fills from each (helpers/MeshHelper.py:206-224).
OWN = {"m_Vertices": "m_Vertices", "m_Normals": "m_Normals", "m_Tangents": "m_Tangents", "m_UV": "m_UV0",
       "m_UV1": "m_UV1", "m_Colors": "m_Colors"}  # fmt: skip


def err(e: BaseException) -> dict:
    return {"err": [type(e).__name__, str(e)]}


def num(x) -> object:
    """A component as the fixture writes it: an int as itself, a float as its f64 bits."""
    if type(x) is float:
        return {"f": struct.pack(">d", x).hex()}
    assert type(x) is int, type(x)
    return x


# --- formats.json

VERSIONS = [(3, 5, 0, 3, 5), (4, 7, 2, 3, 1), (5, 6, 7, 3, 1), (2016, 4, 0, 3, 1), (2017, 1, 0, 3, 3),
            (2018, 4, 0, 3, 1), (2019, 1, 0, 0, 1), (2019, 4, 0, 3, 1), (6000, 0, 0, 255, 0), (2019, 4, 0, 6, 1)]  # fmt: skip


def format_rows() -> list[dict]:
    out = []
    for v in VERSIONS:
        handler = MeshHelper.MeshHandler(None, version=UnityVersion.from_list(*v))
        row = []
        for fmt in range(256):
            channel = ChannelInfo(dimension=1, format=fmt, offset=0, stream=0)
            try:
                row.append({"code": handler.get_channel_dtype(channel), "size": handler.get_channel_component_size(channel)})
            except Exception as e:  # noqa: BLE001 - the exception is the outcome
                row.append(err(e))
        out.append({"version": list(v), "formats": row})
    return out


def enums() -> dict:
    return {
        e.__name__: [[m.name, int(m)] for m in e]
        for e in (VertexChannelFormat, VertexFormat2017, VertexFormat, MeshTopology, GfxPrimitiveType)
    }


# --- unpack.json

M32 = (1 << 32) - 1


def check_len(cbs, vc, soff, stride, choff, dim) -> int:
    """UnityPyBoost's largest offset read, in uint32_t (UnityPyBoost/Mesh.cpp:50)."""
    return (((vc - 1) & M32) * stride + choff + soff + ((cbs * ((dim - 1) & M32)) & M32) + cbs) & M32


def defined(data_len, cbs, vc, soff, stride, choff, dim) -> bool:
    """Whether UnityPyBoost's copy stays in its buffers: the length unwrapped and each component in the data."""
    if vc * dim * cbs > M32:
        return False
    for v in range(vc):
        for d in range(dim):
            at = (soff + choff + stride * v + cbs * d) & M32
            if cbs and at + cbs > data_len:
                return False
    return True


def unpack_rows() -> list[dict]:
    grid = []
    for cbs in (1, 2, 3, 4, 8):
        for vc, soff, stride, choff, dim in ((3, 0, 4 * cbs, 0, 4), (3, 4, 3 * cbs + 1, 2, 3), (1, 0, 0, 0, 1),
                                             (5, 0, 0, cbs, 2), (0, 0, 8, 0, 2), (2, 0, cbs, 0, 0), (3, 2 * cbs, M32 - cbs + 1, 0, 1),
                                             (4, 16, 2 * cbs, 1, 2)):  # fmt: skip
            need = check_len(cbs, vc, soff, stride, choff, dim)
            for extra in (0, -1, 1):
                for swap in (True, False):
                    grid.append((cbs, vc, soff, stride, choff, dim, swap, max(0, need + extra) if need < 4096 else 64))
    # Wrapped: an offset at the top of the range, a vertex count whose (vc - 1) wraps at 0, a huge stride, and a
    # stride of 65536 over 65537 vertices, whose sum wraps to 4 over 4 bytes of data.
    grid += [(4, 8, M32 - 15, 4, 0, 1, True, 16), (4, 0, 0, 4, 0, 1, True, 0), (2, 3, 0, 1 << 31, 0, 1, True, 8),
             (1, 2, 0, M32, 0, 2, False, 4), (4, 1, M32 - 3, 0, 4, 1, True, 8),
             (4, 65537, 0, 65536, 0, 1, True, 4)]  # fmt: skip
    # A component of 0 bytes (an `int` no struct code gives, so only a direct caller's): nothing copied, wherever
    # its offsets point, even past the data.
    grid += [(0, 3, 0, 4, 0, 4, True, 16), (0, 5, 0, 0, 0, 2, False, 0), (0, 2, M32 - 3, 8, 0, 1, True, 4),
             (0, 0, 0, 4, 0, 1, True, 0), (0, 3, 9, 4, 0, 2, False, 4)]  # fmt: skip
    out = []
    for i, (cbs, vc, soff, stride, choff, dim, swap, n) in enumerate(grid):
        data = splitmix(1000 + i, n)
        args = {"cbs": cbs, "vc": vc, "soff": soff, "stride": stride, "choff": choff, "dim": dim, "swap": swap}
        row = {"data": data.hex(), **args}
        if check_len(cbs, vc, soff, stride, choff, dim) <= n and not defined(n, cbs, vc, soff, stride, choff, dim):
            row["wrapped"] = True
        else:
            try:
                row["ok"] = UnityPyBoost.unpack_vertexdata(data, cbs, vc, soff, stride, choff, dim, swap).hex()
            except Exception as e:  # noqa: BLE001
                row.update(err(e))
        out.append(row)
    return out


# --- process.json


class Marker(Exception):
    """Raised where the crate returns Unsupported: a later phase's part of MeshHandler."""

    def __init__(self, kind):
        super().__init__(kind)
        self.kind = kind


def marking(cls, field: str, attr: str, kind: str, handler):
    """`cls.<field>` read as before, but raising `Marker(kind)` where it is non-empty and `handler.<attr>` is
    unset, the test that begins the later phase's copy (helpers/MeshHelper.py:206-224, :246-251)."""
    slot = cls.__dict__.get(field)

    def get(obj):
        value = slot.__get__(obj, cls) if slot is not None else obj.__dict__[field]
        if value and getattr(handler, attr) is None:
            raise Marker(kind)
        return value

    return mock.patch.object(cls, field, property(get), create=slot is None)


def triangles_shape(tris) -> dict:
    """A submesh's list as whole tuples, at most one cut short, then empty ones (asserted)."""
    whole = [list(t) for t in tris if len(t) == 3]
    rest = tris[len(whole) :]
    assert all(len(t) == 3 for t in tris[: len(whole)]), "a short tuple before a whole one"
    cut = list(rest[0]) if rest and len(rest[0]) in (1, 2) else []
    empty = rest[1:] if cut else rest
    assert all(len(t) == 0 for t in empty), "a tuple after the cut one"
    return {"tri": whole, "cut": cut, "empty": len(empty)}


def corrected():
    """`decompress_compressed_mesh` with `FIXES` made in UnityPy's own source (module docs), compiled in memory."""
    source = textwrap.dedent(inspect.getsource(MeshHelper.MeshHandler.decompress_compressed_mesh))
    for wrong, right in FIXES:
        assert source.count(wrong) == 1, wrong
        source = source.replace(wrong, right)
    space: dict = {}
    exec(compile(source, "<decompress_compressed_mesh, corrected>", "exec"), vars(MeshHelper), space)  # noqa: S102
    return space["decompress_compressed_mesh"]


CORRECTED = corrected()


def left(handler) -> dict:
    """What `process` left on the handler."""
    attrs = {name: [[num(c) for c in t] for t in getattr(handler, name)] for name in ATTRS if getattr(handler, name) is not None}
    poses = getattr(handler, "m_BindPose", None)
    if poses is not None:
        attrs["m_BindPose"] = [[[num(c) for c in row] for row in m] for m in poses]
    index = handler.m_IndexBuffer
    return {
        "vertex_count": handler.m_VertexCount,
        "use16": handler.m_Use16BitIndices,
        "index_buffer": None if index is None else list(index),
        "attrs": attrs,
    }


def run(case: dict) -> dict:
    data = case["data"]
    obj = TypeTreeHelper.read_typetree(to_node(case["rows"]), EndianBinaryReader(data, "<"), False, len(data))
    calls = []

    def resource(path, assets_file, offset, size):
        calls.append([path, offset, size])
        return case["resource"]

    if case.get("reader"):
        obj.object_reader = SimpleNamespace(assets_file=None)
    out = {}
    try:
        src = obj if case["source"] == "mesh" else obj.m_RD
    except Exception as e:  # noqa: BLE001
        return {"process": err(e)}

    def fixed(self, streams):
        raise Marker("fixed_channels")

    version = UnityVersion.from_list(*case["version"])

    def patched(handler, *more):
        """The marks and mocks a run of `handler` goes under."""
        marks = [marking(Mesh, field, attr, f"mesh_array:{field}", handler) for field, attr in OWN.items()]
        marks.append(marking(SpriteRenderData, "vertices", "m_Vertices", "sprite_vertices", handler))
        stack = ExitStack()
        for p in (mock.patch.object(MeshHelper, "get_resource_data", resource),
                  mock.patch.object(MeshHelper.MeshHandler, "get_channels", fixed), *marks, *more):  # fmt: skip
            stack.enter_context(p)
        return stack

    def first_process(handler) -> dict:
        """`handler.process()`'s outcome, with the resource call it made."""
        del calls[:]
        try:
            handler.process()
        except Marker as m:
            return {"p2a": m.kind}
        except Exception as e:  # noqa: BLE001
            return err(e)
        return {"ok": {**left(handler), "resource_call": calls[0] if calls else None}}

    # PARITY.md section 19.1: the answer with UnityPy's two Tier C lines corrected, recorded where it differs.
    fixed_handler = MeshHelper.MeshHandler(src, version, case["endian"])
    with patched(fixed_handler, mock.patch.object(MeshHelper.MeshHandler, "decompress_compressed_mesh", CORRECTED)):
        fixed_outcome = first_process(fixed_handler)

    handler = MeshHelper.MeshHandler(src, version, case["endian"])
    with patched(handler):
        out["process"] = first_process(handler)
        if fixed_outcome != out["process"]:
            out["corrected"] = fixed_outcome
        if "ok" not in out["process"]:
            return out
        first = left(handler)
        try:
            tris = handler.get_triangles()
        except Exception as e:  # noqa: BLE001
            out["triangles"] = err(e)
        else:
            counts = [s.indexCount for s in src.m_SubMeshes]
            out["triangles"] = {"ok": {"submeshes": [triangles_shape(t) for t in tris], "index_counts": counts}}
        # UnityPy's handler keeps what its first process set (PARITY.md section 19.3).
        try:
            handler.process()
        except Marker as m:
            out["again"] = {"p2a": m.kind}
        except Exception as e:  # noqa: BLE001
            out["again"] = err(e)
        else:
            out["again"] = "same" if left(handler) == first else "differs"
    return out


def process_rows() -> tuple[dict, list[dict]]:
    trees, rows = {}, []
    for case in cases():
        text = json.dumps(case["rows"], separators=(",", ":"))
        key = hashlib.sha256(text.encode()).hexdigest()[:16]
        trees[key] = case["rows"]
        row = {"name": case["name"], "source": case["source"], "version": case["version"], "tree": key}
        row |= {"hex": case["data"].hex(), "endian": case["endian"]}
        if "resource" in case:
            row |= {"resource": case["resource"].hex(), "reader": case["reader"]}
        meshdecode_cases.AT = case["name"]
        row |= run(case)
        rows.append(row)
    names = [r["name"] for r in rows]
    assert len(names) == len(set(names)), "case names repeat"
    return trees, rows


def meta() -> dict:
    return {"generator": "tools/fixtures/meshdecode.py", "unitypy": UnityPy.__version__}


def fixtures() -> dict[str, dict]:
    trees, rows = process_rows()
    return {
        "formats.json": {**meta(), "enums": enums(), "cases": format_rows()},
        "unpack.json": {**meta(), "cases": unpack_rows()},
        "process.json": {**meta(), "trees": trees, "cases": rows},
    }


def dump(body: dict) -> str:
    """JSON with one list element per line, so a regenerated fixture diffs by case (tools/fixtures/scaffold.py)."""

    def compact(v: object) -> str:
        return json.dumps(v, sort_keys=True, separators=(",", ":"))

    items = sorted(body.items())
    lines = ["{"]
    for i, (key, value) in enumerate(items):
        comma = "," if i < len(items) - 1 else ""
        if isinstance(value, list):
            lines.append(f" {json.dumps(key)}: [")
            lines += [f"  {compact(e)}" + ("," if j < len(value) - 1 else "") for j, e in enumerate(value)]
            lines.append(" ]" + comma)
        elif isinstance(value, dict) and key == "trees":
            lines.append(f" {json.dumps(key)}: {{")
            keys = sorted(value)
            lines += [f"  {json.dumps(k)}: {compact(value[k])}" + ("," if j < len(keys) - 1 else "") for j, k in enumerate(keys)]
            lines.append(" }" + comma)
        else:
            lines.append(f" {json.dumps(key)}: {compact(value)}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


# --- --tpk

OUTCOMES = ("process", "corrected", "triangles", "again")


def types(rows: list[list]) -> dict[tuple, str]:
    """Each node's type by its path of names from the root."""
    out, path = {}, []
    for level, ty, name, _ in rows:
        del path[level:]
        path.append(name)
        out[tuple(path[1:])] = ty
    return out


def on_trees(trees) -> list[dict]:
    """process.json's rows, the cases laid out on `trees`."""
    meshdecode_cases.TREES = trees
    try:
        return process_rows()
    finally:
        meshdecode_cases.TREES = meshdecode_cases.hand_tree


def tpk(cargo: bool, verbose: bool) -> int:
    """The cases on the TPK's trees (module docs), held in memory: compared with the hand-built trees', then handed to
    the crate's replay through a pipe."""
    if not under_no_writes():
        raise RuntimeError("--tpk's hook is not in place")
    seen = set()

    def hand(class_id, version):
        seen.add((class_id, version))
        return meshdecode_cases.hand_tree(class_id, version)

    _, hand_rows = on_trees(hand)
    try:
        trees, tpk_cases = on_trees(tpk_rows)
    except Exception as e:  # noqa: BLE001
        if verbose:
            traceback.print_exc(file=sys.stdout)
            return 1
        # Its message and traceback can hold what the TPK's trees hold (a field's name, a leaf's format).
        print(f"--tpk: {meshdecode_cases.AT} raised {type(e).__name__} on the TPK's trees", file=sys.stderr)
        return 1
    by_name = {r["name"]: r for r in tpk_cases}
    differ = [
        (r["name"], key, r.get(key), by_name[r["name"]].get(key))
        for r in hand_rows
        for key in OUTCOMES
        if r.get(key) != by_name[r["name"]].get(key)
    ]
    print(f"--tpk: {len(hand_rows)} cases on the TPK's trees, {len(differ)} outcomes differ")
    for name, key, a, b in differ:
        print(f"  differs: {name} [{key}]")
        if verbose:
            print(f"    hand-built {json.dumps(a)}\n    TPK        {json.dumps(b)}")
    kinds: dict[tuple, set] = {}
    for class_id, version in sorted(seen) if verbose else ():
        theirs = types(tpk_rows(class_id, version))
        for path, ty in types(meshdecode_cases.hand_tree(class_id, version)).items():
            kinds.setdefault((class_id, path, ty, theirs.get(path, "(none)")), set()).add(".".join(map(str, version[:3])))
    for (class_id, path, ty, theirs), versions in sorted(kinds.items()):
        if ty != theirs or path[-1:] in (("topology",), ("m_IndexFormat",), ("m_Use16BitIndices",)):
            print(f"  {class_id} {'/'.join(path)}: hand-built {ty}, TPK {theirs} at {', '.join(sorted(versions))}")
    failed = 1 if differ else 0
    if cargo:
        env = dict(os.environ, UNITYRS_MESHDECODE_TPK_STDIN="1")
        env.pop("UNITYRS_MESHDECODE_TPK_VERBOSE", None)
        # The child gets child.run's own per-run folder, not this process's (the hook's one exemption).
        env.pop(scratch.VAR, None)
        if verbose:
            env["UNITYRS_MESHDECODE_TPK_VERBOSE"] = "1"
        body = dump({**meta(), "trees": trees, "cases": tpk_cases})
        # The cases reach the replay on its standard input, a pipe: cargo hands the test binary its own.
        try:
            ran = child.run(REPLAY, cwd=ROOT, env=env, check=False, input=body, stdout=child.PIPE, text=True)
        except OSError as e:
            # child.run runs a SIGKILLed child again (child.py), a second Popen the hook refuses (module docs):
            # killed only when the replay's first Popen ran and the hook refused this one.
            if NO_WRITES.popen and NO_WRITES.refused[-1:] == ["subprocess.Popen"]:
                killed = "--tpk: the crate's replay was killed (SIGKILL) and is not run again under --tpk"
                print(killed, file=sys.stderr)
            else:
                print(f"--tpk: the crate's replay could not run ({type(e).__name__})", file=sys.stderr)
            return 1
        sys.stdout.write(ran.stdout)
        # A skipped replay passes too, so the replay's own line, with every case, is required as well.
        read = f"read {len(tpk_cases)} cases from standard input\n"
        if ran.returncode != 0 or "test result: ok. 1 passed;" not in ran.stdout or read not in ran.stdout:
            print("--tpk: the crate's replay failed or did not run", file=sys.stderr)
            failed = 1
    if os.environ.get("UNITYRS_MESHDECODE_TPK_PLANT"):
        plant(os.environ["UNITYRS_MESHDECODE_TPK_PLANT"])
    return failed


def plant(name: str) -> None:
    """Test-only (tools/parity/tests/test_meshdecode_tpk.py): after the TPK pass, one write or spawn the hook must
    refuse, named by `UNITYRS_MESHDECODE_TPK_PLANT`; what it would write is a placeholder, never the cases."""
    import atexit
    import sqlite3
    import tempfile
    import threading
    import time

    tmp = os.environ["TMPDIR"]

    def write(name: str) -> None:
        with open(os.path.join(tmp, name), "w") as f:
            f.write("planted")

    def late() -> None:
        time.sleep(0.2)
        write("thread.txt")

    def quiet() -> None:  # a refusal caught, then standard error closed before the exit check prints
        atexit.register(sys.stderr.close)
        try:
            write("quiet.txt")
        except PermissionError:
            pass

    plants = {
        "atexit": lambda: atexit.register(write, "atexit.txt"),
        "thread": lambda: threading.Thread(target=late).start(),
        "sqlite": lambda: sqlite3.connect(os.path.join(tmp, "cache.sqlite")).execute("create table t(x)"),
        "setxattr": lambda: os.setxattr(tmp, "user.unityrs", b"planted"),
        "tempfile": lambda: tempfile.TemporaryFile("w").write("planted"),
        "target": lambda: (ROOT / "target" / "tpk-plant.json").write_text("planted"),
        "os.open": lambda: os.close(os.open(os.path.join(tmp, "raw.txt"), os.O_WRONLY | os.O_CREAT, 0o600)),
        "popen": lambda: child.run(REPLAY, cwd=ROOT, input="", stdout=child.PIPE, text=True),
        "quiet": quiet,
    }
    plants[name]()


def main() -> int:
    # No abbreviations: `--tpk` is matched in sys.argv before parsing, to put its hook in place first.
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0], allow_abbrev=False)
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    ap.add_argument("--tpk", action="store_true", help="the cases on the TPK's trees, in memory (module docs)")
    ap.add_argument("--no-cargo", action="store_true", help="with --tpk: do not replay through the crate")
    ap.add_argument("--verbose", action="store_true", help="with --tpk: print the outcomes and types (local runs)")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2" or TypeTreeHelper.read_typetree_boost is None:
        print(f"needs UnityPy 1.25.2 with UnityPyBoost, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    if args.tpk:
        return tpk(not args.no_cargo, args.verbose)
    stale = []
    for name, body in fixtures().items():
        text = dump(body)
        path = OUT / name
        if args.check:
            if not path.exists() or path.read_text() != text:
                stale.append(name)
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    if stale:
        print("stale fixtures: " + ", ".join(stale), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
