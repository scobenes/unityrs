# Plan: humanoid body motion

**Status.** Draft, 2026-11-18, with two review rounds folded in. This is the
deferred slice of [`anim.md` section 2.6](anim.md#26-later-slices). A
feasibility study on the P2b fixture recordings found the shape of the
model but not all of its constants, so the slice stays deferred. Section 4
lists the Editor recordings that would settle it. PR B1 is the only one
that can start now. The body-bearing refusal (`unsupported_body_motion`)
stays until section 3's gates and B5's release gate pass.

**Deliverables.** Positive humanoid playback for clips whose hips Unity
places from the body pose. These are muscle-only clips (on the P2b fixtures
their hips always need this correction) and clips with `RootT`/`RootQ` body
curves, in the configurations B5's predicate admits. Each clip recovers the
hips-local TRS from the muscle vector, the body pose and the Avatar's
runtime tables, then feeds P2b's collector unchanged. Everything else keeps
its typed refusal (section 8).

**Starting point.** P2b PR 8 supplies the runtime Avatar reader (`m_RootX`,
`m_Scale`, `m_Human.m_SkeletonPose`, axes, limits and the slot table), the
muscle-to-local-rotation equation confirmed on known vectors, the typed
refusals, the qualification tool `tools/parity/anim_humanoid_qualify.py`
and the recorder's `human_pose` mode
([reference](../anim_reference.md#humanoid-qualification)). DESIGN's risk
still holds: humanoid retargeting has no permissive reference other than a
Python implementation, and L11 must pass before any tuning.

**PARITY row.** This slice owns the tier X, L11 row "Humanoid local poses"
in `PARITY.md` section 24.1, with status `planned (Humanoid body motion)`.
B0 names it. B5b promotes it to verified for the configurations its
predicate admits, and nothing else. The row B5b writes, drafted here and
not yet in `PARITY.md` (the status stays planned until E5 passes):

| Area | Behaviour | Tier | Level | Evidence | Test | Status |
|---|---|---|---|---|---|---|
| Humanoid local poses | Humanoid playback at L11 with the hips placed from the body pose: the muscle pose of every bone (B5a), the centre of mass from the Avatar's `m_HumanBoneMass`, the body frame, the closed-form hips solve and root projection, for muscle-only clips and clips with `RootT`/`RootQ` body curves, in exactly the configuration classes of the release gate's verified table (`humanoid/gate.rs`), each recorded by E5 and passing in both Editor versions. Every other configuration keeps its typed refusal; the humanoid typed refusals row above holds what P2b verified | X | L11 | Same-built-bundle Editor playback (E5, round 11's clip and chain rigs) and known-vector `SetHumanPose` recordings (B5a's muscle pose, all 95 muscles) on the plan's code-built rigs, with derived f32 tolerances | E5's same-bundle replay over the owner's round 11 runs, and B5a's muscle-pose comparison | verified (Humanoid body motion), for the verified table's classes |

## 0. Decisions

| # | Decision | Reason |
|---|---|---|
| 1 | Read the per-bone mass from the target Avatar's runtime `m_Human.m_HumanBoneMass`, never from a table in the source. | The Avatar serializes it, and AssetStudio reads the field. A table in the source would be tuning. Its normalization and folding are checked in E0. |
| 2 | Every placement coefficient and frame convention is a named hypothesis in the reference notes. Each is frozen only when a probe excites it under the conditioning criterion of 2.5. A coefficient that only a least-squares fit over unisolated poses pins stays a refusal. | DESIGN's L11-before-tuning rule. The feasibility fit matches the recordings it was fitted on but leaves several coefficients free. |
| 3 | Solve the hips in closed form from the body pose: rotation from the body frame, then translation from the centre of mass. B3 waits for E2 to show that Unity's own solve is that closed form. | The closed form reproduces all but one recorded sample (section 1). |
| 4 | Ship root projection only for the setting combinations an F4 recording covers. The rest keep `unsupported_humanoid_root_basis` or `unsupported_humanoid_clip_settings`. | Today one recorded combination carries body slots. |
| 5 | Lift the body-bearing refusal per configuration, through a runtime predicate over recorded configurations (B5), never globally. | P2b PR 8 qualified no clip. This slice is the first to lift the refusal, so its scope has to be explicit. |

## 1. Feasibility

Scripts kept outside the repository read the recordings through the
repository's replay files and `unityrs show`. The data were two rigs of
different proportions: one mirror-symmetric rig with every optional torso
bone, and a taller one without UpperChest. Each had an identity Animator
root and one translated and rotated root, in Unity 2022.3 and 6000. Every
claim below is limited to those rigs, roots and versions.

- **Body position, fitted.** With masses at bone origins the model misses
  Unity by centimetres. With each bone's mass at a fixed point along its
  segments toward its mapped human children (2.1), one set of coefficients
  fitted on all the recordings reproduces the body position, and so the
  hips translation, within the derived f32 bound. This is an in-sample fit.
- **Body position, predicted.** A fit on the 2022.3 identity-root
  recordings alone predicts the 6000 recordings and the offset-root
  recordings within the bound. Those recordings share the fitted rigs and
  poses, so this tests only frame and version independence. A fit on one rig
  predicts the other rig's hips centimetres off, so it is not yet known
  whether the coefficients are the same for every rig (2.5).
- **Not identified.** The fixtures only bend the spine and twist the left
  limbs, so several coefficients are fixed only in combination (2.5).
- **Not mirror-symmetric.** On the mirror-symmetric rig the Avatar's rest
  body position lies off the mid-plane. Left and right coefficients are
  therefore separate unknowns.
- **Body orientation.** The frame from Unity's description (2.2) matches the
  recorded body rotation within the bound on the taller rig. On the other
  rig, the largest noncommuting sample is over the bound. Every alternative
  frame tested (equal-weight or single-pair lateral, other reference points,
  lateral kept exact, blended upper and lower frames) does much worse. Either
  the frame differs at second order or Unity's hips solve is inexact. E2
  tells the two apart.
- **Root projection.** Clips without body slots put the body at the
  Animator origin with identity rotation, in all eight keep-original
  combinations with bake off. The one body-motion clip (Y kept original, XZ
  and orientation from the body, bake off, starting at the origin) leaves
  only the body height in the Animator frame. Every recorded clip stores
  `m_StartAtOrigin` on. No other combination carries body slots.

So a mass model partly reproduces Unity's hips, on the tested rigs and
frames only. Its translation is within the bound when fitted, and its
rotation is within the bound on one rig. It is not established until section
4's recordings pin every coefficient, and its constancy across rigs stays a
hypothesis until the hold-out rig passes.

## 2. Model

All of section 2 is hypothesis (decision 2). Frames: `A` is the Animator
node's local frame. Clip body quantities are in the clip's motion space,
with position in normalized units, meaning metres divided by `m_Scale`.

### 2.1 Body position

Unity documents the body position as the humanoid's centre of mass, taken
from an average body-part mass distribution and divided by the Avatar's
human scale. It documents the human scale as the centre-of-mass height in
T-pose. The candidate:

- `c(pose) = Σ_i m_i · (p_i + Σ_j a_ij (p_ij − p_i))` over the mapped human
  bones `i`. Here `m_i` is the bone's `m_HumanBoneMass` entry (E0), `p` are
  positions in one frame, and `p_ij` are the nearest mapped human
  descendants of `i` (the edges of 2.5). The `a_ij` are dimensionless
  coefficients.
- Position-only hypothesis (H-pos): a mass point is an affine combination
  of mapped bone positions. Rotating a terminal bone (one with no mapped
  human descendant) therefore cannot move `c`. The terminal probes of 2.5
  test this.
- `bodyPosition = c / m_Scale`. `RootT` is in this normalized space.
- Scope, frozen on the training rigs (B2): the per-edge fractions fit
  every training topology jointly except four. Those are Neck absent,
  Chest absent, and exactly one Shoulder mapped. Each fits alone, but no
  shared coefficient set fits it beside the others. Folding the absent
  mass to siblings or descendants did not fit them, nor did one fraction
  toward the children's centroid, fractions keyed by the child set, or
  sparse cross-bone terms. So these topologies refuse at the Avatar
  (section 8), and the fit runs on the other eighteen training rigs. The
  scope admits only a pattern of absent body slots that a training rig
  recorded, over every slot but the Eyes and the Jaw (the zero-mass leaves
  of 2.5), so an unrecorded topology (UpperChest and one Shoulder absent)
  refuses too. A leaf's mass must be exactly 0, mapped or not. The check
  reads the Avatar's `m_HumanBoneIndex` and `m_HumanBoneMass` before any
  forward kinematics.

[Unity 2022.3 root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html) ·
[HumanPose (bodyPosition, bodyRotation)](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPose.html) ·
[HumanPoseHandler (GetHumanPose, SetHumanPose)](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPoseHandler.html) ·
[HumanTrait (GetBoneDefaultHierarchyMass)](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanTrait.html) ·
[Mecanim Humanoids](https://unity.com/blog/engine-platform/mecanim-humanoids) ·
[AssetStudio Avatar v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/Avatar.cs)

### 2.2 Body rotation

Unity describes the body orientation as an average of the lower and upper
body orientations relative to the Avatar T-pose, with the up vector taken
from the middle points of the hips and shoulders. The candidate:

- `up = normalize(mid(L,R UpperArm) − mid(L,R UpperLeg))`,
  `lateral = (R − L UpperArm) + (R − L UpperLeg)`,
  `forward = normalize(lateral × up)`, `right = up × forward`.
- `F(pose)` is the rotation with columns `(right, up, forward)`.
  `bodyRotation = F(pose) · F(T)⁻¹`, where `T` is the Avatar T-pose
  (`m_SkeletonPose`). `RootQ` is this rotation. `F` depends only on
  directions, so a uniform scale does not change it.
- Scope, frozen on the training rigs (B2): the read-back differs from this
  frame only by a turn about `up`. That turn grows with the shoulder-hip
  twist, the angle about `up` between the upper-arm and upper-leg
  laterals. A bounded search on the training rigs found no rule that keeps
  every sample within bound. It tried mass-weighted laterals, a yaw
  average or nlerp of an upper and a lower frame, lateral-first and
  symmetric orthogonalization, and laterals from other bones. A global
  lateral weight made it worse on either side of 1. So 2.2 holds up to a
  twist of 4.25°, just under the smallest twist that failed: every
  training sample at or below it is within bound, in both versions. A
  larger twist refuses (section 8), and so does an undefined one (a
  lateral along `up`). A second search found nothing either. It tried
  laterals from left-versus-right mass centroids, mass-weighted sums of the
  projected laterals, normalized laterals and an up from mass-weighted
  midpoints, alone and combined. Each was worse than the plain sum.
- Lateral domain (B2): within the twist scope, the frame still leaves the
  read-back by a pure turn about `up` of up to 0.08° once the spine bends
  sideways. Every such sample has a spine Left-Right muscle (Spine, Chest
  or UpperChest) past the frozen limit. The turn is second-order and
  unexplained: it stays at a near-zero arm-leg yaw gap with oppositely
  tilted laterals, where every rule weighting the two laterals (and their
  sum) is exact. Read-back muscles match the requested ones, so no muscle
  clamp causes it. Axial, polar, half-angle, cosine-power and averaged
  candidates were all ruled out. So 2.2 holds up to the lateral limit,
  set by the checker from the training runs and recorded in the freeze:
  the largest spine Left-Right value of a training sample the model
  compared and passed (a sample the twist scope refused validates nothing)
  that is at least 1/32 below every sample failing E2 or E3a, over every
  training observation: a known vector's applied muscles, or a clip
  sample's muscles sampled from its built clip at its clock. It is
  always an observed passing value, never a clamped untested one, and the
  freeze records the two observations that bracket it: the passing sample
  at the limit and the smallest failing one. A pose past it refuses
  (section 8). Without every required band sweep recorded there is
  no limit, and a limit under 1/16 validates nothing: E2 and E3a then
  fail. F9's band sweeps sample the band past it on the training
  rigs. A clip past the limit gets the named refusal
  (`spine_lateral_range`, section 8). The scope widens only if a later
  model explains the second-order yaw and passes the band sweeps.

### 2.3 Hips-local TRS and scale

The chain from the Animator node to the hips has these parts:

- `(t_p, R_p, σ_p)` is the hips parent's transform in `A`: the composed
  translation, rotation and uniform scale of the nodes between the Animator
  node and the hips parent. `σ_p = 1` when the parent is the Animator node.
- `s_h` is the hips' own uniform local scale, held at rest.
- `k = σ_p · s_h` is the scale of the hips frame in `A`.

The solve, given the target body pose `(T_A, B_A)` in `A` in metres (2.4):

1. Evaluate `c_h` and `F_h` in the hips local frame from every non-hips
   local rotation (P2b's muscle equation), the descendants' local
   translations from the Avatar's `m_SkeletonPose` (the ones the playback
   writes, E3a) and their rest scales. `c_h` excludes `s_h` and every
   ancestor's scale.
2. `H_A = B_A · F(T) · F_h⁻¹` (scale-free).
3. `P_A = T_A − k · H_A · c_h`.
4. Local hips: `t = σ_p⁻¹ · R_p⁻¹ (P_A − t_p)`, `q = R_p⁻¹ H_A`, scale `s_h`.

F5 decides two questions:

- whether the Animator node's own scale applies on top of `m_Scale`;
- whether a non-unit `s_h` enters as `k` above.

A non-uniform scale, a non-TRS link anywhere from the Animator node to a
mapped human bone, or a singular chain refuses
(`unsupported_animator_root`).

### 2.4 Target body pose and root projection

For a clip with body slots, `X_b(t) = (RootT(t), RootQ(t))` is the body in
motion space, in normalized units. The root `X_r(t) = (r(t), y(t))` is the
projection of the body onto the Y plane. `y` is a rotation about the up
axis only. The body in `A` is the rigid inverse composition
`X_r(t)⁻¹ ∘ X_b(t)`:

- `T_A = m_Scale · y(t)⁻¹ (RootT(t) − r(t))`
- `B_A = y(t)⁻¹ · RootQ(t)`

A rotation's yaw is its twist about the up axis, `2·atan2(q_y, q_w)`.
The evidence is the training fixtures (B2's smoke on the F4 clips, both
versions). The yaw implied by the read-back, `RootQ · B_A⁻¹`, is a pure
turn about up. It equals the twist, not the forward's heading
`atan2(f_x, f_z)`, which is off by tenths of a degree on tilted `RootQ`.
With the twist, every evaluable F4 combination passes E4.

The root's components come from the serialized clip fields. The bake flags
are pinned apart from the keep-original flags:

| Component | Bake into pose (`m_LoopBlendOrientation`, `m_LoopBlendPositionY`, `m_LoopBlendPositionXZ`) on | Off, keep original on (`m_KeepOriginalOrientation`, `m_KeepOriginalPositionY`, `m_KeepOriginalPositionXZ`) | Off, based on the body |
|---|---|---|---|
| `y` (yaw) | constant: the start value | original root yaw plus `m_OrientationOffsetY` | the body's yaw plus `m_OrientationOffsetY` |
| `r.y` | constant: the start value | original root height plus `m_Level` | body height plus `m_Level`, or the lowest foot with `m_HeightFromFeet` |
| `r.xz` | constant: the start value | original root XZ | body XZ |

**Serialized inputs.** These are the `ClipMuscleConstant` fields AssetStudio
reads. Which of them each cell uses is part of the hypothesis.

- **Original root.** The clip's motion slots (`MotionT`, `MotionQ`, the
  first seven DOFs of `m_IndexArray`, where P2b's slot table already
  classifies them), with `m_MotionStartX` and `m_MotionStopX`. A clip whose
  original-root cell needs these slots while they are absent refuses.
- **Body start.** `m_StartX` and `m_StopX`, the body at the clip's start and
  stop.
- **Lowest foot.** Two candidates:
  - A: the lower of the two foot goal heights (`LeftFootT`, `RightFootT`
    goal slots) in motion space;
  - B: the lower of the mapped Foot bones' heights from forward kinematics
    of the pose placed at `X_b`.

  `m_LeftFootStartX` and `m_RightFootStartX` give the start reference
  either way. F4's height-from-feet clips, with and without goal curves and
  with foot height changing relative to the body, choose between them or
  leave the cell refused.

**Start reference.** "Start value" means the projection at the clip's start
time.

- With `m_StartAtOrigin` on, the start XZ and yaw are subtracted, so the
  root starts at the motion-space origin.
- If `RootT`/`RootQ` and the root share that rebased frame, the rebase
  cancels in `X_r⁻¹ ∘ X_b`. Whether they share it is unknown. F4's
  nonzero-start clips settle it: the same clip with and without a start
  offset must give the same `(T_A, B_A)`.
- `AnimationClipSettings` exposes no field for `m_StartAtOrigin`. Its value
  comes from the built clip, and every recorded built clip stores it on
  ([crosswalk](../anim_reference.md#humanoid-qualification)). A clip that
  stores it off refuses until a recording covers that case.

**Other cases.**

- **No body slots.** `T_A = 0` and `B_A` = identity, as recorded for the
  bake-off combinations.
- **Root motion off.** With `applyRootMotion` off the projection only
  re-expresses the pose. The root trajectory itself is not exported
  (section 8).
- **Scope.** Each cell, and each combination of cells, is a hypothesis
  until an F4 recording of that combination passes E4 (decision 4).

[Unity 2022.3 root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html) ·
[ModelImporterClipAnimation](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/ModelImporterClipAnimation.html) ·
[AnimationClipSettings, UnityCsReference 2022.3](https://github.com/Unity-Technologies/UnityCsReference/blob/2022.3/Editor/Mono/Animation/AnimationClipSettings.bindings.cs) ·
[Bake Into Pose toggles, UnityCsReference 2022.3](https://github.com/Unity-Technologies/UnityCsReference/blob/2022.3/Editor/Mono/Inspector/AnimationClipEditor.cs) ·
[AssetStudio AnimationClip v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/AnimationClip.cs)

### 2.5 Edges, probes and identifiability

An edge joins a mapped bone `i` to one of its nearest mapped human
descendants `j`. `a_ij` enters `c` only through `v_i = Σ_j a_ij t_ij`, where
`t_ij` is `j`'s offset in `i`'s frame. Rotating bone `i` relative to the hips
excites `v_i`. The individual `a_ij` of a bone with several children separate
only across rigs whose `t_ij` are linearly independent. Every F1 probe runs
on every rig that has its edge, including each F6 rig.

| Edge (topology) | Probe (F1 unless named) | Identifiability |
|---|---|---|
| Hips → Spine, L/R UpperLeg | None: a fixed vector in the hips frame | `v_Hips` comes from the T-pose and every pose once the rest are pinned. The individual coefficients need non-coplanar hip offsets across rigs (F6). Until then a combination stays free (see below). |
| L/R UpperLeg → LowerLeg | Upper Leg Front-Back and In-Out, per side | Per side |
| L/R LowerLeg → Foot | Lower Leg Stretch, per side | Per side |
| L/R Foot → Toes (Toes mapped) | Foot Up-Down, per side | Per side |
| Spine → Chest | Spine muscles with the chest held | The spine moves the whole chain above it, so this separates only together with the chest probes |
| Spine → Neck, L/R Shoulder (no Chest, so no UpperChest) | Spine muscles | Branch, F6 rig |
| Chest → UpperChest | Chest muscles | Single |
| Chest → Neck, L/R Shoulder (no UpperChest) | Chest muscles | Branch |
| UpperChest → Neck, L/R Shoulder | Upper Chest muscles | Branch |
| (Upper)Chest → Head (no Neck) | Chest or Upper Chest muscles | Branch with the shoulders, F6 rig |
| (Upper)Chest → L/R UpperArm (that Shoulder absent) | Chest or Upper Chest muscles | Branch, F6 rig |
| Neck → Head | Neck Nod, Tilt and Turn | Single |
| Head → L/R Eye, Jaw (any of them mapped) | Head Nod, Tilt and Turn | Branch, F6 rig. Head is then not terminal. |
| L/R Shoulder → UpperArm | Shoulder Down-Up and Front-Back, per side | Per side |
| L/R UpperArm → LowerArm | Arm Down-Up and Front-Back, per side | Per side, including the left/right difference |
| L/R LowerArm → Hand | Forearm Stretch, per side | Per side |
| Terminal: Head (no Eyes or Jaw), L/R Hand, L/R Toes, L/R Foot (that Toes absent), each mapped Eye, Jaw | That bone's own muscles alone | No coefficient under H-pos. The probe must leave the hips unmoved within the hips tolerance, or that terminal refuses `unsupported_humanoid_body_model`. |
| Twist muscles | Arm, forearm, upper and lower leg twist | Excite nothing when the child lies on the twist axis. Twist stays refused (section 8). |

**Zero-mass leaves.** The Eyes and the Jaw take no part in the topology
scope. Their mass is exactly 0 in every recorded Avatar of both versions,
and no edge leaves them. Their only columns are the Head edges', whose
fitted coefficients stay within `|a| ≤ 8e-6`. Dropping those edges from
the fit changes no shared coefficient by more than 7.4e-6. So a pattern
differing from a recorded one only in its Eyes or Jaw is the recorded
pattern. This holds only while their mass is 0, so an Avatar with a
nonzero Eye or Jaw mass refuses the scope (section 8).

**Tolerances.** Two are derived separately in B1, from the replay tolerance
procedure already in the tree (`anim_reference.md`, per-recording f32
tolerances), so B1 does not depend on B2's freeze.

- **Measurement tolerance `ε_r`**, for each fitting row: the f32 error of
  the recorded pose that row is built from. This covers Unity's f32 path
  for that pose and the rounding of its recorded outputs, propagated
  through the f64 centre-of-mass equation.
- **Unity's error `E_unity,s`**, for each predicted sample: the derived
  bound on Unity's own f32 error in the recorded hips-local translation,
  along the body path, each operation bounded at its actual operands. That
  path is hips-local forward kinematics from zero, the identity and unit
  scale at the hips (2.3), the weighted sum, the frame and the target body
  rotation, the solve, the hips parent's composed transform and the f32
  output. A known vector's target is the exact body pose the recorder
  passed to SetHumanPose. A clip's target stands in by its read-back, with
  Unity's forward centre-of-mass and frame errors, until E4 checks a
  target independently. It is the recording's noise floor.
- **Model error `E_eval,s`**: the derived error of evaluating the model at
  `s` with the fitted coefficients. B2's f64 reference runs the same path
  from the recorded f32 inputs, each rounding counted once at `2^-53`,
  plus one f32 rounding of the hips translation it stores, bounded by the
  model's pre-cast magnitude.
- **Coefficient uncertainty `u_s`**, from the conditioning criterion below.

E3's comparison tolerance is their sum, `T_s = E_unity,s + u_s + E_eval,s`:
a recorded hips translation and the model's differ by at most Unity's
error, the fitted coefficients' worst-case error and the model's own.

B2 reruns the check with its frozen bounds if they differ.

**Conditioning criterion.**

1. The weighted design matrix `A_w = diag(1/ε) A` over the coefficients
   2.5 calls identifiable must have full column rank and a 2-norm
   condition number of at most `1e6`.
2. For every recorded and hold-out sample `s`, let `J_s` be the
   sensitivity of its hips translation to the coefficients. The propagated
   coefficient uncertainty `u_s = ‖J_s A_w⁺‖_∞` must satisfy
   `u_s ≤ E_unity,s`: the fitted coefficients' worst-case uncertainty may
   not exceed the recording's own noise floor. A design that fails this is
   short. `J_s` uses the solved hips orientation `H_A` of 2.3, not the
   recorded one.

The budget is closed by construction: `T_s` adds each error once, and the
criterion keeps the coefficient term no larger than Unity's own. Neither
`u_s` nor `E_eval,s` is carved out of `E_unity,s`, so no fixed split is
needed.

`‖A_w A_w⁺‖_∞ ≥ 1` bounds the projector as a whole, not each of its rows,
so a sample's own `E_unity,s` against its `ε` is no veto. The check
compares `u_s` directly, per component of the hips translation, against
`E_unity,s`. Each `ε` bounds the Euclidean norm of one sample's residual,
so its three equations form one block, and `u_s` sums the 2-norms of
`J_s A_w⁺`'s blocks.

**Free combinations.** A coefficient combination that fails the criterion
stays free. B3 computes, for the target Avatar, whether the Avatar's geometry
projects onto a free combination. If it does, the Avatar refuses
`unsupported_humanoid_body_model`. A combination that projects to zero on
that Avatar does not affect its hips.

**Topologies.** Support is limited to the topologies F6 records (section
4). Any other absence pattern refuses, as does any combination of
absences F6 does not record.

## 3. Verification

### 3.1 f64 transcription

Extend `tools/parity/anim_reference.py` and its notes with 2.1–2.5,
citing only the public sources above. Freeze f64 bounds derived as in
[`anim.md` 4.2](anim.md#42-l11-and-f64-transcription), from the operation
counts and magnitude bounds through these steps:

- forward kinematics with scale
- the weighted sum
- the cross-product frame and its normalization
- the quaternion solve
- the inverse rigid composition of 2.4
- the angular metric

The analytic cases are a single-segment rig, a rotated and translated root,
a uniform intermediate scale, a non-unit hips scale, an absent optional
bone, a noncommuting pose, and each projection cell of 2.4.

### 3.2 Evidence levels

| Level | Evidence | Pass |
|---|---|---|
| E0a | Decode `m_HumanBoneMass` in both versions: the read is as AssetStudio's, with one f32 per slot of the 25-slot `m_HumanBoneIndex` order (HumanBodyBones 0–8, UpperChest, 9–23). Each mapped slot's skeleton node (`m_Skeleton.m_ID` through `m_TOS`) is the captured bone's node, by path. | Exact length and node assignment: a swapped or aliased slot fails. Malformed data refuses. |
| E0b | Normalization and folding, specified before any comparison: each entry is the Editor's default (own) mass for that slot plus those of the absent optional slots it is the nearest present ancestor of, divided by the sum over present slots. An absent slot is zero. The recorder gives `GetBoneDefaultHierarchyMass`, the slot's mass plus all its descendants', so a slot's own mass is its hierarchy mass less its body-slot children's. Only the body slots are read, and compared across recordings: Unity 6 returns unspecified finger values. Compare on every F6 topology; the hold-out's Avatar only in E3b. | Within a derived bound for the sum and division, plus the own masses' subtraction error |
| E0c | Reproduce `m_RootX.t` from `m_SkeletonPose` with 2.1, and `m_RootX.t.y` against `m_Scale`. | Within its own derived bound (forward kinematics of the skeleton pose plus the weighted sum), on every recorded Avatar. An infinite bound fails. |
| E0d | 2.2's `F(T)` and 2.3's rest translations and scales come from `m_SkeletonPose`. Compare each mapped bone of the recorded rest pose with it. | Within both forward kinematics' errors |
| E1 | Each coefficient that 2.5 calls identifiable, pinned by its probes on the training rigs | 2.5's conditioning criterion with B1's tolerances |
| E2 | F2 read-back on the failing combined-axis pose and on independent noncommuting probes (F1 combined vectors, F3 tilted targets). Convert the world read-back to `A` as the recorder converts the rest body: position by the inverse Animator transform of `bodyPosition · humanScale`, divided by `humanScale`; rotation by the inverse Animator rotation. GetHumanPose's `bodyPosition` is already free of the Animator's scale, so the recorded position is divided by that scale once too often; the checks multiply it back. Bound two residuals separately: requested-to-read-back, and model (2.2 on the recorded bones) to read-back. | Both residuals within their own derived angular bounds on every probe. Anything else is a frame or solve failure, and B3 stays blocked. |
| E3a | Training checkpoint, before the freeze: Python hips TRS (2.3) from the recorded non-hips rotations under the Avatar's `m_SkeletonPose` translations and the rest scales, against every known-vector and clip recording of the training rigs (every rig but the hold-out), all roots and scaled rigs, in both versions. The case inventory is run.json's, and every training body case is required. | `u_s ≤ E_unity,s` and within `T_s = E_unity,s + u_s + E_eval,s` (2.5) per sample, with the translations below the hips exactly the Avatar's `m_SkeletonPose` and the scales below the hips and the hips' scale exactly the rest's. The playback writes `m_SkeletonPose`'s translations: on the training rigs of both versions every sampled one equals it bit for bit, while the prefab rest, `m_AvatarSkeletonPose` and `m_DefaultPose` miss it by up to 9e-8 m of f32 rounding. The sampled scales equal the rest's, which `m_SkeletonPose` misses by one ulp. A node below the hips that the Avatar's skeleton lacks refuses. A body sample without a target fails, and a P2b case without one is excluded by role. It must pass before B2 commits the freeze. |
| E3b | Hold-out evaluation, after the freeze: E0 on the hold-out's Avatar and the same comparison on the hold-out rig, with the stored coefficients only. The checker refuses an artifact that git does not hold unmodified at HEAD, or whose E3a, model, bounds, model-source digest or reproduced fit differ from the code's. | Within `T_s` (2.5) per sample, with the frozen coefficients' `u_s` |
| E4 | Root projection (2.4) from the built clip's runtime body and motion slots and serialized `m_StartX`, against the root or residual body pose that F4 records, independently of the hips solve | Within bound, with each slot's f32 sampling error, ε once, and an unbounded yaw refused, per combination supported. A start variant without its twin fails. |
| E5 | Rust same-bundle L11: Rust reads the bundle, samples the clip and writes the full pose, compared with the same replays. | B5's release gate |

**E0 at runtime.** B3's Rust reader checks the parts of E0 that need no
recording, and refuses an Avatar that fails them: mapped slots resolving
to distinct `m_TOS` paths, one finite, non-negative mass per slot, zero on
an absent slot, the masses summing to one within E0b's bound, the topology
scope of 2.1, and `m_RootX.t.y` against `m_Scale` (E0c). A swap of two
valid slots (E0a) and each slot's mass against the fold of the Editor's
defaults (E0b) need the recorder's capture and `HumanTrait` table. They
stay B2's checker's verification of this plan's reading of the Avatar on
the recordings, not runtime refusals: the model takes the serialized
masses as they are (decision 1), and the crate holds no default mass
table.

The qualification tool's rest-track bound is half an f32 ulp of the rest
value. It is meant for unanimated tracks and is too tight here. The derived
tolerances of 2.5 replace it on the body path.

## 4. Editor fixtures (owner-run)

Each fixture is an addition to the existing builder in `tools/fixtures/unity` and
runs with the same button in both Editor versions. F2 and F3 change only the
recorder, so they need no new assets. The recorder also writes
`HumanTrait.GetBoneDefaultHierarchyMass` for every bone, for E0b.

| # | Fixture | Settles |
|---|---|---|
| F1 | Known vectors for each probe in the 2.5 table, each muscle group alone and left and right separately, then combined noncommuting vectors. They run on both P2b rigs and again on every F6 rig, for the edges that rig adds. | E1 and the terminal (H-pos) checks |
| F2 | After each `SetHumanPose`, record `GetHumanPose`'s body position, rotation and muscles, converted to `A`, with Unity's own world and hips-relative transform of every node. A clip also plays again with root motion applied. | E2 |
| F3 | Known vectors with nonidentity body targets: an offset position and a tilted rotation | The solve away from the rest body pose |
| F4 | See F4 below. | E4 and the open questions of 2.4 |
| F5 | A uniformly scaled Animator node; a proportionally larger skeleton; an extra node with a non-unit uniform scale, a translation and a rotation between the Animator and the hips; and a non-unit hips local scale `s_h`. Each is played with nonzero body translation and a tilted body rotation (F3 targets and the all-original F4 clip with changing `MotionT`/`MotionQ` curves). | Scale and parent-frame conversion (2.3) |
| F6 | One rig per supported topology, each on the compact base rig. The absences: Chest (so also UpperChest), UpperChest alone, Neck, the left Shoulder, the right Shoulder, both Shoulders, the left Toes, the right Toes, both Toes, and UpperChest with both, the left or the right Toes (these three required in every run that has them: Unity refusing one is an error). Mapped extras: Eyes only, Jaw only, Eyes and Jaw. Also a rig whose hip and shoulder child offsets are not coplanar, and a third-proportion rig (long arms, short legs) kept as the hold-out. | Folding, branch coefficients and transfer to an unseen rig (E0b, E1, E3b) |
| F7 | Twist sweeps on the compact and tall training rigs (never the hold-out): the Spine and Chest twist muscles together, turning the upper body by 0, ±2, ±4, ±6, ±8, ±10, ±15, ±20, ±30, ±45 and ±60°, each under an upright body target and three pitch and roll tilts from F3's, with F2's read-back. They are required once the rig's probe case records, and the adequacy checkpoint does not read them. | 2.2's frame against the shoulder-hip twist and the tilt: a search for Unity's lateral rule, or a measured approximate tier (E2) |
| F8 | Range sweeps on the compact, tall and noncoplanar training rigs (never the hold-out): every mapped spine bone's three muscles over the whole cube [-1, 1]³ jointly, by the first 256 points of a 9-d Sobol sequence (unscrambled, Joe and Kuo's direction numbers) and the joint corners, under F7's four tilts (an aligned quarter of the Sobol points each, the corners cycling), with F2's read-back. They are fitted with the probes, required once the rig's probe case records, and corpus-independent: the design comes from the muscle cube, never from recorded animation. `--design-only` predicts them from each rig's single-muscle probes through Unity's muscle map. | The pose domain of 2.1's fit and 2.2's frame: full-range spine bends and twists, measured rather than extrapolated |
| F9 | Lateral band sweeps on the same three training rigs (never the hold-out): the range sweeps' Sobol points rescaled to Front-Back over [-1, 1], Left-Right over ±3/8 and Twist over ±1/32, under F7's four tilts, then the side bends under each tilt (every mapped spine bone alike: twisted at Left-Right 15/128 to 13/64 with Twist ±1/32, opening yaw gaps of either sign in the band's low bins, and pure at Left-Right ±3/8), with F2's read-back. 296 vectors per rig. They are evaluated but never fitted, all three required whenever the run has them (an Avatar Unity refuses is an Editor error), and corpus-independent. `--design-only` predicts them as F8's. | The spine lateral band past 2.2's validated lateral domain (section 8), where the frame's unexplained second-order yaw appears |

**F4: body-motion clips.** Each clip runs on identity and offset roots. It
records its serialized bake, keep-original, height-from-feet,
`m_StartAtOrigin`, level, orientation-offset, motion-slot and foot-goal
fields. Each sample records the projected root (`Animator.rootPosition` and
`rootRotation`) where the Editor exposes it, and always a `GetHumanPose`
read-back of the residual body pose. The cases:

- The eight keep-original combinations with bake off.
- The eight bake combinations with keep-original off.
- With all other settings neutral, each of these alone: height-from-feet,
  a level offset and an orientation offset.

Every clip has tilted `RootQ` with a yaw that varies non-monotonically, and
XZ and Y motion. The rules are told apart by these variants:

- the same clip with a nonzero starting XZ and yaw;
- clips whose motion curves (`MotionT`, `MotionQ`) differ from the body
  curves, so original root motion is distinct from the body projection,
  where the Editor accepts such curves;
- height-from-feet clips in which the feet move relative to the body (a knee
  bend), with and without foot goal curves.

## 5. If the study stays short

The slice stays deferred with the body-bearing refusal in any of these
cases:

- a coefficient stays free on a configuration that needs it;
- E2 shows an inexact solve that has no public description;
- E3b misses its bound.

The plan then records which recording failed and what would settle it. No
coefficient is shipped from a fit.

## 6. PRs

| # | PR | Contents | ~Non-test lines | Depends on |
|---|---|---|---|---|
| B0 | Plan | This document, the `anim.md` 2.6 link, and the "Humanoid local poses" tier X row's cells | docs | — |
| B1 | Recorder and fixtures | F1–F8 in the builder; read-back, root, mass and setting rows in the recorder; contract and `complete` checks; owner workflow notes; 2.5's tolerance derivation. **Adequacy checkpoint:** after the first owner run, a script computes 2.5's rank, conditioning and `u_s ≤ E_unity,s` with B1's own tolerances, and the topology coverage. If any is short, B1 extends the fixtures before B2 starts. The first runs left a few tall-rig samples short of `u_s ≤ E_unity,s`. `body-probes-tall-2` adds the extra tall-rig poses, and `--design-only` predicts their adequacy conservatively from the recorded geometry before the rerun. Under B2's frozen scope (fifteen training rigs, 29 coefficients), the conservative worst margin with the predicted `body-probes-tall-2` stays above 1 in both versions, and the checkpoint is adequate. F8's range sweeps are fitted with the probes: `--design-only` predicts them from each rig's single-muscle probes, and with them (eighteen training rigs, the three UpperChest-and-Toes rigs not yet recorded) the conservative worst margin over every planned and recorded sample stays above 1 in both versions. The checkpoint never reads the hold-out rig; E3b does, after the freeze. | 0.7–1.0k | B0 |
| B2 | Reference and body checker | 2.1–2.5 in the f64 reference with frozen bounds and the analytic cases. A checker runs E0–E4. **Input:** a completed run in both 2022.3 and 6000, read with Unity 6 reader support on main. **Order:** first E3a on the training rigs, then the freeze commit (model choices, coefficients, scope with the lateral limit E2 and E3a set, bounds, E4's passing configurations), then E3b on the hold-out rig against that commit, once. | 0.6–0.9k | B1 recordings |
| B3 | Rust body model | Read and validate `m_HumanBoneMass`; E0's runtime checks (3.2, E0 at runtime); the centre of mass, frame and closed-form hips solve; the free-combination refusal; synthetic tests and fuzz targets for the new reads | 0.5–0.7k | B2 (E3b passing) and E2 passing |
| B4 | Root projection and frames | Target body pose per verified 2.4 combination, conversion from the Animator to the hips parent with scale, narrowed refusals | 0.4–0.6k | B3, F4, F5 |
| B5a | Muscle pose in Rust | Port `anim_humanoid_qualify.muscle_rotation` to Rust: each animated muscle's angle from its value and its limits, then the bone's local rotation from its pre- and post-rotations and rest pose, for every muscle a clip animates. **(a) Verified today:** only the Spine, against P2b's known vectors on the compact and tall rigs in both Editor versions (residual at most about 1.6e-5 degrees). Every other bone's equation is a hypothesis: E3a validated the body model from the recorded node transforms, never through this equation, and Rust plays no muscle (every humanoid clip ends in `unsupported_humanoid_playback`). **(b) Acceptance:** bit-exact agreement with the Python reference on known vectors (the same f32 inputs, the same f32 rotation), and agreement with Unity's recorded local rotations within bounds derived as section 2.5 derives E3a's (Unity's own f32 error, measured, plus the evaluation error), on the training recordings in both versions: rounds 5 to 10's probes, extra probes and sweeps, and their clip playback (each sample's muscle values from the clip's own authored curves at its time). Rounds 1 to 10 are acceptance only; round 11 is confirmation only (e). The hold-out stays sealed. A muscle without a passing recording keeps its refusal. **(c) Recordings:** the body probes cover 45 of HumanTrait's 95 muscles. Round 11 sweeps all 95 with one schedule (each alone at both endpoints and four interior values per sign, then each bone's muscles jointly), and its full-body E5 clips put a curve on every muscle. **(d) Done on known vectors:** `humanoid/muscle_pose.rs` ports the equation; `tools/parity/anim_muscle_pose.py` is the reference with its arithmetic order written out (left-to-right sums, so CPython 3.11 and 3.12+ agree). Rust equals it bit for bit on the committed goldens (every muscle at every value, every bone jointly, synthetic axes) and on the private real-Avatar cases of both Editor versions; `tan` is the C library's, so the goldens record each tangent (glibc's, CI's) and pin everything after it. On the round 10 training recordings the Spine and every other bone with probed muscles agree within the derived bound, except four: LeftShoulder, LeftUpperArm, LeftEye and RightEye. Each disagreement is on an axis with `m_Sgn` −1 and unequal limits, where Unity turns by `m_Max` at value +1 and the hypothesis by −`m_Min`. Nothing is tuned to it: a change to the equation is a plan decision, measured against a hold-out. Until then those bones' muscles keep their refusal, and the gate table stays empty. **(e) Pre-registered correction (written before any code changed; implemented in (f)).** For muscle value `v` on axis `k` with sign `s = m_Sgn[k]` (exactly ±1), the hypothesis computes `d = s·v`, then `θ = d·m_Max[k]` if `d ≥ 0`, else `θ = d·(−m_Min[k])`. The correction picks the limit by the sign of `v`, then applies `s`: **`θ = s·(v·m_Max[k])` if `v ≥ 0`, else `θ = s·(v·(−m_Min[k]))`**. Everything after `θ` stays as it is: `tan(θ/2)`, the pre- and post-rotations, the product order and the norm. *Why:* a muscle's limits belong to the muscle, not to the bone's axis. Its range is `[m_Min, m_Max]`, with −1 at `m_Min` and +1 at `m_Max`, and left and right muscles share it. `m_Sgn` is the per-axis mirror that maps that muscle-space angle onto the bone's axis frame. So the limit is chosen by the value's sign before the mirror, not after. Applying `s` first sends a positive value to `m_Min` whenever `s = −1`. *Where it changes nothing (algebra):* (i) When `s = +1`, `d = v` exactly, so both formulas take the same branch and the same product: identical bits. (ii) When `s = −1` and `m_Max = −m_Min` exactly (bitwise), both formulas have the same magnitude and sign. For `v > 0`, the hypothesis gives `(−v)·(−m_Min) = (−v)·m_Max` and the correction gives `−(v·m_Max)`. IEEE round-to-nearest multiplication is symmetric under negation, so the bits are equal. For `v < 0`, the hypothesis gives `(−v)·m_Max` and the correction gives `−(v·m_Max)`, the same bits. For `v = ±0`, both take the `m_Max` branch, and the zero has the same sign. So `θ` can change only on an axis where `s = −1` and `m_Max ≠ −m_Min`. *Correction size:* there, for either sign of `v`, new minus old is **`−|v|·(m_Max + m_Min)`**, signed and scaling with `|v|`: at an endpoint −40° for LeftUpperArm, −15° for LeftShoulder and −5° for each eye (nominal limits), proportionally less inside. That is the correction to one axis's angle; the bone's local rotation error can differ from it when another swing axis of the bone is active. *Prediction from the Avatar data alone:* in the round 10 training Avatars of both Editor versions, the only probed axes with `s = −1` and unequal limits are the z axes of LeftShoulder (−15°/+30°), LeftUpperArm (−60°/+100°), LeftEye and RightEye (−10°/+15°). Every other probed axis has `s = +1` or symmetric limits, so no bone that agrees today can move. The predicted worst errors, 15°, 40°, 5° and 5°, are the measured ones, and in each Editor the failing rows are exactly those whose z value is nonzero (98 of LeftUpperArm's 161, 90 of LeftShoulder's 148, 8 of each eye's 12). *Equivalent formulations:* applying `s` first with the mirrored pair (`−m_Max`, `−m_Min`) on an `s = −1` axis is the same mapping at every value, so no recording, round 11's included, can tell the two apart; this row means either. Round 11 does tell the correction from the current hypothesis (`s` first with the stored pair): on LeftUpperArm's z axis at `v = +1, −1, +0.5, −0.5` the correction turns `−100°, +60°, −50°, +30°` and the hypothesis `−60°, +100°, −30°, +50°`; LeftShoulder's and the eyes' z axes likewise, and any asymmetric `s = −1` axis among the newly swept muscles (twists, fingers) tells the general rule from one limited to the four known axes. *Honesty about the evidence:* the pattern was found in round 10's recordings, so rounds 1–10 are the acceptance set, not independent evidence. *The split:* rounds 1–10 are acceptance only and round 11 is confirmation only; nothing from round 11 is used before this freeze, and none of it exists yet. Frozen now, before any round 11 data: the formula (above); the fixture revision (round 11's builder and tables as the round 11 run builds them, the fixtures change at `7e6273df` and its review fold, with every round 11 run declaring its round in `round.json`); the required cases (all four muscle sweeps and every full-body clip, in both Editors); and the per-sample bound rule (`anim_muscle_pose.unity_bound`: each sample's own f32 error propagation, plus the clip curve's own evaluation error for clip playback). *Confirmation passes only if* every sweep sample of every bone agrees within its bound in both Editors, and every full-body clip's recorded local rotations agree, sample by sample, with the corrected equation on the clip's own curve values and the same bundle's Avatar. *Acceptance:* on rounds 1–10, training runs only, every bone of both Editor versions agrees within its derived bound (worst around 3e-5°), clip playback included (the case reader compares each clip-playback capture too, its muscle values from the clip's own curves). *Regression:* every agreeing recorded input (each case-file row of a bone that agrees today) keeps its rotation bit for bit. The synthetic goldens, whose axes are independent of any Avatar, are expected to change: the synthetic Spine, for one, has asymmetric `s = −1` x and y axes. They are regenerated from the new reference, and the commit says so. *Implementation, after review:* a new reference version in `anim_muscle_pose.py` (`anim_humanoid_qualify` stays as P2b's evidence and is untouched), the Rust port, and a record of the change of reference. *No hold-out rig is used*. **(f) Implemented as reference 2** (`anim_muscle_pose.REFERENCE`; reference 1, the hypothesis, stays there as `hypothesis_angles` for the regression check) and in `muscle_pose.rs`, which equals it bit for bit on the regenerated goldens and on the round 10 training cases of both Editors, clip playback included. *Acceptance on rounds 1–10:* every bone of both Editors agrees within its bound, worst 3.3e-5° (LeftUpperArm 2.2e-5°, LeftShoulder 1.2e-5°, each eye 1.9e-5° at most), and no recorded row of a bone that agreed before changes; the changed rows are exactly LeftShoulder's 90, LeftUpperArm's 98 and each eye's 8 per Editor. *Refusal cases apart:* `humanoid-twist-refusal`'s playback (limb twist muscles) disagrees on the left arm and leg chains, from UpperArm to Hand and UpperLeg to Foot, by up to 47°, the same under both references and in both Editors: Unity spreads a limb twist over neighbouring bones, which one bone's equation does not model. Those rows are reported apart (`agree`'s refusal table), never acceptance, and limb twist keeps `unsupported_humanoid_twist` until a plan decision covers it. **(g) Confirmation's scope, amended 2026-11-20, before any round 11 recording** (the round 11 fixtures as merged at `80340d46`, which no Editor had then run). *Scope, exactly the runtime's admission, not by muscle name (amended again 2026-11-20, still before any round 11 recording):* B5a claims all 95 muscles, the ten limb-twist muscles included (Left and Right Arm, Forearm, Upper Leg, Lower Leg and Foot Twist In-Out), wherever the runtime plays them. A case's samples are out of scope if and only if some twist setting (`m_ArmTwist`, `m_ForeArmTwist`, `m_UpperLegTwist`, `m_LegTwist`) is not `humanoid::NEUTRAL_TWIST`, 0, and the case animates a bone that setting redistributes over (`TWIST_BONES`), which is exactly when `humanoid_refusal` refuses a clip as `unsupported_humanoid_twist`. A clip animates the bones of the muscles it curves; a known-vector case, those of the muscles some sample sets nonzero. Everything else is in scope: a Spine-only clip on an Avatar with a non-neutral twist setting, say. Every round 11 rig has all four settings at 0. (f)'s 47° came from `humanoid-twist-refusal`, whose Avatar sets them to 0.5, 0.25, 0.75 and 0.375, so Unity spread each twist over the neighbouring bones. *The rule does not loosen:* every in-scope sample of every sweep and full-body clip, limb twists included, passes within the same per-sample bound (`unity_bound`) in both Editors, or confirmation fails. *Twist samples are reported, not counted:* a sample the runtime refuses as `unsupported_humanoid_twist`, like a coverage refusal case, goes in the refusal-evidence table (`refusal_reason`), never counts toward pass or fail, and cannot stand in for a required case. If one agrees, that is noted and changes nothing at runtime. *Shared bones:* each case row compares one bone's recorded local rotation with that bone's own muscles under B5a's equation. So a muscle is judged only on the bone B5a's model says it drives (`muscle_bone`, fixed now), never on a neighbour it might move. *Confirmation is its own tool, amended 2026-11-20 before any round 11 recording, so that it cannot pass vacuously:* `anim_muscle_confirm.py` reads the round 11 run folders, never a case file, and `agree` is acceptance alone. It computes the expected set of (Editor, case, bone, sample) from round 11's builder tables. The cases are the four sweeps and every full-body clip. A case's bones are those its rig's Avatar maps, less the ones its topology omits. A sample's id is a sweep vector's index in the builder's schedule, or a clip sample's time on the recorder's schedule. The counts are therefore the builder's, never copied: today 535, 503, 423 and 69 for the sweeps, and 9 for each full-body clip. The observed set comes from the raw captures. Each sample is identified the same way, from the source time its prediction and bound use, and a sample recorded twice or never scheduled is refused. Each capture must be its case's prefab, clip and Avatar assets and playback mode, recorded at its case's Animator placement. Each recorded bone's transform must resolve through the Avatar's skeleton and `m_TOS` paths to that bone, at that node's `m_SkeletonPose` rest rotation. A clip's capture must play the builder's clip: its authored and compiled muscle curves within f32 rounding of the builder's polylines (every dense frame included), and its variant's root settings. Its compiled index array must hold the runtime's full DOF layout, every entry other than −1 owned by its own DOF's binding. Each clip sample's muscle values and their curve error come from Unity's compiled curves, and each bound is recomputed by the frozen rule. *What it guards against:* an incomplete, buggy or misrouted collection by the recorder or the collector, not someone hand-editing capture files. Integrity against later edits is to come from a sha256 manifest of the captures that the Editor writes at record time. Scope is decided from the runtime's own inputs: the built clip's compiled muscle slots, and the Avatar's mappings, axes and twist settings. It passes only if, in both the 2022.3 and 6000.6 series, the observed in-scope set equals the expected set exactly, with nothing missing and nothing extra, and every sample is within its bound. Each capture's own metadata, never its file name, names its case, run, Editor version and bundle (`anim_muscle_round11`). **(h) Limb twist, 2026-11-20, from round 11's recordings (URS-42).** (g)'s premise fails: on round 11's rigs, all four twist settings 0, Unity turns no upper leg or upper arm by its own twist muscle, and both muscle sweeps stop at the first limb twist (Left Upper Leg Twist In-Out). The full-body clips' recorded poses, read as bone motion alone, show each upper limb bone exactly as its other muscles place it, and the lower leg, forearm, foot and hand turned beyond theirs: Unity carries each limb twist down the limb, which one bone's equation does not model. So the runtime refuses every limb-twist muscle curve, all ten, at any twist setting, as `unsupported_humanoid_limb_twist(muscle)` (`humanoid::LIMB_TWIST_MUSCLES`): a built clip's mapped muscle slot, and an explicit clip's Animator float curve on the muscle's name (`LIMB_TWIST_PROPERTIES`), which is never dropped with the metadata. The neutral setting admits none of them; B5a claims the other 85 muscles. The round 11 completion change on the `r11-completion` branch (`anim_fixture_round11.EXPECTED_FAILURES`) accepts the two sweeps' failure, with the recorder's exact message and each sweep's bundle present, and lists them as not covered, never as passing. Confirmation is not run on the cases that remain: the pre-registration covers the whole set, and `anim_muscle_confirm.py` stays as it is until round 12 restores the body sweeps, with twist distribution fitted on its own exploration data and confirmed apart | 0.3–0.5k | B4, round 11 recordings |
| B5b | Integration and L11 | Collector integration, the gated same-bundle replay (E5), the release gate's verified table (`humanoid/gate.rs`, empty until E5) filled from E5, the body-bearing refusal lifted for the table's classes alone, and the PARITY row promoted. **Checklist:** before the refusal lifts, E5 covers every class in "B4's admitted scope" (b), exactly as (b) defines it. The table admits only recorded classes: a combination B4's predicate admits but E5 did not record (a scaled Animator on `no-toes`, say) keeps its refusal. **Known limitation, not a change now:** UnityCsReference documents 0.5 as the default of each of the four `HumanDescription` twist settings (`upperArmTwist`, `lowerArmTwist`, `upperLegTwist`, `lowerLegTwist`). A default-initialized managed `HumanDescription` has them at 0, and the model importer that sets up an Avatar is native, so what Editor-created Avatars actually get is not verified; round 11's owner steps ask for one such Avatar's four values. If they are 0.5, clips that animate those Avatars' limbs stay refused (`unsupported_humanoid_twist`). Admitting twist distribution is future work, and round 10's `humanoid-twist-refusal` recordings are its starting evidence | 0.3–0.5k | B5a |

**B4's admitted scope.** B4 admits only what freeze v8's evidence
validated, as checkable settings, ranges and structure, never a rig's
identity; `humanoid_refusal` stays global until B5's predicate and E5.

- **Settings.** The clip's settings are one of `e4.passing`'s eleven
  variants, field by field: the six bake and keep-original flags, and
  `m_Level` and `m_OrientationOffsetY` to the bit. Its motion slots are
  all bound for the all-Original variant and none for the others. It has
  no additive reference pose, and E4's exact slot binding key.
- **Chain.** The plain chain admits every variant, at the identity or at
  E4's offset placement. One F5 feature alone admits only the
  all-Original variant, at an exactly identity placement: the Animator
  node at scale 1.25, one armature node at its fixture transform, or the
  hips at scale 1.2. Every ancestor of the Animator node is exactly at the
  identity.
- **Ranges.** `m_Scale` lies in [0.78115433, 1.4346639], the range over
  E3a's and E4's training rigs (the hold-out stays sealed). Every rest
  scale below the hips is exactly 1, as on every validated rig.

What a recording verified and what B4 admits by argument:

- **(a) Verified by a recording** (`e4.passing`'s 52 tuples, both Editor
  versions): the compact rig with each of the eleven variants, at the
  identity and at the offset placement; and F5's four rigs
  (`scaled-animator`, `armature`, `hips-scale`, `large`), each with the
  all-Original motion variant at the identity.
- **(b) Admitted by argument:** any admitted rig other than the compact
  rig: every training topology B3's topology scope admits, each as the
  plain chain, and the plain chain at any human scale in the range,
  each with any variant at either placement (the `large` rig's one cell
  in (a) aside); and each F5 feature at a human scale in the range other
  than its rig's. The eyes, jaw, eyes-jaw and noncoplanar rigs share the
  compact rig's human scale but are other rigs, so they are in (b). The
  argument is h-linearity and E3a's per-topology solve: for fixed
  serialized inputs, `m_Scale` only multiplies the residual translation
  after projection, the cell choice and the rotation do not depend on
  it, and the projection's position error scales with it; E3a validated
  the solve on every training topology.

Unity's read-back for (b) is not verified. B5b's E5 same-bundle replay
must cover every class in (b) before the global refusal lifts; B5b's
predicate and E5 are the release gate. Round 11 records (b)'s classes:
the eleven variants at both placements on the large, tall and `small`
(0.82) rigs and on every topology B3 admits but the compact one, and each
F5 feature at a second human scale. The verified table admits only the
classes E5 recorded and passed, each a topology, a chain feature, a
human-scale interval its recordings span, a variant and a placement, so
the predicate's wider runtime scope (a scaled Animator on `no-toes`, say)
stays refused.

**Round 11's steps.** Three separate steps, in this order, each on both
Editor versions' runs:

1. **Collection** (`anim_fixture_round11.py complete`, one Editor folder at
   a time). It checks that the run recorded what the builder authored and
   B4 reads: the whole body inventory recorded, B4's inputs on every case B4
   reads, each clip's declared scope on its compiled values and recorded
   poses, each full-body clip's curves and each sweep's response. It
   compares nothing with Rust, and a passing collection confirms neither
   B5a nor B5b.
2. **B5a's confirmation** (`anim_muscle_pose.py`, on the completed round 11
   runs of both Editors): the muscle sweeps and full-body clips against the
   frozen muscle-pose equation and its per-sample bound, every sample in
   both Editors.
3. **B5b's comparison** (E5): Rust's full pose for each in-scope clip
   against the same bundle's recording in each Editor, and each refusal-
   evidence clip's typed refusal, in both Editors. Only this step can fill
   the release gate's verified table.

**B5's release gate.**

- **Predicate.** A runtime configuration predicate decides admission. It
  checks the Avatar topology, the Animator-to-hips chain (uniform scale,
  TRS), the clip's serialized settings and 2.4 combination, the
  body-slot presence, the animated muscle set (no twist or stretch) and
  the Editor version family. It admits a configuration only if it is in the
  recorded, passing set, and every other configuration keeps its typed
  refusal.
- **L11 matrix.** Every admitted configuration has a complete, passing
  same-bundle L11 matrix in both versions. A missing, skipped or partial
  replay blocks, as in P2b's `complete`.
- **Freeze before the hold-out.** B2's freeze commit, made before E3b runs,
  fixes the model choices, coefficients, scope and bounds that B3–B5 use.
  The freeze needs E3a passed on every required case of both versions, E4
  passed, full rank and every coefficient within `|a| ≤ 1`. It records E4
  per configuration (variant, rig, Editor version, root placement, each F5
  chain its own). B4 admits the passing configurations it names and,
  by argument, the classes "B4's admitted scope" (b) lists; B5 lifts the
  refusal only for configurations it names or E5 verifies. A
  configuration with no comparison is listed as excluded (or as refused,
  when the topology scope refused all its samples), never passing, and each Editor family the code requires (the same constant
  E3b uses) needs a comparison of its own, whatever runs are given. It records the code's git commit and a digest of its
  sources: the checker's entry points, every local module they import,
  transitively (recording validation, case tables and Avatar decoding
  among them), and the data files they name (the recording schema, the
  bounds), all committed and unchanged; and the sha256 of the unityrs CLI
  that read the Avatars and built clips, resolved once to an absolute real
  path that is both hashed and run. E3b takes only the canonical artifact
  path, a constant of the sources, with no symlink at it or its parents
  inside this code's checkout; it reads the artifact's bytes once, hashes
  and parses those same bytes, and refuses unless the code
  commit is an ancestor of HEAD and of the freeze commit, no source
  changed since it, the artifact is byte-identical to the one the single
  commit that added that path holds, the CLI is the frozen one, and the
  Editor versions, hold-out cases and minimum it records are the code's
  constants. `complete` seals the hold-out's cases: it opens neither their
  recordings nor their bundles, and every single-file reader refuses a
  hold-out or pending file by name before opening it. Only E3b unseals them, through `complete`
  itself (run identity, topology, probe schedule and bundle checks), after
  reproducing the frozen fit and lateral limit from the frozen training
  runs and checking, by file names alone, that each version recorded every
  hold-out case. It then claims its one shot: an exclusive create of a
  marker named by the freeze's digest, in a fixed directory under the
  account's home (from its passwd entry, never `$HOME`), refused if it
  exists, and a ledger row holding each run's manifest and hold-out
  recordings by sha256. From the claim on, any failure is a recorded
  failed attempt and the shot stays consumed. Sealed completion itself
  refuses to open a hold-out file unless that claim exists, and E3b
  evaluates only the documents whose bytes match the claimed digests, so a
  run swapped underneath fails closed. Each version is evaluated alone: it needs every hold-out case, at least 30
  in-scope comparisons (the hold-out schedule's 45 known vectors leave 33
  inside the lateral and frame scopes by design; 30 allows three more
  frame refusals for its longer arms), and a pass. Nothing pools across
  versions.
  The freeze binds one completed run per version, the first with the band
  sweeps, and E3b reads that run's own hold-out recording: until E3b only
  the sealed tools touched it, and nothing opened it. An earlier run's
  hold-out stays sealed and unused. The lateral limit was chosen from the
  training rigs alone. An earlier run of the same training cases can be
  recorded with the freeze as supporting evidence (`--evidence`: E2 and E3
  within the limit with the frozen design and coefficients), never as an
  input.
  These guards protect against accidental re-runs, stale inputs and honest
  mistakes, not against a deliberately adversarial local user, who could
  edit the code anyway.
- **No pass on nothing.** E0, E2, E3 and E4 fail with no comparison, and a
  bound that bounds nothing (an unknown design column, an infinite `u_s`, a
  turn of 2 or more) fails every comparison against it.
- **E3b on freeze v7: FAIL.** E3b ran once, on round 9's hold-out, and
  failed in both Editor versions on one check alone. At one hold-out
  sample (local 0.651947021484375), `u_s` 3.521e-05 exceeds `E_unity,s`
  3.512e-05, by 0.26%: the adequacy criterion of section 7, not a
  residual. Every in-scope residual was within 0.3% of its `T_s`, with 33
  in-scope comparisons per version (minimum 30) and E0 passing. Freeze
  v7 is not validated, so B3 stays blocked. Round 9's hold-out is
  consumed, and E3b is not re-run on it. A later fix to the source scan
  (file-name constants only, for Python 3.11) changes v7's source digest;
  v7 is spent, so nothing rebinds it. Round 8's sealed hold-out
  records the same poses, so it is no fresh test either. The criterion
  depends only on the frozen design and the hold-out's planned geometry
  and poses, so it was predictable before the freeze. The recovery:
  - the freeze and E3a now require it, with margin, at every planned
    hold-out sample, predicted from the fixture spec alone: the base
    rig's recorded rotations on the hold-out rig's layout, the hips
    placed by the model, and `E_unity,s / u_s` of at least 1.05 in each
    Editor version, checked again before E3b claims its shot. It counts
    only what E3b would compare (frame and topology scope, the frozen
    lateral limit) and needs E3b's minimum of in-scope comparisons. On
    round 9's training runs this prediction flags the failing sample.
    Against round 9's consumed hold-out, recomputed at all its samples,
    it predicts the same 33 in-scope comparisons per version (the 4 more
    samples it puts out of frame E3b refuses by the lateral limit), and
    `E_unity,s / u_s` within 0.26% at every one (worst 0.253%, `u_s`
    overpredicted; `E_unity,s` within 0.021%), the same in both
    versions. The 1.05 margin is about twenty times that error;
  - five extra probe cases tighten the coefficient combinations that
    drive it (the shoulder links' sum and difference, and the lower
    legs'). The criterion is block-l1, so repeating rows of a given
    leverage cannot lower `u_s` past what that leverage allows: added
    poses lift the worst planned margin only modestly, which is why the
    required margin is 1.05 and not larger;
  - a new hold-out rig, `holdout-2` (arms 0.8, legs 1.2), with 45 fresh
    pose draws from a new Sobol seed, is recorded in the next run. Both
    earlier hold-out rigs stay sealed and are never fitted.
- **E3b on freeze v8: PASS.** E3b ran once, on round 10's `holdout-2`,
  and passed in both Editor versions. Each had 46 in-scope comparisons
  (minimum 44) and 2 refused; the worst residual was 0.0213 of its
  `T_s`, and E0 passed. The worst actual `E_unity,s / u_s` was 1.1430,
  against 1.1434 predicted at the freeze, at the same sample; the
  prediction was within 0.168% at every in-scope sample. Round 10's
  hold-out is consumed. Freeze v8 is validated, and B3 is unblocked.
- **Freeze v8's arithmetic.** v8 was fitted and validated (E3a, the
  freeze and E3b) under CPython 3.15.2, whose builtin float `sum` is
  compensated (Neumaier, from 3.12 on); the Rust model mirrors that
  summation wherever the reference uses `sum`, through `unityrs-pysum` (a
  PSF-2.0 port of that float path), and its reference vectors
  are regenerated and checked under 3.15.2. A successor freeze must
  specify its summation explicitly (left to right, or `math.fsum`) so that
  its results do not depend on the interpreter.

**Total estimate.** 2.6–3.8k non-test lines. B1 can start now. B2 waits for
the owner's runs of B1 in both Editor versions, and B3–B5 wait for B2's
frozen constants.

## 7. Risks and gates

- **Fitted constants passing as verified.** A least-squares fit matches the
  current recordings and still misses another rig by centimetres. Gates:
  2.5's probes and conditioning, E3b on the hold-out rig, and decision 2.
- **Unpublished solve.** If Unity's hips solve iterates, the closed form
  can differ from it at second order. Gate: E2 before B3.
- **Asymmetry.** A symmetric shortcut breaks the rest body position. Gate:
  E0c on every Avatar.
- **Version drift.** The model must hold in both 2022.3 and 6000, so each
  coefficient and bound is checked per version.
- **Cost.** One forward-kinematics pass and one weighted sum per sample, on
  the existing metered path, with no new allocation per sample.

## 8. What stays refused

- **P2b refusals.** Mirror, cycle offset, loop-time blending, additive
  reference poses, IK goals, translation DOFs, twist redistribution and
  stretch keep their P2b refusals.
- **Unrecorded settings.** Root-basis, bake and clip-setting combinations
  with no passing F4 recording stay `unsupported_humanoid_root_basis` or
  `unsupported_humanoid_clip_settings`. So do `m_StartAtOrigin` off and any
  original-root or lowest-foot rule F4 leaves undecided.
- **Frozen model scope (B2, training rigs only).** An Avatar with Neck
  absent, Chest absent or exactly one Shoulder mapped refuses at the
  Avatar check (2.1), as does any pattern of absent body slots no training
  rig recorded (the Eyes and Jaw aside) and any nonzero Eye or Jaw mass. A pose whose shoulder-hip twist exceeds 4.25°, or is
  undefined, refuses in 2.2 and therefore in the hips solve. The hold-out rig's topology, read
  from the builder's table and not from its recordings, is inside the
  scope.
- **Spine lateral range (B2, a scope rule).** A pose whose largest spine
  Left-Right muscle (Spine, Chest or UpperChest) exceeds the frozen
  lateral limit refuses as
  `unsupported_humanoid_body_model(spine_lateral_range)`, in 2.2 and
  therefore in the hips solve. The checker sets the limit after a run
  (2.2), and E3b applies the frozen one. The reason: past it, 2.2's frame
  leaves Unity by an unexplained second-order yaw of up to 0.08°.
- **Unsupported rigs.** These refuse: topologies outside F6, an Avatar
  whose geometry depends on a free coefficient combination, a terminal
  bone whose probe moved the hips, non-uniform scale, and a non-TRS link
  between the Animator and the human bones.
- **Root trajectory.** The accumulated root delta that `applyRootMotion`
  would apply is not exported. That is a later slice.
- **Controllers.** Controller state and blend-tree evaluation stays in its
  own later slice.
