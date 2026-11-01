"""Oracle fixtures for package D (docs/plans/core.md section 7.5): the Environment and Python strings.

Runs in the oracle image against UnityPy 1.25.2 and writes small JSON files under
crates/unityrs-core/tests/fixtures/d/, which `cargo test -p unityrs-core` replays
without Python (crates/unityrs-core/tests/env_fixtures.rs). Every input is built
in code: the scenes by unityrs-testkit (`env::scenes`), whose bytes this script
asks cargo for, the path cases' files and the strings here. No file is read but
those the script writes itself.

    scripts/dev python tools/fixtures/env.py           # write the fixtures
    scripts/dev python tools/fixtures/env.py --check   # regenerate and compare

`env.json`: each scene is run through one script of steps (load each file in
order, read every object, dereference every pointer, read every resource,
dereference every pointer again (a resource fallback may have loaded files),
list the container, ask every object its `container` path, list the container
again, ask every object of a file loaded as a dependency its `container`
path), and after each step the Environment's state is recorded:
`env.files` (key, `is_dependency`, kind), `env.cabs` keys in order, and
`env.objects` (file, path id, class id). The resolver's files sit in a
temporary `env.path`; a key under it is written `@/<name>`. Each scene of
`dependency_scenes` loads the files it names as dependencies
(`load_file(..., is_dependency=True)`), then lists the container, asks
every object its `container` path, and dereferences every pointer and reads
every resource of the objects of the files it loaded as dependencies (which
`env.objects` leaves out), the state recorded after each step.

`resolver.json`: the names UnityPy's lookups hand on where it looks in
`env.path` (the core's `CabResolver`): a resource read's fallback and a
pointer to each external of a file, none of them found.

`paths.json`: files UnityPy reads differently from a path, where `Environment` opens the
file and reads it with the stream reader (`environment.py:136-139`), than from bytes,
which the memory reader reads: each case's files are loaded both ways, from a temporary
directory removed whatever happens, and for each way the loads, every object's
`parse_as_dict` with and without `check_read` and its raw data, the `SerializedFile`'s
`userInformation`, and the resource reads it names are recorded. The inputs are built
here (`tools/parity/tests/synth.py`, and `files.py`'s short reads) and stored as
`[count, byte]` runs.

`pystr.json`: `str.lower` over every BMP code point (as the list of those it
changes), astral samples and final-sigma contexts, `ntpath` cases, which
paths UnityPy's `reSplit` reads as the parts of a split file, and every
character `c` for which it reads `a.split` + `c` so (its `\\d`).

    scripts/dev python tools/fixtures/env.py --g5 <env_corpus.rs output> --corpus <corpus root>

checks G5's corpus probe (crates/unityrs-core/tests/env_corpus.rs) against
UnityPy; it reads the corpus and writes nothing.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import ntpath
import os
import struct
import sys
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-core/tests/fixtures/d"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import child  # noqa: E402
import scratch  # noqa: E402
import UnityPy  # noqa: E402
from UnityPy.classes import PPtr  # noqa: E402
from UnityPy.environment import reSplit  # noqa: E402
from UnityPy.files import BundleFile, SerializedFile  # noqa: E402
from UnityPy.helpers.ResourceReader import get_resource_data  # noqa: E402

import dump_records as D  # noqa: E402
from scaffold import dump  # noqa: E402

# ------------------------------------------------------------------ scenes


def scene_inputs() -> list[dict]:
    """unityrs-testkit's env::scenes() and env::dependency_scenes() (a line
    with a `dependencies` key), as the replay test writes them."""
    with scratch.folder() as tmp:
        out = Path(tmp) / "scenes.jsonl"
        env = dict(os.environ, UNITYRS_WRITE_SCENES=str(out))
        child.run(
            [
                "cargo",
                "test",
                "-q",
                "-j",
                "2",
                "--locked",
                "-p",
                "unityrs-core",
                "--test",
                "env_fixtures",
                "--",
                "--exact",
                "scenes_are_the_testkit_scenes",
            ],
            cwd=ROOT,
            env=env,
            check=True,
            stdout=child.DEVNULL,
        )
        return [json.loads(line) for line in out.read_text().splitlines()]


def kind(f) -> str:
    if isinstance(f, SerializedFile):
        return "serialized"
    if isinstance(f, BundleFile):
        return "bundle"
    return "raw"


class Run:
    """One scene in one Environment, its steps recorded."""

    def __init__(self, scene: dict, path: str):
        self.scene = scene
        self.path = path
        self.env = UnityPy.Environment(path=path)
        # The files the scene loaded as dependencies, in load order.
        self.dependencies = []

    def key(self, name: str) -> str:
        return name.replace(self.path, "@", 1) if name.startswith(self.path) else name

    def state(self) -> dict:
        env = self.env
        return {
            "files": [[self.key(k), bool(getattr(f, "is_dependency", False)), kind(f)] for k, f in env.files.items()],
            "cabs": list(env.cabs.keys()),
            "objects": [[self.key(o.assets_file.name), o.path_id, o.class_id] for o in env.objects],
        }

    def load(self, i: int, is_dependency: bool = False):
        name, data = self.scene["files"][i]
        try:
            f = self.env.load_file(bytes.fromhex(data), name=name, is_dependency=is_dependency)
        except Exception:  # noqa: BLE001 - a failed load is a recorded outcome
            return "error"
        if is_dependency:
            self.dependencies.append(f)
        return "ok"

    def objects(self) -> list:
        return list(self.env.objects)

    def reads(self):
        out = []
        for o in self.objects():
            try:
                d = o.parse_as_dict()
            except Exception as e:  # noqa: BLE001
                out.append(type(e).__name__)
                continue
            out.append(list(d.keys()) if o.class_id == 142 else d.get("m_Name"))
        return out

    def target(self, t) -> list:
        try:
            name = t.parse_as_dict().get("m_Name")
        except Exception as e:  # noqa: BLE001
            name = type(e).__name__
        return [self.key(t.assets_file.name), t.path_id, t.class_id, name]

    def dep_objects(self) -> list:
        """Every object of the files the scene loaded as dependencies, in
        load order: a bundle's entries in its order, a nested bundle's in
        place."""
        out = []

        def walk(item):
            if isinstance(item, SerializedFile):
                out.extend(item.objects.values())
            elif getattr(item, "files", None):
                for sub in item.files.values():
                    walk(sub)

        for f in self.dependencies:
            walk(f)
        return out

    def derefs(self, objects=None):
        out = []
        for o in self.objects() if objects is None else objects:
            if o.class_id != 1:
                continue
            d = o.parse_as_dict()
            for k, v in d.items():
                if not k.startswith("m_Ptr"):
                    continue
                p = PPtr(m_FileID=v["m_FileID"], m_PathID=v["m_PathID"], assetsfile=o.assets_file)
                try:
                    out.append(self.target(p.deref()))
                except Exception as e:  # noqa: BLE001
                    out.append(type(e).__name__)
        return out

    def resources(self, objects=None):
        out = []
        for o in self.objects() if objects is None else objects:
            if o.class_id != 28:
                continue
            s = o.parse_as_dict()["m_StreamData"]
            try:
                out.append(get_resource_data(s["path"], o.assets_file, s["offset"], s["size"]).hex())
            except Exception as e:  # noqa: BLE001
                out.append(type(e).__name__)
        return out

    def container(self):
        try:
            c = self.env.container
        except Exception as e:  # noqa: BLE001
            return type(e).__name__
        return [
            [path, info.preloadIndex, info.preloadSize, info.asset.m_FileID, info.asset.m_PathID]
            for path, info in c.container
        ]

    @staticmethod
    def container_of(o):
        try:
            c = o.container
        except Exception as e:  # noqa: BLE001
            return type(e).__name__
        return c if c is None or isinstance(c, str) else "<not a str>"

    def paths(self):
        return [self.container_of(o) for o in self.objects()]

    def dep_paths(self):
        """`obj.container` for the objects of every `files` entry loaded as a
        dependency, which `env.objects` leaves out: `[key, path id, path]`."""
        out = []

        def walk(key, item):
            if isinstance(item, SerializedFile):
                out.extend([key, o.path_id, self.container_of(o)] for o in item.objects.values())
            elif getattr(item, "files", None):
                for sub in item.files.values():
                    walk(key, sub)

        for k, f in self.env.files.items():
            if getattr(f, "is_dependency", False):
                walk(self.key(k), f)
        return out

    def steps(self) -> list[dict]:
        out = []

        def step(op, result):
            out.append({"op": op, "result": result, **self.state()})

        for i in range(len(self.scene["files"])):
            step(["load", i], self.load(i))
        step(["reads"], self.reads())
        step(["derefs"], self.derefs())
        step(["resources"], self.resources())
        step(["derefs"], self.derefs())
        step(["container"], self.container())
        step(["paths"], self.paths())
        step(["container"], self.container())
        step(["dep_paths"], self.dep_paths())
        return out

    def dependency_steps(self) -> list[dict]:
        """The loads, each file `dependencies` names as a dependency, then the
        container, every object's `container` path, and every pointer and
        resource of the dependencies' objects."""
        out = []

        def step(op, result):
            out.append({"op": op, "result": result, **self.state()})

        deps = set(self.scene["dependencies"])
        for i in range(len(self.scene["files"])):
            if i in deps:
                step(["load_dependency", i], self.load(i, is_dependency=True))
            else:
                step(["load", i], self.load(i))
        step(["container"], self.container())
        step(["paths"], self.paths())
        step(["dep_derefs"], self.derefs(self.dep_objects()))
        step(["dep_resources"], self.resources(self.dep_objects()))
        return out


def scene_case(scene: dict) -> dict:
    with scratch.folder() as tmp:
        for name, data in scene["resolver"]:
            (Path(tmp) / name).write_bytes(bytes.fromhex(data))
        with contextlib.redirect_stdout(io.StringIO()):
            run = Run(scene, str(tmp))
            steps = run.dependency_steps() if "dependencies" in scene else run.steps()
    case = {"name": scene["name"], "files": scene["files"], "resolver": scene["resolver"]}
    if "dependencies" in scene:
        case["dependencies"] = scene["dependencies"]
    return {**case, "steps": steps}


# ------------------------------------------------------------------ resolver names

RESOLVER_EXTERNALS = ["archive:/CAB-DE/CAB-DE", "Assets/Level/Thing.asset", "archive:/CAB-E/CAB-E"]
RESOLVER_RESOURCE = "archive:/CAB-Q/CAB-Q.resS"


def resolver_fixture() -> dict:
    """The names UnityPy hands on where it looks in `env.path` (the core's
    `CabResolver`), for a file whose externals are `RESOLVER_EXTERNALS`: a
    read of the resource `RESOLVER_RESOURCE` that no CAB answers
    (`get_resource_data`, whose fallback `load_dependencies` calls
    `load_file` with each external's stored path, then each candidate), and a
    pointer to each external that no sibling answers (`PPtr.deref`, which
    calls `find_file` with the cleaned path). UnityPy's own functions run on
    stand-ins for the file and its container, whose `load_file` and
    `find_file` record what they are asked and find nothing."""
    asked = []
    env = UnityPy.Environment()
    env.load_file = lambda name, *_a, **_k: asked.append(name)
    env.find_file = lambda name, *_a, **_k: asked.append(name)
    f = types.SimpleNamespace(
        environment=env,
        externals=[types.SimpleNamespace(path=p) for p in RESOLVER_EXTERNALS],
        parent=types.SimpleNamespace(files={}),
    )
    f.load_dependencies = types.MethodType(SerializedFile.load_dependencies, f)
    out = {"externals": RESOLVER_EXTERNALS, "resource": RESOLVER_RESOURCE}
    try:
        get_resource_data(RESOLVER_RESOURCE, f, 0, 4)
        out["resource_error"] = None
    except Exception as e:  # noqa: BLE001 - the raise is the recorded outcome
        out["resource_error"] = type(e).__name__
    out["resource_asked"] = list(asked)
    derefs = []
    for file_id in range(1, len(RESOLVER_EXTERNALS) + 1):
        asked.clear()
        p = PPtr.__new__(PPtr)
        p.m_FileID, p.m_PathID, p.assetsfile = file_id, 5, f
        try:
            p.deref(f)
            error = None
        except Exception as e:  # noqa: BLE001
            error = type(e).__name__
        derefs.append({"file_id": file_id, "asked": list(asked), "error": error})
    out["derefs"] = derefs
    return out


# ------------------------------------------------------------------ paths

# The stream reader's own exceptions (tests/fixtures/scaffold/reader.json), recorded by
# their text; any other failure is recorded by its L2 kind, or as "error".
STREAM_REFUSALS = {
    ("ValueError", "Position cannot be negative"),
    ("ValueError", "cannot fit 'int' into an offset-sized integer"),
    ("ValueError", "read length must be non-negative or -1"),
    ("OverflowError", "byte string is too large"),
    ("OverflowError", "cannot fit 'int' into an index-sized integer"),
}


def refusal(e: BaseException) -> str | None:
    text = (type(e).__name__, str(e))
    return f"{text[0]}: {text[1]}" if text in STREAM_REFUSALS else None


def path_inputs() -> list[dict]:
    """The cases: each names its files (loaded in order) and the resource reads it makes."""
    sys.path.insert(0, str(ROOT / "tools/parity/tests"))
    from files import short_read_inputs
    from synth import Node, Obj, Type, fs_bundle, serialized_file

    def tree(*kids):
        return Node("Base", "Base", list(kids))

    two_ints = Type(1, tree(Node("int", "a"), Node("int", "b")))

    def with_start(data: bytes, start: int) -> bytes:
        """The first object's stored (format 22) start replaced, in a file of `two_ints`."""
        out = bytearray(data)
        at = 48 + len(b"2021.3.2f1\0") + 5 + 4 + len(two_ints.encode(22, True, False, "<")) + 4
        at += -at % 4
        assert struct.unpack_from("<q", out, at)[0] == 1
        struct.pack_into("<q", out, at + 8, start)
        return bytes(out)

    cases = []

    def case(name, files, resources=()):
        cases.append({"name": name, "files": list(files), "resources": list(resources)})

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
    case("strings-outside-the-test", [("a.assets", serialized_file([t], [Obj(1, 0, data, byte_size=0)]))])
    # A format-22 start below 0 or at 2^63 and up: `reset()` raises on a stream; bytes slice
    # from the end, or read nothing. Both readers, Boost (byte_size 8) and pure Python (0).
    for size, label in ((None, "boost"), (0, "python")):
        base = serialized_file([two_ints], [Obj(1, 0, struct.pack("<ii", 5, 6), byte_size=size)])
        for start, what in ((-(10**6), "far-below"), (-(2**63), "i64-min"), (2**63 - 1, "i64-max")):
            case(f"start-{what}-{label}", [("a.assets", with_start(base, start))])
        case(f"start-past-the-end-{label}", [("a.assets", with_start(base, 10**6))])
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


def path_case(case: dict) -> dict:
    from files import runs

    with scratch.folder() as tmp:
        for name, data in case["files"]:
            (Path(tmp) / name).write_bytes(data)
        outcomes = {how: path_run(case, Path(tmp), how) for how in ("bytes", "path")}
    return {
        "name": case["name"],
        "files": [[name, runs(data)] for name, data in case["files"]],
        "resources": [[str(o), str(s)] for o, s in case["resources"]],
        **outcomes,
    }


# ------------------------------------------------------------------ strings

ASTRAL = [0x10400, 0x10428, 0x104B0, 0x10570, 0x10C80, 0x118A0, 0x16E40, 0x1E900, 0x1F600, 0x1D400, 0x10FFFF]

SIGMA = [
    "Σ",
    "ΑΣ",
    "ΑΣΑ",
    "ΑΣ.txt",
    "ΑΣ.",
    "Α.Σ",
    "Α'Σ",
    "Α'Σ'",
    "ΑΣ'Α",
    "ΑΣ\u0301",
    "ΑΣ\u0301Α",
    "\u0301Σ",
    "1Σ",
    "ΑΣ1",
    "ΑΣ Α",
    "ΑΣΣ",
    "ΑΣΣΑ",
    "aΣ",
    "aΣb",
    "ǅΣ",
    "ᾈΣ",
    "Σ\u00ad",
    "A\u00adΣ",
    "AΣ\u00ad",
    "AΣ\u00adB",
    "İΣ",
    "\U00010400Σ",
    "ΣΑ",
    "/ΑΣ/x",
    # Characters assigned after Unicode 14.0 have no class to Python 3.11, though
    # Rust's newer tables give them one (D request 5): U+A7CB is cased to Rust,
    # U+1E030 case-ignorable.
    "\ua7cbΣ",
    "Α\U0001e030Σ",
    "ΑΣ\U0001e030",
    "ΑΣ\U0001e030Α",
]

NTPATH = [
    "",
    "a",
    "a/b",
    "a\\b",
    "a/b/",
    "a\\b\\",
    "c:",
    "c:x",
    "c:/x",
    "C:\\x\\y.z",
    "//server/share",
    "//server/share/x/y",
    "\\\\server\\share\\x",
    "//server",
    "//",
    "///a/b",
    "\\\\?\\UNC\\s\\sh\\x",
    "//?/unc/s/sh/x",
    "\\\\?\\c:\\x",
    "é:foo",
    ":",
    ".",
    "..",
    "...",
    ".a",
    "..a",
    "a.",
    "a..b",
    "a/.b",
    "a/b.c.d",
    "a.b/c",
    "x.tar.gz",
    "archive:/CAB-x/CAB-x.resS",
    "archive:/CAB-x/",
    "CAB-ABC.resS",
    "dir.d/file",
]

SPLIT = [
    "a.split0",
    "a.split12",
    "dir/a.split0",
    "dir\\a.split3",
    "a.split",
    "a.splitx",
    ".split0",
    "/.split0",
    "dir/.split0",
    "\\.split0",
    "a.split0.extra",
    "a.SPLIT0",
    "a.split\u0663",
    "x\ny/a.split0",
    "x/y\na.split0",
    "a\n.split0",
    "\na.split0",
    "a.b.split0",
    "a/b/c.split9.d",
    "",
    # Numeric characters that are no decimal digit (`No`, `Nl`): one file.
    "a.split\u00b2",
    "a.split\u00bd",
    "a.split\u2160",
    "a.split\u3007",
    "a.split\U00010107",
    "dir/data.unity3d.split\u00b9",
    # Decimal digits (`Nd`) past ASCII: the parts of a split file.
    "a.split\uff11",
    "a.split\U0001d7ce",
    "a.split\u00b20",
]


def split_digit_ranges() -> list[list[int]]:
    """Every code point `c` for which `reSplit` matches `"a.split" + c`, as inclusive ranges."""
    out: list[list[int]] = []
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF or not reSplit.match("a.split" + chr(cp)):
            continue
        if out and out[-1][1] == cp - 1:
            out[-1][1] = cp
        else:
            out.append([cp, cp])
    return out


def lower_bmp() -> list[list[str]]:
    """Every BMP scalar `str.lower` changes, with its lowering (hex of UTF-8)."""
    out = []
    for cp in range(0x10000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        c = chr(cp)
        low = c.lower()
        if low != c:
            out.append([cp, low.encode().hex()])
    return out


def pystr_fixture() -> dict:
    return {
        "bmp_changed": lower_bmp(),
        "astral": [[cp, chr(cp).lower().encode().hex()] for cp in ASTRAL],
        "sigma": [[s.encode().hex(), s.lower().encode().hex()] for s in SIGMA],
        "ntpath": [
            [
                p,
                ntpath.basename(p),
                list(ntpath.splitext(ntpath.basename(p))),
                ntpath.basename(p).lower(),
            ]
            for p in NTPATH
        ],
        "split": [[p.encode().hex(), reSplit.match(p) is not None] for p in SPLIT],
        "split_digits": split_digit_ranges(),
    }


# ---------------------------------------------------------------- G5 corpus


def fnv64(b: bytes) -> str:
    h = 0xCBF29CE484222325
    for x in b:
        h = ((h ^ x) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{h:016x}"


def check_g5(probe_path: Path, corpus: Path) -> int:
    """G5's corpus half: UnityPy, given the bundles crates/unityrs-core/tests/env_corpus.rs
    loaded, dereferences each pointer and reads each stream it took; each
    must equal the core's. Reads the corpus and the probe's output (both
    outside the repository) and prints counts only."""
    report = json.loads(probe_path.read_text())
    rels = report["bundles"]
    with scratch.folder() as empty:
        return check_g5_in(report, rels, corpus, str(empty))


def check_g5_in(report: dict, rels: list[str], corpus: Path, empty: str) -> int:
    """check_g5 with `empty` as env.path, so find_file finds only the loaded bundles."""
    with contextlib.redirect_stdout(io.StringIO()):
        env = UnityPy.Environment(path=empty)
        for r in rels:
            env.load_file(str(corpus / r))
    files = {}
    for k, f in env.files.items():
        files[Path(k).name] = f
    by_name = {r: files[Path(r).name] for r in rels}

    def obj(case):
        return by_name[case["bundle"]].files[case["cab"]].objects[case["path_id"]]

    counts = {"pptr": [0, 0], "stream": [0, 0]}
    for case in report["probe"]["pptr"]:
        v = obj(case).parse_as_dict()
        for p in case["field"]:
            v = v[p]
        p = PPtr(m_FileID=v["m_FileID"], m_PathID=v["m_PathID"], assetsfile=obj(case).assets_file)
        try:
            t = p.deref()
            got = {"class_id": t.class_id, "name": t.parse_as_dict().get("m_Name")}
        except Exception as e:  # noqa: BLE001
            got = {"error": type(e).__name__}
        counts["pptr"][0 if got == case["core"] else 1] += 1
    for case in report["probe"]["stream"]:
        o = obj(case)
        sd = o.parse_as_dict()["m_StreamData"]
        try:
            data = bytes(get_resource_data(sd["path"], o.assets_file, sd["offset"], sd["size"]))
            got = {"fnv64": fnv64(data), "len": len(data)}
        except Exception as e:  # noqa: BLE001
            got = {"error": type(e).__name__}
        counts["stream"][0 if got == case["core"] else 1] += 1
    print(json.dumps({"agree": {k: v[0] for k, v in counts.items()}, "differ": {k: v[1] for k, v in counts.items()}}))
    return 0 if all(v[1] == 0 for v in counts.values()) else 1


# -------------------------------------------------------------------- main


def fixtures() -> dict[str, dict]:
    meta = {"generator": "tools/fixtures/env.py", "unitypy": UnityPy.__version__}
    inputs = scene_inputs()
    plain = [scene_case(s) for s in inputs if "dependencies" not in s]
    deps = [scene_case(s) for s in inputs if "dependencies" in s]
    return {
        "env.json": {**meta, "scenes": plain, "dependency_scenes": deps},
        "paths.json": {**meta, "cases": [path_case(c) for c in path_inputs()]},
        "pystr.json": {**meta, **pystr_fixture()},
        "resolver.json": {**meta, **resolver_fixture()},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    ap.add_argument("--g5", type=Path, help="check env_corpus.rs's output against UnityPy, write nothing")
    ap.add_argument("--corpus", type=Path, help="the corpus root the --g5 output's bundle paths are under")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2":
        print(f"needs UnityPy 1.25.2, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    if args.g5:
        return check_g5(args.g5, args.corpus)
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
