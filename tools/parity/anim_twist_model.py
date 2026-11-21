"""Round 12's limb-twist candidates (round 12 design section 1): each an executable chain, its bound alongside.

A **limb candidate** is (D, S, R, sigma, C):

- **D** the direction: the source keeps w = t (D1) or w = 1 - t (D2) of its twist angle.
- **S** the source split of theta (reference 2's signed angle on the source's x axis, its own limits and sign):
  S1 x angle w theta and phi = (1 - w) theta; S2 x tangent w tan(theta/2) and phi = 2 atan((1 - w) tan(theta/2));
  S3 the joint at the full theta, then T_x(-phi) on the left, phi = (1 - w) theta.
- **R** how the receiver takes sigma phi, on B5a's local L: R1 L T(post x), R2 T(a1) L, R3 T(a2) L, R4 T(a3) L,
  R5 L T(a2), R6 L T(a3), with a1 = n(rest_R pre_R x), a2 = n(post_B x), a3 = n(conj(local_B) rest_B pre_B x),
  local_B the source's final local (its own split and its own receipt), then one normalization.
- **sigma** the receiver's sign: +1, -1, or the product of the source's and receiver's x signs ("s").
- **C** C0 a received share stops at its receiver; C1 (with R1 only) it joins the receiver's own angle before the
  receiver's own split, so part moves on (LowerArm to Hand, LowerLeg to Foot).

A **foot candidate**: F1 Foot Twist on the Foot's own y (B5a); F2 and F3 (each signed) Foot keeps none of it and
Toes take T(n(rest_T pre_T y))(+-theta_F) on the left of their own local; without toes F2 loses it, F3 is F1.

`chains` runs one side's arm and leg, proximal to distal, through anim_twist_bound's arithmetic, so each bone's
prediction is the f64 value with its derived radius. With every limb-twist value 0 it is B5a's equation bit for bit.
`canonical` keys a candidate on a cell (the twist tuple, toes per side, the Avatar's x signs and x reaches): equal
keys are algebraically one equation there, refusing exactly where each other refuses, by the rules its docstring
proves; anything else needs a witness.
"""

import itertools
import math
import sys
from collections import namedtuple
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_twist_bound as b  # noqa: E402
from anim_muscle_pose import MUSCLE_AXES, bone_muscles  # noqa: E402

Limb = namedtuple("Limb", "d s r sigma c")
Foot = namedtuple("Foot", "kind sign")
LIMBS = tuple(Limb(d, s, r, sigma, 0) for d in (1, 2) for s in (1, 2, 3) for r in range(1, 7)
              for sigma in ("+", "-", "s")) + tuple(Limb(d, s, 1, sigma, 1) for d in (1, 2) for s in (1, 2, 3)
                                                     for sigma in ("+", "-", "s"))
FEET = (Foot(1, 0), Foot(2, 1), Foot(2, -1), Foot(3, 1), Foot(3, -1))
# The twist settings in m_Human order (anim_humanoid_qualify.TWIST).
FIELDS = ("m_ArmTwist", "m_ForeArmTwist", "m_UpperLegTwist", "m_LegTwist")
# Each side's chains: (bone, the field of the pair it sources or None, its twist muscle or None).
SIDES = {
    "Left": dict(arm=((13, 0, 41), (15, 1, 43), (17, None, None)), leg=((1, 2, 23), (3, 3, 25), (5, None, None)),
                 toes=19, foot_twist=27),
    "Right": dict(arm=((14, 0, 50), (16, 1, 52), (18, None, None)), leg=((2, 2, 31), (4, 3, 33), (6, None, None)),
                  toes=20, foot_twist=35)}
LIMB_TWISTS = (23, 25, 27, 31, 33, 35, 41, 43, 50, 52)
X, Y = (1., 0., 0.), (0., 1., 0.)
# Each pair's source bone (FIELDS order) and each receiver (LowerArm, Hand, LowerLeg, Foot), per side.
SOURCES = {"Left": (13, 15, 1, 3), "Right": (14, 16, 2, 4)}
RECEIVERS = {"Left": (15, 17, 3, 5), "Right": (16, 18, 4, 6)}
# How far below pi an x angle's reach must stay for two candidates that differ only past a tangent there to refuse
# alike: far above any chain radius (the tangent refuses at |theta| + e >= pi).
REACH_SLACK = 1e-3


def _vector(v):
    return (v, 0.)


def own_angles(bone, axes, values, errors):
    """Reference 2's three angles of a bone's own muscles, each with its radius (a later muscle on an axis replaces
    an earlier one, as `anim_muscle_pose.angles`)."""
    turns = [b.exact(0.), b.exact(0.), b.exact(0.)]
    for m in bone_muscles(bone):
        axis = MUSCLE_AXES[m]
        turns[axis] = b.limit_angle(axes, axis, values[m], errors[m])
    return turns


def _weights(limb, t):
    """(own w, receiver 1 - w) for a setting t, each subtraction Unity's f32 rounding: under D2, w = 1 - t rounds,
    then 1 - w rounds again (sequential subtraction), so the receiver's share carries both."""
    own = b.exact(t) if limb.d == 1 else b.one_minus(b.exact(t))
    return own, b.one_minus(own)


def _split(limb, theta, t):
    """The source's x tangent, the joint replacement for S3 (a function of the joint), and phi."""
    own, rest = _weights(limb, t)
    if limb.s == 1:
        return b.tan_half(b.scale(own, theta)), None, b.scale(rest, theta)
    half = b.tan_half(theta)
    if limb.s == 2:
        return b.scale(own, half), None, b.atan2_half(b.scale(rest, half))
    phi = b.scale(rest, theta)
    if phi == (0., 0.):
        return half, None, phi
    counter = b.axis_angle(_vector(X), (-phi[0], phi[1]))
    return half, lambda joint: b.unit(b.product(counter, joint)), phi


def _sign(limb, source_axes, receiver_axes):
    if limb.sigma == "+":
        return 1.
    if limb.sigma == "-":
        return -1.
    return source_axes["sign"][0] * receiver_axes["sign"][0]


def _receive(limb, local, phi, bone, source, finals, rig):
    """The receiver's local after taking phi (already signed), by R."""
    axes, rest = rig[bone]["axes"], rig[bone]["rest"]
    s_axes, s_rest = rig[source]["axes"], rig[source]["rest"]
    if limb.r == 1:
        axis = b.rotate(b.exact(tuple(axes["post"])), _vector(X))
    elif limb.r == 2:
        axis = b.rotate(b.product(b.exact(tuple(rest)), b.exact(tuple(axes["pre"]))), _vector(X))
    elif limb.r in (3, 5):
        axis = b.rotate(b.exact(tuple(s_axes["post"])), _vector(X))
    else:
        held = b.rotate(b.product(b.exact(tuple(s_rest)), b.exact(tuple(s_axes["pre"]))), _vector(X))
        axis = b.rotate(b.conjugate(finals[source]), held)
    turn = b.axis_angle(axis, phi)
    return b.unit(b.product(local, turn) if limb.r in (1, 5, 6) else b.product(turn, local))


def chains(rig, values, errors, settings, limb, foot, side):
    """{bone: (local quaternion, radius)} for one side's arm and leg (and Toes, where `rig` maps them) under a
    candidate. `rig` is {bone: {"axes", "rest"}}; `values`/`errors` the 95 muscle values and their errors;
    `settings` the four twist settings in FIELDS order. A pole or a degenerate norm raises `anim_twist_bound.Pole`."""
    spec = SIDES[side]
    toes = spec["toes"] if spec["toes"] in rig else None
    finals = {}
    theta_foot = None
    for chain in ("arm", "leg"):
        incoming, source = None, None
        for bone, field, twist in spec[chain]:
            axes, rest = rig[bone]["axes"], rig[bone]["rest"]
            turns = own_angles(bone, axes, values, errors)
            if bone == spec["leg"][2][0]:
                theta_foot = turns[1]
                if foot.kind != 1 and (toes is not None or foot.kind == 2):
                    turns[1] = b.exact(0.)
            theta = turns[0]
            if incoming is not None and limb.c == 1:
                theta, incoming = (theta if incoming == (0., 0.) else b.add(theta, incoming)), None
            replace, outgoing = None, None
            if field is not None:
                tx, replace, outgoing = _split(limb, theta, settings[field])
            else:
                tx = b.tan_half(theta)
            joint = b.joint((tx, b.tan_half(turns[1]), b.tan_half(turns[2])))
            if replace is not None:
                joint = replace(joint)
            local = b.local(rest, axes, joint)
            # An exact zero share is the identity: B5a's local, its bits kept.
            if incoming is not None and incoming != (0., 0.):
                local = _receive(limb, local, incoming, bone, source, finals, rig)
            finals[bone] = local
            if outgoing is not None:
                nxt = spec[chain][[row[0] for row in spec[chain]].index(bone) + 1][0]
                sign = _sign(limb, axes, rig[nxt]["axes"])
                incoming, source = (sign * outgoing[0], outgoing[1]), bone
            else:
                incoming, source = None, None
    if toes is not None:
        axes, rest = rig[toes]["axes"], rig[toes]["rest"]
        turns = own_angles(toes, axes, values, errors)
        local = b.local(rest, axes, b.joint(tuple(b.tan_half(a) for a in turns)))
        if foot.kind != 1 and theta_foot != (0., 0.):
            axis = b.rotate(b.product(b.exact(tuple(rest)), b.exact(tuple(axes["pre"]))), _vector(Y))
            local = b.unit(b.product(b.axis_angle(axis, (foot.sign * theta_foot[0], theta_foot[1])), local))
        finals[toes] = local
    return finals


def canonical(limb, foot, settings, toes, signs, reach):
    """The key of a joint candidate on a cell: `settings` the four twist settings, `toes` {side: bool}, `signs`
    {side: {pair field: the source's x sign times the receiver's}}, `reach` `x_reach` of the cell's Avatar. Equal
    keys are one equation on the cell, and refuse alike (a tangent meets its pole on both or on neither):

    - w is the f32 number the cell gives (D2's 1 - t rounded), so D1 and D2 meet where t = .5.
    - w = 1 moves nothing (phi = 0 under every S, the source's joint B5a's), so S, R and sigma drop for that pair.
    - w = 0: S1 and S2 are both the zero x angle and phi = theta (2 atan(tan(theta/2)) = theta), where the source's
      x angle stays below pi: its own reach, plus under C1 its source's when one feeds it, and REACH_SLACK. S2 takes
      tan(theta/2) and S1 does not, so past that they refuse differently and stay apart. S3 stays distinct (its
      counter-rotation is outside the swing).
    - sigma is the resolved sign per side.
    - C1 is R1 at a receiver whose own pair keeps everything (w = 1) or with no source pair (Hand, Foot): adding
      to the x angle composes T_x on the joint's right, which is R1's T(post x) on the local's right. Likewise only
      where the receiver's x angle plus what it receives stays below pi: C1 takes the sum's tangent, R1 does not.
    - Foot: with toes F2 and F3 of a sign are one; without, F2's signs are one (lost) and F3 is F1.
    """
    key = []
    for side in sorted(toes):
        pairs, carried = [], []
        for field in range(4):
            t = settings[field]
            w = t if limb.d == 1 else _f32(1. - t)
            # The source's x angle: its own, plus under C1 what the pair before it sends (FIELDS 1 and 3).
            fed = limb.c == 1 and field in (1, 3) and pairs[field - 1] != ("keep",)
            carried.append(reach[side][SOURCES[side][field]] + (carried[field - 1] if fed else 0.))
            if w == 1.:
                pairs.append(("keep",))
                continue
            below = carried[field] + REACH_SLACK < math.pi
            split = "S12" if w == 0. and limb.s in (1, 2) and below else limb.s
            sign = {"+": 1., "-": -1., "s": signs[side][field]}[limb.sigma]
            pairs.append((w, split, sign))
        receivers = []
        # (incoming field, outgoing field or None) per receiver: LowerArm, Hand, LowerLeg, Foot.
        for incoming, outgoing in ((0, 1), (1, None), (2, 3), (3, None)):
            total = reach[side][RECEIVERS[side][incoming]] + carried[incoming] + REACH_SLACK
            if pairs[incoming] == ("keep",):
                receivers.append(None)
            elif limb.c == 1 and ((outgoing is not None and pairs[outgoing] != ("keep",)) or total >= math.pi):
                receivers.append("C1")
            else:
                receivers.append(f"R{limb.r}")
        if foot.kind == 1 or (not toes[side] and foot.kind == 3):
            feet = ("F1",)
        elif toes[side]:
            feet = ("F2", foot.sign)
        else:
            feet = ("lost",)
        key.append((side, tuple(pairs), tuple(receivers), feet))
    return tuple(key)


def x_reach(rig):
    """{side: {bone: the largest |x angle| its own muscles give}} over muscle values in [-1, 1], the tables'
    (0 on a bone no muscle turns on x): canonical's refusal rule."""
    reach = {}
    for side in SIDES:
        reach[side] = {}
        for bone in SOURCES[side] + RECEIVERS[side]:
            axes = rig[bone]["axes"]
            moved = any(MUSCLE_AXES[m] == 0 for m in bone_muscles(bone))
            reach[side][bone] = max(abs(axes["min"][0]), abs(axes["max"][0])) if moved else 0.
    return reach


def _f32(x):
    return b._f32(x)


def candidates():
    return tuple(itertools.product(LIMBS, FEET))
