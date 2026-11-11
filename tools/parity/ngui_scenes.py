"""The NGUI scenes the cutter's reference is tested and recorded on, as unityrs-testkit builds them.

`unityrs_testkit::ngui::scenes()` builds each scene's bundles in code (synthetic, never game data). Its test
`crates/unityrs-testkit/tests/ngui_scenes.rs` writes them, one JSON line a scene, when `UNITYRS_WRITE_NGUI_SCENES`
names a file; `rows()` runs that test through `child.run` into a scratch folder and reads the lines back. A scene
is its bundles (id, bytes, `deps`), its inputs and the cutter's options; `run` cuts each input over its view
(docs/BATCH_SPEC.md 4.2). `tools/parity/tests/test_ngui_cutter.py` and `tools/fixtures/ngui.py` share it.
"""

from __future__ import annotations

import functools
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import child  # noqa: E402
import ngui_cutter  # noqa: E402
import scratch  # noqa: E402


@dataclass(frozen=True)
class Scene:
    name: str
    bundles: dict[str, bytes]
    deps: dict[str, list[str]]
    inputs: list[str]
    options: ngui_cutter.Options
    row: dict  # the line as the test wrote it, the bundles' hex among it

    def view(self, x: str) -> dict[str, bytes]:
        return {b: self.bundles[b] for b in ngui_cutter.view_of(self.deps, x)}


def rows() -> list[dict]:
    """The scenes as the testkit's test writes them (cargo builds the testkit's tests on the way)."""
    with scratch.folder() as tmp:
        out = Path(tmp) / "ngui_scenes.jsonl"
        env = dict(os.environ, UNITYRS_WRITE_NGUI_SCENES=str(out))
        args = ["cargo", "test", "-q", "-j", "2", "--locked", "-p", "unityrs-testkit", "--test", "ngui_scenes"]
        args += ["--", "--exact", "scenes_are_the_recorded_scenes"]
        child.run(args, cwd=ROOT, env=env, check=True, stdout=child.DEVNULL)
        return [json.loads(line) for line in out.read_text().splitlines()]


def options(o: dict) -> ngui_cutter.Options:
    return ngui_cutter.Options(
        sprite_key=o["sprite_key"],
        json_key=o["json_key"],
        texture_key=o["texture_key"],
        script_classes=tuple(o["script_classes"]),
        skip_name_suffixes=tuple(o["skip_name_suffixes"]),
    )


@functools.cache
def scenes() -> dict[str, Scene]:
    """Every scene, by name, in the testkit's order."""
    out = {}
    for r in rows():
        bundles = {b["id"]: bytes.fromhex(b["hex"]) for b in r["bundles"]}
        deps = {b["id"]: b["deps"] for b in r["bundles"]}
        out[r["name"]] = Scene(r["name"], bundles, deps, r["inputs"], options(r["options"]), r)
    return out


def run(scene: Scene) -> list[ngui_cutter.Result]:
    """The cutter's result for each of the scene's inputs, in its order."""
    return [ngui_cutter.cut(x, scene.view(x), scene.options) for x in scene.inputs]
