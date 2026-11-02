"""UnityPy's container preload walk with another load made at one point of it,
for `env.py`'s race scenes (unityrs-testkit's `env::race_scenes`, review
round 14).

UnityPy runs on one thread, so "another thread's load at a point of the
walk" is, for UnityPy, the load made there: before the walk (`"before"`),
before the walk looks at element `x` of the preload span of container row
`r` (`[r, x]`; `x` the span's end is after the row, before the walk's next
look, or at its end), or after the walk (`"after"`).
`helpers/ContainerHelper.py:31-49` runs unchanged, on stand-ins for the
walked table's elements and rows that only tell each look: an element's
`bool` (`if not pptr`, the walk's first use of each element) and a row's
`preloadIndex` (its first use of each row). The core's walk is stopped at
each point it reaches and the load made by another thread there
(`crates/unityrs-core/src/env/race_tests.rs`).

A load publishes its keys one at a time (plan section 3.16 item 6), and the
core's other load is also run stopped right after it published its first
key, then let go once the walk has ended. For a load that registers more
than one key, each point inside the walk is recorded that way too
(`first_key`): the load made there with only its first `register_cab`, and
the rest made after the walk, in their order (review round 15).
"""

from __future__ import annotations

import contextlib
import io
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))

from UnityPy.helpers.ContainerHelper import ContainerHelper  # noqa: E402

import scratch  # noqa: E402

WALK = ContainerHelper.parse_preload_table


class Race:
    """Where the other load is made, and the looks the walk has made; with
    `first_key`, the load's registrations past its first are held back
    until `rest`."""

    def __init__(self, run, at, first_key: bool = False):
        self.run = run
        self.at = tuple(at) if isinstance(at, list) else None
        self.first_key = first_key
        self.made = False
        self.row = None
        self.looks: list[tuple[int, int]] = []
        self.keys = 0
        self.held: list = []

    def make(self):
        name, data = self.run.scene["other"]
        env = self.run.env
        register = env.register_cab

        def counted(cab, item):
            self.keys += 1
            if self.first_key and self.keys > 1:
                self.held.append((cab, item))
            else:
                register(cab, item)

        # `File.read_files` and `load_file` call it through the instance.
        env.register_cab = counted
        try:
            env.load_file(bytes.fromhex(data), name=name)
        finally:
            del env.register_cab
        self.made = True

    def rest(self):
        """The registrations held back, made now."""
        for cab, item in self.held:
            self.run.env.register_cab(cab, item)
        self.held = []

    def look(self, index: int):
        """The walk looks at element `index` of the current row's span."""
        here = (self.row, index)
        self.looks.append(here)
        if self.at is not None and not self.made and here >= self.at:
            self.make()

    def end(self):
        """The walk is done: a point past its last look is here."""
        if self.at is not None and not self.made:
            self.make()


class Element:
    """A preload table element, which tells the race when the walk looks."""

    __slots__ = ("pptr", "index", "race")

    def __init__(self, pptr, index: int, race: Race):
        self.pptr, self.index, self.race = pptr, index, race

    def __bool__(self) -> bool:
        self.race.look(self.index)
        return bool(self.pptr)

    def deref(self):
        return self.pptr.deref()

    @property
    def path_id(self):
        return self.pptr.path_id


class Row:
    """A container row's AssetInfo, which tells the race a row begins."""

    __slots__ = ("info", "r", "race")

    def __init__(self, info, r: int, race: Race):
        self.info, self.r, self.race = info, r, race

    @property
    def preloadIndex(self):  # noqa: N802 - UnityPy's name
        self.race.row = self.r
        return self.info.preloadIndex

    @property
    def preloadSize(self):  # noqa: N802 - UnityPy's name
        return self.info.preloadSize


@contextlib.contextmanager
def watched(race: Race):
    """Every table walked meanwhile is walked through stand-ins telling
    `race` (a race scene has one)."""

    def walk(helper):
        if helper._preload_table is None:
            return WALK(helper)
        rows = helper.container
        helper._preload_table = [Element(p, j, race) for j, p in enumerate(helper._preload_table)]
        helper.container = [(path, Row(info, r, race)) for r, (path, info) in enumerate(rows)]
        try:
            return WALK(helper)
        finally:
            helper.container = rows
            race.end()

    ContainerHelper.parse_preload_table = walk
    try:
        yield
    finally:
        ContainerHelper.parse_preload_table = WALK


def race_point(scene: dict, new_run, at, first_key: bool = False) -> tuple[dict, list, int]:
    """The scene with the other load made at `at` (with `first_key`, only
    its first registration there, the rest after the walk): what it
    records, the looks the walk made, and the keys the load registered."""
    with scratch.folder() as tmp:
        for name, data in scene["resolver"]:
            (Path(tmp) / name).write_bytes(bytes.fromhex(data))
        with contextlib.redirect_stdout(io.StringIO()):
            run = new_run(scene, str(tmp))
            for i in range(len(scene["files"])):
                run.load(i)
            race = Race(run, at, first_key)
            if at == "before":
                race.make()
            with watched(race):
                container = run.container()
            race.rest()
            if at == "after":
                race.make()
            assert race.made, at
            out = {
                "at": at,
                "container": container,
                "paths": run.paths(),
                "dep_paths": run.dep_paths(),
                **run.state(),
            }
    return out, race.looks, race.keys


# What a run whose load is stopped after its first key is compared on: the
# rest of the record lists the load's `files` entry, which the core makes
# after the walk (`race_tests.rs`'s `compared`).
FIRST_KEY = ("container", "paths", "dep_paths", "cabs")


def race_case(scene: dict, new_run) -> dict:
    """Every point of the walk: before it, before each element of each
    row's span and after each row, after it; for a load of more than one
    key, each point inside the walk also with only its first key there."""
    _, looks, keys = race_point(scene, new_run, "after")
    spans: dict[int, list[int]] = {}
    for r, j in looks:
        spans.setdefault(r, []).append(j)
    points: list = ["before"]
    for r in sorted(spans):
        points.extend([r, x] for x in range(min(spans[r]), max(spans[r]) + 2))
    points.append("after")
    recorded = []
    for at in points:
        out = race_point(scene, new_run, at)[0]
        if keys > 1 and isinstance(at, list):
            first = race_point(scene, new_run, at, first_key=True)[0]
            out["first_key"] = {k: first[k] for k in FIRST_KEY}
        recorded.append(out)
    case = {k: scene[k] for k in ("name", "files", "resolver", "other")}
    return {**case, "points": recorded}
