"""The body rigs' geometry and the hold-out's pose draws (UnityrsBodyFixtures.cs BodyLayout, RigNodes and
HoldoutPlans).

`rig_layout` is the builder's T-pose of a rig, so a planned case's
geometry is known before any run (anim_body_design predicts a hold-out
rig's adequacy from it). The hold-out rig "holdout-2" records its own pose
draws (`holdout_plan`): each is an extra-probe-style vector
(anim_fixture_body.compose) whose bones each take one of the rig's own
probe vectors, so every bone's local rotation is one a training rig of the
same topology records, and its geometry is predicted exactly. The draws
are points of the unscrambled Sobol sequence past HOLDOUT_SEED, in six
dimensions: three (bone, source) picks per vector, a repeated bone dropped.
By design no draw sets a muscle that bends the spine sideways (2.2's
lateral scope) or one 2.2's frame scope refuses (a spine twist, a
shoulder's Front-Back): of a spine bone only its Front-Back probe, of a
shoulder only its Down-Up probe, of any other bone any single probe or
combined vector.
"""

from anim_fixture_body import PROBE_GROUPS, PROBE_RIGS, RIG_SIZES, rig_bones, sobol

# (arms, legs) length factors of the builder's BodyRig, 1 elsewhere.
RIG_PROPORTIONS = {"holdout": (1.3, .8), "holdout-2": (.8, 1.2)}
# Rigs whose hip and shoulder children leave the plane. A rig's size (RIG_SIZES) scales every offset.
NONCOPLANAR = ("noncoplanar", "no-chest")


def rig_layout(rig):
    """The builder's T-pose (UnityrsBodyFixtures.cs BodyLayout and RigNodes) as (name, parent, offset), in
    creation order: an omitted bone's children keep their place from its parent."""
    base, omit, extras = PROBE_RIGS.get(rig, ("compact", (), ()))
    tall = base == "tall"
    size = RIG_SIZES.get(rig, 1)
    arms, legs = RIG_PROPORTIONS.get(rig, (1, 1))
    hip_z, shoulder_z = (.03, -.025) if rig in NONCOPLANAR else (0, 0)
    rows = [("Hips", "", (0, (1.18 if tall else .88) - (1 - legs) * (1.06 if tall else .73), 0)),
            ("Spine", "Hips", (0, .21 if tall else .16, 0)), ("Chest", "Spine", (0, .27 if tall else .13, 0))]
    torso = "Chest" if tall else "UpperChest"
    if not tall:
        rows.append(("UpperChest", "Chest", (0, .08, 0)))
    rows += [("Neck", torso, (0, .14, 0)), ("Head", "Neck", (0, .12, 0))]
    rows += [row for row in (("LeftEye", "Head", (-.03, .06, .08)), ("RightEye", "Head", (.03, .06, .08)),
                             ("Jaw", "Head", (0, -.03, .05))) if row[0] in extras]
    for side, sign in (("Left", -1), ("Right", 1)):
        rows += [(side + "UpperLeg", "Hips", (sign * (.10 if tall else .13), -.05, hip_z)),
                 (side + "LowerLeg", side + "UpperLeg", (0, (-.58 if tall else -.36) * legs, .04)),
                 (side + "Foot", side + "LowerLeg", (0, (-.48 if tall else -.37) * legs, -.04)),
                 (side + "Toes", side + "Foot", (0, -.04, .14)),
                 (side + "Shoulder", torso, (sign * .08, .08, shoulder_z)),
                 (side + "UpperArm", side + "Shoulder", (sign * (.10 if tall else .14), 0, 0)),
                 (side + "LowerArm", side + "UpperArm", (sign * (.34 if tall else .26) * arms, 0, 0)),
                 (side + "Hand", side + "LowerArm", (sign * (.26 if tall else .23) * arms, 0, 0))]
    offsets = {name: (parent, tuple(size * v for v in offset)) for name, parent, offset in rows}
    result = []
    for name, (parent, offset) in offsets.items():
        if name in omit:
            continue
        while parent in omit:
            offset = tuple(a + b for a, b in zip(offset, offsets[parent][1]))
            parent = offsets[parent][0]
        result.append((name, parent, offset))
    return result


# The hold-out draws: Sobol points past this many (a seed no training design shares), and how many vectors.
HOLDOUT_SEED = 1 << 12
HOLDOUT_DRAWS = 45
SPINE_BONES = ("Spine", "Chest", "UpperChest")
SHOULDERS = ("LeftShoulder", "RightShoulder")


def sources_of(bone, size):
    """The probe sources a hold-out draw may give `bone` (single probe k from 1, combined vector c as -1 - c)."""
    if bone in SPINE_BONES or bone in SHOULDERS:
        return (1,)
    return (*range(1, size + 1), -1, -2, -3)


def holdout_plan(rig, seed=HOLDOUT_SEED, count=HOLDOUT_DRAWS):
    """The hold-out rig's vectors as (bone, source) picks (module docstring)."""
    mapped = rig_bones(rig)
    bones = [(bone, len(group)) for bone, group in PROBE_GROUPS if bone in mapped]
    plan = []
    for point in sobol(seed + count, 6)[seed:]:
        picks = []
        for u, v in zip(point[0::2], point[1::2]):
            bone, size = bones[int(u * len(bones))]
            options = sources_of(bone, size)
            if bone not in (b for b, _ in picks):
                picks.append((bone, options[int(v * len(options))]))
        plan.append(tuple(picks))
    return tuple(plan)
