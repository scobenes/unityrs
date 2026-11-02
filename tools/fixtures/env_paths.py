"""`paths.json` for `tools/fixtures/env.py`: files UnityPy reads differently from a path than from bytes.

`Environment` opens a path as a file and reads it with the stream reader (`environment.py:136-139`); bytes are
read by the memory reader. Each case's files are loaded both ways, from a temporary directory removed whatever
happens, and for each way the loads, every object's `parse_as_dict` with and without `check_read` and its raw
data, the `SerializedFile`'s `userInformation`, and the resource reads it names are recorded. The inputs are
built here (`tools/parity/tests/synth.py`, and `files.py`'s short reads) and stored as `[count, byte]` runs.

How far a file's `seek` goes depends on the file system: past the largest file it takes, `seek` raises `OSError`
(`EINVAL`), past 2^44 - 4096 on ext4 with 4 KiB blocks (`ext4_max_size`). A case marked `tmpfs` is loaded from
tmpfs files instead (each a `memfd_create` file, which the temporary directory names by a symbolic link to its
`/proc/self/fd` path, so `load_file` opens it by that name), whose largest is 2^63 - 1 (`MAX_LFS_FILESIZE`): those
cases record what UnityPy does at the positions such a file system takes. A case marked `control` is one both
readers read alike. Imported by `env.py`, which writes and checks the fixture.
"""

from __future__ import annotations

import contextlib
import io
import os
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import scratch  # noqa: E402
import UnityPy  # noqa: E402
from UnityPy.files import BundleFile, SerializedFile  # noqa: E402
from UnityPy.helpers.ResourceReader import get_resource_data  # noqa: E402

import dump_records as D  # noqa: E402

# The stream reader's own exceptions (tests/fixtures/scaffold/reader.json), recorded by
# their text; any other failure is recorded by its L2 kind, or as "error".
STREAM_REFUSALS = {
    ("ValueError", "Position cannot be negative"),
    ("ValueError", "cannot fit 'int' into an offset-sized integer"),
    ("ValueError", "read length must be non-negative or -1"),
    ("OverflowError", "byte string is too large"),
    ("OverflowError", "cannot fit 'int' into an index-sized integer"),
}


def kind(f) -> str:
    if isinstance(f, SerializedFile):
        return "serialized"
    if isinstance(f, BundleFile):
        return "bundle"
    return "raw"


def refusal(e: BaseException) -> str | None:
    text = (type(e).__name__, str(e))
    return f"{text[0]}: {text[1]}" if text in STREAM_REFUSALS else None


def path_inputs() -> list[dict]:
    """The cases: each names its files (loaded in order) and the resource reads it makes."""
    sys.path.insert(0, str(ROOT / "tools/parity/tests"))
    from files import short_read_inputs
    from synth import Node, Obj, Type, fs_bundle, serialized_file

    def tree(*kids, meta=0):
        return Node("Base", "Base", list(kids), meta=meta)

    two_ints = Type(1, tree(Node("int", "a"), Node("int", "b")))

    def with_starts(data: bytes, t: Type, starts: list[int]) -> bytes:
        """Each object's stored (format 22) start replaced, in order, in a file of the one type `t` whose
        objects' path ids are 1, 2, ...: an object table row is 24 bytes, its path id aligned to 4."""
        out = bytearray(data)
        at = 48 + len(b"2021.3.2f1\0") + 5 + 4 + len(t.encode(22, True, False, "<")) + 4
        at += -at % 4
        for k, start in enumerate(starts):
            row = at + 24 * k
            assert struct.unpack_from("<q", out, row)[0] == k + 1
            struct.pack_into("<q", out, row + 8, start)
        return bytes(out)

    def with_start(data: bytes, start: int) -> bytes:
        """The first object's stored (format 22) start replaced, in a file of `two_ints`."""
        return with_starts(data, two_ints, [start])

    def with_info(bundle: bytes, patch) -> bytes:
        """A `fs_bundle` (format 8, its blocks-info LZ4 after the header) with its blocks-info replaced by
        `patch(info)`, compressed again."""
        import lz4.block

        head = bundle[: bundle.index(b"\0", bundle.index(b"5.x.x\0") + 6) + 1]
        _, packed, unpacked, flags = struct.unpack_from(">qIII", bundle, len(head))
        start = len(head) + 20 + (-(len(head) + 20) % 16)
        info = patch(lz4.block.decompress(bundle[start : start + packed], uncompressed_size=unpacked))
        repacked = lz4.block.compress(info, store_size=False)
        out = bytearray(head + struct.pack(">qIII", 0, len(repacked), len(info), flags))
        out += bytes(-len(out) % 16) + repacked + bundle[start + packed :]
        struct.pack_into(">q", out, len(head), len(out))
        return bytes(out)

    def data_offset(data: bytes) -> int:
        """A format-22 file's data offset: a stored start plus it is the object's `byte_start`."""
        return struct.unpack_from(">q", data, 32)[0]

    cases = []

    def case(name, files, resources=(), tmpfs=False, control=False):
        c = {"name": name, "files": list(files), "resources": list(resources)}
        cases.append({**c, **({"tmpfs": True} if tmpfs else {}), **({"control": True} if control else {})})

    # The pure-Python reader (byte_size 0): a `TypelessData` length of -2 raises from a path
    # and moves the position back from bytes (claude9-b MEDIUM-2's case); a length past the
    # end stops at the end of a stream, so `bytes_read` differs.
    t = Type(1, tree(Node("TypelessData", "d"), Node("int", "x")))
    data = struct.pack("<ii", -2, 7) + b"\x11\x22\x33\x44"
    case("typeless-minus-2", [("a.assets", serialized_file([t], [Obj(1, 0, data, byte_size=0)]))])
    t = Type(1, tree(Node("TypelessData", "d")))
    data = struct.pack("<i", 100) + b"abcd"
    case("typeless-past-the-end", [("a.assets", serialized_file([t], [Obj(1, 0, data, byte_size=0)]))])
    # Aligned strings (a control): a length outside `0 < n <= remaining` reads "" on both readers.
    t = Type(1, tree(Node("string", "a"), Node("string", "b"), Node("string", "c"), Node("int", "x")))
    data = struct.pack("<i", 3) + b"abc\0" + struct.pack("<iii", -1, 99, 7)
    case("strings-outside-the-test", [("a.assets", serialized_file([t], [Obj(1, 0, data, byte_size=0)]))], control=True)
    # A format-22 start below 0 or at 2^63 and up: `reset()` raises on a stream; bytes slice
    # from the end, or read nothing. Both readers, Boost (byte_size 8) and pure Python (0).
    for size, label in ((None, "boost"), (0, "python")):
        base = serialized_file([two_ints], [Obj(1, 0, struct.pack("<ii", 5, 6), byte_size=size)])
        for start, what in ((-(10**6), "far-below"), (-(2**63), "i64-min"), (2**63 - 1, "i64-max")):
            case(f"start-{what}-{label}", [("a.assets", with_start(base, start))])
        case(f"start-past-the-end-{label}", [("a.assets", with_start(base, 10**6))], control=True)
        # Past ext4's largest file (2^44 - 4096), which tmpfs takes: both readers read nothing there.
        far = 2**45 - data_offset(base)
        case(f"start-past-ext4-{label}", [("a.assets", with_start(base, far))], tmpfs=True, control=True)
    # A root that aligns (0x4000) and reads nothing, at a `byte_start` of 2^63 - 1, 2^63 - 3 and
    # 2^63 - 4 (byte_size 0): `align_stream` assigns `Position`, whose `seek` refuses 2^63 on a stream,
    # where the memory reader's position passes it (`bytes_read` 1). A tmpfs file takes those starts.
    t = Type(1, tree(meta=0x4000))
    base = serialized_file([t], [Obj(k, 0, b"", byte_size=0) for k in (1, 2, 3)])
    starts = [2**63 - 1 - data_offset(base) - k for k in (0, 2, 3)]
    case("align-to-2-63", [("a.assets", with_starts(base, t, starts))], tmpfs=True)
    # Strings to NUL: no NUL in reach drops the last byte read on a stream, which raises
    # only past the end; the memory reader returns the window, or raises at the end.
    case("user-information-past-the-window", [("a.assets", serialized_file([two_ints], [], user_information=b"U" * 40000))])
    sf = serialized_file([two_ints], [], user_information=b"abc")
    cut = bytearray(sf[: sf.find(b"abc\0") + 3])
    struct.pack_into(">qq", cut, 24, len(cut), len(cut))  # file_size and data_offset
    case("user-information-unterminated", [("a.assets", bytes(cut))])
    # A blob whose string buffer size is negative (files.py's short reads): `read(n)` below
    # -1 raises from a path.
    name, data = short_read_inputs()[0]
    case(f"blob-{name}", [("a.assets", data)])
    # A blob whose node count is negative: `read(32 * count)` raises from a path; from bytes the
    # slice is empty and the position moves back (`helpers/TypeTreeNode.py:115`).
    plain = serialized_file([two_ints], [Obj(1, 0, struct.pack("<ii", 5, 6))])
    count_at = 48 + len(b"2021.3.2f1\0") + 5 + 4 + 4 + 1 + 2 + 16  # the first type's blob
    assert struct.unpack_from("<i", plain, count_at)[0] == 3
    blob = bytearray(plain)
    struct.pack_into("<i", blob, count_at, -1)
    case("blob-negative-node-count", [("a.assets", bytes(blob))])
    # A blob's string buffer is read from memory whatever the file's reader (`TypeTreeNode.py:118`):
    # a name that runs to the buffer's end raises there, from a path too (a control).
    size = struct.unpack_from("<i", plain, count_at + 4)[0]
    end = count_at + 8 + 3 * 32 + size  # after the count, the size, three 32-byte rows and the buffer
    blob = bytearray(plain)
    assert blob[end - 2 : end] == b"b\0"
    blob[end - 1] = ord("X")
    case("blob-buffer-name-unterminated", [("a.assets", bytes(blob))], control=True)
    # A bundle's entries and its blocks-info table are read from memory whatever the bundle's reader
    # (`BundleFile.py:141-142`, `:173-186`): an entry whose `userInformation` runs to its end, and a
    # directory whose last path runs to the table's end, raise from a path too (controls).
    case("bundle-entry-unterminated", [("a.bundle", fs_bundle([(b"CAB-x", bytes(cut), 4)], codec=0))], control=True)
    good = fs_bundle([(b"CAB-x", serialized_file([two_ints], [Obj(1, 0, struct.pack("<ii", 5, 6))]), 4)], codec=0)
    case("bundle-directory-path-unterminated", [("a.bundle", with_info(good, lambda i: i[:-1] + b"X"))], control=True)
    # Blocks-info at the end with a compressed size past the file: a negative position.
    cab = serialized_file([two_ints], [Obj(1, 0, struct.pack("<ii", 5, 6))])
    b = bytearray(fs_bundle([(b"CAB-x", cab, 4)], codec=0))
    at = b.find(b"2021.3.2f1\0") + len(b"2021.3.2f1\0") + 8
    _, unc, flags = struct.unpack_from(">III", b, at)
    struct.pack_into(">III", b, at, len(b) + 100, unc, flags | 0x80)
    case("bundle-blocks-info-before-the-start", [("a.bundle", bytes(b))])
    # Detection: 20 bytes with no NUL raise from bytes, and are a resource from a path.
    case("twenty-bytes-no-nul", [("a.bin", b"A" * 20)])
    # Resource reads from a .resS kept as a reader: past i64 an offset is refused by the
    # file's `seek` and a size by its `read`; below, both readers read what exists.
    res = bytes(range(10))
    reads = [(0, 3), (8, 5), (12, 3), (3, 0), (2**40, 3), (2**63, 1), (2**64 - 1, 1), (0, 2**63 - 33), (0, 2**63), (0, 2**64 - 1), (5, 2**32)]
    case("resource-reads", [("a.assets", serialized_file([two_ints], [])), ("a.resS", res)], reads)
    # Offsets past ext4's largest file, on tmpfs: both readers read nothing (a read whose
    # buffered fill would pass 2^63 - 1 is the kernel's `EINVAL`, reader.json's tmpfs reads).
    reads = [(2**45, 3), (2**62, 1), (2**63 - 1, 0)]
    case("resource-reads-past-ext4", [("a.assets", serialized_file([two_ints], [])), ("a.resS", res)], reads, True, True)
    return cases


def object_read(o, check: bool) -> dict:
    try:
        d = o.parse_as_dict(check_read=check)
    except Exception as e:  # noqa: BLE001 - the exception is the outcome
        stream = refusal(e)
        if stream:
            return {"stream": stream}
        kind, read = D.error_kind(e)
        return {"error": kind, "bytes_read": read}
    return {"value": {k: v.hex() if isinstance(v, (bytes, bytearray)) else v for k, v in d.items()}}


def raw_read(o):
    try:
        return o.get_raw_data().hex()
    except Exception as e:  # noqa: BLE001
        return {"stream": refusal(e) or "error"}


def path_run(case: dict, tmp: Path, how: str) -> dict:
    env = UnityPy.Environment(path=str(tmp))
    loads, files = [], []
    for name, data in case["files"]:
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                if how == "bytes":
                    f = env.load_file(data, name=name)
                else:
                    f = env.load_file(str(tmp / name))
        except Exception as e:  # noqa: BLE001
            loads.append(refusal(e) or "error")
            files.append(None)
            continue
        loads.append(kind(f))
        files.append(f)
    out = {"loads": loads, "files": []}
    for f in files:
        if not isinstance(f, SerializedFile):
            out["files"].append(None)
            continue
        info = f.userInformation.encode("utf8", "surrogateescape")
        out["files"].append(
            {
                # Its length, first and last bytes: the long case's string is 32 KiB.
                "user_information": [len(info), info[:8].hex(), info[-8:].hex()],
                "objects": [
                    [o.path_id, object_read(o, True), object_read(o, False), raw_read(o)] for o in f.objects.values()
                ],
            }
        )
    resources = []
    sf = next((f for f in files if isinstance(f, SerializedFile)), None)
    for offset, size in case["resources"]:
        try:
            resources.append(get_resource_data("a.resS", sf, offset, size).hex())
        except Exception as e:  # noqa: BLE001
            resources.append({"stream": refusal(e) or "error"})
    out["resources"] = resources
    return out


@contextlib.contextmanager
def tmpfs_files(case: dict, tmp: Path):
    """The case's files on tmpfs (module docs), each named in `tmp` by a symbolic link; closed whatever happens."""
    fds = []
    try:
        for name, data in case["files"]:
            fds.append(os.memfd_create(name))
            view = memoryview(data)
            while view:
                view = view[os.write(fds[-1], view) :]
            (tmp / name).symlink_to(f"/proc/self/fd/{fds[-1]}")
        yield
    finally:
        for fd in fds:
            os.close(fd)


def path_case(case: dict) -> dict:
    from files import runs

    with scratch.folder() as tmp:
        tmp = Path(tmp)
        with tmpfs_files(case, tmp) if case.get("tmpfs") else contextlib.nullcontext():
            if not case.get("tmpfs"):
                for name, data in case["files"]:
                    (tmp / name).write_bytes(data)
            outcomes = {how: path_run(case, tmp, how) for how in ("bytes", "path")}
    flags = {k: True for k in ("tmpfs", "control") if case.get(k)}
    return {
        "name": case["name"],
        "files": [[name, runs(data)] for name, data in case["files"]],
        "resources": [[str(o), str(s)] for o, s in case["resources"]],
        **flags,
        **outcomes,
    }
