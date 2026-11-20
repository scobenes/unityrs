"""Generate and check the vectors holding the Rust body model's `python_sum` to CPython's compensated float sum.

Freeze v8 was fitted and validated under CPython 3.15.2, whose builtin `sum` compensates float additions
(Neumaier) from 3.12 on; the Rust model mirrors it. This writes `crates/unityrs-anim/tests/support/
python_sum.json`: random and adversarial float vectors (cancellation, huge and tiny mixes, subnormals, signed
zeros, overflow, infinities, NaN) with `sum`'s result, as f64 bit patterns. A NaN result is stored as "nan":
its sign and payload are the hardware's. Generating, and the check, need a compensating interpreter
(`compensated`); an older one skips the check and refuses to generate."""

import json
import random
import struct
import sys
import unittest
from pathlib import Path

PARITY = Path(__file__).resolve().parents[1]
OUTPUT = PARITY.parents[1] / "crates/unityrs-anim/tests/support/python_sum.json"
NAN = float("nan")
INF = float("inf")
EDGES = (0., -0., 1., -1., 1e16, -1e16, 1e308, -1e308, 1.7976931348623157e308, 5e-324, -5e-324, 2.2250738585072014e-308,
         1e-300, 0.1, 0.2, 0.3, 2. ** 53, 2. ** -53, 1. + 2. ** -52, INF, -INF, NAN)


def compensated():
    """Whether this interpreter's `sum` of floats is CPython 3.12+'s compensated one."""
    return sys.version_info >= (3, 12) and sum([1e16, 1.0, -1e16]) == 1.0 and sum([0.1] * 10) == 1.0


def bits(value):
    return f"{struct.unpack('<Q', struct.pack('<d', value))[0]:016x}"


def vectors():
    rng = random.Random(20261120)
    out = [[], [0.], [-0.], [-0., -0.], [-0., 0.], [1e16, 1., -1e16], [1e308, 1e308], [1e308, 1e308, -1e308],
           [INF, -INF], [INF, 1.], [NAN, 1.], [1., NAN], [1e308, 1e308, -INF], [.1] * 10, [1., 1e100, 1., -1e100],
           [5e-324] * 7, [2. ** 53, 1., 1.], [-2. ** 53, -1., -1.], [1e16, 1., -1e16, -1.]]
    for _ in range(400):
        out.append([rng.choice(EDGES) for _ in range(rng.randint(1, 6))])
    for _ in range(1200):
        scale = float(f"1e{rng.randint(-30, 30)}")
        values = [rng.uniform(-1, 1) * scale * float(f"1e{rng.randint(-8, 8)}") for _ in range(rng.randint(1, 12))]
        if rng.random() < .4:
            # Cancellation: the vector and its negation, shuffled with a small remainder.
            values += [-v for v in values] + [rng.uniform(-1, 1) * 1e-10 * scale]
            rng.shuffle(values)
        out.append(values)
    for _ in range(300):
        # Products of three, as the body model's dot products are.
        a = [rng.uniform(-2, 2) for _ in range(3)]
        b = [rng.uniform(-2, 2) for _ in range(3)]
        out.append([x * y for x, y in zip(a, b)])
    return out


def result(values):
    total = sum(values)
    return "nan" if total != total else bits(total)


def dump():
    if not compensated():
        raise SystemExit(f"python_sum vectors need CPython 3.12+'s compensated sum, not {sys.version.split()[0]}")
    rows = [dict(v=[bits(x) for x in values], sum=result(values)) for values in vectors()]
    lines = ["{", f'  "python": "{sys.version.split()[0]}",', '  "vectors": [']
    lines.append(",\n".join("    " + json.dumps(row) for row in rows))
    return "\n".join([*lines, "  ]", "}", ""])


@unittest.skipUnless(compensated(), "needs CPython 3.12+'s compensated float sum (CI's body-vectors job)")
class TestPythonSum(unittest.TestCase):
    def test_checked_in_vectors_are_this_interpreters_sum(self):
        self.assertEqual(OUTPUT.read_text(encoding="utf-8").split("\n", 2)[2], dump().split("\n", 2)[2])

    def test_the_vectors_cover_the_edges(self):
        stored = json.loads(OUTPUT.read_text(encoding="utf-8"))["vectors"]
        results = [row["sum"] for row in stored]
        self.assertIn("nan", results)
        self.assertIn(bits(INF), results)
        # From the integer 0, a sum of negative zeros is a positive zero.
        self.assertEqual(stored[2], dict(v=[bits(-0.)], sum=bits(0.)))
        self.assertGreater(len(stored), 1900)


if __name__ == "__main__":
    if sys.argv[1:] == ["--generate"]:
        OUTPUT.write_text(dump(), encoding="utf-8")
    else:
        unittest.main()
