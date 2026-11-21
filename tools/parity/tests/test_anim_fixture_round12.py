"""Round 12's intake (anim_fixture_round12): completion, the allowlisted views, the intake digest and B5a's rows, on
synthetic run folders, wrong, swapped, tampered and sealed ones among them."""

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
import anim_muscle_confirm as confirm  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_round12_tables as r12tables  # noqa: E402
import anim_twist_seal as seal  # noqa: E402
import scratch  # noqa: E402
from anim_twist_witness import allowlisted  # noqa: E402
from test_anim_round12_tables import authored, humantrait_names  # noqa: E402

VERSIONS = {"2022": "2022.3.62f3", "6000": "6000.6.9f1"}
# A case of each partition and family the tests read: an exploration probe, a confirmation twin, a selection probe.
EXPLORATION, TWIN = "body-twist-compact-t0", "body-clip-compact-scoped-twin-level"
TWIST_CLIP = "body-clip-compact-scoped-twist-level"


def rows():
    return {row["name"]: row for row in round12.tables()[0]["cases"]}


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def capture(case, run_id, version, bundle_bytes, bundle, times):
    return dict(spec=dict(bundles=[dict(name=bundle)], evidence=dict(editor=dict(fixture_case=case, run_id=run_id))),
                capture=dict(editor_version=version, bundle_sha256=[hashlib.sha256(bundle_bytes).hexdigest()],
                             samples=[dict(local=replay.f32_from_bits(t)) for t in times]))


def make_round12(folder, editor, authored_doc, recorded=(EXPLORATION, TWIN, TWIST_CLIP)):
    """A round 12 run of `editor` whose run files are the builder's, holding captures and bundles of `recorded`."""
    run_id = ("a" if editor == "2022" else "b") * 32
    doc, tables_sha256 = round12.tables()
    names = sorted(round12.partitions())
    write_json(folder / "run.json", dict(format="unityrs.fixture_run", version=1, run=dict(
        run_id=run_id, editor_version=VERSIONS[editor], build_target="StandaloneOSX", build_target_id="2",
        variant="round12"), cases=[dict(name=name, status="recorded") for name in names]))
    write_json(folder / "round.json", dict(format="unityrs.fixture_round", round=12, run_id=run_id,
                                           tables_sha256=tables_sha256, tables_digest=doc["tables_digest"],
                                           editor_sources_sha256=round12.editor_sources_sha256(),
                                           importer_default=round12.IMPORTER_DEFAULT))
    write_json(folder / "inventory.json", dict(format="unityrs.round12_inventory", run_id=run_id,
                                               tables_sha256=tables_sha256, cases=len(names), matched=len(names),
                                               rows=[dict(name=n, status="matched", reason="", samples=1)
                                                     for n in names], extra=[]))
    (folder / "authored.json").write_text(re.sub('"run_id": ?"r"', f'"run_id": "{run_id}"', authored_doc, count=1),
                                          encoding="utf-8")
    (folder / "build.log").write_text("built\n", encoding="utf-8")
    (folder / "bundles").mkdir()
    (folder / "replays" / "body").mkdir(parents=True)
    for case in recorded:
        row = rows()[case]
        bundle = round12.bundle_of(row)
        data = f"{editor} {bundle}".encode()
        (folder / "bundles" / bundle).write_bytes(data)
        write_json(folder / "replays" / "body" / f"{case}.pending.json",
                   capture(case, run_id, VERSIONS[editor], data, bundle, row["times"]))
    return folder


def make_round11(folder, editor):
    """A declared round 11 run of `editor` holding every reused case's capture and bundle."""
    run_id = ("c" if editor == "2022" else "d") * 32
    write_json(folder / "run.json", dict(run=dict(run_id=run_id, editor_version=VERSIONS[editor]), cases=[]))
    write_json(folder / "round.json", dict(format="unityrs.fixture_round", round=11, run_id=run_id))
    (folder / "bundles").mkdir()
    for case in round12.round11_cases():
        bundle = r11.ROUND11_CASES[case].bundle
        data = f"{editor} {bundle}".encode()
        (folder / "bundles" / bundle).write_bytes(data)
        write_json(folder / "replays" / "body" / f"{case}.pending.json",
                   capture(case, run_id, VERSIONS[editor], data, bundle, []))
    return folder


STUB = json.dumps(dict(run_id="r", clips="stub"))


def stub_check(authored_doc, doc):
    """anim_round12_tables.check_authored for a run whose authored.json is STUB (AuthoredIntakeTests reads the real
    one): the curves pass, a lacking clip case is reported as the real check reports it."""
    return [] if authored_doc.get("clips") == "stub" else ["not the stub"]


class Runs:
    """Both Editors' round 12 and round 11 runs, built once per class and copied for each test. Unless REAL, their
    authored.json is STUB, checked by `stub_check`."""
    REAL = False

    @classmethod
    def setUpClass(cls):
        doc = round12.tables()[0]
        text = json.dumps(authored(doc, humantrait_names(doc)), indent=1) if cls.REAL else STUB
        cls.template = scratch.for_class(cls)
        for editor in VERSIONS:
            make_round12(cls.template / f"r12-{editor}", editor, text)
            make_round11(cls.template / f"r11-{editor}", editor)

    def setUp(self):
        if not self.REAL:
            patcher = mock.patch.object(r12tables, "check_authored", stub_check)
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
    def test_the_digest_binds_every_run_and_reused_file_in_any_order(self):
        r12, r11_ = self.runs()
        digest, record = self.intake(r12, r11_)
        self.assertEqual(round12.intake(list(r12.values())[::-1], list(r11_.values())[::-1])[0], digest)
        self.assertEqual(sorted(record["round12"]), ["2022", "6000"])
        files = record["round12"]["2022"]["files"]
        self.assertLessEqual({*round12.RUN_FILES, f"replays/body/{TWIN}.pending.json"}, files.keys())
        self.assertEqual(record["round12"]["2022"]["twist_only_lacking"], [])
        reused = record["round11"]["6000"]["files"]
        self.assertEqual(len([p for p in reused if p.startswith("replays/")]), len(round12.round11_cases()))
        self.assertEqual(record["partition_digest"], round12.partition_digest())
        # Any byte of a bound file changes it; a round 11 capture outside the reuse is not bound.
        (r11_["6000"] / "replays" / "body" / "body-clip-tall-scoped-level-offset.pending.json").write_text("{}")
        self.assertEqual(self.intake(r12, r11_)[0], digest)
        for path in (r12["2022"] / "build.log", r12["6000"] / "bundles" / round12.bundle_of(rows()[TWIN]),
                     r11_["2022"] / "replays" / "body" / "body-probes-face.pending.json"):
            with self.subTest(path=path.name):
                path.write_bytes(path.read_bytes() + b" ")
                self.assertNotEqual(self.intake(r12, r11_)[0], digest)

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
        # A round 11 run given as round 12, and the other way round.
        r12, r11_ = self.runs()
        with self.assertRaisesRegex(ValueError, "not a round 12 run|expected unityrs.fixture_run"):
            round12.intake([r11_["2022"], r12["6000"]], [r12["2022"], r11_["6000"]])

    def test_swapped_editors_are_refused(self):
        r12, r11_ = self.runs()
        with self.assertRaisesRegex(ValueError, "two round 12 runs of Editor 2022"):
            round12.intake([r12["2022"], r12["2022"]], list(r11_.values()))
        with self.assertRaisesRegex(ValueError, "two round 11 runs of Editor 6000"):
            round12.intake(list(r12.values()), [r11_["6000"], r11_["6000"]])
        # A capture copied from the other Editor's run names its own run and Editor.
        name = f"{EXPLORATION}.pending.json"
        shutil.copyfile(r12["6000"] / "replays" / "body" / name, r12["2022"] / "replays" / "body" / name)
        with self.assertRaisesRegex(ValueError, "recorded by another run"):
            round12.complete(round12.round12_view(r12["2022"]), [EXPLORATION])

    def test_a_b5a_case_must_be_recorded_and_a_lacking_twist_case_is_listed(self):
        for case, refused in ((TWIN, True), ("body-twist-sweep-compact", True), (TWIST_CLIP, False),
                              ("body-twist-compact-t5", False)):
            r12, r11_ = self.runs()
            path = r12["6000"] / "inventory.json"
            value = json.loads(path.read_text(encoding="utf-8"))
            next(row for row in value["rows"] if row["name"] == case)["status"] = "missing"
            write_json(path, value)
            with self.subTest(case=case):
                if refused:
                    with self.assertRaisesRegex(ValueError, "B5a case"):
                        self.intake(r12, r11_)
                else:
                    self.assertEqual(self.intake(r12, r11_)[1]["round12"]["6000"]["twist_only_lacking"], [case])

    def test_a_symlink_or_a_missing_reused_file_is_refused(self):
        r12, r11_ = self.runs()
        path = r12["2022"] / "replays" / "body" / f"{TWIN}.pending.json"
        moved = path.with_name("elsewhere.json")
        path.rename(moved)
        path.symlink_to(moved)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.intake(r12, r11_)
        r12, r11_ = self.runs()
        (r11_["2022"] / "replays" / "body" / "body-probes-fingers.pending.json").unlink()
        with self.assertRaisesRegex(ValueError, "not a file of the run"):
            self.intake(r12, r11_)


class AuthoredIntakeTests(Runs, unittest.TestCase):
    REAL = True

    def test_the_authored_curves_are_checked_and_a_lacking_twist_clip_may_be_unbuilt(self):
        r12, r11_ = self.runs()
        self.assertEqual(self.intake(r12, r11_)[1]["round12"]["2022"]["twist_only_lacking"], [])

        def drop(case, status):
            r12, r11_ = self.runs()
            path = r12["2022"] / "authored.json"
            value = json.loads(path.read_text(encoding="utf-8"))
            value["clips"] = [clip for clip in value["clips"] if case not in clip["cases"]]
            write_json(path, value)
            if status:
                inventory = json.loads((r12["2022"] / "inventory.json").read_text(encoding="utf-8"))
                for row in inventory["rows"]:
                    if row["name"] in (case, case + "-offset"):
                        row["status"] = status
                write_json(r12["2022"] / "inventory.json", inventory)
            return self.intake(r12, r11_)
        # A twist clip the run never built is lacking, never an authored problem; one it lists as matched is.
        self.assertEqual(drop(TWIST_CLIP, "missing")[1]["round12"]["2022"]["twist_only_lacking"],
                         [TWIST_CLIP, TWIST_CLIP + "-offset"])
        with self.assertRaisesRegex(ValueError, "no authored clip"):
            drop(TWIST_CLIP, None)
        with self.assertRaisesRegex(ValueError, "B5a case"):
            drop(TWIN, "missing")


class ViewTests(Runs, unittest.TestCase):
    def test_a_sealed_file_is_never_reached_early(self):
        r12, r11_ = self.runs()
        view = round12.round12_view(r12["2022"])
        self.assertEqual(sorted(round12.complete(view, [EXPLORATION])), [EXPLORATION])
        for case in (TWIN, "body-twist-compact-m1"):
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, "sealed"):
                view.capture(case)
            with self.assertRaisesRegex(ValueError, "sealed"):
                view.bundle(case)
        self.assertFalse(any(TWIN in path for path in view.read))
        # By default completion reads only what the view allows.
        self.assertEqual((view.allows(EXPLORATION), view.allows(TWIN), view.allows("body-probes-face")),
                         (True, False, False))
        # A sealed view needs the claim, and the claim needs its marker.
        ledger = scratch.for_case(self)
        digest = "f" * 64
        with self.assertRaisesRegex(ValueError, "needs the intake's claim"):
            round12.round12_view(r12["2022"], round12.PARTITIONS)
        with self.assertRaisesRegex(ValueError, "opens only after the intake's claim"):
            round12.round12_view(r12["2022"], round12.PARTITIONS, lambda: seal.require_claim(digest, ledger))
        seal.claim(digest, ledger)
        sealed = round12.round12_view(r12["2022"], round12.PARTITIONS, lambda: seal.require_claim(digest, ledger))
        self.assertEqual(sorted(round12.complete(sealed, [TWIN, TWIST_CLIP])), [TWIN, TWIST_CLIP])
        # Round 11: B5a's cases are sealed, the witness's open, any other is no case of the view.
        view11 = round12.round11_view(r11_["2022"])
        with self.assertRaisesRegex(ValueError, "sealed"):
            view11.capture("body-probes-face")
        witness = allowlisted("compact")
        self.assertEqual(view11.capture(witness)["spec"]["evidence"]["editor"]["fixture_case"], witness)
        with self.assertRaisesRegex(ValueError, "not a case this view can open"):
            view11.capture("body-clip-tall-scoped-level-offset")
        with self.assertRaisesRegex(ValueError, "is not a run file"):
            view.run_file("../round.json")

    def test_completion_checks_the_captures_case_bundle_and_times(self):
        def broken(change, pattern):
            r12, _ = self.runs()
            path = r12["2022"] / "replays" / "body" / f"{EXPLORATION}.pending.json"
            doc = json.loads(path.read_text(encoding="utf-8"))
            change(doc, r12["2022"])
            write_json(path, doc)
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                round12.complete(round12.round12_view(r12["2022"]), [EXPLORATION])
        broken(lambda d, f: d["spec"]["evidence"]["editor"].update(fixture_case="body-twist-compact-t1"),
               "records another case")
        broken(lambda d, f: d["capture"].update(editor_version="6000.6.9f1"), "another Editor version")
        broken(lambda d, f: d["spec"].update(bundles=[dict(name="body-twist-compact-t1")]), "plays another bundle")
        broken(lambda d, f: (f / "bundles" / EXPLORATION).write_bytes(b"other"), "pinned sha256")
        broken(lambda d, f: d["capture"]["samples"].pop(), "not the tables' times")
        broken(lambda d, f: d["capture"]["samples"][1].update(local=d["capture"]["samples"][1]["local"] * 1.001),
               "not the tables' times")

    def test_every_byte_read_must_be_the_intakes(self):
        r12, r11_ = self.runs()
        _, record = self.intake(r12, r11_)
        view = round12.round12_view(r12["2022"])
        round12.complete(view, [EXPLORATION])
        round12.consumed(record, [("2022", "round12", view)])
        path = r12["2022"] / "replays" / "body" / f"{EXPLORATION}.pending.json"
        path.write_text(path.read_text(encoding="utf-8") + " ", encoding="utf-8")
        changed = round12.round12_view(r12["2022"])
        round12.complete(changed, [EXPLORATION])
        with self.assertRaisesRegex(ValueError, "read bytes the intake never bound"):
            round12.consumed(record, [("2022", "round12", changed)])


def observation(rows_, refused, editors=VERSIONS):
    seen = confirm.Observation()
    seen.runs = {editor: "r" for editor in editors}
    seen.rows = {key: (0., 1.) for key in rows_}
    seen.refused = {key: (0., 1., reason) for key, reason in refused.items()}
    return seen


class B5aTests(unittest.TestCase):
    def test_the_pinned_refusals_and_the_twist_free_rows(self):
        pinned = round12.pinned_refusals()
        self.assertEqual(len(pinned), 25)
        self.assertEqual(set(pinned.values()), {"unsupported_humanoid_limb_twist(23)"})
        self.assertTrue(all(r11.ROUND11_INFO[case]["scope"] == "refusal" for case in pinned))
        free = round12.twist_free()
        families = {row["name"]: row["family"] for row in round12.tables()[0]["cases"]}
        self.assertEqual({families.get(case, "round 11") for case in free}, {"sweep", "twin", "round 11"})
        self.assertEqual(sum(families.get(case) == "twin" for case in free), 318)
        self.assertEqual(sorted(case for case in free if case not in families), ["body-probes-face",
                                                                                "body-probes-fingers"])
        self.assertEqual(free[TWIN], tuple(rows()[TWIN]["times"]))
        # A sweep keeps exactly its vectors whose ten limb-twist values are 0.
        sets = {row["name"]: row["vectors"] for row in round12.tables()[0]["sets"]}
        twists = {name for name, ordinal in ((r["name"], r["ordinal"]) for r in round12.tables()[0]["names"])
                  if ordinal in round12.LIMB_TWIST_ORDINALS}
        self.assertEqual(len(twists), 10)
        vectors = sets["sweep-compact"]
        kept = [k for k, v in enumerate(vectors) if all(replay.f32_from_bits(b) == 0
                                                        for m, b in zip(v["muscles"], v["bits"]) if m in twists)]
        self.assertEqual(free["body-twist-sweep-compact"], tuple(kept))
        self.assertLess(len(kept), len(vectors))

    def test_the_verdict_is_exact(self):
        rows_ = {(e, TWIN, 7, sid) for e in VERSIONS for sid in rows()[TWIN]["times"][:2]}
        case = sorted(round12.pinned_refusals())[0]
        refused = {(e, case, 7, "00000000") for e in VERSIONS}
        reason = "unsupported_humanoid_limb_twist(23)"
        expected = (rows_, refused)
        self.assertEqual(round12.b5a_failures(observation(rows_, dict.fromkeys(refused, reason)), expected), [])
        some = sorted(rows_)[1:]
        for seen, pattern in (
                (observation(some, dict.fromkeys(refused, reason)), "lack 1 row"),
                (observation(rows_ | {("2022", TWIN, 8, "00000000")}, dict.fromkeys(refused, reason)), "beyond"),
                (observation(rows_ | {("2022", TWIST_CLIP, 7, "00000000")}, dict.fromkeys(refused, reason)),
                 "twist-active"),
                (observation(rows_, dict.fromkeys(refused, "unsupported_humanoid_twist(m_ArmTwist)")),
                 "refuses as unsupported_humanoid_twist"),
                (observation(rows_, dict.fromkeys(sorted(refused)[:1], reason)), "lack refusal 1"),
                (observation(rows_, dict.fromkeys(refused, reason), editors=("2022",)), "no B5a runs of Editor 6000")):
            with self.subTest(pattern=pattern):
                self.assertTrue(any(pattern in line for line in round12.b5a_failures(seen, expected)),
                                round12.b5a_failures(seen, expected))
        outside = observation(rows_, dict.fromkeys(refused, reason))
        outside.rows[sorted(rows_)[0]] = (2., 1.)
        self.assertEqual(round12.b5a_failures(outside, expected), [
            f"1 B5a row(s) outside their bound: {[sorted(rows_)[0]]}"])

    def test_the_expected_rows_never_hold_a_twist_active_case(self):
        rows_, refused = round12.b5a_expected()
        families = {row["name"]: row["family"] for row in round12.tables()[0]["cases"]}
        self.assertFalse({key[1] for key in rows_ | refused if families.get(key[1]) in round12.TWIST_ONLY})
        self.assertEqual({key[1] for key in refused}, set(round12.pinned_refusals()))
        self.assertEqual({key[0] for key in rows_ | refused}, set(VERSIONS))


if __name__ == "__main__":
    unittest.main()
