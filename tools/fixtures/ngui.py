"""Oracle fixture for the NGUI cutter (docs/plans/extract.md E21 to E26, X5): the reference's outputs on the testkit scenes.

Runs in the oracle image against UnityPy 1.25.2 and Pillow 9.5.0 and writes
crates/unityrs-testkit/tests/fixtures/ngui.json, which the Rust cutter's tests replay without Python, and whose
scenes `crates/unityrs-testkit/tests/ngui_scenes.rs` holds the testkit's builders to. The inputs are
`unityrs_testkit::ngui::scenes()`, built in code, whose bytes this script asks cargo for
(`tools/parity/ngui_scenes.py`); no other file is read. The outputs are `tools/parity/ngui_cutter.py`'s (section 5
of docs/BATCH_SPEC.md on each input over its view) and `tools/parity/ngui_winners.py`'s (4.5's winners).

    scripts/dev python -E -s tools/fixtures/ngui.py           # write the fixture
    scripts/dev python -E -s tools/fixtures/ngui.py --check   # regenerate and compare

`scenes`: each scene's bundles (id, sha256, `deps`), inputs and options. `results`, one per scene and input: each
output in export order (its kind, key, unit key, serialized file, path id and sprite index; a JSON's text, MD5 and
SHA-256; an image's mode, size, the SHA-256 of its pixels (`tobytes()`), those pixels as hex for at most 64 of
them, and its PNG's SHA-256 and MD5), the skips in record order (docs/BATCH_SPEC.md 4.4, with the output each
skips and the sprite's index, which 7.3's `skips` rows leave out), `unresolved_scripts`, the textures the plan
keys that no atlas cuts, and each found sheet with the step that found it. `winners`: the winners scene's results decided with several `prefer` maps, failed inputs
and `publish_incomplete` (each key's status, reason, flip, winner and every candidate's rule).

The PNG bytes depend on the zlib Pillow loads; the reference is zlib 1.2.13, the oracle image's (as
`tools/fixtures/imageio.py`'s). Writing refuses any other. `--check` under another zlib (a runner's own) compares
everything but the PNGs' SHA-256 and MD5, and says so on stderr: the pixels, every JSON byte and every decision
are compared under any zlib, the winners' `prefer` MD5s being JSON MD5s.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-testkit/tests/fixtures/ngui.json"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import ngui_scenes  # noqa: E402
import ngui_winners  # noqa: E402
import PIL  # noqa: E402
import UnityPy  # noqa: E402
from PIL import Image  # noqa: E402

from scaffold import dump  # noqa: E402

REFERENCE_ZLIB = "1.2.13"
# The fields that depend on the zlib Pillow loads.
ZLIB_FIELDS = ("png_sha256", "png_md5")
HEX_PIXELS = 64


def output_row(o) -> dict:
    row = {"kind": o.kind, "key": o.key, "unit": o.unit, "serialized_file": o.serialized_file, "path_id": o.path_id,
           "sprite": o.sprite}  # fmt: skip
    if o.kind == "atlas_json":
        row.update(text=o.data.decode("ascii"), md5=hashlib.md5(o.data).hexdigest(),
                   sha256=hashlib.sha256(o.data).hexdigest())  # fmt: skip
        return row
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
    return {"name": scene.name, "bundles": bundles, "inputs": scene.inputs, "options": scene.row["options"]}


def winner_cases(results) -> list[dict]:
    """The winners scene decided several ways: all five inputs, then `a` to `d` (no unit conflict) with each
    rule of E31, E61's withholding and `publish_incomplete` false."""
    four = [r for r in results if r.input != "e.bundle"]
    c_json = next(o.data for r in four if r.input == "c.bundle" for o in r.outputs if o.key == "shared.json")
    cases = [
        ("five_inputs", results, {}, [], True),
        ("first_input", four, {}, [], True),
        ("prefer_input", four, {"shared.json": {"input": "c.bundle", "md5": None}}, [], True),
        ("prefer_md5", four, {"shared.json": {"input": None, "md5": hashlib.md5(c_json).hexdigest()}}, [], True),
        ("member_key_entry", four, {"k.png": {"input": "d.bundle", "md5": None}}, [], True),
        ("withheld_unit", four, {"shared.json": {"input": "z.bundle", "md5": None}}, ["z.bundle"], True),
        ("withheld_plain", four, {"h.png": {"input": "z.bundle", "md5": None}}, ["z.bundle"], True),
        ("publish_incomplete", four, {}, ["z.bundle"], False),
    ]
    out = []
    for name, rs, prefer, failed, publish_incomplete in cases:
        decided = ngui_winners.winners(rs, prefer=prefer, failed=failed, publish_incomplete=publish_incomplete)
        out.append({"case": name, "inputs": [r.input for r in rs], "prefer": prefer, "failed": failed,
                    "publish_incomplete": publish_incomplete, "decisions": ngui_winners.report(decided)})  # fmt: skip
    return out


def fixture() -> dict:
    scenes = ngui_scenes.scenes()
    results, winners = [], []
    for scene in scenes.values():
        rs = ngui_scenes.run(scene)
        for r in rs:
            results.append({
                "scene": scene.name,
                "input": r.input,
                "outputs": [output_row(o) for o in r.outputs],
                "skips": [s.__dict__ for s in r.skips],
                "unresolved_scripts": r.unresolved_scripts,
                "uncut_textures": [list(t) for t in r.uncut_textures],
                "sheets": [list(t) for t in r.sheets],
            })  # fmt: skip
        if scene.name == "winners":
            winners = winner_cases(rs)
    return {
        "zlib": Image.core.zlib_version,
        "scenes": [scene_row(s) for s in scenes.values()],
        "results": results,
        "winners": winners,
    }


def without_zlib(body: dict) -> dict:
    """The fixture less what depends on the zlib Pillow loads."""
    results = [
        {**r, "outputs": [{k: v for k, v in o.items() if k not in ZLIB_FIELDS} for o in r["outputs"]]}
        for r in body["results"]
    ]
    return {**{k: v for k, v in body.items() if k not in ("results", "zlib")}, "results": results}


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
            f"ngui.py: Pillow loads zlib {loaded}, not {REFERENCE_ZLIB}: the PNGs' SHA-256 and MD5 are not compared "
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
