"""B5a's confirmation on round 11 (docs/plans/humanoid-body.md B5a (g)), built so it cannot pass vacuously.

    python3 -I tools/parity/anim_muscle_confirm.py --unityrs PATH <round 11 run folder>...

It reads no case file, only the run folders.

**What it guards against.** Confirmation guards against an incomplete, buggy or misrouted collection: a recorder
or collector that skips a case, a bone or a sample, repeats one, files a capture under the wrong case, or reads
the wrong transform or clip. It does not guard against someone hand-editing capture files. Integrity against
later edits is to come from a sha256 manifest the Editor writes at record time.

**Expected** (`expected`): every (Editor, case, bone, sample) that round 11 must record. It is computed from
round 11's builder tables, never from literals:
- the cases: the four muscle sweeps (anim_fixture_muscles.MUSCLE_SWEEPS), and every full-body clip of round
  11's case table (anim_fixture_round11.ROUND11_INFO);
- a case's bones: every bone its rig's Avatar maps (anim_fixture_round11.rig_bones, which leaves out the bones
  its topology omits) that some muscle turns;
- a sample's id: a sweep vector's index in the builder's schedule (anim_fixture_round11.probe_schedule), or a
  clip sample's time on the recorder's schedule over the clip's key times (`clip_times`, the recorder's
  Schedule);
- the Editors: the two frozen series, `EDITORS`, matched by minor version.

**Observed** (`observe`): what the run folders recorded, from their raw captures and bundles.
- Each run must declare round 11 and record its whole body inventory (anim_fixture_round11.check_inventory).
- Each capture's own metadata names its case, run, Editor and bundle (anim_muscle_round11.Recordings).
- A sample is identified as above, its time from the same source time the prediction and bound use (one
  whose recorded local time disagrees is refused). A sweep's must be one of the builder's vectors, at its time
  and with its values, and a clip's must be one of the schedule's times. A sample recorded twice is refused.
- Each capture must be the builder's case: its prefab, clip and Avatar assets and its playback mode
  (anim_fixture_contract.check_identity), and its Animator placement in the recorded rest pose (the prefab root
  at the identity, the Animator node at the case's root, `placement`).
- Each recorded bone's transform must be the Avatar's node for that bone, by path (`m_Skeleton.m_ID` through
  `m_TOS` against the capture's node paths), and its recorded rest rotation that node's in `m_SkeletonPose`.
- A clip's capture must play the builder's clip: its authored and compiled muscle curves within f32 rounding of
  the builder's, and its root settings the variant's (anim_muscle_clip). The compiled index array must hold the
  runtime's layout, every entry owned by its own binding.
- Each (bone, sample) is compared with reference 2 on the bundle's Avatar, using the capture's recorded
  rotation. A clip's muscle values and their errors come from the compiled curves. The bound comes from the
  frozen rule (`anim_muscle_pose.unity_bound`) at the sample's own values, never from a file.

**Scope** (`scope`) is the runtime's admission, `humanoid_refusal` (mod.rs), computed from the same inputs:
- a clip's animated bones are those of its mapped muscle slots (an `m_IndexArray` entry naming stored data,
  anim_muscle_clip.compiled_muscles) whose bones the Avatar maps with axes. A sweep plays no clip, so its animated bones are those the builder's vectors move.
- An unmapped required bone, or a mapped one without axes, is refused as Rust refuses it.
- A case is twist evidence where a twist setting is not neutral and the case animates a bone that setting
  redistributes over.

Refused samples are reported in their own table. They never count, and cannot stand in for a required one.

**Passes** if and only if, in each Editor, the observed in-scope (case, bone, sample) set equals the expected
one exactly (nothing missing, nothing extra), and every sample is within its bound.
"""

import argparse
import math
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_humanoid_qualify import LIMBS, MUSCLE_COUNT, REQUIRED_BONES, TWIST, muscle_bone  # noqa: E402

# The two Editor series the plan freezes, round 10's and round 11's: {Editor: its version's prefix}.
EDITORS = {"2022": "2022.3.", "6000": "6000.6."}
# The recorder's off-key sample, this far into each key interval (UnityrsFixtureRecording.cs Schedule; a test
# reads it there).
RECORDER_OFFSET = 391 / 1024


def f32_bits(value):
    return struct.pack(">f", value).hex()


def f32(value):
    return struct.unpack(">f", struct.pack(">f", value))[0]


def editor_of(version):
    """The frozen series a version belongs to. Any other is refused, a 2022.1 or a 6000.0 among them."""
    found = [key for key, prefix in EDITORS.items() if isinstance(version, str) and version.startswith(prefix)]
    replay.require(len(found) == 1, f"Editor {version} is not one of B5a's frozen series "
                                    f"{', '.join(prefix + 'x' for prefix in EDITORS.values())}")
    return found[0]


def bone_names():
    """HumanBodyBones' names by index, as HumanTrait.BoneName gives them: the body's 24, the 30 finger bones,
    then UpperChest."""
    from anim_body_tolerance import NAMES
    from anim_fixture_muscles import FINGER_BONES

    return (*NAMES[:24], *FINGER_BONES, NAMES[24])


def builder_muscles():
    """{muscle: its bone's name}: the 95 muscles round 11's builder curves and sweeps, in the builder's order."""
    from anim_fixture_body import PROBE_GROUPS
    from anim_fixture_muscles import FINGER_GROUPS, LIMB_TWISTS, muscle_bone as bone_of

    names = [*(m for _, group in PROBE_GROUPS for m in group), *(m for m, _ in LIMB_TWISTS),
             *(m for group in FINGER_GROUPS for m in group)]
    replay.require(len(set(names)) == len(names) == MUSCLE_COUNT, "the builder's muscles are not HumanTrait's 95")
    return {name: bone_of(name, PROBE_GROUPS) for name in names}


def b5a_cases():
    """{case: (kind, rig, variant)}: the four muscle sweeps, and every full-body clip round 11's builder makes."""
    import anim_fixture_round11 as r11
    from anim_fixture_muscles import MUSCLE_SWEEPS

    return {name: (info["kind"], info["rig"], info["variant"]) for name, info in r11.ROUND11_INFO.items()
            if name in MUSCLE_SWEEPS or info["kind"] == "clip" and info["rig"] in r11.FULL_CLIP_RIGS}


def case_bones(rig):
    """A case's bones, as HumanBodyBones indices: every bone its rig maps that some muscle turns."""
    import anim_fixture_round11 as r11

    names, mapped = bone_names(), r11.rig_bones(rig)
    return frozenset(bone for bone in {muscle_bone(m) for m in range(MUSCLE_COUNT)} if names[bone] in mapped)


def clip_times(keys):
    """The recorder's samples of a clip that starts at 0 (UnityrsFixtureRecording.cs Schedule, no held boundary):
    each key, and an off-key sample RECORDER_OFFSET of the way into each interval where it is a distinct f32."""
    times = {}
    for i, key in enumerate(keys):
        times[f32(key)] = key
        if i:
            off = keys[i - 1] + (key - keys[i - 1]) * RECORDER_OFFSET
            if f32(keys[i - 1]) < f32(off) < f32(key):
                times[f32(off)] = off
    times[0.] = 0.
    times[f32(keys[-1])] = keys[-1]
    return tuple(times[t] for t in sorted(times))


def clip_keys(rig, variant):
    """A full-body clip's key times, as the builder authors them: every full-body curve's, the body curves'
    (BODY_TIMES), and the knee and foot-goal curves' if the variant has them."""
    from anim_fixture_body import BODY_TIMES, CLIP_VARIANTS, KNEE_TIMES
    from anim_fixture_full import full_groups

    clip = CLIP_VARIANTS[variant]
    times = {*BODY_TIMES, *(KNEE_TIMES if clip["knee"] or clip["goals"] else ())}
    times |= {t for group in full_groups(list(builder_muscles()), rig) for row in group for t in row[2]}
    return tuple(sorted(times))


def samples(case, kind, rig, variant):
    """{sample id: what the builder sets there}. For a sweep, each vector's index maps to its time's and values'
    f32 bits, (time, {muscle: value}). For a clip, each sample time's f32 bits map to None, since the values are
    the clip's own curves, which completion checks against the builder."""
    import anim_fixture_round11 as r11

    if kind == "probes":
        names = list(builder_muscles())
        return {k: (f32_bits(t), tuple(zip(names, map(f32_bits, vector))))
                for k, (t, vector) in enumerate(r11.probe_schedule(rig, names, None, case))}
    return {f32_bits(t): None for t in clip_times(clip_keys(rig, variant))}


def expected(cases=None):
    """{(Editor, case, bone, sample id)}: everything round 11 must record, for the builder's cases (`cases`
    narrows them for a test)."""
    found = set()
    for case, (kind, rig, variant) in sorted((cases or b5a_cases()).items()):
        ids, bones = samples(case, kind, rig, variant), case_bones(rig)
        found |= {(editor, case, bone, sid) for editor in EDITORS for bone in bones for sid in ids}
    return found


def scope(human, moved):
    """None for a case in B5a's scope, else the runtime's refusal. `moved` is the muscle ordinals a clip maps
    (anim_muscle_clip.compiled_muscles), or those a sweep's vectors set."""
    import anim_muscle_pose as pose

    if any(pose.avatar_node(human, bone) is None for bone in REQUIRED_BONES):
        return "missing_human_bone"
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    animated = set()
    for muscle in sorted(moved):
        bone = muscle_bone(muscle)
        node = pose.avatar_node(human, bone)
        if node is None:
            continue
        if skeleton["m_Node"][node]["m_AxesId"] < 0:
            return "unsupported_humanoid_axes"
        animated.add(bone)
    values = [human.get(field) for field in TWIST]
    replay.require(all(type(v) in (int, float) for v in values), "the Avatar lacks a twist setting")
    twisted = any(value != 0 and set(LIMBS[field]) & animated for field, value in zip(TWIST, values))
    return "unsupported_humanoid_twist" if twisted else None


class Observation:
    """What the run folders recorded. `rows` maps (Editor, case, bone, sample id) to (angle, bound) in scope, and
    `refused` to (angle, bound, reason) for refusal evidence. `runs` is each Editor's run, and `errors` holds every
    reason confirmation fails besides the comparison."""

    def __init__(self):
        self.rows, self.refused, self.runs, self.errors = {}, {}, {}, []


def check_tables(case, capture):
    """The capture's muscle and bone tables against HumanTrait's, by bone and axis, and the builder's muscles."""
    import anim_muscle_pose as pose

    muscles = capture["humanoid"]["muscles"]
    replay.require([(row["bone"], row["axis"]) for row in muscles]
                   == [(muscle_bone(m), pose.MUSCLE_AXES[m]) for m in range(MUSCLE_COUNT)],
                   f"{case}: the recorded muscle axes differ from MUSCLE_AXES")
    names = tuple(row["name"] for row in muscles)
    builder, bones = builder_muscles(), bone_names()
    replay.require(sorted(names) == sorted(builder) and all(builder[name] == bones[muscle_bone(m)]
                                                            for m, name in enumerate(names)),
                   f"{case}: the recorded muscles are not the builder's on the same bones")
    replay.require([row["name"] for row in capture["humanoid"]["bones"]] == list(bones),
                   f"{case}: the recorded bones are not HumanBodyBones")
    return names


def placement(case, info, doc):
    """The capture's Animator placement in its recorded rest pose: the prefab root at the identity, the Animator
    node at the case's root (B4's offset placement on a plain chain's offset root, else the identity) and, on a
    scaled-Animator rig, at its scale."""
    import anim_fixture_round11 as r11

    feature = r11.RIG_FEATURES.get(info["rig"], ("", 1))[0]
    placed = r11.OFFSET_PLACEMENT if info["root"] == "offset" and feature == "" else ((0, 0, 0), (0, 0, 0, 1))
    scale = r11.ANIMATOR_SCALE if feature == "scaled-animator" else 1
    nodes = {node["address"]: k for k, node in enumerate(doc["spec"]["nodes"])}
    rest = doc["capture"]["rest"]["transforms"]

    def at(address, translation, rotation, size):
        row = rest[nodes[address]] if address in nodes and nodes[address] < len(rest) else {}
        return (r11.same(row.get("translation", ()), translation) and r11.same(row.get("rotation", ()), rotation)
                and r11.same(row.get("scale", ()), (size,) * 3))
    replay.require(at("", (0, 0, 0), (0, 0, 0, 1), 1) and at("0", *placed, scale),
                   f"{case} was recorded at another placement than its {info['root']} root")


def observe_capture(editor, case, spec, doc, avatar, clip, seen):
    """One capture's comparisons into `seen`. `spec` is the builder's (kind, rig, variant), `avatar` the bundle's
    Avatar and `clip` its built clip (None for a sweep)."""
    import anim_body_runs as runs
    import anim_fixture_round11 as r11
    import anim_muscle_clip as clips
    import anim_muscle_pose as pose
    from anim_humanoid_qualify import node_paths, tos_paths, vec
    from anim_reference import angle

    from anim_fixture_contract import check_identity

    capture = doc["capture"]
    check_identity(case, r11.ROUND11_CASES[case], doc["spec"], capture, replay)
    placement(case, r11.ROUND11_INFO[case], doc)
    names = check_tables(case, capture)
    ids = samples(case, *spec)
    sweep = spec[0] == "probes"
    human = runs.human_of(avatar)
    # A sweep's vectors by (time, values), its values in the capture's muscle order.
    vectors = {(time, tuple(dict(values)[name] for name in names)): k for k, (time, values) in ids.items()} \
        if sweep else {}
    tracks = {}
    if not sweep:
        curves = clips.builder_curves(names, spec[1], spec[2])
        clips.check_authored(case, doc, curves, r11.ROUND11_CASES[case].clip)
        clips.check_root_settings(case, spec[2], clip)
        tracks = clips.compiled_muscles(case, clip, curves, names)
    sampled = {}
    for sample in capture["samples"]:
        # The id, the prediction and the bound all take the sample's source time.
        source = sample.get("effective_source")
        replay.require(type(source) in (int, float) and math.isfinite(source),
                       f"Editor {editor}'s {case} records a sample without a source time")
        time = f32_bits(source)
        replay.require(str(sample.get("local_f32", "")).lower() == time,
                       f"Editor {editor}'s {case} records a sample whose local time {sample.get('local_f32')} "
                       f"disagrees with its source time {time}")
        if sweep:
            sid = vectors.get((time, tuple(str(v).lower() for v in sample["muscles_f32"])))
        else:
            sid = time if time in ids else None
        replay.require(sid is not None, f"Editor {editor}'s {case} records a sample the builder never scheduled "
                                        f"(at {time})")
        replay.require(sid not in sampled, f"Editor {editor}'s {case} records sample {sid} twice")
        values = (([replay.f32_from_bits(v) for v in sample["muscles_f32"]], [0.] * MUSCLE_COUNT) if sweep
                  else clips.compiled_values(tracks, source))
        sampled[sid] = (sample, values)
    nonzero = {name for _, values in ids.values() for name, bits in values if replay.f32_from_bits(bits) != 0} \
        if sweep else set()
    moved = {m for m, name in enumerate(names) if name in nonzero} if sweep else set(tracks)
    reason = scope(human, moved)
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    bones = capture["humanoid"]["bones"]
    paths, nodes = tos_paths(avatar), node_paths(doc["spec"])
    pose_rows = human["m_SkeletonPose"].get("data", human["m_SkeletonPose"])["m_X"]
    for bone in sorted({muscle_bone(m) for m in range(MUSCLE_COUNT)}):
        node = bones[bone]["node"]
        if node < 0:
            continue
        found = pose.avatar_node(human, bone)
        if found is None or skeleton["m_Node"][found]["m_AxesId"] < 0:
            seen.errors.append(f"Editor {editor}'s {case} records {bones[bone]['name']}, which its Avatar maps "
                               "without axes")
            continue
        path = paths.get(skeleton["m_ID"][found])
        replay.require(path is not None and nodes.get(node) == path,
                       f"Editor {editor}'s {case} records {bones[bone]['name']} from transform {node} "
                       f"({nodes.get(node)!r}), not the Avatar's {path!r}")
        replay.require(r11.same(capture["rest"]["transforms"][node]["rotation"],
                                [float(pose_rows[found]["q"][a]) for a in "xyzw"]),
                       f"Editor {editor}'s {case} records {bones[bone]['name']} at a rest rotation not the Avatar's")
        record = skeleton["m_AxesArray"][skeleton["m_Node"][found]["m_AxesId"]]
        axes = {key: [pose.f32(v) for v in vec(record[name])] for key, name in
                (("pre", "m_PreQ"), ("post", "m_PostQ"), ("sign", "m_Sgn"))}
        axes |= {key: [pose.f32(v) for v in vec(record["m_Limit"][name])] for key, name in
                 (("min", "m_Min"), ("max", "m_Max"))}
        rest = [replay.f32_from_bits(v) for v in capture["rest"]["transforms"][node]["rotation"]]
        ordinals = pose.bone_muscles(bone)
        for sid, (sample, (vector, errors)) in sampled.items():
            values = [(vector[m], pose.MUSCLE_AXES[m]) for m in ordinals]
            local = pose.bone_rotation(rest, axes, values)
            replay.require(local is not None, f"{case}: bone {bone} has no rotation at sample {sid}")
            recorded = [replay.f32_from_bits(v) for v in sample["pose"]["transforms"][node]["rotation"]]
            bound = pose.unity_bound(rest, axes, values, [errors[m] for m in ordinals])
            key = (editor, case, bone, sid)
            if reason is None:
                seen.rows[key] = (angle(local, recorded), bound)
            else:
                seen.refused[key] = (angle(local, recorded), bound, reason)


def observe(folders, avatar_of, clip_of, cases=None):
    """The round 11 runs' comparisons, through `avatar_of` and `clip_of` (anim_body_freeze.readers)."""
    import anim_fixture_round11 as r11
    import anim_muscle_round11 as round11

    builder = b5a_cases()
    cases = cases or builder
    seen = Observation()
    for folder in folders:
        replay.require(round11.declared_round(folder) == round11.ROUND, f"{folder} is not a declared round 11 run")
        r11.check_inventory(folder, replay)
        editor = editor_of(replay.read_json((folder / "run.json").read_text(encoding="utf-8"))["run"]
                           .get("editor_version"))
        run = round11.Recordings(folder)
        replay.require(editor not in seen.runs, f"two round 11 runs of Editor {editor}")
        seen.runs[editor] = run.run_id
        seen.errors += [f"Editor {editor}'s run lacks {case}" for case in run.missing if case in cases]
        for case, doc in sorted(run.documents.items()):
            if case not in builder:
                seen.errors.append(f"Editor {editor}'s run records {case}, a full-body clip the builder never made")
            if case not in cases:
                continue
            clip = None if cases[case][0] == "probes" else clip_of(run, doc)
            observe_capture(editor, case, cases[case], doc, avatar_of(run, doc), clip, seen)
    return seen


def failures(seen, wanted):
    """Why confirmation fails, empty when it passes: an Editor without its run; any other error observing them;
    anything in scope the runs lack or record beyond the expected set; any in-scope sample outside its bound."""
    replay.require(wanted, "the builder expects nothing")
    found = [f"no round 11 run of Editor {editor}" for editor in EDITORS if editor not in seen.runs]
    found += seen.errors

    def shown(keys):
        return ", ".join(f"{e} {c} {bone_names()[b]} sample {s}" for e, c, b, s in sorted(keys, key=str)[:10])
    for title, keys in (("lack", wanted - seen.rows.keys()), ("record beyond the builder's", seen.rows.keys() - wanted)):
        if keys:
            found.append(f"the runs {title} {len(keys)} in-scope (case, bone, sample)(s): {shown(keys)}")
    outside = [key for key, (turn, bound) in seen.rows.items() if not turn <= bound]
    if outside:
        found.append(f"{len(outside)} confirmation sample(s) outside their bound: {shown(outside)}")
    return found


def table(rows):
    """{(Editor, bone): (worst angle, its bound, samples, samples outside)}."""
    result = {}
    for (editor, _, bone, _), (turn, bound, *_) in rows.items():
        worst, at, count, outside = result.get((editor, bone), (0., 0., 0, 0))
        if turn > worst:
            worst, at = turn, bound
        result[editor, bone] = (worst, at, count + 1, outside + (not turn <= bound))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--unityrs", required=True)
    parser.add_argument("folders", type=Path, nargs="+")
    args = parser.parse_args()
    try:
        for folder in args.folders:
            replay.require(folder.is_dir(), f"{folder} is not a run folder (confirm reads no case file)")
        from anim_body_freeze import readers

        avatar_of, clip_of = readers(str(Path(args.unityrs).resolve()))
        seen = observe(args.folders, avatar_of, clip_of)
        names = bone_names()
        for title, rows in (("Confirmation (round 11)", seen.rows), ("Refusal evidence (never counted)", seen.refused)):
            print(f"{title}:")
            for (editor, bone), (worst, bound, count, outside) in sorted(table(rows).items()):
                print(f"{editor} {names[bone]:24s} worst {math.degrees(worst):.3e} deg  bound "
                      f"{math.degrees(bound):.3e} deg  {count} samples  "
                      + ("agrees" if not outside else f"DISAGREES in {outside}"))
        found = failures(seen, expected())
        replay.require(not found, "; ".join(found))
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_muscle_confirm: {exc}\n")


if __name__ == "__main__":
    sys.exit(main())
