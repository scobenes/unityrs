"""Round 11's full-body clips as Unity binds them, each clip's declared scope against freeze v8's limits
(anim_fixture_full, anim_fixture_scope), and B4's exact chain rule on the built Transforms of every clip B4
reads (anim_fixture_round11.serialized_chain)."""

import json
import re
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_body_support as support  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import anim_fixture_full as full  # noqa: E402
import anim_fixture_muscles as muscles  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_fixture_scope as scope  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_fixture_support import RUN  # noqa: E402
from test_anim_body_fixtures import source  # noqa: E402
from test_anim_fixture_round11 import (FREEZE, HUMANOID_SPINE, NAMES, Built, Round11CheckTests,  # noqa: E402
                                       full_clip, humanoid_spine, round11_docs)

check = Round11CheckTests.check


def doc_of(name, **kwargs):
    """A full-body clip case; an in-scope one's samples recorded at the rest pose (the synthetic poses follow
    the humanoid Spine curves, whose twist is out of scope)."""
    doc = support.body_doc(name, RUN)
    if r11.ROUND11_INFO[name].get("scope") == "in":
        rest = doc["capture"]["rest"]["transforms"]
        for sample in doc["capture"]["samples"]:
            sample["pose"]["transforms"] = [dict(row) for row in rest]
    return full_clip(doc, name, **kwargs)


class PropertyTests(unittest.TestCase):
    def test_finger_muscles_bind_under_unitys_animator_properties(self):
        # Unity binds a finger muscle's curve as `<Side>Hand.<Finger>.<rest>`, not under HumanTrait's name.
        self.assertEqual(full.muscle_property("Left Thumb 1 Stretched"), "LeftHand.Thumb.1 Stretched")
        self.assertEqual(full.muscle_property("Left Thumb Spread"), "LeftHand.Thumb.Spread")
        self.assertEqual(full.muscle_property("Right Index 3 Stretched"), "RightHand.Index.3 Stretched")
        self.assertEqual(full.muscle_property("Right Little Spread"), "RightHand.Little.Spread")
        fingers = [name for group in muscles.FINGER_GROUPS for name in group]
        properties = [full.muscle_property(name) for name in fingers]
        self.assertEqual(len(set(properties)), 40)
        self.assertTrue(all(re.fullmatch(r"(Left|Right)Hand\.(Thumb|Index|Middle|Ring|Little)\."
                                         r"(1 Stretched|2 Stretched|3 Stretched|Spread)", p) for p in properties))
        # A body muscle's property is its own name.
        for name in ("Spine Front-Back", "Left Arm Twist In-Out", "Right Hand In-Out", "Jaw Close",
                     "Left Eye Down-Up", "Right Shoulder Front-Back", "Left Upper Leg Twist In-Out"):
            self.assertEqual(full.muscle_property(name), name)

    def test_the_full_curves_are_distinct_and_exact(self):
        for scoped in (False, True):
            groups = full.full_groups(list(NAMES), scoped)
            curves = [group[0][3] for group in groups]
            self.assertEqual(len(groups), 95 if scoped else 92)
            # No two muscles share a curve, so a swapped channel changes the pose; each moves through three
            # or more values; each value is exact in f32.
            self.assertEqual(len(set(curves)), len(curves))
            self.assertTrue(all(len(set(curve)) >= 3 for curve in curves))
            self.assertTrue(all(replay.f32_from_bits(replay.f32_bits(v)) == v for curve in curves for v in curve))
            self.assertEqual({group[0][1] for group in groups},
                             {body.MUSCLE_ATTRIBUTE + i for i, name in enumerate(NAMES)
                              if scoped or name not in full.SPINE_MUSCLES})

    def test_in_scope_curves_stay_inside_the_frozen_lateral_limit(self):
        freeze = json.loads(FREEZE.read_text(encoding="utf-8"))
        self.assertEqual((scope.LATERAL_LIMIT, scope.TWIST_LIMIT),
                         (freeze["scope"]["lateral_limit"], freeze["bounds"]["twist_limit"]))
        values = {group[0][0]: group[0][3] for group in full.full_groups(list(NAMES), True)}
        lateral = [abs(v) for name in ("Spine Left-Right", "Chest Left-Right", "UpperChest Left-Right")
                   for v in values[name]]
        self.assertTrue(lateral and max(lateral) <= scope.LATERAL_LIMIT)
        # Every other muscle still moves: at least an eighth at some key, the twist muscles a 1/256.
        self.assertTrue(all(max(map(abs, curve)) >= 1 / 256 for curve in values.values()))
        self.assertTrue(all(max(map(abs, values[name])) >= .1875 for name in values
                            if not any(name in names for names, _ in full.SCOPED_AMPLITUDES)))
        # A refusal clip's Spine Left-Right, AuthorHumanoidClip's, is over it.
        self.assertIn("CombinedAxes = { -.25f, .375f, -.5f };", source("UnityrsHumanoidMotion.cs"))
        self.assertGreater(max(HUMANOID_SPINE["Spine Left-Right"]), scope.LATERAL_LIMIT)


class ScopeTests(unittest.TestCase):
    def test_accepts_clips_as_declared(self):
        with round11_docs():
            for name in ("body-clip-compact-full-keep-o0-y0-xz0", "body-clip-large-full-level-offset",
                         "body-clip-armature-large-full-" + body.CHAIN_VARIANT,
                         "body-clip-compact-scoped-orientation-offset-offset",
                         "body-clip-armature-scoped-" + body.CHAIN_VARIANT,
                         "body-clip-scaled-animator-scoped-" + body.CHAIN_VARIANT,
                         "body-clip-hips-scale-scoped-" + body.CHAIN_VARIANT,
                         "body-clip-no-upper-chest-toes-scoped-keep-o0-y0-xz0-offset",
                         "body-clip-noncoplanar-scoped-keep-o0-y0-xz0"):
                doc, clip, _ = doc_of(name)
                with self.subTest(name=name):
                    self.assertIsNotNone(check(self, name, doc, Built(name, doc, clip)))

    def test_spine_only_e5_clips_are_refusal_evidence(self):
        # E5's Spine-only clips and chain clips reach Spine Left-Right .375: refusal evidence, as declared.
        with round11_docs():
            for name in ("body-clip-large-keep-o0-y0-xz0-offset", "body-chain-hips-scale-small-clip"):
                self.assertEqual(r11.ROUND11_INFO[name]["scope"], "refusal")
                for spine in (True, False):
                    doc = support.body_doc(name, RUN)
                    clip = support.clip_case(doc, r11.ROUND11_INFO[name]["variant"])
                    if spine:
                        doc["spec"]["evidence"]["editor"]["bindings"] += humanoid_spine()
                    with self.subTest(name=name, spine=spine):
                        if spine:
                            self.assertIsNotNone(check(self, name, doc, Built(name, doc, clip)))
                        else:
                            with self.assertRaisesRegex(ValueError, "declared refusal evidence but"):
                                check(self, name, doc, Built(name, doc, clip))

    def test_refuses_an_in_scope_clip_over_the_lateral_limit(self):
        name = "body-clip-small-scoped-level"
        with round11_docs():
            doc, clip, _ = doc_of(name)
            row = next(row for row in doc["spec"]["evidence"]["editor"]["bindings"]
                       if row["property"] == "Chest Left-Right")
            row["keys"][3]["value_f32"] = replay.f32_bits(.25)
            with self.assertRaisesRegex(ValueError, "authored full-body Chest Left-Right curve mismatch"):
                check(self, name, doc, Built(name, doc, clip))
            # The declaration alone, on curves the design never had.
            with self.assertRaisesRegex(ValueError, "declared in scope but a spine Left-Right key is 0.25"):
                scope.check_scope(name, "in", doc, body._authored(doc["spec"]["evidence"]["editor"], replay), replay)

    def test_refuses_an_in_scope_clip_whose_recorded_shoulders_twist(self):
        name = "body-clip-compact-scoped-keep-o0-y0-xz0"
        with round11_docs():
            for degrees, ok in ((4, True), (4.5, False)):
                doc, clip, _ = doc_of(name)
                node = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "UpperChest")
                turned = [replay.f32_bits(v) for v in support.euler((0, degrees, 0), 4)]
                for sample in doc["capture"]["samples"][1:]:
                    sample["pose"]["transforms"][node]["rotation"] = list(turned)
                with self.subTest(degrees=degrees):
                    if ok:
                        self.assertIsNotNone(check(self, name, doc, Built(name, doc, clip)))
                    else:
                        with self.assertRaisesRegex(ValueError, "declared in scope but its shoulder-hip twist"):
                            check(self, name, doc, Built(name, doc, clip))

    def test_refuses_refusal_evidence_inside_the_scope(self):
        # A clip declared refusal evidence must be over the frozen lateral limit somewhere.
        name = "body-clip-compact-full-bake-o1-y1-xz1"
        with round11_docs():
            doc, clip, _ = doc_of(name)
            authored = body._authored(doc["spec"]["evidence"]["editor"], replay)
            scope.check_scope(name, "refusal", doc, authored, replay)
            for prop in ("Spine Left-Right", "Chest Left-Right", "UpperChest Left-Right"):
                for key in authored[prop]["keys"]:
                    key["value_f32"] = replay.f32_bits(.125)
            with self.assertRaisesRegex(ValueError, "declared refusal evidence but its spine Left-Right curves"):
                scope.check_scope(name, "refusal", doc, authored, replay)
            # And a clip with no spine Left-Right curve has no declared scope to check.
            doc, clip, _ = doc_of(name)
            bindings = doc["spec"]["evidence"]["editor"]["bindings"]
            bindings[:] = [row for row in bindings if row["property"] != "Chest Left-Right"]
            with self.assertRaisesRegex(ValueError, "authored full-body Chest Left-Right curve mismatch"):
                check(self, name, doc, Built(name, doc, clip))

    def test_refuses_a_finger_curve_under_its_humantrait_name(self):
        # The builder's earlier mistake: a finger curve under HumanTrait's name binds nothing in Unity.
        name = "body-clip-compact-scoped-level-offset"
        with round11_docs():
            doc, clip, _ = doc_of(name)
            for row in doc["spec"]["evidence"]["editor"]["bindings"]:
                if row["property"] == "LeftHand.Index.Spread":
                    row["property"] = "Left Index Spread"
            with self.assertRaisesRegex(ValueError, "authored full-body LeftHand.Index.Spread curve mismatch"):
                check(self, name, doc, Built(name, doc, clip))


class SerializedChainTests(unittest.TestCase):
    def test_unit_scale_below_the_hips_exactly(self):
        # B4 (chain.rs unit_below_hips): every node below the hips at scale one, bit for bit.
        with round11_docs():
            for name in ("body-clip-compact-scoped-keep-o0-y0-xz0", "body-clip-hips-scale-scoped-" + body.CHAIN_VARIANT):
                for bone in ("Spine", "LeftLowerLeg", "Left Thumb Distal"):
                    doc, clip, _ = doc_of(name)
                    view = Built(name, doc, clip)
                    address = next(node["address"] for node in doc["spec"]["nodes"] if node["name"] == bone)
                    row = view.at(doc, address)["m_LocalScale"]
                    row["y"] = replay.f32_from_bits(f"{int(replay.f32_bits(row['y']), 16) + 1:08x}")
                    with self.subTest(name=name, bone=bone), self.assertRaisesRegex(ValueError, "not one B4 admits"):
                        check(self, name, doc, view)

    def test_f5s_own_chain_clips_are_b4s_too(self):
        # body-chain-armature-clip, which B4 reads, gets the serialized check and B4's human scale as well.
        name = "body-chain-armature-clip"
        self.assertIn(name, r11.F5_CHAIN_CLIPS)
        with round11_docs():
            doc = support.body_doc(name, RUN)
            clip = support.clip_case(doc, body.CHAIN_VARIANT)
            self.assertIsNotNone(check(self, name, doc, Built(name, doc, clip)))
            for edit in ("rotation", "scale"):
                view = Built(name, doc, clip)
                if edit == "rotation":
                    row = view.at(doc, "0/0")["m_LocalRotation"]
                    row["x"] = replay.f32_from_bits(f"{int(replay.f32_bits(row['x']), 16) + 1:08x}")
                else:
                    view.human_scale = 2.
                pattern = "not one B4 admits" if edit == "rotation" else "outside B4's range"
                with self.subTest(edit=edit), self.assertRaisesRegex(ValueError, pattern):
                    check(self, name, doc, view)


if __name__ == "__main__":
    unittest.main()
