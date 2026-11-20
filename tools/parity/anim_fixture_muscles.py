"""B5a's muscle sweeps (UnityrsBodyMuscles.cs): known vectors for the muscles the body probes leave out.

The Rust muscle pose (docs/plans/humanoid-body.md, B5a) must be checked on
every muscle a clip animates. The body probes cover 45 of HumanTrait's 95;
these cover the rest: the ten limb twist muscles on the compact and tall
rigs, and the forty finger muscles on the compact rig with all thirty
finger bones mapped ("fingers"). Each muscle is probed alone at every
MUSCLE_VALUES value, both signs out to its limits (a twist sweep probes
only its twist muscles alone, the body probes having the others), then
each group (a twist muscle with its bone's probe muscles, a finger's four
muscles) is set jointly at MUSCLE_COMBINED's two vectors. Each sweep is a
bundle of its own, and B2 never reads them.
"""

MUSCLE_VALUES = (1, -1, .75, -.75, .5, -.5, .25, -.25, .125, -.125)
MUSCLE_COMBINED = ((.6, -.5, .4, -.3), (-.6, .5, -.4, .3))
# Each limb twist muscle and the bone whose probe muscles it is combined with.
LIMB_TWISTS = (("Left Upper Leg Twist In-Out", "LeftUpperLeg"), ("Left Lower Leg Twist In-Out", "LeftLowerLeg"),
               ("Left Foot Twist In-Out", "LeftFoot"), ("Right Upper Leg Twist In-Out", "RightUpperLeg"),
               ("Right Lower Leg Twist In-Out", "RightLowerLeg"), ("Right Foot Twist In-Out", "RightFoot"),
               ("Left Arm Twist In-Out", "LeftUpperArm"), ("Left Forearm Twist In-Out", "LeftLowerArm"),
               ("Right Arm Twist In-Out", "RightUpperArm"), ("Right Forearm Twist In-Out", "RightLowerArm"))
SIDES = ("Left", "Right")
FINGERS = ("Thumb", "Index", "Middle", "Ring", "Little")
PHALANGES = ("Proximal", "Intermediate", "Distal")
# Each finger's phalanx offsets on the left hand (x mirrored on the right).
FINGER_LAYOUT = (((.02, -.01, .025), (.025, 0, .015), (.02, 0, .01)),
                 ((.075, 0, .022), (.03, 0, 0), (.022, 0, 0)),
                 ((.078, 0, .007), (.033, 0, 0), (.024, 0, 0)),
                 ((.074, 0, -.008), (.03, 0, 0), (.022, 0, 0)),
                 ((.066, 0, -.022), (.024, 0, 0), (.018, 0, 0)))
FINGER_BONES = tuple(f"{side} {finger} {phalanx}" for side in SIDES for finger in FINGERS for phalanx in PHALANGES)
# Each finger's muscles, in HumanTrait order.
FINGER_GROUPS = tuple((f"{side} {finger} 1 Stretched", f"{side} {finger} Spread", f"{side} {finger} 2 Stretched",
                       f"{side} {finger} 3 Stretched") for side in SIDES for finger in FINGERS)
FINGERS_RIG = "fingers"
# name: rig
MUSCLE_SWEEPS = {"body-probes-limb-twist-compact": "compact", "body-probes-limb-twist-tall": "tall",
                 "body-probes-fingers": FINGERS_RIG}


def finger_rows():
    """The fingers' (name, parent, offset) rows below each hand, as the builder's AddFingers lays them."""
    rows = []
    for side, sign in zip(SIDES, (-1, 1)):
        for finger, offsets in zip(FINGERS, FINGER_LAYOUT):
            parent = side + "Hand"
            for phalanx, (x, y, z) in zip(PHALANGES, offsets):
                name = f"{side} {finger} {phalanx}"
                rows.append((name, parent, (sign * x, y, z)))
                parent = name
    return rows


def groups(name, probe_groups):
    """A sweep's muscle groups: each finger's, or each twist muscle with its bone's probe muscles."""
    if name == "body-probes-fingers":
        return FINGER_GROUPS
    table = dict(probe_groups)
    return tuple((muscle, *table[bone]) for muscle, bone in LIMB_TWISTS)


def muscle_vectors(name, index, probe_groups):
    """The sweep's known vectors, as {muscle index: value}, in the builder's order."""
    rows = groups(name, probe_groups)
    singles = [m for group in rows for m in group] if name == "body-probes-fingers" else [group[0] for group in rows]
    vectors = [{index[muscle]: value} for muscle in singles for value in MUSCLE_VALUES]
    vectors += [{index[muscle]: values[k % len(values)] for k, muscle in enumerate(group)}
                for group in rows for values in MUSCLE_COMBINED]
    return vectors
