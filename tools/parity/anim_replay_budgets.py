"""Author the per-recording budget rows of a completed generic or legacy replay.

The rows follow the per-recording f32 tolerance derivation in
docs/anim_reference.md. Each row's track kind and stage ranges come from
the replay's own digest-pinned bundles, read through `unityrs show`: the
serialized rest pose, compiled or explicit curve data and the playback
interval. Observed poses are never read. The Rust same-bundle comparison
checks every range and track kind against its own decoding, and
`anim_replay.py derive` turns the rows into frozen per-sample bounds.

    python3 -I tools/parity/anim_replay_budgets.py REPLAY --bundle-dir DIR \
        [--unityrs BIN] > budgeted-replay.json

Supported paths: rest, constant, held and streamed compiled slots, explicit
Hermite curves, and Euler input through the joint stage. Dense, weighted and
humanoid paths are refused until they have a budget author.
"""
import argparse
import json
import math
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import child  # noqa: E402
from anim_replay import TRS, read_json, require  # noqa: E402

STREAM_SEED, STREAM_END = 0xff7fffff, 0x7f800000
# Dyadic pieces per key interval for the rotation output boxes. The Rust
# audit splits intervals at midpoints, so these align with its subdivision.
PIECES = 64
# Outward padding of every recorded range, relative and absolute.
RELATIVE_PAD, ABSOLUTE_PAD = 2**-40, 2**-140
EXPLICIT = {"m_PositionCurves": ("translation", 3), "m_ScaleCurves": ("scale", 3),
            "m_RotationCurves": ("rotation", 4), "m_EulerCurves": ("euler", 3)}
AXES = "xyzw"
EULER_ORDERS = ((0, 1, 2), (0, 2, 1), (1, 2, 0), (1, 0, 2), (2, 0, 1), (2, 1, 0))


def f32(value):
    """Widen a decimal f32 rendering back to its exact stored value."""
    return struct.unpack("<f", struct.pack("<f", value))[0]


def from_bits(word):
    return struct.unpack("<f", struct.pack("<I", word))[0]


def crc(text):
    return zlib.crc32(text.encode("utf-8"))


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


class Bundles:
    """The replay's bundles, read object by object through `unityrs show`."""

    def __init__(self, spec, folder, unityrs):
        self.names = [row["name"] for row in spec["bundles"]]
        self.folder, self.unityrs = folder, unityrs

    def value(self, ident):
        name = self.names[ident["bundle"]]
        result = child.run([self.unityrs, "show", name, "--path-id", ident["path_id"],
                            "--file-index", f"0:{ident['file_index']}", "--max-items", "1000000",
                            "--max-bytes", "1073741824", "--max-depth", "128"],
                           cwd=self.folder, capture_output=True, text=True)
        require(result.returncode == 0, f"unityrs show {name}: {(result.stderr or result.stdout).strip()}")
        doc = read_json(result.stdout)
        require(doc.get("format") == "unityrs.show" and doc["object"]["file"] == ident["file"],
                f"unexpected show document for {name}")
        return doc["value"]


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
        parts = address[len(prefix):].split("/")
        base = prefix.rstrip("/")
        names_below = []
        for part in parts:
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
    constants = [f32(value) for value in playback["m_ConstantClip"]["data"]]
    keys = [[] for _ in range(streamed)]
    seeds = {}
    words, cursor = stream["data"], 0
    while cursor < len(words):
        bits, count = words[cursor:cursor + 2]
        entries = [words[i:i + 5] for i in range(cursor + 2, cursor + 2 + 5 * count, 5)]
        cursor += 2 + 5 * count
        if bits == STREAM_END:
            break
        for slot, *raw in entries:
            coefficients = tuple(map(from_bits, raw))
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


def explicit_tracks(clip):
    tracks = {}
    for field, (prop, width) in EXPLICIT.items():
        for row in clip.get(field, []):
            curve = row["curve"]
            require(curve["m_PreInfinity"] == 2 and curve["m_PostInfinity"] == 2,
                    "only clamped explicit curves have budgets")
            require(all(key.get("weightedMode", 0) == 0 for key in curve["m_Curve"]),
                    "weighted budgets are not authored yet")
            span = [Hermite([(f32(key["time"]), f32(key["value"][a]), f32(key["inSlope"][a]),
                              f32(key["outSlope"][a])) for key in curve["m_Curve"]])
                    for a in AXES[:width]]
            order = EULER_ORDERS[curve["m_RotationOrder"]] if prop == "euler" else None
            tracks[crc(row["path"]), prop] = (span, order)
    for row in clip.get("m_FloatCurves", []):
        attribute = row["attribute"]
        if row["classID"] == 137 and attribute.startswith("blendShape."):
            keys = [(f32(key["time"]), f32(key["value"]), f32(key["inSlope"]), f32(key["outSlope"]))
                    for key in row["curve"]["m_Curve"]]
            tracks[crc(row["path"]), ("weight", crc(attribute.removeprefix("blendShape.")))] = (
                [Hermite(keys)], None)
    return tracks


class Ranges:
    """Inclusive envelopes of every value one budget stage reads."""

    def __init__(self, width):
        self.lo, self.hi = [math.inf] * width, [-math.inf] * width

    def add(self, i, lo, hi=None):
        hi = lo if hi is None else hi
        self.lo[i], self.hi[i] = min(self.lo[i], lo), max(self.hi[i], hi)

    def rows(self):
        def pad(x):
            return abs(x) * RELATIVE_PAD + ABSOLUTE_PAD
        # Padding keeps a nonnegative range nonnegative, as streamed time
        # deltas must be.
        return [[max(0., lo - pad(lo)) if lo >= 0 else lo - pad(lo), hi + pad(hi)]
                for lo, hi in zip(self.lo, self.hi)]


WIDTHS = {"rest": 1, "constant": 1, "held": 1, "streamed": 5, "hermite": 5}


def record(ranges, stage, segment, lo, hi):
    if segment.kind in ("constant", "held"):
        ranges.add(3 if stage == "streamed" else 0, segment.value)
    elif segment.kind == "streamed":
        for i, value in enumerate(segment.coefficients):
            ranges.add(i, value)
        ranges.add(4, lo - segment.key, hi - segment.key)
    else:
        for i, value in enumerate(segment.hermite):
            ranges.add(i, value)


def multiply(a, b):
    x, y, z, w = a
    p, q, r, s = b
    return (w * p + x * s + y * r - z * q, w * q - x * r + y * s + z * p,
            w * r + x * q - y * p + z * s, w * s - x * p - y * q - z * r)


def euler(order, degrees):
    result = (0., 0., 0., 1.)
    for axis in order:
        half = degrees[axis] * math.pi / 360
        factor = [0., 0., 0., math.cos(half)]
        factor[axis] = math.sin(half)
        result = multiply(tuple(factor), result)
    norm = math.sqrt(sum(x * x for x in result))
    return tuple(x / norm for x in result)


def budget(prop, target, channel, rest, sources, order, times):
    """One budget row in the order the Rust audit walks the playback path."""
    joint = order is not None

    def axis_source(axis):
        return sources[axis] if axis < len(sources) else None

    name = None
    for lo, hi in zip(times, times[1:]):
        mid = lo + (hi - lo) * .5
        for axis in range(len(rest)):
            source = axis_source(axis)
            stage = source.segment(mid).stage() if source else None
            if stage is not None:
                # A held segment of a streamed track is a cubic with zero
                # slopes, which the streamed stage bounds.
                if {name, stage} == {"held", "streamed"}:
                    name = stage = "streamed"
                require(name in (None, stage), f"{prop} {target} mixes {name} and {stage} segments")
                name = stage
    name = name or ("constant" if sources else "rest")
    require(name in WIDTHS, f"{name} budgets are not authored yet")
    ranges = Ranges(WIDTHS[name])
    for index, time in enumerate(times):
        for axis in range(len(rest)):
            source = axis_source(axis)
            if source is None:
                record(ranges, name, Segment("constant", rest[axis]), time, time)
            elif (segment := source.segment(time)).kind in ("constant", "held"):
                record(ranges, name, segment, time, time)
        if index + 1 < len(times):
            stop = times[index + 1]
            for axis in range(len(sources)):
                record(ranges, name, sources[axis].segment(time + (stop - time) * .5), time, stop)
    kind = f"euler_{name}" if joint else name
    stages = [{"stage": name, "range": ranges.rows()}]
    if joint:
        stages.append({"stage": "joint", "range": []})
    row = {"property": prop, "target": target, "channel": channel, "track_kind": kind, "stages": stages}
    if prop == "weight":
        row["weight_units"] = "editor"
    if prop == "rotation":
        boxes, degrees = rotation_boxes(rest, sources, order, times)
        if joint:
            stages[-1]["range"] = degrees.rows()
        stages[-1]["norm_ranges"] = boxes
    return row


def rotation_boxes(rest, sources, order, times):
    """Raw (or joint) quaternion boxes over dyadic pieces of each interval."""
    degrees = Ranges(1)
    if not sources:
        return [[[x - ABSOLUTE_PAD - abs(x) * RELATIVE_PAD, x + ABSOLUTE_PAD + abs(x) * RELATIVE_PAD]
                 for x in rest]], degrees
    spans = list(zip(times, times[1:])) or [(times[0], times[0])]
    boxes = []
    for lo, hi in spans:
        segments = [source.segment(lo + (hi - lo) * .5) for source in sources]
        steps = PIECES if hi > lo else 1
        for piece in range(steps):
            a = lo + (hi - lo) * piece / steps
            b = hi if piece + 1 == steps else lo + (hi - lo) * (piece + 1) / steps
            h = b - a
            rates = [segment.rate(b) for segment in segments]
            ends = [[segment.at(t) for segment in segments] for t in (a, b)]
            if order is not None:
                for axis in range(3):
                    pad = h * rates[axis]
                    degrees.add(0, min(e[axis] for e in ends) - pad, max(e[axis] for e in ends) + pad)
                quaternions = [euler(order, end) for end in ends]
                pads = [h * sum(rates) * math.pi / 360] * 4
            else:
                quaternions, pads = ends, [h * rate for rate in rates]
            box = []
            for axis in range(4):
                lo_value = min(q[axis] for q in quaternions) - pads[axis]
                hi_value = max(q[axis] for q in quaternions) + pads[axis]
                box.append([lo_value - abs(lo_value) * RELATIVE_PAD - ABSOLUTE_PAD,
                            hi_value + abs(hi_value) * RELATIVE_PAD + ABSOLUTE_PAD])
            boxes.append(box)
    return boxes, degrees


def author(manifest, folder, unityrs):
    spec = manifest["spec"]
    require(spec["mode"] in ("animator", "legacy") and "humanoid" not in spec["evidence"],
            "budgets are authored only for generic and legacy playback")
    require("budgets" not in spec, "the replay already has budgets")
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
        weights = [f32(x) for x in bundles.value(row["id"]).get("m_BlendShapeWeights", [])]
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


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("replay", type=Path)
    parser.add_argument("--bundle-dir", type=Path, required=True)
    parser.add_argument("--unityrs", default="unityrs")
    args = parser.parse_args()
    try:
        manifest = read_json(args.replay.read_text(encoding="utf-8"))
        result = author(manifest, args.bundle_dir, args.unityrs)
        print(json.dumps(result, indent=2, ensure_ascii=False, allow_nan=False))
    except (ValueError, OSError, KeyError, IndexError) as exc:
        parser.exit(1, f"anim_replay_budgets: {exc}\n")


if __name__ == "__main__":
    main()
