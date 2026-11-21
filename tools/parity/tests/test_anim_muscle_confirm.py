"""B5a's confirmation (anim_muscle_confirm, anim_muscle_clip): its expected set from round 11's builder, its
observed set from the raw captures, and why an incomplete, buggy or misrouted collection cannot pass."""

import contextlib
import copy
import io
import json
import math
import re
import struct
import sys
import unittest
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_fixture_body as body  # noqa: E402
import anim_fixture_full as full  # noqa: E402
import anim_fixture_muscles as muscles  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_fixture_round12 as round12  # noqa: E402
import anim_humanoid_qualify as qualify  # noqa: E402
import anim_muscle_clip as clips  # noqa: E402
import anim_muscle_confirm as confirm  # noqa: E402
import anim_muscle_pose as pose  # noqa: E402
import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402
from test_anim_body_fixtures import source  # noqa: E402

SWEEP, CLIP = "body-probes-face", "body-clip-compact-scoped-level"
VERSIONS = {"2022": "2022.3.62f3", "6000": "6000.6.9f1"}
BONES = confirm.bone_names()
SPINE, LEFT_EYE, RIGHT_EYE, LEFT_UPPER_ARM = 7, 21, 22, 13
NODES = 56  # the Avatar's skeleton: the root, then HumanBodyBones b at node b + 1, path `B<b>`
CAPTURED = 57  # the capture's nodes: the prefab root, the Animator node, then HumanBodyBones b at node b + 2
IDENTITY = dict(translation=["00000000"] * 3, rotation=["00000000"] * 3 + ["3f800000"], scale=["3f800000"] * 3)


def trait_names():
    """A HumanTrait muscle order: each ordinal takes the next builder muscle of its bone."""
    pool = {}
    for name, bone in confirm.builder_muscles().items():
        pool.setdefault(bone, []).append(name)
    return [pool[BONES[qualify.muscle_bone(m)]].pop(0) for m in range(qualify.MUSCLE_COUNT)]


NAMES = trait_names()
BY_PROPERTY = {full.muscle_property(name): name for name in NAMES}


def word(value):
    return struct.unpack(">I", struct.pack(">f", value))[0]


def avatar(twist=0.):
    """An Avatar mapping every bone at node b + 1 (path `B<b>`), each on its synthetic axes, its twist settings
    `twist`."""
    def xyzw(values):
        return dict(zip("xyzw", values))

    records = [{"m_PreQ": xyzw(a["pre"]), "m_PostQ": xyzw(a["post"]), "m_Sgn": xyzw(a["sign"]),
                "m_Limit": {"m_Min": xyzw(a["min"]), "m_Max": xyzw(a["max"])}}
               for a in map(pose.synthetic_axes, range(55))]
    human = {"m_HumanBoneIndex": [b + 1 for b in range(9)] + [55] + [b + 1 for b in range(9, 24)],
             "m_LeftHand": {"data": {"m_HandBoneIndex": [25 + k for k in range(15)]}},
             "m_RightHand": {"data": {"m_HandBoneIndex": [40 + k for k in range(15)]}},
             "m_Skeleton": {"m_Node": [{"m_AxesId": -1}, *({"m_AxesId": b} for b in range(55))],
                            "m_ID": list(range(NODES)), "m_AxesArray": records},
             "m_SkeletonPose": {"m_X": [{"q": dict(zip("xyzw", rest(n - 1) if n else (0, 0, 0, 1)))}
                                        for n in range(NODES)]},
             **{field: twist for field in qualify.TWIST}}
    return {"m_Avatar": {"m_Human": human}, "m_TOS": [[n, f"B{n - 1}" if n else ""] for n in range(NODES)]}


def rest(bone):
    return [pose.f32(v) for v in pose._quaternion(f"muscle-pose/{bone}/rest")]


def builder_curves(case=CLIP):
    _, rig, variant = confirm.b5a_cases()[case]
    return clips.builder_curves(NAMES, rig, variant)


def authored(curves):
    """The Editor's bindings of the builder's polylines, each tangent its segment's slope."""
    rows = []
    for prop, (times, values) in curves.items():
        slopes = [slope for *_, slope in clips.segments(times, values)]
        keys = [dict(time_f32=confirm.f32_bits(t), value_f32=confirm.f32_bits(v),
                     in_tangent_f32=confirm.f32_bits(slopes[max(k - 1, 0)]),
                     out_tangent_f32=confirm.f32_bits(slopes[min(k, len(slopes) - 1)]), weighted_mode="None")
                for k, (t, v) in enumerate(zip(times, values))]
        rows.append(dict(path="", type="UnityEngine.Animator", property=prop, keys=keys))
    return rows


def built(curves):
    """A built clip streaming the builder's polylines, one humanoid binding and slot per curve."""
    index, bindings, frames = [-1] * clips.DOF_COUNT, [], {}
    for slot, (prop, (times, values)) in enumerate(curves.items()):
        dof = 42 + NAMES.index(BY_PROPERTY[prop])
        index[dof] = slot
        bindings.append(dict(path=0, typeID=95, attribute=dof, customType=8, isPPtrCurve=0))
        for t0, _, v0, _, slope in clips.segments(times, values):
            frames.setdefault(t0, []).append((slot, (0., 0., slope, v0)))
        frames.setdefault(clips.f32(times[-1]), []).append((slot, (0., 0., 0., clips.f32(values[-1]))))
    words = []
    for time in sorted(frames):
        words += [word(time), len(frames[time])]
        for slot, coefficients in frames[time]:
            words += [slot, *map(word, coefficients)]
    return {"m_ClipBindingConstant": {"genericBindings": bindings},
            "m_MuscleClip": {**ROOT_SETTINGS, "m_IndexArray": index, "m_Clip": {"m_StreamedClip": dict(curveCount=len(bindings),
                                                                                       data=words),
                                                               "m_DenseClip": {}, "m_ConstantClip": {"data": []}}}}


# The level variant's root settings, as the built clip serializes them.
ROOT_SETTINGS = dict(m_StartAtOrigin=True, m_Mirror=False, m_LoopTime=False, m_LoopBlend=False, m_CycleOffset=0.,
                     m_LoopBlendOrientation=False, m_LoopBlendPositionY=False, m_LoopBlendPositionXZ=False,
                     m_KeepOriginalOrientation=False, m_KeepOriginalPositionY=False, m_KeepOriginalPositionXZ=False,
                     m_HeightFromFeet=False, m_Level=.125, m_OrientationOffsetY=0.)


def capture(case, editor, bones=None, turned=(), nodes=None):
    """`case`'s recording by Editor `editor`: every bone of its rig recorded (or `bones`), each from its own
    transform (or `nodes`' {bone: transform}), its rotation the reference's in f32, those in `turned` half a
    degree off."""
    kind, rig, _ = confirm.b5a_cases()[case]
    recorded = confirm.case_bones(rig) if bones is None else bones
    nodes = {bone: bone + 2 for bone in recorded} | (nodes or {})
    if kind == "probes":
        points = [(t, [pose.f32(v) for v in vector]) for t, vector in r11.probe_schedule(rig, NAMES, None, case)]
        bindings = []
    else:
        curves = builder_curves(case)
        tracks = clips.compiled_muscles(case, built(curves), curves, NAMES)
        points = [(t, clips.compiled_values(tracks, t)[0])
                  for t in confirm.clip_times(confirm.clip_keys(*confirm.b5a_cases()[case][1:]))]
        bindings = authored(curves)
    rows = []
    for t, vector in points:
        transforms = [dict(rotation=["00000000"] * 3 + ["3f800000"]) for _ in range(CAPTURED)]
        for bone in recorded:
            local = pose.bone_rotation(rest(bone), pose.synthetic_axes(bone),
                                       [(vector[m], pose.MUSCLE_AXES[m]) for m in pose.bone_muscles(bone)])
            if bone in turned:
                local = pose.multiply(local, (math.sin(math.radians(.25)), 0., 0., math.cos(math.radians(.25))))
            transforms[bone + 2] = dict(rotation=pose.f32_hexes(local))
        row = dict(local=t, effective_source=t, local_f32=confirm.f32_bits(t), pose=dict(transforms=transforms))
        if kind == "probes":
            row["muscles_f32"] = [confirm.f32_bits(v) for v in vector]
        rows.append(row)
    fixture = r11.ROUND11_CASES[case]
    folder = "assets/unityrsfixtures/" + fixture.folder + "/"
    mode = "human_pose" if kind == "probes" else "animator"
    return dict(
        spec=dict(mode=mode, prefab_asset=folder + fixture.prefab + ".prefab", clip_asset=folder + fixture.clip + ".anim",
                  avatar_asset=fixture.avatar, clip_name=fixture.clip, binding_address="0", binding_component=0,
                  nodes=[dict(address="", name=fixture.prefab), dict(address="0", name="HumanoidRig"),
                         *(dict(address=f"0/{n}", name=f"B{n}") for n in range(55))],
                  bundles=[dict(name=fixture.bundle)],
                  evidence=dict(editor=dict(fixture_case=case, run_id="r" + editor, bindings=bindings,
                                            animation_source="procedural_humanoid", expected_discovery=""))),
        capture=dict(editor_version=VERSIONS[editor], samples=rows, playback=dict(api=replay.API[mode],
                                                                                  clip_human_motion=True),
                     rest=dict(transforms=[IDENTITY, IDENTITY, *(dict(IDENTITY, rotation=pose.f32_hexes(rest(b)))
                                                                 for b in range(55))]), humanoid=dict(
                bones=[dict(name=name, node=nodes[b] if b in recorded else -1) for b, name in enumerate(BONES)],
                muscles=[dict(name=NAMES[m], bone=qualify.muscle_bone(m), axis=pose.MUSCLE_AXES[m])
                         for m in range(qualify.MUSCLE_COUNT)])))


OFF_KEY = "double offKey = keys[i - 1] - start + (keys[i] - keys[i - 1]) * (391.0 / 1024.0);"


def recorder_offset(text):
    """The off-key fraction in the recorder's Schedule, as its C# writes it."""
    body_text = text[text.index("static Timepoint[] Schedule("):]
    numerator, denominator = re.search(r"\* \((\d+(?:\.\d+)?) / (\d+(?:\.\d+)?)\);", body_text).groups()
    return float(numerator) / float(denominator)


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

    def run_confirm(self, replace=None, editors=VERSIONS, twist=0., clip=None, versions=None):
        """`observe` and `failures` on runs of `editors`, `replace` {(Editor, case): capture} replacing captures."""
        docs = {**self.docs, **(replace or {})}
        folders = [self.folder(editor, {case: doc for (e, case), doc in docs.items() if e == editor},
                               (versions or {}).get(editor)) for editor in editors]
        seen = confirm.observe(folders, lambda run, doc: avatar(twist),
                               lambda run, doc: clip or built(builder_curves()), self.CASES)
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

    def test_a_bone_read_from_another_transform_fails(self):
        doc = capture(SWEEP, "6000", nodes={LEFT_EYE: RIGHT_EYE + 2})
        with self.assertRaisesRegex(ValueError, r"records LeftEye from transform 24 \('B22'\), not the Avatar's "
                                                r"'B21'"):
            self.run_confirm({("6000", SWEEP): doc})

    def test_a_duplicated_sample_in_place_of_a_missing_one_fails(self):
        for case in self.CASES:
            samples = self.docs["2022", case]["capture"]["samples"]
            # A copy, as it is, repeats its sample.
            doc = copy.deepcopy(self.docs["2022", case])
            doc["capture"]["samples"][3] = copy.deepcopy(samples[2])
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, "records sample .* twice"):
                self.run_confirm({("2022", case): doc})
            # Relabelled with the missing sample's local time, it disagrees with its own source time.
            doc["capture"]["samples"][3]["local_f32"] = samples[3]["local_f32"]
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, "disagrees with its source time"):
                self.run_confirm({("2022", case): doc})
            # Relabelled at both, it is a new sample: the sweep's vector is not the builder's at that time, and the
            # clip's rotations are not those at the missing time.
            doc["capture"]["samples"][3]["effective_source"] = samples[3]["effective_source"]
            if case == SWEEP:
                with self.subTest(case=case), self.assertRaisesRegex(ValueError, "never scheduled"):
                    self.run_confirm({("2022", case): doc})
            else:
                found = self.run_confirm({("2022", case): doc})[1]
                self.assertEqual(len(found), 1)
                self.assertRegex(found[0], r"confirmation sample\(s\) outside their bound: .* sample 3eb0e000")
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

    def test_another_clip_fails(self):
        def edited(change):
            doc = copy.deepcopy(self.docs["2022", CLIP])
            change(doc)
            return {("2022", CLIP): doc}

        def keys(doc, prop="Spine Front-Back"):
            return next(row for row in doc["spec"]["evidence"]["editor"]["bindings"] if row["property"] == prop)["keys"]
        # A sibling clip or prefab of the same bundle filed under this case, by asset or by name.
        folder = "assets/unityrsfixtures/body-clips-compact-scoped/"
        for field, value, pattern in (
                ("clip_asset", folder + "body-clip-compact-scoped-keep-o0-y0-xz0.anim", "fixture identity mismatch"),
                ("prefab_asset", folder + "offset.prefab", "fixture identity mismatch"),
                ("clip_name", "body-clip-compact-scoped-level-x", "not the builder's 'body-clip-compact-scoped-level'")):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, pattern):
                self.run_confirm(edited(lambda doc: doc["spec"].update({field: value})))
        # Authored curves unlike the builder's: a value, an inflated tangent, a dropped curve.
        for change in (lambda doc: keys(doc)[2].update(value_f32=confirm.f32_bits(.4)),
                       lambda doc: keys(doc)[1].update(out_tangent_f32=confirm.f32_bits(8.)),
                       lambda doc: doc["spec"]["evidence"]["editor"]["bindings"].pop()):
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "authored|unlike the builder"):
                self.run_confirm(edited(change))

    def test_a_built_clip_unlike_the_builders_fails(self):
        curves = builder_curves()
        good = built(curves)

        def changed(change):
            clip = copy.deepcopy(good)
            change(clip)
            return clip
        stream = good["m_MuscleClip"]["m_Clip"]["m_StreamedClip"]["data"]

        def dropped(clip):
            row = clip["m_ClipBindingConstant"]["genericBindings"].pop()
            clip["m_MuscleClip"]["m_IndexArray"][row["attribute"]] = -1
        # The first frame's first slot: (slot, c3, c2, c1, c0) after the frame's time and count.
        cases = {"an inflated cubic term": (lambda clip: clip["m_MuscleClip"]["m_Clip"]["m_StreamedClip"]["data"]
                                            .__setitem__(4, word(3.)), "differs from the builder's"),
                 "a short index array": (lambda clip: clip["m_MuscleClip"]["m_IndexArray"].pop(), "200-DOF"),
                 "an entry past the storage": (lambda clip: clip["m_MuscleClip"]["m_IndexArray"].__setitem__(
                     42, 10 ** 6), "orphaned or conflicting|other muscles"),
                 "a dropped binding": (lambda clip: clip["m_ClipBindingConstant"]["genericBindings"].pop(),
                                       "no binding of theirs"),
                 "a dropped binding, its entry cleared": (dropped, "storage does not cover")}
        self.assertEqual(stream[2], 0)
        for label, (change, pattern) in cases.items():
            with self.subTest(label), self.assertRaisesRegex(ValueError, pattern):
                self.run_confirm(clip=changed(change))
        # The values and their errors come from the compiled curves.
        tracks = clips.compiled_muscles(CLIP, good, curves, NAMES)
        values, errors = clips.compiled_values(tracks, .345458984375)
        times, keyed = curves["Spine Front-Back"]
        spine = NAMES.index("Spine Front-Back")
        self.assertEqual(values[spine], clips.f32(keyed[1] + (keyed[2] - keyed[1]) * (.345458984375 - .25) / .25))
        self.assertGreater(errors[spine], 0)

    def test_the_placement_is_the_cases(self):
        # An offset case recorded on the identity prefab's placement fails, though clip, Avatar and paths agree.
        offset = CLIP + "-offset"
        info = r11.ROUND11_INFO[offset]
        self.assertEqual((info["root"], r11.ROUND11_INFO[CLIP]["root"]), ("offset", "identity"))
        doc = copy.deepcopy(self.docs["2022", CLIP])
        with self.assertRaisesRegex(ValueError, "another placement than its offset root"):
            confirm.placement(offset, info, doc)
        translation, rotation = r11.OFFSET_PLACEMENT
        doc["capture"]["rest"]["transforms"][1] = dict(IDENTITY, translation=pose.f32_hexes(translation),
                                                       rotation=pose.f32_hexes(rotation))
        confirm.placement(offset, info, doc)
        with self.assertRaisesRegex(ValueError, "another placement than its identity root"):
            self.run_confirm({("2022", CLIP): doc})

    def test_a_bone_filled_from_another_fails_by_its_rest(self):
        # Another bone's rows at this bone's transform: its rest rotation is not the Avatar's for this bone.
        doc = copy.deepcopy(self.docs["6000", SWEEP])
        rows = doc["capture"]["rest"]["transforms"]
        rows[LEFT_EYE + 2] = rows[RIGHT_EYE + 2]
        with self.assertRaisesRegex(ValueError, "records LeftEye at a rest rotation not the Avatar's"):
            self.run_confirm({("6000", SWEEP): doc})

    def test_the_variants_root_settings(self):
        # level and keep-o0-y0-xz0 share their muscle curves; the root settings tell them apart.
        for key, value in (("m_Level", 0.), ("m_KeepOriginalPositionY", True), ("m_LoopTime", True)):
            clip = built(builder_curves())
            clip["m_MuscleClip"][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "root settings|settings B4 reads"):
                self.run_confirm(clip=clip)

    def test_every_index_entry_is_owned(self):
        # As clip.rs: an entry other than -1 must be its own DOF's binding's slot, an unbound DOF's included.
        for dof, slot in ((199, 0), (7, 1), (42, -2)):
            clip = built(builder_curves())
            clip["m_MuscleClip"]["m_IndexArray"][dof] = slot
            with self.subTest(dof=dof), self.assertRaisesRegex(ValueError, "orphaned or conflicting|no binding of theirs"):
                self.run_confirm(clip=clip)

    def test_every_dense_frame_is_the_builders(self):
        times, values = builder_curves()["Spine Front-Back"]
        frames = [k / 60 for k in range(61)]

        def polyline(t):
            for (t0, v0), (t1, v1) in zip(zip(times, values), zip(times[1:], values[1:])):
                if t0 <= t <= t1:
                    return v0 + (t - t0) / (t1 - t0) * (v1 - v0)
        dense = [(t, (0., 0., 0., clips.f32(polyline(t)))) for t in frames]
        self.assertTrue(clips.check_track((False, dense), times, values))
        # One wrong frame near the 0.0955 off-key sample, between the keys and midpoints, fails.
        wrong = list(dense)
        wrong[6] = (frames[6], (0., 0., 0., dense[6][1][3] + .01))
        self.assertFalse(clips.check_track((False, wrong), times, values))
        # Frames that stop short of the last key fail too.
        self.assertFalse(clips.check_track((False, dense[:-1]), times, values))

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

    def test_the_recorder_schedule_is_the_recorders(self):
        # The off-key fraction is read from the recorder's C#, and each body clip starts at 0 and stops at 1.
        recording = source("UnityrsFixtureRecording.cs")
        self.assertEqual(recorder_offset(recording), confirm.RECORDER_OFFSET)
        self.assertIn("double start = keys[0], length = keys[keys.Length - 1] - start;", recording)
        # Each off-key sample is based at its interval's first key.
        self.assertIn(OFF_KEY, recording)
        self.assertNotIn(OFF_KEY, recording.replace("keys[i - 1] - start + (keys[i]", "keys[i] - start + (keys[i]"))
        self.assertIn("s.startTime == 0 && s.stopTime == 1", source("UnityrsBodyClips.cs"))
        # A drifted recorder fails the check, and its schedule is not the expected one.
        drifted = recording.replace("(391.0 / 1024.0)", "(400.0 / 1024.0)")
        self.assertNotEqual(recorder_offset(drifted), confirm.RECORDER_OFFSET)
        before = confirm.expected(self.CASES)
        with mock.patch.object(confirm, "RECORDER_OFFSET", recorder_offset(drifted)):
            self.assertNotEqual(confirm.expected(self.CASES), before)
            with self.assertRaisesRegex(ValueError, "never scheduled"):
                self.run_confirm()

    def test_the_editor_series_are_exact(self):
        self.assertEqual([confirm.editor_of(v) for v in ("2022.3.62f3", "6000.6.9f1")], ["2022", "6000"])
        for version in ("2022.2.21f1", "2022.1.0f1", "6000.0.30f1", "6000.60.1f1", "2023.3.0f1", None):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "frozen series"):
                confirm.editor_of(version)
        with self.assertRaisesRegex(ValueError, "frozen series"):
            self.run_confirm(versions={"2022": "2022.2.21f1"})
        # One Editor alone fails.
        self.assertEqual(self.run_confirm(editors=("2022",))[1][0], "no round 11 run of Editor 6000")

    def test_round_11s_own_confirmation_needs_no_view_and_round_12s_view_seals(self):
        # Round 11's standalone confirmation opens through round 11's own view by default (run_confirm injects
        # none); round 12's exploration view refuses B5a's sealed face sweep and an unreused clip.
        self.assertEqual(self.run_confirm()[1], [])
        folders = [self.folder(editor, {case: doc for (e, case), doc in self.docs.items() if e == editor})
                   for editor in VERSIONS]
        with self.assertRaisesRegex(ValueError, "sealed|not a case this view can open"):
            confirm.observe(folders, lambda run, doc: avatar(0.), lambda run, doc: built(builder_curves()), self.CASES,
                            round12.round11_view)
        with self.assertRaisesRegex(ValueError, "sealed"):
            confirm.observe(folders, None, None, {SWEEP: self.CASES[SWEEP]}, round12.round11_view)

    def test_a_round_10_run_is_refused(self):
        folder = self.folder("2022", {}, declared=False)
        with self.assertRaisesRegex(ValueError, "not a declared round 11 run"):
            confirm.observe([folder], None, None, self.CASES)

    def test_any_sample_outside_its_bound_fails(self):
        _, found = self.run_confirm({("6000", CLIP): capture(CLIP, "6000", turned={SPINE})})
        self.assertEqual(found, ["9 confirmation sample(s) outside their bound: " + ", ".join(
            f"6000 {CLIP} Spine sample {sid}" for sid in sorted(confirm.samples(CLIP, *self.CASES[CLIP])))])

    def test_scope_is_the_runtimes(self):
        human = avatar(.5)["m_Avatar"]["m_Human"]
        spine = {NAMES.index("Spine Front-Back")}
        arm = {NAMES.index("Left Arm Down-Up")}
        # On a twisted Avatar, a case moving only the Spine is in scope; one moving a twisted limb is not.
        self.assertIsNone(confirm.scope(human, spine))
        self.assertEqual(confirm.scope(human, spine | arm), "unsupported_humanoid_twist")
        self.assertIsNone(confirm.scope(avatar()["m_Avatar"]["m_Human"], spine | arm))
        # The full-body clip maps the arm: its samples are twist evidence, reported apart, and lacking in scope.
        seen, found = self.run_confirm(twist=.5)
        self.assertEqual({reason for *_, reason in seen.refused.values()}, {"unsupported_humanoid_twist"})
        self.assertTrue(any(key[1] == CLIP and key[2] == LEFT_UPPER_ARM for key in seen.refused))
        self.assertRegex(found[0], r"^the runs lack \d+ in-scope")
        # An unmapped required bone, or a moved bone without axes, is the runtime's refusal too.
        unmapped = avatar()["m_Avatar"]["m_Human"]
        unmapped["m_HumanBoneIndex"][1] = -1
        self.assertEqual(confirm.scope(unmapped, spine), "missing_human_bone")
        bare = avatar()["m_Avatar"]["m_Human"]
        bare["m_Skeleton"]["m_Node"][SPINE + 1]["m_AxesId"] = -1
        self.assertEqual(confirm.scope(bare, spine), "unsupported_humanoid_axes")
        self.assertIsNone(confirm.scope(bare, arm))

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
