"""Round 12's intake (round 12 design v4 section 4): completion of a round 12 output folder, the allowlisted views
of it and of round 11's reused runs, and the intake digest. B5a's rows and refusals are anim_twist_b5a's.

    python3 -I tools/parity/anim_fixture_round12.py digest --round12 RUN RUN --round11 RUN RUN

prints the intake digest and its record for both Editors' round 12 and round 11 runs (each run's own run.json
names its Editor, so the order is free).

**Threat model**, anim_body_seal's: a buggy or misrouted collection (a wrong run, swapped Editors, a capture filed
under another case, a sealed file read early, a second evaluation), not a deliberately adversarial user.

**Views** (`View`, the allowlist). Completion and bundle decoding open a run's files only through a view: its run
files (RUN_FILES) always, a case's capture and bundle only where the case's partition is one the view allows.
Round 12's partitions are the generated tables' (anim_round12_tables: exploration, selection, confirmation).
Round 11's reused cases (`round11_cases`) are each scoped rig's witness Avatar (anim_twist_witness.allowlisted,
exploration) and B5a's round 11 rows (the finger and face sweeps and the 25 refusal clips, confirmation); no other
round 11 capture opens through round 12's views. A view allowing selection needs the selection transition
(anim_twist_seal.open_selection, once, after exploration records an ambiguity) or the claim; one allowing
confirmation needs the intake's claim (anim_twist_seal.require_claim). Every path is plain (anim_twist_witness.
run_file: no symlink from the run root down nor above it). Every byte read is hashed into `View.read`, and a
bundle decodes from a private copy of exactly the bytes hashed, beside a copy of the hashed run.json, so the
evaluation can require that everything it read is what its intake bound. Round 11's own tools (its confirmation
and case export) keep round 11's permissive view (`round11_own_view`).

**Completion** (`complete`): the run is round 12's (run.json's variant; round.json's round, run, tables sha256 and
digest, importer note and Editor sources sha256, this checkout's fixture revision; build.log's run stamp); its
inventory lists every table case; its authored curves are the tables' (anim_round12_tables.check_authored); each
read capture names its own case, run, Editor version and bundle, is the builder's fixture (its prefab, clip and
Avatar assets, resolved through the bundle's container, and its playback: anim_fixture_contract.check_identity),
pins its bundle's sha256 and has the tables' sample times bit for bit. A clip case's built clip compiles exactly
its case's muscles, each slot owned by its own binding through m_IndexArray (anim_muscle_clip.compiled_muscles,
which the Editor may leave unverified), and a subset clip's compiled value at each key is the authored f32.

A run may lack a twist-only case (TWIST_ONLY: a probe, toe-absent, subset or twist clip case), or author one
wrongly: the case is listed as lacking, which refuses all twist and leaves B5a standing. A B5a case (a sweep or a
twin) it lacks refuses the intake.

**Intake digest** (`intake`): the sha256 of canonical JSON over the fixture revision, the tables' sha256 and
digest, the partition digest, both Editors' round 12 runs and round 11's reused files, each run under the Editor
its run.json declares. A round 12 run's hashed set is exactly its inventory's: every run file, a capture and a
bundle for each case it records, any case's diagnostics, nothing else in replays/body or bundles. The intake
hashes sealed files' raw bytes and checks, by byte search alone, that each capture carries its own run's id and
Editor version and no other run's id, and that each bundle's UnityFS header names its run's Editor version. It
never parses or decodes a sealed file: that happens after the claim, through sealed views.
"""

import argparse
import hashlib
import json
import os
import sys
import weakref
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
CLIP_FAMILIES = ("subset", "twist-clip", "twin")
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


def rows():
    return {row["name"]: row for row in tables()[0]["cases"]}


def partitions():
    """{case: partition} of every round 12 case."""
    return {row["name"]: row["partition"] for row in tables()[0]["cases"]}


def partition_digest():
    return sha256(canonical_json(sorted(partitions().items())))


def editor_sources_sha256(folder=EDITOR_FOLDER):
    """The fixture revision as the builder writes it (UnityrsRound12Output.cs EditorSourcesSha256): the sha256 of
    each *.cs file's name and sha256, a line each, in ordinal name order."""
    rows_ = "".join(f"{path.name}\n{sha256(path.read_bytes())}\n" for path in
                    sorted(folder.glob("*.cs"), key=lambda path: path.name.encode("utf-8")))
    return sha256(rows_.encode("utf-8"))


def bundle_of(row):
    """A round 12 case's bundle as the builder names it: a clip case's `body-clips-<rig>-<twist|twin>`
    (UnityrsBodyClips.cs ClipBundle), any other case's own name without `probes-` (BuildBodyRig)."""
    if row["family"] in ("twist-clip", "twin"):
        return f"body-clips-{row['rig']}-{'twist' if row['family'] == 'twist-clip' else 'twin'}"
    return row["name"].replace("probes-", "")


def fixture_of(row):
    """The builder's fixture of a round 12 case (anim_fixture_contract.Fixture): a SetHumanPose case plays its
    bundle's `-muscles` clip by HumanPoseHandler, a subset case that clip through the Animator, and a twist clip
    or twin its rig's clip `body-clip-<rig>-<twist|twin>-<variant>` from the identity or offset prefab."""
    from anim_fixture_body import _fixture

    bundle = bundle_of(row)
    if row["family"] in ("twist-clip", "twin"):
        clip = row["name"].removesuffix("-offset")
        return _fixture(bundle, "offset" if row["name"].endswith("-offset") else "identity", clip, "clip")
    return _fixture(bundle, "identity", bundle + "-muscles", "clip" if row["family"] == "subset" else "probes")


class View:
    """What a reader may open of one run folder. `cases` maps each case to (partition, bundle); a case's capture
    and bundle open only when its partition is in `allowed`. Allowing confirmation needs `claim`, called now,
    which refuses unless the intake's claim exists; allowing selection needs `claim` or `selection`, likewise.
    `read` maps each path read to its bytes' sha256."""

    def __init__(self, folder, cases, allowed=("exploration",), claim=None, selection=None):
        from anim_twist_witness import run_file

        unknown = set(allowed) - set(PARTITIONS)
        replay.require(not unknown, f"no partition {sorted(unknown)}")
        if "confirmation" in allowed:
            replay.require(claim is not None, "a sealed view needs the intake's claim")
            claim()
        if "selection" in allowed:
            replay.require(claim is not None or selection is not None,
                           "a selection view needs the selection transition or the intake's claim")
            (claim or selection)()
        self.folder, self.cases, self.allowed, self._file = Path(folder), dict(cases), tuple(allowed), run_file
        self.read, self._private = {}, None

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

    def private(self):
        """This view's private folder (scratch, removed with the view), holding the hashed run.json."""
        if self._private is None:
            import scratch

            self._private = scratch.make("unityrs-view-")
            weakref.finalize(self, scratch.remove, self._private)
            (self._private / "run.json").write_bytes(self.run_file("run.json"))
        return self._private

    def bundle(self, case, doc=None):
        """A private copy of a viewed case's bundle, written from exactly the bytes hashed, to decode; given its
        capture `doc`, the bundle it names and pins."""
        name = self._bundle_name(case)
        data = self._bytes("bundles", name)
        if doc is not None:
            replay.require([row.get("name") for row in doc["spec"]["bundles"]] == [name],
                           f"{case} plays another bundle than {name}")
            replay.require(doc["capture"].get("bundle_sha256") == [sha256(data)],
                           f"bundle {name} differs from {case}'s pinned sha256")
        path = self.private() / "bundles" / name
        if not path.is_file() or sha256(path.read_bytes()) != sha256(data):
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(data)
        return path


def round12_view(folder, allowed=("exploration",), claim=None, selection=None):
    return View(folder, {name: (row["partition"], bundle_of(row)) for name, row in rows().items()}, allowed, claim,
                selection)


def round11_cases():
    """{case: partition} of round 11's reused cases: each scoped rig's witness Avatar (exploration; anim_twist_witness
    read them in stage (a)), and B5a's round 11 rows, the finger and face sweeps and the refusal clips
    (confirmation)."""
    from anim_fixture_muscles import MUSCLE_SWEEPS
    from anim_twist_b5a import pinned_refusals
    from anim_twist_witness import SCOPED_RIGS, allowlisted

    found = {case: "confirmation" for case in (*pinned_refusals(), *(s for s in MUSCLE_SWEEPS if s.endswith(
        ("-fingers", "-face"))))}
    return found | {allowlisted(rig): "exploration" for rig in SCOPED_RIGS}


def round11_view(folder, allowed=("exploration",), claim=None):
    import anim_fixture_round11 as r11

    return View(folder, {case: (partition, r11.ROUND11_CASES[case].bundle)
                         for case, partition in round11_cases().items()}, allowed, claim)


def round11_own_view(folder):
    """Round 11's own permissive view, for its standalone confirmation and case export (anim_muscle_confirm,
    anim_muscle_pose): every round 11 case, opened as before round 12. Round 12's intake and seal never use it."""
    import anim_fixture_round11 as r11

    return View(folder, {case: ("exploration", fixture.bundle) for case, fixture in r11.ROUND11_CASES.items()})


def readers(unityrs):
    """(avatar_of, clip_of)(view, case, doc): the bundle's Avatar and built clip through `unityrs show`, each
    decoded from the view's private copy of the hashed bytes, with the hashed run.json (anim_body_freeze.readers'
    decoding, behind the allowlist), each read once."""
    from anim_replay_complete import BundleView, read_manifest

    views = {}

    def view_of(view, case, doc):
        path = view.bundle(case, doc)
        key = (str(path), doc["capture"]["bundle_sha256"][0])
        if key not in views:
            views[key] = BundleView(path, key[1], 0, unityrs, None, replay, read_manifest(view.private(), replay)[1])
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


def _authored_lacking(authored, doc, lacking):
    """The cases whose authored curves fail check_authored, refusing unless each is twist-only: each problem's
    clip (its label, or the case it names) mapped to its cases."""
    import anim_round12_tables as r12

    clips = {clip.get("clip"): clip.get("cases") or [] for clip in authored.get("clips") or [] if isinstance(clip, dict)}
    families = {name: row["family"] for name, row in rows().items()}
    found = set()
    for line in r12.check_authored(authored, doc):
        label = line.split(": ", 1)[0]
        cases = clips.get(label) or ([label] if label in families else [])
        replay.require(cases and all(families.get(case) in TWIST_ONLY for case in cases),
                       f"authored.json: {line}")
        found |= set(cases) - set(lacking)
    return found


def check_run_files(view):
    """A round 12 run's run files (no capture opened): (run identity, {case: status}, the twist-only cases it lacks
    or authored wrongly). Every B5a case must be recorded with the tables' samples and authored as the tables."""
    from anim_replay_complete import run_stamp

    doc, tables_sha256 = tables()
    run, statuses = manifest_of(view)
    replay.require(run.get("variant") == "round12", f"{view.folder.name} is not a round 12 run")
    declared = replay.read_json(view.run_file("round.json").decode("utf-8"))
    replay.require(declared == {"format": "unityrs.fixture_round", "round": ROUND, "run_id": run.get("run_id"),
                                "tables_sha256": tables_sha256, "tables_digest": doc["tables_digest"],
                                "editor_sources_sha256": editor_sources_sha256(), "importer_default": IMPORTER_DEFAULT},
                   f"{view.folder.name}: round.json does not declare this round 12 run of these fixtures and tables")
    log = view.run_file("build.log").decode("utf-8-sig").splitlines()
    replay.require(all(isinstance(run.get(key), str) for key in ("run_id", "editor_version", "build_target"))
                   and log[:1] == [run_stamp(run)], f"{view.folder.name}: build.log is not this run's")
    inventory = replay.read_json(view.run_file("inventory.json").decode("utf-8"))
    found = inventory.get("rows") if isinstance(inventory, dict) else None
    replay.require(isinstance(found, list) and all(isinstance(row, dict) for row in found)
                   and inventory.get("run_id") == run["run_id"] and inventory.get("tables_sha256") == tables_sha256
                   and inventory.get("extra") == [] and sorted(row.get("name") for row in found) == sorted(partitions()),
                   f"{view.folder.name}: inventory.json does not list every round 12 case once with nothing extra")
    families = {name: row["family"] for name, row in rows().items()}
    lacking = {row["name"] for row in found if row.get("status") != "matched"} | {
        case for case in families if statuses.get(case) != "recorded"}
    b5a = sorted(case for case in lacking if families[case] not in TWIST_ONLY)
    replay.require(not b5a, f"{view.folder.name}: B5a case(s) not recorded with the tables' samples: {b5a[:5]}")
    authored = replay.read_json(view.run_file("authored.json").decode("utf-8"))
    replay.require(isinstance(authored, dict) and authored.get("run_id") == run["run_id"],
                   f"{view.folder.name}: authored.json is another run's")
    lacking |= _authored_lacking(authored, doc, lacking)
    return run, statuses, sorted(lacking)


def muscle_curves(case, row):
    """{Animator muscle property: (key times, values)} a clip case's built clip compiles: the tables' muscle curves
    (anim_round12_tables.expected_curves, each Linear) as floats, its body and motion curves left out."""
    import anim_round12_tables as r12

    variant = None if row["family"] == "subset" else {name: v for name, _, _, v, _ in r12.full_clips()}[
        case.removesuffix("-offset")]
    return {prop: (tuple(map(r12._value, times)), tuple(map(r12._value, values)))
            for prop, (times, values, _, mode) in r12.expected_curves(row, tables()[0], variant).items()
            if mode == "Linear"}


def check_clip(case, row, capture, clip):
    """A clip case's built clip: exactly its case's muscles compiled, each slot its own binding's through
    m_IndexArray, each within f32 rounding of the authored curve, and a subset clip's value at each key the
    authored f32 exactly."""
    import anim_muscle_clip as clips
    import anim_round12_tables as r12

    names = [muscle["name"] for muscle in capture["capture"]["humanoid"]["muscles"]]
    replay.require(names == r12.humantrait_order(), f"{case}: the capture's muscle names are not HumanTrait's")
    curves = muscle_curves(case, row)
    tracks = clips.compiled_muscles(case, clip, curves, names)
    if row["family"] == "subset":
        from anim_fixture_full import muscle_property

        ordinal = {muscle_property(name): m for m, name in enumerate(names)}
        for prop, (times, values) in curves.items():
            for time, value in zip(times, values):
                got = clips.track_value(tracks[ordinal[prop]], clips.f32(time))[0]
                replay.require(clips.f32(got) == clips.f32(value),
                               f"{case}: {prop} compiles {got!r} at its key {time!r}, not the authored {value!r}")


def complete(view, readers_, cases=None):
    """Round 12's completion of one run through `view`, decoding through `readers_` (`readers`): {case: capture}
    for `cases` (by default every recorded case the view allows). A named case the view seals, or one the run
    lacks, is refused, never skipped."""
    import anim_round12_tables as r12
    from anim_fixture_contract import check_identity

    run, _, lacking = check_run_files(view)
    avatar_of, clip_of = readers_
    found = {}
    for case in sorted(rows() if cases is None else cases):
        if cases is None and (not view.allows(case) or case in lacking):
            continue
        replay.require(case not in lacking, f"{case} is lacking from {view.folder.name}")
        row = rows()[case]
        capture = view.capture(case)
        editor = capture["spec"]["evidence"]["editor"]
        replay.require(editor.get("fixture_case") == case, f"{case}'s capture records another case")
        replay.require(editor.get("run_id") == run["run_id"], f"{case} was recorded by another run")
        replay.require(capture["capture"].get("editor_version") == run["editor_version"],
                       f"{case} was recorded by another Editor version than {run['editor_version']}")
        check_identity(case, fixture_of(row), capture["spec"], capture["capture"], replay)
        times = [r12.bits(sample["local"]) for sample in capture["capture"]["samples"]]
        replay.require(times == row["times"], f"{case}'s samples are not the tables' times")
        # Both assets resolve through the bundle's own container (BundleView.asset), so a capture naming a
        # sibling's clip in the same bundle fails check_identity, and one naming a clip the bundle lacks fails here.
        avatar_of(view, case, capture)
        if row["family"] in CLIP_FAMILIES:
            check_clip(case, row, capture, clip_of(view, case, capture))
        found[case] = capture
    return found


def editor_of_run(folder):
    """(the frozen Editor series its run.json declares, its run id, its Editor version)."""
    from anim_muscle_confirm import editor_of
    from anim_twist_witness import run_file

    run = replay.read_json(run_file(Path(folder), "run.json").read_text(encoding="utf-8")).get("run") or {}
    return editor_of(run.get("editor_version")), run.get("run_id"), run.get("editor_version")


def by_editor(folders, label):
    """{Editor: (folder, run id, Editor version)}: exactly one run per frozen Editor series, as run.json declares."""
    from anim_muscle_confirm import EDITORS

    found = {}
    for folder in folders:
        editor, run_id, version = editor_of_run(folder)
        replay.require(editor not in found, f"two {label} runs of Editor {editor}")
        replay.require(isinstance(run_id, str) and len(run_id) == 32, f"{Path(folder).name} has no run id")
        found[editor] = (Path(folder), run_id, version)
    replay.require(sorted(found) == sorted(EDITORS), f"the {label} runs are not one per Editor {sorted(EDITORS)}")
    return found


def _listing(folder, *sub):
    from anim_twist_witness import run_file

    top = run_file(Path(folder), "run.json").parent.joinpath(*sub)
    replay.require(top.is_dir() and not top.is_symlink(), f"{Path(folder).name} has no {'/'.join(sub)}")
    return set(os.listdir(top))


def _hashed(folder, names, stamps):
    """{relative path: sha256} of a run's named files, each through anim_twist_witness.run_file, each capture and
    bundle checked against `stamps` (`_stamped`) on its raw bytes."""
    from anim_twist_witness import run_file

    found = {}
    for parts in sorted(set(names)):
        data = run_file(Path(folder), *parts).read_bytes()
        _stamped(parts, data, *stamps)
        found["/".join(parts)] = sha256(data)
    return found


def _stamped(parts, data, run_id, version, others):
    """By byte search alone, never parsing: a capture carries its run's id and Editor version and no other run's
    id; a bundle's UnityFS header names its run's Editor version."""
    name = "/".join(parts)
    if name.endswith(".pending.json"):
        replay.require(run_id.encode() in data and version.encode() in data
                       and not any(other.encode() in data for other in others),
                       f"{name} is not a capture of run {run_id} ({version})")
    elif parts[0] == "bundles":
        replay.require(data[:8] == b"UnityFS\0" and b"\0" + version.encode() + b"\0" in data[:128],
                       f"{name} is not a bundle of Unity {version}")


def round12_files(folder, lacking, stamps):
    """Exactly the inventory's set: every run file, the capture and bundle of each case it records (a lacking
    twist-only case's if present), any case's diagnostics, and nothing else in replays/body or bundles."""
    names = [(name,) for name in RUN_FILES]
    captures, bundles = _listing(folder, "replays", "body"), _listing(folder, "bundles")
    wanted = {name: (f"{name}.pending.json", bundle_of(row)) for name, row in rows().items()}
    allowed = {file for capture, _ in wanted.values() for file in (capture, capture.replace(".pending.", ".diagnostics."))}
    replay.require(captures <= allowed, f"replays/body holds files no case records: {sorted(captures - allowed)[:3]}")
    replay.require(bundles <= {bundle for _, bundle in wanted.values()},
                   f"bundles holds files no case builds: {sorted(bundles - {b for _, b in wanted.values()})[:3]}")
    missing = sorted(file for case, files in wanted.items() if case not in lacking
                     for file in files if file not in captures | bundles)
    replay.require(not missing, f"{Path(folder).name}: recorded cases lack {missing[:3]}")
    names += [("replays", "body", file) for file in captures] + [("bundles", file) for file in bundles]
    return _hashed(folder, names, stamps)


def round11_files(folder, stamps):
    """A round 11 run's run.json and round.json, and each reused case's capture and bundle."""
    import anim_fixture_round11 as r11

    names = [("run.json",), ("round.json",)]
    for case in round11_cases():
        names += [("replays", "body", f"{case}.pending.json"), ("bundles", r11.ROUND11_CASES[case].bundle)]
    return _hashed(folder, names, stamps)


def intake(round12, round11):
    """(intake digest, its record) over both Editors' round 12 and round 11 runs (folder lists in any order)."""
    import anim_muscle_round11 as r11run

    runs, reused = by_editor(round12, "round 12"), by_editor(round11, "round 11")
    ids = {r for _, r, _ in runs.values()} | {r for _, r, _ in reused.values()}
    views, lacking = [], {}
    for editor, (folder, run_id, _) in runs.items():
        view = round12_view(folder)
        run, _, lacking[editor] = check_run_files(view)
        replay.require(run["run_id"] == run_id, f"Editor {editor}'s round 12 run changed while it was read")
        views.append((editor, "round12", view))
    for editor, (folder, _, _) in reused.items():
        replay.require(r11run.declared_round(folder) == r11run.ROUND, f"Editor {editor}'s {folder.name} is not a "
                                                                      "declared round 11 run")
    doc, tables_sha256 = tables()
    record = dict(format=FORMAT, version=1, fixture_revision=editor_sources_sha256(), tables_sha256=tables_sha256,
                  tables_digest=doc["tables_digest"], partition_digest=partition_digest(),
                  round12={e: dict(run_id=r, twist_only_lacking=lacking[e],
                                   files=round12_files(f, lacking[e], (r, v, ids - {r})))
                           for e, (f, r, v) in sorted(runs.items())},
                  round11={e: dict(run_id=r, files=round11_files(f, (r, v, ids - {r})))
                           for e, (f, r, v) in sorted(reused.items())})
    # The run files completion checked are the bytes the digest binds.
    consumed(record, views)
    return sha256(canonical_json(record)), record


def consumed(record, views):
    """Refuses unless every byte the views read is the intake's: each view's `read` inside its run's record."""
    for editor, round_, view in views:
        bound = record[round_][editor]["files"]
        changed = sorted(path for path, digest in view.read.items() if bound.get(path) != digest)
        replay.require(not changed, f"Editor {editor}'s {round_} run read bytes the intake never bound: {changed[:3]}")


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
