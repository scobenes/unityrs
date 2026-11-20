"""The extra probe cases added with the second hold-out rig (docs/plans/humanoid-body.md, E3b on freeze v7).

E3b on freeze v7 failed on section 7's criterion at one hold-out sample
whose u_s was driven by the coefficients of the shoulder links, which the
design held weakly. Each case here pairs a core of two bones that load a
weak direction of the design (both shoulders up together, which doubles the
leverage of their sum; a shoulder with its upper arm; both lower legs; the
spine with an arm) with each of a few other bones in turn, every bone taking
one of its rig's own probe vectors (anim_fixture_body.compose), and repeats
the list once. They were chosen, from the training design and the planned
hold-out alone, to raise the worst predicted margin over the training and
hold-out samples (anim_body_holdout.py); no hold-out recording was read.
"""

# The other bones each core is paired with, one at a time.
OTHERS = (("Neck", 1), ("Head", 1), ("Neck", -1), ("Head", -2), ("LeftHand", 1), ("RightHand", 1))
# name: (rig, core)
CORES = {
    "body-probes-compact-2": ("compact", (("LeftShoulder", 1), ("RightShoulder", 1))),
    "body-probes-compact-3": ("compact", (("LeftShoulder", 1), ("LeftUpperArm", 1))),
    "body-probes-compact-4": ("compact", (("LeftLowerLeg", 1), ("RightLowerLeg", 1))),
    "body-probes-compact-5": ("compact", (("Spine", 1), ("RightUpperArm", 1))),
    "body-probes-tall-3": ("tall", (("LeftLowerLeg", 1), ("RightLowerLeg", 1))),
}


def plan(core):
    """A case's (bone, source) vectors: the core with each other bone in turn, twice over."""
    bones = {bone for bone, _ in core}
    return tuple((*core, other) for other in OTHERS if other[0] not in bones) * 2


ADDED_PROBES = {name: plan(core) for name, (_, core) in CORES.items()}
ADDED_RIGS = {name: rig for name, (rig, _) in CORES.items()}
