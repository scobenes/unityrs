"""Generate and check the Rust sampler's independent f64 replay records."""

import json
import math
import sys
import unittest
from pathlib import Path

PARITY = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PARITY))

import anim_reference as r  # noqa: E402

OUTPUT = PARITY.parents[1] / "crates/unityrs-anim/tests/support/clip_expected.json"


def component(equation, value, scale=0):
    return {"equation": equation, "value": value, "scale": scale}


def constants(values):
    return [component("constant", value) for value in values]


def clock(source):
    return {"source": source, "local": r.source_to_local(2, 6, source)}


def explicit():
    for t in [2, 2.25, 3, 4, 5.99, 6]:
        position = []
        for a, b, outgoing, incoming in [(2, 10, 6, -2), (-2, 8, 0, 0), (3, 4, math.inf, 0)]:
            value = r.hermite(2, 6, a, b, outgoing, incoming, t)
            if t in (2, 6):
                position.append(component("exact", value))
            elif math.isinf(outgoing):
                position.append(component("constant", value))
            else:
                scale = 16 * (abs(a) + abs(b) + 4 * (abs(outgoing) + abs(incoming)))
                position.append(component("hermite", value, scale))
        degrees = tuple(r.hermite(2, 6, a, b, 0, 0, t) for a, b in [(10, 370), (20, -80), (30, 60)])
        rotation = component("joint", r.euler(degrees, 5), 8 * (1 + 370 * math.pi / 360))
        # Forward the Hermite input error (largest envelope over the three
        # axes) with the joint gain 3*pi/360. Express it in joint-scale units.
        rotation["scale"] += (3 * math.pi / 360 * r.transcription_bound("hermite", 16 * (10 + 370))
                              / r.transcription_bound("joint", 1))
        args = (2, 6, 4, 60, 20, -8, .25, 1 / 3, t)
        weight = {"equation": "weighted", "value": r.weighted(*args), "bound": r.weighted_bound(*args)}
        yield {**clock(t), "position": position, "scale": constants([8, 9, 10]),
               "rotation": rotation, "weight": weight}


def streamed():
    keys = [(3, (2, -1, 3, 4)), (5, (9, 8, 7, -2))]
    for t in [2, 3, 3.125, 4, 5, 6]:
        x = component("streamed" if 3 < t < 5 else "constant", r.streamed(keys, t), 30)
        yield {**clock(t), "position": [x, *constants([5, 6])], "scale": constants([2, 3, 4]),
               "rotation": component("constant", [0, 0, 0, 1])}


def dense():
    values = [0, 0, 0, 1, 4, 0, 0, 1, 1, 20, 0, 0, 1, 0, 84]
    for t in [2, 3, 3.125, 3.25, 3.375, 6]:
        def slot(i):
            return r.dense(values, 5, i, 3, 4, t)
        # Conservative common scale from the adjacent sample magnitudes,
        # the largest adjacent difference, and the entire grid/time span.
        scale = 2 * 84 + 64 * (1 + 4 * (6 + 3 + .5))
        raw = tuple(slot(i) for i in range(4))
        rotation = {**component("dense", r.normalize(raw), scale), "raw": raw}
        yield {**clock(t), "position": constants([9, 8, 7]), "scale": constants([8, 9, 10]),
               "rotation": rotation,
               "weight": component("dense", slot(4), scale)}


def dump():
    groups = [("explicit", explicit()), ("streamed", streamed()), ("dense", dense())]
    lines = ['{', f'  "version": {r.BOUNDS["version"]},']
    for index, (name, samples) in enumerate(groups):
        lines.append(f'  "{name}": [')
        lines.append(",\n".join("    " + json.dumps(row, allow_nan=False) for row in samples))
        lines.append("  ]," if index < len(groups) - 1 else "  ]")
    return "\n".join([*lines, "}", ""])


class TestSyntheticSamplerReplay(unittest.TestCase):
    def test_explicit_hermite_endpoints_equal_the_serialized_keys(self):
        rows = list(explicit())
        for row, source, xyz in ((rows[0], 2, (2, -2, 3)), (rows[-1], 6, (10, 8, 4))):
            self.assertEqual(row["source"], source)
            self.assertEqual(row["position"], [component("exact", value) for value in xyz])

    def test_checked_in_samples_come_from_the_reference(self):
        stored = json.loads(OUTPUT.read_text(encoding="utf-8"))
        generated = json.loads(dump())
        # Platform libm implementations may round trig and normalization
        # differently. Check rotations with the same frozen angular metric.
        floor = r.BOUNDS["transcription"]["min_raw_quaternion_norm"]
        for group in ("explicit", "streamed", "dense"):
            for left, right in zip(stored[group], generated[group], strict=True):
                a, b = left["rotation"], right["rotation"]
                error = r.transcription_bound(b["equation"], b["scale"])
                norm = max(floor, math.hypot(*b.get("raw", b["value"])))
                self.assertLessEqual(r.angle(a.pop("value"), b.pop("value")),
                                     r.transcription_angle_bound(2 * error, norm))
        self.assertEqual(stored, generated)


if __name__ == "__main__":
    if sys.argv[1:] == ["--generate"]:
        OUTPUT.write_text(dump(), encoding="utf-8")
    else:
        unittest.main()
