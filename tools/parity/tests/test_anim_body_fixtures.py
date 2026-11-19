"""The optional body-motion fixtures: their inventory mirrors the builder, their rows are checked
fail-closed, and a run without them, or with all of them unrecorded, still completes."""

import copy
import re
import unittest
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage as coverage
from anim_body_support import MUSCLES, body_doc, clip_case
from anim_coverage_support import COVERAGE
from anim_fixture_support import RUN, write

import anim_fixture_body as body  # noqa: E402  (test_anim_replay puts it on the path)
import anim_fixture_coverage  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_replay_complete import OPTIONAL_CASES  # noqa: E402

EDITOR = Path(__file__).resolve().parents[2] / "fixtures" / "unity" / "Editor"


class View:
    def __init__(self, clip):
        self.clip = clip

    def value(self, key):
        return self.clip


def source(name):
    return (EDITOR / name).read_text(encoding="utf-8")


class BodyInventoryTests(unittest.TestCase):
    def test_cases_mirror_the_builder(self):
        self.assertEqual(len(body.BODY_CASES), 106)
        kinds = [info["kind"] for info in body.BODY_INFO.values()]
        self.assertEqual((kinds.count("probes"), kinds.count("targets"), kinds.count("clip")), (16, 8, 82))
        self.assertEqual(len(body.CLIP_VARIANTS), 39)
        self.assertLessEqual(body.BODY_CASES.keys(), OPTIONAL_CASES)
        self.assertTrue(body.BODY_CASES.keys().isdisjoint(anim_fixture_coverage.COVERAGE_CASES))
        builder = source("UnityrsBodyFixtures.cs")
        rigs = re.findall(r'new BodyRig \{ Name = "([a-z-]+)"', builder)
        self.assertEqual(rigs, [*body.PROBE_RIGS, *body.CHAIN_RIGS])
        table = builder[builder.index("ProbeGroups = {"):]
        groups = re.findall(r'new\[\] \{ "([A-Za-z]+)", ((?:"[^"]+"(?:, )?)+) \}', table[:table.index("};")])
        self.assertEqual([(bone, tuple(re.findall(r'"([^"]+)"', muscles))) for bone, muscles in groups],
                         [(bone, tuple(muscles)) for bone, muscles in body.PROBE_GROUPS])
        clips = source("UnityrsBodyClips.cs")
        for name in body.CLIP_VARIANTS:
            stem = re.sub(r"-start$", "", name)
            self.assertTrue(stem.startswith(("keep-", "bake-")) or f'"{stem}"' in clips, name)
        self.assertIn('BodyClipBundle = "body-clips"', clips)

    def test_schedules_keep_the_spine_anchors_on_an_exact_grid(self):
        names = list(MUSCLES)
        spine = [names.index(muscle) for muscle in body.SPINE]
        for rig in body.PROBE_RIGS:
            for targets in (False, True):
                points = body.probe_schedule(rig, names, targets)
                times = [t for t, _ in points]
                self.assertEqual(times, sorted(set(times)))
                self.assertTrue(all(t * 65536 == int(t * 65536) for t in times))
                anchors = {t: [vector[i] for i in spine] for t, vector in points if t in (0, .5, 1)}
                self.assertEqual(anchors, {t: list(v) for t, v in body.ANCHORS.items()})
                probes = sum(len(group) for bone, group in body.PROBE_GROUPS if bone in body.rig_bones(rig))
                self.assertEqual(len(points), 6 if targets else probes + 6)
        self.assertNotIn("Chest", body.rig_bones("no-chest"))
        self.assertEqual(body.rig_bones("eyes-jaw") - body.rig_bones("compact"), {"LeftEye", "RightEye", "Jaw"})


class CheckBodyTests(unittest.TestCase):
    def check(self, name, doc, clip=None):
        return body.check_body(name, body.BODY_CASES[name], doc, View(clip), None, replay)

    def assert_refused(self, message, name, doc, clip=None):
        with self.assertRaisesRegex(ValueError, message):
            self.check(name, doc, clip)

    def test_accepts_each_kind(self):
        for name in ("body-probes-compact", "body-probes-no-chest", "body-targets-tall-offset",
                     "body-chain-armature-targets", "body-chain-large-targets", "body-chain-hips-scale-targets",
                     "body-chain-scaled-animator-targets"):
            with self.subTest(name=name):
                self.assertIsNone(self.check(name, body_doc(name, RUN)))
        for name in ("body-clip-keep-o1-y0-xz1-start-offset", "body-clip-height-from-feet-knee-goals",
                     "body-clip-keep-o1-y1-xz1-motion", "body-chain-armature-clip"):
            with self.subTest(name=name):
                doc = body_doc(name, RUN)
                built = clip_case(doc, body.BODY_INFO[name]["variant"])
                settings = self.check(name, doc, built)
                self.assertLessEqual(set(range(7, 14)), set(settings["animator_attributes"]))
                self.assertEqual(settings["serialized"]["m_StartAtOrigin"], True)

    def test_refuses_a_wrong_case_row_or_topology(self):
        doc = body_doc("body-probes-eyes", RUN)
        doc["spec"]["evidence"]["editor"]["body_case"][0]["rig"] = "jaw"
        self.assert_refused("body case row mismatch", "body-probes-eyes", doc)
        doc = body_doc("body-probes-eyes", RUN)
        next(row for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Jaw")["node"] = 3
        self.assert_refused("captured Avatar topology differs", "body-probes-eyes", doc)
        doc = body_doc("body-probes-eyes", RUN)
        doc["spec"]["evidence"]["editor"]["human_bones"][4]["mass"] = "7f800000"
        self.assert_refused("expected finite f32 bits", "body-probes-eyes", doc)

    def test_refuses_missing_or_malformed_rows(self):
        name = "body-probes-compact"
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"].pop()
        self.assert_refused("body read-back rows mismatch", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"][2]["readback_rotation"] = [replay.f32_bits(.5)] * 4 + []
        doc["spec"]["evidence"]["editor"]["body_samples"][2]["readback_rotation"][3] = replay.f32_bits(.9)
        self.assert_refused("not a unit quaternion", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"][0]["root_position"] = ["00000000"] * 3
        self.assert_refused("body root rows mismatch", name, doc)

    def test_refuses_missing_muscles_or_node_rows(self):
        name = "body-probes-compact"
        for edit, message in (
                (lambda row: row["readback_muscles"].pop(), "body read-back muscles mismatch"),
                (lambda row: row.pop("nodes"), "body node rows mismatch"),
                (lambda row: row["nodes"].pop(), "body node rows mismatch"),
                (lambda row: row["nodes"][4].__setitem__("rotation", [replay.f32_bits(.6)] * 4),
                 "body node rotation is not a unit quaternion"),
                (lambda row: row["nodes"][4].__setitem__("scale", ["7f800000"] * 3), "expected finite f32 bits"),
                (lambda row: row["nodes"][2].__setitem__("hips_position", [replay.f32_bits(.1)] * 3),
                 "hips are not at the origin")):
            with self.subTest(message=message):
                doc = body_doc(name, RUN)
                edit(doc["spec"]["evidence"]["editor"]["body_samples"][1])
                self.assert_refused(message, name, doc)

    def test_a_clip_needs_its_root_motion_rows(self):
        name = "body-clip-keep-o1-y1-xz1-motion"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "keep-o1-y1-xz1-motion")
        self.check(name, doc, built)
        for field in ("motion_position", "motion_delta_rotation"):
            with self.subTest(field=field):
                changed = copy.deepcopy(doc)
                changed["spec"]["evidence"]["editor"]["body_samples"][0][field] = []
                self.assert_refused("body root rows mismatch", name, changed, built)
        # A known-vector case has no playback and so no root-motion rows.
        doc = body_doc("body-probes-compact", RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"][0]["motion_position"] = ["00000000"] * 3
        self.assert_refused("body root rows mismatch", "body-probes-compact", doc)

    def test_refuses_a_schedule_or_target_off_the_contract(self):
        name = "body-targets-compact"
        doc = body_doc(name, RUN)
        doc["spec"]["schedule"][1]["muscles"][0] += .125
        self.assert_refused("body known-vector schedule mismatch", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_targets"][1]["position"][0] = replay.f32_bits(.5)
        self.assert_refused("applied body target differs", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_targets"][0]["tilt"] = ["00000000"] * 3
        self.assert_refused("body target request mismatch", name, doc)

    def test_refuses_a_chain_off_its_rig(self):
        name = "body-chain-hips-scale-targets"
        doc = body_doc(name, RUN)
        for pose in (doc["capture"]["rest"],):
            pose["transforms"][2]["scale"] = [replay.f32_bits(1)] * 3
        self.assert_refused("body rig chain differs", name, doc)

    def test_refuses_clip_settings_or_curves_off_the_variant(self):
        name = "body-clip-bake-o1-y0-xz0-offset"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "bake-o1-y0-xz0")
        changed = copy.deepcopy(doc)
        changed["capture"]["clip_settings"]["loopBlendOrientation"] = False
        self.assert_refused("captured clip settings differ", name, changed, built)
        changed = copy.deepcopy(built)
        changed["m_MuscleClip"]["m_LoopBlendOrientation"] = 0
        self.assert_refused("built clip root settings differ", name, doc, changed)
        changed = copy.deepcopy(doc)
        changed["spec"]["evidence"]["editor"]["bindings"][0]["keys"][2]["value_f32"] = replay.f32_bits(.5)
        self.assert_refused("authored body curve mismatch", name, changed, built)
        changed = copy.deepcopy(doc)
        changed["spec"]["evidence"]["editor"]["body_targets"] = [{}]
        self.assert_refused("known-vector targets", name, changed, built)


    def test_a_dense_slot_interpolates_between_frames(self):
        # The last streamed slot (the right knee) moves to a dense clip at 1.5 frames per second, so the key
        # at .5 lies between frames 0 and 2/3 and only linear interpolation reproduces it.
        name = "body-clip-height-from-feet-knee"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "height-from-feet-knee")
        stream = built["m_MuscleClip"]["m_Clip"]["data"]["m_StreamedClip"]
        last = stream["curveCount"] - 1
        words, kept, cursor = stream["data"], [], 0
        while cursor < len(words):
            time, count = words[cursor:cursor + 2]
            entries = [words[i:i + 5] for i in range(cursor + 2, cursor + 2 + 5 * count, 5) if words[i] != last]
            kept += [time, len(entries)] + [word for entry in entries for word in entry]
            cursor += 2 + 5 * count
        a, b, c = body.KNEE["Right Lower Leg Stretch"]
        second = a + (b - a) / .75
        frames = [a, second, 2 * c - second]
        stream.update(curveCount=last, data=kept)
        built["m_MuscleClip"]["m_Clip"]["data"]["m_DenseClip"] = dict(m_CurveCount=1, m_SampleArray=frames,
                                                                      m_BeginTime=0., m_SampleRate=1.5)
        self.assertTrue(self.check(name, doc, built))
        frames[1] += .01
        self.assert_refused("built clip lost its", name, doc, built)

    def test_a_variant_counts_only_with_its_built_distinguishing_slots(self):
        self.assertEqual(body.CHAIN_VARIANT, "keep-o1-y1-xz1-motion")
        self.assertTrue(body.CLIP_VARIANTS[body.CHAIN_VARIANT]["motion"])
        self.assertEqual(len({y for _, y, _ in body.MOTION_PATH}), len(body.MOTION_PATH))
        self.assertTrue(all(y for _, y, _ in body.MOTION_PATH))
        for name, attribute in (("body-clip-keep-o1-y1-xz1-motion", 1), ("body-clip-height-from-feet-knee-goals", 15),
                                ("body-clip-height-from-feet-knee", None)):
            doc = body_doc(name, RUN)
            built = clip_case(doc, body.BODY_INFO[name]["variant"])
            bindings = built["m_ClipBindingConstant"]["genericBindings"]
            if attribute is None:
                # Drop the left knee's stretch slot: the slot count no longer matches the bindings.
                attribute = 42 + list(MUSCLES).index("Left Lower Leg Stretch")
            lost = copy.deepcopy(built)
            lost["m_ClipBindingConstant"]["genericBindings"] = [row for row in bindings if row["attribute"] != attribute]
            self.assert_refused("built clip lost its", name, doc, lost)
            # An Editor curve copy does not stand in for the runtime slot.
            variant = body.CLIP_VARIANTS[body.BODY_INFO[name]["variant"]]
            prop, times, values = next((prop, times, values) for group in body.distinguishing_curves(variant, list(MUSCLES))
                                       for prop, slot, times, values in group if slot == attribute)
            lost["m_EditorCurves"] = lost["m_FloatCurves"] = [dict(path="", classID=95, attribute=prop, curve=dict(
                m_Curve=[dict(time=t, value=v) for t, v in zip(times, values)]))]
            self.assert_refused("built clip lost its", name, doc, lost)
            # The DOF absent from m_IndexArray, or pointing at another DOF's slot.
            for value in (-1, (built["m_MuscleClip"]["m_IndexArray"][attribute] + 1) % len(bindings)):
                moved = copy.deepcopy(built)
                moved["m_MuscleClip"]["m_IndexArray"][attribute] = value
                self.assert_refused("built clip lost its", name, doc, moved)
            # The same slot with another value at its middle key.
            slot = [row["attribute"] for row in bindings].index(attribute)
            changed = copy.deepcopy(built)
            data = changed["m_MuscleClip"]["m_Clip"]["data"]["m_StreamedClip"]["data"]
            cursor, frame = 0, 0
            while data[cursor] != 0x7f800000:
                count = data[cursor + 1]
                for i in range(cursor + 2, cursor + 2 + 5 * count, 5):
                    if data[i] == slot and frame == 1:
                        data[i + 4] = int(replay.f32_bits(.75), 16)
                cursor, frame = cursor + 2 + 5 * count, frame + 1
            self.assert_refused("built clip lost its", name, doc, changed)
        name = "body-clip-keep-o1-y1-xz1-motion"
        doc = body_doc(name, RUN)
        built = clip_case(doc, body.BODY_INFO[name]["variant"])
        motion = next(row for row in doc["spec"]["evidence"]["editor"]["bindings"] if row["property"] == "MotionT.y")
        motion["keys"][2]["value_f32"] = replay.f32_bits(0)
        self.assert_refused("authored MotionT.x curve mismatch", name, doc, built)


class BodyRunTests(unittest.TestCase):
    # The coverage completion run, with or without the body rows.
    setUp = coverage.CoverageCompletionTests.setUp
    complete = coverage.CoverageCompletionTests.complete
    assert_refused = coverage.CoverageCompletionTests.assert_refused

    def run_with(self, names, statuses=None, **kwargs):
        self.coverage = coverage.coverage_archives(self.bundles, self.inspect)
        self.statuses |= statuses or {}
        with mock.patch.object(coverage, "DEFERRED", (*coverage.DEFERRED, *names)):
            return self.complete(**kwargs)

    def test_a_run_with_every_body_case_unrecorded_completes(self):
        result = self.run_with(body.BODY_CASES)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("body-clip-level-offset: not_covered (Unity refused it)", result.stdout)

    def test_a_run_lists_all_body_cases_or_none(self):
        missing = "body-probes-holdout"
        result = self.run_with([name for name in body.BODY_CASES if name != missing])
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"missing run.json case: {missing}", result.stderr)

    def test_a_failed_or_unwritten_body_case_fails_the_run(self):
        name = "body-targets-tall"
        result = self.run_with(body.BODY_CASES, {name: "failed"})
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"{name}: failed (Unity refused it)", result.stderr)
        self.statuses[name] = "recorded"
        result = self.run_with(body.BODY_CASES)
        self.assertIn(f"{name}: missing replays/body/{name}.pending.json", result.stderr)

    def test_refuses_an_unlisted_body_recording_or_bundle(self):
        (self.replays / "body").mkdir()
        write(self.replays / "body" / "body-probes-jaw.pending.json", body_doc("body-probes-jaw", RUN))
        result = self.run_with(body.BODY_CASES)
        self.assertIn("unexpected pending recording: replays/body/body-probes-jaw.pending.json", result.stderr)
        (self.replays / "body" / "body-probes-jaw.pending.json").unlink()
        (self.bundles / "body-jaw").write_bytes(b"UnityFS\0")
        self.assertEqual(self.run_with(body.BODY_CASES).returncode, 0)
        (self.bundles / "body-unknown").write_bytes(b"UnityFS\0")
        result = self.run_with(body.BODY_CASES)
        self.assertIn("unexpected fixture bundle inventory", result.stderr)

    def test_an_ordinary_case_claiming_body_rows_is_refused(self):
        self.coverage = coverage.coverage_archives(self.bundles, self.inspect)
        self.coverage[COVERAGE[0]]["spec"]["evidence"]["editor"]["body_case"] = [{"kind": "probes"}]
        self.assert_refused("claims body fixture evidence")


if __name__ == "__main__":
    unittest.main()
