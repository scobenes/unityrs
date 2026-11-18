"""The humanoid qualification evidence on a synthetic three-node rig: the slot inventory and crosswalk,
the root and hips-local comparisons, the candidate muscle-to-pose equation, and the refusal order the
Rust reader shares. All values are made up here; none come from an Editor run."""

import copy
import math
import sys
import unittest
from pathlib import Path
from zlib import crc32

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import anim_humanoid_qualify as q  # noqa: E402
import anim_replay as replay  # noqa: E402

BITS = replay.f32_bits
LIMIT = math.radians(40)
ROOT_X = [0.0, 0.5, 0.0]


def pose(translation, rotation=(0.0, 0.0, 0.0, 1.0)):
    return dict(translation=[BITS(x) for x in translation], rotation=[BITS(x) for x in rotation],
                scale=[BITS(1.0)] * 3)


def rig_avatar(twist=0.0):
    axes = dict(m_PreQ=dict(x=0.0, y=0.0, z=0.0, w=1.0), m_PostQ=dict(x=0.0, y=0.0, z=0.0, w=1.0),
                m_Sgn=dict(x=1.0, y=1.0, z=1.0), m_Type=1,
                m_Limit=dict(m_Min=dict(x=-LIMIT, y=-LIMIT, z=-LIMIT), m_Max=dict(x=LIMIT, y=LIMIT, z=LIMIT)))
    bones = [-1] * 25
    bones[0], bones[7] = 1, 2
    hand = dict(data=dict(m_HandBoneIndex=[-1] * 15))
    human = dict(m_HumanBoneIndex=bones, m_LeftHand=hand, m_RightHand=hand,
                 m_RootX=dict(t=dict(zip("xyz", ROOT_X))),
                 m_Skeleton=dict(data=dict(
                     m_Node=[dict(m_ParentId=-1, m_AxesId=-1), dict(m_ParentId=0, m_AxesId=0),
                             dict(m_ParentId=1, m_AxesId=1)],
                     m_ID=[0, crc32(b"Hips"), crc32(b"Hips/Spine")], m_AxesArray=[axes, axes])),
                 m_ArmTwist=twist, m_ForeArmTwist=0.0, m_UpperLegTwist=0.0, m_LegTwist=0.0,
                 m_ArmStretch=0.0, m_LegStretch=0.0, m_FeetSpacing=0.0, m_HasTDoF=False)
    return dict(m_Avatar=dict(m_Human=dict(data=human), m_RootMotionBoneIndex=-1),
                m_TOS=[[0, ""], [crc32(b"Hips"), "Hips"], [crc32(b"Hips/Spine"), "Hips/Spine"]])


def rig_clip(dofs=(42,), **settings):
    index = [-1] * 200
    for slot, dof in enumerate(dofs):
        index[dof] = slot
    muscle = dict(m_IndexArray=index, m_ValueArrayReferencePose=[],
                  m_Clip=dict(data=dict(m_StreamedClip=dict(curveCount=len(dofs), discreteCurveCount=0),
                                        m_DenseClip=dict(m_CurveCount=0),
                                        m_ConstantClip=dict(data={"t": "f32s", "v": ""}))))
    for name in q.SETTINGS:
        muscle[name] = 0.0 if name in ("m_CycleOffset", "m_Level", "m_OrientationOffsetY") else False
    muscle.update(settings)
    bindings = [dict(typeID=95, customType=8, attribute=dof) for dof in dofs]
    return dict(m_MuscleClip=muscle, m_ClipBindingConstant=dict(genericBindings=bindings))


def capture_humanoid():
    names = ["Hips"] + [f"Bone{i}" for i in range(1, 54)] + ["UpperChest"]
    names[7] = "Spine"
    bones = [dict(name=name, node={0: 1, 7: 2}.get(i, -1), required=i in q.REQUIRED_BONES)
             for i, name in enumerate(names)]
    muscles = [dict(name=f"Muscle {i}", bone=q.muscle_bone(i), axis=(2, 1, 0)[i % 3],
                    min=BITS(-40.0), max=BITS(40.0)) for i in range(q.MUSCLE_COUNT)]
    limits = [dict(human="Spine", defaults=True)]
    return dict(bones=bones, muscles=muscles, limits=limits)


def spine_rotation(value):
    record = dict(m_PreQ=(0.0, 0.0, 0.0, 1.0), m_PostQ=(0.0, 0.0, 0.0, 1.0), m_Sgn=(1.0, 1.0, 1.0),
                  m_Limit=dict(m_Min=(-LIMIT,) * 3, m_Max=(LIMIT,) * 3))
    return q.muscle_rotation(record, [(value, 2)])


def recordings(values=(0.0, 0.5)):
    """A clip replay and its known-vector baseline: the clip's body sits at the root origin."""
    spec = dict(mode="animator", binding_address="0", binding_component=0,
                nodes=[dict(address="0", name="rig"), dict(address="0/0", name="Hips"),
                       dict(address="0/0/0", name="Spine")],
                evidence=dict(editor=dict(bindings=[dict(property="Muscle 0")])),
                bundles=[dict(name="rig", sha256="0" * 64)], prefab_asset="p", clip_asset="c",
                avatar_asset="a", prefab={}, binding={},
                clip=dict(file_index=1, path_id="5"), avatar=dict(file_index=1, path_id="6"))
    rest = [pose((0, 0, 0)), pose((0, 1, 0)), pose((0, 0.2, 0))]

    def samples(baseline):
        rows = []
        for i, value in enumerate(values):
            hips = (0.0, 1.0 - 0.01 * value, 0.0)
            if not baseline:
                hips = tuple(a - b for a, b in zip(hips, ROOT_X))
            row = dict(local=float(i), local_f32=BITS(float(i)),
                       pose=dict(transforms=[pose((0, 0, 0)), pose(hips), pose((0, 0.2, 0), spine_rotation(value))]))
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


def qualify(clip=None, avatar=None, name="humanoid-synthetic", baseline=True):
    doc, known = recordings()
    clip = clip or rig_clip()
    # The Editor's curve names for the clip's own DOFs.
    names = [f"Muscle {dof - 42}" if q.dof_kind(dof)[0] == "muscle" else q.dof_kind(dof)[2]
             for dof in (row["attribute"] for row in clip["m_ClipBindingConstant"]["genericBindings"])
             if dof < 200]
    doc["spec"]["evidence"]["editor"]["bindings"] = [dict(property=name) for name in names]
    view = View(clip, avatar or rig_avatar())
    return q.qualify_clip(name, doc, known if baseline else None, view, None)


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


class QualificationTests(unittest.TestCase):
    def test_inventory_crosswalk_and_comparison(self):
        entry = qualify()
        mapped = [row for row in entry["slots"] if row["storage"] != "absent"]
        self.assertEqual(mapped, [dict(dof=42, kind="muscle", name="Muscle 0", index_value=0, storage="streamed")])
        self.assertTrue(all(row["index_value"] == -1 for row in entry["slots"] if row["storage"] == "absent"))
        self.assertEqual(entry["storage_counts"], dict(streamed=1, dense=0, constant=0))
        self.assertEqual([row["bone"] for row in entry["axes"]], [7])
        compared = entry["root_hips"]
        self.assertEqual(compared["reset"]["root"]["translation"], 0)
        self.assertAlmostEqual(compared["reset"]["hips_local"]["translation"], 0.505, places=6)
        self.assertAlmostEqual(compared["baseline"]["hips_local"]["translation"], 0.5, places=6)
        self.assertLess(compared["body_origin_translation"], 1e-7)
        self.assertLess(entry["muscle_pose"]["known_vectors"]["Spine"], 1e-4)
        self.assertFalse(entry["body_bearing"] or entry["qualifies"])
        self.assertEqual(entry["refusal"], "unsupported_body_motion(hips_correction)")
        self.assertTrue(any("hips_local differs from the baseline" in reason for reason in entry["reasons"]))

    def test_refusal_order_matches_the_rust_reader(self):
        cases = [
            (dict(clip=rig_clip((42, 7))), "unsupported_body_motion(body_slots)"),
            (dict(clip=rig_clip((0, 42))), "unsupported_locomotion"),
            (dict(clip=rig_clip((14,))), "unsupported_humanoid_ik"),
            (dict(clip=rig_clip((137,))), "unsupported_translation_dof"),
            (dict(avatar=rig_avatar(twist=0.5), clip=rig_clip(m_Level=0.125)),
             "unsupported_humanoid_twist(m_ArmTwist)"),
            (dict(clip=rig_clip(m_KeepOriginalPositionY=True, m_Level=0.125)),
             "unsupported_humanoid_clip_settings(m_Level)"),
            (dict(clip=rig_clip(m_StartAtOrigin=True)), "unsupported_humanoid_clip_settings(m_StartAtOrigin)"),
        ]
        for kwargs, expected in cases:
            with self.subTest(expected):
                self.assertEqual(qualify(**kwargs, baseline=False)["refusal"], expected)

    def test_a_probe_must_give_its_contracted_refusal(self):
        with self.assertRaisesRegex(ValueError, "differs from the probe's unsupported_humanoid_twist"):
            qualify(name="humanoid-twist-refusal", baseline=False)
        entry = qualify(name="humanoid-twist-refusal", avatar=rig_avatar(twist=0.25), baseline=False)
        self.assertEqual(entry["refusal"], "unsupported_humanoid_twist(m_ArmTwist)")

    def test_disagreeing_tables_are_refused(self):
        clip = rig_clip()
        clip["m_ClipBindingConstant"]["genericBindings"][0]["attribute"] = 43
        with self.assertRaisesRegex(ValueError, "disagrees with m_IndexArray"):
            qualify(clip=clip)
        clip = rig_clip()
        clip["m_MuscleClip"]["m_IndexArray"][43] = 0
        with self.assertRaisesRegex(ValueError, "share a slot|binding and slot counts"):
            qualify(clip=clip)
        avatar = rig_avatar()
        avatar["m_TOS"][2][1] = "Hips/Other"
        with self.assertRaisesRegex(ValueError, "maps to a different node"):
            qualify(avatar=avatar)
        avatar = rig_avatar()
        avatar["m_Avatar"]["m_Human"]["data"]["m_Skeleton"]["data"]["m_AxesArray"][1]["m_Limit"]["m_Max"]["z"] = 0.1
        with self.assertRaisesRegex(ValueError, "runtime limits"):
            qualify(avatar=avatar)


if __name__ == "__main__":
    unittest.main()
