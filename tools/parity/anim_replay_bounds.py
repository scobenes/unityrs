"""Range-based f32 replay budgets. See the per-recording derivation in anim_reference.md."""
import math
from dataclasses import dataclass

from anim_reference import BOUNDS, gamma

U32 = 2**-24
MIN_NORMAL_F32 = 2**-126
OPERATIONS = BOUNDS["transcription"]["operations"]
RANGE_LENGTHS = {"rest": 1, "constant": 1, "held": 1, "streamed": 5, "dense": 6,
                 "hermite": 5, "weighted": 7, "muscle": 2, "joint": 1}
INPUT_STAGES = ("rest", "constant", "held", "streamed", "dense", "hermite", "weighted")
TRACK_STAGES = (
    {stage: (stage,) for stage in INPUT_STAGES}
    | {f"euler_{stage}": (stage, "joint") for stage in INPUT_STAGES if stage != "rest"}
    | {f"muscle_{stage}": (stage, "muscle", "joint") for stage in ("constant", "streamed", "dense")}
)


def roundoff(operations, scale, flush_gain=1):
    if not 0 <= scale <= BOUNDS["transcription"]["max_operation_scale"]:
        raise ValueError("budget operation scale is outside the frozen range")
    return gamma(operations, U32) * scale + operations * MIN_NORMAL_F32 * flush_gain


def magnitude(interval):
    lo, hi = interval
    if lo > hi:
        raise ValueError("reversed budget range")
    return max(abs(lo), abs(hi))


def difference(left, right):
    return max(abs(left[0] - right[1]), abs(left[1] - right[0]))


@dataclass(frozen=True)
class CurveError:
    arithmetic: float
    rate: float = 0.
    # Weighted time inverse: propagate normalized time through the value cubic.
    duration: float = 1.
    value_derivative: float = 0.
    time_derivative: float = 0.
    time_error: float = 0.

    def error(self, dt):
        if self.value_derivative:
            span = min(1., dt / self.duration + self.time_error)
            parameter = span ** (1 / 3)
            if self.time_derivative > 0:
                parameter = min(parameter, span / self.time_derivative)
            return self.arithmetic + self.value_derivative * parameter
        return self.arithmetic + dt * self.rate


def _curve_error(stage, ranges):
    bounds = list(map(magnitude, ranges))
    if stage in ("rest", "constant", "held"):
        return CurveError(0.)
    operations = OPERATIONS[stage]
    if stage == "streamed":
        c3, c2, c1, c0, d = bounds
        if ranges[4][0] < 0:
            raise ValueError("streamed time deltas must be nonnegative")
        scale = ((c3 * d + c2) * d + c1) * d + c0
        rate = (3 * c3 * d + 2 * c2) * d + c1
        flush = max(1., rate, d, d**2, d**3)
        return CurveError(roundoff(operations, scale, flush), rate)
    if stage == "dense":
        a, b, rate, time, begin, offset = bounds
        if ranges[2][0] <= 0:
            raise ValueError("dense rate range must be positive")
        delta = difference(*ranges[:2])
        scale = a + b + delta * (1 + rate * (time + begin + offset))
        flush = max(1., delta, delta * rate, delta * rate / ranges[2][0])
        return CurveError(roundoff(operations, scale, flush), delta * rate)

    a, b, outgoing, incoming, h = bounds[:5]
    shortest = ranges[4][0]
    if shortest < MIN_NORMAL_F32:
        raise ValueError("explicit segment needs a normal f32 duration")
    delta = difference(*ranges[:2])
    slopes = outgoing + incoming
    if stage == "hermite":
        scale = 16 * (a + b + h * slopes)
        rate = 1.5 * delta / shortest + slopes
        flush = max(1., scale, scale / shortest, h)
        return CurveError(roundoff(operations, scale, flush), rate)

    # The last two ranges are the fractional out and in weights.
    out_weight, in_weight = ranges[-2:]
    if not all(0 <= lo <= hi <= 1 for lo, hi in (out_weight, in_weight)):
        raise ValueError("weighted range outside the fractional encoding")
    scale = max(a, b) + h * slopes
    value_derivative = 3 * (delta + h * slopes)
    sum_upper = math.nextafter(out_weight[1] + in_weight[1], math.inf)
    middle_lower = math.nextafter(1 - sum_upper, -math.inf)
    time_derivative = math.nextafter(3 * min(out_weight[0], middle_lower, in_weight[0]), -math.inf)
    arithmetic = roundoff(operations, scale, max(1., scale, h))
    time_error = 2 * roundoff(OPERATIONS["weighted"], 1., max(1., 1 / shortest)) + 2**-23
    return CurveError(arithmetic, duration=shortest, value_derivative=value_derivative,
                      time_derivative=time_derivative, time_error=time_error)


def value_magnitude(stage, ranges):
    """The largest value magnitude an input stage's ranges admit."""
    bounds = list(map(magnitude, ranges))
    if stage in ("rest", "constant", "held"):
        return bounds[0]
    if stage == "streamed":
        c3, c2, c1, c0, d = bounds
        return ((c3 * d + c2) * d + c1) * d + c0
    if stage == "dense":
        return max(bounds[:2])
    # A cubic Bezier stays in the hull of its controls, v0 + h*outSlope/3
    # and v1 - h*inSlope/3 between the two values.
    a, b, outgoing, incoming, h = bounds[:5]
    return max(a, b) + h * (outgoing + incoming)


def _norm_floor(boxes, *, unit_space):
    # Each box encloses the raw quaternion over a part of the playback path.
    # Its largest guaranteed component magnitude is a lower bound on its norm.
    floors = []
    for box in boxes:
        maxima = [magnitude(interval) for interval in box]
        minima = [max(0., lo, -hi) for lo, hi in box]
        if max(maxima) > 2**60:
            raise ValueError("raw quaternion range exceeds safe f32 normalization")
        if unit_space and not math.hypot(*minima) <= 1 <= math.hypot(*maxima):
            raise ValueError("joint output box must admit a unit quaternion")
        floors.append(max(minima))
    floor = min(floors)
    if floor < 2**-60:
        raise ValueError("raw quaternion range cannot establish a safe positive norm floor")
    return min(floor, 1.) if unit_space else floor


@dataclass(frozen=True)
class BudgetModel:
    # One (curve error, value magnitude) per component with its own ranges,
    # or a single pair shared by every component of the row.
    components: tuple
    gain: float
    extra_error: float
    min_norm: float | None
    per_component: bool

    def errors(self, dt):
        """Each component's arithmetic error and the output magnitude it reaches."""
        for curve, value in self.components:
            error = curve.error(dt) * self.gain + self.extra_error
            yield error, value * self.gain + error


def _component_ranges(row, stage):
    """Per-component ranges of a translation or scale row, in the stage's layout.

    A range a component never reads is [0, 0]. The Rust audit checks each
    component's values against its own ranges and the row's stage range.
    """
    rows = row.get("component_ranges")
    if (rows is None) == (row["property"] in ("translation", "scale")):
        raise ValueError("translation and scale rows, and only they, need component ranges")
    if rows is None:
        return None
    for ranges in rows:
        if len(ranges) != len(stage["range"]):
            raise ValueError(f"wrong component range width for {stage['stage']}")
        for interval in ranges:
            magnitude(interval)
    return rows


def budget_model(row):
    prop = row["property"]
    stages = row["stages"]
    sequence = tuple(stage["stage"] for stage in stages)
    if len(set(sequence)) != len(sequence):
        raise ValueError("duplicate budget stage")
    if sequence != TRACK_STAGES[row["track_kind"]]:
        raise ValueError("budget stage sequence does not match the track kind")
    if any(("norm_ranges" in stage) != (prop == "rotation" and index == len(stages) - 1)
           for index, stage in enumerate(stages)):
        raise ValueError("norm ranges are required only on the rotation output stage")
    if ("weight_units" in row) != (prop == "weight"):
        raise ValueError("weight units are required only for weights")
    joint = sequence[-1] == "joint"
    if joint and prop != "rotation":
        raise ValueError("joint composition requires a rotation budget")
    for stage in stages:
        if len(stage["range"]) != RANGE_LENGTHS[stage["stage"]]:
            raise ValueError(f"wrong budget range width for {stage['stage']}")
        for interval in stage["range"]:
            magnitude(interval)
    floor = _norm_floor(stages[-1]["norm_ranges"], unit_space=joint) if prop == "rotation" else None

    gain = 100. if row.get("weight_units") == "fraction_of_100" else 1.
    # A native percent multiply may flush its input or result. Its normal
    # rounding is covered by the final output ulp.
    extra = (gain + 1) * MIN_NORMAL_F32 if gain == 100. else 0.
    if joint:
        degrees = magnitude(stages[-1]["range"][0])
        if degrees > BOUNDS["transcription"]["max_abs_degrees"]:
            raise ValueError("joint angle range exceeds the frozen equation domain")
        # Unit quaternion products preserve Euclidean norms. Each axis adds
        # at most one half-angle of chord displacement.
        gain = 3 * math.pi / 360
        extra = roundoff(OPERATIONS["joint"], 8 * (1 + degrees * math.pi / 360)) + 24 * 2**-23
        if sequence[-2] == "muscle":
            muscle_range, limit_range = stages[1]["range"]
            max_muscle, max_limit = magnitude(muscle_range), magnitude(limit_range)
            if max_muscle * max_limit > degrees:
                raise ValueError("joint angle range does not cover the muscle range and limits")
            extra += gain * roundoff(OPERATIONS["muscle"], max_muscle * max_limit)
            gain *= max_limit
    own = _component_ranges(row, stages[0])
    if joint or prop == "rotation":
        # The output is a normalized quaternion, with components at most one.
        components = ((_curve_error(sequence[0], stages[0]["range"]), 0.),)
    else:
        # A weight has one component; a vector row bounds each from its own
        # ranges and never from a range shared across components.
        rows = own or [stages[0]["range"]]
        components = tuple((_curve_error(sequence[0], ranges), value_magnitude(sequence[0], ranges))
                           for ranges in rows)
    return BudgetModel(components, gain, extra, floor, "component_ranges" in row)
