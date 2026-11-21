"""Round 12's error arithmetic: a value with a radius bounding Unity's f32 result's distance from the exact one.

The limb-twist candidates (anim_twist_model) run their whole chain through these operations, so each chain bone's
prediction carries its own derived bound: never fitted, frozen before round 12 collects (round 12 design section 6).
Every operation states its assumption; the f64 value is the reference, and the radius covers Unity evaluating the
same equation in f32 (u = 2^-24), with its inputs' errors propagated:

- **A limit product** (`limit_angle`): the product rounds once (u|v L|); a value's own error moves the angle by at
  most the larger limit's magnitude times it, since the error can carry the value across 0.
- **Scalars** (`scale`, `add`): |a| e_b + |b| e_a + e_a e_b, then one rounding at the perturbed magnitudes.
- **tan(theta/2)** (`tan_half`): over the whole interval [theta - e, theta + e], the derivative's supremum at the
  outer end, plus one ulp (2u|tan|). An interval meeting a pole (|theta| + e >= pi) refuses (`Pole`).
- **sin, cos, atan** (`axis_angle`, `atan2_half`): one ulp each (2u|value|), the argument's error times a derivative
  bounded by 1.
- **Quaternion products** (`product`): |a| e_b + |b| e_a + e_a e_b, plus 2 gamma_4 (|a| + e_a)(|b| + e_b) of rounding:
  the perturbed norms, so both operands may carry error.
- **Normalizing** (`unit`): 2e/|q| + 4u (the exact norm; a sum of squares within gamma_4, sqrt and the division
  within u each); an axis (`unit3`) 2e/|v| + 3u + gamma_3; a norm not above its error refuses.
- **The angle** (`angle_bound`): 4 asin(e/2) plus `anim_replay.F64_ANGLE_FLOOR`, the one capture convention: Unity's
  recorded f32 components are compared as stored.
"""

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

U = 2.0 ** -24
GAMMA4 = 4 * U / (1 - 4 * U)
GAMMA3 = 3 * U / (1 - 3 * U)


class Pole(ValueError):
    """A tangent's interval meets a pole, or a quaternion's norm does not exceed its error: no bound exists."""


def norm(q):
    """The 2-norm with its sum's order written out (anim_muscle_pose.normalize's), for any Python's bits."""
    total = 0.
    for x in q:
        total = total + x * x
    return math.sqrt(total)


def multiply(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def conj(q):
    return (-q[0], -q[1], -q[2], q[3])


# A value and its radius: (value, error); a quaternion's value is a 4-tuple, its error a bound on the 2-norm.
def exact(value):
    return (value, 0.)


def limit_angle(axes, axis, value, value_error=0.):
    """Reference 2's angle on one axis (anim_muscle_pose.angles) with its radius."""
    limit = axes["max"][axis] if value >= 0 else -axes["min"][axis]
    theta = axes["sign"][axis] * (value * limit)
    moved = max(abs(axes["max"][axis]), abs(axes["min"][axis])) * value_error
    return (theta, abs(theta) * U + moved)


def scale(a, b):
    """a * b, both scalars with radii, rounded once."""
    (x, ex), (y, ey) = a, b
    return (x * y, abs(x) * ey + abs(y) * ex + ex * ey + U * (abs(x) + ex) * (abs(y) + ey))


def add(a, b):
    (x, ex), (y, ey) = a, b
    return (x + y, ex + ey + U * (abs(x) + ex + abs(y) + ey))


def one_minus(w):
    """1 - w for an exact f32 w: Unity's f32 subtraction rounds once."""
    value = 1. - w
    return (value, U * abs(value))


def tan_half(a):
    """tan(theta/2) over the whole input interval; a pole inside it refuses."""
    theta, error = a
    outer = abs(theta) + error
    if not math.isfinite(outer) or outer >= math.pi:
        raise Pole(f"tan(theta/2) meets a pole: |theta| + e = {outer!r}")
    value = math.tan(theta / 2)
    slope = (1 + math.tan(outer / 2) ** 2) / 2
    return (value, slope * error + 2 * U * abs(value))


def atan2_half(t):
    """2 atan(t): the inverse of `tan_half`, its derivative bounded by 2 (half angle) and one ulp."""
    value, error = t
    angle = 2 * math.atan(value)
    return (angle, 2 * error + 2 * U * abs(angle))


def product(a, b):
    (p, ep), (q, eq) = a, b
    np_, nq = norm(p), norm(q)
    rounding = 2 * GAMMA4 * (np_ + ep) * (nq + eq)
    return (multiply(p, q), np_ * eq + nq * ep + ep * eq + rounding)


def unit(a):
    q, error = a
    n = norm(q)
    if not n > error:
        raise Pole(f"a quaternion's norm {n!r} does not exceed its error {error!r}")
    return (tuple(x / n for x in q), 2 * error / n + 4 * U)


def unit3(a):
    v, error = a
    n = norm(v)
    if not n > error:
        raise Pole(f"an axis's norm {n!r} does not exceed its error {error!r}")
    return (tuple(x / n for x in v), 2 * error / n + 3 * U + GAMMA3)


def conjugate(a):
    q, error = a
    return (conj(q), error)


def rotate(q, v):
    """The vector q (v, 0) conj(q), normalized: two products and one normalization."""
    turned = product(product(q, ((v[0][0], v[0][1], v[0][2], 0.), v[1])), conjugate(q))
    return unit3((turned[0][:3], turned[1]))


def axis_angle(axis, phi):
    """T_axis(phi) = (sin(phi/2) axis, cos(phi/2)) for a unit axis with a radius."""
    (a, ea), (angle, e) = axis, phi
    half, eh = angle / 2, e / 2
    s, c = math.sin(half), math.cos(half)
    es, ec = eh + 2 * U * abs(s), eh + 2 * U * abs(c)
    # The three components s * a_i: |s| e_a + e_s |a| + e_s e_a, then their rounding.
    vector = abs(s) * ea + es + es * ea + U * (abs(s) + es) * (1 + ea)
    return ((s * a[0], s * a[1], s * a[2], c), vector + ec)


def joint(tangents):
    """B5a's joint, normalize((0, ty, tz, 1)(tx, 0, 0, 1)), from tangents with radii."""
    (tx, ex), (ty, ey), (tz, ez) = tangents
    return unit(product(((0., ty, tz, 1.), ey + ez), ((tx, 0., 0., 1.), ex)))


def local(rest, axes, joint_rotation):
    """B5a's local, normalize(rest normalize(pre J conj(post))), the f32 constants exact."""
    post = axes["post"]
    turned = product(product(exact(tuple(axes["pre"])), joint_rotation),
                     exact((-post[0], -post[1], -post[2], post[3])))
    return unit(product(exact(tuple(rest)), unit(turned)))


def angle_bound(error):
    return 4 * math.asin(min(1., error / 2)) + replay.F64_ANGLE_FLOOR
