"""The humanoid qualification evidence on a synthetic rig of the fifteen required bones: the slot inventory and
crosswalk, the rest-track bounds on the root and hips, the candidate muscle-to-pose equation, the three
verdicts and the unordered causes, the full clip set, and the case lists the Rust check shares. All values are
made up here; none come from an Editor run."""

import copy
import json
import math
import re
import sys
import unittest
from pathlib import Path
from zlib import crc32

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import anim_humanoid_qualify as q  # noqa: E402
import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402

BITS = replay.f32_bits
LIMIT = math.radians(40)
BODY = [0.0, 0.5, 0.0]
# HumanBodyBones index and name of each rig bone; every bone but Hips is the Hips' child.
BONES = [(0, "Hips"), (1, "LeftUpperLeg"), (2, "RightUpperLeg"), (3, "LeftLowerLeg"), (4, "RightLowerLeg"),
         (5, "LeftFoot"), (6, "RightFoot"), (7, "Spine"), (10, "Head"), (13, "LeftUpperArm"),
         (14, "RightUpperArm"), (15, "LeftLowerArm"), (16, "RightLowerArm"), (17, "LeftHand"), (18, "RightHand")]
SPINE_NODE = 1 + [bone for bone, _ in BONES].index(7)
NEUTRAL = dict.fromkeys(q.TWIST + q.AVATAR_SETTINGS, 0.0)
# Left Arm Twist In-Out (LeftUpperArm) and an UpperChest muscle the rig leaves out.
ARM_TWIST, UPPER_CHEST = 42 + 41, 42 + 6


def path(i):
    return "Hips" if i == 0 else "Hips/" + BONES[i][1]


def pose(translation, rotation=(0.0, 0.0, 0.0, 1.0)):
    return dict(translation=[BITS(x) for x in translation], rotation=[BITS(x) for x in rotation],
                scale=[BITS(1.0)] * 3)


def rig_avatar(twist=0.0, root_x=BODY):
    axes = dict(m_PreQ=dict(x=0.0, y=0.0, z=0.0, w=1.0), m_PostQ=dict(x=0.0, y=0.0, z=0.0, w=1.0),
                m_Sgn=dict(x=1.0, y=1.0, z=1.0), m_Type=1,
                m_Limit=dict(m_Min=dict(x=-LIMIT, y=-LIMIT, z=-LIMIT), m_Max=dict(x=LIMIT, y=LIMIT, z=LIMIT)))
    bones = [-1] * 25
    for node, (bone, _) in enumerate(BONES, 1):
        bones[bone if bone <= 8 else bone + 1] = node
    hand = dict(data=dict(m_HandBoneIndex=[-1] * 15))
    nodes = [dict(m_ParentId=-1, m_AxesId=-1)] + [dict(m_ParentId=0 if i == 0 else 1, m_AxesId=i)
                                                   for i in range(len(BONES))]
    ids = [0] + [crc32(path(i).encode()) for i in range(len(BONES))]
    human = dict(m_HumanBoneIndex=bones, m_LeftHand=hand, m_RightHand=hand,
                 m_RootX=dict(t=dict(zip("xyz", root_x))),
                 m_Skeleton=dict(data=dict(m_Node=nodes, m_ID=ids, m_AxesArray=[axes] * len(BONES))),
                 m_ArmTwist=twist, m_ForeArmTwist=0.0, m_UpperLegTwist=0.0, m_LegTwist=0.0,
                 m_ArmStretch=0.0, m_LegStretch=0.0, m_FeetSpacing=0.0, m_HasTDoF=False)
    return dict(m_Avatar=dict(m_Human=dict(data=human), m_RootMotionBoneIndex=-1),
                m_TOS=[[0, ""]] + [[crc32(path(i).encode()), path(i)] for i in range(len(BONES))])


def rig_clip(dofs=(42,), writers=(), **settings):
    index = [-1] * 200
    for slot, dof in enumerate(dofs):
        index[dof] = slot
    constants = 3 * len(writers)
    muscle = dict(m_IndexArray=index, m_ValueArrayReferencePose=[],
                  m_Clip=dict(data=dict(m_StreamedClip=dict(curveCount=len(dofs), discreteCurveCount=0),
                                        m_DenseClip=dict(m_CurveCount=0),
                                        m_ConstantClip=dict(data={"t": "f32s", "v": "00000000" * constants}))))
    for name in q.SETTINGS:
        muscle[name] = 0.0 if name in ("m_CycleOffset", "m_Level", "m_OrientationOffsetY") else False
    muscle["m_StartAtOrigin"] = True
    muscle.update(settings)
    bindings = [dict(typeID=95, customType=8, attribute=dof, path=0) for dof in dofs]
    bindings += [dict(typeID=4, customType=0, attribute=1, path=crc32(name.encode())) for name in writers]
    return dict(m_MuscleClip=muscle, m_ClipBindingConstant=dict(genericBindings=bindings))


def capture_humanoid():
    names = ["Hips"] + [f"Bone{i}" for i in range(1, 54)] + ["UpperChest"]
    nodes = {}
    for node, (bone, name) in enumerate(BONES, 1):
        names[bone] = name
        nodes[bone] = node + 1
    bones = [dict(name=name, node=nodes.get(i, -1), required=i in q.REQUIRED_BONES) for i, name in enumerate(names)]
    muscles = [dict(name=f"Muscle {i}", bone=q.muscle_bone(i), axis=(2, 1, 0)[i % 3],
                    min=BITS(-40.0), max=BITS(40.0)) for i in range(q.MUSCLE_COUNT)]
    return dict(bones=bones, muscles=muscles, limits=[dict(human="Spine", defaults=True)])


def spine_rotation(value):
    record = dict(m_PreQ=(0.0, 0.0, 0.0, 1.0), m_PostQ=(0.0, 0.0, 0.0, 1.0), m_Sgn=(1.0, 1.0, 1.0),
                  m_Limit=dict(m_Min=(-LIMIT,) * 3, m_Max=(LIMIT,) * 3))
    return q.muscle_rotation(record, [(value, 2)])


def recordings(values=(0.0, 0.5), moved=True, root=(0.0, 0.0, 0.0), animator=(0.0, 0.0, 0.0)):
    """A clip replay and its known-vector baseline. A moved clip has its body at the root origin; the
    Animator node rests at `animator` and plays at `root` from there."""
    nodes = [dict(address="", name="fixture"), dict(address="0", name="rig"), dict(address="0/0", name="Hips")]
    nodes += [dict(address=f"0/0/{i}", name=name) for i, (_, name) in enumerate(BONES[1:])]
    spec = dict(mode="animator", binding_address="0", binding_component=0, nodes=nodes,
                evidence=dict(editor=dict(bindings=[])),
                bundles=[dict(name="rig", sha256="0" * 64)], prefab_asset="p", clip_asset="c",
                avatar_asset="a", prefab={}, binding={},
                clip=dict(file_index=1, path_id="5"), avatar=dict(file_index=1, path_id="6"))
    rest = [pose((0, 0, 0)), pose(animator), pose((0, 1, 0))] + [pose((0, 0.2, 0)) for _ in BONES[1:]]

    def samples(baseline):
        rows = []
        for i, value in enumerate(values):
            transforms = copy.deepcopy(rest)
            transforms[1] = pose(tuple(a + b for a, b in zip(animator, root)))
            hips = (0.0, 1.0 - 0.01 * value, 0.0) if moved else (0.0, 1.0, 0.0)
            if moved and not baseline:
                hips = tuple(a - b for a, b in zip(hips, BODY))
            transforms[2] = pose(hips)
            transforms[1 + SPINE_NODE] = pose((0, 0.2, 0), spine_rotation(value))
            row = dict(local=float(i), local_f32=BITS(float(i)), pose=dict(transforms=transforms))
            if baseline:
                row["muscles_f32"] = [BITS(value)] + [BITS(0.0)] * (q.MUSCLE_COUNT - 1)
            rows.append(row)
        return rows

    capture = dict(editor_version="2022.3.0f1", bundle_sha256=["0" * 64], humanoid=capture_humanoid(),
                   playback=dict(clip_human_motion=True, api=replay.API["animator"]),
                   rest=dict(transforms=rest), samples=samples(False))
    doc = dict(format="unityrs.anim_replay", version=1, spec=spec, capture=capture)
    baseline = copy.deepcopy(doc)
    baseline["spec"]["mode"] = "human_pose"
    baseline["capture"]["playback"]["api"] = replay.API["human_pose"]
    baseline["capture"]["samples"] = samples(True)
    return doc, baseline


class View:
    def __init__(self, clip, avatar):
        self.objects = {(1, 5): clip, (1, 6): avatar}

    def value(self, key):
        return self.objects[key]


def qualify(clip=None, avatar=None, name="humanoid-synthetic", baseline=True, edit=None, **recorded):
    doc, known = recordings(**recorded)
    if edit:
        edit(known)
    clip = clip or rig_clip()
    # The Editor's Animator curve names for the clip's own DOFs.
    names = [f"Muscle {row['attribute'] - 42}" if q.dof_kind(row["attribute"])[0] == "muscle"
             else q.dof_kind(row["attribute"])[2]
             for row in clip["m_ClipBindingConstant"]["genericBindings"] if row["typeID"] == 95]
    doc["spec"]["evidence"]["editor"]["bindings"] = [dict(property=name) for name in names]
    view = View(clip, avatar or rig_avatar())
    return q.qualify_clip(name, doc, known if baseline else None, view, NEUTRAL)


class LayoutTests(unittest.TestCase):
    def test_dof_kinds_and_labels(self):
        self.assertEqual(q.dof_kind(0), ("motion", 0, "MotionT.x"))
        self.assertEqual(q.dof_kind(13), ("body", 6, "RootQ.w"))
        self.assertEqual(q.dof_kind(41), ("goal", 27, "RightHandQ.w"))
        self.assertEqual(q.dof_kind(42)[:2], ("muscle", 0))
        self.assertEqual(q.dof_kind(199), ("translation", 62, "TDoF20.z"))
        with self.assertRaises(ValueError):
            q.dof_kind(200)

    def test_muscle_bones_match_the_rust_table(self):
        # crates/unityrs-anim/tests/humanoid.rs pins the same points.
        for muscle, bone in [(0, 7), (6, 54), (41, 13), (55, 24), (56, 24), (94, 53)]:
            self.assertEqual(q.muscle_bone(muscle), bone)

    def test_candidate_rotation_uses_the_limit_on_each_side(self):
        record = dict(m_PreQ=(0.0, 0.0, 0.0, 1.0), m_PostQ=(0.0, 0.0, 0.0, 1.0), m_Sgn=(1.0, 1.0, -1.0),
                      m_Limit=dict(m_Min=(-0.2,) * 3, m_Max=(0.4,) * 3))
        # z has sign -1: m = -0.5 gives d = 0.5 and the max limit, 0.2 rad.
        rotation = q.muscle_rotation(record, [(-0.5, 2)])
        self.assertAlmostEqual(q.angle_degrees(rotation, (0, 0, math.sin(0.1), math.cos(0.1))), 0, places=9)
        # x has sign 1: m = -0.5 gives d = -0.5 and the min limit, -0.1 rad.
        rotation = q.muscle_rotation(record, [(-0.5, 0)])
        self.assertAlmostEqual(q.angle_degrees(rotation, (math.sin(-0.05), 0, 0, math.cos(0.05))), 0, places=9)
        self.assertEqual(q.muscle_rotation(record, []), (0.0, 0.0, 0.0, 1.0))

    def test_generic_rows_take_their_widths(self):
        rows, width = q.binding_rows([dict(typeID=95, customType=8, attribute=42),
                                      dict(typeID=4, customType=0, attribute=2), dict(typeID=137, customType=20,
                                                                                      attribute=7)])
        self.assertEqual([(row["first"], row["width"], row["role"]) for row in rows],
                         [(0, 1, "human"), (1, 4, "transform"), (5, 1, "other")])
        self.assertEqual(width, 6)


class VerdictTests(unittest.TestCase):
    def test_a_clip_within_the_rest_bounds_qualifies(self):
        entry = qualify(moved=False, avatar=rig_avatar(root_x=(0.0, 0.0, 0.0)))
        self.assertEqual(entry["causes"], [])
        self.assertTrue(entry["qualifies"] and entry["verdicts"]["residual"] == dict(root=True, hips_local=True))
        self.assertEqual(entry["verdict"], "qualified_pending_playback")
        self.assertTrue(entry["root_hips"]["baseline_within"])
        self.assertEqual(entry["verdicts"]["clip_settings"] + entry["verdicts"]["root_basis"], [])
        # The same clip with no known-vector baseline does not.
        self.assertFalse(qualify(moved=False, baseline=False)["qualifies"])

    def test_a_clip_over_the_hips_bound_needs_a_correction(self):
        entry = qualify()
        mapped = [row for row in entry["slots"] if row["storage"] != "absent"]
        self.assertEqual(mapped, [dict(dof=42, kind="muscle", name="Muscle 0", index_value=0, storage="streamed")])
        self.assertTrue(all(row["index_value"] == -1 for row in entry["slots"] if row["storage"] == "absent"))
        reset = entry["root_hips"]["reset"]
        self.assertTrue(reset["root"]["within"])
        self.assertFalse(reset["hips_local"]["within"])
        self.assertAlmostEqual(reset["hips_local"]["translation"], 0.505, places=6)
        # The hips' rest bound is half an f32 ulp at 1, widened.
        self.assertLess(entry["root_hips"]["bounds"]["hips_local"]["translation"][1], 2**-23)
        self.assertLess(entry["root_hips"]["body_origin_translation"], 1e-7)
        self.assertLess(entry["muscle_pose"]["known_vectors"]["Spine"], 1e-4)
        self.assertEqual(entry["causes"], ["unsupported_body_motion(hips_correction)"])
        self.assertFalse(entry["qualifies"] or entry["body_bearing"])

    def test_root_motion_is_a_root_projection(self):
        entry = qualify(moved=False, root=(0.0, 0.0, 0.25), avatar=rig_avatar(root_x=(0.0, 0.0, 0.0)))
        self.assertEqual(entry["causes"], ["unsupported_body_motion(root_projection)"])

    def test_the_three_verdicts_are_independent(self):
        entry = qualify(clip=rig_clip(m_Level=0.125, m_KeepOriginalPositionY=True))
        self.assertEqual(entry["verdicts"]["clip_settings"], ["m_Level"])
        self.assertEqual(entry["verdicts"]["root_basis"], ["m_KeepOriginalPositionY"])
        self.assertEqual(entry["verdicts"]["residual"], dict(root=True, hips_local=False))
        self.assertEqual(entry["causes"], ["unsupported_body_motion(hips_correction)",
                                           "unsupported_humanoid_clip_settings(m_Level)",
                                           "unsupported_humanoid_root_basis(m_KeepOriginalPositionY)"])
        # Each basis flag off its neutral profile is listed.
        self.assertEqual(qualify(clip=rig_clip(m_StartAtOrigin=False))["verdicts"]["root_basis"], ["m_StartAtOrigin"])
        self.assertEqual(qualify(clip=rig_clip(m_KeepOriginalPositionXZ=True, m_StartAtOrigin=False))
                         ["verdicts"]["root_basis"], ["m_KeepOriginalPositionXZ", "m_StartAtOrigin"])


class BaselineTests(unittest.TestCase):
    def still(self, **kwargs):
        return qualify(moved=False, avatar=rig_avatar(root_x=(0.0, 0.0, 0.0)), **kwargs)

    def test_a_baseline_beyond_its_bound_does_not_qualify(self):
        def lift(known):
            for row in known["capture"]["samples"]:
                row["pose"]["transforms"][2] = pose((0.0, 1.25, 0.0))
        entry = self.still(edit=lift)
        self.assertEqual(entry["causes"], [])
        self.assertFalse(entry["qualifies"] or entry["root_hips"]["baseline_within"])
        self.assertEqual(entry["verdict"], "refused")
        self.assertTrue(any("known-vector baseline differs" in reason for reason in entry["reasons"]))

    def test_a_baseline_that_moves_its_root_does_not_qualify(self):
        def drift(known):
            for row in known["capture"]["samples"]:
                row["pose"]["transforms"][1] = pose((0.0, 0.0, 0.5))
        entry = self.still(edit=drift)
        self.assertFalse(entry["root_hips"]["baseline_root_at_rest"] or entry["qualifies"])

    def test_a_baseline_of_another_prefab_or_rig_is_refused(self):
        for edit, message in [
            (lambda known: known["spec"].update(prefab_asset="other"), "another prefab"),
            (lambda known: known["spec"].update(binding_component=1), "another prefab"),
            (lambda known: known["spec"]["nodes"][2].update(name="Pelvis"), "another reset rig"),
            (lambda known: known["capture"]["rest"]["transforms"].__setitem__(2, pose((0, 2, 0))),
             "another reset rig"),
        ]:
            with self.subTest(message), self.assertRaisesRegex(ValueError, message):
                self.still(edit=edit)

    def test_a_neutral_basis_matrix_clip_qualifies_on_its_reset_baseline(self):
        neutral = "humanoid-root-basis-o0-y0-xz0"
        entry = self.still(name=neutral, baseline=False)
        self.assertEqual((entry["causes"], entry["verdict"]), ([], "qualified_pending_playback"))
        self.assertEqual(entry["root_hips"]["baseline_kind"], "reset")
        # Over its hips bound, the same clip is refused by its residual alone.
        entry = qualify(name=neutral, baseline=False)
        self.assertEqual(entry["causes"], ["unsupported_body_motion(hips_correction)"])
        self.assertEqual(entry["verdict"], "refused")
        # Any other clip without a known-vector baseline does not qualify.
        self.assertEqual(self.still(baseline=False)["verdict"], "refused")


class CauseTests(unittest.TestCase):
    def causes(self, **kwargs):
        return qualify(baseline=False, **kwargs)["causes"]

    def test_each_cause(self):
        for kwargs, expected in [
            (dict(clip=rig_clip((42, 7))), "unsupported_body_motion(body_slots)"),
            (dict(clip=rig_clip((0, 42))), "unsupported_locomotion"),
            (dict(clip=rig_clip((14,))), "unsupported_humanoid_ik"),
            (dict(clip=rig_clip((137,))), "unsupported_translation_dof"),
            (dict(clip=rig_clip(writers=("Hips/Spine",))), "conflicting_humanoid_writer(1)"),
            (dict(animator=(2.0, 0.25, -1.0)), "unsupported_animator_root"),
        ]:
            with self.subTest(expected):
                self.assertIn(expected, self.causes(**kwargs))

    def test_twist_acts_only_on_its_limb(self):
        twist = rig_avatar(twist=0.5)
        self.assertNotIn("unsupported_humanoid_twist(m_ArmTwist)", self.causes(avatar=twist))
        self.assertIn("unsupported_humanoid_twist(m_ArmTwist)",
                      self.causes(avatar=twist, clip=rig_clip((42, ARM_TWIST))))

    def test_a_limb_twist_muscle_is_a_cause_at_any_twist_setting(self):
        # URS-42: the runtime refuses every limb-twist curve, the neutral twist setting's included.
        cause = f"unsupported_humanoid_limb_twist({ARM_TWIST - 42})"
        self.assertNotIn(cause, self.causes())
        self.assertIn(cause, self.causes(clip=rig_clip((42, ARM_TWIST))))
        self.assertIn(cause, self.causes(avatar=rig_avatar(twist=0.5), clip=rig_clip((42, ARM_TWIST))))
        self.assertFalse([c for c in self.causes(clip=rig_clip((42, 42 + 2))) if "limb_twist" in c])

    def test_a_generic_writer_off_the_skeleton_is_not_a_conflict(self):
        entry = qualify(baseline=False, clip=rig_clip(writers=("Hips/Prop",)))
        self.assertEqual(entry["generic_writers"], [])

    def test_an_absent_optional_bone_drops_its_muscle(self):
        entry = qualify(baseline=False, clip=rig_clip((42, UPPER_CHEST)))
        self.assertEqual(entry["dropped_muscles"], dict(count=1, names=["Muscle 6"]))
        self.assertFalse([cause for cause in entry["causes"] if cause.startswith("missing_human_bone")])

    def test_a_probe_must_list_its_contracted_cause(self):
        with self.assertRaisesRegex(ValueError, "lack the probe's unsupported_humanoid_twist"):
            qualify(name="humanoid-twist-refusal", avatar=rig_avatar(twist=0.25), baseline=False)
        entry = qualify(name="humanoid-twist-refusal", avatar=rig_avatar(twist=0.25), baseline=False,
                        clip=rig_clip((42, ARM_TWIST)))
        self.assertIn("unsupported_humanoid_twist(m_ArmTwist)", entry["causes"])

    def test_disagreeing_tables_are_refused(self):
        clip = rig_clip()
        clip["m_ClipBindingConstant"]["genericBindings"][0]["attribute"] = 43
        with self.assertRaisesRegex(ValueError, "disagrees with m_IndexArray"):
            qualify(clip=clip)
        clip = rig_clip()
        clip["m_MuscleClip"]["m_IndexArray"][43] = 0
        with self.assertRaisesRegex(ValueError, "share a slot"):
            qualify(clip=clip)
        clip = rig_clip()
        clip["m_MuscleClip"]["m_Clip"]["data"]["m_StreamedClip"]["curveCount"] = 2
        with self.assertRaisesRegex(ValueError, "binding widths and slot counts differ"):
            qualify(clip=clip)
        avatar = rig_avatar()
        avatar["m_TOS"][1 + SPINE_NODE - 1][1] = "Hips/Other"
        with self.assertRaisesRegex(ValueError, "maps to a different node"):
            qualify(avatar=avatar)
        avatar = rig_avatar()
        skeleton = avatar["m_Avatar"]["m_Human"]["data"]["m_Skeleton"]["data"]
        skeleton["m_AxesArray"] = copy.deepcopy(skeleton["m_AxesArray"])
        skeleton["m_AxesArray"][SPINE_NODE - 1]["m_Limit"]["m_Max"]["z"] = 0.1
        with self.assertRaisesRegex(ValueError, "runtime limits"):
            qualify(avatar=avatar)


class ClipSetTests(unittest.TestCase):
    def test_the_expected_set_is_the_required_clips_and_the_recorded_coverage(self):
        rows = [dict(name=name, status="recorded", reason="") for name in q.REQUIRED_CASES]
        rows.append(dict(name="humanoid-twist-refusal", status="recorded", reason=""))
        rows.append(dict(name="humanoid-non-neutral-setting-refusal", status="not_covered", reason="old run"))
        self.assertEqual(q.expected_cases(dict(cases=rows)), [*q.REQUIRED_CASES, "humanoid-twist-refusal"])

    def test_a_missing_replay_fails(self):
        folder = scratch.for_case(self)
        rows = [dict(name=name, status="recorded", reason="") for name in q.REQUIRED_CASES]
        run = dict(run_id="0" * 32, editor_version="2022.3.0f1", build_target="StandaloneWindows64",
                   build_target_id=19)
        (folder / "run.json").write_text(json.dumps(dict(format="unityrs.fixture_run", version=1, run=run,
                                                         cases=rows)), encoding="utf-8")
        (folder / "replays").mkdir()
        with self.assertRaisesRegex(ValueError, "missing completed replay"):
            q.qualify_folder(folder, "unityrs")

    def test_the_rust_check_shares_the_case_lists(self):
        source = (HERE.parent.parent / "crates/unityrs-export/tests/anim_replay/humanoid.rs").read_text(
            encoding="utf-8")

        def listed(name):
            block = re.search(rf"const {name}: &\[&str\] = &\[(.*?)\];", source, re.S)
            self.assertIsNotNone(block, name)
            return re.findall(r'"([^"]+)"', block.group(1))

        self.assertEqual(listed("REQUIRED"), list(q.REQUIRED_CASES))
        self.assertEqual(listed("OPTIONAL"), sorted(set(q.humanoid_cases()) - set(q.REQUIRED_CASES)))


if __name__ == "__main__":
    unittest.main()
