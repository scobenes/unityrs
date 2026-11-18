"""Analytic checks for the independent f64 animation transcription."""
import math
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import anim_reference as reference  # noqa: E402
from anim_reference import (  # noqa: E402
    angle, bake_bound, constant, dense, euler, hermite, joint_rotation,
    local_pose, muscle_angle, normalize, packed_times, schedule, source_to_local,
    streamed, transcription_angle_bound, transcription_bound, ulp32, weighted, weighted_bound,
)


class TestAnimReference(unittest.TestCase):
    def test_streamed_horner_and_sparse_holds(self):
        keys = [(2, (2, -1, 3, 4)), (4, (9, 8, 7, -2))]
        self.assertEqual([streamed(keys, t) for t in (0, 2, 3, 4, 9)], [4, 4, 8, -2, -2])

    def test_discrete_seed_and_right_hand_value(self):
        keys = [(2, (0, 0, 0, 4)), (4, (0, 0, 0, 9))]
        self.assertEqual([streamed(keys, t, held=True, seed=1) for t in (0, 2, 3, 4)],
                         [1, 4, 4, 9])
        self.assertEqual(streamed([], 5, held=True, seed=0), 0)
        with self.assertRaises(ValueError):
            streamed([], 5)

    def test_dense_frame_major_and_clamp(self):
        frames = [0, 4, 30, 8, 60, 20]
        self.assertEqual([dense(frames, 2, 1, 2, 4, t) for t in (1, 2, 2.125, 2.25, 9)],
                         [4, 4, 6, 8, 20])

    def test_dense_discrete_and_single_frame(self):
        self.assertEqual(dense([4, 8], 1, 0, 2, 4, 2.125, held=True), 4)
        self.assertEqual(dense([4, 8], 1, 0, 2, 4, 2.25, held=True), 8)
        self.assertEqual(dense([2], 1, 0, 4, 3, 0), 2)
        with self.assertRaises(ValueError):
            dense([2], 2, 0, 4, 3, 0)

    def test_constant(self):
        self.assertEqual(constant([2, 3], 1), 3)

    def test_hermite_scaled_asymmetric_tangents(self):
        # At s=1/2 the bases are 1/2, 1/8, 1/2, -1/8 and dt=4.
        self.assertEqual(hermite(4, 8, 2, 10, 6, -2, 6), 10)
        self.assertEqual(hermite(4, 8, 2, 10, 6, -2, 99), 10)

    def test_explicit_held_segments_change_at_the_exact_key(self):
        for t, expected in ((0, 2), (.5, 2), (1, 10), (2, 10)):
            self.assertEqual(hermite(0, 1, 2, 10, math.inf, math.inf, t, held=True), expected)

    def test_short_curves_refuse_unconfirmed_wrap(self):
        for evaluate, weights in ((hermite, ()), (weighted, (.25, .25))):
            for time, side in ((.5, "pre_wrap"), (2.5, "post_wrap")):
                with self.subTest(evaluator=evaluate.__name__, side=side):
                    args = (1, 2, 3, 5, 0, 0, *weights, time)
                    self.assertEqual(evaluate(*args, **{side: "ClampForever"}), 3 if time < 1 else 5)
                    with self.assertRaises(reference.UnsupportedCurveWrap) as refusal:
                        evaluate(*args, **{side: "Loop"})
                    self.assertEqual(refusal.exception.code, "unsupported_curve_wrap")
            self.assertEqual(evaluate(1, 2, 3, 5, 0, 0, *weights, 1, pre_wrap="Loop"), 3)

    def test_only_one_infinite_tangent_is_needed_to_hold(self):
        for outgoing, incoming in ((math.inf, 0), (0, -math.inf), (math.inf, -math.inf)):
            for time, value in ((.5, 2), (1, 10)):
                args = (0, 1, 2, 10, outgoing, incoming)
                with self.subTest(outgoing=outgoing, incoming=incoming, time=time):
                    self.assertEqual(hermite(*args, time), value)
                    self.assertEqual(weighted(*args, .25, .25, time), value)
                    self.assertEqual(weighted_bound(*args, .25, .25, time), 0)

    def test_nan_tangents_refuse_even_at_endpoints(self):
        for outgoing, incoming in ((math.nan, math.inf), (math.inf, math.nan)):
            with self.subTest(outgoing=outgoing, incoming=incoming):
                args = (0, 1, 2, 10, outgoing, incoming)
                with self.assertRaisesRegex(ValueError, "NaN"):
                    hermite(*args, 0, held=True)
                with self.assertRaisesRegex(ValueError, "NaN"):
                    weighted(*args, .25, .25, 0)
                with self.assertRaisesRegex(ValueError, "NaN"):
                    weighted_bound(*args, .25, .25, 0)

    def test_weighted_inverts_time_and_returns_endpoints(self):
        # x controls (0, 0, 0, 1), y controls (0, 0, 1, 1).
        # x(1/2)=1/8, y(1/2)=1/2.
        args = (2, 4, 0, 1, 0, 0, 0, 1)
        self.assertEqual(weighted(*args, 2.25), .5)
        self.assertEqual(weighted(*args, 2), 0)
        self.assertEqual(weighted(*args, 4), 1)

    def test_weighted_values_include_slope_times_handle_width(self):
        # t=1/2, x=[0,1/4,3/4,1], y=[0,2,4,4], so y=11/4.
        self.assertEqual(weighted(0, 4, 0, 4, 2, 0, .25, .25, 2), 2.75)

    def test_weighted_rejects_out_of_range_weights_and_allows_isolated_flat(self):
        args = (0, 1, 0, 1, 1, 1, 1, 1)
        self.assertEqual(weighted(*args, .5), .5)  # x' = 3*(2p-1)^2.
        with self.assertRaisesRegex(ValueError, "fractional encoding"):
            weighted(0, 1, 0, 1, 1, 1, 1.25, 1, .5)

    def test_weighted_gate_accounts_for_inverse_near_flat(self):
        args = (0, 1, 0, 1, 0, 0, 1, 1)
        self.assertGreater(weighted_bound(*args, .5), 1e-5)
        self.assertLess(weighted_bound(*args, .25), 1e-11)
        self.assertEqual(weighted_bound(*args, 0), 0)

    def test_quaternion_interpolates_components_then_normalizes(self):
        x = hermite(0, 2, 0, 0, 4, 0, 1)
        self.assertLess(angle(normalize((x, 0, 0, 1)), (1, 0, 0, 1)), 1e-14)
        with self.assertRaises(ValueError):
            normalize((0, 0, 0, 0))

    def test_angular_distance_and_sign_equivalence(self):
        self.assertEqual(angle((0, 0, 0, -2), (0, 0, 0, 3)), 0)
        self.assertAlmostEqual(angle((1, 0, 0, 0), (0, 0, 0, 1)), math.pi)
        self.assertAlmostEqual(angle((1, 0, 0, 1), (0, 0, 0, 1)), math.pi / 2)

    def test_euler_orders_with_noncommuting_turns(self):
        s = 2**-.5
        answers = ((0, s, 0, s), (s, s, 0, 0), (0, 0, s, s),
                   (0, s, s, 0), (s, 0, 0, s), (s, 0, s, 0))
        for order, answer in enumerate(answers):
            with self.subTest(order=order):
                self.assertLess(angle(euler((90, 90, 90), order), answer), 1e-14)
        self.assertLess(angle(euler((450, 90, 0), 4), (.5, .5, -.5, .5)), 1e-14)
        with self.assertRaises(ValueError):
            euler((0, 0, 0), 6)

    def test_muscle_limits_sign_and_axis_hypothesis(self):
        self.assertEqual([muscle_angle(x, -30, 10, -1) for x in (-.5, 0, .5)],
                         [5, 0, -15])
        identity = (0, 0, 0, 1)
        basis = (0, 0, 2**-.5, 2**-.5)
        inverse = (0, 0, -2**-.5, 2**-.5)
        self.assertLess(angle(joint_rotation(identity, basis, inverse, (90, 0, 0), 0),
                              (0, 2**-.5, 0, 2**-.5)), 1e-14)
        self.assertLess(angle(joint_rotation(identity, basis, inverse, (0, 0, 0), 0),
                              identity), 1e-14)
        self.assertIsNone(joint_rotation(None, basis, inverse, (0, 0, 0), 0, optional=True))
        with self.assertRaises(ValueError):
            joint_rotation(None, basis, inverse, (0, 0, 0), 0)

    def test_rest_components_remain_local_on_differently_proportioned_rigs(self):
        for y in (2, 7):
            rest = ((1, y, 3), (0, 0, 0, 1), (1, 1, 1))
            self.assertEqual(local_pose(rest, ((None, None, 4), None, (2, None, None))),
                             ((1, y, 4), (0, 0, 0, 1), (2, 1, 1)))

    def test_source_schedule_and_one_pass(self):
        self.assertEqual(schedule(2, 3.25, 2), (2, 2.5, 3, 3.25))
        self.assertEqual(schedule(2, 2, 2), (2,))
        self.assertEqual(schedule(2, 3, 2), (2, 2.5, 3))
        self.assertEqual(source_to_local(2, 3, 3), 1)
        with self.assertRaises(ValueError):
            source_to_local(2, 3, 4)
        with self.assertRaises(ValueError):
            schedule(2, 2, 0)

    def test_schedule_keeps_exact_f32_stop_once(self):
        stop = struct.unpack(">f", struct.pack(">f", 1 / 30))[0]
        self.assertEqual(schedule(0, stop, 30), (0, stop))
        self.assertEqual(schedule(2, 2 + stop, 30), (2, 2 + stop))

    def test_packed_times_accumulate_in_source_centiseconds(self):
        self.assertEqual(packed_times((5, 20, 100)), (.05, .25, 1.25))
        with self.assertRaises(ValueError):
            packed_times((2**63 - 1, 1))

    def test_frozen_bounds_scale_and_refuse_outside_envelope(self):
        self.assertGreater(transcription_bound("hermite", 128), transcription_bound("hermite", 1))
        self.assertGreater(bake_bound("translation", 8, 2, 1), bake_bound("translation", 1, 2, 1))
        self.assertGreater(bake_bound("rotation", 1, 2, 1), bake_bound("rotation", 1, 0, 1))
        with self.assertRaises(ValueError):
            bake_bound("translation", 1, 4097, 1)

    def test_angular_and_bake_rounding_budgets(self):
        self.assertEqual(ulp32(0), 2**-149)
        self.assertEqual(ulp32(8), 2**-20)
        self.assertAlmostEqual(transcription_angle_bound(.125, .5),
                               2 * math.pi / 3 + 256 * 2**-53)
        self.assertAlmostEqual(bake_bound("scale", 8, 2, 1),
                               1e-5 + 2**-30 + .500001 * 2**-20 + 2**-23)
