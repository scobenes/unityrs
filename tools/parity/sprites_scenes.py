"""The `sprites` kind's scenes the reference (`sprites_kind.py`) is recorded on, as unityrs-testkit builds them.

`unityrs_testkit::ngui::sprite_scenes()` builds each scene's bundles in code (synthetic, never game data). Its test
`crates/unityrs-testkit/tests/sprite_scenes.rs` writes them, one JSON line a scene, when
`UNITYRS_WRITE_SPRITE_SCENES` names a file; `rows()` runs that test through `child.run` into a scratch folder and
reads the lines back. A scene is its bundles (id, bytes, `deps`), its inputs and the `sprites` key; `run` exports
each input's Sprites over its view (docs/BATCH_SPEC.md 4.2). `tools/fixtures/sprites.py` uses it.
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
import sprites_kind  # noqa: E402


@dataclass(frozen=True)
class Scene:
    name: str
    bundles: dict[str, bytes]
    deps: dict[str, list[str]]
    inputs: list[str]
    key: str
    row: dict  # the line as the test wrote it, the bundles' hex among it

    def view(self, x: str) -> dict[str, bytes]:
        return {b: self.bundles[b] for b in ngui_cutter.view_of(self.deps, x)}


def rows() -> list[dict]:
    """The scenes as the testkit's test writes them (cargo builds the testkit's tests on the way)."""
    with scratch.folder() as tmp:
        out = Path(tmp) / "sprite_scenes.jsonl"
        env = dict(os.environ, UNITYRS_WRITE_SPRITE_SCENES=str(out))
        args = ["cargo", "test", "-q", "-j", "2", "--locked", "-p", "unityrs-testkit", "--test", "sprite_scenes"]
        args += ["--", "--exact", "sprite_scenes_are_the_recorded_scenes"]
        child.run(args, cwd=ROOT, env=env, check=True, stdout=child.DEVNULL)
        return [json.loads(line) for line in out.read_text().splitlines()]


@functools.cache
def scenes() -> dict[str, Scene]:
    """Every scene, by name, in the testkit's order."""
    out = {}
    for r in rows():
        bundles = {b["id"]: bytes.fromhex(b["hex"]) for b in r["bundles"]}
        deps = {b["id"]: b["deps"] for b in r["bundles"]}
        out[r["name"]] = Scene(r["name"], bundles, deps, r["inputs"], r["key"], r)
    return out


def run(scene: Scene) -> list[ngui_cutter.Result]:
    """The kind's result for each of the scene's inputs, in its order."""
    return [sprites_kind.cut(x, scene.view(x), scene.key) for x in scene.inputs]
