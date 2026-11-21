"""B5a's round 11 inventory (anim_muscle_round11): the cases its confirmation requires, the builder's, read beside
freeze v8's frozen Run, and a round 11 run's declaration."""

import json
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_body_runs as runs  # noqa: E402
import anim_fixture_round12 as round12  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_muscle_round11 as round11  # noqa: E402
import scratch  # noqa: E402


class InventoryTests(unittest.TestCase):
    def test_the_frozen_cases(self):
        # All four sweeps and every full-body clip, as round 11's builder makes them: 25 refusal evidence, 318 in
        # scope, each on the builder's bundle.
        sweeps = sorted(case for case in round11.CASES if case not in round11.FULL_CLIPS)
        self.assertEqual(sweeps, ["body-probes-face", "body-probes-fingers", "body-probes-muscles-compact",
                                  "body-probes-muscles-tall"])
        self.assertEqual(round11.CASES["body-probes-muscles-tall"], "body-muscles-tall")
        self.assertEqual({case: r11.ROUND11_CASES[case].bundle for case in round11.CASES}, round11.CASES)
        self.assertEqual(sum(r11.ROUND11_INFO[case].get("scope") == "in" for case in round11.FULL_CLIPS), 318)
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


def open_view(folder):
    """A view opening every round 11 case, as the round 12 seal's after its claim."""
    return round12.View(folder, {case: ("exploration", bundle) for case, bundle in round11.CASES.items()})


def read(folder, view=None):
    """Recordings of `folder`, every document read."""
    run = round11.Recordings(folder, view or open_view(folder))
    dict(run.documents)
    return run


class RunTests(unittest.TestCase):
    def folder(self, recorded=(), round_json=True, run_id="r", bundle=None, identity=None):
        """A round 11 run folder; each recorded capture names its own case, run, Editor and bundle, as
        `identity` (case: (case, run, Editor version)) overrides."""
        folder = scratch.for_case(self)
        rows = [dict(name=name, status="recorded" if name in recorded else "not_covered") for name in round11.CASES]
        (folder / "run.json").write_text(json.dumps(dict(run=dict(run_id="r", editor_version="6000.6.9f1"),
                                                         cases=rows)), encoding="utf-8")
        if round_json:
            (folder / "round.json").write_text(json.dumps(dict(format="unityrs.fixture_round", round=11,
                                                               run_id=run_id)), encoding="utf-8")
        (folder / "replays" / "body").mkdir(parents=True)
        for name in recorded:
            case, run, version = (identity or {}).get(name, (name, "r", "6000.6.9f1"))
            doc = dict(spec=dict(bundles=[dict(name=bundle or round11.CASES[name])],
                                 evidence=dict(editor=dict(fixture_case=case, run_id=run))),
                       capture=dict(editor_version=version))
            (folder / "replays" / "body" / f"{name}.pending.json").write_text(json.dumps(doc), encoding="utf-8")
        return folder

    def test_the_declared_round(self):
        self.assertEqual(round11.declared_round(self.folder()), 11)
        self.assertIsNone(round11.declared_round(self.folder(round_json=False)))
        with self.assertRaisesRegex(ValueError, "does not declare"):
            round11.declared_round(self.folder(run_id="another"))

    def test_the_recordings_and_what_the_run_lacks(self):
        recorded = ("body-probes-face", "body-clip-compact-full-level")
        run = read(self.folder(recorded))
        self.assertEqual(sorted(run.documents), sorted(recorded))
        self.assertEqual(len(run.missing), 345)
        self.assertIn("body-probes-fingers", run.missing)
        self.assertEqual(run.version, "6000.6.9f1")

    def test_a_recorded_case_without_its_file_is_missing(self):
        folder = self.folder(("body-probes-face",))
        (folder / "replays" / "body" / "body-probes-face.pending.json").unlink()
        self.assertIn("body-probes-face", read(folder).missing)

    def test_another_bundle_is_refused(self):
        with self.assertRaisesRegex(ValueError, "another bundle"):
            read(self.folder(("body-probes-face",), bundle="body-muscles-compact"))

    def test_a_capture_is_its_own_metadatas_case(self):
        # Its file name never decides: a capture copied to another case of the same bundle, another run's, or
        # another Editor's is refused.
        clips = ("body-clip-compact-scoped-level", "body-clip-compact-scoped-level-offset")
        for identity, pattern in (((clips[1], "r", "6000.6.9f1"), "records another case"),
                                  ((clips[0], "other", "6000.6.9f1"), "another run"),
                                  ((clips[0], "r", "2022.3.62f3"), "another Editor version")):
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                read(self.folder(clips[:1], identity={clips[0]: identity}))
        self.assertEqual(sorted(read(self.folder(clips)).documents), sorted(clips))

    def test_a_sealed_case_is_listed_but_never_opened(self):
        # The default view is round 12's exploration one: B5a's face sweep is listed by name, and reading it is
        # refused before its file opens; a witness Avatar's clip opens.
        import anim_twist_witness as witness

        allowed = witness.allowlisted("compact")
        folder = self.folder(("body-probes-face", allowed))
        run = round11.Recordings(folder)
        self.assertEqual(sorted(run.documents), sorted(("body-probes-face", allowed)))
        self.assertEqual(run.view.read, {"run.json": run.view.read["run.json"]})
        with self.assertRaisesRegex(ValueError, "sealed"):
            run.documents["body-probes-face"]
        self.assertNotIn("replays/body/body-probes-face.pending.json", run.view.read)
        self.assertEqual(run.documents[allowed]["spec"]["evidence"]["editor"]["fixture_case"], allowed)
        # Another round 11 clip is no case of the allowlist at all.
        other = "body-clip-tall-scoped-level-offset"
        with self.assertRaisesRegex(ValueError, "not a case this view can open"):
            folder = self.folder((other,))
            read(folder, round12.round11_view(folder))


if __name__ == "__main__":
    unittest.main()
