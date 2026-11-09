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

Two plain scenes are also recorded in the orders the core's unit test
`env::race_tests` runs their loads in on two threads (review round 16),
in `env_threads.json`: `nested_between_loads` with one load's
registrations past its first made only after the files after it are
loaded and the container listed (`held_case`, whose limit its docs state),
and `listings_raced` with its two files loaded in each order, under their
names, both under one name, and with the first found by `env.find_file` in
`env.path`, and three loads of its files under `x`, `y` and `x`, in the
order of their numbers and in the order the core's race lists them in
(`orders_case`). So is `env.files` after every set of the listings of the
core's unit test `env::listing_tests`, made in the order of their numbers
(`listings_case`; review round 16's follow-up: that test's expected values
were a hand-written model of a Python dict).
"""

from __future__ import annotations

import contextlib
import io
import sys
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))

import UnityPy  # noqa: E402
from UnityPy.classes import PPtr  # noqa: E402
from UnityPy.environment import simplify_name  # noqa: E402
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


# The plain scenes `env_threads.json` records, and how (module docs): a
# held scene by the index of the load whose registrations past its first
# are held, an orders scene by its runs, each the scene's files it loads
# (by index, in the order of their numbers), the names it loads them under
# (`None`: their own) and whether the first is found by `find_file`.
HELD = {"nested_between_loads": 1}
ORDERS = {
    "listings_raced": (
        ((0, 1), None, False),
        ((0, 1), ("x", "x"), False),
        ((0, 1), None, True),
        ((0, 0, 1), ("x", "y", "x"), False),
    )
}


def held_case(scene: dict, new_run, held: int) -> dict:
    """The scene's files loaded in order, file `held`'s load making only its
    first `register_cab` in its turn; the files after it loaded, the
    container listed (`before`), then the held registrations made, in their
    order; then the container, every object's `container` path, the
    dependencies' paths and the state.

    Its limit (review round 16's verifier): the held registrations are made
    after the later files' loads, so for a key a later file also registers
    UnityPy's dict holds the held file (the last assigned), while the core,
    whose racing loads end as if made in the order of their numbers (plan
    section 3.16, R1), holds the later file's. So this is UnityPy's answer
    only when no later file registers a key the held load registers past
    its first, which is asserted here. The scene it cannot record: `a`, `c`
    and `b` each holding a `SerializedFile` `n`, `c`'s load held after
    another first key, so that `c`'s `n` ranks between the key's position
    (`a`'s) and its value (`b`'s): here UnityPy would give `c`'s `n` as the
    value, where the core, and UnityPy's run of the three in the order of
    their numbers, give `b`'s. In `nested_between_loads` the held
    registration is `CAB-Z`, a key no other file makes, and `c`'s `n` is a
    `BundleFile`, which UnityPy never registers."""
    with scratch.folder() as tmp:
        with contextlib.redirect_stdout(io.StringIO()):
            run = new_run(scene, str(tmp))
            env = run.env
            register = env.register_cab
            made: list = []
            later: list = []
            after: list = []

            def first_only(cab, item):
                if made:
                    later.append((cab, item))
                else:
                    made.append(cab)
                    register(cab, item)

            def noted(cab, item):
                after.append(cab)
                register(cab, item)

            for i in range(len(scene["files"])):
                # `File.read_files` and `load_file` call it through the
                # instance.
                env.register_cab = first_only if i == held else noted if i > held else register
                try:
                    assert run.load(i) == "ok", i
                finally:
                    del env.register_cab
            before = run.container()
            assert later, "the held load registered one key"
            # `register_cab` keys a CAB by its simplified name.
            keys = {simplify_name(cab) for cab in after}
            clash = [cab for cab, _ in later if simplify_name(cab) in keys]
            assert not clash, f"a later file registers {clash}, which this cannot record (docs)"
            for cab, item in later:
                env.register_cab(cab, item)
            out = {
                "name": scene["name"],
                "held": held,
                "before": before,
                "container": run.container(),
                "paths": run.paths(),
                "dep_paths": run.dep_paths(),
                **run.state(),
            }
    return out


def orders_case(scene: dict, new_run, picks, names, find: bool) -> list:
    """The scene's files `picks` (by index, in the order of their numbers)
    loaded under `names` (`None`: their own), the first, with `find`, by
    `env.find_file(name, is_dependency=True)` from `env.path`, where it lies
    under its name: in the order of their numbers, and with the first moved
    last (the order the core's race lists them in), each the container,
    every object's `container` path, the dependencies' paths and the state
    after all of them."""
    out = []
    own = scene["files"]
    files = [[own[j][0] if names is None else names[i], own[j][1]] for i, j in enumerate(picks)]
    n = len(files)
    for order in (tuple(range(n)), (*range(1, n), 0)):
        with scratch.folder() as tmp:
            with contextlib.redirect_stdout(io.StringIO()):
                run = new_run({**scene, "files": files}, str(tmp))
                if find:
                    (Path(tmp) / files[0][0]).write_bytes(bytes.fromhex(files[0][1]))
                for i in order:
                    if find and i == 0:
                        assert run.env.find_file(files[0][0], is_dependency=True) is not None
                    else:
                        assert run.load(i) == "ok", i
                out.append(
                    {
                        "name": scene["name"],
                        "picks": list(picks),
                        "names": [f[0] for f in files],
                        "find": find,
                        "order": list(order),
                        "container": run.container(),
                        "paths": run.paths(),
                        "dep_paths": run.dep_paths(),
                        **run.state(),
                    }
                )
    return out


def rule4(env, name: str, made: dict) -> int | None:
    """The load (a key of `made`, whose value is the file it loaded) whose
    file a pointer to an external named `name` resolves to through
    `env.files`: UnityPy's `PPtr.deref` (`classes/PPtr.py:72-75`, the first
    key in the dict that lowers to the external's name) on a stand-in file
    whose container is `env`, asked for each load's object (path id its
    number + 1) until the file it resolved to holds one; `None` when no key
    lowers to it and `deref` falls back on `env.find_file`, which finds
    nothing here."""
    env.find_file = lambda *_a, **_k: None
    f = types.SimpleNamespace(
        environment=env,
        externals=[types.SimpleNamespace(path=name)],
        parent=env,
    )
    try:
        for load in made:
            p = PPtr.__new__(PPtr)
            p.m_FileID, p.m_PathID, p.assetsfile = 1, load + 1, f
            try:
                found = p.deref(f)
            except KeyError:
                continue
            return next(i for i, g in made.items() if g is found.assets_file)
    except FileNotFoundError:
        return None
    finally:
        del env.find_file
    raise AssertionError(f"{name}: the file it resolved to holds no load's object")


def listings_case(line: dict) -> dict:
    """UnityPy's `env.files` after the listings of every non-empty set of
    the loads of `env::listing_tests` (the line's files, each load's number
    its index, those `listings` names loaded as dependencies), made in the
    order of their numbers: each key with the load whose file it holds and
    that file's `is_dependency`, the path ids of `env.objects`, and the load
    whose file a pointer to an external named `x`, and one named `y`,
    resolves to through `env.files` (`rule4`)."""
    files = line["files"]
    dependencies = set(line["listings"])
    sets = []
    for mask in range(1, 1 << len(files)):
        loads = [i for i in range(len(files)) if mask >> i & 1]
        with contextlib.redirect_stdout(io.StringIO()):
            env = UnityPy.Environment()
            made = {}
            for i in loads:
                name, data = files[i]
                made[i] = env.load_file(bytes.fromhex(data), name=name, is_dependency=i in dependencies)

            def load_of(f, made=made):
                return next(i for i, g in made.items() if g is f)

            sets.append(
                {
                    "loads": loads,
                    "files": [[k, load_of(f), bool(f.is_dependency)] for k, f in env.files.items()],
                    "objects": [o.path_id for o in env.objects],
                    "rule4": {n: rule4(env, n, made) for n in ("x", "y")},
                }
            )
    return {"name": line["name"], "files": files, "dependencies": sorted(dependencies), "sets": sets}


def threads_fixture(inputs: list[dict], new_run) -> dict:
    """`env_threads.json`'s cases (module docs)."""
    by_name = {s["name"]: s for s in inputs}
    held = [held_case(by_name[n], new_run, i) for n, i in HELD.items()]
    orders = [
        case
        for name, runs in ORDERS.items()
        for picks, names, find in runs
        for case in orders_case(by_name[name], new_run, picks, names, find)
    ]
    (listings,) = [s for s in inputs if "listings" in s]
    return {"held": held, "orders": orders, "listings": listings_case(listings)}
