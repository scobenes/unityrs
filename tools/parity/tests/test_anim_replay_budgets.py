"""Budget rows authored from synthetic bundle values, independent of Unity and Rust."""
import json
import math
import struct
import sys
import unittest
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402
import anim_replay_budgets as budgets  # noqa: E402
from anim_replay_bounds import budget_model  # noqa: E402
from test_anim_replay import synthetic  # noqa: E402

SEED, END = 0xff7fffff, 0x7f800000
SHOWS = HERE / "tests" / "anim_budget_show"
NODE = "-9223372036854775808"


def word(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


def frame(time_bits, *entries):
    words = [time_bits, len(entries)]
    for slot, coefficients in entries:
        words += [slot, *map(word, coefficients)]
    return words


def compiled(frames, constants=(), bindings=None, dense=0, curves=1):
    """A compiled clip whose slot 0 is a weight and whose constants are translation."""
    bindings = bindings or [dict(path=0, attribute=zlib.crc32(b"blendShape.Lift"), typeID=137, customType=20,
                                 isPPtrCurve=0),
                            dict(path=0, attribute=1, typeID=4, customType=0, isPPtrCurve=0)]
    data = [w for f in frames for w in f] + [END, 0]
    playback = dict(m_StreamedClip=dict(data=data, curveCount=curves),
                    m_DenseClip=dict(m_CurveCount=dense), m_ConstantClip=dict(data=list(constants)))
    return dict(m_MuscleClip=dict(m_Clip=dict(data=playback)),
                m_ClipBindingConstant=dict(genericBindings=bindings))


def held_weight_clip():
    return compiled([frame(SEED, (0, (0, 0, 0, 12.5))), frame(word(2), (0, (0, 0, 0, 12.5))),
                     frame(word(2.5), (0, (0, 0, 0, 17.5))), frame(word(3), (0, (0, 0, 0, 17.5)))],
                    constants=(1, -2, 0))


class FakeBundles:
    values = {}

    def __init__(self, spec, folder, unityrs):
        pass

    def value(self, ident):
        return self.values[ident["path_id"]]


def values(clip):
    return {"2": clip, "7": dict(m_BlendShapeWeights=[12.5]), "8": dict(m_BlendShapeWeights=[-4]),
            NODE: dict(m_LocalPosition=dict(x=0, y=0, z=0), m_LocalRotation=dict(x=0, y=0, z=0, w=1),
                       m_LocalScale=dict(x=1, y=1, z=1))}


def authored(clip, manifest=None):
    manifest = manifest or synthetic()
    FakeBundles.values = values(clip)
    original = budgets.Bundles
    budgets.Bundles = FakeBundles
    try:
        return budgets.author(manifest, None, "unityrs")
    finally:
        budgets.Bundles = original


def legacy():
    manifest = synthetic()
    manifest["spec"]["mode"] = "legacy"
    return manifest


def euler_clip(keys):
    """A clamped legacy Euler curve on the root, with flat tangents."""
    flat = dict(x=0, y=0, z=0)
    curve = dict(m_Curve=[dict(time=t, value=dict(zip("xyz", v)), inSlope=flat, outSlope=flat) for t, v in keys],
                 m_PreInfinity=2, m_PostInfinity=2, m_RotationOrder=4)
    return dict(m_EulerCurves=[dict(path="", curve=curve)])


def held(boxes, q):
    """Whether some box holds the quaternion, up to its sign."""
    return any(all(inside(s * x, b) for x, b in zip(q, box)) for box in boxes for s in (1, -1))


def row(manifest, prop, target):
    return next(r for r in manifest["spec"]["budgets"] if (r["property"], r["target"]) == (prop, target))


def inside(value, interval):
    return interval[0] <= value <= interval[1]


class AuthoredBudgets(unittest.TestCase):
    def test_every_target_gets_a_row_that_derive_accepts(self):
        m = authored(held_weight_clip())
        weight = row(m, "weight", 0)
        self.assertEqual((weight["track_kind"], weight["weight_units"]), ("held", "editor"))
        [[lo, hi]] = weight["stages"][0]["range"]
        self.assertTrue(lo < 12.5 and 17.5 < hi and hi - lo < 5.001)
        translation = row(m, "translation", 0)
        self.assertEqual(translation["track_kind"], "constant")
        self.assertTrue(all(inside(x, translation["stages"][0]["range"][0]) for x in (1, -2, 0)))
        rotation = row(m, "rotation", 0)
        self.assertEqual(rotation["track_kind"], "rest")
        [box] = rotation["stages"][0]["norm_ranges"]
        self.assertTrue(all(inside(x, b) for x, b in zip((0, 0, 0, 1), box)))
        self.assertEqual(len(replay.derive(m)["samples"]), 4)

    def test_a_held_segment_of_a_streamed_slot_is_streamed(self):
        clip = compiled([frame(SEED, (0, (0, 0, 0, 12.5))), frame(word(2), (0, (0, 0, 10, 12.5))),
                         frame(word(2.5), (0, (0, 0, 0, 17.5))), frame(word(3), (0, (0, 0, 0, 17.5)))],
                        constants=(1, -2, 0))
        ranges = row(authored(clip), "weight", 0)
        self.assertEqual(ranges["track_kind"], "streamed")
        c3, c2, c1, c0, d = ranges["stages"][0]["range"]
        # The held value is read as c0, like a clamped one.
        self.assertTrue(inside(10, c1) and inside(12.5, c0) and inside(17.5, c0))
        # Padding keeps the key-relative time nonnegative.
        self.assertTrue(d[0] == 0 and inside(.5, d))

    def test_an_unverified_seed_is_refused(self):
        def clip(seed):
            return compiled([frame(SEED, (0, (0, 0, 0, seed))), frame(word(2.25), (0, (0, 0, 0, 12.5))),
                             frame(word(3), (0, (0, 0, 0, 17.5)))], constants=(1, -2, 0))
        # Playback starts before the first key, where only a seed that
        # repeats that key's value is the built pattern.
        self.assertEqual(row(authored(clip(12.5)), "weight", 0)["track_kind"], "held")
        with self.assertRaisesRegex(ValueError, "seed"):
            authored(clip(1))

    def test_dense_storage_and_unclaimed_bindings_are_refused(self):
        with self.assertRaisesRegex(ValueError, "dense"):
            authored(compiled([], dense=1))
        clip = held_weight_clip()
        clip["m_ClipBindingConstant"]["genericBindings"][1]["path"] = zlib.crc32(b"Elsewhere")
        with self.assertRaisesRegex(ValueError, "no recorded target"):
            authored(clip)

    def test_euler_hermite_boxes_hold_independent_quaternions(self):
        m = authored(euler_clip([(2, (0, 0, 0)), (3, (10, 0, 90))]), legacy())
        rotation = row(m, "rotation", 0)
        self.assertEqual(rotation["track_kind"], "euler_hermite")
        joint = rotation["stages"][1]
        self.assertEqual(joint["stage"], "joint")
        self.assertTrue(inside(0, joint["range"][0]) and inside(90, joint["range"][0]))
        for t in (0, .3, .5, .99, 1):
            u = t * t * (3 - 2 * t)
            # Order 4 turns about Z, then X: the product qx * qz in closed form.
            sx, cx = math.sin(math.radians(5 * u)), math.cos(math.radians(5 * u))
            sz, cz = math.sin(math.radians(45 * u)), math.cos(math.radians(45 * u))
            self.assertTrue(held(joint["norm_ranges"], (sx * cz, -sx * sz, cx * sz, cx * cz)), t)
        budget_model(row(m, "rotation", 0))

    def test_a_constant_angle_on_a_trig_zero_is_enclosed(self):
        # Half of 3060 degrees is 1530, a quarter turn past four whole turns.
        m = authored(euler_clip([(2, (0, 0, 3060)), (3, (0, 0, 3060))]), legacy())
        joint = row(m, "rotation", 0)["stages"][1]
        self.assertTrue(all(held([box], (0, 0, 1, 0)) for box in joint["norm_ranges"]))
        budget_model(row(m, "rotation", 0))

    def test_boxes_subdivide_a_fast_turn_until_each_excludes_the_origin(self):
        m = authored(euler_clip([(2, (0, 0, 0)), (3, (0, 0, 3000))]), legacy())
        boxes = row(m, "rotation", 0)["stages"][1]["norm_ranges"]
        self.assertGreater(len(boxes), 8)
        self.assertTrue(all(max(max(0, lo, -hi) for lo, hi in box) > .3 for box in boxes))
        for i in range(101):
            t = i / 100
            half = math.radians(1500 * t * t * (3 - 2 * t))
            self.assertTrue(held(boxes, (0, 0, math.sin(half), math.cos(half))), t)
        budget_model(row(m, "rotation", 0))

    def test_a_held_jump_at_the_stop_has_its_own_box(self):
        # The stepped key's tangent arrives in show's tagged f32 encoding.
        infinity = dict(t="f32", v="7f800000")
        keys = [dict(time=2, value=dict(x=0, y=0, z=0, w=1), inSlope=dict(x=0, y=0, z=0, w=0),
                     outSlope=dict(x=infinity, y=infinity, z=infinity, w=infinity)),
                dict(time=3, value=dict(x=0, y=0, z=1, w=0), inSlope=dict(x=0, y=0, z=0, w=0),
                     outSlope=dict(x=0, y=0, z=0, w=0))]
        curve = dict(m_Curve=keys, m_PreInfinity=2, m_PostInfinity=2, m_RotationOrder=4)
        m = authored(dict(m_RotationCurves=[dict(path="", curve=curve)]), legacy())
        rotation = row(m, "rotation", 0)
        self.assertEqual(rotation["track_kind"], "held")
        boxes = rotation["stages"][0]["norm_ranges"]
        self.assertTrue(held(boxes, (0, 0, 0, 1)) and held(boxes, (0, 0, 1, 0)))
        budget_model(rotation)

    def test_a_raw_path_near_the_origin_is_refused(self):
        # W runs linearly from 1 to -1 beside an X of 2^-14, so the raw norm
        # falls to 2^-14 at 2.5 s.
        tiny = 2.**-14
        keys = [dict(time=2, value=dict(x=tiny, y=0, z=0, w=1), inSlope=dict(x=0, y=0, z=0, w=-2),
                     outSlope=dict(x=0, y=0, z=0, w=-2)),
                dict(time=3, value=dict(x=tiny, y=0, z=0, w=-1), inSlope=dict(x=0, y=0, z=0, w=-2),
                     outSlope=dict(x=0, y=0, z=0, w=-2))]
        curve = dict(m_Curve=keys, m_PreInfinity=2, m_PostInfinity=2, m_RotationOrder=4)
        with self.assertRaisesRegex(ValueError, "rotation path too close to the origin to bound"):
            authored(dict(m_RotationCurves=[dict(path="", curve=curve)]), legacy())

    def test_a_zero_rest_rotation_is_refused(self):
        clip = held_weight_clip()
        FakeBundles.values = values(clip)
        FakeBundles.values[NODE]["m_LocalRotation"] = dict(x=0, y=0, z=0, w=0)
        original = budgets.Bundles
        budgets.Bundles = FakeBundles
        try:
            with self.assertRaisesRegex(ValueError, "too close to the origin"):
                budgets.author(synthetic(), None, "unityrs")
        finally:
            budgets.Bundles = original

    def test_weighted_scalar_curves_are_refused(self):
        key = dict(time=2, value=1, inSlope=0, outSlope=0, weightedMode=3)
        curve = dict(m_Curve=[key], m_PreInfinity=2, m_PostInfinity=2)
        clip = dict(m_FloatCurves=[dict(path="", attribute="blendShape.Lift", classID=137, curve=curve)])
        with self.assertRaisesRegex(ValueError, "weighted"):
            authored(clip, legacy())
        key["weightedMode"], curve["m_PostInfinity"] = 0, 1
        with self.assertRaisesRegex(ValueError, "clamped"):
            authored(clip, legacy())

    def test_each_component_has_its_own_tolerance(self):
        # X ramps from 0 to 1 beside a Y held at 2^20.
        large = 2.**20
        clip = compiled([frame(word(2), (0, (0, 0, 0, 12.5)), (1, (0, 0, 1, 0)), (2, (0, 0, 0, large)),
                               (3, (0, 0, 0, 0))),
                         frame(word(3), (0, (0, 0, 0, 17.5)), (1, (0, 0, 0, 1)), (2, (0, 0, 0, large)))],
                        curves=4)
        m = authored(clip)
        derived = replay.derive(m)
        translation = [sample["bounds"][0] for sample in derived["samples"]]
        # A replay leaving X at 0 errs by 1 at the stop, far past X's bound.
        self.assertTrue(all(x < 1e-3 and y > 2**-5 for x, y, _ in translation))
        # A vector row without its own component ranges is refused, never
        # bounded by the envelope its components share.
        del row(m, "translation", 0)["component_ranges"]
        with self.assertRaisesRegex(ValueError, "no schema alternative"):
            replay.derive(m)

    def test_verify_refuses_rows_widened_after_the_capture(self):
        m = authored(held_weight_clip())
        FakeBundles.values = values(held_weight_clip())
        original = budgets.Bundles
        budgets.Bundles = FakeBundles
        try:
            budgets.verify(json.loads(json.dumps(m)), None, "unityrs")
            row(m, "weight", 0)["stages"][0]["range"][0][1] += 1
            with self.assertRaisesRegex(ValueError, "differs from the bundle derivation"):
                budgets.verify(m, None, "unityrs")
        finally:
            budgets.Bundles = original

class RecordedShow(unittest.TestCase):
    """Saved `unityrs show --value canonical` documents of a synthetic legacy bundle.

    The testkit built the bundle: an Euler curve stepped from 0 to 90 degrees
    about Z at 2.25 s, whose infinite tangents show tags as f32 bits.
    """

    SHA256 = "857374a9815ef2567dc6bb466b51cb5b295e1398f246c88a30a2ec554a8413f5"

    def spec(self):
        def ident(path_id):
            return dict(bundle=0, file_index=2, file="CAB-synthetic", path_id=str(path_id))
        channels = [dict(index=0, name="bend", full_weights=[25, 80])]
        return dict(mode="legacy", evidence={}, start=2, stop=3, binding_address="", clip=ident(9),
                    bundles=[dict(name="synthetic", sha256=self.SHA256)],
                    nodes=[dict(address="", name="root", id=ident(2)), dict(address="0", name="instance", id=ident(5))],
                    renderers=[dict(address="", id=ident(6), channels=channels),
                               dict(address="0", id=ident(8), channels=channels)])

    def test_the_cli_encoding_authors_a_stepped_euler_row(self):
        m = budgets.author(dict(spec=self.spec()), SHOWS / "stepped", None)
        rotation = row(m, "rotation", 0)
        self.assertEqual(rotation["track_kind"], "euler_held")
        boxes = rotation["stages"][1]["norm_ranges"]
        half = math.radians(45)
        self.assertTrue(held(boxes, (0, 0, 0, 1)) and held(boxes, (0, 0, math.sin(half), math.cos(half))))
        budget_model(rotation)
        # The serialized f32s blend-shape weight is the untouched instance's rest.
        [[lo, hi]] = row(m, "weight", 1)["stages"][0]["range"]
        self.assertTrue(lo < 30 < hi and hi - lo < 1e-9)

    def test_a_truncated_or_misplaced_show_document_is_refused(self):
        for field, value, reason in (("truncated", "items", "truncated show"),
                                     ("object", "file_index", "unexpected show document")):
            with self.subTest(field=field):
                folder = scratch.for_case(self)
                for path in (SHOWS / "stepped").iterdir():
                    doc = json.loads(path.read_text(encoding="utf-8"))
                    doc[field][value] += 1
                    (folder / path.name).write_text(json.dumps(doc, ensure_ascii=False), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, reason):
                    budgets.author(dict(spec=self.spec()), folder, None)

    def test_a_mismatched_pin_is_refused(self):
        spec = self.spec()
        spec["bundles"][0]["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            budgets.author(dict(spec=spec), SHOWS / "stepped", None)


if __name__ == "__main__":
    unittest.main()
