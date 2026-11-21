"""Round 12's twist probe tables (round 12 design v3 section 2, v4 section 3): the setting tuples, the vector
sets, the cases and their pre-registered partitions. The builder's C# mirror (stage b) must produce these exactly.

Vectors are {muscle ordinal: value}, every other muscle 0, each once (a receiver's swing is the next pair's
source swing, so those controls appear once). Each pair's bindings are HumanTrait's names
(`NAMES`, checked against a capture's own muscle list by anim_twist_witness):

- **Core set** (113): each pair and side, its twist alone at +-1 and +-.5; a swing-only control on the source's
  first swing and on the receiver's first swing at +.5; the twist at +.5 with each of those; a divergence pose,
  the receiver's first swing at +1, the source's at +.5 and the twist at +-.5. Then per side the chains at .5
  (arm and forearm, upper and lower leg, lower leg and foot, each ++ and +-; upper, lower leg and foot ++ + and
  +-+), the ten limb twists together (all +.5, then signs alternating) and the zero vector.
- **Full set** (211): the core set, then the twist alone at +-.25, every swing of the pair at -.5 and the twist at
  -.5 with every swing at +.5 (+.5 where the core lacks it), and the divergence pose with the receiver at -1.
- **Toe-absent set** (29): per side the Foot Twist alone at +-1, +-.5, +-.25, the Foot Up-Down at +-.5, the Foot
  Twist at +.5 with the Foot Up-Down at +-.5, lower leg and foot ++ and +-, the Lower Leg Twist at +-.5; zero.
"""

import hashlib
import json

from anim_twist_model import LIMB_TWISTS

SIDE_OFFSETS = {"Left": (0, 0), "Right": (8, 9)}
# The left side's ordinals; the right's legs are 8 on, its arms 9 on (HumanTrait order).
LEFT = {"Upper Leg Front-Back": 21, "Upper Leg In-Out": 22, "Upper Leg Twist In-Out": 23, "Lower Leg Stretch": 24,
        "Lower Leg Twist In-Out": 25, "Foot Up-Down": 26, "Foot Twist In-Out": 27, "Toes Up-Down": 28,
        "Arm Down-Up": 39, "Arm Front-Back": 40, "Arm Twist In-Out": 41, "Forearm Stretch": 42,
        "Forearm Twist In-Out": 43, "Hand Down-Up": 44, "Hand In-Out": 45}
NAMES = {f"{side} {name}": ordinal + SIDE_OFFSETS[side][ordinal >= 37] for side in ("Left", "Right")
         for name, ordinal in LEFT.items()}
# Each pair: (twist, source swings, receiver swings).
PAIRS = {"arm": ("Arm Twist In-Out", ("Arm Down-Up", "Arm Front-Back"), ("Forearm Stretch",)),
         "forearm": ("Forearm Twist In-Out", ("Forearm Stretch",), ("Hand Down-Up", "Hand In-Out")),
         "upper leg": ("Upper Leg Twist In-Out", ("Upper Leg Front-Back", "Upper Leg In-Out"), ("Lower Leg Stretch",)),
         "lower leg": ("Lower Leg Twist In-Out", ("Lower Leg Stretch",), ("Foot Up-Down",)),
         "foot": ("Foot Twist In-Out", ("Foot Up-Down",), ("Toes Up-Down",))}
CHAINS = ((("Arm Twist In-Out", "Forearm Twist In-Out"), ((1, 1), (1, -1))),
          (("Upper Leg Twist In-Out", "Lower Leg Twist In-Out"), ((1, 1), (1, -1))),
          (("Lower Leg Twist In-Out", "Foot Twist In-Out"), ((1, 1), (1, -1))),
          (("Upper Leg Twist In-Out", "Lower Leg Twist In-Out", "Foot Twist In-Out"), ((1, 1, 1), (1, -1, 1))))
SIDES = ("Left", "Right")
HALF = .5


def _v(side, **values):
    return {NAMES[f"{side} {name}"]: value for name, value in values.items()}


def _named(side, pairs):
    return {NAMES[f"{side} {name}"]: value for name, value in pairs}


def core():
    vectors = []
    for side in SIDES:
        for twist, sources, receivers in PAIRS.values():
            s1, r1 = sources[0], receivers[0]
            vectors += [_named(side, [(twist, v)]) for v in (1., -1., .5, -.5)]
            vectors += [_named(side, [(s1, HALF)]), _named(side, [(r1, HALF)])]
            vectors += [_named(side, [(twist, HALF), (s1, HALF)]), _named(side, [(twist, HALF), (r1, HALF)])]
            vectors += [_named(side, [(r1, 1.), (s1, HALF), (twist, v)]) for v in (HALF, -HALF)]
        for names, signs in CHAINS:
            vectors += [_named(side, [(name, sign * HALF) for name, sign in zip(names, chosen)]) for chosen in signs]
    vectors.append({m: HALF for m in LIMB_TWISTS})
    vectors.append({m: HALF * (-1) ** k for k, m in enumerate(LIMB_TWISTS)})
    vectors.append({})
    return _unique(vectors)


def _unique(vectors):
    """Each vector once, at its first place: a receiver's swing is the next pair's source swing (Forearm Stretch,
    Lower Leg Stretch, Foot Up-Down), so their controls coincide."""
    seen, kept = set(), []
    for vector in vectors:
        if _key(vector) not in seen:
            seen.add(_key(vector))
            kept.append(vector)
    return kept


def full():
    vectors = core()
    for side in SIDES:
        for twist, sources, receivers in PAIRS.values():
            extra = [_named(side, [(twist, v)]) for v in (.25, -.25)]
            for swing in sources + receivers:
                extra += [_named(side, [(swing, v)]) for v in (HALF, -HALF)]
                extra += [_named(side, [(twist, v), (swing, HALF)]) for v in (HALF, -HALF)]
            extra += [_named(side, [(receivers[0], -1.), (sources[0], HALF), (twist, v)]) for v in (HALF, -HALF)]
            vectors += extra
    return _unique(vectors)


def toe_absent():
    vectors = []
    for side in SIDES:
        vectors += [_v(side, **{"Foot Twist In-Out": v}) for v in (1., -1., .5, -.5, .25, -.25)]
        vectors += [_v(side, **{"Foot Up-Down": v}) for v in (HALF, -HALF)]
        vectors += [_v(side, **{"Foot Twist In-Out": HALF, "Foot Up-Down": v}) for v in (HALF, -HALF)]
        vectors += [_v(side, **{"Lower Leg Twist In-Out": HALF, "Foot Twist In-Out": v}) for v in (HALF, -HALF)]
        vectors += [_v(side, **{"Lower Leg Twist In-Out": v}) for v in (HALF, -HALF)]
    vectors.append({})
    return vectors


def _key(vector):
    return tuple(sorted(vector.items()))


# (arm, forearm, upper leg, leg) per tuple.
TUPLES = {"t0": (0., 0., 0., 0.), "t25": (.25,) * 4, "t5": (.5,) * 4, "t75": (.75,) * 4, "t1": (1.,) * 4,
          "arm25": (.25, .5, .5, .5), "forearm25": (.5, .25, .5, .5), "upperleg25": (.5, .5, .25, .5),
          "leg25": (.5, .5, .5, .25), "m1": (.25, .75, .75, .25), "m2": (.75, .25, .25, .75)}
ALL_EQUAL = ("t0", "t25", "t5", "t75", "t1")
SHORT = ("arm25", "forearm25", "upperleg25", "leg25", "m1", "m2")


def cases():
    """{case: (rig, tuple, vector set name, partition)}: the SetHumanPose twist probes, pre-registered."""
    found = {}
    for rig in ("compact", "tall"):
        for name in ALL_EQUAL + SHORT:
            kind = "full" if name in ALL_EQUAL else "core"
            if rig == "compact" and name in ("t0", "t5", "t1", "arm25", "forearm25", "upperleg25", "leg25"):
                part = "exploration"
            elif rig == "tall" and name in ("t0", "t5", "t1"):
                part = "selection"
            else:
                part = "confirmation"
            found[f"body-twist-{rig}-{name}"] = (rig, name, kind, part)
    found["body-twist-no-toes-t0"] = ("no-toes", "t0", "toe-absent", "exploration")
    found["body-twist-no-toes-t5"] = ("no-toes", "t5", "toe-absent", "confirmation")
    return found


SETS = {"full": full, "core": core, "toe-absent": toe_absent}


def sample_index(k, count):
    """Vector k's capture sample index among `count`, as the builder's Scheduled (UnityrsBodyFixtures.cs) lays a
    probe case out: an anchor at 0, the first ceil(count/2) vectors, an anchor at .5, the rest, an anchor at 1."""
    if not 0 <= k < count:
        raise ValueError(f"vector {k} outside a set of {count}")
    return 1 + k + (k >= (count + 1) // 2)


def digest():
    """The sha256 of the canonical tables: what the owner manifest and the intake digest pin."""
    tables = {"tuples": TUPLES, "cases": cases(),
              "sets": {name: [sorted(v.items()) for v in make()] for name, make in SETS.items()}}
    return hashlib.sha256(json.dumps(tables, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
