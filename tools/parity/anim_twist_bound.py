"""Round 12's error arithmetic: a value with a radius bounding Unity's f32 result's distance from the exact one.

The limb-twist candidates (anim_twist_model) run their whole chain through these operations, so each chain bone's
prediction carries its own derived bound: never fitted, frozen before round 12 collects (round 12 design section 6).
Every operation states its assumption; the f64 value is the reference, and the radius covers Unity evaluating the
same equation in f32 (u = 2^-24), with its inputs' errors propagated:

Every rounding allowance is charged at the input interval's worst endpoint (the largest magnitude any input within
its radius gives), never at the nominal value: an f32 operation on a perturbed input rounds at that input. Each
also carries one subnormal ulp (2^-149, `ulps`), so a result near the subnormal range is covered absolutely; an exact
zero carries none.

- **A limit product** (`limit_angle`): a value's own error moves the angle by at most the larger limit's magnitude
  times it, since the error can carry the value across 0; the product rounds once at the perturbed angle,
  u (|v L| + that move).
- **Scalars** (`scale`, `add`, `sub`): |a| e_b + |b| e_a + e_a e_b (a sum: e_a + e_b), then one rounding at the
  perturbed magnitude. A share 1 - w is `sub` on w's own radius, so D2's sequential subtraction (w = 1 - t rounded,
  then 1 - w) carries both roundings.
- **tan(theta/2)** (`tan_half`): over the whole interval [theta - e, theta + e], the derivative's supremum at the
  outer end, plus one ulp at the outer end (2u tan((|theta| + e)/2)). An interval meeting a pole (|theta| + e >= pi)
  refuses (`Pole`).
- **sin, cos, atan** (`axis_angle`, `atan2_half`): the argument's error times a derivative bounded by 1 (2 for
  2 atan), plus one ulp at the interval's largest magnitude.
- **Quaternion products** (`product`): |a| e_b + |b| e_a + e_a e_b, plus 2 gamma_4 (|a| + e_a)(|b| + e_b) of rounding:
  the perturbed norms, so both operands may carry error.
- **Normalizing** (`unit`): 2e/|q| + 4u (the exact norm; a sum of squares within gamma_4, sqrt and the division
  within u each); an axis (`unit3`) 2e/|v| + 3u + gamma_3; a norm not above its error refuses.
- **The angle** (`angle_bound`): a stored quaternion within e (2-norm) of a unit one, whatever its own norm, points
  within asin(e) of it in R^4, so the rotations differ by at most 2 asin(e) (pi from e = 1); plus
  `anim_replay.F64_ANGLE_FLOOR`, the one capture convention: Unity's recorded f32 components are compared as
  stored.
"""

import math
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

U = 2.0 ** -24
GAMMA4 = 4 * U / (1 - 4 * U)
GAMMA3 = 3 * U / (1 - 3 * U)
# f64's unit roundoff, for the witness's own separation angle (`separation`).
# The smallest f32 subnormal: an absolute floor under every relative allowance (a product or a function
# of magnitude m rounds within u m, or within one subnormal ulp where m is tiny).
TINY = 2.0 ** -149
U64 = 2.0 ** -53
GAMMA4_64 = 4 * U64 / (1 - 4 * U64)


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
    return (theta, moved + (0. if value_error == 0 and _f32(theta) == theta else ulps(abs(theta) + moved)))


def scale(a, b):
    """a * b, both scalars with radii, rounded once."""
    (x, ex), (y, ey) = a, b
    return (x * y, abs(x) * ey + abs(y) * ex + ex * ey + ulps((abs(x) + ex) * (abs(y) + ey)))


def ulps(magnitude, count=1):
    """`count` roundings of a result of at most `magnitude`: count u magnitude plus count subnormal ulps, none where
    the magnitude is 0 (an exact zero)."""
    return count * (U * magnitude + TINY) if magnitude > 0 else 0.


def _f32(x):
    return struct.unpack("<f", struct.pack("<f", x))[0]


def _sum(value, error):
    """A sum's radius: its inputs' errors, then one rounding at the perturbed result, none where the inputs are
    exact and the result is an f32 (1 - 0, 1 - .25: Unity's subtraction is exact there too)."""
    return error + (0. if error == 0 and _f32(value) == value else ulps(abs(value) + error))


def add(a, b):
    (x, ex), (y, ey) = a, b
    return (x + y, _sum(x + y, ex + ey))


def sub(a, b):
    (x, ex), (y, ey) = a, b
    return (x - y, _sum(x - y, ex + ey))


def one_minus(w):
    """1 - w, w a value with its radius: Unity's f32 subtraction rounds once at the perturbed result."""
    return sub(exact(1.), w)


def tan_half(a):
    """tan(theta/2) over the whole input interval; a pole inside it refuses."""
    theta, error = a
    outer = abs(theta) + error
    if not math.isfinite(outer) or outer >= math.pi:
        raise Pole(f"tan(theta/2) meets a pole: |theta| + e = {outer!r}")
    value = math.tan(theta / 2)
    widest = math.tan(outer / 2)
    slope = (1 + widest ** 2) / 2
    return (value, slope * error + ulps(widest, 2))


def atan2_half(t):
    """2 atan(t): the inverse of `tan_half`, its derivative bounded by 2 (half angle) and one ulp."""
    value, error = t
    angle = 2 * math.atan(value)
    return (angle, 2 * error + ulps(2 * math.atan(abs(value) + error), 2))


def product(a, b):
    (p, ep), (q, eq) = a, b
    np_, nq = norm(p), norm(q)
    rounding = 2 * GAMMA4 * (np_ + ep) * (nq + eq) + (TINY if (np_ + ep) * (nq + eq) > 0 else 0.)
    return (multiply(p, q), np_ * eq + nq * ep + ep * eq + rounding)


def unit(a):
    q, error = a
    n = norm(q)
    if not n > error:
        raise Pole(f"a quaternion's norm {n!r} does not exceed its error {error!r}")
    return (tuple(x / n for x in q), 2 * error / n + 4 * U + TINY)


def unit3(a):
    v, error = a
    n = norm(v)
    if not n > error:
        raise Pole(f"an axis's norm {n!r} does not exceed its error {error!r}")
    return (tuple(x / n for x in v), 2 * error / n + 3 * U + GAMMA3 + TINY)


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
    es, ec = eh + ulps(min(1., abs(s) + eh), 2), eh + ulps(min(1., abs(c) + eh), 2)
    # The three components s * a_i: |s| e_a + e_s |a| + e_s e_a, then their rounding.
    vector = abs(s) * ea + es + es * ea + ulps((abs(s) + es) * (1 + ea))
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
    return 2 * math.asin(min(1., error)) + replay.F64_ANGLE_FLOOR


def separation(p, q):
    """The rotation angle between two f64 quaternions and its own f64 error: 2 atan2(|v|, |w|) of conj(p) q, well
    conditioned near the identity (acos of a dot product is not). That angle is twice the angle in R^4 between r and
    the w axis, whose gradient is at most 1/|r|; r's components carry at most gamma_4 |p| |q| each (2 gamma_4 |p| |q|
    together, |r| = |p| |q|), and |v|, atan2 and the doubling a few ulps more, charged generously."""
    r = multiply(conj(p), q)
    size = norm(p) * norm(q)
    if not size > 0:
        raise Pole("a prediction with a zero norm")
    turn = 2 * math.atan2(norm(r[:3]), abs(r[3]))
    return turn, 2 * (2 * GAMMA4_64 * size) / (size * (1 - 4 * GAMMA4_64)) + 8 * U64 * turn
