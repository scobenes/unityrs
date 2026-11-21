"""B5a's round 12 verdict (round 12 design v4 section 4): its expected rows, its 25 pinned refusal cases with the
runtime's exact reasons, and the exact verdict.

**Rows** (`twist_free`, `b5a_expected`): those whose ten limb-twist values are all 0, from the generated tables:
each round 12 sweep vector without a limb twist, every twin sample, and round 11's finger and face sweeps. A
twist-active row (a limb-twist vector, a twist clip, a probe, a subset sample) never enters B5a.

**Scope** (`refusal`) is B5a's: the runtime's admission of a muscle pose, humanoid_refusal's (mod.rs) checks on the
muscles a case animates: a missing required bone, a mapped bone without axes, a non-neutral twist setting over an
animated limb, a limb-twist slot.

**Refusal rows** are the 25 refusal cases' (round 11's full-body clips outside freeze v8's scope), each with the
runtime's exact reason, its first refusal in humanoid_refusal's whole order (humanoid::refusal in mod.rs): a missing
required bone, an Animator node or ancestor off the identity (`unsupported_animator_root`), a generic Transform
writer on a human bone, locomotion (a mapped motion slot, a root motion bone, a generic root or motion float
curves), IK goals, translation DOFs, an additive reference pose, body slots, then B5a's scope above. Its hierarchy
check comes first and is not modelled: every round 11 rig is one B4 admits. `pinned_refusals` predicts each from
round 11's builder tables before any data (the case's root placement and rig feature, its variant's motion and
goal curves, the RootT and RootQ curves every full-body clip has); the verdict requires the runtime's observed
reason equal to it.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

# humanoid::LIMB_TWIST_MUSCLES: the ten limb-twist muscles' HumanTrait ordinals.
LIMB_TWIST_ORDINALS = (23, 25, 27, 31, 33, 35, 41, 43, 50, 52)


def twist_free():
    """{case: sample ids} of B5a's rows: each round 12 sweep's vectors without a limb twist (by vector index), every
    twin's sample times (f32 bits), and round 11's finger and face sweeps (each vector index,
    anim_muscle_confirm.samples)."""
    import anim_fixture_round12 as round12
    from anim_fixture_muscles import LIMB_TWISTS, MUSCLE_SWEEPS
    from anim_muscle_confirm import b5a_cases, samples

    doc = round12.tables()[0]
    twists = {m for m, _ in LIMB_TWISTS}
    sets = {row["name"]: row["vectors"] for row in doc["sets"]}
    found = {}
    for row in doc["cases"]:
        if row["family"] == "sweep":
            found[row["name"]] = tuple(k for k, vector in enumerate(sets[row["set"]]) if not any(
                name in twists and replay.f32_from_bits(value) != 0
                for name, value in zip(vector["muscles"], vector["bits"])))
        elif row["family"] == "twin":
            found[row["name"]] = tuple(row["times"])
    for case in (s for s in MUSCLE_SWEEPS if s.endswith(("-fingers", "-face"))):
        found[case] = tuple(samples(case, *b5a_cases()[case]))
    return found


def _rig(case):
    import anim_fixture_round11 as r11
    import anim_fixture_round12 as round12

    found = round12.rows()
    return found[case]["rig"] if case in found else r11.ROUND11_INFO[case]["rig"]


def b5a_expected():
    """(rows, refusal rows): {(Editor, case, bone, sample id)} B5a must observe, in scope and as refusal evidence."""
    from anim_muscle_confirm import EDITORS, b5a_cases, case_bones, samples

    rows = {(e, case, bone, sid) for case, ids in twist_free().items() for bone in case_bones(_rig(case))
            for sid in ids for e in EDITORS}
    refused = {(e, case, bone, sid) for case in pinned_refusals() for bone in case_bones(_rig(case))
               for sid in samples(case, *b5a_cases()[case]) for e in EDITORS}
    return rows, refused


def refusal(human, moved):
    """B5a's scope: the runtime's refusal of muscle ordinals `moved` on Avatar `human` among its muscle-pose checks,
    as its message, or None."""
    import anim_muscle_pose as pose
    from anim_humanoid_qualify import LIMBS, REQUIRED_BONES, TWIST, muscle_bone

    for bone in REQUIRED_BONES:
        if pose.avatar_node(human, bone) is None:
            return f"missing_human_bone({bone})"
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    animated = set()
    for muscle in sorted(moved):
        node = pose.avatar_node(human, muscle_bone(muscle))
        if node is None:
            continue
        if skeleton["m_Node"][node]["m_AxesId"] < 0:
            return f"unsupported_humanoid_axes({muscle_bone(muscle)})"
        animated.add(muscle_bone(muscle))
    for field in TWIST:
        value = human.get(field)
        replay.require(type(value) in (int, float), f"the Avatar lacks {field}")
        if value != 0 and set(LIMBS[field]) & animated:
            return f"unsupported_humanoid_twist({field})"
    twisted = sorted(set(moved) & set(LIMB_TWIST_ORDINALS))
    return f"unsupported_humanoid_limb_twist({twisted[0]})" if twisted else None


def pinned_refusals():
    """{case: reason}: B5a's 25 refusal cases, each with the runtime's first refusal predicted from round 11's
    builder tables: its Animator off the identity on an offset root or a scaled-Animator rig; locomotion on a
    variant with motion curves; IK on one with goal curves; otherwise body slots, which every full-body clip's
    RootT and RootQ curves map."""
    import anim_fixture_round11 as r11
    from anim_fixture_body import CLIP_VARIANTS
    from anim_muscle_confirm import b5a_cases

    found = {}
    for case, (kind, rig, variant) in sorted(b5a_cases().items()):
        info = r11.ROUND11_INFO[case]
        if kind != "clip" or info.get("scope") != "refusal":
            continue
        feature = r11.RIG_FEATURES.get(rig, ("", 1))[0]
        clip = CLIP_VARIANTS[variant]
        if info["root"] == "offset" and feature == "" or feature == "scaled-animator":
            found[case] = "unsupported_animator_root"
        elif clip["motion"]:
            found[case] = "unsupported_locomotion"
        elif clip["goals"]:
            found[case] = "unsupported_humanoid_ik"
        else:
            found[case] = "unsupported_body_motion(body_slots)"
    replay.require(len(found) == 25, f"B5a pins 25 refusal cases, not {len(found)}")
    return found


def b5a_failures(seen, expected=None):
    """Why B5a's verdict fails, empty when it passes. `seen` is an anim_muscle_confirm.Observation whose rows are
    (angle, bound) and refused rows (angle, bound, reason). In each Editor: rows exactly the expected ones, nothing
    twist-active among them, every row within its bound; refusal rows exactly the 25 cases' and each the case's
    pinned reason."""
    from anim_muscle_confirm import EDITORS

    rows, refused = expected or b5a_expected()
    pinned = pinned_refusals()
    found = [f"no B5a runs of Editor {editor}" for editor in EDITORS if editor not in seen.runs]
    found += seen.errors
    free = twist_free()
    active = sorted({key[1] for key in seen.rows.keys() | seen.refused.keys()
                     if key[1] not in free and key[1] not in pinned})
    if active:
        found.append(f"twist-active case(s) entered B5a: {active[:5]}")
    for title, keys in (("lack", rows - seen.rows.keys()), ("hold beyond the expected", seen.rows.keys() - rows),
                        ("lack refusal", refused - seen.refused.keys()),
                        ("hold refusal rows beyond the expected", seen.refused.keys() - refused)):
        if keys:
            found.append(f"the runs {title} {len(keys)} row(s): {sorted(keys, key=str)[:3]}")
    wrong = sorted({(key[1], reason) for key, (*_, reason) in seen.refused.items()
                    if key[1] in pinned and reason != pinned[key[1]]})
    found += [f"{case} refuses as {reason}, not {pinned[case]}" for case, reason in wrong]
    outside = [key for key, (turn, bound) in seen.rows.items() if not turn <= bound]
    if outside:
        found.append(f"{len(outside)} B5a row(s) outside their bound: {sorted(outside, key=str)[:3]}")
    return found
