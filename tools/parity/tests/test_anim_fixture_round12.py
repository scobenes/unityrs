"""Round 12's intake (anim_fixture_round12): completion, the allowlisted views and the intake digest, on synthetic
run folders, wrong, swapped, tampered and sealed ones among them; and round 11's completion held back from round
12's sealed evidence."""

import hashlib
import json
import re
import shutil
import unittest
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_fixture_round11 as r11  # noqa: E402
import anim_fixture_round12 as round12  # noqa: E402
import anim_muscle_clip as clips  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_replay_complete as complete  # noqa: E402
import anim_round12_tables as r12tables  # noqa: E402
import anim_twist_seal as seal  # noqa: E402
import scratch  # noqa: E402
from anim_fixture_contract import ASSETS  # noqa: E402
from anim_twist_witness import allowlisted  # noqa: E402
from test_anim_round12_tables import authored  # noqa: E402

VERSIONS = {"2022": "2022.3.62f3", "6000": "6000.6.9f1"}
RUN_IDS = {"2022": "a" * 32, "6000": "b" * 32}
# A case of each partition and family the tests read: an exploration probe, a confirmation twin and twist clip.
EXPLORATION, TWIN = "body-twist-compact-t0", "body-clip-compact-scoped-twin-level"
TWIST_CLIP, SUBSET = "body-clip-compact-scoped-twist-level", "body-twist-clip-compact-t0"
SELECTION = "body-twist-tall-t0"


def rows():
    return round12.rows()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def bundle_bytes(version, name):
    """A bundle whose UnityFS header names its Editor version."""
    return b"UnityFS\0\0\0\0\x08" + b"5.x.x\0" + version.encode() + b"\0" + name.encode()


def capture(case, run_id, version, data, bundle, times, fixture=None):
    """A capture document: its case, run, Editor and bundle, and with `fixture` the builder's identity."""
    spec = dict(bundles=[dict(name=bundle)], evidence=dict(editor=dict(
        fixture_case=case, run_id=run_id, expected_discovery="", animation_source="procedural_humanoid")))
    body = dict(editor_version=version, bundle_sha256=[hashlib.sha256(data).hexdigest()],
                samples=[dict(local=replay.f32_from_bits(t)) for t in times])
    if fixture is not None:
        prefix = ASSETS + fixture.folder + "/"
        spec.update(prefab_asset=prefix + fixture.prefab + ".prefab", clip_asset=prefix + fixture.clip + ".anim",
                    avatar_asset=fixture.avatar, mode=fixture.playback, binding_address="0", binding_component=0)
        doc = round12.tables()[0]
        body.update(playback=dict(api=replay.API[fixture.playback], clip_human_motion=True),
                    humanoid=dict(muscles=[dict(name=name) for name in r12tables.humantrait_order()]))
    return dict(spec=spec, capture=body)


def make_round12(folder, editor, authored_doc, recorded=None):
    """A round 12 run of `editor` whose run files are the builder's, holding captures and bundles of `recorded`
    (every case by default); every other twist-only case is marked lacking in its inventory."""
    run_id, version = RUN_IDS[editor], VERSIONS[editor]
    doc, tables_sha256 = round12.tables()
    names = sorted(round12.partitions())
    families = {name: row["family"] for name, row in rows().items()}
    held = set(names if recorded is None else recorded) | {n for n in names if families[n] not in round12.TWIST_ONLY}
    run = dict(run_id=run_id, editor_version=version, build_target="StandaloneOSX", build_target_id=2,
               variant="round12")
    write_json(folder / "run.json", dict(format="unityrs.fixture_run", version=1, run=run, cases=[
        dict(name=name, status="recorded" if name in held else "skipped", reason="" if name in held else "test")
        for name in names]))
    write_json(folder / "round.json", dict(format="unityrs.fixture_round", round=12, run_id=run_id,
                                           tables_sha256=tables_sha256, tables_digest=doc["tables_digest"],
                                           editor_sources_sha256=round12.editor_sources_sha256(),
                                           importer_default=round12.IMPORTER_DEFAULT))
    write_json(folder / "inventory.json", dict(format="unityrs.round12_inventory", run_id=run_id,
                                               tables_sha256=tables_sha256, cases=len(names), matched=len(held),
                                               rows=[dict(name=n, status="matched" if n in held else "missing",
                                                          reason="", samples=1) for n in names], extra=[]))
    (folder / "authored.json").write_text(re.sub('"run_id": ?"r"', f'"run_id": "{run_id}"', authored_doc, count=1),
                                          encoding="utf-8")
    (folder / "build.log").write_text(complete.run_stamp(run) + "\nbuilt\n", encoding="utf-8")
    (folder / "bundles").mkdir()
    (folder / "replays" / "body").mkdir(parents=True)
    for case in sorted(held):
        row = rows()[case]
        bundle = round12.bundle_of(row)
        data = bundle_bytes(version, bundle)
        (folder / "bundles" / bundle).write_bytes(data)
        write_json(folder / "replays" / "body" / f"{case}.pending.json",
                   capture(case, run_id, version, data, bundle, row["times"], round12.fixture_of(row)))
    return folder


def make_round11(folder, editor):
    """A declared round 11 run of `editor` holding every reused case's capture and bundle."""
    run_id, version = ("c" if editor == "2022" else "d") * 32, VERSIONS[editor]
    write_json(folder / "run.json", dict(run=dict(run_id=run_id, editor_version=version), cases=[]))
    write_json(folder / "round.json", dict(format="unityrs.fixture_round", round=11, run_id=run_id))
    (folder / "bundles").mkdir()
    for case in round12.round11_cases():
        bundle = r11.ROUND11_CASES[case].bundle
        data = bundle_bytes(version, bundle)
        (folder / "bundles" / bundle).write_bytes(data)
        write_json(folder / "replays" / "body" / f"{case}.pending.json", capture(case, run_id, version, data, bundle, []))
    return folder


STUB = json.dumps(dict(run_id="r", clips="stub"))


def stub_check(authored_doc, doc):
    """anim_round12_tables.check_authored for a run whose authored.json is STUB (AuthoredIntakeTests reads the real
    one)."""
    return [] if authored_doc.get("clips") == "stub" else ["not the stub"]


def readers(clip=None):
    """Fake decoding readers: the bundle through the view (hashed, its private copy), and `clip` as the built clip."""
    def avatar_of(view, case, doc):
        path = view.bundle(case, doc)
        assert path.read_bytes()[:7] == b"UnityFS"
        return {}

    def clip_of(view, case, doc):
        view.bundle(case, doc)
        return clip
    return avatar_of, clip_of


def claimed(record, ledger=None):
    """Claims the intake whose record is `record`: its digest."""
    digest = round12.sha256(round12.canonical_json(record))
    seal.claim(digest, record, ledger)
    return digest


class Runs:
    """Both Editors' round 12 and round 11 runs, built once per class and copied for each test. Unless REAL, their
    authored.json is STUB, checked by `stub_check`, and clip checks are stubbed."""
    REAL = False

    @classmethod
    def setUpClass(cls):
        doc = round12.tables()[0]
        text = json.dumps(authored(doc, r12tables.humantrait_order()), indent=1) if cls.REAL else STUB
        cls.template = scratch.for_class(cls)
        for editor in VERSIONS:
            make_round12(cls.template / f"r12-{editor}", editor, text)
            make_round11(cls.template / f"r11-{editor}", editor)

    def setUp(self):
        if not self.REAL:
            for target, name, value in ((r12tables, "check_authored", stub_check),
                                        (round12, "check_clip", lambda *_: None)):
                patcher = mock.patch.object(target, name, value)
                patcher.start()
                self.addCleanup(patcher.stop)

    def runs(self):
        """({Editor: round 12 run}, {Editor: round 11 run}), fresh copies."""
        top = scratch.for_case(self)
        copies = [{e: Path(shutil.copytree(self.template / f"{kind}-{e}", top / f"{kind}-{e}")) for e in VERSIONS}
                  for kind in ("r12", "r11")]
        return copies[0], copies[1]

    def intake(self, r12, r11_):
        return round12.intake(list(r12.values()), list(r11_.values()))


class IntakeTests(Runs, unittest.TestCase):
    def test_the_digest_binds_exactly_the_inventorys_set_in_any_order(self):
        r12, r11_ = self.runs()
        digest, record = self.intake(r12, r11_)
        self.assertEqual(round12.intake(list(r12.values())[::-1], list(r11_.values())[::-1])[0], digest)
        files = record["round12"]["2022"]["files"]
        self.assertEqual(sorted(p for p in files if p.startswith("replays/")),
                         sorted(f"replays/body/{c}.pending.json" for c in rows()))
        self.assertEqual(sorted(p for p in files if p.startswith("bundles/")),
                         sorted({f"bundles/{round12.bundle_of(row)}" for row in rows().values()}))
        self.assertEqual(record["round12"]["2022"]["twist_only_lacking"], [])
        self.assertEqual(len([p for p in record["round11"]["6000"]["files"] if p.startswith("replays/")]),
                         len(round12.round11_cases()))
        # Any byte of a bound file changes it; a round 11 capture outside the reuse is not bound.
        write_json(r11_["6000"] / "replays" / "body" / "body-clip-tall-scoped-level-offset.pending.json", {})
        self.assertEqual(self.intake(r12, r11_)[0], digest)
        for path in (r12["2022"] / "authored.json", r12["6000"] / "bundles" / round12.bundle_of(rows()[TWIN]),
                     r11_["2022"] / "replays" / "body" / "body-probes-face.pending.json"):
            with self.subTest(path=path.name):
                path.write_bytes(path.read_bytes() + b" ")
                self.assertNotEqual(self.intake(r12, r11_)[0], digest)

    def test_the_intake_never_parses_a_sealed_file(self):
        # It hashes sealed bytes and searches them for run stamps only: a sealed capture that is no JSON at all, or
        # a bundle that is only a header, passes the intake; reading them is completion's, after the claim.
        r12, r11_ = self.runs()
        for folder, case, run_id, version in ((r12["2022"], TWIN, RUN_IDS["2022"], VERSIONS["2022"]),
                                              (r11_["6000"], "body-probes-face", "d" * 32, VERSIONS["6000"])):
            (folder / "replays" / "body" / f"{case}.pending.json").write_bytes(
                b"\xff not json " + run_id.encode() + b" " + version.encode())
        (r12["2022"] / "bundles" / round12.bundle_of(rows()[TWIN])).write_bytes(bundle_bytes(VERSIONS["2022"], ""))
        with mock.patch.object(replay, "read_json", wraps=replay.read_json) as parsed:
            self.intake(r12, r11_)
        self.assertFalse([c for c in parsed.call_args_list if "not json" in str(c)])

    def test_a_wrong_run_is_refused(self):
        def changed(edit, pattern, kind="r12"):
            r12, r11_ = self.runs()
            edit((r12 if kind == "r12" else r11_)["2022"])
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                self.intake(r12, r11_)

        def edit_json(name, change):
            def edit(folder):
                value = json.loads((folder / name).read_text(encoding="utf-8"))
                change(value)
                write_json(folder / name, value)
            return edit
        changed(edit_json("run.json", lambda v: v["run"].pop("variant")), "not a round 12 run")
        changed(edit_json("round.json", lambda v: v.update(run_id="e" * 32)), "does not declare this round 12 run")
        changed(edit_json("round.json", lambda v: v.update(editor_sources_sha256="0" * 64)), "does not declare")
        changed(edit_json("round.json", lambda v: v.update(tables_digest="0" * 64)), "does not declare")
        changed(edit_json("inventory.json", lambda v: v.update(extra=["stray.json"])), "nothing extra")
        changed(edit_json("inventory.json", lambda v: v["rows"].pop()), "every round 12 case once")
        changed(edit_json("authored.json", lambda v: v.update(run_id="e" * 32)), "another run's")
        changed(edit_json("round.json", lambda v: v.update(round=12)), "does not declare this round 11 run", "r11")
        r12, r11_ = self.runs()
        with self.assertRaisesRegex(ValueError, "not a round 12 run|expected unityrs.fixture_run"):
            round12.intake([r11_["2022"], r12["6000"]], [r12["2022"], r11_["6000"]])

    def test_swapped_editors_and_misfiled_files_fail_the_intake(self):
        r12, r11_ = self.runs()
        with self.assertRaisesRegex(ValueError, "two round 12 runs of Editor 2022"):
            round12.intake([r12["2022"], r12["2022"]], list(r11_.values()))
        with self.assertRaisesRegex(ValueError, "two round 11 runs of Editor 6000"):
            round12.intake(list(r12.values()), [r11_["6000"], r11_["6000"]])

        def misfiled(change, pattern):
            r12, r11_ = self.runs()
            change(r12, r11_)
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                self.intake(r12, r11_)

        def copy(source, target, *parts):
            shutil.copyfile(source.joinpath(*parts), target.joinpath(*parts))
        body = ("replays", "body", f"{TWIN}.pending.json")
        bundle = ("bundles", round12.bundle_of(rows()[TWIN]))
        # The other Editor's capture, bundle or build.log, in either round.
        misfiled(lambda a, b: copy(a["6000"], a["2022"], *body), "not a capture of run " + RUN_IDS["2022"])
        misfiled(lambda a, b: copy(a["6000"], a["2022"], *bundle), "not a bundle of Unity 2022")
        misfiled(lambda a, b: copy(a["6000"], a["2022"], "build.log"), "build.log is not this run's")
        misfiled(lambda a, b: copy(b["6000"], b["2022"], "replays", "body", "body-probes-face.pending.json"),
                 "not a capture of run")
        # A recorded case's capture or bundle gone, or a file no case records.
        misfiled(lambda a, b: a["2022"].joinpath(*body).unlink(), "recorded cases lack")
        misfiled(lambda a, b: a["2022"].joinpath(*bundle).unlink(), "recorded cases lack")
        misfiled(lambda a, b: write_json(a["2022"] / "replays" / "body" / "stray.pending.json", {}),
                 "holds files no case records")
        misfiled(lambda a, b: (a["2022"] / "bundles" / "stray").write_bytes(b""), "holds files no case builds")

    def test_a_b5a_case_must_be_recorded_and_a_lacking_twist_case_is_listed(self):
        for case, refused in ((TWIN, True), ("body-twist-sweep-compact", True), (TWIST_CLIP, False),
                              ("body-twist-compact-t5", False)):
            r12, r11_ = self.runs()
            path = r12["6000"] / "inventory.json"
            value = json.loads(path.read_text(encoding="utf-8"))
            next(row for row in value["rows"] if row["name"] == case)["status"] = "mismatch"
            write_json(path, value)
            with self.subTest(case=case):
                if refused:
                    with self.assertRaisesRegex(ValueError, "B5a case"):
                        self.intake(r12, r11_)
                else:
                    self.assertIn(case, self.intake(r12, r11_)[1]["round12"]["6000"]["twist_only_lacking"])

    def test_a_symlink_is_refused(self):
        r12, r11_ = self.runs()
        path = r12["2022"] / "replays" / "body" / f"{TWIN}.pending.json"
        moved = r12["2022"] / "elsewhere.json"
        path.rename(moved)
        path.symlink_to(moved)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.intake(r12, r11_)


class AuthoredIntakeTests(Runs, unittest.TestCase):
    REAL = True

    def test_a_wrong_twist_only_curve_is_twists_alone(self):
        def broken(case, gone=False):
            r12, r11_ = self.runs()
            path = r12["2022"] / "authored.json"
            value = json.loads(path.read_text(encoding="utf-8"))
            clip = next(clip for clip in value["clips"] if case in clip["cases"])
            curve = next(b for b in clip["bindings"] if b["values"])
            curve["values"][1] = r12tables.bits(.4375)
            write_json(path, value)
            if gone:
                (r12["2022"] / "replays" / "body" / f"{case}.pending.json").unlink()
            return self.intake(r12, r11_)
        record = broken(TWIST_CLIP)[1]["round12"]["2022"]
        self.assertLessEqual({TWIST_CLIP, TWIST_CLIP + "-offset"}, set(record["twist_only_misauthored"]))
        self.assertEqual(record["twist_only_lacking"], [])
        with self.assertRaisesRegex(ValueError, "authored.json"):
            broken(TWIN)
        # The case is still recorded and matched in both manifests: its file is required, authored wrongly or not.
        with self.assertRaisesRegex(ValueError, "recorded cases lack"):
            broken(TWIST_CLIP, gone=True)


class ViewTests(Runs, unittest.TestCase):
    def test_a_sealed_file_is_never_reached_early(self):
        r12, r11_ = self.runs()
        view = round12.round12_view(r12["2022"])
        self.assertEqual(sorted(round12.complete(view, readers(), [EXPLORATION])), [EXPLORATION])
        for case in (TWIN, "body-twist-compact-m1", SELECTION):
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, "sealed"):
                view.capture(case)
            with self.assertRaisesRegex(ValueError, "sealed"):
                view.bundle(case)
        self.assertFalse(any(TWIN in path for path in view.read))
        self.assertEqual((view.allows(EXPLORATION), view.allows(TWIN), view.allows("body-probes-face")),
                         (True, False, False))
        ledger, record = scratch.for_case(self), dict(intake="f")
        digest = round12.sha256(round12.canonical_json(record))
        with self.assertRaisesRegex(ValueError, "needs the intake's claim"):
            round12.round12_view(r12["2022"], round12.PARTITIONS)
        with self.assertRaisesRegex(ValueError, "opens only after the intake's claim"):
            round12.round12_view(r12["2022"], round12.PARTITIONS, lambda: seal.require_claim(digest, ledger))
        claimed(record, ledger)
        sealed = round12.round12_view(r12["2022"], round12.PARTITIONS, lambda: seal.require_claim(digest, ledger))
        self.assertEqual(sorted(round12.complete(sealed, readers(), [TWIN, TWIST_CLIP])), [TWIN, TWIST_CLIP])
        view11 = round12.round11_view(r11_["2022"])
        with self.assertRaisesRegex(ValueError, "sealed"):
            view11.capture("body-probes-face")
        witness = allowlisted("compact")
        self.assertEqual(view11.capture(witness)["spec"]["evidence"]["editor"]["fixture_case"], witness)
        with self.assertRaisesRegex(ValueError, "not a case this view can open"):
            view11.capture("body-clip-tall-scoped-level-offset")
        with self.assertRaisesRegex(ValueError, "is not a run file"):
            view.run_file("../round.json")

    def test_selection_opens_once_after_this_intakes_ambiguity_and_before_its_claim(self):
        r12, _ = self.runs()
        ledger, record = scratch.for_case(self), dict(intake="e")
        digest = round12.sha256(round12.canonical_json(record))

        def selected():
            return seal.require_selection(digest, ledger)
        with self.assertRaisesRegex(ValueError, "selection transition"):
            round12.round12_view(r12["2022"], ("exploration", "selection"))
        with self.assertRaisesRegex(ValueError, "no selection marker"):
            round12.round12_view(r12["2022"], ("exploration", "selection"), selection=selected)
        with self.assertRaisesRegex(ValueError, "after this intake's exploration is recorded"):
            seal.open_selection(digest, ledger)
        # Another intake's ambiguity opens nothing here; nor does an exploration that resolved everything.
        seal.record_exploration("0" * 64, [["cell", "t5"]], ledger)
        with self.assertRaisesRegex(ValueError, "after this intake's exploration is recorded"):
            seal.open_selection(digest, ledger)
        resolved = scratch.for_case(self) / "resolved"
        seal.record_exploration(digest, [], resolved)
        with self.assertRaisesRegex(ValueError, "on an ambiguity"):
            seal.open_selection(digest, resolved)
        seal.record_exploration(digest, [["cell", "t5"]], ledger)
        with self.assertRaisesRegex(ValueError, "recorded once"):
            seal.record_exploration(digest, [["cell", "t1"]], ledger)
        seal.open_selection(digest, ledger)
        with self.assertRaisesRegex(ValueError, "opens once"):
            seal.open_selection(digest, ledger)
        view = round12.round12_view(r12["2022"], ("exploration", "selection"), selection=selected)
        self.assertEqual(sorted(round12.complete(view, readers(), [SELECTION])), [SELECTION])
        with self.assertRaisesRegex(ValueError, "sealed"):
            view.capture(TWIN)
        # After the claim, the transition is closed: selection opens only through the claim's sealed views.
        claimed(record, ledger)
        with self.assertRaisesRegex(ValueError, "after the claim"):
            round12.round12_view(r12["2022"], ("exploration", "selection"), selection=selected)
        late = scratch.for_case(self) / "late"
        claimed(record, late)
        with self.assertRaisesRegex(ValueError, "before the intake's claim"):
            seal.record_exploration(digest, [["cell", "t5"]], late)
        with self.assertRaisesRegex(ValueError, "before the intake's claim"):
            seal.open_selection(digest, late)

    def test_completion_checks_identity_bundle_and_times(self):
        def broken(change, pattern, case=EXPLORATION):
            r12, _ = self.runs()
            path = r12["2022"] / "replays" / "body" / f"{case}.pending.json"
            doc = json.loads(path.read_text(encoding="utf-8"))
            change(doc, r12["2022"])
            write_json(path, doc)
            ledger = scratch.for_case(self)
            digest = claimed(dict(intake="d"), ledger)
            view = round12.round12_view(r12["2022"], round12.PARTITIONS, lambda: seal.require_claim(digest, ledger))
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                round12.complete(view, readers(), [case])
        sibling = "assets/unityrsfixtures/body-clips-compact-scoped-twin/body-clip-compact-scoped-twin-bake-o1-y1-xz1.anim"
        broken(lambda d, f: d["spec"].update(clip_asset=sibling), "fixture identity mismatch", TWIN)
        broken(lambda d, f: d["spec"].update(prefab_asset=d["spec"]["prefab_asset"].replace("identity", "offset")),
               "fixture identity mismatch", TWIN)
        broken(lambda d, f: d["spec"].update(mode="animator"), "playback mode mismatch")
        broken(lambda d, f: d["spec"]["evidence"]["editor"].update(fixture_case="body-twist-compact-t1"),
               "records another case")
        broken(lambda d, f: d["capture"].update(editor_version="6000.6.9f1"), "another Editor version")
        broken(lambda d, f: d["spec"].update(bundles=[dict(name="body-twist-compact-t1")]), "identity mismatch")
        broken(lambda d, f: (f / "bundles" / EXPLORATION).write_bytes(b"other"), "pinned sha256")
        broken(lambda d, f: d["capture"]["samples"].pop(), "not the tables' times")
        broken(lambda d, f: d["capture"]["samples"][1].update(local=d["capture"]["samples"][1]["local"] * 1.001),
               "not the tables' times")

    def test_a_bundle_decodes_from_the_hashed_bytes(self):
        r12, _ = self.runs()
        view = round12.round12_view(r12["2022"])
        doc = view.capture(EXPLORATION)
        private = view.bundle(EXPLORATION, doc)
        source = r12["2022"] / "bundles" / EXPLORATION
        self.assertNotEqual(private, source)
        hashed = source.read_bytes()
        source.write_bytes(hashed + b"replaced")
        self.assertEqual(private.read_bytes(), hashed)
        self.assertEqual((view.private() / "run.json").read_bytes(), (r12["2022"] / "run.json").read_bytes())
        self.assertIn("run.json", view.read)

    def test_every_byte_read_must_be_the_intakes(self):
        r12, r11_ = self.runs()
        _, record = self.intake(r12, r11_)
        view = round12.round12_view(r12["2022"])
        round12.complete(view, readers(), [EXPLORATION])
        round12.consumed(record, [("2022", "round12", view)])
        path = r12["2022"] / "replays" / "body" / f"{EXPLORATION}.pending.json"
        path.write_text(path.read_text(encoding="utf-8") + " ", encoding="utf-8")
        changed = round12.round12_view(r12["2022"])
        round12.complete(changed, readers(), [EXPLORATION])
        with self.assertRaisesRegex(ValueError, "read bytes the intake never bound"):
            round12.consumed(record, [("2022", "round12", changed)])


class FixtureTests(unittest.TestCase):
    def test_every_clip_case_is_its_tables_variant_and_placement(self):
        found = {name: (variant, root) for name, _, _, variant, root in r12tables.full_clips()}
        cases = [row for row in rows().values() if row["family"] in ("twist-clip", "twin")]
        self.assertEqual(sorted(row["name"] for row in cases), sorted(found))
        self.assertEqual({root for _, root in found.values()}, {"identity", "offset"})
        for row in cases:
            variant, root = found[row["name"]]
            tag = "twist" if row["family"] == "twist-clip" else "twin"
            fixture = round12.fixture_of(row)
            with self.subTest(case=row["name"]):
                self.assertEqual((fixture.prefab, fixture.clip, fixture.bundle, fixture.playback),
                                 (root, f"body-clip-{row['rig']}-{tag}-{variant}", round12.bundle_of(row), "animator"))
                self.assertTrue(round12.muscle_curves(row["name"], row))
        # A variant whose own name ends in a placement word: the identity clip is not an offset one.
        for case, prefab, clip in (
                ("body-clip-compact-scoped-twin-orientation-offset", "identity",
                 "body-clip-compact-scoped-twin-orientation-offset"),
                ("body-clip-compact-scoped-twin-orientation-offset-offset", "offset",
                 "body-clip-compact-scoped-twin-orientation-offset")):
            fixture = round12.fixture_of(rows()[case])
            self.assertEqual((fixture.prefab, fixture.clip), (prefab, clip))

    def test_the_prefab_avatar_and_clip_resolve_in_the_bundle(self):
        resolved = []

        class Bundle:
            def __init__(self, path, digest, *_):
                self.path = path

            def asset(self, name, class_id):
                resolved.append((name, class_id))
                return name

            def value(self, key):
                return {"asset": key}
        folder = make_round12(scratch.for_case(self) / "run", "2022", STUB, [EXPLORATION])
        view = round12.round12_view(folder)
        doc = view.capture(EXPLORATION)
        with mock.patch.object(complete, "BundleView", Bundle), \
                mock.patch.object(complete, "read_manifest", return_value=(None, {})) as manifest:
            avatar_of, clip_of = round12.readers("unityrs")
            avatar_of(view, EXPLORATION, doc)
            clip_of(view, EXPLORATION, doc)
        self.assertEqual(manifest.call_args.args[0], view.private())
        spec = doc["spec"]
        self.assertEqual(resolved, [(spec["prefab_asset"], 1), (spec["avatar_asset"], 90), (spec["clip_asset"], 74)])


class ClipTests(unittest.TestCase):
    def tracks(self, row, change=None):
        """Dense compiled tracks equal to the case's authored curves, by muscle ordinal."""
        names = r12tables.humantrait_order()
        from anim_fixture_full import muscle_property

        ordinal = {muscle_property(name): m for m, name in enumerate(names)}
        curves = round12.muscle_curves(row["name"], row)
        found = {ordinal[prop]: (False, [(clips.f32(t), (0., 0., 0., clips.f32(v))) for t, v in zip(*curve)])
                 for prop, curve in curves.items()}
        if change:
            change(found)
        return curves, found

    def check(self, case, change=None):
        row = rows()[case]
        doc = capture(case, "r", "v", b"", "", [], round12.fixture_of(row))
        curves, tracks = self.tracks(row, change)
        with mock.patch.object(clips, "compiled_muscles", return_value=tracks) as compiled:
            round12.check_clip(case, row, doc, {"clip": case})
        self.assertEqual(compiled.call_args.args[2], curves)

    def test_the_compiled_clip_is_the_cases(self):
        self.check(SUBSET)
        self.check(TWIN)

        def drift(tracks):
            m = sorted(tracks)[0]
            stream, keys = tracks[m]
            t, (a, b, c, v) = keys[1]
            keys[1] = (t, (a, b, c, clips.f32(v + 2 ** -10)))
        with self.assertRaisesRegex(ValueError, "not the authored"):
            self.check(SUBSET, drift)
        # A twin's or twist clip's compiled values are checked within f32 rounding by compiled_muscles itself.
        self.check(TWIN, drift)

    def test_the_slot_check_is_compiled_muscles(self):
        # compiled_muscles refuses an index entry no binding of its DOF owns (an m_IndexArray the Editor may leave
        # unverified), before any value is read.
        row = rows()[TWIN]
        doc = capture(TWIN, "r", "v", b"", "", [], round12.fixture_of(row))
        clip = {"m_MuscleClip": {"m_IndexArray": [0] + [-1] * (clips.DOF_COUNT - 1)}}
        with mock.patch("anim_fixture_scope.check_human_mapping"), \
                mock.patch.object(clips, "owners", return_value={}), \
                self.assertRaisesRegex(ValueError, "no binding of theirs owns"):
            round12.check_clip(TWIN, row, doc, clip)


class Round11CompletionTests(unittest.TestCase):
    def test_round_12s_sealed_evidence_opens_only_through_its_own_intakes_claim(self):
        folder = scratch.for_case(self) / "run"
        body = folder / "replays" / "body"
        names = ("body-probes-face", "body-probes-fingers", sorted(round12.round11_cases())[0],
                 allowlisted("tall"), "body-clip-tall-scoped-level-offset")
        for name in names:
            write_json(body / f"{name}.pending.json", dict(case=name))
        write_json(folder / "run.json", dict(run=dict(run_id="c" * 32, editor_version=VERSIONS["2022"])))
        seen = []

        def recorded_paths(folder_, *_):
            return sorted(Path(folder_) / "replays" / "body" / f"{name}.pending.json" for name in names), False, {}, []

        def complete_folder(folder_, unityrs, inspect_dir, replay_):
            paths = complete.recorded_paths(folder_, replay_, {}, {})[0]
            seen.append((Path(folder_), {p.name: p.read_bytes() for p in paths}))
            return len(paths), [], []

        def bound(run_id, *files):
            return dict(round11={"2022": dict(run_id=run_id, files={name: hashlib.sha256(
                (folder / name).read_bytes()).hexdigest() for name in ("run.json", *files)})})
        face = "replays/body/body-probes-face.pending.json"
        ledger = scratch.for_case(self) / "ledger"
        with mock.patch.object(r11, "check_inventory"), mock.patch.object(complete, "complete_folder", complete_folder), \
                mock.patch.object(complete, "recorded_paths", recorded_paths), mock.patch.object(seal, "LEDGER_DIR", ledger):
            count, _, _, held = r11.complete_round(folder, "unityrs", None, replay)
            sealed = {n for n, p in round12.round11_cases().items() if p == "confirmation"}
            self.assertEqual(held, sorted(n for n in names if n in sealed))
            self.assertEqual(count, len(names) - len(held))
            self.assertFalse({name.removesuffix(".pending.json") for name in seen[-1][1]} & sealed)
            self.assertIs(complete.recorded_paths, recorded_paths)
            record = bound("c" * 32, face)
            digest = round12.sha256(round12.canonical_json(record))
            with self.assertRaisesRegex(ValueError, "opens only after the intake's claim"):
                r11.complete_round(folder, "unityrs", None, replay, digest)
            # Another intake's claim, binding another run, opens nothing here.
            other = claimed(bound("e" * 32, face), ledger)
            with self.assertRaisesRegex(ValueError, "does not bind round 11 run"):
                r11.complete_round(folder, "unityrs", None, replay, other)
            claimed(record, ledger)
            (folder / face).write_bytes(b"changed after the intake")
            with self.assertRaisesRegex(ValueError, "differ from the claimed intake's"):
                r11.complete_round(folder, "unityrs", None, replay, digest)
            write_json(folder / face, dict(case="body-probes-face"))
            self.assertEqual(r11.complete_round(folder, "unityrs", None, replay, digest)[3], [])
            private, read = seen[-1]
            self.assertNotEqual(private, folder)
            self.assertEqual(sorted(read), sorted(f"{name}.pending.json" for name in names))
            self.assertEqual(read["body-probes-face.pending.json"], (folder / face).read_bytes())


if __name__ == "__main__":
    unittest.main()
