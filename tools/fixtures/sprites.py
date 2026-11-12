"""Oracle fixture for the `sprites` output kind (docs/plans/extract.md E17, X2, X6): the reference on the testkit
scenes.

Runs in the oracle image against UnityPy 1.25.2 and Pillow 9.5.0 and writes
crates/unityrs-testkit/tests/fixtures/sprites.json, which the engine's tests replay without Python
(`crates/unityrs-export/tests/sprites_fixture.rs`), and whose scenes `crates/unityrs-testkit/tests/sprite_scenes.rs`
holds the testkit's builders to. The inputs are `unityrs_testkit::ngui::sprite_scenes()`, built in code, whose
bytes this script asks cargo for (`tools/parity/sprites_scenes.py`); no other file is read. The outputs are
`tools/parity/sprites_kind.py`'s (the kind on each input over its view) and `tools/parity/ngui_winners.py`'s (4.5's
winners, a Sprite's output a plain candidate).

    scripts/dev python -E -s tools/fixtures/sprites.py           # write the fixture
    scripts/dev python -E -s tools/fixtures/sprites.py --check   # regenerate and compare

`scenes`: each scene's bundles (id, sha256, `deps`), inputs and key. `results`, one per scene and input: each output
in export order (its key, serialized file and path id; its image's mode, size, the SHA-256 of its pixels
(`tobytes()`), those pixels as hex for at most 64 of them, and its PNG's SHA-256 and MD5), and the skips in record
order (7.3's `skips` rows). `winners`: the winners scene's results decided with several `prefer` maps.

The PNG bytes depend on the zlib Pillow loads; the reference is zlib 1.2.13, the oracle image's (as
`tools/fixtures/ngui.py`'s). Writing refuses any other. `--check` under another zlib compares everything but the PNGs'
SHA-256 and MD5 and the winner case whose `prefer` names a PNG's MD5 (`zlib`), and says so on stderr.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-testkit/tests/fixtures/sprites.json"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import PIL  # noqa: E402
import UnityPy  # noqa: E402
from PIL import Image  # noqa: E402
from scaffold import dump  # noqa: E402

import ngui_winners  # noqa: E402
import sprites_scenes  # noqa: E402

REFERENCE_ZLIB = "1.2.13"
# The fields that depend on the zlib Pillow loads.
ZLIB_FIELDS = ("png_sha256", "png_md5")
HEX_PIXELS = 64


def output_row(o) -> dict:
    row = {"key": o.key, "serialized_file": o.serialized_file, "path_id": o.path_id}
    with Image.open(io.BytesIO(o.data)) as im:
        pixels = im.tobytes()
        row.update(mode=im.mode, size=list(im.size))
    row["pixels_sha256"] = hashlib.sha256(pixels).hexdigest()
    if o.size[0] * o.size[1] <= HEX_PIXELS:
        row["pixels"] = pixels.hex()
    row["png_sha256"] = hashlib.sha256(o.data).hexdigest()
    row["png_md5"] = hashlib.md5(o.data).hexdigest()
    return row


def scene_row(scene) -> dict:
    bundles = [{"id": b["id"], "sha256": hashlib.sha256(bytes.fromhex(b["hex"])).hexdigest(), "deps": b["deps"]}
               for b in scene.row["bundles"]]  # fmt: skip
    return {"name": scene.name, "bundles": bundles, "inputs": scene.inputs, "key": scene.key}


def winner_cases(results: list) -> list[dict]:
    """The winners scene decided several ways (docs/BATCH_SPEC.md 4.5): `shared.png` held by `a` and `b` alike and
    by `c` with other pixels, by the first input, a preferred input and a preferred MD5 (a PNG's: `zlib`)."""
    by = {r.input: r for r in results}
    md5 = hashlib.md5(next(o.data for o in by["c.bundle"].outputs if o.key == "shared.png")).hexdigest()
    cases = [
        ("first_input", {}),
        ("prefer_input", {"shared.png": {"input": "c.bundle", "md5": None}}),
        ("prefer_md5", {"shared.png": {"input": None, "md5": md5}}),
    ]
    out = []
    for name, prefer in cases:
        decided = ngui_winners.winners(results, prefer=prefer)
        row = {"case": name, "inputs": [r.input for r in results], "prefer": prefer,
               "decisions": ngui_winners.report(decided)}  # fmt: skip
        if name == "prefer_md5":
            row["zlib"] = True
        out.append(row)
    return out


def fixture() -> dict:
    scenes = sprites_scenes.scenes()
    results, ran = [], {}
    for scene in scenes.values():
        rs = sprites_scenes.run(scene)
        ran[scene.name] = rs
        for r in rs:
            results.append({
                "scene": scene.name,
                "input": r.input,
                "outputs": [output_row(o) for o in r.outputs],
                "skips": [{k: v for k, v in s.__dict__.items() if k != "sprite"} for s in r.skips],
            })  # fmt: skip
    return {
        "zlib": Image.core.zlib_version,
        "scenes": [scene_row(s) for s in scenes.values()],
        "results": results,
        "winners": winner_cases(ran["sprite_winners"]),
    }


def without_zlib(body: dict) -> dict:
    """The fixture less what depends on the zlib Pillow loads: the PNGs' digests, and the winner case that names one."""
    results = [
        {**r, "outputs": [{k: v for k, v in o.items() if k not in ZLIB_FIELDS} for o in r["outputs"]]}
        for r in body["results"]
    ]
    winners = [w for w in body["winners"] if not w.get("zlib")]
    return {**{k: v for k, v in body.items() if k not in ("results", "winners", "zlib")}, "results": results,
            "winners": winners}  # fmt: skip


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2" or PIL.__version__ != "9.5.0":
        print(f"needs UnityPy 1.25.2 and Pillow 9.5.0, found {UnityPy.__version__}, {PIL.__version__}", file=sys.stderr)
        return 2
    loaded = Image.core.zlib_version
    if not args.check and loaded != REFERENCE_ZLIB:
        print(f"Pillow loads zlib {loaded}; the fixture is written with zlib {REFERENCE_ZLIB}", file=sys.stderr)
        return 2
    body = fixture()
    if not args.check:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(dump(body))
        return 0
    if not OUT.exists():
        print(f"stale fixture: {OUT.name} is missing", file=sys.stderr)
        return 1
    if loaded == REFERENCE_ZLIB:
        same = OUT.read_text() == dump(body)
    else:
        print(
            f"sprites.py: Pillow loads zlib {loaded}, not {REFERENCE_ZLIB}: the PNGs' SHA-256 and MD5 are not compared "
            "(the oracle image compares them)",
            file=sys.stderr,
        )
        stored = json.loads(OUT.read_text())
        same = stored.get("zlib") == REFERENCE_ZLIB and without_zlib(stored) == without_zlib(body)
    if not same:
        print(f"stale fixture: {OUT.name}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
