"""B5a's round 11 inventory (anim_muscle_round11): the cases its confirmation requires, read beside freeze v8's
frozen Run, and a round 11 run's declaration."""

import json
import math
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_body_runs as runs  # noqa: E402
import anim_humanoid_qualify as qualify  # noqa: E402
import anim_muscle_pose as pose  # noqa: E402
import anim_muscle_round11 as round11  # noqa: E402
import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402
import test_anim_muscle_pose as pose_tests  # noqa: E402


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

    def test_a_capture_is_its_own_metadatas_case(self):
        # Its file name never decides: a capture copied to another case of the same bundle, another run's, or
        # another Editor's is refused.
        clips = ("body-clip-compact-scoped-level", "body-clip-compact-scoped-level-offset")
        for identity, pattern in (((clips[1], "r", "6000.6.9f1"), "records another case"),
                                  ((clips[0], "other", "6000.6.9f1"), "another run"),
                                  ((clips[0], "r", "2022.3.62f3"), "another Editor version")):
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                round11.Recordings(self.folder(clips[:1], identity={clips[0]: identity}))
        self.assertEqual(sorted(round11.Recordings(self.folder(clips)).documents), sorted(clips))


SPINE = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")
AXES = pose.synthetic_axes(7)
REST = [pose.f32(v) for v in pose._quaternion("muscle-pose/7/rest")]


def human(twist=0.):
    """An Avatar's human with the Spine alone mapped (node 0) on synthetic axes, its twist settings `twist`."""
    def xyzw(values):
        return dict(zip("xyzw", values))
    record = {"m_PreQ": xyzw(AXES["pre"]), "m_PostQ": xyzw(AXES["post"]), "m_Sgn": xyzw(AXES["sign"]),
              "m_Limit": {"m_Min": xyzw(AXES["min"]), "m_Max": xyzw(AXES["max"])}}
    return {"m_HumanBoneIndex": [-1] * 7 + [0] + [-1] * 47, "m_Skeleton": {"m_Node": [{"m_AxesId": 0}],
            "m_AxesArray": [record]}, **{field: twist for field in qualify.TWIST}}


def capture(case, samples=None, clip=False, run_id="r", version="6000.6.9f1", turn=0.):
    """A round 11 capture of `case` on human()'s rig, the Spine's recorded rotation the reference's in f32 (turned
    by `turn` radians), its full sample count unless `samples`: known vectors, or a clip's playback."""
    count = round11.expected_samples(case) if samples is None else samples
    names = [SPINE[m] if m < 3 else f"m{m}" for m in range(qualify.MUSCLE_COUNT)]
    keys = [dict(time_f32=replay.f32_bits(t), value_f32=replay.f32_bits(v), in_tangent_f32="00000000",
                 out_tangent_f32="00000000", weighted_mode="None") for t, v in ((0., -.25), (1., .5))]
    rows = []
    for k in range(count):
        time = k / max(count - 1, 1)
        if clip:
            vector, _ = pose.clip_values(dict(spec=dict(evidence=dict(editor=dict(bindings=[dict(
                path="", type="UnityEngine.Animator", property=SPINE[0], keys=keys)])))), dict(effective_source=time),
                names)
        else:
            vector = [pose.f32((k % 9 - 4) / 8) if m == 0 else 0. for m in range(qualify.MUSCLE_COUNT)]
        local = pose.bone_rotation(REST, AXES, [(vector[m], pose.MUSCLE_AXES[m]) for m in range(3)])
        recorded = pose.multiply(local, (math.sin(turn / 2), 0., 0., math.cos(turn / 2)))
        row = dict(local=time, effective_source=time, pose=dict(transforms=[
            dict(rotation=["00000000"] * 3 + ["3f800000"]), dict(rotation=pose.f32_hexes(recorded))]))
        if not clip:
            row["muscles_f32"] = pose.f32_hexes(vector)
        rows.append(row)
    bones = [dict(name="Spine" if b == 7 else f"b{b}", node=1 if b == 7 else -1) for b in range(55)]
    bindings = [dict(path="", type="UnityEngine.Animator", property=SPINE[0], keys=keys)] if clip else []
    return dict(spec=dict(mode="animator", clip_asset="c.anim" if clip else "", bundles=[dict(name=round11.CASES[case])],
                          evidence=dict(editor=dict(fixture_case=case, run_id=run_id, bindings=bindings))),
                capture=dict(editor_version=version, samples=rows, rest=dict(transforms=[
                    dict(rotation=["00000000"] * 3 + ["3f800000"]), dict(rotation=pose.f32_hexes(REST))]),
                    humanoid=dict(bones=bones, muscles=[dict(name=names[m], bone=qualify.muscle_bone(m),
                                                             axis=pose.MUSCLE_AXES[m])
                                                        for m in range(qualify.MUSCLE_COUNT)])))


class ConfirmTests(unittest.TestCase):
    """`confirm`, round 11 alone, on entries `capture_entries` makes from captures, as `cases` does."""

    @classmethod
    def setUpClass(cls):
        cls.entries = {}
        for editor in pose.EDITORS:
            for case in round11.CASES:
                cls.entries[editor, case] = cls.entries_of(editor, case, capture(case, clip=case in round11.FULL_CLIPS))

    @staticmethod
    def entries_of(editor, case, doc, confirmation=True, twist=0.):
        entries, _ = pose.capture_entries(editor, case, doc, human(twist), confirmation, {})
        return entries

    def document(self, editors=pose.EDITORS, replace=None):
        entries = {**self.entries, **(replace or {})}
        return dict(format=pose.FORMAT, kind="cases", reference=pose.REFERENCE, missing={},
                    entries=[entry for (editor, _), found in sorted(entries.items()) if editor in editors
                             for entry in found],
                    round11={editor: dict(runs=["r"], required=sorted(round11.CASES), missing=[])
                             for editor in editors})

    def test_a_complete_round_in_both_editors_passes(self):
        document = self.document()
        self.assertTrue(all(entry["confirmation"] is True and entry["refusal"] is False
                            for entry in document["entries"]))
        self.assertEqual(pose.confirm_failures(document), [])
        self.assertEqual(pose_tests.FailureTests.agree(self, document, "confirm"), (0, ""))
        # A sweep's 69 samples in rows that each count theirs; a clip's 9.
        face = self.entries["2022", "body-probes-face"][0]["vectors"]
        self.assertEqual((sum(row["samples"] for row in face), len(face)), (69, 9))
        # Acceptance never sees them.
        self.assertEqual(pose.agreement(document), {})

    def test_acceptance_alone_fails(self):
        acceptance = {key: self.entries_of(key[0], key[1], capture(key[1]), confirmation=False)
                      for key in (("2022", "body-probes-face"), ("6000", "body-probes-face"))}
        document = dict(self.document(editors=()), entries=[e for found in acceptance.values() for e in found])
        self.assertEqual(pose.confirm_failures(document), [
            "no declared round 11 run in Editor 2022", "no confirmation comparison in Editor 2022",
            "no declared round 11 run in Editor 6000", "no confirmation comparison in Editor 6000"])
        self.assertEqual(pose_tests.FailureTests.agree(self, document, "confirm")[0], 1)
        # Nor does acceptance stand in for a required case.
        document = self.document(replace={("6000", "body-probes-face"): acceptance["6000", "body-probes-face"]})
        self.assertEqual(pose.confirm_failures(document),
                         ["Editor 6000 compares 1 required case(s) short of their full sample count: body-probes-face"])

    def test_round_11_from_one_editor_fails(self):
        self.assertEqual(pose.confirm_failures(self.document(editors=("2022",))),
                         ["no declared round 11 run in Editor 6000", "no confirmation comparison in Editor 6000"])

    def test_a_truncated_or_short_round_fails(self):
        for case in ("body-probes-muscles-tall", "body-clip-tall-scoped-level-offset"):
            short = capture(case, round11.expected_samples(case) - 1, clip=case in round11.FULL_CLIPS)
            document = self.document(replace={("2022", case): self.entries_of("2022", case, short)})
            self.assertEqual(pose.confirm_failures(document), [
                f"Editor 2022 compares 1 required case(s) short of their full sample count: {case}"])
        document = self.document()
        document["round11"]["6000"]["required"] = sorted(round11.CASES)[1:]
        document["round11"]["2022"]["missing"] = ["body-probes-face"]
        self.assertEqual(len(pose.confirm_failures(document)), 2)

    def test_any_sample_outside_its_bound_fails(self):
        case = "body-clip-compact-scoped-level"
        turned = self.entries_of("6000", case, capture(case, clip=True, turn=math.radians(.5)))
        document = self.document(replace={("6000", case): turned})
        self.assertEqual(pose.confirm_failures(document), ["confirmation samples outside their bound: 6000 Spine in 9"])

    def test_scope_follows_the_runtime_per_case(self):
        # A twisted Avatar's Spine-only case is in scope, as the runtime admits it: its failure fails.
        case = "body-clip-compact-scoped-level"
        doc = capture(case, clip=True, turn=math.radians(.5))
        spine_only = self.entries_of("6000", case, doc, twist=.5)
        self.assertFalse(spine_only[0]["refusal"])
        # One that animates a twisted limb is refusal evidence: reported apart, never confirming its case.
        arm = next(m for m in range(qualify.MUSCLE_COUNT) if qualify.muscle_bone(m) in qualify.LIMBS["m_ArmTwist"])
        bindings = doc["spec"]["evidence"]["editor"]["bindings"]
        bindings.append(dict(bindings[0], property=f"m{arm}"))
        twisted = self.entries_of("6000", case, doc, twist=.5)
        self.assertEqual(twisted[0]["refusal_reason"], "unsupported_humanoid_twist")
        document = self.document(replace={("6000", case): twisted})
        self.assertEqual(pose.agreement(document, True)[("6000", "Spine")][3], 9)
        self.assertEqual(pose.confirm_failures(document),
                         [f"Editor 6000 compares 1 required case(s) short of their full sample count: {case}"])

    def test_duplicate_samples_keep_the_tightest_bound(self):
        case = "body-clip-compact-scoped-level"
        doc = capture(case, clip=True)
        doc["capture"]["samples"] = [doc["capture"]["samples"][0]] * 3
        errors = iter((1e-3, 1e-9, 1e-5))
        real = pose.clip_values

        def values(document, sample, names):
            vector, _ = real(document, sample, names)
            return vector, [next(errors)] * len(names)
        pose.clip_values = values
        try:
            (entry,) = self.entries_of("2022", case, doc)
        finally:
            pose.clip_values = real
        (row,) = entry["vectors"]
        self.assertEqual(row["samples"], 3)
        tight = pose.unity_bound(REST, AXES, [(v, a) for v, a in ((replay.f32_from_bits(v), a)
                                                                    for v, a in row["values"])], [1e-9] * 3)
        self.assertEqual(row["bound"], tight)


if __name__ == "__main__":
    unittest.main()
