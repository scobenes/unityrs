"""B5a's confirmation (anim_muscle_confirm): its expected set from round 11's builder, its observed set from the
raw captures, and why it cannot pass vacuously."""

import contextlib
import copy
import io
import json
import math
import sys
import unittest
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_fixture_body as body  # noqa: E402
import anim_fixture_muscles as muscles  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_humanoid_qualify as qualify  # noqa: E402
import anim_muscle_confirm as confirm  # noqa: E402
import anim_muscle_pose as pose  # noqa: E402
import scratch  # noqa: E402

SWEEP, CLIP = "body-probes-face", "body-clip-compact-scoped-level"
VERSIONS = {"2022": "2022.3.62f3", "6000": "6000.6.9f1"}
BONES = confirm.bone_names()
SPINE, LEFT_EYE, LEFT_UPPER_ARM = 7, 21, 13
NODES = 56  # the root, then HumanBodyBones b at node b + 1, in the capture and the Avatar alike


def trait_names():
    """A HumanTrait muscle order: each ordinal takes the next builder muscle of its bone."""
    pool = {}
    for name, bone in confirm.builder_muscles().items():
        pool.setdefault(bone, []).append(name)
    return [pool[BONES[qualify.muscle_bone(m)]].pop(0) for m in range(qualify.MUSCLE_COUNT)]


NAMES = trait_names()


def axes(bone):
    return pose.synthetic_axes(bone)


def human(twist=0.):
    """An Avatar mapping every bone, each on its synthetic axes, its twist settings `twist`."""
    def xyzw(values):
        return dict(zip("xyzw", values))

    records = [{"m_PreQ": xyzw(a["pre"]), "m_PostQ": xyzw(a["post"]), "m_Sgn": xyzw(a["sign"]),
                "m_Limit": {"m_Min": xyzw(a["min"]), "m_Max": xyzw(a["max"])}} for a in map(axes, range(55))]
    return {"m_HumanBoneIndex": [b + 1 for b in range(9)] + [55] + [b + 1 for b in range(9, 24)],
            "m_LeftHand": {"data": {"m_HandBoneIndex": [25 + k for k in range(15)]}},
            "m_RightHand": {"data": {"m_HandBoneIndex": [40 + k for k in range(15)]}},
            "m_Skeleton": {"m_Node": [{"m_AxesId": -1}, *({"m_AxesId": b} for b in range(55))],
                           "m_AxesArray": records},
            **{field: twist for field in qualify.TWIST}}


def rest(bone):
    return [pose.f32(v) for v in pose._quaternion(f"muscle-pose/{bone}/rest")]


def curve(name, values=(-.25, .5)):
    keys = [dict(time_f32=confirm.f32_bits(t), value_f32=confirm.f32_bits(v), in_tangent_f32="00000000",
                 out_tangent_f32="00000000", weighted_mode="None") for t, v in zip((0., 1.), values)]
    return dict(path="", type="UnityEngine.Animator", property=pose.muscle_property(name), keys=keys)


CURVED = ("Spine Front-Back", "Left Arm Down-Up")


def capture(case, editor, curved=CURVED, bones=None, turned=()):
    """`case`'s recording by Editor `editor`: every bone of its rig recorded (or `bones`), each one's rotation the
    reference's in f32, those in `turned` half a degree off."""
    kind, rig, variant = confirm.b5a_cases()[case]
    recorded = confirm.case_bones(rig) if bones is None else bones
    rows = []
    if kind == "probes":
        points = [(t, [pose.f32(v) for v in vector], [0.] * len(NAMES))
                  for t, vector in r11.probe_schedule(rig, NAMES, None, case)]
    else:
        bindings = [curve(name) for name in curved]
        doc = dict(spec=dict(evidence=dict(editor=dict(bindings=bindings))))
        points = [(t, *pose.clip_values(doc, dict(effective_source=t), NAMES))
                  for t in confirm.clip_times(confirm.clip_keys(rig, variant))]
    for t, vector, _ in points:
        transforms = [dict(rotation=["00000000"] * 3 + ["3f800000"]) for _ in range(NODES)]
        for bone in recorded:
            local = pose.bone_rotation(rest(bone), axes(bone), [(vector[m], pose.MUSCLE_AXES[m])
                                                                for m in pose.bone_muscles(bone)])
            if bone in turned:
                local = pose.multiply(local, (math.sin(math.radians(.25)), 0., 0., math.cos(math.radians(.25))))
            transforms[bone + 1] = dict(rotation=pose.f32_hexes(local))
        row = dict(local=t, effective_source=t, local_f32=confirm.f32_bits(t), pose=dict(transforms=transforms))
        if kind == "probes":
            row["muscles_f32"] = [confirm.f32_bits(v) for v in vector]
        rows.append(row)
    return dict(
        spec=dict(mode="animator", clip_asset="" if kind == "probes" else "c.anim",
                  bundles=[dict(name=r11.ROUND11_CASES[case].bundle)],
                  evidence=dict(editor=dict(fixture_case=case, run_id="r" + editor,
                                            bindings=[] if kind == "probes" else bindings))),
        capture=dict(editor_version=VERSIONS[editor], samples=rows, rest=dict(transforms=[
            dict(rotation=pose.f32_hexes(rest(n - 1)) if n else ["00000000"] * 3 + ["3f800000"])
            for n in range(NODES)]), humanoid=dict(
                bones=[dict(name=name, node=b + 1 if b in recorded else -1) for b, name in enumerate(BONES)],
                muscles=[dict(name=NAMES[m], bone=qualify.muscle_bone(m), axis=pose.MUSCLE_AXES[m])
                         for m in range(qualify.MUSCLE_COUNT)])))


def index_of(curved=CURVED):
    """A built clip's m_IndexArray compiling the muscles `curved`."""
    index = [-1] * 200
    for slot, name in enumerate(curved):
        index[42 + NAMES.index(name)] = slot
    return index


class ConfirmTests(unittest.TestCase):
    CASES = {case: confirm.b5a_cases()[case] for case in (SWEEP, CLIP)}

    @classmethod
    def setUpClass(cls):
        cls.docs = {(editor, case): capture(case, editor) for editor in VERSIONS for case in cls.CASES}

    def folder(self, editor, docs, version=None, declared=True):
        """A round 11 run of Editor `editor`, recording every case and holding the captures `docs`."""
        folder = scratch.for_case(self)
        names = sorted(body.BODY_CASES.keys() | r11.ROUND11_CASES.keys())
        (folder / "run.json").write_text(json.dumps(dict(
            run=dict(run_id="r" + editor, editor_version=version or VERSIONS[editor]),
            cases=[dict(name=name, status="recorded") for name in names])), encoding="utf-8")
        if declared:
            (folder / "round.json").write_text(json.dumps(dict(format="unityrs.fixture_round", round=11,
                                                               run_id="r" + editor)), encoding="utf-8")
        (folder / "replays" / "body").mkdir(parents=True)
        for case, doc in docs.items():
            (folder / "replays" / "body" / f"{case}.pending.json").write_text(json.dumps(doc), encoding="utf-8")
        return folder

    def run_confirm(self, replace=None, editors=VERSIONS, twist=0., index=None, versions=None):
        """`observe` and `failures` on runs of `editors`, `replace` {(Editor, case): capture} replacing captures."""
        docs = {**self.docs, **(replace or {})}
        folders = [self.folder(editor, {case: doc for (e, case), doc in docs.items() if e == editor},
                               (versions or {}).get(editor)) for editor in editors]
        seen = confirm.observe(folders, lambda run, doc: {"m_Avatar": {"m_Human": human(twist)}},
                               lambda run, doc: {"m_MuscleClip": {"m_IndexArray": index or index_of()}}, self.CASES)
        return seen, confirm.failures(seen, confirm.expected(self.CASES))

    def test_a_complete_round_in_both_editors_passes(self):
        seen, found = self.run_confirm()
        self.assertEqual(found, [])
        self.assertEqual(seen.rows.keys(), confirm.expected(self.CASES))
        self.assertEqual(seen.refused, {})
        # The face sweep's every bone at each of its 69 vectors, the clip's at its 9 times, in both Editors.
        face, clip = (len(confirm.case_bones(self.CASES[case][1])) for case in (SWEEP, CLIP))
        self.assertEqual(len(seen.rows), 2 * (face * 69 + clip * 9))

    def test_the_builders_frozen_counts(self):
        # The plan's numbers, now the builder's: 4 sweeps and 343 full-body clips; 535, 503, 423 and 69 sweep
        # vectors; 9 samples per clip. The checker computes them, never copies them.
        cases = confirm.b5a_cases()
        self.assertEqual(len(cases), 347)
        counts = {case: len(confirm.samples(case, *spec)) for case, spec in cases.items()}
        self.assertEqual({case: counts[case] for case in muscles.MUSCLE_SWEEPS},
                         {"body-probes-muscles-compact": 535, "body-probes-muscles-tall": 503,
                          "body-probes-fingers": 423, "body-probes-face": 69})
        self.assertEqual({count for case, count in counts.items() if case not in muscles.MUSCLE_SWEEPS}, {9})

    def test_spine_only_coverage_fails(self):
        spine = {(editor, case): capture(case, editor, bones={SPINE}) for editor in VERSIONS for case in self.CASES}
        _, found = self.run_confirm(spine)
        self.assertEqual(len(found), 1)
        self.assertRegex(found[0], r"^the runs lack \d+ in-scope")

    def test_one_missing_bone_in_one_case_fails(self):
        bones = confirm.case_bones(self.CASES[SWEEP][1]) - {LEFT_EYE}
        _, found = self.run_confirm({("6000", SWEEP): capture(SWEEP, "6000", bones=bones)})
        self.assertEqual(len(found), 1)
        self.assertRegex(found[0], r"^the runs lack 69 in-scope .*: 6000 body-probes-face LeftEye sample")

    def test_a_duplicated_sample_in_place_of_a_missing_one_fails(self):
        for case in self.CASES:
            doc = copy.deepcopy(self.docs["2022", case])
            doc["capture"]["samples"][3] = doc["capture"]["samples"][2]
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, "records sample .* twice"):
                self.run_confirm({("2022", case): doc})
        # Short of one, it lacks that sample on every bone.
        doc = copy.deepcopy(self.docs["2022", CLIP])
        del doc["capture"]["samples"][3]
        _, found = self.run_confirm({("2022", CLIP): doc})
        self.assertRegex(found[0], r"^the runs lack \d+ in-scope")

    def test_a_sample_the_builder_never_scheduled_fails(self):
        doc = copy.deepcopy(self.docs["6000", SWEEP])
        doc["capture"]["samples"][5]["muscles_f32"][0] = confirm.f32_bits(.3)
        with self.assertRaisesRegex(ValueError, "never scheduled"):
            self.run_confirm({("6000", SWEEP): doc})

    def test_a_forged_case_file_cannot_pass(self):
        # confirm reads run folders, never a case file: one claiming both Editors and every sample is refused as
        # an argument, and beside an empty run it confirms nothing.
        forged = scratch.for_case(self) / "cases.json"
        forged.write_text(json.dumps(dict(format=pose.FORMAT, kind="cases", reference=pose.REFERENCE,
                                          round11={e: dict(runs=["r"], missing=[]) for e in VERSIONS},
                                          entries=[])), encoding="utf-8")
        code, message = self.main([str(forged)])
        self.assertEqual(code, 1)
        self.assertIn("confirm reads no case file", message)
        folders = [self.folder(editor, {}) for editor in VERSIONS]
        (folders[0] / "cases.json").write_text(forged.read_text(encoding="utf-8"), encoding="utf-8")
        seen = confirm.observe(folders, None, None, self.CASES)
        found = confirm.failures(seen, confirm.expected(self.CASES))
        # Each Editor's run lacks both cases, and nothing in scope is compared.
        self.assertEqual(len(found), 5, found)
        self.assertRegex(found[-1], r"^the runs lack \d+ in-scope")

    def test_a_builder_change_changes_the_expected_set(self):
        before = confirm.expected(self.CASES)
        with mock.patch.object(muscles, "MUSCLE_VALUES", muscles.MUSCLE_VALUES[:-2]):
            fewer = confirm.expected(self.CASES)
            # The recordings of the old builder hold vectors the changed one never schedules.
            with self.assertRaisesRegex(ValueError, "never scheduled"):
                self.run_confirm()
        self.assertLess({sid for _, case, _, sid in fewer if case == SWEEP},
                        {sid for _, case, _, sid in before if case == SWEEP})
        with mock.patch.object(body, "BODY_TIMES", (0, .125, .25, .5, .75, 1)):
            more = confirm.expected(self.CASES)
            # The old clip's off-key samples are not the new schedule's.
            with self.assertRaisesRegex(ValueError, "never scheduled"):
                self.run_confirm()
        self.assertEqual(len({sid for _, case, _, sid in more if case == CLIP}), 11)

    def test_the_editor_series_are_exact(self):
        self.assertEqual([confirm.editor_of(v) for v in ("2022.3.62f3", "6000.6.9f1")], ["2022", "6000"])
        for version in ("2022.2.21f1", "2022.1.0f1", "6000.0.30f1", "6000.60.1f1", "2023.3.0f1", None):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "frozen series"):
                confirm.editor_of(version)
        with self.assertRaisesRegex(ValueError, "frozen series"):
            self.run_confirm(versions={"2022": "2022.2.21f1"})
        # One Editor alone fails.
        self.assertEqual(self.run_confirm(editors=("2022",))[1][0], "no round 11 run of Editor 6000")

    def test_a_round_10_run_is_refused(self):
        folder = self.folder("2022", {}, declared=False)
        with self.assertRaisesRegex(ValueError, "not a declared round 11 run"):
            confirm.observe([folder], None, None, self.CASES)

    def test_any_sample_outside_its_bound_fails(self):
        _, found = self.run_confirm({("6000", CLIP): capture(CLIP, "6000", turned={SPINE})})
        self.assertEqual(found, ["9 confirmation sample(s) outside their bound: " + ", ".join(
            f"6000 {CLIP} Spine sample {sid}" for sid in sorted(confirm.samples(CLIP, *self.CASES[CLIP])))])

    def test_scope_is_the_compiled_clips(self):
        # On a twisted Avatar, an authored arm curve the built clip never compiled leaves the case in scope, as
        # the runtime admits it.
        self.assertIsNone(confirm.scope(human(.5), index_of(CURVED[:1]), None))
        seen, found = self.run_confirm(twist=.5, index=index_of(CURVED[:1]))
        self.assertEqual((found, seen.refused), ([], {}))
        # A compiled arm slot makes it twist evidence: reported apart, and its required samples are lacking.
        seen, found = self.run_confirm(twist=.5)
        self.assertEqual({reason for *_, reason in seen.refused.values()}, {"unsupported_humanoid_twist"})
        self.assertTrue(any(key[1] == CLIP and key[2] == LEFT_UPPER_ARM for key in seen.refused))
        self.assertRegex(found[0], r"^the runs lack \d+ in-scope")
        # An unmapped required bone, or a compiled bone without axes, is the runtime's refusal too.
        unmapped = human()
        unmapped["m_HumanBoneIndex"][1] = -1
        self.assertEqual(confirm.scope(unmapped, index_of(), None), "missing_human_bone")
        bare = human()
        bare["m_Skeleton"]["m_Node"][SPINE + 1]["m_AxesId"] = -1
        self.assertEqual(confirm.scope(bare, index_of(), None), "unsupported_humanoid_axes")
        self.assertIsNone(confirm.scope(bare, index_of(CURVED[1:]), None))

    def main(self, argv):
        """`anim_muscle_confirm.py`'s exit status and stderr on `argv`."""
        saved, err = sys.argv, io.StringIO()
        sys.argv = ["anim_muscle_confirm.py", "--unityrs", "unityrs", *argv]
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
                confirm.main()
            code = 0
        except SystemExit as exit:
            code = exit.code
        finally:
            sys.argv = saved
        return code, err.getvalue()


if __name__ == "__main__":
    unittest.main()
