"""Oracle fixtures for unityrs-texture's sprite renderer: UnityPy 1.25.2's SpriteHelper (docs/plans/extract.md E17,
PARITY.md section 15.2), and numpy's solutions of its affine systems (section 3, item 6).

Runs in the oracle image against UnityPy 1.25.2, Pillow 9.5.0 and numpy's OpenBLAS pinned to its Haswell kernels
(`OPENBLAS_CORETYPE=Haswell`, the reference host's; tools/parity/oracle_env.py's core check, which the script runs
first), and writes two fixtures, one entry a line, which `cargo test -p unityrs-texture` replays without Python:
crates/unityrs-texture/tests/fixtures/sprite.json (tests/sprite_oracle.rs) and sprite_solve.json
(tests/sprite_solve.rs). Every input is built here in code; no file is read.

    OPENBLAS_CORETYPE=Haswell scripts/dev python -E -s tools/fixtures/sprite.py           # write the fixtures
    OPENBLAS_CORETYPE=Haswell scripts/dev python -E -s tools/fixtures/sprite.py --check   # regenerate and compare
    scripts/dev python -E -s tools/fixtures/sprite.py --check [--reference]

Writing needs the Haswell kernels, and a pin set to anything else fails the core check. A `--check` on other kernels
compares every sprite case numpy solves nothing for, and skips the rest (each case records `solves`, how many systems
numpy solved in its UnityPy run) and sprite_solve.json, saying how many; `--reference`, which CI's step passes on the
reference host, fails there instead (docs/plans/extract.md section 3, item 6).

**A sprite case** is a Sprite, and where it names one a SpriteAtlas, each laid out from a hand-built tree
(sprite_build.py, which names each tree's sources), written by UnityPy's `TypeTreeHelper.write_typetree` from a
dict-mode value and read back in object mode, as meshdecode.py reads its objects; the cases themselves are
sprite_cases.py's. Its textures are RGBA32 or RGB24 images of SplitMix64 bytes, which
`Texture2DConverter.get_image_from_texture2d` decodes from a Texture2D-like object (`texture`); each `PPtr`'s
`deref_parse_as_object` returns the case's object of that path id, and the Sprite's `assets_file` holds an empty cache
and, for an `m_AtlasTags` lookup, the atlas as an object reader. Then `SpriteHelper.get_image_from_sprite(sprite)`
runs, and the case records the image (its mode, size and `tobytes()` as hex, or, past 4,096 bytes, their sha256 and
how many pixels are not all zero) or the exception (its class's `__name__` and message), and `paths`, how many of its
affine copies took each of Pillow's paths (`ImagingTransformAffine`: `scale` where the map has no shear, `fixed` where
all four of the output's corners pass `check_fixed`, else `float`), each computed by `pillow_path`, which repeats
Geometry.c's decision from the size and coefficients of each `Image.transform` call (Pillow reports no path). A case
that names a `path` must take that one alone, which the recorder asserts. A case that raises `LinAlgError` also
records, as `tier_c`, the image UnityPy gives when `copy_triangle` returns for a singular system instead (`PARITY.md`
section 15.2's Tier C row), which the crate renders.

**A solve case** (sprite_solve.py) is a system of `copy_triangle`'s shape and numpy's solution's bits.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from collections import Counter
from contextlib import ExitStack, suppress
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-texture/tests/fixtures"
sys.path.insert(0, str(ROOT / "tools/fixtures"))
sys.path.insert(0, str(ROOT / "tools/parity"))

import numpy as np  # noqa: E402
import PIL  # noqa: E402
import sprite_solve  # noqa: E402
import UnityPy  # noqa: E402
from meshdecode_build import to_node  # noqa: E402
from PIL import Image  # noqa: E402
from sprite_build import SPRITE_ROWS, TREES, sheet_bytes  # noqa: E402
from sprite_cases import cases as sprite_cases  # noqa: E402
from UnityPy.classes import PPtr  # noqa: E402
from UnityPy.enums import BuildTarget, ClassIDType, TextureFormat  # noqa: E402
from UnityPy.export import SpriteHelper  # noqa: E402
from UnityPy.helpers import TypeTreeHelper  # noqa: E402
from UnityPy.helpers.UnityVersion import UnityVersion  # noqa: E402
from UnityPy.streams import EndianBinaryReader  # noqa: E402

import oracle_env  # noqa: E402

WINDOWS = int(BuildTarget.StandaloneWindows64)
FORMATS = {"RGBA32": int(TextureFormat.RGBA32), "RGB24": int(TextureFormat.RGB24)}
# An image past this many bytes is recorded by its digest.
HEX_LIMIT = 4096


def texture(t: dict) -> SimpleNamespace:
    """What `get_image_from_texture2d` reads of a Texture2D (tools/fixtures/texture.py's `fake_texture`)."""
    data = sheet_bytes(t)
    return SimpleNamespace(
        get_image_data=lambda: data, m_Width=t["width"], m_Height=t["height"], m_TextureFormat=FORMATS[t["format"]],
        object_reader=SimpleNamespace(version=(2021, 3, 1, 1), platform=WINDOWS), m_PlatformBlob=None,
    )  # fmt: skip


def read(rows: list[list], hexed: str):
    data = bytes.fromhex(hexed)
    return TypeTreeHelper.read_typetree(to_node(rows), EndianBinaryReader(data, "<"), False, len(data))


def image(im) -> dict:
    raw = im.tobytes()
    if len(raw) <= HEX_LIMIT:
        return {"mode": im.mode, "size": list(im.size), "hex": raw.hex()}
    n = len(im.mode)
    nonzero = sum(1 for i in range(0, len(raw), n) if any(raw[i : i + n]))
    return {"mode": im.mode, "size": list(im.size), "sha256": hashlib.sha256(raw).hexdigest(), "nonzero": nonzero}


def setup(case: dict):
    """The Sprite read back, with its object reader (the version, and a file of an empty cache and the atlas), and
    the case's objects by path id."""
    sprite = read(SPRITE_ROWS, case["sprite"])
    objects = {int(k): texture(t) for k, t in case["textures"].items()}
    readers = {}
    if "atlas" in case:
        objects[5] = read(TREES[case.get("atlas_tree", "atlas")], case["atlas"])
        readers[5] = SimpleNamespace(
            type=ClassIDType.SpriteAtlas, peek_name=lambda: objects[5].m_Name, parse_as_object=lambda: objects[5]
        )
    assets_file = SimpleNamespace(_cache={}, objects=readers)
    sprite.object_reader = SimpleNamespace(version=UnityVersion.from_list(*case["version"]), assets_file=assets_file)
    return sprite, objects


def pillow_path(size, data) -> str:
    """The path Pillow 9.5.0's `ImagingTransformAffine` takes for an AFFINE transform to `size` by `data` (NEAREST):
    `src/libImaging/Geometry.c`'s scale test, then `check_fixed` at the output's four corners."""
    a = [float(v) for v in data[:6]]
    if a[1] == 0 and a[3] == 0:
        return "scale"
    w, h = size

    def fixed(x, y):
        return abs(x * a[0] + y * a[1] + a[2]) < 32768.0 and abs(x * a[3] + y * a[4] + a[5]) < 32768.0

    return "fixed" if all(fixed(x, y) for x, y in ((0, 0), (w, h), (0, h), (w, 0))) else "float"


def run(sprite, objects: dict, singular_copies_nothing: bool = False):
    """One UnityPy run: (its image, how many systems numpy solved, how many transforms took each path)."""
    sprite.object_reader.assets_file._cache.clear()
    copy = SpriteHelper.copy_triangle
    solve = np.linalg.solve
    transform = Image.Image.transform
    solves, paths = [0], Counter()

    def counted(*args):
        solves[0] += 1
        return solve(*args)

    def traced(self, size, method, data=None, *args, **kwargs):
        paths[pillow_path(size, data)] += 1
        return transform(self, size, method, data, *args, **kwargs)

    def tolerant(*args):
        with suppress(np.linalg.LinAlgError):
            copy(*args)

    with ExitStack() as stack:
        stack.enter_context(mock.patch.object(PPtr, "deref_parse_as_object", lambda self: objects[self.m_PathID]))
        stack.enter_context(mock.patch.object(np.linalg, "solve", counted))
        stack.enter_context(mock.patch.object(Image.Image, "transform", traced))
        if singular_copies_nothing:
            stack.enter_context(mock.patch.object(SpriteHelper, "copy_triangle", tolerant))
        try:
            return SpriteHelper.get_image_from_sprite(sprite), solves[0], dict(paths)
        except Exception as e:  # noqa: BLE001 - every exception is an outcome to record
            e.solves, e.paths = solves[0], dict(paths)
            raise


def outcome(case: dict) -> dict:
    sprite, objects = setup(case)
    try:
        im, solves, paths = run(sprite, objects)
        out = {"image": image(im)}
    except Exception as e:  # noqa: BLE001 - as above
        solves, paths = e.solves, e.paths
        out = {"err": [type(e).__name__, str(e)]}
        if isinstance(e, np.linalg.LinAlgError):
            try:
                im, _, _ = run(sprite, objects, singular_copies_nothing=True)
                out["tier_c"] = {"image": image(im)}
            except Exception as e2:  # noqa: BLE001 - as above
                out["tier_c"] = {"err": [type(e2).__name__, str(e2)]}
    if "path" in case:
        assert set(paths) == {case["path"]}, f"{case['name']} takes {paths}, not the {case['path']} path alone"
    # How many systems numpy solved in the UnityPy run: an outcome that depends on the kernel's bits where it is not 0.
    return {**case, **out, "solves": solves, **({"paths": paths} if paths else {})}


def solve_cases() -> list[dict]:
    return [{**s, "a": sprite_solve.solution(np, s["src"], s["dst"])} for s in sprite_solve.systems()]


def fixtures(core: str | None) -> dict[str, dict]:
    meta = {"generator": "tools/fixtures/sprite.py", "unitypy": UnityPy.__version__, "pillow": PIL.__version__,
            "numpy": np.__version__, "openblas_core": core}  # fmt: skip
    return {
        "sprite.json": {**meta, "trees": TREES, "cases": [outcome(c) for c in sprite_cases()]},
        "sprite_solve.json": {**meta, "seed": sprite_solve.SEED, "cases": solve_cases()},
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
        else:
            lines.append(f" {json.dumps(key)}: {compact(value)}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def without_solves(body: dict) -> dict:
    """A sprite fixture less what the kernel decides: its core, and each case numpy solved for."""
    meta = {k: v for k, v in body.items() if k not in ("openblas_core", "cases")}
    return {**meta, "cases": [c for c in body["cases"] if not c["solves"]]}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    ap.add_argument("--reference", action="store_true", help="with --check: fail off the Haswell kernels")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2" or PIL.__version__ != "9.5.0":
        print(f"needs UnityPy 1.25.2 and Pillow 9.5.0, found {UnityPy.__version__}, {PIL.__version__}", file=sys.stderr)
        return 2
    core = oracle_env.openblas_core()
    problems, notes = oracle_env.core_problems(core)
    for line in problems + notes:
        print(f"sprite.py: {line}", file=sys.stderr)
    if problems:
        return 2
    pinned = core == oracle_env.REFERENCE_CORE
    if not pinned and (args.reference or not args.check):
        need = "needs numpy's OpenBLAS on its Haswell kernels (OPENBLAS_CORETYPE=Haswell)"
        print(f"{need}, found {core}", file=sys.stderr)
        return 2
    stale = []
    for name, body in fixtures(core).items():
        text = dump(body)
        path = OUT / name
        if args.check and not pinned:
            # Off the reference host's kernels: every case numpy solves for is skipped (docs/plans/extract.md
            # section 3, item 6), and said so; the solve fixture is all such cases.
            if name == "sprite_solve.json":
                print(f"{name}: all {len(body['cases'])} systems skipped on the {core} kernels", file=sys.stderr)
                continue
            kept = json.loads(path.read_text()) if path.exists() else None
            if kept is None or without_solves(kept) != without_solves(body):
                stale.append(name)
            skipped = sum(1 for c in body["cases"] if c["solves"])
            print(f"{name}: {skipped} cases that call solve skipped on the {core} kernels", file=sys.stderr)
        elif args.check:
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
