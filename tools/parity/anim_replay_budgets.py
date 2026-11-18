"""Author the per-recording budget rows of a completed generic or legacy replay.

The rows follow the per-recording f32 tolerance derivation in
docs/anim_reference.md. Each row's track kind and stage ranges come from
the replay's own digest-pinned bundles, read through `unityrs show`: the
serialized rest pose, compiled or explicit curve data and the playback
interval. Observed poses are never read. The Rust same-bundle comparison
checks every range and track kind against its own decoding, and
`anim_replay.py derive` turns the rows into frozen per-sample bounds.

    python3 -I tools/parity/anim_replay_budgets.py REPLAY --bundle-dir DIR \
        [--unityrs BIN] [--verify] > budgeted-replay.json

`--verify` authors the rows again for a budgeted replay and fails unless
they equal the stored ones, so rows widened after the capture was seen do
not pass. Supported paths: rest, constant, held and streamed compiled
slots, clamped explicit Hermite curves, and Euler input through the joint
stage. Dense, weighted and humanoid paths are refused until they have a
budget author.
"""
import argparse
import hashlib
import json
import math
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay  # noqa: E402
import child  # noqa: E402
from anim_replay import TRS, read_json, require  # noqa: E402
from anim_replay_complete import wire_value  # noqa: E402

STREAM_SEED, STREAM_END = 0xff7fffff, 0x7f800000
# Every computed enclosure is widened by RELATIVE_PAD of the magnitudes of
# the terms that produced it, never of the result, plus ABSOLUTE_PAD for
# f64 underflow. See "Budget authoring" in docs/anim_reference.md.
RELATIVE_PAD, ABSOLUTE_PAD = 2**-40, 2**-1022
# Rotation boxes halve a key interval until each box's guaranteed largest
# component is at least FLOOR_SHARE of its largest possible component.
FLOOR_SHARE, MAX_DEPTH = .5, 16
EXPLICIT = {"m_PositionCurves": ("translation", 3), "m_ScaleCurves": ("scale", 3),
            "m_RotationCurves": ("rotation", 4), "m_EulerCurves": ("euler", 3)}
AXES = "xyzw"
EULER_ORDERS = ((0, 1, 2), (0, 2, 1), (1, 2, 0), (1, 0, 2), (2, 0, 1), (2, 1, 0))
SHOW_LIMITS = ["--max-items", "1000000", "--max-bytes", "1073741824", "--max-depth", "128"]


def f32(value, *, finite=True):
    """A show float: an exact f32 number, or its tagged bits for an infinity or NaN."""
    if isinstance(value, dict):
        require(value.get("t") == "f32" and isinstance(value.get("v"), str) and len(value["v"]) == 8,
                "unexpected show float encoding")
        number, = struct.unpack(">f", bytes.fromhex(value["v"]))
    else:
        require(type(value) in (int, float), "expected a show float")
        try:
            number, = struct.unpack("<f", struct.pack("<f", value))
        except (OverflowError, struct.error) as exc:
            raise ValueError("show float overflows f32") from exc
        require(number == value, "show float is not an exact f32")
    require(not math.isnan(number) and (math.isfinite(number) or not finite), "unexpected nonfinite show float")
    return number


def floats(values):
    if isinstance(values, dict):
        require(values.get("t") == "f32s" and len(values["v"]) % 8 == 0, "incomplete show float array")
        return [f32({"t": "f32", "v": values["v"][i:i + 8]}) for i in range(0, len(values["v"]), 8)]
    return [f32(value) for value in values]


def from_bits(word):
    return struct.unpack("<f", struct.pack("<I", word))[0]


def crc(text):
    return zlib.crc32(text.encode("utf-8"))


def widen(lo, hi, scale):
    pad = scale * RELATIVE_PAD + ABSOLUTE_PAD
    return lo - pad, hi + pad


def magnitude(interval):
    return max(abs(interval[0]), abs(interval[1]))


class Segment:
    """One scalar playback segment, as the Rust sampler classifies it."""

    def __init__(self, kind, value=None, key=0., coefficients=None, hermite=None):
        self.kind, self.value, self.key = kind, value, key
        self.coefficients, self.hermite = coefficients, hermite

    def stage(self):
        return None if self.kind == "constant" else self.kind

    def at(self, time):
        if self.kind in ("constant", "held"):
            return self.value
        if self.kind == "streamed":
            c3, c2, c1, c0 = self.coefficients
            d = time - self.key
            return ((c3 * d + c2) * d + c1) * d + c0
        a, b, outgoing, incoming, h = self.hermite
        u = (time - self.key) / h
        return ((2 * u**3 - 3 * u**2 + 1) * a + (u**3 - 2 * u**2 + u) * h * outgoing
                + (-2 * u**3 + 3 * u**2) * b + (u**3 - u**2) * h * incoming)

    def scale(self, time):
        """The sum of term magnitudes in evaluating the segment up to `time`."""
        if self.kind in ("constant", "held"):
            return abs(self.value)
        if self.kind == "streamed":
            d = abs(time - self.key)
            return sum(abs(c) * d**(3 - i) for i, c in enumerate(self.coefficients)) + d
        a, b, outgoing, incoming, h = self.hermite
        return abs(a) + abs(b) + h * (abs(outgoing) + abs(incoming)) + abs(time) + abs(self.key)

    def rate(self, hi):
        """An upper bound on the derivative up to `hi`."""
        if self.kind in ("constant", "held"):
            return 0.
        if self.kind == "streamed":
            c3, c2, c1, _ = self.coefficients
            end = abs(hi - self.key)
            return 3 * abs(c3) * end * end + 2 * abs(c2) * end + abs(c1)
        a, b, outgoing, incoming, h = self.hermite
        return 3 * (abs(b - a) + h * (abs(outgoing) + abs(incoming))) / h

    def over(self, a, b):
        """An enclosure of the segment's values over [a, b]."""
        va, vb = self.at(a), self.at(b)
        spread = (b - a) * self.rate(b)
        return widen(min(va, vb) - spread, max(va, vb) + spread, self.scale(b) + spread)


class Streamed:
    """A compiled streamed slot: keys of (time, coefficients) and its seed."""

    def __init__(self, keys, seed):
        self.keys, self.seed = keys, seed

    def times(self):
        return [time for time, _ in self.keys]

    def segment(self, time):
        later = [i for i, (key, _) in enumerate(self.keys) if key > time]
        nxt = later[0] if later else len(self.keys)
        if nxt == 0:
            require(self.seed is None or (self.seed[:3] == (0., 0., 0.) and (
                not self.keys or self.seed[3] == self.keys[0][1][3])), "unverified pre-first-key seed")
            return Segment("constant", self.keys[0][1][3] if self.keys else self.seed[3])
        key, coefficients = self.keys[nxt - 1]
        if nxt == len(self.keys):
            return Segment("constant", coefficients[3])
        if coefficients[:3] == (0., 0., 0.):
            return Segment("held", coefficients[3])
        return Segment("streamed", key=key, coefficients=coefficients)


class Constant:
    def __init__(self, value):
        self.value = value

    def times(self):
        return []

    def segment(self, _time):
        return Segment("constant", self.value)


class Hermite:
    """An explicit clamped curve component: keys of (time, value, in, out)."""

    def __init__(self, keys):
        self.keys = keys

    def times(self):
        return [key[0] for key in self.keys]

    def segment(self, time):
        later = [i for i, key in enumerate(self.keys) if key[0] > time]
        nxt = later[0] if later else len(self.keys)
        if nxt == 0:
            return Segment("constant", self.keys[0][1])
        left = self.keys[nxt - 1]
        if nxt == len(self.keys):
            return Segment("constant", left[1])
        right = self.keys[nxt]
        slopes = (left[3], right[2])
        if any(math.isinf(slope) for slope in slopes):
            return Segment("held", left[1])
        return Segment("hermite", key=left[0],
                       hermite=(left[1], right[1], slopes[0], slopes[1], right[0] - left[0]))


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


class Bundles:
    """The replay's pinned bundles, read object by object through `unityrs show`.

    With no executable, `folder` holds saved show documents instead, named
    `BUNDLE.FILE_INDEX.PATH_ID.show.json`, each with its bundle's digest.
    """

    def __init__(self, spec, folder, unityrs):
        self.rows, self.folder, self.unityrs = spec["bundles"], folder, unityrs
        if unityrs is not None:
            for row in self.rows:
                require(digest(folder / row["name"]) == row["sha256"], f"bundle digest mismatch: {row['name']}")

    def value(self, ident):
        row = self.rows[ident["bundle"]]
        name, path_id = row["name"], ident["path_id"]
        if self.unityrs is None:
            doc = read_json((self.folder / f"{name}.{ident['file_index']}.{path_id}.show.json")
                            .read_text(encoding="utf-8"))
            require(doc.pop("bundle_sha256", None) == row["sha256"], f"saved show digest mismatch: {name}")
        else:
            result = child.run([self.unityrs, "show", name, "--path-id", path_id, "--file-index",
                                f"0:{ident['file_index']}", "--value", "canonical", *SHOW_LIMITS],
                               cwd=self.folder, capture_output=True, text=True)
            require(result.returncode == 0, f"unityrs show {name}: {(result.stderr or result.stdout).strip()}")
            doc = read_json(result.stdout)
        require(doc.get("format") == "unityrs.show" and doc.get("version") == 1
                and doc["object"]["file"] == ident["file"] and str(doc["object"]["path_id"]) == path_id,
                f"unexpected show document for {name} {path_id}")
        truncated = doc.get("truncated")
        require(isinstance(truncated, dict) and set(truncated) == {"cuts", "items", "depth", "bytes", "binary"}
                and all(type(count) is int and count == 0 for count in truncated.values()),
                f"truncated show output for {name} {path_id}")
        value = wire_value(doc["value"], anim_replay)
        require(isinstance(value, dict) and "$unityrs" not in value, f"unreadable show value for {name} {path_id}")
        return value


def vector(value, axes):
    return [f32(value[axis]) for axis in AXES[:axes]]


def node_paths(spec):
    """Each node's path below the binding component's node, or None."""
    names = {row["address"]: row["name"] for row in spec["nodes"]}
    root = spec["binding_address"]
    paths = []
    for row in spec["nodes"]:
        address = row["address"]
        if address == root:
            paths.append("")
            continue
        prefix = root + "/" if root else ""
        if not address.startswith(prefix):
            paths.append(None)
            continue
        base = prefix.rstrip("/")
        names_below = []
        for part in address[len(prefix):].split("/"):
            base = f"{base}/{part}" if base else part
            names_below.append(names[base])
        paths.append("/".join(names_below))
    return paths


def compiled_tracks(clip):
    """Map (path hash, property) to per-axis slot sources and Euler order."""
    playback = clip["m_MuscleClip"]["m_Clip"]
    playback = playback.get("data", playback)
    stream, dense = playback["m_StreamedClip"], playback["m_DenseClip"]
    require(dense["m_CurveCount"] == 0, "dense budgets are not authored yet")
    streamed = stream["curveCount"] + stream.get("discreteCurveCount", 0)
    constants = floats(playback["m_ConstantClip"]["data"])
    keys = [[] for _ in range(streamed)]
    seeds = {}
    words, cursor = stream["data"], 0
    require(isinstance(words, list) and all(type(word) is int for word in words), "unexpected streamed words")
    while cursor < len(words):
        bits, count = words[cursor:cursor + 2]
        entries = [words[i:i + 5] for i in range(cursor + 2, cursor + 2 + 5 * count, 5)]
        cursor += 2 + 5 * count
        if bits == STREAM_END:
            break
        for slot, *raw in entries:
            coefficients = tuple(map(from_bits, raw))
            require(all(map(math.isfinite, coefficients)), "nonfinite streamed coefficient")
            if bits == STREAM_SEED and cursor == 2 + 5 * count:
                seeds[slot] = coefficients
            else:
                keys[slot].append((from_bits(bits), coefficients))
    sources = [Streamed(keys[i], seeds.get(i)) for i in range(streamed)]
    sources += [Constant(value) for value in constants]
    tracks, first = {}, 0
    for row in clip["m_ClipBindingConstant"]["genericBindings"]:
        class_id, attribute, custom = row.get("typeID", row.get("classID")), row["attribute"], row["customType"]
        if class_id == 4:
            prop, width = {1: ("translation", 3), 2: ("rotation", 4), 3: ("scale", 3),
                           4: ("euler", 3)}[attribute]
        else:
            # Weights and object references, a discrete slot, read one slot.
            prop, width = None, 1
        span = sources[first:first + width]
        first += width
        if prop is not None:
            tracks[row["path"], prop] = (span, EULER_ORDERS[custom] if prop == "euler" else None)
        elif class_id == 137 and custom == 20:
            tracks[row["path"], ("weight", attribute)] = (span, None)
    require(first == len(sources), "binding slots do not match the compiled storage")
    return tracks


def explicit_curve(curve, axes):
    """Each axis of a clamped, unweighted explicit curve as a Hermite source."""
    require(curve["m_PreInfinity"] == 2 and curve["m_PostInfinity"] == 2, "only clamped explicit curves have budgets")
    keys = curve["m_Curve"]
    require(keys and all(key.get("weightedMode", 0) == 0 for key in keys), "weighted budgets are not authored yet")

    def member(key, field, axis):
        value = key[field]
        return value if axis is None else value[axis]

    return [Hermite([(f32(key["time"]), f32(member(key, "value", axis)),
                      f32(member(key, "inSlope", axis), finite=False),
                      f32(member(key, "outSlope", axis), finite=False)) for key in keys])
            for axis in axes]


def explicit_tracks(clip):
    tracks = {}
    for field, (prop, width) in EXPLICIT.items():
        for row in clip.get(field, []):
            span = explicit_curve(row["curve"], AXES[:width])
            order = EULER_ORDERS[row["curve"]["m_RotationOrder"]] if prop == "euler" else None
            tracks[crc(row["path"]), prop] = (span, order)
    for row in clip.get("m_FloatCurves", []):
        attribute = row["attribute"]
        if row["classID"] == 137 and attribute.startswith("blendShape."):
            tracks[crc(row["path"]), ("weight", crc(attribute.removeprefix("blendShape.")))] = (
                explicit_curve(row["curve"], [None]), None)
    return tracks


class Ranges:
    """Inclusive envelopes of every value one budget stage reads."""

    def __init__(self, width):
        self.lo, self.hi = [math.inf] * width, [-math.inf] * width
        self.scale = [0.] * width

    def add(self, i, lo, hi=None, scale=None):
        hi = lo if hi is None else hi
        self.lo[i], self.hi[i] = min(self.lo[i], lo), max(self.hi[i], hi)
        self.scale[i] = max(self.scale[i], magnitude((lo, hi)) if scale is None else scale)

    def rows(self):
        rows = []
        for lo, hi, scale in zip(self.lo, self.hi, self.scale):
            if lo > hi:
                # A component reads no value from this range.
                rows.append([0., 0.])
                continue
            padded = widen(lo, hi, scale)
            # A nonnegative range, as streamed time deltas are, stays so.
            rows.append([max(0., padded[0]) if lo >= 0 else padded[0], padded[1]])
        return rows


WIDTHS = {"rest": 1, "constant": 1, "held": 1, "streamed": 5, "hermite": 5}


def record(ranges, stage, segment, lo, hi):
    if segment.kind in ("constant", "held"):
        ranges.add(3 if stage == "streamed" else 0, segment.value)
    elif segment.kind == "streamed":
        for i, value in enumerate(segment.coefficients):
            ranges.add(i, value)
        ranges.add(4, lo - segment.key, hi - segment.key, abs(hi) + abs(segment.key))
    else:
        for i, value in enumerate(segment.hermite):
            ranges.add(i, value)


def interval_product(a, b):
    products = [x * y for x in a for y in b]
    return widen(min(products), max(products), magnitude(a) * magnitude(b))


def quaternion_product(a, b):
    """An enclosure of the Hamilton product of two quaternion boxes."""
    x, y, z, w = a
    p, q, r, s = b
    rows = (((1, w, p), (1, x, s), (1, y, r), (-1, z, q)), ((1, w, q), (-1, x, r), (1, y, s), (1, z, p)),
            ((1, w, r), (1, x, q), (-1, y, p), (1, z, s)), ((1, w, s), (-1, x, p), (-1, y, q), (-1, z, r)))
    result = []
    for row in rows:
        lo = hi = scale = 0.
        for sign, u, v in row:
            term = interval_product(u, v)
            lo, hi = (lo + term[0], hi + term[1]) if sign > 0 else (lo - term[1], hi - term[0])
            scale += magnitude(term)
        result.append(widen(lo, hi, scale))
    return result


def sine_cosine(lo, hi):
    """Enclosures of sin and cos over [lo, hi] radians, extrema included."""
    if hi - lo >= 2 * math.pi:
        return (-1., 1.), (-1., 1.)
    slack = RELATIVE_PAD * max(1., abs(lo), abs(hi))

    def reaches(phase):
        turn = math.ceil((lo - slack - phase) / (2 * math.pi))
        return phase + 2 * math.pi * turn <= hi + slack

    sines, cosines = (math.sin(lo), math.sin(hi)), (math.cos(lo), math.cos(hi))
    sine = (-1. if reaches(-math.pi / 2) else min(sines), 1. if reaches(math.pi / 2) else max(sines))
    cosine = (-1. if reaches(math.pi) else min(cosines), 1. if reaches(0.) else max(cosines))
    return widen(*sine, 1.), widen(*cosine, 1.)


def euler_box(order, degrees):
    """Enclose the joint quaternion over per-axis degree intervals.

    Each half angle is widened before the trig by RELATIVE_PAD of its
    magnitude. That covers the rounding of the degree conversion and the
    difference between math.pi and pi, which is below 2^-52 of pi.
    """
    result = [(0., 0.), (0., 0.), (0., 0.), (1., 1.)]
    for axis in order:
        lo, hi = (x * math.pi / 360 for x in degrees[axis])
        sine, cosine = sine_cosine(*widen(lo, hi, magnitude((lo, hi))))
        factor = [(0., 0.)] * 3 + [cosine]
        factor[axis] = sine
        result = quaternion_product(factor, result)
    # The exact product of unit quaternions is a unit quaternion, so the
    # sampler's normalization moves each component by rounding alone.
    return [widen(lo, hi, 1.) for lo, hi in result]


def floor_of(box):
    return max(max(0., lo, -hi) for lo, hi in box)


def rotation_boxes(rest, sources, order, times):
    """Raw (or joint) quaternion boxes at every time and over every interval.

    The point boxes take each time's own segment, so a held jump at a key or
    at the stop is enclosed. Each interval is halved until every box has a
    norm floor of FLOOR_SHARE of its largest component.
    """
    degrees = Ranges(1)
    joint = order is not None

    def box(values):
        return euler_box(order, values) if joint else values

    def keep(values, enclosure):
        if joint:
            for interval in values:
                degrees.add(0, *interval)
        return [enclosure]

    def point(time):
        values = []
        for axis in range(3 if joint else 4):
            if axis < len(sources):
                segment = sources[axis].segment(time)
                values.append(widen(segment.at(time), segment.at(time), segment.scale(time)))
            else:
                values.append(widen(rest[axis], rest[axis], abs(rest[axis])))
        return keep(values, box(values))

    def pieces(segments, a, b, depth):
        values = [segment.over(a, b) for segment in segments]
        enclosure = box(values)
        if floor_of(enclosure) >= FLOOR_SHARE * max(map(magnitude, enclosure)):
            return keep(values, enclosure)
        middle = a + (b - a) * .5
        require(depth < MAX_DEPTH and a < middle < b,
                f"rotation boxes reach depth {MAX_DEPTH} without a norm floor near the origin")
        return pieces(segments, a, middle, depth + 1) + pieces(segments, middle, b, depth + 1)

    if not sources:
        return point(times[0]), degrees
    boxes = [box for time in times for box in point(time)]
    for lo, hi in zip(times, times[1:]):
        middle = lo + (hi - lo) * .5
        boxes += pieces([source.segment(middle) for source in sources], lo, hi, 0)
    require(all(floor_of(b) > 0 for b in boxes), "a rotation box contains the origin")
    return boxes, degrees


def track_stage(prop, target, rest, sources, times):
    name = None
    for lo, hi in zip(times, times[1:]):
        mid = lo + (hi - lo) * .5
        for axis in range(min(len(rest), len(sources))):
            stage = sources[axis].segment(mid).stage()
            if stage is not None:
                # A held segment of a streamed track is a cubic with zero
                # slopes, which the streamed stage bounds.
                if {name, stage} == {"held", "streamed"}:
                    name = stage = "streamed"
                require(name in (None, stage), f"{prop} {target} mixes {name} and {stage} segments")
                name = stage
    name = name or ("constant" if sources else "rest")
    require(name in WIDTHS, f"{name} budgets are not authored yet")
    return name


def budget(prop, target, channel, rest, sources, order, times):
    """One budget row in the order the Rust audit walks the playback path."""
    joint = order is not None
    name = track_stage(prop, target, rest, sources, times)
    envelope = Ranges(WIDTHS[name])
    # Translation and scale give each component its own ranges, so one
    # component's magnitude never widens another's tolerance.
    components = [Ranges(WIDTHS[name]) for _ in rest] if prop in ("translation", "scale") else []

    def note(axis, segment, lo, hi):
        record(envelope, name, segment, lo, hi)
        if components:
            record(components[axis], name, segment, lo, hi)

    for index, time in enumerate(times):
        for axis in range(len(rest)):
            if axis >= len(sources):
                note(axis, Segment("constant", rest[axis]), time, time)
            elif (segment := sources[axis].segment(time)).kind in ("constant", "held"):
                note(axis, segment, time, time)
        if index + 1 < len(times):
            stop = times[index + 1]
            for axis in range(min(len(rest), len(sources))):
                note(axis, sources[axis].segment(time + (stop - time) * .5), time, stop)
    stages = [{"stage": name, "range": envelope.rows()}]
    if joint:
        stages.append({"stage": "joint", "range": []})
    row = {"property": prop, "target": target, "channel": channel,
           "track_kind": f"euler_{name}" if joint else name, "stages": stages}
    if components:
        row["component_ranges"] = [ranges.rows() for ranges in components]
    if prop == "weight":
        row["weight_units"] = "editor"
    if prop == "rotation":
        boxes, degrees = rotation_boxes(rest, sources, order, times)
        if joint:
            stages[-1]["range"] = degrees.rows()
        stages[-1]["norm_ranges"] = [[list(interval) for interval in box] for box in boxes]
    return row


def author(manifest, folder, unityrs):
    """The manifest with budget rows authored from its bundles alone."""
    spec = {key: value for key, value in manifest["spec"].items() if key != "budgets"}
    require(spec["mode"] in ("animator", "legacy") and "humanoid" not in spec["evidence"],
            "budgets are authored only for generic and legacy playback")
    bundles = Bundles(spec, folder, unityrs)
    clip = bundles.value(spec["clip"])
    tracks = explicit_tracks(clip) if spec["mode"] == "legacy" else compiled_tracks(clip)
    start, stop = float(spec["start"]), float(spec["stop"])
    paths = node_paths(spec)
    budgets = []
    claimed = set()

    def interval_times(sources):
        inside = {t for source in sources for t in source.times() if start < t < stop}
        return sorted({start, stop} | inside)

    for target, (row, path) in enumerate(zip(spec["nodes"], paths)):
        transform = bundles.value(row["id"])
        rest = {"translation": vector(transform["m_LocalPosition"], 3),
                "rotation": vector(transform["m_LocalRotation"], 4),
                "scale": vector(transform["m_LocalScale"], 3)}
        for prop in TRS:
            keys = [(crc(path), name) for name in ((prop, "euler") if prop == "rotation" else (prop,))
                    if path is not None and (crc(path), name) in tracks]
            require(len(keys) <= 1, f"node {target} has two rotation writers")
            sources, order = tracks[keys[0]] if keys else ([], None)
            claimed.update(keys)
            budgets.append(budget(prop, target, -1, rest[prop], sources, order, interval_times(sources)))
    for target, row in enumerate(spec["renderers"]):
        weights = floats(bundles.value(row["id"]).get("m_BlendShapeWeights", []))
        path = paths[[node["address"] for node in spec["nodes"]].index(row["address"])]
        for channel in row["channels"]:
            hashes = {crc(channel["name"]), crc("blendShape." + channel["name"])}
            keys = [key for key in tracks if path is not None and key[0] == crc(path)
                    and isinstance(key[1], tuple) and key[1][1] in hashes]
            require(len(keys) <= 1, f"renderer {target} channel {channel['index']} has two writers")
            sources = tracks[keys[0]][0] if keys else []
            claimed.update(keys)
            rest = [weights[channel["index"]] if channel["index"] < len(weights) else 0.]
            budgets.append(budget("weight", target, channel["index"], rest, sources, None,
                                  interval_times(sources)))
    require(claimed == set(tracks), "a playable binding has no recorded target")
    return manifest | {"spec": spec | {"budgets": budgets}}


def verify(manifest, folder, unityrs):
    """Refuse stored budget rows that differ from the bundle derivation."""
    stored = manifest["spec"].get("budgets")
    require(stored is not None, "the replay has no budgets to verify")
    # A JSON round trip compares the rows as they are stored.
    derived = json.loads(json.dumps(author(manifest, folder, unityrs)["spec"]["budgets"], allow_nan=False))
    require(len(derived) == len(stored), "budget rows differ from the bundle derivation")
    for index, (ours, theirs) in enumerate(zip(derived, stored)):
        require(ours == theirs, f"budget row {index} differs from the bundle derivation")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("replay", type=Path)
    parser.add_argument("--bundle-dir", type=Path, required=True)
    parser.add_argument("--unityrs", default="unityrs")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    try:
        manifest = read_json(args.replay.read_text(encoding="utf-8"))
        if args.verify:
            verify(manifest, args.bundle_dir, args.unityrs)
            return
        require("budgets" not in manifest["spec"], "the replay already has budgets")
        result = author(manifest, args.bundle_dir, args.unityrs)
        print(json.dumps(result, indent=2, ensure_ascii=False, allow_nan=False))
    except (ValueError, OSError, KeyError, IndexError, TypeError) as exc:
        parser.exit(1, f"anim_replay_budgets: {exc}\n")


if __name__ == "__main__":
    main()
