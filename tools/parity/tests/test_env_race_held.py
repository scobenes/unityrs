"""tools/fixtures/env_race.py's `held_case` against UnityPy (fix pass 17's verifier). It runs a scene with the held
load's registrations past its first made after the later files' loads, and records it as the answer of the loads made
in the order of their numbers (what the core's racing loads end as, plan section 3.16, R1). So it must record a scene
exactly when UnityPy's two runs leave the same `env.cabs`, keys in order and values, and refuse every other scene.
Each scene's loads only register the names given, through the Environment's `register_cab` (the instance's, which
`held_case` replaces), so both runs, and the stand-in load `held_case` drives, are UnityPy's. Needs UnityPy 1.25.2
(the oracle image); skipped elsewhere.
"""

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

if ORACLE:
    import UnityPy

    sys.path.insert(0, str(ROOT / "tools" / "fixtures"))
    import env_race

# Each scene's files, in the order of their numbers, with the names each load registers, in its order; the second
# file's load is the held one.
HELD = 1
SCENES = {
    # review round 16's verifier's scene: a later file registers the held key, so UnityPy's dict would hold the held
    # file's value.
    "a later file registers the held key": [["a", ["n"]], ["c", ["e0", "n"]], ["b", ["n"]]],
    "the held key simplifies to a later file's": [["a", ["n"]], ["c", ["E0", "dir/N"]], ["b", ["n"]]],
    "a later file's key simplifies to the held one": [["a", ["n"]], ["c", ["e0", "n"]], ["b", ["dir/N"]]],
    # fix pass 17: the held key would be listed after the key the later file added.
    "the held load adds a key after a later file added one": [["a", ["n"]], ["c", ["e0", "cab-z"]], ["b", ["n", "m"]]],
    # `nested_between_loads`'s registrations (env.py).
    "a key no other file makes": [["a", ["n"]], ["c", ["e0", "CAB-Z"]], ["b", ["n"]]],
    "only an earlier file registers the held key": [["a", ["n"]], ["c", ["e0", "n"]], ["b", ["m"]]],
    "the held key is an earlier file's while a later file adds one": [
        ["a", ["n", "k"]],
        ["c", ["e0", "k"]],
        ["b", ["m"]],
    ],
}


def unitypy(files: list, held: int | None = None) -> list:
    """UnityPy's `env.cabs`, as (key, value) pairs in order, after the loads of `files` in the order of their numbers;
    with `held`, that file's registrations past its first made after the later files' loads."""
    env = UnityPy.Environment()
    waiting = []
    for i, (name, keys) in enumerate(files):
        for j, key in enumerate(keys):
            if i == held and j > 0:
                waiting.append((key, f"{name}:{key}"))
            else:
                env.register_cab(key, f"{name}:{key}")
    for key, item in waiting:
        env.register_cab(key, item)
    return list(env.cabs.items())


class Loads:
    """The run `held_case` drives: file `i`'s load registers its names with the item `file:name`."""

    def __init__(self, scene: dict, _folder: str):
        self.scene = scene
        self.env = UnityPy.Environment()

    def load(self, i: int) -> str:
        name, keys = self.scene["files"][i]
        for key in keys:
            self.env.register_cab(key, f"{name}:{key}")
        return "ok"

    def container(self) -> list:
        return []

    def paths(self) -> list:
        return []

    def dep_paths(self) -> list:
        return []

    def state(self) -> dict:
        return {"cabs": list(self.env.cabs.items())}


def held_case(what: str, files: list) -> dict:
    return env_race.held_case({"name": what, "files": files}, Loads, HELD)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class HeldCase(unittest.TestCase):
    def test_a_scene_both_runs_agree_on_is_recorded_as_the_run_in_load_order(self):
        agreed = [what for what, files in SCENES.items() if unitypy(files, HELD) == unitypy(files)]
        self.assertEqual(len(agreed), 3, agreed)
        for what in agreed:
            with self.subTest(scene=what):
                self.assertEqual(held_case(what, SCENES[what])["cabs"], unitypy(SCENES[what]))

    def test_a_scene_the_runs_disagree_on_is_refused(self):
        differ = [what for what, files in SCENES.items() if unitypy(files, HELD) != unitypy(files)]
        self.assertEqual(len(differ), 4, differ)
        for what in differ:
            with self.subTest(scene=what):
                with self.assertRaisesRegex(AssertionError, "which this cannot record"):
                    held_case(what, SCENES[what])


if __name__ == "__main__":
    unittest.main()
