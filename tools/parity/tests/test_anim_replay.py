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


def probe(sample, value, *, target=0):
    return dict(sample=sample, property="weight", target=target, channel=0, component=0, value=bits(value))


def clock_evidence(jump, right=2):
    return dict(start=dict(source=2, field="m_StartTime", probe=probe(0, 12.5)),
                stop=dict(source=3, field="m_StopTime", probe=probe(3, 17.5)),
                key_times=[2, jump, 3],
                held=[dict(source=jump, left=probe(right - 1, 12.5), right=probe(right, 17.5))])


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
                schedule=[dict(local=t) for t in (0, .1, .5, 1)], nodes=[node],
                renderers=renderers, evidence=dict(layout="streamed", clock=clock_evidence(jump), discovery=[
                    dict(owner=identity(3), field="m_AnimationClips", ordinal=0,
                         original=identity(2), resolved=identity(2), outcome="resolved")],
                    import_settings=[dict(property="loopTime", effective=False,
                                          serialized_path="m_LoopTime", serialized=False)]))
    samples = []
    for t, effective, side in ((0, 0, "left"), (.1, 0.10000000149011612, "left"),
                               (.5, .5, "right"), (1, 1, "right")):
        pose = copy.deepcopy(rest)
        if side == "right":
            pose["weights"][0]["channels"][0] = bits(17.5)
        samples.append(dict(local=t, local_f32=bits(effective), source=2 + t,
                            effective_source=2 + effective, sides=[side], muscles_f32=[], pose=pose))
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
                                 clip_human_motion=False,
                                 reset="fresh_prefab", speed=1, delta_time=0, apply_root_motion=False,
                                 foot_ik=False, playable_ik=False, animator_move=False,
                                 controller_blending=False, events=False, legacy_state="unused",
                                 clip_length_f32=bits(1), frame_rate_f32=bits(60)), rest=rest, samples=samples,
                   clip_settings=settings)
    return dict(format="unityrs.anim_replay", version=1, spec=spec, capture=capture)


def declare_budgets(manifest):
    def row(prop, target, channel=-1):
        stages = [dict(stage="rest", range=[[0, 1]])]
        result = dict(property=prop, target=target, channel=channel, stages=stages)
        if prop == "rotation":
            result["norm_ranges"] = [[[0, 0], [0, 0], [0, 0], [1, 1]]]
        elif prop in ("translation", "weight"):
            stages[:] = [dict(stage="streamed", range=[[0, 0], [0, 0], [2, 2], [6, 6], [0, 1]])]
            if prop == "weight":
                result["weight_units"] = "editor"
        return result

    spec = manifest["spec"]
    spec["budgets"] = [row(prop, i) for i in range(len(spec["nodes"])) for prop in ("translation", "rotation", "scale")]
    spec["budgets"] += [row("weight", i, 0) for i in range(len(spec["renderers"]))]
    return manifest


def human_capture():
    human = dict.fromkeys(("upper_arm_twist", "lower_arm_twist", "upper_leg_twist", "lower_leg_twist",
                           "arm_stretch", "leg_stretch", "feet_spacing"), bits(0))
    human.update(human_scale=bits(1), translation_dof=False, limits=[], axis_properties=[],
                 bones=[dict(name="Hips", node=0, required=True)],
                 muscles=[dict(name="Synthetic axis", bone=0, axis=0, min=bits(-40), max=bits(20))],
                 rest_body_position=[bits(0)] * 3, rest_body_rotation=[bits(0)] * 3 + [bits(1)],
                 rest_muscles=[bits(0)])
    return human


def qualified_humanoid():
    m = synthetic()
    spec, capture = m["spec"], m["capture"]
    spec.update(avatar=identity(111), avatar_asset="assets/synthetic.asset")
    spec["nodes"].append(dict(address="0", name="Hips", id=identity(4)))
    for pose in [capture["rest"], *(row["pose"] for row in capture["samples"])]:
        pose["transforms"].append(copy.deepcopy(pose["transforms"][0]))
    human = human_capture()
    human["bones"] = [dict(name="Hips", node=1, required=True)] + [
        dict(name=f"Optional {i}", node=-1, required=False) for i in range(1, 32)]
    human["muscles"] = [dict(name=f"Muscle {i}", bone=i // 3, axis=i % 3, min=bits(-40), max=bits(20))
                        for i in range(95)]
    human["rest_muscles"] = [bits(0)] * 95
    capture["humanoid"] = human
    capture["playback"]["clip_human_motion"] = True
    slots = [dict(dof=name, kind="body", index_value=-1, storage="absent")
             for name in ("RootT.x", "RootT.y", "RootT.z", "RootQ.x", "RootQ.y", "RootQ.z", "RootQ.w")]
    slots += [dict(dof=row["name"], kind="muscle", index_value=-1, storage="absent")
              for row in human["muscles"]]
    slots[7].update(index_value=0, storage="constant")
    rest = capture["rest"]["transforms"]
    baseline = dict(bundle_sha256=capture["bundle_sha256"][:],
                    **{key: copy.deepcopy(spec[key]) for key in ("prefab", "binding", "avatar")},
                    samples=[dict(local_f32=sample["local_f32"], root=copy.deepcopy(rest[0]),
                                  hips_local=copy.deepcopy(rest[1])) for sample in capture["samples"]])
    baseline_digest = hashlib.sha256(json.dumps(baseline, sort_keys=True, separators=(",", ":")).encode("ascii"))
    spec["evidence"]["humanoid"] = dict(
        absent_slot=-1, slots=slots, storage_counts=dict(streamed=0, dense=0, constant=1),
        axes=[dict(bone=0, axis_type=0, pre=rest[0]["rotation"], post=rest[0]["rotation"],
                   sign=[bits(1)] * 3, min=[bits(-40)] * 3, max=[bits(20)] * 3)],
        disposition="qualified_muscle_only",
        root_hips_comparison=dict(baseline_sha256=baseline_digest.hexdigest(), root_node=0, hips_node=1,
                                  baseline=baseline))
    spec["evidence"]["import_settings"] = [
        dict(property=name, effective=value, serialized_path="m_" + name, serialized=value)
        for name, value in replay.NEUTRAL_SETTINGS.items()]
    return declare_budgets(m)


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
        jump = 2 + replay.f32_from_bits("3dcccccd")
        m["spec"]["held_boundaries"] = [jump]
        m["spec"]["evidence"]["clock"] = clock_evidence(jump, right=1)
        with self.assertRaisesRegex(ValueError, "held"):
            replay.validate(m)
        m["capture"]["samples"][1]["sides"] = ["right"]
        m["capture"]["samples"][1]["pose"]["weights"][0]["channels"][0] = bits(17.5)
        replay.validate(m)

    def test_f32_rounding_can_select_the_other_side(self):
        m = synthetic()
        jump = 2.1000000007  # source < jump < effective
        m["spec"]["held_boundaries"] = [jump]
        m["spec"]["evidence"]["clock"] = clock_evidence(jump, right=1)
        with self.assertRaisesRegex(ValueError, "held"):
            replay.validate(m)
        m["capture"]["samples"][1]["sides"] = ["right"]
        with self.assertRaisesRegex(ValueError, "independent clock probe"):
            replay.validate(m)
        m["capture"]["samples"][1]["pose"]["weights"][0]["channels"][0] = bits(17.5)
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

    def test_off_key_sample_must_survive_f32_rounding(self):
        m = synthetic()
        spec = m["spec"]
        spec["schedule"] = [dict(local=t) for t in (0, 1 - 2**-26, 1)]
        spec["held_boundaries"] = []
        spec["evidence"]["clock"].update(held=[], key_times=[2, 3])
        spec["evidence"]["clock"]["stop"]["probe"]["sample"] = 2
        with self.assertRaisesRegex(ValueError, "off-key"):
            replay.validate_spec(spec)

    def test_f32_source_clock_can_round_an_interior_sample_onto_a_key(self):
        m = synthetic()
        spec = m["spec"]
        spec.update(source_clock="f32_add", held_boundaries=[], schedule=[dict(local=t) for t in (0, .1, 1)])
        clock = spec["evidence"]["clock"]
        clock.update(held=[], key_times=[2, 2.0999999046325684, 3])
        clock["stop"]["probe"]["sample"] = 2
        with self.assertRaisesRegex(ValueError, "off-key"):
            replay.validate_spec(spec)
        # Clamping a rounded source time to the exact unrounded stop
        # must also count as an endpoint.
        stop = 1 + 3 * 2**-25
        spec.update(start=1, stop=stop, schedule=[dict(local=t) for t in (0, 5 * 2**-26, stop - 1)])
        clock["start"]["source"], clock["stop"]["source"] = 1, stop
        clock["key_times"] = [1, stop]
        with self.assertRaisesRegex(ValueError, "off-key"):
            replay.validate_spec(spec)

    def test_clock_model_and_held_observations_are_required(self):
        m = synthetic()
        m["spec"]["source_clock"] = "probably widened"
        with self.assertRaisesRegex(ValueError, "source_clock"):
            replay.validate(m)
        m = synthetic()
        m["spec"]["evidence"]["clock"]["held"] = []
        with self.assertRaisesRegex(ValueError, "held observations"):
            replay.validate(m)
        m = synthetic()
        m["spec"]["evidence"]["clock"]["held"][0]["right"]["value"] = bits(20)
        with self.assertRaisesRegex(ValueError, "independent clock probe"):
            replay.validate(m)
        m = synthetic()
        m["spec"]["evidence"]["clock"]["held"][0]["right"]["sample"] = 3
        with self.assertRaisesRegex(ValueError, "first effective right sample"):
            replay.validate(m)

    def test_source_origin_is_pinned_by_independently_inspected_endpoints(self):
        m = synthetic()
        spec = m["spec"]
        for key in ("start", "stop"):
            spec[key] += 1
        spec["held_boundaries"] = [value + 1 for value in spec["held_boundaries"]]
        for sample in m["capture"]["samples"]:
            sample["source"] += 1
            sample["effective_source"] += 1
        with self.assertRaisesRegex(ValueError, "origin"):
            replay.validate(m)

    def test_missing_humanoid_slot_inventory_cannot_qualify(self):
        m = qualified_humanoid()
        replay.validate(m)
        m["spec"]["evidence"]["humanoid"]["slots"].pop()
        with self.assertRaisesRegex(ValueError, "complete muscle"):
            replay.validate(m)

    def test_qualification_requires_axes_and_independent_human_trait_order(self):
        m = qualified_humanoid()
        m["spec"]["evidence"]["humanoid"]["axes"] = []
        with self.assertRaisesRegex(ValueError, "runtime axes"):
            replay.validate(m)
        m = qualified_humanoid()
        m["spec"]["evidence"]["humanoid"]["slots"][-1]["dof"] = "invented muscle"
        with self.assertRaisesRegex(ValueError, "HumanTrait inventory"):
            replay.validate(m)

    def test_qualification_refuses_missing_non_neutral_or_unserialized_settings(self):
        for name, field, value in (("mirror", "effective", True), ("mirror", "serialized", True),
                                   ("cycleOffset", "effective", .125), ("startAtOrigin", "serialized", None)):
            with self.subTest(name=name, field=field):
                m = qualified_humanoid()
                setting = next(row for row in m["spec"]["evidence"]["import_settings"] if row["property"] == name)
                setting[field] = value
                with self.assertRaisesRegex(ValueError, name):
                    replay.validate(m)
        m = qualified_humanoid()
        m["spec"]["evidence"]["import_settings"].pop()
        with self.assertRaisesRegex(ValueError, "complete effective import settings"):
            replay.validate(m)
        m = qualified_humanoid()
        m["capture"]["clip_settings"]["mirror"] = True
        with self.assertRaisesRegex(ValueError, "effective clip setting mismatch: mirror"):
            replay.validate(m)

    def test_qualification_requires_distinct_root_and_hips_with_measured_residuals(self):
        m = qualified_humanoid()
        comparison = m["spec"]["evidence"]["humanoid"]["root_hips_comparison"]
        comparison["hips_node"] = comparison["root_node"]
        with self.assertRaisesRegex(ValueError, "distinct descendant"):
            replay.validate(m)
        for target, side, prop, value in ((0, "root", "translation", [bits(1)] * 3),
                                          (1, "hips_local", "rotation", [bits(1), bits(0), bits(0), bits(0)])):
            with self.subTest(side=side):
                m = qualified_humanoid()
                m["capture"]["samples"][1]["pose"]["transforms"][target][prop] = value
                with self.assertRaisesRegex(ValueError, f"{side} {prop} residual"):
                    replay.validate(m)
        m = qualified_humanoid()
        comparison = m["spec"]["evidence"]["humanoid"]["root_hips_comparison"]
        comparison["within_bounds"] = True
        with self.assertRaisesRegex(ValueError, "unknown member within_bounds"):
            replay.validate(m)
        del comparison["within_bounds"]
        comparison["baseline"]["samples"].pop()
        with self.assertRaisesRegex(ValueError, "incomplete root and hips-local baseline"):
            replay.validate(m)
        for field, value, reason in (("bundle_sha256", [], "same built rig"),
                                     ("local_f32", bits(.2), "schedule mismatch"),
                                     ("root", dict(translation=[bits(1)] * 3, rotation=[bits(0)] * 3 + [bits(1)],
                                                   scale=[bits(1)] * 3), "baseline digest mismatch")):
            with self.subTest(field=field):
                m = qualified_humanoid()
                baseline = m["spec"]["evidence"]["humanoid"]["root_hips_comparison"]["baseline"]
                target = baseline if field == "bundle_sha256" else baseline["samples"][1]
                target[field] = value
                with self.assertRaisesRegex(ValueError, reason):
                    replay.validate(m)
        m = qualified_humanoid()
        m["capture"]["playback"]["clip_human_motion"] = False
        with self.assertRaisesRegex(ValueError, "observed human-motion clip"):
            replay.validate(m)

    def test_budget_stage_requires_a_range_and_a_supported_stage(self):
        for field, value in (("stage", "unknown"), ("range", None), ("range", [[1, 0]] * 5),
                             ("gain", 1000)):
            m = declare_budgets(synthetic())
            m["spec"]["budgets"][0]["stages"][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                replay.derive(m)
        m = declare_budgets(synthetic())
        m["spec"]["budgets"][0]["rate"] = 1000
        with self.assertRaisesRegex(ValueError, "unknown member rate"):
            replay.derive(m)

    def test_rotation_bound_must_be_smaller_than_pi(self):
        m = declare_budgets(synthetic())
        budget = m["spec"]["budgets"][1]
        budget["norm_ranges"][0][3] = [1e-12, 1e-12]
        budget["stages"] = copy.deepcopy(m["spec"]["budgets"][0]["stages"])
        with self.assertRaisesRegex(ValueError, "reaches pi"):
            replay.derive(m)

    def test_derivation_recomputes_dense_and_hermite_clock_rates(self):
        # Dense runs from 0 to 3 in 1/2 s. Hermite's slope with flat
        # tangents is greatest at the midpoint, 1.5*3/0.5 = 9.
        for stage, ranges, rate in (
                ("dense", [[0, 0], [3, 3], [2, 2], [2, 3], [2, 2], [0, .5]], 6),
                ("hermite", [[0, 0], [3, 3], [0, 0], [0, 0], [.5, .5]], 9)):
            with self.subTest(stage=stage):
                m = declare_budgets(synthetic())
                m["spec"]["budgets"][0]["stages"] = [dict(stage=stage, range=ranges)]
                first, off_key, *_ = replay.derive(m)["samples"]
                dt = abs(off_key["source"] - off_key["effective_source"])
                self.assertAlmostEqual(off_key["bounds"][0] - first["bounds"][0], rate * dt, delta=1e-17)

    def test_weighted_budget_encloses_flat_time_inverse(self):
        m = declare_budgets(synthetic())
        stages = m["spec"]["budgets"][0]["stages"]
        stages[:] = [dict(stage="weighted", range=[[0, 0], [1, 1], [0, 0], [0, 0],
                                                     [1, 1], [1, 1], [1, 1]])]
        flat = replay.derive(m)["samples"][1]["bounds"][0]
        stages[0]["range"][-2:] = [[.25, .25], [.25, .25]]
        regular = replay.derive(m)["samples"][1]["bounds"][0]
        self.assertGreater(flat, .01)
        self.assertLess(regular, .001)

    def test_joint_budget_propagates_the_recorded_muscle_limit(self):
        m = qualified_humanoid()
        m["spec"]["evidence"]["humanoid"]["disposition"] = "pending"
        stages = m["spec"]["budgets"][4]["stages"]
        stages[:] = [dict(stage="streamed", range=[[0, 0], [0, 0], [1, 1], [0, 0], [0, 1]]),
                     dict(stage="muscle", range=[[0, 1], [-10, 10]]),
                     dict(stage="joint", range=[[-40, 40]])]
        narrow = replay.derive(m)["samples"][1]["bounds"][4]
        stages[1]["range"][1] = [-40, 40]
        wide = replay.derive(m)["samples"][1]["bounds"][4]
        self.assertGreater(wide, narrow)
        stages[1]["range"][1] = [-80, 80]
        with self.assertRaisesRegex(ValueError, "joint angle range"):
            replay.derive(m)

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
        expected = 8 / (2**20 - 1) + 2 * abs(frame["effective_source"] - frame["source"])
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
        m["spec"]["budgets"][3]["weight_units"] = "fraction_of_100"
        bound = replay.derive(m)["samples"][0]["bounds"][3]
        self.assertAlmostEqual(bound - result, 99 * 8 / (2**20 - 1), delta=1e-16)

        # The percent conversion can flush a subnormal result to zero.
        m["spec"]["budgets"][4].update(weight_units="fraction_of_100",
                                      stages=[dict(stage="constant", range=[[2**-149, 2**-149]])])
        for sample in m["capture"]["samples"]:
            sample["pose"]["weights"][1]["channels"][0] = bits(0)
        bound = replay.derive(m)["samples"][0]["bounds"][4]
        self.assertGreaterEqual(bound, 100 * 2**-149)

    def test_zero_duration_has_one_sample(self):
        m = synthetic()
        m["spec"].update(stop=2, held_boundaries=[], schedule=[dict(local=0)])
        clock = m["spec"]["evidence"]["clock"]
        clock.update(stop=dict(clock["start"]), key_times=[2], held=[])
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
        m = qualified_humanoid()
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
        m["spec"]["evidence"]["clock"].update(held=[], key_times=[2, 3])
        m["spec"]["evidence"]["humanoid"] = dict(
            absent_slot=-1, slots=[dict(dof="RootT.x", kind="body", index_value=-1, storage="absent")],
            axes=[], disposition="pending")
        m["capture"]["playback"]["api"] = "HumanPoseHandler.SetHumanPose"
        m["capture"]["humanoid"] = human_capture()
        for point, sample in zip(m["spec"]["schedule"], m["capture"]["samples"]):
            point["muscles"] = [.1]
            sample.update(sides=[], muscles_f32=[bits(.1)])
        replay.validate(m)
        m["capture"]["samples"][1]["muscles_f32"] = [bits(.2)]
        with self.assertRaisesRegex(ValueError, "known-vector bits"):
            replay.validate(m)
