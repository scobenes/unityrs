"""B5a's confirmation of a full-body clip: the clip a capture played is the builder's, and its muscle values and
their errors come from Unity's compiled curves (anim_muscle_confirm).

- `builder_curves`: the muscle curves round 11's builder authors for a clip. Every one is a polyline (its
  segments' tangents are the segment slopes):
  - each full-body curve (anim_fixture_full.full_groups);
  - on a refusal rig, the Spine's three from AuthorHumanoidClip (anim_fixture_body.ANCHORS).
- `check_authored`: the capture's clip name and its authored Animator muscle curves are the builder's. The key
  times and values must match exactly in f32, and the tangents that shape each segment must match within f32
  rounding.
- `check_root_settings`: the built clip's root settings are the variant's (anim_fixture_body.check_clip's and
  anim_fixture_round11.clip_configuration's), so variants sharing their muscle curves stay apart.
- `compiled_muscles`: the built clip's muscle tracks, read as the runtime reads them (clip.rs `read`):
  - `m_IndexArray` holds the full DOF layout;
  - every compiled humanoid binding sits at its entry's slot (anim_fixture_scope.check_human_mapping);
  - every entry other than -1 is owned by its own DOF's binding (`ConflictingHumanMapping` otherwise);
  - the storage covers the bindings exactly.
  Each mapped muscle's track must be the builder's curve: a streamed one's coefficients, or a dense one's every
  frame, within f32 rounding of the polyline's.
  An inflated tangent in the built clip, which would widen a sample's bound, is refused.
- `compiled_values`: each muscle's value at a sample's source time from its compiled track (0 unmapped). Its
  error bound has the same terms as `anim_muscle_pose.clip_values`, at the compiled cubic's operands.
"""

import math
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_humanoid_qualify import MUSCLE_COUNT  # noqa: E402

DOF_COUNT = 200
U32 = 2.0 ** -24
GAMMA = 12 * U32 / (1 - 12 * U32)
# Coefficients within this many f32 roundings of the polyline's, scaled by their operands.
ROUNDINGS = 16


def f32(value):
    return struct.unpack(">f", struct.pack(">f", value))[0]


def builder_curves(names, rig, variant):
    """{Animator property: (key times, values)} of the muscle curves the builder authors for a full-body clip.
    `names` is HumanTrait's muscle order, which the full-body curves' shapes follow."""
    from anim_fixture_body import ANCHORS, CLIP_VARIANTS, SPINE
    from anim_fixture_full import SCOPED_RIGS, full_groups

    clip = CLIP_VARIANTS[variant]
    replay.require(not clip["knee"] and not clip["goals"], f"full-body variant {variant} curves knee or goal muscles")
    curves = {prop: (tuple(times), tuple(values)) for (prop, _, times, values), in full_groups(list(names), rig)}
    if rig not in SCOPED_RIGS:
        times = tuple(sorted(ANCHORS))
        curves |= {muscle: (times, tuple(ANCHORS[t][i] for t in times)) for i, muscle in enumerate(SPINE)}
    return curves


def segments(times, values):
    """A polyline's segments as (start, length, start value, end value, slope), in f32 as the builder's C#."""
    rows = []
    for (t0, v0), (t1, v1) in zip(zip(times, values), zip(times[1:], values[1:])):
        rows.append((f32(t0), f32(f32(t1) - f32(t0)), f32(v0), f32(v1),
                     f32(f32(f32(v1) - f32(v0)) / f32(f32(t1) - f32(t0)))))
    return rows


def near(value, want, scale):
    return abs(value - want) <= ROUNDINGS * U32 * scale


def check_authored(case, doc, curves, clip_name):
    """The capture's clip is the builder's `clip_name` and its authored muscle curves are `curves`."""
    from anim_fixture_full import muscle_property

    spec = doc["spec"]
    replay.require(spec.get("clip_name") == clip_name, f"{case} plays clip {spec.get('clip_name')!r}, "
                                                       f"not the builder's {clip_name!r}")
    names = {muscle_property(row["name"]) for row in doc["capture"]["humanoid"]["muscles"]}
    authored = {row["property"]: row["keys"] for row in spec["evidence"]["editor"]["bindings"]
                if (row["path"], row["type"]) == ("", "UnityEngine.Animator") and row["property"] in names}
    replay.require(authored.keys() == curves.keys(), f"{case} authors muscle curves "
                                                     f"{sorted(authored.keys() ^ curves.keys())[:5]} unlike the builder")
    for prop, (times, values) in curves.items():
        keys = sorted(authored[prop], key=lambda key: replay.f32_from_bits(key["time_f32"]))
        rows = [tuple(replay.f32_from_bits(key[field]) for field in
                      ("time_f32", "value_f32", "in_tangent_f32", "out_tangent_f32")) for key in keys]
        good = all(key.get("weighted_mode", "None") == "None" for key in keys) \
            and [(t, v) for t, v, _, _ in rows] == [(f32(t), f32(v)) for t, v in zip(times, values)]
        for k, (_, dt, v0, v1, slope) in enumerate(segments(times, values)):
            scale = abs(slope) + (abs(v0) + abs(v1)) / dt
            good = good and near(rows[k][3], slope, scale) and near(rows[k + 1][2], slope, scale)
        replay.require(good, f"{case}'s authored {prop} is not the builder's polyline")


def check_root_settings(case, variant, clip):
    """The built clip's root settings are the variant's: its bake and keep flags, height from feet, level and
    orientation offset, and the settings B4 reads beyond them."""
    import anim_fixture_round11 as r11
    from anim_fixture_body import CLIP_VARIANTS, SERIALIZED_ROOT, _built

    want = CLIP_VARIANTS[variant]
    built = dict(zip(SERIALIZED_ROOT[1:8], (*want["bake"], *want["keep"], want["height_from_feet"])))
    built |= {"m_Level": want["level"], "m_OrientationOffsetY": want["orientation"]}
    muscle = clip.get("m_MuscleClip", {})
    replay.require(all(_built(muscle.get(key), value, replay) for key, value in built.items()),
                   f"{case}: the built clip's root settings are not the {variant} variant's")
    r11.clip_configuration(case, dict(variant=variant), clip, replay)


def owners(clip):
    """{slot: Animator DOF} of the built clip's humanoid bindings, each binding counted at its width."""
    from anim_fixture_contract import TRANSFORM_CURVES

    widths = {attribute: len(axes) for _, attribute, axes in TRANSFORM_CURVES.values()}
    found, slot = {}, 0
    for row in clip.get("m_ClipBindingConstant", {}).get("genericBindings", []):
        class_id, attribute = row.get("typeID", row.get("classID")), row.get("attribute")
        if class_id == 95 and row.get("customType") == 8:
            found[slot] = attribute
        slot += widths.get(attribute, 1) if class_id == 4 else 1
    return found


def compiled_muscles(case, clip, curves, names):
    """{muscle ordinal: compiled track} for every mapped muscle of the built clip, each checked against its
    builder curve; refuses an index, a binding or a track unlike the builder's or the runtime's layout."""
    from anim_fixture_contract import compiled_curves
    from anim_fixture_full import muscle_property
    from anim_fixture_scope import check_human_mapping

    index = clip.get("m_MuscleClip", {}).get("m_IndexArray")
    replay.require(isinstance(index, list) and len(index) == DOF_COUNT and all(type(v) is int for v in index),
                   f"{case}: the built clip's m_IndexArray is not the {DOF_COUNT}-DOF humanoid layout")
    check_human_mapping(case, clip, replay)
    owned = owners(clip)
    unowned = [dof for dof, entry in enumerate(index) if entry != -1 and owned.get(entry) != dof]
    replay.require(not unowned, f"{case}: m_IndexArray maps DOF(s) {unowned[:5]} to slots no binding of theirs "
                                "owns (a conflicting mapping)")
    tracks = compiled_curves(clip, replay)
    bindings = clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    replay.require(bool(tracks) == bool(bindings) and len(tracks) == len(bindings),
                   f"{case}: the built clip's storage does not cover its bindings")
    total = sum(len(spans) for spans in tracks.values())
    found = {}
    for muscle in range(MUSCLE_COUNT):
        entry = index[42 + muscle]
        if not 0 <= entry < total:
            continue
        spans = tracks.get((0, 95, 42 + muscle, 8, False))
        replay.require(spans is not None and len(spans) == 1,
                       f"{case}: m_IndexArray maps muscle {names[muscle]} to a slot not its own")
        found[muscle] = spans[0]
    props = {muscle_property(names[m]): m for m in range(MUSCLE_COUNT)}
    replay.require({props[prop] for prop in curves} == found.keys(),
                   f"{case}: the built clip compiles other muscles than the builder curves")
    for prop, (times, values) in curves.items():
        replay.require(check_track(found[props[prop]], times, values),
                       f"{case}: the built {prop} differs from the builder's beyond f32 rounding")
    return found


def check_track(track, times, values):
    """A compiled track within f32 rounding of the builder's polyline: a streamed one coefficient by coefficient
    at the same key times, a dense or constant one by value at each key and between them."""
    is_streamed, keys = track
    rows = segments(times, values)
    if is_streamed:
        if [time for time, _ in keys] != [f32(t) for t in times]:
            return False
        for (_, (c3, c2, c1, c0)), (_, dt, v0, v1, slope) in zip(keys, rows):
            ends, steep = abs(v0) + abs(v1), abs(slope)
            if not (near(c0, v0, ends) and near(c1, slope, steep + ends / dt) and near(c2, 0, (steep + ends / dt) / dt)
                    and near(c3, 0, (steep + ends / dt) / dt / dt)):
                return False
        last = keys[-1][1]
        return near(last[3], f32(values[-1]), abs(values[-1])) and last[:3] == (0, 0, 0)
    if len(keys) == 1:
        return all(near(keys[0][1][3], v, abs(v)) for v in values)
    # Every dense frame against the polyline at its time, and the frames spanning the curve's keys.
    first, last = f32(times[0]), f32(times[-1])
    if not (keys[0][0] <= first and keys[-1][0] >= last):
        return False
    for time, (_, _, _, value) in keys:
        for t0, dt, v0, v1, _ in rows:
            if t0 <= time <= t0 + dt:
                want = v0 + (time - t0) / dt * (v1 - v0)
                if not near(value, want, abs(v0) + abs(v1)):
                    return False
                break
    return True


def track_value(track, time):
    """A compiled track's value at `time` and its operands' magnitude: a streamed key's cubic in Horner form, a
    dense track's linear interpolation between frames, a constant."""
    is_streamed, keys = track
    replay.require(bool(keys), "an empty compiled track")
    k = max([i for i, (t, _) in enumerate(keys) if t <= time] or [0])
    if is_streamed:
        start, (c3, c2, c1, c0) = keys[k]
        if time < start or k == len(keys) - 1:
            return c0, abs(c0)
        t = time - start
        return ((c3 * t + c2) * t + c1) * t + c0, ((abs(c3) * t + abs(c2)) * t + abs(c1)) * t + abs(c0)
    first = keys[k][1][3]
    if len(keys) == 1 or k == len(keys) - 1 or time <= keys[k][0]:
        return first, abs(first)
    second, span = keys[k + 1][1][3], keys[k + 1][0] - keys[k][0]
    return first + (second - first) * min(1., (time - keys[k][0]) / span), abs(first) + abs(second)


def compiled_values(tracks, time):
    """Every muscle's value at source time `time` from its compiled track (0 where none), and each one's error
    bound: the f32 evaluation at its operands' magnitude (gamma_12), the time's own f32 rounding and the value's
    f32 rounding."""
    replay.require(type(time) in (int, float) and math.isfinite(time), "a clip sample without a source time")
    values, errors = [0.] * MUSCLE_COUNT, [0.] * MUSCLE_COUNT
    for muscle, track in tracks.items():
        value, magnitude = track_value(track, time)
        other, _ = track_value(track, f32(time))
        values[muscle] = f32(value)
        errors[muscle] = GAMMA * magnitude + abs(value - other) + abs(value) * U32
    return values, errors
