"""Budget rows authored from synthetic bundle values, independent of Unity and Rust."""
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
import anim_replay_budgets as budgets  # noqa: E402
from test_anim_replay import synthetic  # noqa: E402

SEED, END = 0xff7fffff, 0x7f800000
NODE = "-9223372036854775808"


def word(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


def frame(time_bits, *entries):
    words = [time_bits, len(entries)]
    for slot, coefficients in entries:
        words += [slot, *map(word, coefficients)]
    return words


def compiled(frames, constants=(), bindings=None, dense=0):
    """A compiled clip whose slot 0 is a weight and whose constants are translation."""
    bindings = bindings or [dict(path=0, attribute=zlib.crc32(b"blendShape.Lift"), typeID=137, customType=20,
                                 isPPtrCurve=0),
                            dict(path=0, attribute=1, typeID=4, customType=0, isPPtrCurve=0)]
    data = [w for f in frames for w in f] + [END, 0]
    playback = dict(m_StreamedClip=dict(data=data, curveCount=1),
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

    def test_euler_hermite_boxes_hold_the_curve_quaternions(self):
        keys = [dict(time=2, value=dict(x=0, y=0, z=0), inSlope=dict(x=0, y=0, z=0),
                     outSlope=dict(x=0, y=0, z=0)),
                dict(time=3, value=dict(x=10, y=0, z=90), inSlope=dict(x=0, y=0, z=0),
                     outSlope=dict(x=0, y=0, z=0))]
        curve = dict(m_Curve=keys, m_PreInfinity=2, m_PostInfinity=2, m_RotationOrder=4)
        clip = dict(m_EulerCurves=[dict(path="", curve=curve)])
        manifest = synthetic()
        manifest["spec"]["mode"] = "legacy"
        m = authored(clip, manifest)
        rotation = row(m, "rotation", 0)
        self.assertEqual(rotation["track_kind"], "euler_hermite")
        joint = rotation["stages"][1]
        self.assertEqual(joint["stage"], "joint")
        self.assertTrue(inside(0, joint["range"][0]) and inside(90, joint["range"][0]))
        self.assertEqual(len(joint["norm_ranges"]), budgets.PIECES)
        for t in (0, .3, .5, .99, 1):
            u = t * t * (3 - 2 * t)
            q = budgets.euler(budgets.EULER_ORDERS[4], (10 * u, 0, 90 * u))
            self.assertTrue(any(all(inside(x, b) for x, b in zip(q, box)) for box in joint["norm_ranges"]), t)
        self.assertAlmostEqual(math.fsum(x * x for x in q), 1)


if __name__ == "__main__":
    unittest.main()
