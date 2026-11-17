"""Synthetic replay records. These are deliberately independent of Unity and Rust."""
import copy
import hashlib
import json
import math
import struct
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import anim_replay as replay  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402


def identity(path_id, bundle=0):
    return dict(bundle=bundle, file_index=1, file="CAB-synthetic", path_id=str(path_id))


def bits(value):
    return struct.pack(">f", value).hex()


def synthetic():
    # Two renderer components share a mesh but have different animated weights.
    node = dict(address="", name="Synthetic", id=identity(-9223372036854775808))
    channels = [dict(name="Lift", index=0, full_weights=[25, 80])]
    renderers = [dict(address="", component=i, id=identity(i + 7), mesh=identity(9),
                      channels=channels) for i in range(2)]
    jump = 2 + .1 + 2**-28
    rest = dict(transforms=[dict(translation=["80000000", bits(0), bits(0)],
                                 rotation=[bits(0)] * 3 + [bits(1)],
                                 scale=[bits(1)] * 3)],
                weights=[dict(channels=[bits(12.5)]), dict(channels=[bits(-4)])])
    spec = dict(bundles=[dict(name="synthetic", sha256="a" * 64)], prefab=identity(1),
                prefab_asset="assets/synthetic.prefab", clip=identity(2),
                clip_asset="assets/synthetic.anim", clip_name="Synthetic", binding=identity(3), binding_address="",
                binding_component=0, avatar=None, avatar_asset="", build_target="StandaloneLinux64",
                mode="animator", start=2, stop=3, sample_rate=60,
                source_clock="widened", held_boundaries=[jump],
                schedule=[dict(local=t) for t in (0, .1, 1)], nodes=[node],
                renderers=renderers, evidence=dict(layout="streamed", clock="Synthetic widened local clock", discovery=[
                    dict(owner=identity(3), field="m_AnimationClips", ordinal=0,
                         original=identity(2), resolved=identity(2), outcome="resolved")],
                    import_settings=[dict(property="loopTime", effective=False,
                                          serialized_path="m_LoopTime", serialized=False)]))
    samples = []
    for t, effective, side in ((0, 0, "left"), (.1, 0.10000000149011612, "left"), (1, 1, "right")):
        samples.append(dict(local=t, local_f32=bits(effective), source=2 + t,
                            effective_source=2 + effective, sides=[side], muscles_f32=[], pose=copy.deepcopy(rest)))
    settings = dict.fromkeys(("startTime", "stopTime", "orientationOffsetY", "level", "cycleOffset",
                              "additiveReferencePoseTime"), bits(0))
    settings.update(dict.fromkeys(("loopTime", "loopBlend", "loopBlendOrientation", "loopBlendPositionY",
                                   "loopBlendPositionXZ", "keepOriginalOrientation", "keepOriginalPositionY",
                                   "keepOriginalPositionXZ", "heightFromFeet", "mirror",
                                   "hasAdditiveReferencePose"), False))
    settings["additiveReferencePoseClipName"] = ""
    capture = dict(editor_version="2022.3.1f1", build_target=spec["build_target"],
                   bundle_sha256=["a" * 64],
                   playback=dict(api="AnimationClipPlayable", wrap_mode="Default", clip_looping=False,
                                 reset="fresh_prefab", speed=1, delta_time=0, apply_root_motion=False,
                                 foot_ik=False, playable_ik=False, animator_move=False,
                                 controller_blending=False, events=False, legacy_state="unused",
                                 clip_length_f32=bits(1), frame_rate_f32=bits(60)), rest=rest, samples=samples,
                   clip_settings=settings)
    return dict(format="unityrs.anim_replay", version=1, spec=spec, capture=capture)


def declare_budgets(manifest):
    def row(prop, target=0, channel=-1, rate=0):
        return dict(property=prop, target=target, channel=channel, rate=rate,
                    output_gain=1, min_norm=.5 if prop == "rotation" else 0,
                    stages=[dict(name="synthetic operation", operations=1, scale=8,
                                 gain=1, elementary_error=0)])
    manifest["spec"]["budgets"] = [row("translation", rate=2), row("rotation"), row("scale"),
                                  row("weight", channel=0), row("weight", 1, 0)]
    return manifest


class TestAnimReplay(unittest.TestCase):
    def test_isolated_cli_validates_bundle_bytes_and_derives_bounds(self):
        with scratch.folder("anim-replay-") as root:
            bundle = root / "synthetic"
            bundle.write_bytes(b"synthetic digest input")
            digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
            manifest = declare_budgets(synthetic())
            manifest["spec"]["bundles"][0]["sha256"] = digest
            manifest["capture"]["bundle_sha256"][0] = digest
            fixture = root / "capture.json"
            fixture.write_text(json.dumps(manifest), encoding="utf-8")
            cli = [sys.executable, "-I", str(HERE / "anim_replay.py")]
            result = child.run(cli + ["derive", str(fixture), "--bundle-dir", str(root)],
                               capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            expected = json.loads(json.dumps(replay.derive(manifest)))
            self.assertEqual(json.loads(result.stdout), expected)
            bundle.write_bytes(b"changed synthetic input")
            result = child.run(cli + ["validate", str(fixture), "--bundle-dir", str(root)],
                               capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn("bundle digest mismatch", result.stderr)

    def test_synthetic_validates_and_preserves_exact_f32_values(self):
        replay.validate(synthetic())
        self.assertEqual(replay.f32_from_bits("3dcccccd"), .10000000149011612)
        self.assertEqual(math.copysign(1, replay.f32_from_bits("80000000")), -1)

    def test_bundle_digest_and_serialized_identity_are_checked(self):
        for field, value in (("bundle", 1), ("path_id", "9223372036854775808"),
                             ("path_id", "-0"), ("file", "another-file")):
            m = synthetic()
            m["spec"]["nodes"][0]["id"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                replay.validate(m)
        m = synthetic()
        m["capture"]["bundle_sha256"][0] = "b" * 64
        with self.assertRaisesRegex(ValueError, "digest"):
            replay.validate(m)

    def test_held_side_uses_effective_clock_and_includes_equality(self):
        m = synthetic()
        m["spec"]["held_boundaries"] = [2 + replay.f32_from_bits("3dcccccd")]
        with self.assertRaisesRegex(ValueError, "held"):
            replay.validate(m)
        m["capture"]["samples"][1]["sides"] = ["right"]
        replay.validate(m)

    def test_f32_rounding_can_select_the_other_side(self):
        m = synthetic()
        m["spec"]["held_boundaries"] = [2.1000000007]  # source < jump < effective
        with self.assertRaisesRegex(ValueError, "held"):
            replay.validate(m)
        m["capture"]["samples"][1]["sides"] = ["right"]
        replay.validate(m)

    def test_f32_source_addition_has_its_own_rounding(self):
        m = synthetic()
        m["spec"]["source_clock"] = "f32_add"
        m["capture"]["samples"][1]["effective_source"] = replay.f32_from_bits(bits(2.1))
        replay.validate(m)
        self.assertNotEqual(m["capture"]["samples"][1]["source"],
                            m["capture"]["samples"][1]["effective_source"])

    def test_sample_clock_and_endpoints_cannot_be_inferred_from_the_observations(self):
        m = synthetic()
        m["capture"]["samples"][1]["source"] = .1
        with self.assertRaisesRegex(ValueError, "clock"):
            replay.validate(m)
        m = synthetic()
        m["capture"]["samples"].pop()
        m["spec"]["schedule"].pop()
        with self.assertRaisesRegex(ValueError, "endpoints"):
            replay.validate(m)

    def test_invalid_f32_and_missing_renderer_weights_refuse(self):
        for value in ("7f800000", "7fc00000", "1234"):
            m = synthetic()
            m["capture"]["rest"]["transforms"][0]["translation"][0] = value
            with self.assertRaises(ValueError):
                replay.validate(m)
        m = synthetic()
        m["capture"]["samples"][0]["pose"]["weights"].pop()
        with self.assertRaisesRegex(ValueError, "coverage"):
            replay.validate(m)

    def test_identity_and_component_duplicates_refuse(self):
        m = synthetic()
        m["spec"]["renderers"][1]["id"] = m["spec"]["renderers"][0]["id"]
        with self.assertRaisesRegex(ValueError, "duplicate"):
            replay.validate(m)
        m = synthetic()
        m["spec"]["renderers"][1]["component"] = 0
        with self.assertRaisesRegex(ValueError, "duplicate"):
            replay.validate(m)

    def test_manifest_rejects_enabled_root_motion_unknown_members_and_duplicate_keys(self):
        m = synthetic()
        m["capture"]["playback"]["apply_root_motion"] = True
        with self.assertRaises(ValueError):
            replay.validate(m)
        m = synthetic()
        m["capture"]["samples"][0]["source_seconds"] = 2
        with self.assertRaises(ValueError):
            replay.validate(m)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            replay.read_json('{"version":1,"version":2}')

    def test_derivation_uses_roundoff_clock_rate_and_output_units(self):
        m = declare_budgets(synthetic())
        result = replay.derive(m)
        frame = result["samples"][1]
        trans, rot, _, small_weight, larger_ulp_weight = frame["bounds"]
        expected = 8 / (2**24 - 1) + 2 * abs(frame["effective_source"] - frame["source"])
        self.assertAlmostEqual(trans, expected, delta=1e-18)
        self.assertGreater(rot, 0)
        self.assertGreater(small_weight, larger_ulp_weight)
        self.assertEqual(replay.derive(m), result)

    def test_derivation_requires_explicit_complete_property_budgets(self):
        with self.assertRaisesRegex(ValueError, "budgets"):
            replay.derive(synthetic())
        m = declare_budgets(synthetic())
        m["spec"]["budgets"].pop()
        with self.assertRaisesRegex(ValueError, "coverage"):
            replay.derive(m)

    def test_bound_uses_gain_for_blend_shape_weight_units(self):
        m = declare_budgets(synthetic())
        result = replay.derive(m)["samples"][0]["bounds"][3]
        m["spec"]["budgets"][3]["output_gain"] = 100
        bound = replay.derive(m)["samples"][0]["bounds"][3]
        self.assertAlmostEqual(bound - result, 99 * 8 / (2**24 - 1), delta=1e-16)

    def test_zero_duration_has_one_sample(self):
        m = synthetic()
        m["spec"].update(stop=2, held_boundaries=[], schedule=[dict(local=0)])
        m["capture"]["playback"]["clip_length_f32"] = bits(0)
        m["capture"]["samples"] = m["capture"]["samples"][:1]
        m["capture"]["samples"][0]["sides"] = []
        replay.validate(m)

    def test_observed_clip_settings_must_agree_with_effective_evidence(self):
        m = synthetic()
        m["spec"]["evidence"]["import_settings"][0]["effective"] = True
        with self.assertRaisesRegex(ValueError, "effective clip setting mismatch"):
            replay.validate(m)

    def test_clock_addition_uses_f64_for_integer_json_times(self):
        spec = dict(start=2**53, stop=2**53 + 4, source_clock="widened")
        self.assertEqual(replay.clocks(spec, 1, 4), (bits(1), float(2**53), float(2**53)))

    def test_populated_constant_body_slot_blocks_qualification(self):
        m = synthetic()
        m["spec"].update(avatar=identity(111), avatar_asset="assets/synthetic.asset")
        m["spec"]["evidence"]["humanoid"] = dict(
            absent_slot=-1, slots=[dict(dof=dof, kind="body", index_value=-1, storage="absent")
                                   for dof in ("RootT.x", "RootT.y", "RootT.z", "RootQ.x",
                                               "RootQ.y", "RootQ.z", "RootQ.w")],
            axes=[], disposition="qualified_muscle_only",
            root_hips_comparison=dict(baseline_sha256="c" * 64, root_node=0, hips_node=0, within_bounds=True))
        replay.validate_spec(m["spec"])
        m["spec"]["evidence"]["humanoid"]["slots"][0].update(index_value=0, storage="constant")
        with self.assertRaisesRegex(ValueError, "absent body"):
            replay.validate_spec(m["spec"])

    def test_legacy_replay_uses_the_forced_clamp_even_for_a_looping_clip(self):
        m = synthetic()
        m["spec"]["mode"] = "legacy"
        m["capture"]["playback"].update(api="Animation.Sample", wrap_mode="ClampForever", clip_looping=True,
                                        legacy_state="enabled_weight_one_clamp_forever")
        replay.validate(m)
        m["capture"]["playback"]["wrap_mode"] = "Loop"
        with self.assertRaisesRegex(ValueError, "clamped"):
            replay.validate(m)

    def test_known_vector_replay_pins_the_effective_f32_input(self):
        m = synthetic()
        m["spec"].update(mode="human_pose", avatar=identity(111), avatar_asset="assets/synthetic.asset",
                         held_boundaries=[])
        m["spec"]["evidence"]["humanoid"] = dict(
            absent_slot=-1, slots=[dict(dof="RootT.x", kind="body", index_value=-1, storage="absent")],
            axes=[], disposition="pending")
        m["capture"]["playback"]["api"] = "HumanPoseHandler.SetHumanPose"
        human = dict.fromkeys(("upper_arm_twist", "lower_arm_twist", "upper_leg_twist", "lower_leg_twist",
                               "arm_stretch", "leg_stretch", "feet_spacing"), bits(0))
        human.update(human_scale=bits(1), translation_dof=False, limits=[], axis_properties=[],
                     bones=[dict(name="Hips", node=0, required=True)],
                     muscles=[dict(name="Synthetic axis", bone=0, axis=0, min=bits(-40), max=bits(20))],
                     rest_body_position=[bits(0)] * 3, rest_body_rotation=[bits(0)] * 3 + [bits(1)],
                     rest_muscles=[bits(0)])
        m["capture"]["humanoid"] = human
        for point, sample in zip(m["spec"]["schedule"], m["capture"]["samples"]):
            point["muscles"] = [.1]
            sample.update(sides=[], muscles_f32=[bits(.1)])
        replay.validate(m)
        m["capture"]["samples"][1]["muscles_f32"] = [bits(.2)]
        with self.assertRaisesRegex(ValueError, "known-vector bits"):
            replay.validate(m)
