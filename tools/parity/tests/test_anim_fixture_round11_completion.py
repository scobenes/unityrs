"""Round 11's completion (anim_fixture_round11): the Avatar read as a value, never its asset key; each bundle
object's identity with its full container path where `inspect` lists a shortened one; and the two body sweeps'
expected failure (URS-42), accepted with the recorder's exact message alone."""

import json
import unittest

import test_anim_replay_coverage as coverage  # noqa: F401  (puts tools/parity on the path)
from anim_fixture_support import RUN
from test_anim_fixture_round11 import Built

import anim_fixture_body as body  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_replay_complete as complete  # noqa: E402
import scratch  # noqa: E402

FULL = "assets/unityrsfixtures/" + "x" * 120 + ".anim"
HEADER = dict(input=0, file_index=1, file="CAB-0", path_id=-7, class_id=74, class_name="AnimationClip", name="x")
SWEEPS = ("body-probes-muscles-compact", "body-probes-muscles-tall")


def preview(text, keep=128, **edits):
    return dict({"$unityrs": "preview", "length": len(text), "prefix": text[:keep], "omitted": len(text) - keep},
                **edits)


class AvatarTests(unittest.TestCase):
    def test_the_avatar_is_its_value_never_its_key(self):
        # BundleView.asset returns a key, as the fake now does: a key in place of the value refuses.
        self.assertEqual(Built.AVATAR, (0, -90))
        with self.assertRaisesRegex(ValueError, "no serialized Avatar m_Scale in x"):
            r11.human_scale("x", Built.AVATAR, replay)
        self.assertEqual(r11.human_scale("x", {"m_Avatar": {"m_Human": {"m_Scale": 1.25}}}, replay), 1.25)


class PreviewTests(unittest.TestCase):
    def resolved(self, listed, shown):
        headers = {(1, -7): dict(HEADER, container_path=listed)}
        r11.resolve_preview(headers, {"object": dict(HEADER, **shown)})
        return headers[(1, -7)]["container_path"]

    def test_a_shortened_path_resolves_to_shows_full_identity(self):
        self.assertEqual(self.resolved(preview(FULL), dict(container_path=FULL)), FULL)

    def test_anything_but_that_paths_preview_stays_and_refuses(self):
        # (The omitted characters are the listing's to leave out: show's path supplies them.)
        other = FULL[:-6] + "y.anim"
        for label, listed, shown in (
                ("another length", preview(FULL), dict(container_path=FULL + "x")),
                ("another prefix", preview(other.replace("assets", "assetz")), dict(container_path=other)),
                ("a wrong omitted count", preview(FULL, omitted=1), dict(container_path=FULL)),
                ("another object", preview(FULL), dict(container_path=FULL, name="y")),
                ("a listed full path", "a.anim", dict(container_path="b.anim"))):
            with self.subTest(label):
                self.assertEqual(self.resolved(listed, shown), listed)

    def test_completion_compares_the_full_identity_while_it_runs(self):
        base = complete.BundleView
        with r11.round11_tables():
            self.assertTrue(issubclass(complete.BundleView, base) and complete.BundleView is not base)
        self.assertIs(complete.BundleView, base)


class ExpectedFailureTests(unittest.TestCase):
    def row(self, name, status="failed", reason=None):
        return dict(name=name, status=status,
                    reason=reason if reason is not None else r11.EXPECTED_FAILURES[name][0])

    def test_only_the_body_sweeps_failing_with_the_recorders_message(self):
        self.assertEqual(set(r11.EXPECTED_FAILURES), set(SWEEPS))
        for name in SWEEPS:
            self.assertEqual(r11.accepted_failure(self.row(name)), dict(
                name=name, status="not_covered", reason="expected failure: limb-twist redistributes at twist 0 "
                                                        "(URS-42)"))
            self.assertIsNone(r11.accepted_failure(self.row(name, reason=f"Record {name}: Spine Front-Back at 1 "
                                                                          f"turned no bone in {name}.")))
            self.assertIsNone(r11.accepted_failure(self.row(name, status="skipped")))
        self.assertIsNone(r11.accepted_failure(self.row("body-probes-fingers", reason=r11.EXPECTED_FAILURES[
            SWEEPS[0]][0])))

    def inventory(self, rows):
        with scratch.folder() as folder:
            names = [*body.BODY_CASES, *r11.ROUND11_CASES]
            cases = {name: dict(name=name, status="recorded", reason="") for name in names}
            cases |= {row["name"]: row for row in rows}
            (folder / "run.json").write_text(json.dumps(dict(format="unityrs.fixture_run", version=1, run=RUN,
                                                             cases=list(cases.values()))), encoding="utf-8")
            (folder / "round.json").write_text(json.dumps(dict(format="unityrs.fixture_round", round=11,
                                                               run_id=RUN["run_id"])), encoding="utf-8")
            r11.check_inventory(folder, replay)

    def test_the_inventory_accepts_them_and_nothing_else(self):
        self.inventory([self.row(name) for name in SWEEPS])
        self.inventory([])
        with self.assertRaisesRegex(ValueError, r"not recorded \(1\): body-probes-muscles-tall \(failed\)"):
            self.inventory([self.row(SWEEPS[0]), self.row(SWEEPS[1], reason="Record: another failure.")])
        with self.assertRaisesRegex(ValueError, r"not recorded \(1\): body-probes-fingers \(failed\)"):
            self.inventory([self.row("body-probes-fingers", reason="x")])

    def test_completion_lists_them_as_not_covered(self):
        with scratch.folder() as folder:
            cases = [self.row(SWEEPS[0]), self.row("body-probes-fingers", reason="x")]
            (folder / "run.json").write_text(json.dumps(dict(format="unityrs.fixture_run", version=1, run=RUN,
                                                             cases=cases)), encoding="utf-8")
            with r11.round11_tables():
                found, _ = complete.read_manifest(folder, replay)
            self.assertEqual([(row["status"], row["reason"]) for row in found["cases"]], [
                ("not_covered", "expected failure: limb-twist redistributes at twist 0 (URS-42)"), ("failed", "x")])
            self.assertEqual(complete.read_manifest(folder, replay)[0]["cases"], cases)


if __name__ == "__main__":
    unittest.main()
