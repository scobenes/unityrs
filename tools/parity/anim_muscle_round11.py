"""B5a's round 11 inventory: the cases its confirmation requires of each Editor's round 11 run.

docs/plans/humanoid-body.md, B5a (e), froze them before any round 11 data: all four muscle sweeps
and every full-body clip, in both Editors. Freeze v8's `anim_body_runs.Run` lists neither and is
not edited; `Recordings` reads them beside it, from the same run folder.

The names are round 11's builder's (UnityrsBodyMuscles.cs and UnityrsBodyClips.cs), as round 11's
case table (`anim_fixture_round11.ROUND11_CASES`) lists them: 4 sweeps, 25 refusal-evidence and 318
in-scope full-body clips. A run declares round 11 in `round.json`. Each run's required cases are
this inventory together with any full-body clip its `run.json` lists, so neither the inventory
nor the run can drop one unseen. Each must be recorded, its capture's own metadata naming its
case, run, Editor version and bundle, and compared in full (`expected_samples`);
`anim_muscle_pose.py confirm` fails otherwise. No round 11 rig is a hold-out rig. Only an
in-scope sample confirms a required case: one the runtime refuses as `unsupported_humanoid_twist`
(a non-neutral twist setting and a case animating a bone it redistributes over) is refusal
evidence (`anim_muscle_pose.refusal_reason`, the plan's B5a (g)), and every round 11 rig's twist
settings are neutral.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_fixture_body import CHAIN_VARIANT, CLIP_VARIANTS  # noqa: E402

ROUND = 11
# sweep: bundle
SWEEPS = {name: name.replace("probes-", "") for name in (
    "body-probes-muscles-compact", "body-probes-muscles-tall", "body-probes-fingers", "body-probes-face")}
FULL_VARIANTS = ("keep-o0-y0-xz0", "bake-o1-y1-xz1", "level", "keep-o1-y1-xz1-motion")
# The variants B4 admits, freeze v8's E4 passing set.
ADMITTED = ("bake-o0-y0-xz1", "bake-o0-y1-xz0", "bake-o0-y1-xz1", "bake-o1-y0-xz0", "bake-o1-y0-xz1",
            "bake-o1-y1-xz0", "bake-o1-y1-xz1", "keep-o0-y0-xz0", "keep-o1-y1-xz1-motion", "level",
            "orientation-offset")
TOPOLOGIES = ("no-upper-chest", "no-shoulders", "no-left-toes", "no-right-toes", "no-toes", "no-upper-chest-toes",
              "no-upper-chest-left-toes", "no-upper-chest-right-toes", "noncoplanar")
CHAIN_FEATURES = ("armature", "scaled-animator", "hips-scale")
# The face configurations B3 admits besides both eyes and the jaw, on the compact and tall rigs.
FACES = ("no-face", "eyes", "jaw")
FACE_VARIANTS = ("keep-o0-y0-xz0", "bake-o1-y1-xz1")
# rig: (variants, both placements). A plain chain plays on both roots; an F5 feature's on the identity root.
FULL_CLIP_RIGS = {
    **{rig + "-full": (FULL_VARIANTS, True) for rig in ("compact", "large", "small")},
    "armature-large-full": ((CHAIN_VARIANT,), False),
    **{rig + "-scoped": (ADMITTED, True) for rig in ("compact", "large", "small", "tall", *TOPOLOGIES)},
    **{f"{rig}-{face}-scoped": (FACE_VARIANTS, True) for rig in ("compact", "tall") for face in FACES},
    **{f"{feature}{size}-scoped": ((CHAIN_VARIANT,), False)
       for feature in CHAIN_FEATURES for size in ("", "-large", "-small") if (feature, size) != ("armature", "-small")}}


def _full_clips():
    clips = {}
    for rig, (chosen, both) in FULL_CLIP_RIGS.items():
        for variant in (name for name in CLIP_VARIANTS if name in chosen):
            for suffix in ("", "-offset") if both else ("",):
                clips[f"body-clip-{rig}-{variant}{suffix}"] = "body-clips-" + rig
    return clips


FULL_CLIPS = _full_clips()
# case: bundle
CASES = SWEEPS | FULL_CLIPS
# Each sweep's sample count: its known vectors (each muscle alone at ten values, then each bone's muscles jointly
# twice) and the three spine anchors (round 11's anim_fixture_round11.probe_schedule).
SWEEP_SAMPLES = {"body-probes-muscles-compact": 535, "body-probes-muscles-tall": 503, "body-probes-fingers": 423,
                 "body-probes-face": 69}
# A full-body clip's: its five key times and an off-key sample in each of its four intervals, as every F4 clip.
CLIP_SAMPLES = 9


def expected_samples(case):
    """The samples a complete capture of a round 11 required case records."""
    return SWEEP_SAMPLES.get(case, CLIP_SAMPLES)


def full_clip_bundle(name):
    """A full-body clip's bundle, `body-clips-<rig>`, from its name; None for any other case."""
    if not name.startswith("body-clip-"):
        return None
    for marker in ("-full-", "-scoped-"):
        rig, found, _ = name.removeprefix("body-clip-").partition(marker)
        if found:
            return "body-clips-" + rig + marker[:-1]
    return None


def declared_round(folder):
    """ROUND for a run whose `round.json` declares round 11, None for a run without one (rounds 1 to 10)."""
    path = folder / "round.json"
    if not path.is_file():
        return None
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    declared = replay.read_json(path.read_text(encoding="utf-8"))
    replay.require(declared == {"format": "unityrs.fixture_round", "round": ROUND,
                                "run_id": manifest["run"].get("run_id")},
                   f"{folder.name}: round.json does not declare this round 11 run")
    return ROUND


def required(statuses):
    """{case: bundle}: the inventory and every full-body clip `run.json` lists."""
    listed = {name: full_clip_bundle(name) for name in statuses if full_clip_bundle(name)}
    return dict(sorted((listed | CASES).items()))


class Recordings:
    """One round 11 training run's B5a cases: Unity's recordings (`.pending.json`, as `Run(pending=True)` reads
    them) by case, and the required cases it lacks."""

    def __init__(self, folder):
        import anim_body_runs as runs

        manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
        self.folder, self.version, self.run_id = folder, manifest["run"]["editor_version"], manifest["run"]["run_id"]
        statuses = runs.statuses(manifest)
        self.required = required(statuses)
        self.documents, self.missing = {}, []
        for case, bundle in self.required.items():
            path = folder / "replays" / "body" / f"{case}.pending.json"
            if statuses.get(case) != "recorded" or not path.is_file():
                self.missing.append(case)
                continue
            doc = replay.read_json(path.read_text(encoding="utf-8"))
            # The capture's own identity, never its file name: its case, run, Editor and bundle.
            editor = doc["spec"]["evidence"]["editor"]
            replay.require(editor.get("fixture_case") == case, f"{path.name} records another case than {case}")
            replay.require(editor.get("run_id") == self.run_id, f"{case} was recorded by another run")
            replay.require(doc["capture"].get("editor_version") == self.version,
                           f"{case} was recorded by another Editor version than {self.version}")
            replay.require([row["name"] for row in doc["spec"]["bundles"]] == [bundle],
                           f"{case} plays another bundle than {bundle}")
            self.documents[case] = doc
