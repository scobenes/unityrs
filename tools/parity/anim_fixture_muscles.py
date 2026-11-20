"""B5a's muscle sweeps (UnityrsBodyMuscles.cs): known vectors for every one of HumanTrait's 95 muscles.

The Rust muscle pose (docs/plans/humanoid-body.md, B5a) must be checked on
every muscle a clip animates, at both signs out to its limits. Each sweep
gives every muscle it covers the same schedule: the muscle alone at every
MUSCLE_VALUES value (both endpoints and four interior values per sign),
then each bone's muscles jointly at MUSCLE_COMBINED's two opposite
vectors. The sweeps:

- `body-probes-muscles-compact` and `-tall`: every body muscle the rig
  maps, the face aside (the probe groups' muscles with each limb's twist
  muscle in its bone's group): 49 and 46 muscles;
- `body-probes-fingers`: the forty finger muscles, on the compact rig with
  all thirty finger bones mapped ("fingers");
- `body-probes-face`: the Eyes' and the Jaw's six muscles, on the eyes-jaw
  rig.

Each sweep is a bundle of its own, and B2 never reads them.
"""

MUSCLE_VALUES = (1, -1, .75, -.75, .5, -.5, .25, -.25, .125, -.125)
MUSCLE_COMBINED = ((.6, -.5, .4, -.3), (-.6, .5, -.4, .3))
# Each limb twist muscle and the bone whose group it joins.
LIMB_TWISTS = (("Left Upper Leg Twist In-Out", "LeftUpperLeg"), ("Left Lower Leg Twist In-Out", "LeftLowerLeg"),
               ("Left Foot Twist In-Out", "LeftFoot"), ("Right Upper Leg Twist In-Out", "RightUpperLeg"),
               ("Right Lower Leg Twist In-Out", "RightLowerLeg"), ("Right Foot Twist In-Out", "RightFoot"),
               ("Left Arm Twist In-Out", "LeftUpperArm"), ("Left Forearm Twist In-Out", "LeftLowerArm"),
               ("Right Arm Twist In-Out", "RightUpperArm"), ("Right Forearm Twist In-Out", "RightLowerArm"))
FACE = ("LeftEye", "RightEye", "Jaw")
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
# Each finger's muscles, in HumanTrait order, and the phalanx each turns.
FINGER_MUSCLES = (("1 Stretched", "Proximal"), ("Spread", "Proximal"), ("2 Stretched", "Intermediate"),
                  ("3 Stretched", "Distal"))
FINGER_GROUPS = tuple(tuple(f"{side} {finger} {muscle}" for muscle, _ in FINGER_MUSCLES)
                      for side in SIDES for finger in FINGERS)
FINGERS_RIG = "fingers"
# name: rig
MUSCLE_SWEEPS = {"body-probes-muscles-compact": "compact", "body-probes-muscles-tall": "tall",
                 "body-probes-fingers": FINGERS_RIG, "body-probes-face": "eyes-jaw"}


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


def groups(name, probe_groups, mapped):
    """A sweep's (bone, muscles) groups, in the builder's order: each finger's on its proximal phalanx, the face
    bones', or each mapped body bone's probe muscles with its limb twist muscle."""
    if name == "body-probes-fingers":
        return tuple((group[0].rsplit(" ", 2)[0] + " Proximal", group) for group in FINGER_GROUPS)
    twists = {bone: muscle for muscle, bone in LIMB_TWISTS}
    face = name == "body-probes-face"
    return tuple((bone, (*muscles, *([twists[bone]] if bone in twists else [])))
                 for bone, muscles in probe_groups if bone in mapped and (bone in FACE) == face)


def muscle_bone(muscle, probe_groups):
    """The human bone a muscle turns, by name."""
    for side in SIDES:
        for finger in FINGERS:
            for suffix, phalanx in FINGER_MUSCLES:
                if muscle == f"{side} {finger} {suffix}":
                    return f"{side} {finger} {phalanx}"
    found = [bone for bone, muscles in probe_groups if muscle in muscles] + [b for m, b in LIMB_TWISTS if m == muscle]
    if len(found) != 1:
        raise ValueError(f"no bone for muscle {muscle}")
    return found[0]


def muscle_vectors(name, index, probe_groups, mapped):
    """The sweep's known vectors, as {muscle index: value}, in the builder's order: each muscle alone at every
    value, then each group jointly."""
    rows = [muscles for _, muscles in groups(name, probe_groups, mapped)]
    vectors = [{index[muscle]: value} for group in rows for muscle in group for value in MUSCLE_VALUES]
    vectors += [{index[muscle]: values[k % len(values)] for k, muscle in enumerate(group)}
                for group in rows for values in MUSCLE_COMBINED]
    return vectors
