"""B5a's round 11 inventory (anim_muscle_round11): the cases its confirmation requires, read beside freeze v8's
frozen Run, and a round 11 run's declaration."""

import json
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_body_runs as runs  # noqa: E402
import anim_muscle_round11 as round11  # noqa: E402
import scratch  # noqa: E402


class InventoryTests(unittest.TestCase):
    def test_the_frozen_cases(self):
        # All four sweeps and every full-body clip: 25 refusal evidence, 318 in scope.
        self.assertEqual(sorted(round11.SWEEPS), ["body-probes-face", "body-probes-fingers",
                                                  "body-probes-muscles-compact", "body-probes-muscles-tall"])
        self.assertEqual(round11.SWEEPS["body-probes-muscles-tall"], "body-muscles-tall")
        clips = round11.FULL_CLIPS
        self.assertEqual((len(clips), sum("-full-" in name for name in clips),
                          sum("-scoped-" in name for name in clips)), (343, 25, 318))
        self.assertEqual(len(round11.CASES), 347)
        self.assertEqual(clips["body-clip-tall-jaw-scoped-bake-o1-y1-xz1-offset"], "body-clips-tall-jaw-scoped")
        self.assertEqual(clips["body-clip-tall-scoped-level-offset"], "body-clips-tall-scoped")
        self.assertEqual(clips["body-clip-hips-scale-small-scoped-keep-o1-y1-xz1-motion"],
                         "body-clips-hips-scale-small-scoped")
        # An F5 feature's clips play on the identity root alone; no start twin is a full-body clip.
        self.assertNotIn("body-clip-armature-scoped-keep-o1-y1-xz1-motion-offset", clips)
        self.assertFalse(any("-start" in name for name in clips))
        for name, bundle in clips.items():
            self.assertEqual(round11.full_clip_bundle(name), bundle)
        self.assertIsNone(round11.full_clip_bundle("body-clip-bake-o0-y0-xz1"))
        # The frozen Run never lists them, and is not edited to.
        self.assertFalse(round11.CASES.keys() & runs.CASES.keys())

    def test_a_listed_full_body_clip_is_required_too(self):
        extra = "body-clip-huge-full-level"
        required = round11.required({extra: "recorded", "body-clip-level": "recorded"})
        self.assertEqual(required[extra], "body-clips-huge-full")
        self.assertNotIn("body-clip-level", required)
        self.assertEqual(len(required), 348)


class RunTests(unittest.TestCase):
    def folder(self, recorded=(), round_json=True, run_id="r", bundle=None):
        folder = scratch.for_case(self)
        rows = [dict(name=name, status="recorded" if name in recorded else "not_covered") for name in round11.CASES]
        (folder / "run.json").write_text(json.dumps(dict(run=dict(run_id="r", editor_version="6000.6.9f1"),
                                                         cases=rows)), encoding="utf-8")
        if round_json:
            (folder / "round.json").write_text(json.dumps(dict(format="unityrs.fixture_round", round=11,
                                                               run_id=run_id)), encoding="utf-8")
        (folder / "replays" / "body").mkdir(parents=True)
        for name in recorded:
            doc = dict(spec=dict(bundles=[dict(name=bundle or round11.CASES[name])]))
            (folder / "replays" / "body" / f"{name}.pending.json").write_text(json.dumps(doc), encoding="utf-8")
        return folder

    def test_the_declared_round(self):
        self.assertEqual(round11.declared_round(self.folder()), 11)
        self.assertIsNone(round11.declared_round(self.folder(round_json=False)))
        with self.assertRaisesRegex(ValueError, "does not declare"):
            round11.declared_round(self.folder(run_id="another"))

    def test_the_recordings_and_what_the_run_lacks(self):
        recorded = ("body-probes-face", "body-clip-compact-full-level")
        run = round11.Recordings(self.folder(recorded))
        self.assertEqual(sorted(run.documents), sorted(recorded))
        self.assertEqual(len(run.missing), 345)
        self.assertIn("body-probes-fingers", run.missing)
        self.assertEqual(run.version, "6000.6.9f1")

    def test_a_recorded_case_without_its_file_is_missing(self):
        folder = self.folder(("body-probes-face",))
        (folder / "replays" / "body" / "body-probes-face.pending.json").unlink()
        self.assertIn("body-probes-face", round11.Recordings(folder).missing)

    def test_another_bundle_is_refused(self):
        with self.assertRaisesRegex(ValueError, "another bundle"):
            round11.Recordings(self.folder(("body-probes-face",), bundle="body-muscles-compact"))


if __name__ == "__main__":
    unittest.main()
