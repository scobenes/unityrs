"""Round 11's full-body clips as Unity binds them and their expected-absent muscles, each clip's declared scope
against freeze v8's limits on its authored keys, compiled values and recorded poses (anim_fixture_full,
anim_fixture_scope), and B4's inputs on every clip B4 reads: the exact chain rule on the built Transforms, the
Avatar skeleton's Transforms through m_TOS, and the clip settings root.rs reads (anim_fixture_round11)."""

import copy
import itertools
import json
import math
import re
import unittest
import zlib

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_body_support as support  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import anim_fixture_full as full  # noqa: E402
import anim_fixture_muscles as muscles  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_fixture_scope as scope  # noqa: E402
import anim_humanoid_qualify as qualify  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_body_runs as runs  # noqa: E402
from anim_fixture_support import RUN  # noqa: E402
from test_anim_body_fixtures import source  # noqa: E402
from test_anim_fixture_round11 import (FREEZE, HUMANOID_SPINE, NAMES, Built, Round11CheckTests,  # noqa: E402
                                       full_clip, round11_docs, spine_clip)

check = Round11CheckTests.check


def doc_of(name):
    """A full-body clip case; an in-scope one's samples recorded at the rest pose (the synthetic poses follow
    the humanoid Spine curves, whose twist is out of scope)."""
    doc = support.body_doc(name, RUN)
    if r11.ROUND11_INFO[name].get("scope") == "in":
        rest = doc["capture"]["rest"]["transforms"]
        for sample in doc["capture"]["samples"]:
            sample["pose"]["transforms"] = [dict(row) for row in rest]
    return full_clip(doc, name)


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
            groups = full.full_groups(list(NAMES), "compact-scoped" if scoped else "compact-full")
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
        values = {group[0][0]: group[0][3] for group in full.full_groups(list(NAMES), "compact-scoped")}
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


    def test_every_scoped_clip_turns_the_spine_well_over_the_recorders_threshold(self):
        """The recorder (CheckHumanoidPlayback) and the pinned completion (anim_fixture_contract.check_spine)
        each need the Spine to turn more than 1 degree from 0 to .5 and from .5 to 1. Every in-scope clip's
        authored Spine turns 2 degrees or more on its own rig's limits, whichever sign each axis has (the angle
        between two rotations does not depend on the pre and post rotations)."""
        clips = source("UnityrsBodyClips.cs")
        self.assertIn('("Spine Front-Back", new[] { -.3125f, .125f, .1875f, -.0625f, -.125f }),', clips)
        self.assertEqual(full.SCOPED_KEYS, {"Spine Front-Back": (-.3125, .125, .1875, -.0625, -.125)})
        self.assertIn("min = new Vector3(-18, -32, -12), max = new Vector3(42, 14, 26),",
                      source("UnityrsHumanoidFixture.cs"))
        self.assertEqual(full.TALL_SPINE_LIMITS, ((-18, -32, -12), (42, 14, 26)))
        self.assertEqual({full.spine_limits(rig) for rig in full.SCOPED_RIGS},
                         {full.DEFAULT_SPINE_LIMITS, full.TALL_SPINE_LIMITS})
        spine = ("Spine Twist Left-Right", "Spine Left-Right", "Spine Front-Back")  # axes x, y, z
        identity = (0., 0., 0., 1.)

        def turn(rig, a, b, signs):
            low, high = full.spine_limits(rig)
            axes = {"m_Sgn": signs, "m_Limit": {"m_Min": [math.radians(v) for v in low],
                                                 "m_Max": [math.radians(v) for v in high]},
                    "m_PreQ": identity, "m_PostQ": identity}
            p, q = (qualify.muscle_rotation(axes, [(muscles_at[name], axis) for axis, name in enumerate(spine)])
                    for muscles_at in (a, b))
            return math.degrees(2 * math.acos(min(1., abs(sum(x * y for x, y in zip(p, q))))))

        least = {}
        for rig in full.SCOPED_RIGS:
            keys = [full.authored_muscles(list(NAMES), rig, time) for time in (0, .5, 1)]
            least[rig] = min(turn(rig, a, b, signs) for a, b in zip(keys, keys[1:])
                             for signs in itertools.product((1., -1.), repeat=3))
            with self.subTest(rig=rig):
                self.assertGreaterEqual(least[rig], 2.)
        # The tall Spine's narrow limits are the least turn; before the zero crossing it was under 1 degree.
        self.assertEqual(min(least, key=least.get).split("-")[0], "tall")
        before = dict(full.authored_muscles(list(NAMES), "tall-scoped", 1))
        mid = dict(full.authored_muscles(list(NAMES), "tall-scoped", .5), **{"Spine Front-Back": -.1875})
        self.assertLess(turn("tall-scoped", mid, before, (1., 1., 1.)), 1.)


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
                         "body-clip-noncoplanar-scoped-keep-o0-y0-xz0",
                         "body-clip-compact-no-face-scoped-keep-o0-y0-xz0",
                         "body-clip-compact-eyes-scoped-bake-o1-y1-xz1-offset",
                         "body-clip-tall-jaw-scoped-bake-o1-y1-xz1"):
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
                    variant = r11.ROUND11_INFO[name]["variant"]
                    clip = spine_clip(doc, variant) if spine else support.clip_case(doc, variant)
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
                scope.check_scope(name, "in", doc, body._authored(doc["spec"]["evidence"]["editor"], replay), clip,
                                  replay)

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
            scope.check_scope(name, "refusal", doc, authored, clip, replay)
            for prop in ("Spine Left-Right", "Chest Left-Right", "UpperChest Left-Right"):
                for key in authored[prop]["keys"]:
                    key["value_f32"] = replay.f32_bits(.125)
            with self.assertRaisesRegex(ValueError, "declared refusal evidence but its spine Left-Right curves"):
                scope.check_scope(name, "refusal", doc, authored, clip, replay)
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



class ExpectedAbsentTests(unittest.TestCase):
    def test_a_topology_leaves_its_omitted_bones_muscles_uncurved(self):
        absent = {rig: full.expected_absent(rig, NAMES) for rig in full.SCOPED_RIGS}
        shoulders = {f"{side} Shoulder {axis}" for side in ("Left", "Right") for axis in ("Down-Up", "Front-Back")}
        upper = {"UpperChest Front-Back", "UpperChest Left-Right", "UpperChest Twist Left-Right"}
        toes = {"Left Toes Up-Down", "Right Toes Up-Down"}
        self.assertEqual(absent["no-shoulders-scoped"], shoulders)
        self.assertEqual(absent["tall-scoped"], upper)
        self.assertEqual(absent["no-upper-chest-toes-scoped"], upper | toes)
        self.assertEqual(absent["no-left-toes-scoped"], {"Left Toes Up-Down"})
        for rig in ("compact-scoped", "large-scoped", "armature-large-scoped", "hips-scale-small-scoped",
                    "noncoplanar-scoped"):
            self.assertEqual(absent[rig], frozenset())
        for rig, names in absent.items():
            self.assertEqual(len(full.full_groups(list(NAMES), rig)), 95 - len(names))

    def test_refuses_a_curve_on_an_expected_absent_muscle(self):
        with round11_docs():
            for name in ("body-clip-no-shoulders-scoped-level", "body-clip-tall-scoped-bake-o1-y0-xz1-offset",
                         "body-clip-no-upper-chest-toes-scoped-keep-o1-y1-xz1-motion"):
                doc, clip, _ = doc_of(name)
                with self.subTest(name=name):
                    self.assertIsNotNone(check(self, name, doc, Built(name, doc, clip)))
            name = "body-clip-no-shoulders-scoped-level"
            doc, clip, _ = doc_of(name)
            doc["spec"]["evidence"]["editor"]["bindings"].append(
                support.animator("Left Shoulder Front-Back", support.keys(body.BODY_TIMES, (0, .1, 0, .1, 0))))
            with self.assertRaisesRegex(ValueError, "curves expected-absent muscle"):
                check(self, name, doc, Built(name, doc, clip))


class CompiledScopeTests(unittest.TestCase):
    def test_refuses_an_in_scope_clip_whose_compiled_values_cross_the_limit(self):
        # The authored keys inside the limit, the built clip's Chest Left-Right over it at a sample time.
        name = "body-clip-large-scoped-orientation-offset"
        with round11_docs():
            doc, clip, groups = doc_of(name)
            authored = body._authored(doc["spec"]["evidence"]["editor"], replay)
            scope.check_scope(name, "in", doc, authored, clip, replay)
            over = [[(prop, attribute, times, tuple(.25 if prop == "Chest Left-Right" and k == 2 else v
                                                    for k, v in enumerate(values)))]
                    for (prop, attribute, times, values), in groups]
            built = support.clip_case(copy.deepcopy(doc), r11.ROUND11_INFO[name]["variant"], over)
            with self.assertRaisesRegex(ValueError, "its compiled spine Left-Right value at 0.5 is 0.25"):
                scope.check_scope(name, "in", doc, authored, built, replay)

    def test_refusal_evidence_must_cross_the_limit_as_compiled(self):
        # Authored over the limit, but a built clip without the Spine's slots plays it at 0 throughout.
        name = "body-clip-tall-keep-o0-y0-xz0-start-offset"
        with round11_docs():
            doc = support.body_doc(name, RUN)
            clip = spine_clip(doc, r11.ROUND11_INFO[name]["variant"])
            authored = body._authored(doc["spec"]["evidence"]["editor"], replay)
            scope.check_scope(name, "refusal", doc, authored, clip, replay)
            bare = support.clip_case(copy.deepcopy(doc), r11.ROUND11_INFO[name]["variant"])
            with self.assertRaisesRegex(ValueError, "compiled spine Left-Right values stay within"):
                scope.check_scope(name, "refusal", doc, authored, bare, replay)

    def test_a_malformed_lateral_slot_refuses(self):
        # A muscle the clip does not curve (m_IndexArray -1, no binding) plays 0; any other unreadable slot is an
        # error, a -1 beside the attribute's compiled binding (orphaned, which clip.rs refuses) too.
        name = "body-clip-large-scoped-orientation-offset"
        attribute = body.MUSCLE_ATTRIBUTE + 4  # Chest Left-Right
        with round11_docs():
            doc, clip, _ = doc_of(name)
            authored = body._authored(doc["spec"]["evidence"]["editor"], replay)
            index = clip["m_MuscleClip"]["m_IndexArray"]
            other = next(slot for k, slot in enumerate(index) if slot >= 0 and k != attribute)
            for edit, pattern in ((lambda index: index.__setitem__(attribute, -1), "orphaned binding"),
                                  (lambda index: index.__setitem__(attribute, other), "is malformed"),
                                  (lambda index: index.__setitem__(attribute, len(index) + 999), "is malformed"),
                                  (lambda index: index.__setitem__(attribute, "1"), "no entry"),
                                  (lambda index: index.__delitem__(slice(attribute, None)), "no entry")):
                broken = copy.deepcopy(clip)
                edit(broken["m_MuscleClip"]["m_IndexArray"])
                with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                    scope.check_scope(name, "in", doc, authored, broken, replay)
            del clip["m_MuscleClip"]["m_IndexArray"]
            with self.assertRaisesRegex(ValueError, "no entry"):
                scope.check_scope(name, "in", doc, authored, clip, replay)


class B4InputTests(unittest.TestCase):
    def test_f4s_compact_clips_are_b4s_too(self):
        self.assertEqual(len(r11.F4_CLIPS), 2 * len(body.CLIP_VARIANTS))
        self.assertTrue(r11.F4_CLIPS | r11.F5_CHAIN_CLIPS | r11.E5_CASES <= r11.B4_CASES)
        with round11_docs():
            for name in ("body-clip-level-offset", "body-clip-height-from-feet-knee-goals",
                         "body-clip-keep-o1-y1-xz1-motion"):
                doc = support.body_doc(name, RUN)
                clip = spine_clip(doc, body.BODY_INFO[name]["variant"])
                with self.subTest(name=name):
                    self.assertIsNotNone(check(self, name, doc, Built(name, doc, clip)))
                view = Built(name, doc, clip)
                view.at(doc, "0/0")["m_LocalScale"]["x"] = 1.5
                with self.subTest(name=name, edit="hips"), self.assertRaisesRegex(ValueError, "not one B4 admits"):
                    check(self, name, doc, view)

    def skeleton_case(self, edit):
        name = "body-clip-compact-scoped-level"
        doc, clip, _ = doc_of(name)
        view = Built(name, doc, clip)
        at = {path: k for k, (_, path) in enumerate(view.tos)}
        edit(view, at)
        return name, doc, view

    def test_the_avatar_skeleton_resolves_as_b4_resolves_it(self):
        def name_path(view, k, path):
            """A node named by another path, its m_ID that path's hash, as Unity would build it."""
            view.ids[k] = view.tos[k][0] = zlib.crc32(path.encode("utf-8"))
            view.tos[k][1] = path

        def shared(view, at):
            name_path(view, at["Hips/LeftUpperLeg/LeftLowerLeg"], "Hips/LeftUpperLeg")

        def rehashed(view, at):
            # The right path under another ID: Rust finds the Transform by the hash, so it would not resolve.
            k = at["Hips/LeftUpperLeg/LeftLowerLeg"]
            view.ids[k] = view.tos[k][0] = view.ids[k] ^ 1

        def misparented(view, at):
            view.parents[at["Hips/LeftUpperLeg/LeftLowerLeg"]] = at["Hips"]

        def finger(at, bone):
            return next(k for path, k in at.items() if path.endswith("/" + bone))

        def unresolved(view, at):
            name_path(view, finger(at, "Left Thumb Intermediate"), "Hips/Elsewhere")

        def unnamed(view, at):
            tip = finger(at, "Left Thumb Distal")
            name_path(view, tip, "Nowhere")
            view.parents[tip] = -1

        def remapped(view, at):
            view.bone_index[runs.SLOTS.index("Spine")] = at["Hips/Spine/Chest"]

        with round11_docs():
            name, doc, view = self.skeleton_case(lambda view, at: None)
            self.assertIsNotNone(check(self, name, doc, view))
            for edit, pattern in ((shared, "share a Transform"), (rehashed, "not its m_TOS path's hash"),
                                  (misparented, "missing or misparented"),
                                  (unresolved, "missing or misparented"), (unnamed, "not the Avatar skeleton's"),
                                  (remapped, "Spine resolves to another Transform")):
                name, doc, view = self.skeleton_case(edit)
                with self.subTest(edit=edit.__name__), self.assertRaisesRegex(ValueError, pattern):
                    check(self, name, doc, view)

    def test_the_built_settings_b4_reads(self):
        with round11_docs():
            for name, field, value in (("body-clip-compact-scoped-level", "m_Mirror", True),
                                       ("body-clip-level", "m_LoopTime", True),
                                       ("body-chain-armature-clip", "m_CycleOffset", .5),
                                       ("body-clip-small-scoped-bake-o1-y1-xz0", "m_StartAtOrigin", False),
                                       ("body-clip-level-offset", "m_ValueArrayReferencePose", [1.])):
                info = (r11.ROUND11_INFO | body.BODY_INFO)[name]
                doc = support.body_doc(name, RUN)
                clip = full_clip(doc, name)[1] if name in r11.ROUND11_INFO else spine_clip(doc, info["variant"])
                clip["m_MuscleClip"][field] = value
                pattern = "additive reference pose" if field == "m_ValueArrayReferencePose" else "settings B4 reads"
                with self.subTest(name=name, field=field), self.assertRaisesRegex(ValueError, pattern):
                    check(self, name, doc, Built(name, doc, clip))
            # The motion variant binds all seven motion slots, any other none, B4's rule.
            name = "body-chain-armature-clip"
            doc = support.body_doc(name, RUN)
            clip = spine_clip(doc, body.CHAIN_VARIANT)
            r11.clip_configuration(name, body.BODY_INFO[name], clip, replay)
            clip["m_MuscleClip"]["m_IndexArray"][6] = -1
            with self.assertRaisesRegex(ValueError, "binds 6 motion slot"):
                r11.clip_configuration(name, body.BODY_INFO[name], clip, replay)
            with self.assertRaisesRegex(ValueError, "binds 6 motion slot"):
                r11.clip_configuration(name, dict(variant="level"), clip, replay)

if __name__ == "__main__":
    unittest.main()
