"""Round 11's full-body clips (UnityrsBodyClips.cs FullRigs): E5 clips with every muscle on a curve.

E5's other clips animate the Spine alone (with the body curves), so a collector that dropped every other bone's
channels could still pass them. These clips put a curve on every one of the 95 muscles and play on rigs that map
every bone a muscle turns: the compact rig's topology with the face (LeftEye, RightEye, Jaw) and all thirty
fingers added, at three sizes, and the large armature. The plain rigs play FULL_VARIANTS, four variants B4
admits, at both placements; the armature plays the chain variant at the identity, the one pairing B4 admits for
an F5 feature. The Spine's three muscles keep the curves every humanoid clip has (`AuthorHumanoidClip`); each
other muscle `m` has a polyline through BODY_TIMES with `full_values(m)`: eighths within +-.5, three distinct
values per muscle, exact in f32.

Completion checks each such clip's authored curves and its built clip's slots at every key, as it checks the
variants' own curves (anim_fixture_body.check_clip), so a dropped or moved channel fails the case.
"""

from anim_fixture_body import BODY_TIMES, CHAIN_VARIANT, MUSCLE_ATTRIBUTE
from anim_fixture_muscles import FACE, FINGER_BONES

# rig: (F5 feature, size); each maps the face and every finger too.
FULL_RIGS = {"compact-full": ("", 1), "large-full": ("", 1.5), "small-full": ("", .82),
             "armature-large-full": ("armature", 1.8)}
FULL_EXTRAS = (*FACE, *FINGER_BONES)
FULL_VARIANTS = ("keep-o0-y0-xz0", "bake-o1-y1-xz1", "level", "keep-o1-y1-xz1-motion")
SPINE_MUSCLES = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")


def variants(rig):
    """A full rig's variants: FULL_VARIANTS on a plain chain, the chain variant on an F5 feature's."""
    return FULL_VARIANTS if FULL_RIGS[rig][0] == "" else (CHAIN_VARIANT,)


def placements(rig):
    """Both placements on a plain chain; the identity alone on an F5 feature's, as B4 admits it."""
    return (("identity", ""), ("offset", "-offset")) if FULL_RIGS[rig][0] == "" else (("identity", ""),)


def full_values(muscle):
    return tuple(((muscle * 7 + key * 3) % 9 - 4) / 8 for key in range(len(BODY_TIMES)))


def full_groups(names):
    """The full curves as anim_fixture_body.distinguishing_curves' groups: (property, built attribute, key
    times, values), one group per muscle the Spine's three aside."""
    return [[(name, MUSCLE_ATTRIBUTE + i, BODY_TIMES, full_values(i))]
            for i, name in enumerate(names) if name not in SPINE_MUSCLES]
