"""Independent f64 equations for decoded animation inputs. See docs/anim_reference.md."""

import json
import math
from bisect import bisect_right
from fractions import Fraction
from pathlib import Path

BOUNDS = json.loads(Path(__file__).with_name("anim_bounds.json").read_text(encoding="utf-8"))
ORDERS = ("XYZ", "XZY", "YZX", "YXZ", "ZXY", "ZYX")


def lerp(a, b, t):
    return a + (b - a) * t


def streamed(keys, sample, *, held=False, seed=None):
    """Sample ordered real (time, (c3, c2, c1, c0)) keys after layout checks."""
    i = bisect_right(keys, sample, key=lambda key: key[0]) - 1
    if i < 0:
        if held and seed is not None:
            return seed
        if not keys:
            raise ValueError("streamed slot has no real key or supported seed")
        return keys[0][1][3]
    time, (c3, c2, c1, c0) = keys[i]
    if held or i == len(keys) - 1:
        return c0
    t = sample - time
    return ((c3 * t + c2) * t + c1) * t + c0


def dense(values, width, slot, begin, rate, sample, *, held=False):
    """Frame-major values for one dense slot. The decoded grid must be distinct."""
    if not (0 <= slot < width and values and len(values) % width == 0
            and math.isfinite(begin) and math.isfinite(rate) and rate > 0):
        raise ValueError("invalid dense layout or clock")
    count = len(values) // width
    frames = range(count)

    def frame_time(frame):
        return begin + frame / rate

    i = min(count - 1, max(0, bisect_right(frames, sample, key=frame_time) - 1))
    first = values[i * width + slot]
    if held or i == count - 1 or sample <= frame_time(i):
        return first
    second = values[(i + 1) * width + slot]
    return lerp(first, second, min(1., (sample - frame_time(i)) * rate))


def constant(values, slot):
    if not 0 <= slot < len(values):
        raise ValueError("invalid constant slot")
    return values[slot]


def hermite(left, right, a, b, outgoing, incoming, sample, *, held=False):
    _interval(left, right)
    if sample <= left:
        return a
    if sample >= right:
        return b
    if held:
        return a
    dt = right - left
    t = (sample - left) / dt
    t2 = t * t
    t3 = t2 * t
    return ((2 * t3 - 3 * t2 + 1) * a + (t3 - 2 * t2 + t) * (dt * outgoing)
            + (-2 * t3 + 3 * t2) * b + (t3 - t2) * (dt * incoming))


def bezier(controls, t):
    a, b, c, d = controls
    ab, bc, cd = lerp(a, b, t), lerp(b, c, t), lerp(c, d, t)
    return lerp(lerp(ab, bc, t), lerp(bc, cd, t), t)


def _time_controls(out_weight, in_weight):
    if not (0 <= out_weight <= 1 and 0 <= in_weight <= 1):
        raise ValueError("weights outside the fractional encoding")
    # Check the quadratic derivative exactly, including a double zero.
    first, last = Fraction(out_weight), Fraction(in_weight)
    middle = 1 - first - last
    a = first - 2 * middle + last
    b = 2 * (middle - first)
    if a > 0 and 0 < -b < 2 * a and 4 * a * first < b * b:
        raise ValueError("weighted time reverses direction")
    return 0, first, 1 - last, 1


def _solve_time(controls, target):
    lo, hi = 0., 1.
    for _ in range(64):
        mid = lo + (hi - lo) * .5
        value = bezier(controls, mid)
        if value == target or mid in (lo, hi):
            return mid
        if value < target:
            lo = mid
        else:
            hi = mid
    return lo + (hi - lo) * .5


def _value_controls(dt, a, b, outgoing, incoming, out_weight, in_weight):
    return a, a + (dt * outgoing) * out_weight, b - (dt * incoming) * in_weight, b


def weighted(left, right, a, b, outgoing, incoming, out_weight, in_weight, sample):
    _interval(left, right)
    controls = tuple(map(float, _time_controls(out_weight, in_weight)))
    if sample <= left:
        return a
    if sample >= right:
        return b
    dt = right - left
    p = _solve_time(controls, (sample - left) / dt)
    return bezier(_value_controls(dt, a, b, outgoing, incoming, out_weight, in_weight), p)


def gamma(operations, unit_roundoff):
    error = operations * unit_roundoff
    if not 0 <= error < 1:
        raise ValueError("rounding model requires n*u < 1")
    return error / (1 - error)


def transcription_bound(equation, scale):
    limits = BOUNDS["transcription"]
    if not 0 <= scale <= limits["max_operation_scale"]:
        raise ValueError("operation scale outside frozen transcription range")
    return limits["absolute_floor"] + gamma(limits["operations"][equation],
                                            limits["unit_roundoff"]) * scale


def _inverse_bracket(controls, target):
    lo, hi = Fraction(0), Fraction(1)
    for _ in range(64):
        mid = (lo + hi) / 2
        if bezier(controls, mid) < target:
            lo = mid
        else:
            hi = mid
    return lo, hi


def weighted_bound(left, right, a, b, outgoing, incoming, out_weight, in_weight, sample):
    """Enclose the exact and rounded time roots and propagate through the value cubic."""
    _interval(left, right)
    controls = _time_controls(out_weight, in_weight)
    limits = BOUNDS["transcription"]
    dt = right - left
    if not (limits["min_interval"] <= dt and max(abs(left), abs(right)) <= limits["max_abs_source_time"]):
        raise ValueError("weighted time outside frozen transcription range")
    if sample <= left or sample >= right:
        return 0.
    target = (Fraction(sample) - Fraction(left)) / (Fraction(right) - Fraction(left))
    band = Fraction(gamma(limits["weighted_time_operations"], limits["unit_roundoff"]))
    lo = _inverse_bracket(controls, max(0, target - band))[0]
    hi = _inverse_bracket(controls, min(1, target + band))[1]
    # All rounded predicate decisions outside the band have the exact sign.
    # Include the final bracket's width and binary64 spacing even on an early exit.
    width = math.nextafter(float(hi - lo) + 2**-52, math.inf)
    values = _value_controls(dt, a, b, outgoing, incoming, out_weight, in_weight)
    variation = 3 * max(abs(y - x) for x, y in zip(values, values[1:]))
    scale = max(abs(a), abs(b)) + abs(dt * outgoing) + abs(dt * incoming)
    evaluation = transcription_bound("weighted", scale)
    return math.nextafter((variation + 6 * evaluation) * width + evaluation, math.inf)


def _interval(start, stop, *, zero=False):
    if not (math.isfinite(start) and math.isfinite(stop) and math.isfinite(stop - start)
            and (start < stop or (zero and start == stop))):
        raise ValueError("invalid source interval")


def source_to_local(start, stop, source):
    _interval(start, stop, zero=True)
    if not start <= source <= stop:
        raise ValueError("sample outside source interval")
    return source - start


def schedule(start, stop, rate):
    _interval(start, stop, zero=True)
    if not math.isfinite(rate) or rate <= 0:
        raise ValueError("invalid serialized sample rate")
    count = math.ceil((stop - start) * rate)
    interior = tuple(start + i / rate for i in range(1, count + 1))
    times = (start,) + tuple(t for t in interior if t < stop)
    if any(a >= b for a, b in zip(times, times[1:])):
        raise ValueError("collapsed source schedule")
    return times if start == stop else times + (stop,)


def packed_times(deltas):
    """Checked source centiseconds only. The packed slopes need fixture evidence."""
    total = 0
    seconds = []
    for delta in deltas:
        if type(delta) is not int or not 0 <= delta <= 2**63 - 1 - total:
            raise ValueError("invalid or overflowing packed time delta")
        total += delta
        seconds.append(total / 100)
    return tuple(seconds)


def normalize(q):
    length = math.hypot(*q)
    if len(q) != 4 or not math.isfinite(length) or length == 0:
        raise ValueError("invalid XYZW quaternion")
    return tuple(v / length for v in q)


def angle(q, r):
    q, r = normalize(q), normalize(r)
    if sum(a * b for a, b in zip(q, r)) < 0:
        r = tuple(-b for b in r)
    return 4 * math.atan2(math.hypot(*(a - b for a, b in zip(q, r))),
                          math.hypot(*(a + b for a, b in zip(q, r))))


def angular_bound(component_error, min_norm, *, arithmetic=0.):
    """Map raw component errors to an SO(3) angle, given a norm lower bound."""
    if not 0 <= component_error < math.inf or not 0 < min_norm < math.inf:
        raise ValueError("invalid quaternion error or norm")
    return min(math.pi, 4 * math.asin(min(1., 2 * component_error / min_norm))) + arithmetic


def transcription_angle_bound(component_error, min_norm):
    limits = BOUNDS["transcription"]
    if min_norm < limits["min_raw_quaternion_norm"]:
        raise ValueError("quaternion is outside the frozen norm range")
    return angular_bound(component_error, min_norm,
                         arithmetic=limits["angular_floor_u64"] * limits["unit_roundoff"])


def multiply(q, r):
    x, y, z, w = q
    a, b, c, d = r
    return (w * a + x * d + y * c - z * b,
            w * b - x * c + y * d + z * a,
            w * c + x * b - y * a + z * d,
            w * d - x * a - y * b - z * c)


def euler(degrees, order):
    if type(order) is not int or not 0 <= order < len(ORDERS):
        raise ValueError("unsupported rotation order")
    result = (0., 0., 0., 1.)
    for axis in ORDERS[order]:
        i = "XYZ".index(axis)
        half_angle = degrees[i] * (math.pi / 360)
        xyz = [0., 0., 0.]
        xyz[i] = math.sin(half_angle)
        result = multiply((*xyz, math.cos(half_angle)), result)
    return normalize(result)


def muscle_angle(value, lower, upper, sign):
    """Candidate neutral-relative limit map, pending recorded axis evidence."""
    if not (-1 <= value <= 1 and -math.inf < lower <= 0 <= upper < math.inf and sign in (-1, 1)):
        raise ValueError("unsupported muscle limits or value")
    directed = sign * value
    return directed * (upper if directed >= 0 else -lower)


def joint_rotation(rest, pre, post, degrees, order, *, optional=False):
    """Candidate axis composition. pre and post must describe inverse bases."""
    if rest is None:
        if optional:
            return None
        raise ValueError("missing required bone")
    return normalize(multiply(multiply(multiply(normalize(rest), normalize(pre)),
                                       euler(degrees, order)), normalize(post)))


def local_pose(rest, sampled):
    """Merge sparse position and scale components and a single rotation owner."""
    p, q, s = rest
    position, rotation, scale = sampled
    return (tuple(a if b is None else b for a, b in zip(p, position)),
            q if rotation is None else normalize(rotation),
            tuple(a if b is None else b for a, b in zip(s, scale)))


def ulp32(magnitude):
    """Binary32 spacing at a nonnegative magnitude, including subnormals."""
    if not 0 <= magnitude < 2**128:
        raise ValueError("magnitude outside f32 range")
    return 2.**max(-149, math.frexp(magnitude)[1] - 24) if magnitude else 2.**-149


def bake_bound(property_name, magnitude, rate, local_time_bound):
    """Use the sum of source/reconstructed rate bounds and their max local time."""
    bake = BOUNDS["bake"]
    rotation = property_name == "rotation"
    max_rate = bake["max_angular_rate" if rotation else "max_component_rate"]
    if not (0 <= rate <= max_rate and 0 <= local_time_bound <= bake["max_local_seconds"]
            and 0 <= magnitude <= bake["max_abs_component"] and (not rotation or magnitude == 1)):
        raise ValueError("outside frozen bake range")
    rounding = (bake["angular_ulp_coefficient"] * ulp32(1.) if rotation else
                bake["component_ulp_coefficient"] * ulp32(magnitude))
    return (bake["approximation"][property_name] + bake["arithmetic_floor"]
            + rounding + bake["timestamp_ulp_coefficient"] * ulp32(local_time_bound) * rate)
