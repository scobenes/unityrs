"""Generate and check the Rust body solve's f64 reference vectors (freeze v8, synthetic rigs only).

Each case gives a synthetic rig's rest points and a posed body in the hips frame, its weights and edges,
and a requested placement, all as f64 bit patterns; the expected centre, frames, twist, body rotation and
hips solve are the reference's (anim_body_model) with freeze v8's coefficients. The inputs are made with
products and square roots only, so they are the same on every platform; the check recomputes the outputs
from the stored inputs. Freeze v8 was fitted and validated under CPython 3.15.2, whose float `sum` is
compensated (from 3.12 on), and the reference's dot products and norms use `sum`: generating needs exactly
CPython 3.15.2, which the vectors record (test_anim_python_sum.canonical), and the checks skip under an
interpreter without a compensated sum (test_anim_python_sum.compensated) and hold the recorded version to
3.15.2. CI's body-vectors job runs this folder under 3.15.2; the parity suite (tests/, under 3.11) never
discovers it. Only the twist (the platform's `atan2`) is compared within ulps."""

import json
import math
import random
import struct
import sys
import unittest
from pathlib import Path

PARITY = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PARITY))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_body_model as model  # noqa: E402
import anim_body_tolerance as tolerance  # noqa: E402
from test_anim_python_sum import CANONICAL, canonical, compensated  # noqa: E402

OUTPUT = PARITY.parents[1] / "crates/unityrs-anim/tests/support/body_vectors.json"
FREEZE = PARITY / "anim_body_freeze_v8.json"
COEFFICIENTS = json.loads(FREEZE.read_text(encoding="utf-8"))["coefficients"]

# HumanTrait's parents of the body slots.
PARENT = {"Hips": None, "LeftUpperLeg": "Hips", "RightUpperLeg": "Hips", "LeftLowerLeg": "LeftUpperLeg",
          "RightLowerLeg": "RightUpperLeg", "LeftFoot": "LeftLowerLeg", "RightFoot": "RightLowerLeg",
          "Spine": "Hips", "Chest": "Spine", "UpperChest": "Chest", "Neck": "UpperChest", "Head": "Neck",
          "LeftShoulder": "UpperChest", "RightShoulder": "UpperChest", "LeftUpperArm": "LeftShoulder",
          "RightUpperArm": "RightShoulder", "LeftLowerArm": "LeftUpperArm", "RightLowerArm": "RightUpperArm",
          "LeftHand": "LeftLowerArm", "RightHand": "RightLowerArm", "LeftToes": "LeftFoot", "RightToes": "RightFoot",
          "LeftEye": "Head", "RightEye": "Head", "Jaw": "Head"}
# A dyadic T-pose (the testkit's), the Eyes and the Jaw weightless.
T_POSE = {"Hips": (0, 1, 0), "LeftUpperLeg": (-.125, .875, 0), "RightUpperLeg": (.125, .875, 0),
          "LeftLowerLeg": (-.125, .5, 0), "RightLowerLeg": (.125, .5, 0), "LeftFoot": (-.125, .125, 0),
          "RightFoot": (.125, .125, 0), "Spine": (0, 1.125, 0), "Chest": (0, 1.25, 0), "UpperChest": (0, 1.375, 0),
          "Neck": (0, 1.5, 0), "Head": (0, 1.625, 0), "LeftShoulder": (-.0625, 1.4375, 0),
          "RightShoulder": (.0625, 1.4375, 0), "LeftUpperArm": (-.1875, 1.4375, 0),
          "RightUpperArm": (.1875, 1.4375, 0), "LeftLowerArm": (-.4375, 1.4375, 0),
          "RightLowerArm": (.4375, 1.4375, 0), "LeftHand": (-.6875, 1.4375, 0), "RightHand": (.6875, 1.4375, 0),
          "LeftToes": (-.125, 0, .125), "RightToes": (.125, 0, .125), "LeftEye": (-.03125, 1.6875, .0625),
          "RightEye": (.03125, 1.6875, .0625), "Jaw": (0, 1.5625, .0625)}
MASSES = {name: 0. if name in ("LeftEye", "RightEye", "Jaw") else (2 + i % 7) / 128
          for i, name in enumerate(tolerance.NAMES)}
# Rig variants whose every edge freeze v8 fitted.
VARIANTS = {"full": (), "no-upper-chest": ("UpperChest",), "no-shoulders": ("LeftShoulder", "RightShoulder"),
            "no-toes": ("LeftToes", "RightToes"), "no-upper-chest-toes": ("UpperChest", "LeftToes", "RightToes"),
            "no-leaves": ("LeftEye", "RightEye", "Jaw")}
POSES = 4
UNDER = ("LeftUpperArm", "LeftLowerArm", "LeftHand")


def bits(value):
    return f"{struct.unpack('<Q', struct.pack('<d', float(value)))[0]:016x}"


def unbits(text):
    return struct.unpack("<d", struct.pack("<Q", int(text, 16)))[0]


def dyadic(rng, scale, exponent=10):
    """A uniform value in [-scale, scale] on a 2^-exponent grid."""
    return round(rng.uniform(-scale, scale) * 2 ** exponent) / 2 ** exponent


def turn(rng, size):
    """A unit quaternion near the identity, w > 0, from products and a square root."""
    x, y, z = (dyadic(rng, size) for _ in range(3))
    w = 1.
    length = math.sqrt(x * x + y * y + z * z + w * w)
    return tuple(v / length for v in (x, y, z, w))


def case(rng, variant, absent, pose, refuse=False):
    mapped = [name for name in tolerance.NAMES if name not in absent]
    links = tolerance.edges(PARENT, mapped)
    assert all(f"{i}>{j}" in COEFFICIENTS for i, j in links), variant
    total = sum(MASSES[name] for name in mapped)
    weights = {name: MASSES[name] / total for name in mapped}
    size = 1 + dyadic(rng, .25, 6)
    rest = {name: tuple(size * (a - b) + dyadic(rng, .002, 14) for a, b in zip(T_POSE[name], T_POSE["Hips"]))
            for name in mapped}
    rest["Hips"] = (0., 0., 0.)
    spin = turn(rng, .6)
    local = {name: tuple(c + dyadic(rng, .0005, 16) for c in tolerance.rotate(spin, p)) for name, p in rest.items()}
    local["Hips"] = (0., 0., 0.)
    if refuse:
        # The left arm moved forward: a twist well over the limit.
        local |= {name: tolerance.rotate(spin, (rest[name][0], rest[name][1], rest[name][2] + .3)) for name in UNDER}
    out = dict(name=f"{variant}/{pose}", weights={n: bits(v) for n, v in weights.items()},
               links=[list(link) for link in links],
               rest={n: [bits(c) for c in p] for n, p in rest.items()},
               local={n: [bits(c) for c in p] for n, p in local.items()},
               target=[bits(dyadic(rng, 2)) for _ in range(3)], rotation=[bits(v) for v in turn(rng, 2)],
               human_scale=bits(1 + dyadic(rng, .5, 6)),
               parent=dict(position=[bits(dyadic(rng, 3)) for _ in range(3)],
                           rotation=[bits(v) for v in turn(rng, 1.5)], scale=bits(2 ** rng.randint(-2, 2))),
               hips_scale=bits(1 + dyadic(rng, .5, 6)))
    out["expect"] = expect(out)
    return out


def inputs(entry):
    point = lambda p: tuple(unbits(c) for c in p)  # noqa: E731
    parent = entry["parent"]
    frame = tolerance.Frame(point(parent["position"]), point(parent["rotation"]), unbits(parent["scale"]), 0., 0)
    return dict(weights={n: unbits(v) for n, v in entry["weights"].items()},
                links=[tuple(link) for link in entry["links"]],
                rest={n: point(p) for n, p in entry["rest"].items()},
                local={n: point(p) for n, p in entry["local"].items()},
                target=point(entry["target"]), rotation=point(entry["rotation"]),
                human_scale=unbits(entry["human_scale"]), parent=frame, hips_scale=unbits(entry["hips_scale"]))


def expect(entry):
    """The reference's outputs for `entry`'s stored inputs."""
    x = inputs(entry)
    rest_frame = tolerance.body_frame(x["rest"])
    found = dict(rest_frame=[[bits(v) for v in row] for row in rest_frame],
                 centre=[bits(v) for v in model.centre(x["local"], x["weights"], x["links"], COEFFICIENTS)],
                 frame=[[bits(v) for v in row] for row in tolerance.body_frame(x["local"])],
                 twist=bits(model.shoulder_hip_twist(x["local"])))
    try:
        rotation = model.body_rotation(x["local"], rest_frame)
        t, q = model.solve_hips(x["target"], x["rotation"], x["human_scale"], x["local"], x["weights"],
                                x["links"], COEFFICIENTS, rest_frame, x["parent"], x["hips_scale"])
    except model.Refused as refused:
        assert "shoulder-hip twist" in str(refused) and "undefined" not in str(refused), refused
        return found | dict(refused="shoulder_hip_twist")
    return found | dict(body_rotation=[bits(v) for v in rotation], t=[bits(v) for v in t],
                        q=[bits(v) for v in q])


def dump():
    """The vectors' file, recording CPython 3.15.2, with this interpreter's results."""
    rng = random.Random(20261120)
    cases = [case(rng, variant, absent, pose) for variant, absent in VARIANTS.items() for pose in range(POSES)]
    cases += [case(rng, "full", (), "twisted", refuse=True), case(rng, "no-toes", VARIANTS["no-toes"], "twisted",
                                                                  refuse=True)]
    lines = ["{", f'  "python": "{CANONICAL}",', f'  "freeze": "{FREEZE.name}",', '  "cases": [']
    lines.append(",\n".join("    " + json.dumps(entry, allow_nan=False) for entry in cases))
    return "\n".join([*lines, "  ]", "}", ""])


@unittest.skipUnless(compensated(), "needs CPython 3.12+'s compensated float sum (CI's body-vectors job)")
class TestBodyVectors(unittest.TestCase):
    def test_the_vectors_cover_solves_and_refusals(self):
        cases = json.loads(OUTPUT.read_text(encoding="utf-8"))["cases"]
        refused = [entry["name"] for entry in cases if "refused" in entry["expect"]]
        self.assertEqual(refused, ["full/twisted", "no-toes/twisted"])
        self.assertEqual(len(cases), len(VARIANTS) * POSES + 2)
        for entry in cases:
            twist = unbits(entry["expect"]["twist"])
            self.assertEqual(twist > model.TWIST_LIMIT, "refused" in entry["expect"], entry["name"])

    def test_checked_in_vectors_come_from_the_reference(self):
        stored = json.loads(OUTPUT.read_text(encoding="utf-8"))
        self.assertEqual(stored["python"], CANONICAL)
        self.assertEqual(stored["freeze"], FREEZE.name)
        for entry in stored["cases"]:
            found, want = expect(entry), dict(entry["expect"])
            # atan2 is the platform's: the twist within four ulps.
            a, b = unbits(found.pop("twist")), unbits(want.pop("twist"))
            self.assertLessEqual(abs(a - b), 4 * abs(b) * tolerance.U64 * 2, entry["name"])
            self.assertEqual(found, want, entry["name"])

    def test_the_inputs_regenerate(self):
        stored = json.loads(OUTPUT.read_text(encoding="utf-8"))["cases"]
        for left, right in zip(stored, json.loads(dump())["cases"], strict=True):
            left, right = dict(left), dict(right)
            left.pop("expect"), right.pop("expect")
            self.assertEqual(left, right)


if __name__ == "__main__":
    if sys.argv[1:] == ["--generate"]:
        canonical()
        OUTPUT.write_text(dump(), encoding="utf-8")
    else:
        unittest.main()
