# Plan: humanoid body motion

**Status.** Draft, 2026-11-18. The deferred slice of
[`anim.md` section 2.6](anim.md#26-later-slices). A feasibility study on the
P2b fixture recordings (both rigs, both Animator roots, Unity 2022.3 and
6000) found the model's shape but not all of its constants, so the slice
stays deferred: section 4 lists the Editor recordings that would settle it,
and PR B1 is the only one that can start now. The body-bearing refusal
(`unsupported_body_motion`) stays until section 3's gates pass.

**Deliverables.** Positive humanoid playback for clips whose hips Unity
places from the body pose: muscle-only clips (the P2b fixtures show their
hips always need this correction) and clips with `RootT`/`RootQ` body
curves, on the root-basis settings section 2.4 verifies. Each recovers the
hips-local TRS from the muscle vector, the body pose and the Avatar's
runtime tables, then feeds P2b's collector unchanged. Everything else keeps
its typed refusal (section 8).

**Starting point.** P2b PR 8: the runtime Avatar reader (`m_RootX`,
`m_Scale`, `m_Human.m_SkeletonPose`, axes, limits and the slot table), the
muscle-to-local-rotation equation confirmed on known vectors, the typed
refusals, the qualification tool `tools/parity/anim_humanoid_qualify.py`
and the recorder's `human_pose` mode
([reference](../anim_reference.md#humanoid-qualification)). DESIGN's risk
still holds: humanoid retargeting has no permissive reference other than a
Python implementation, and L11 must pass before any tuning.

## 0. Decisions

| # | Decision | Reason |
|---|---|---|
| 1 | Read the per-bone mass from the target Avatar's runtime `m_Human.m_HumanBoneMass`, never from a table in the source. | The Avatar serializes it, already normalized, with an absent optional bone's share folded into its parent. AssetStudio reads the field. A source table would be tuning. |
| 2 | Every placement constant and frame convention is a named hypothesis in the reference notes, frozen only when a recording isolates it (section 3.2). A constant only a least-squares fit pins stays a refusal. | DESIGN's L11-before-tuning rule. The feasibility fit matches every recording but leaves several constants free. |
| 3 | Solve the hips in closed form from the body pose: rotation from the body frame, then translation from the centre of mass. Add any iteration only if the read-back recording (F2) shows Unity's own solve is approximate. | The closed form reproduces all but one recorded sample (section 1). |
| 4 | Ship root projection only for the setting combinations a body-motion recording covers. The rest keep `unsupported_humanoid_root_basis` or `unsupported_humanoid_clip_settings`. | One recorded combination carries body slots today. |
| 5 | Lift the body-bearing refusal per verified configuration, not globally. | As P2b PR 8 did for muscle-only clips. |

## 1. Feasibility

Scripts outside the repository read the recordings through the repository's
replay files and `unityrs show`. Results, qualitatively:

- **Body position.** Masses at bone origins miss Unity by centimetres.
  Placing each bone's mass at a fixed fraction along its segment toward its
  human child reproduces the body position, and hence the hips translation,
  within the derived f32 bound on every recording: both rigs, identity and
  translated/rotated Animator roots, muscle and body-motion clips,
  known-vector `SetHumanPose` poses, and both Editor versions. A fit on one
  version and on identity roots predicts the other version and the offset
  roots within the bound, so the model is frame and version independent.
- **Not identified.** The fixtures only bend the spine and twist the left
  limbs, so the fractions of the spine chain, neck, head, shoulders and toes
  are fixed only in combination. Fractions fitted on one rig miss the other
  rig by centimetres. The identified ones are simple ratios (the hips at the
  centroid of its children, the shins and forearms at mid-segment).
- **Not mirror-symmetric.** On mirror-symmetric rigs the Avatar's rest body
  position is off the mid-plane by an amount that scales exactly with the
  upper-arm segment: the two upper arms' mass points sit at different
  fractions. The model must not assume left/right symmetry.
- **Body orientation.** The frame from Unity's description (up from the
  mid upper legs to the mid upper arms, lateral from the left-to-right
  vectors of both pairs, up kept exact) matches the recorded body rotation
  within the bound on the taller rig. On the compact rig its largest
  noncommuting sample is over the bound, while every alternative tested
  (equal-weight or single-pair lateral, other reference points, lateral kept
  exact, blended upper and lower frames) is much worse. Either the frame
  differs at second order or Unity's hips solve is inexact. F2 tells them
  apart.
- **Root projection.** Clips without body slots put the body at the Animator
  origin with identity rotation, under all eight keep-original combinations.
  The body-motion clip (Y kept, XZ and orientation from the body) leaves
  only the body height in the Animator frame. No other combination carries
  body slots.

So a mass model partly reproduces Unity's hips: translation within the
bound wherever its constants are pinned, rotation within the bound on one
rig. It is not established until section 4's recordings pin every constant.

## 2. Model

### 2.1 Body position

Unity documents the body position as the humanoid's centre of mass from an
average body-part mass distribution, divided by the Avatar's human scale,
and the human scale as the centre-of-mass height in T-pose. The candidate:

- `c(pose) = Σ_i m_i · (p_i + Σ_j a_ij (p_child_ij − p_i))` over the mapped
  human bones, with `m_i` from `m_HumanBoneMass` and `p` positions in the
  Animator root frame. `a_ij` are per-segment fractions, constant across
  rigs, toward the nearest mapped human descendants. An unmapped bone
  contributes nothing.
- The T-pose check `c(m_SkeletonPose) = m_RootX.t` and
  `m_RootX.t.y = m_Scale` holds on every Avatar before any clip is played,
  and refuses `unsupported_humanoid_body_model` otherwise.
- `bodyPosition = c / m_Scale`. Clip `RootT` is in this normalized space.

[Unity root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html) ·
[HumanPose.bodyPosition](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPose-bodyPosition.html) ·
[HumanPoseHandler.GetHumanPose](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPoseHandler.GetHumanPose.html) ·
[HumanTrait.GetBoneDefaultHierarchyMass](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanTrait.GetBoneDefaultHierarchyMass.html) ·
[Mecanim Humanoids](https://unity.com/blog/engine-platform/mecanim-humanoids) ·
[AssetStudio Avatar v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/Avatar.cs)

### 2.2 Body rotation

Unity describes the body orientation as an average of the lower and upper
body orientations relative to the Avatar T-pose, with the up vector from
the hips and shoulders middle points. The candidate:

- `up = normalize(mid(L,R UpperArm) − mid(L,R UpperLeg))`,
  `lateral = (R − L UpperArm) + (R − L UpperLeg)`,
  `forward = normalize(lateral × up)`, `right = up × forward`.
- `F(pose)` is the rotation with columns `(right, up, forward)`, and
  `bodyRotation = F(pose) · F(T-pose)⁻¹`. `RootQ` is this rotation.

The lateral weighting, the reference points and which axis is kept exact
are hypotheses (decision 2). F2 decides whether the compact rig's residual
belongs to the frame or to the solve.
[HumanPose.bodyRotation](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPose-bodyRotation.html)

### 2.3 Hips-local TRS

With every non-hips local rotation from P2b's muscle equation, place the
hips at the identity and evaluate `c_h` and `F_h` in the hips frame. For the
target body pose `(T, B)` in the Animator root frame (2.4):
`H = B · F(T-pose) · F_h⁻¹` and `P = T · m_Scale − H · c_h`. Then convert
`(P, H)` from the Animator root frame to the hips parent's frame as
section 2.4 of `anim.md` prescribes for a nonidentity root. A singular or
non-TRS chain refuses. Hips scale stays at rest.

### 2.4 Target body pose and root projection

- No body slots: `T = 0`, `B = identity`, for the verified settings.
- Body slots: `RootT`/`RootQ` minus the root transform, which is the
  Y-plane projection of the body under the clip's keep-original flags,
  `m_StartAtOrigin`, height-from-feet and the level and orientation offsets.
  With `applyRootMotion` off the projection is removed from the pose, and
  the root motion itself is not exported (section 7).
- Each combination is supported only once a body-motion recording covers it
  (decision 4). The first target is the recorded one: Y kept, XZ and
  orientation from the body.

[AnimationClipSettings](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AnimationClipSettings.html) ·
[Unity root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html)

## 3. Verification

### 3.1 f64 transcription

Extend `tools/parity/anim_reference.py` and its notes with 2.1–2.4, citing
only the public sources above, and freeze f64 bounds derived as in
[`anim.md` 4.2](anim.md#42-l11-and-f64-transcription): operation counts and
magnitude bounds through forward kinematics, the weighted sum, the
cross-product frame and its normalization, the quaternion solve and the
angular metric. Analytic cases: a single-segment rig, a rotated and
translated root, a uniform scale, an absent optional bone, and a
noncommuting pose.

### 3.2 Evidence levels

| Level | Evidence | Gate |
|---|---|---|
| E0 | Every recorded Avatar's `m_HumanBoneMass`, read through `unityrs show`, against the Editor's `HumanTrait` values; the T-pose check of 2.1. | Exact f32, both versions. |
| E1 | Each placement constant pinned by a known-vector recording that moves only its segment (F1). | One least-squares solution, with full rank. A constant that F1 leaves free stays a refusal. |
| E2 | The read-back (F2): Unity's own body pose after `SetHumanPose` against the requested one, and against 2.1–2.2 on the resulting bones. | Decides decision 3 and the frame hypothesis. |
| E3 | Python hips TRS (2.3) from the recorded non-hips rotations against every known-vector and clip recording: both rigs, the hold-out rig, all roots, scaled rigs and both versions. | Within the derived f32 bound per sample. The hold-out rig enters no fit. |
| E4 | Root projection: each body-motion recording of F4 under 2.4. | Within bound, per combination supported. |
| E5 | Rust same-bundle L11: Rust reads the bundle, samples the clip and writes the full pose, compared with the same replays. | Within the frozen replay bounds; this promotes the PARITY row. |

The derived Editor bound for a computed hips TRS follows the replay
tolerance procedure (`anim_reference.md`, per-recording f32 tolerances),
extended to the body path. The qualification tool's rest-track bound,
half an f32 ulp of the rest value, is for unanimated tracks and is too
tight here.

## 4. Editor fixtures (owner-run)

All are additions to the existing builder in `tools/fixtures/unity` and run
with the same button in both Editor versions. F2 and F3 change only the
recorder, so they need no new assets.

| # | Fixture | Settles |
|---|---|---|
| F1 | Per-segment known vectors on both rigs: each muscle group alone, left and right separately (legs, knees, feet, toes, shoulders, upper arms, forearms, hands, chest, upper chest, neck, head), then combined vectors. | Every placement fraction, including the asymmetric upper-arm pair and the spine chain (E1). |
| F2 | After each `SetHumanPose`, record `GetHumanPose`'s body position and rotation in the Animator root frame. | Frame versus solve (E2). |
| F3 | Known vectors with nonidentity body targets: offset position and tilted rotation. | The solve away from the rest body pose. |
| F4 | Body-motion clips with tilted `RootQ` and XZ/Y motion in each of the eight keep-original combinations, plus height-from-feet, a level offset and an orientation offset each alone, on identity and offset roots. | Root projection (E4). |
| F5 | Scaled rigs: a uniformly scaled Animator node, a proportionally larger skeleton, and an extra node with its own TRS between the Animator and the hips. | Human-scale and parent-frame conversion. |
| F6 | A rig missing Neck, Shoulders and Toes, and a third proportion rig (long arms, short legs) kept as a hold-out. | Mass folding and transfer to an unseen rig (E3). |

The recorder also writes `HumanTrait.GetBoneDefaultHierarchyMass` for every
bone so E0 has an independent Editor value.

## 5. If the study stays short

If F1 leaves a constant free, if E2 shows an inexact solve without a
public description, or if the hold-out rig misses its bound, the slice
stays deferred with the body-bearing refusal. The plan then records which
recording failed and what would settle it. No constant is shipped from a fit.

## 6. PRs

| # | PR | Contents | ~Non-test lines | Depends on |
|---|---|---|---|---|
| B0 | Plan | This document and the `anim.md` 2.6 link | docs | — |
| B1 | Recorder and fixtures | F1–F6 in the builder, read-back and mass rows in the recorder, contract and `complete` checks, owner workflow notes | 0.6–0.9k | B0 |
| B2 | Reference and body checker | 2.1–2.4 in the f64 reference with frozen bounds, the analytic cases, and a checker over a completed run for E0–E4 that freezes each constant with the recording that pins it | 0.6–0.9k | B1 recordings |
| B3 | Rust body model | `m_HumanBoneMass` read and validated, T-pose check, centre of mass, frame and closed-form hips solve, synthetic tests and fuzz targets for the new reads | 0.5–0.7k | B2 |
| B4 | Root projection and frames | Target body pose per verified setting combination, root-to-hips-parent and scale conversion, narrowed refusals | 0.4–0.6k | B3, F4, F5 |
| B5 | Integration and L11 | Lift the body-bearing refusal per verified configuration, collector integration, the gated same-bundle replay (E5), PARITY row | 0.4–0.6k | B4 |

**Total estimate.** 2.5–3.7k non-test lines. B1 can start now. B2 waits
for the owner's runs of B1 in both Editor versions, and B3–B5 for B2's
frozen constants. The 6000 bundles need the Unity 6 reader on main.

## 7. Risks and gates

- **Fitted constants passing as verified.** A least-squares fit matches the
  current recordings and still misses another rig by centimetres. Gate:
  E1's isolation and E3's hold-out rig, decision 2.
- **Unpublished solve.** If Unity's hips solve iterates, the closed form can
  differ from it at second order. Gate: E2 before B3.
- **Asymmetry.** A symmetric shortcut breaks the rest body position. Gate:
  the T-pose check on every Avatar.
- **Version drift.** The same model must hold in 2022.3 and 6000; each
  constant is checked per version.
- **Cost.** One forward-kinematics pass and one weighted sum per sample on
  the existing metered path; no new allocation per sample.

## 8. What stays refused

Mirror, cycle offset, loop blend, additive reference poses, IK goals,
translation DOFs, twist redistribution and stretch keep their P2b refusals.
Root basis and clip-setting combinations without an F4 recording stay
`unsupported_humanoid_root_basis` or `unsupported_humanoid_clip_settings`.
Non-uniform scale on the Animator chain and a non-TRS hips parent refuse.
The root motion trajectory (the accumulated root delta that
`applyRootMotion` would apply) is not exported; that is a later slice.
Controller state and blend-tree evaluation stays in its own later slice.
