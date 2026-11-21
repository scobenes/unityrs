"""Round 12's intake (round 12 design v4 section 4): completion of a round 12 output folder, the allowlisted views
of it and of round 11's reused runs, the intake digest, and B5a's expected rows and pinned refusals.

    python3 -I tools/parity/anim_fixture_round12.py digest --round12 RUN RUN --round11 RUN RUN

prints the intake digest and its record for both Editors' round 12 and round 11 runs (each run's own run.json
names its Editor, so the order is free). It hashes bytes and parses only run files; it opens no capture.

**Threat model**, anim_body_seal's: a buggy or misrouted collection (a wrong run, swapped Editors, a capture filed
under another case, a sealed file read early, a second evaluation), not a deliberately adversarial user.

**Views** (`View`, the allowlist). Completion and bundle decoding open a run's files only through a view: its run
files (RUN_FILES) always, a case's capture and bundle only where the case's partition is one the view allows.
Round 12's partitions are the generated tables' (anim_round12_tables: exploration, selection, confirmation).
Round 11's reused cases (`round11_cases`) are each scoped rig's witness Avatar (anim_twist_witness.allowlisted,
exploration) and B5a's round 11 rows (the finger and face sweeps and the 25 refusal clips, confirmation); no other
round 11 capture opens at all. Confirmation is sealed: a view allowing it is made only after the intake's claim
exists (anim_twist_seal.require_claim). Every path is plain (anim_twist_witness.run_file: no symlink from the run
root down nor above it), and every byte read is hashed into `View.read`, so an evaluation can require the bytes
its intake bound. `anim_muscle_round11.Recordings` reads through a view, lazily, and the witness's Avatars too.

**Completion** (`complete`): the run is round 12's (run.json's variant; round.json's round, run, tables sha256 and
digest, importer note, and Editor sources sha256, this checkout's fixture revision); its inventory matched every
table case with nothing extra; its authored curves are the tables' (anim_round12_tables.check_authored); and each
viewed case is recorded, its capture naming its own case, run, Editor version and bundle, its bundle the capture's
pinned sha256, its sample times the tables' bit for bit.

**Intake digest** (`intake`): the sha256 of canonical JSON over the fixture revision, the tables' sha256 and
digest, the partition digest, both Editors' round 12 runs (every run file, capture, diagnostics file and bundle by
sha256) and round 11's reused run files, captures and bundles, each run under the Editor its run.json declares.

**B5a** (`b5a_expected`, `pinned_refusals`, `b5a_failures`): its rows are those whose ten limb-twist values are
all 0, from the generated tables: each round 12 sweep vector without a limb twist, every twin sample, and round
11's finger and face sweeps. A twist-active row (a limb-twist vector, a twist clip, a probe, a subset sample)
never enters it. The 25 refusal clips' rows are refusal evidence, each with its pinned reason, the runtime's
(`refusal`: humanoid_refusal's order from a missing required bone to the first limb-twist slot).
"""

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

HERE = Path(__file__).resolve().parent
EDITOR_FOLDER = HERE.parent / "fixtures" / "unity" / "Editor"
FORMAT = "unityrs.round12_intake"
ROUND = 12
RUN_FILES = ("run.json", "round.json", "build.log", "inventory.json", "authored.json")
PARTITIONS = ("exploration", "selection", "confirmation")
IMPORTER_DEFAULT = "not measured (needs an imported model)"
# The families only the twist verdict reads: a run may lack them, refusing all twist. B5a reads sweeps and twins.
TWIST_ONLY = ("probe", "toe-absent", "subset", "twist-clip")
# humanoid::LIMB_TWIST_MUSCLES: the ten limb-twist muscles' HumanTrait ordinals.
LIMB_TWIST_ORDINALS = (23, 25, 27, 31, 33, 35, 41, 43, 50, 52)
_TABLES = {}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def canonical_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def tables():
    """(the tables' document, the sha256 of its canonical bytes, which the builder pins), computed once."""
    if not _TABLES:
        import anim_round12_tables as r12

        doc = r12.document()
        _TABLES.update(doc=doc, sha256=sha256(r12.canonical(doc)))
    return _TABLES["doc"], _TABLES["sha256"]


def partitions():
    """{case: partition} of every round 12 case."""
    return {row["name"]: row["partition"] for row in tables()[0]["cases"]}


def partition_digest():
    return sha256(canonical_json(sorted(partitions().items())))


def editor_sources_sha256(folder=EDITOR_FOLDER):
    """The fixture revision as the builder writes it (UnityrsRound12Output.cs EditorSourcesSha256): the sha256 of
    each *.cs file's name and sha256, a line each, in ordinal name order."""
    rows = "".join(f"{path.name}\n{sha256(path.read_bytes())}\n" for path in
                   sorted(folder.glob("*.cs"), key=lambda path: path.name.encode("utf-8")))
    return sha256(rows.encode("utf-8"))


def bundle_of(row):
    """A round 12 case's bundle as the builder names it: a clip case's `body-clips-<rig>-<twist|twin>`
    (UnityrsBodyClips.cs ClipBundle), any other case's own name without `probes-` (BuildBodyRig)."""
    if row["family"] in ("twist-clip", "twin"):
        return f"body-clips-{row['rig']}-{'twist' if row['family'] == 'twist-clip' else 'twin'}"
    return row["name"].replace("probes-", "")


class View:
    """What a reader may open of one run folder. `cases` maps each case to (partition, bundle); a case's capture
    and bundle open only when its partition is in `allowed`. A view allowing confirmation needs `claim`, called
    now, which refuses unless the intake's claim exists. `read` maps each path read to its bytes' sha256."""

    def __init__(self, folder, cases, allowed=("exploration",), claim=None):
        from anim_twist_witness import run_file

        unknown = set(allowed) - set(PARTITIONS)
        replay.require(not unknown, f"no partition {sorted(unknown)}")
        if "confirmation" in allowed:
            replay.require(claim is not None, "a sealed view needs the intake's claim")
            claim()
        self.folder, self.cases, self.allowed, self._file = Path(folder), dict(cases), tuple(allowed), run_file
        self.read = {}

    def _bytes(self, *parts):
        data = self._file(self.folder, *parts).read_bytes()
        self.read["/".join(parts)] = sha256(data)
        return data

    def run_file(self, name):
        replay.require(name in RUN_FILES, f"{name} is not a run file")
        return self._bytes(name)

    def allows(self, case):
        return case in self.cases and self.cases[case][0] in self.allowed

    def _bundle_name(self, case):
        replay.require(case in self.cases, f"{case} is not a case this view can open")
        partition, bundle = self.cases[case]
        replay.require(partition in self.allowed, f"{case} is sealed ({partition}) in this view")
        return bundle

    def capture(self, case):
        """A viewed case's capture document."""
        self._bundle_name(case)
        return replay.read_json(self._bytes("replays", "body", f"{case}.pending.json").decode("utf-8"))

    def bundle(self, case, doc=None):
        """A viewed case's bundle path, to decode, after hashing its bytes: given its capture `doc`, the bundle
        it names and pins."""
        name = self._bundle_name(case)
        digest = sha256(self._bytes("bundles", name))
        if doc is not None:
            replay.require([row.get("name") for row in doc["spec"]["bundles"]] == [name],
                           f"{case} plays another bundle than {name}")
            replay.require(doc["capture"].get("bundle_sha256") == [digest],
                           f"bundle {name} differs from {case}'s pinned sha256")
        return self._file(self.folder, "bundles", name)


def round12_view(folder, allowed=("exploration",), claim=None):
    return View(folder, {row["name"]: (row["partition"], bundle_of(row)) for row in tables()[0]["cases"]},
                allowed, claim)


def round11_cases():
    """{case: partition} of round 11's reused cases: each scoped rig's witness Avatar (exploration; anim_twist_witness
    read them in stage (a)), and B5a's round 11 rows, the finger and face sweeps and the refusal clips
    (confirmation)."""
    from anim_fixture_muscles import MUSCLE_SWEEPS
    from anim_twist_witness import SCOPED_RIGS, allowlisted

    found = {case: "confirmation" for case in (*pinned_refusals(), *(s for s in MUSCLE_SWEEPS if s.endswith(
        ("-fingers", "-face"))))}
    return found | {allowlisted(rig): "exploration" for rig in SCOPED_RIGS}


def round11_view(folder, allowed=("exploration",), claim=None):
    import anim_fixture_round11 as r11

    return View(folder, {case: (partition, r11.ROUND11_CASES[case].bundle)
                         for case, partition in round11_cases().items()}, allowed, claim)


def readers(unityrs):
    """(avatar_of, clip_of)(view, case, doc): the bundle's Avatar and built clip through `unityrs show`, each bundle
    opened through its view (anim_body_freeze.readers' decoding, behind the allowlist) and read once."""
    from anim_replay_complete import BundleView, read_manifest

    views = {}

    def view_of(view, case, doc):
        path = view.bundle(case, doc)
        key = (str(path), doc["capture"]["bundle_sha256"][0])
        if key not in views:
            views[key] = BundleView(path, key[1], 0, unityrs, None, replay, read_manifest(view.folder, replay)[1])
        return views[key]

    def avatar_of(view, case, doc):
        found = view_of(view, case, doc)
        return found.value(found.asset(doc["spec"]["avatar_asset"], 90))

    def clip_of(view, case, doc):
        found = view_of(view, case, doc)
        return found.value(found.asset(doc["spec"]["clip_asset"], 74))
    return avatar_of, clip_of


def manifest_of(view):
    """(run identity, {case: status}) from run.json, through the view."""
    from anim_body_runs import statuses

    manifest = replay.read_json(view.run_file("run.json").decode("utf-8"))
    replay.require(isinstance(manifest, dict) and manifest.get("format") == "unityrs.fixture_run"
                   and isinstance(manifest.get("run"), dict), "expected unityrs.fixture_run in run.json")
    return manifest["run"], statuses(manifest)


def check_run_files(view):
    """A round 12 run's run files (no capture opened): (run identity, {case: status}, the twist-only cases it lacks).
    Every B5a case (a sweep or a twin) must be recorded with the tables' samples; a twist-only case (TWIST_ONLY) may
    be lacking, which refuses all twist (anim_twist_seal.twist_verdict) and leaves B5a's verdict standing."""
    import anim_round12_tables as r12

    doc, tables_sha256 = tables()
    run, statuses = manifest_of(view)
    replay.require(run.get("variant") == "round12", f"{view.folder.name} is not a round 12 run")
    declared = replay.read_json(view.run_file("round.json").decode("utf-8"))
    replay.require(declared == {"format": "unityrs.fixture_round", "round": ROUND, "run_id": run.get("run_id"),
                                "tables_sha256": tables_sha256, "tables_digest": doc["tables_digest"],
                                "editor_sources_sha256": editor_sources_sha256(), "importer_default": IMPORTER_DEFAULT},
                   f"{view.folder.name}: round.json does not declare this round 12 run of these fixtures and tables")
    inventory = replay.read_json(view.run_file("inventory.json").decode("utf-8"))
    rows = inventory.get("rows") if isinstance(inventory, dict) else None
    replay.require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows)
                   and inventory.get("run_id") == run["run_id"] and inventory.get("tables_sha256") == tables_sha256
                   and inventory.get("extra") == [] and sorted(row.get("name") for row in rows) == sorted(partitions()),
                   f"{view.folder.name}: inventory.json does not list every round 12 case once with nothing extra")
    families = {row["name"]: row["family"] for row in doc["cases"]}
    lacking = sorted({row["name"] for row in rows if row.get("status") != "matched"}
                     | {case for case in families if statuses.get(case) != "recorded"})
    b5a = [case for case in lacking if families[case] not in TWIST_ONLY]
    replay.require(not b5a, f"{view.folder.name}: B5a case(s) not recorded with the tables' samples: {b5a[:5]}")
    authored = replay.read_json(view.run_file("authored.json").decode("utf-8"))
    replay.require(isinstance(authored, dict) and authored.get("run_id") == run["run_id"],
                   f"{view.folder.name}: authored.json is another run's")
    unbuilt = {f"{case}: no authored clip" for case in lacking}
    problems = [line for line in r12.check_authored(authored, doc) if line not in unbuilt]
    replay.require(not problems, f"{view.folder.name}: authored.json: {'; '.join(problems[:3])}")
    return run, statuses, lacking


def complete(view, cases=None):
    """Round 12's completion of one run through `view`: {case: capture} for `cases` (by default every recorded case
    the view allows). A named case the view seals, or one the run lacks, is refused, never skipped."""
    import anim_round12_tables as r12

    run, _, lacking = check_run_files(view)
    rows = {row["name"]: row for row in tables()[0]["cases"]}
    found = {}
    for case in sorted(rows if cases is None else cases):
        if cases is None and (not view.allows(case) or case in lacking):
            continue
        capture = view.capture(case)
        editor = capture["spec"]["evidence"]["editor"]
        replay.require(editor.get("fixture_case") == case, f"{case}'s capture records another case")
        replay.require(editor.get("run_id") == run["run_id"], f"{case} was recorded by another run")
        replay.require(capture["capture"].get("editor_version") == run["editor_version"],
                       f"{case} was recorded by another Editor version than {run['editor_version']}")
        view.bundle(case, capture)
        times = [r12.bits(sample["local"]) for sample in capture["capture"]["samples"]]
        replay.require(times == rows[case]["times"], f"{case}'s samples are not the tables' times")
        found[case] = capture
    return found


def editor_of_run(folder):
    """(the frozen Editor series its run.json declares, its run id)."""
    from anim_muscle_confirm import editor_of
    from anim_twist_witness import run_file

    run = replay.read_json(run_file(Path(folder), "run.json").read_text(encoding="utf-8")).get("run") or {}
    return editor_of(run.get("editor_version")), run.get("run_id")


def by_editor(folders, label):
    """{Editor: (folder, run id)}: exactly one run per frozen Editor series, as each run.json declares."""
    from anim_muscle_confirm import EDITORS

    found = {}
    for folder in folders:
        editor, run_id = editor_of_run(folder)
        replay.require(editor not in found, f"two {label} runs of Editor {editor}")
        found[editor] = (Path(folder), run_id)
    replay.require(sorted(found) == sorted(EDITORS), f"the {label} runs are not one per Editor {sorted(EDITORS)}")
    return found


def _hashed(folder, names):
    """{relative path: sha256} of a run's named files, each through anim_twist_witness.run_file."""
    from anim_twist_witness import run_file

    return {"/".join(parts): sha256(run_file(Path(folder), *parts).read_bytes()) for parts in sorted(set(names))}


def round12_files(folder):
    """Every file of a round 12 run but nothing outside its run files, replays/body and bundles."""
    from anim_twist_witness import run_file

    names = [(name,) for name in RUN_FILES]
    for sub in (("replays", "body"), ("bundles",)):
        top = run_file(Path(folder), "run.json").parent.joinpath(*sub)
        replay.require(top.is_dir() and not top.is_symlink(), f"{Path(folder).name} has no {'/'.join(sub)}")
        names += [(*sub, name) for name in os.listdir(top)]
    return _hashed(folder, names)


def round11_files(folder):
    """A round 11 run's run.json and round.json, and each reused case's capture and bundle."""
    import anim_fixture_round11 as r11

    names = [("run.json",), ("round.json",)]
    for case in round11_cases():
        names += [("replays", "body", f"{case}.pending.json"), ("bundles", r11.ROUND11_CASES[case].bundle)]
    return _hashed(folder, names)


def intake(round12, round11):
    """(intake digest, its record) over both Editors' round 12 and round 11 runs (folder lists in any order)."""
    import anim_muscle_round11 as r11run

    runs, reused = by_editor(round12, "round 12"), by_editor(round11, "round 11")
    views, missing = [], {}
    for editor, (folder, run_id) in runs.items():
        view = round12_view(folder)
        run, _, lacking = check_run_files(view)
        replay.require(run["run_id"] == run_id, f"Editor {editor}'s round 12 run changed while it was read")
        views.append((editor, "round12", view))
        missing[editor] = lacking
    for editor, (folder, _) in reused.items():
        replay.require(r11run.declared_round(folder) == r11run.ROUND, f"Editor {editor}'s {folder.name} is not a "
                                                                      "declared round 11 run")
    doc, tables_sha256 = tables()
    record = dict(format=FORMAT, version=1, fixture_revision=editor_sources_sha256(), tables_sha256=tables_sha256,
                  tables_digest=doc["tables_digest"], partition_digest=partition_digest(),
                  round12={e: dict(run_id=r, twist_only_lacking=missing[e], files=round12_files(f))
                           for e, (f, r) in sorted(runs.items())},
                  round11={e: dict(run_id=r, files=round11_files(f)) for e, (f, r) in sorted(reused.items())})
    # The run files completion checked are the bytes the digest binds.
    consumed(record, views)
    return sha256(canonical_json(record)), record


def consumed(record, views):
    """Refuses unless every byte the views read is the intake's: each view's `read` inside its run's record."""
    for editor, round_, view in views:
        bound = record[round_][editor]["files"]
        changed = sorted(path for path, digest in view.read.items() if bound.get(path) != digest)
        replay.require(not changed, f"Editor {editor}'s {round_} run read bytes the intake never bound: {changed[:3]}")


def twist_free():
    """{case: sample ids} of B5a's rows, from the generated tables: each round 12 sweep's vectors without a limb
    twist (by vector index), every twin's sample times (f32 bits), and round 11's finger and face sweeps (each
    vector index, anim_muscle_confirm.samples)."""
    from anim_fixture_muscles import LIMB_TWISTS, MUSCLE_SWEEPS
    from anim_muscle_confirm import b5a_cases, samples

    doc = tables()[0]
    twists = {m for m, _ in LIMB_TWISTS}
    sets = {row["name"]: row["vectors"] for row in doc["sets"]}
    found = {}
    for row in doc["cases"]:
        if row["family"] == "sweep":
            found[row["name"]] = tuple(k for k, vector in enumerate(sets[row["set"]]) if not any(
                name in twists and replay.f32_from_bits(value) != 0
                for name, value in zip(vector["muscles"], vector["bits"])))
        elif row["family"] == "twin":
            found[row["name"]] = tuple(row["times"])
    for case in (s for s in MUSCLE_SWEEPS if s.endswith(("-fingers", "-face"))):
        found[case] = tuple(samples(case, *b5a_cases()[case]))
    return found


def _rig(case):
    import anim_fixture_round11 as r11

    rows = {row["name"]: row for row in tables()[0]["cases"]}
    return rows[case]["rig"] if case in rows else r11.ROUND11_INFO[case]["rig"]


def b5a_expected():
    """(rows, refusal rows): {(Editor, case, bone, sample id)} B5a must observe, in scope and as refusal evidence."""
    from anim_muscle_confirm import EDITORS, b5a_cases, case_bones, samples

    rows = {(e, case, bone, sid) for case, ids in twist_free().items() for bone in case_bones(_rig(case))
            for sid in ids for e in EDITORS}
    refused = {(e, case, bone, sid) for case in pinned_refusals() for bone in case_bones(_rig(case))
               for sid in samples(case, *b5a_cases()[case]) for e in EDITORS}
    return rows, refused


def refusal(human, moved):
    """The runtime's refusal of muscle ordinals `moved` on Avatar `human`, as its message, or None, in
    humanoid_refusal's order (mod.rs): a missing required bone, a mapped bone without axes, a non-neutral twist
    setting over an animated limb, then the first limb-twist slot."""
    import anim_muscle_pose as pose
    from anim_humanoid_qualify import LIMBS, REQUIRED_BONES, TWIST, muscle_bone

    for bone in REQUIRED_BONES:
        if pose.avatar_node(human, bone) is None:
            return f"missing_human_bone({bone})"
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    animated = set()
    for muscle in sorted(moved):
        node = pose.avatar_node(human, muscle_bone(muscle))
        if node is None:
            continue
        if skeleton["m_Node"][node]["m_AxesId"] < 0:
            return f"unsupported_humanoid_axes({muscle_bone(muscle)})"
        animated.add(muscle_bone(muscle))
    for field in TWIST:
        value = human.get(field)
        replay.require(type(value) in (int, float), f"the Avatar lacks {field}")
        if value != 0 and set(LIMBS[field]) & animated:
            return f"unsupported_humanoid_twist({field})"
    twisted = sorted(set(moved) & set(LIMB_TWIST_ORDINALS))
    return f"unsupported_humanoid_limb_twist({twisted[0]})" if twisted else None


def pinned_refusals():
    """{case: reason}: B5a's 25 refusal cases, round 11's full-body clips outside freeze v8's scope, pinned from
    round 11's builder tables. Their rigs keep all four twist settings at 0 and map every bone, and each curves
    every muscle its bones show but the Spine's three (anim_fixture_full.full_groups), so the runtime refuses
    each at its first limb-twist slot."""
    import anim_fixture_round11 as r11
    import anim_twist_tables as twist_tables
    from anim_fixture_full import full_groups, muscle_property
    from anim_fixture_muscles import LIMB_TWISTS
    from anim_muscle_confirm import b5a_cases, builder_muscles

    found = {}
    for case, (kind, rig, _) in sorted(b5a_cases().items()):
        if kind != "clip" or r11.ROUND11_INFO[case].get("scope") != "refusal":
            continue
        curved = {prop for (prop, *_), in full_groups(list(builder_muscles()), rig)}
        ordinals = sorted(twist_tables.NAMES[m] for m, _ in LIMB_TWISTS if muscle_property(m) in curved)
        replay.require(ordinals, f"{case} curves no limb twist")
        found[case] = f"unsupported_humanoid_limb_twist({ordinals[0]})"
    replay.require(len(found) == 25, f"B5a pins 25 refusal cases, not {len(found)}")
    return found


def b5a_failures(seen, expected=None):
    """Why B5a's verdict fails, empty when it passes. `seen` is an anim_muscle_confirm.Observation whose rows are
    (angle, bound) and refused rows (angle, bound, reason). In each Editor: rows exactly the expected ones, nothing
    twist-active among them, every row within its bound; refusal rows exactly the 25 cases' and each the case's
    pinned reason."""
    from anim_muscle_confirm import EDITORS

    rows, refused = expected or b5a_expected()
    pinned = pinned_refusals()
    found = [f"no B5a runs of Editor {editor}" for editor in EDITORS if editor not in seen.runs]
    found += seen.errors
    free = twist_free()
    active = sorted({key[1] for key in seen.rows.keys() | seen.refused.keys()
                     if key[1] not in free and key[1] not in pinned})
    if active:
        found.append(f"twist-active case(s) entered B5a: {active[:5]}")
    for title, keys in (("lack", rows - seen.rows.keys()), ("hold beyond the expected", seen.rows.keys() - rows),
                        ("lack refusal", refused - seen.refused.keys()),
                        ("hold refusal rows beyond the expected", seen.refused.keys() - refused)):
        if keys:
            found.append(f"the runs {title} {len(keys)} row(s): {sorted(keys, key=str)[:3]}")
    wrong = sorted({(key[1], reason) for key, (*_, reason) in seen.refused.items()
                    if key[1] in pinned and reason != pinned[key[1]]})
    found += [f"{case} refuses as {reason}, not {pinned[case]}" for case, reason in wrong]
    outside = [key for key, (turn, bound) in seen.rows.items() if not turn <= bound]
    if outside:
        found.append(f"{len(outside)} B5a row(s) outside their bound: {sorted(outside, key=str)[:3]}")
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    digest = sub.add_parser("digest")
    digest.add_argument("--round12", nargs=2, type=Path, required=True)
    digest.add_argument("--round11", nargs=2, type=Path, required=True)
    args = parser.parse_args()
    try:
        found, record = intake(args.round12, args.round11)
        print(json.dumps(dict(intake_digest=found, record=record), indent=1, sort_keys=True, ensure_ascii=False))
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_fixture_round12: {exc}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
