"""Oracle fixtures for package D (docs/plans/core.md section 7.5): the Environment and Python strings.

Runs in the oracle image against UnityPy 1.25.2 and writes small JSON files under
crates/unityrs-core/tests/fixtures/d/, which `cargo test -p unityrs-core` replays
without Python (crates/unityrs-core/tests/env_fixtures.rs). Every input is built
in code: the scenes by unityrs-testkit (`env::scenes`), whose bytes this script
asks cargo for, and the strings here. No file is read.

    scripts/dev python tools/fixtures/env.py           # write the fixtures
    scripts/dev python tools/fixtures/env.py --check   # regenerate and compare

`env.json`: each scene is run through one script of steps (load each file in
order, read every object, dereference every pointer, read every resource,
dereference every pointer again (a resource fallback may have loaded files),
list the container, ask every object its `container` path, list the container
again), and after each step the Environment's state is recorded:
`env.files` (key, `is_dependency`, kind), `env.cabs` keys in order, and
`env.objects` (file, path id, class id). The resolver's files sit in a
temporary `env.path`; a key under it is written `@/<name>`.

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
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-core/tests/fixtures/d"
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import UnityPy  # noqa: E402
from UnityPy.classes import PPtr  # noqa: E402
from UnityPy.environment import reSplit  # noqa: E402
from UnityPy.files import BundleFile, SerializedFile  # noqa: E402
from UnityPy.helpers.ResourceReader import get_resource_data  # noqa: E402

from scaffold import dump  # noqa: E402

# ------------------------------------------------------------------ scenes


def scene_inputs() -> list[dict]:
    """unityrs-testkit's env::scenes(), as the replay test writes them."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "scenes.jsonl"
        env = dict(os.environ, UNITYRS_WRITE_SCENES=str(out))
        subprocess.run(
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
            stdout=subprocess.DEVNULL,
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

    def key(self, name: str) -> str:
        return name.replace(self.path, "@", 1) if name.startswith(self.path) else name

    def state(self) -> dict:
        env = self.env
        return {
            "files": [[self.key(k), bool(getattr(f, "is_dependency", False)), kind(f)] for k, f in env.files.items()],
            "cabs": list(env.cabs.keys()),
            "objects": [[self.key(o.assets_file.name), o.path_id, o.class_id] for o in env.objects],
        }

    def load(self, i: int):
        name, data = self.scene["files"][i]
        try:
            self.env.load_file(bytes.fromhex(data), name=name)
        except Exception:  # noqa: BLE001 - a failed load is a recorded outcome
            return "error"
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

    def derefs(self):
        out = []
        for o in self.objects():
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

    def resources(self):
        out = []
        for o in self.objects():
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

    def paths(self):
        out = []
        for o in self.objects():
            try:
                c = o.container
            except Exception as e:  # noqa: BLE001
                out.append(type(e).__name__)
                continue
            out.append(c if c is None or isinstance(c, str) else "<not a str>")
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
        return out


def scene_case(scene: dict) -> dict:
    with tempfile.TemporaryDirectory() as tmp:
        for name, data in scene["resolver"]:
            (Path(tmp) / name).write_bytes(bytes.fromhex(data))
        with contextlib.redirect_stdout(io.StringIO()):
            steps = Run(scene, tmp).steps()
    return {"name": scene["name"], "files": scene["files"], "resolver": scene["resolver"], "steps": steps}


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
    with tempfile.TemporaryDirectory() as empty:
        return check_g5_in(report, rels, corpus, empty)


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
    return {
        "env.json": {**meta, "scenes": [scene_case(s) for s in scene_inputs()]},
        "pystr.json": {**meta, **pystr_fixture()},
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
