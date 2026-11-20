"""Round 11's full-body clips (UnityrsBodyClips.cs FullRigs and ScopedRigs): E5 clips with every muscle on a curve.

E5's other clips animate the Spine alone (with the body curves), so a collector that dropped every other bone's
channels could still pass them. These clips put a curve on every muscle and play on rigs that map every bone a
muscle turns: a body topology with the face (LeftEye, RightEye, Jaw) and all thirty fingers added.

Each clip declares its scope against freeze v8's (anim_fixture_scope):

- **Refusal evidence** (FULL_RIGS): the compact rig's topology at three sizes and the large armature. Each
  muscle but the Spine's three, which keep the curves every humanoid clip has (`AuthorHumanoidClip`, Spine
  Left-Right .375 at the end, over the frozen lateral limit), has a curve of amplitude 1/2. The plain rigs play
  FULL_VARIANTS at both placements; the armature plays the chain variant at the identity.
- **In scope** (SCOPED_RIGS), B5b's passing evidence over B4's whole admitted matrix: every muscle on a curve,
  the Spine's three too, with the frozen-limited ones held inside the limits by amplitude (SCOPED_AMPLITUDES):
  the spine Left-Right muscles within 3/16 of the lateral limit 0.193359375 at every key (polylines, so between
  keys too), and the spine twist and shoulder Front-Back muscles, which turn the shoulders about the body's up,
  small. Completion checks both on what Unity built and recorded (anim_fixture_scope): the compiled lateral
  values at every sample time, and the shoulder-hip twist of every recorded pose against 4.25 degrees. The
  plain chain plays every variant B4 admits at both placements on the compact, large, small and tall rigs and
  on every other body topology B3 admits; each F5 feature plays the chain variant at the identity at every
  scale a chain rig plays it at, but the armature at 1.25 and 1.8: at F5's own scale its human scale is B4's
  range's lower end, which a full-body Avatar must not cross.

A rig maps every bone a muscle turns but those its topology omits (no shoulders, no UpperChest, no toes, the
tall rig's UpperChest): those bones' muscles are **expected absent** (`expected_absent`), never curved, since
no bone could show them, and completion requires that they are not.

Each muscle `m` has its own polyline through BODY_TIMES, `amplitude(m) * unit(m, key)`, where `unit` takes the
muscle index modulo 17, 13 and 11 at different keys: no two of the 95 curves are equal, so a collector that
swapped two channels would change the pose. Every value is exact in f32.

Unity binds a muscle curve by its Animator property, HumanTrait's name for a body muscle and
`<Side>Hand.<Finger>.<rest>` for a finger muscle (`muscle_property`). Completion checks each clip's authored
curves under those properties and its built clip's slots (muscle `m` at Animator attribute MUSCLE_ATTRIBUTE + m)
at every key, as it checks the variants' own curves (anim_fixture_body.check_clip).
"""

from anim_fixture_body import BODY_TIMES, CHAIN_VARIANT, CLIP_VARIANTS, MUSCLE_ATTRIBUTE
from anim_fixture_muscles import FACE, FINGER_BONES, FINGERS, SIDES

# rig: (F5 feature, size); each maps the face and every finger too.
FULL_RIGS = {"compact-full": ("", 1), "large-full": ("", 1.5), "small-full": ("", .82),
             "armature-large-full": ("armature", 1.8)}
FULL_EXTRAS = (*FACE, *FINGER_BONES)
FULL_VARIANTS = ("keep-o0-y0-xz0", "bake-o1-y1-xz1", "level", "keep-o1-y1-xz1-motion")
SPINE_MUSCLES = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")
# The variants B4 admits (anim_fixture_round11.E5_ADMITTED), without their start twins.
SCOPED_ADMITTED = ("bake-o0-y0-xz1", "bake-o0-y1-xz0", "bake-o0-y1-xz1", "bake-o1-y0-xz0", "bake-o1-y0-xz1",
                   "bake-o1-y1-xz0", "bake-o1-y1-xz1", "keep-o0-y0-xz0", "keep-o1-y1-xz1-motion", "level",
                   "orientation-offset")
SCOPED_TOPOLOGIES = ("no-upper-chest", "no-shoulders", "no-left-toes", "no-right-toes", "no-toes",
                     "no-upper-chest-toes", "no-upper-chest-left-toes", "no-upper-chest-right-toes", "noncoplanar")
# rig: (topology, F5 feature, size, variants). Each F5 feature at its chain rigs' scales
# (anim_fixture_round11.CHAIN_FEATURES), the armature at 1.25 for F5's own 1.
SCOPED_RIGS = {"compact-scoped": ("compact", "", 1, SCOPED_ADMITTED),
               "large-scoped": ("compact", "", 1.5, SCOPED_ADMITTED),
               "small-scoped": ("compact", "", .82, SCOPED_ADMITTED),
               "tall-scoped": ("tall", "", 1, SCOPED_ADMITTED),
               "armature-scoped": ("compact", "armature", 1.25, (CHAIN_VARIANT,)),
               "armature-large-scoped": ("compact", "armature", 1.8, (CHAIN_VARIANT,)),
               "scaled-animator-scoped": ("compact", "scaled-animator", 1, (CHAIN_VARIANT,)),
               "scaled-animator-large-scoped": ("compact", "scaled-animator", 1.5, (CHAIN_VARIANT,)),
               "scaled-animator-small-scoped": ("compact", "scaled-animator", .82, (CHAIN_VARIANT,)),
               "hips-scale-scoped": ("compact", "hips-scale", 1, (CHAIN_VARIANT,)),
               "hips-scale-large-scoped": ("compact", "hips-scale", 1.2, (CHAIN_VARIANT,)),
               "hips-scale-small-scoped": ("compact", "hips-scale", .7, (CHAIN_VARIANT,))} | {
    topology + "-scoped": (topology, "", 1, SCOPED_ADMITTED) for topology in SCOPED_TOPOLOGIES}
# An in-scope clip's amplitudes: (muscle names, amplitude); every other muscle's is 1/2.
SCOPED_AMPLITUDES = ((("Spine Left-Right", "Chest Left-Right", "UpperChest Left-Right"), 3 / 16),
                     (("Spine Twist Left-Right", "Chest Twist Left-Right", "UpperChest Twist Left-Right"), 1 / 32),
                     (("Left Shoulder Front-Back", "Right Shoulder Front-Back"), 1 / 8))
MODULI = (17, 13, 11, 13, 17)


def topology(rig):
    """A full-body clip rig's body topology."""
    return SCOPED_RIGS[rig][0] if rig in SCOPED_RIGS else "compact"


def feature(rig):
    return SCOPED_RIGS[rig][1] if rig in SCOPED_RIGS else FULL_RIGS[rig][0]


def size(rig):
    return SCOPED_RIGS[rig][2] if rig in SCOPED_RIGS else FULL_RIGS[rig][1]


def variants(rig):
    """A full rig's variants in the builder's (CLIP_VARIANTS) order: an in-scope rig's own; FULL_VARIANTS on a
    plain chain; the chain variant on an F5 feature's."""
    chosen = SCOPED_RIGS[rig][3] if rig in SCOPED_RIGS else FULL_VARIANTS if feature(rig) == "" else (CHAIN_VARIANT,)
    return tuple(name for name in CLIP_VARIANTS if name in chosen)


def placements(rig):
    """Both placements on a plain chain; the identity alone on an F5 feature's, as B4 admits it."""
    return (("identity", ""), ("offset", "-offset")) if feature(rig) == "" else (("identity", ""),)


def unit(muscle, key):
    """A muscle's curve shape at a key: eighths in [-1, 1], distinct over the 95 muscles."""
    modulus = MODULI[key]
    return ((muscle * 7 + 3 + key * 5) % modulus - (modulus - 1) // 2) / 8


def amplitude(name, scoped):
    return next((value for names, value in SCOPED_AMPLITUDES if name in names), .5) if scoped else .5


def full_values(muscle, name, scoped):
    return tuple(amplitude(name, scoped) * unit(muscle, key) for key in range(len(BODY_TIMES)))


def muscle_property(name):
    """The Animator curve property Unity binds for a HumanTrait muscle name: `LeftHand.Thumb.1 Stretched` for
    `Left Thumb 1 Stretched`, a body muscle's own name."""
    side, _, rest = name.partition(" ")
    finger, _, part = rest.partition(" ")
    return f"{side}Hand.{finger}.{part}" if side in SIDES and finger in FINGERS and part else name


def mapped_bones(rig):
    """The human bones a full-body clip rig maps: its topology's, the face and every finger."""
    from anim_fixture_body import rig_bones

    return rig_bones(topology(rig)) | set(FULL_EXTRAS)


def expected_absent(rig, names):
    """The muscles a full-body clip rig leaves without a curve because its topology omits their bone."""
    from anim_fixture_body import PROBE_GROUPS
    from anim_fixture_muscles import muscle_bone

    bones = mapped_bones(rig)
    return frozenset(name for name in names if muscle_bone(name, PROBE_GROUPS) not in bones)


def full_groups(names, rig):
    """A full-body clip rig's curves as anim_fixture_body.distinguishing_curves' groups: (property, built
    attribute, key times, values), one per curved muscle: every muscle the rig's bones show (`expected_absent`
    aside), and on a refusal rig all but the Spine's three."""
    scoped, absent = rig in SCOPED_RIGS, expected_absent(rig, names)
    return [[(muscle_property(name), MUSCLE_ATTRIBUTE + i, BODY_TIMES, full_values(i, name, scoped))]
            for i, name in enumerate(names) if name not in absent and (scoped or name not in SPINE_MUSCLES)]
