# P2b plan: animation

**Status.** Draft, 2026-11-17, with both review rounds folded into the decisions
and eight PRs in section 6. Generic export and batch have synthetic integration
gates. PR 2's [reference equations, bounds, replay format and recorder](../anim_reference.md)
are in the tree. Serialized playback and humanoid L11 depend on Editor
recordings and Rust comparisons.

**Deliverables.** Discover clips by PPtr and bind them to a prefab or scene.
Sample generic and legacy TRS and morph weights, and provide humanoid retargeting
for qualified clips, with typed refusals for the rest. The library and
`unityrs gltf` write editable animated GLBs.
Batch spec 3 writes one loose GLB per selected prefab. Section 2.6 names the
deferred slices.

**Starting point.** Core supplies checked dict reads, embedded and provider
typetrees, file-scoped PPtr resolution and meters. P2a supplies the scene graph
and units, mesh, skin and morph data, and the editable glTF model, collector and
streaming writer. Batch already has sealed attempts, private dependency views,
output-backed units and spec 2 (`docs/plans/rest.md` decision 3).

## 0. Decisions

| # | Decision | Reason |
|---|---|---|
| 1 | Keep Tier A at UnityPy's existing read surfaces. File the planned animation rows in `PARITY.md` section 24 as tier X, with discovery identity and order checked separately from poses. | UnityPy exposes serialized animation data. |
| 2 | Put animation reading, binding and sampling in `unityrs-anim`, on core and scene. The glTF collector supplies mesh channel facts and the caller supplies the animation callback (2.1, 2.5). | P2a decision 3 and DESIGN section 4 keep animation and glTF independent of each other. |
| 3 | Give each target property one playback source under 2.2. Keep `m_EditorCurves` and `m_EulerEditorCurves` as authoring metadata only. | Compiled, explicit and authoring representations can coexist. |
| 4 | Discover clips in stored order per Animation or Animator component and select a clip directly by its rig-scoped identity. Read controller tables for discovery and direct selection only. | A shared clip can bind to several different rigs. |
| 5 | Bind paths relative to the owning component. Diagnose duplicate paths, CRC collisions and ambiguous targets (2.3). | A guess can move the wrong Transform or renderer. |
| 6 | Use same-bundle Editor playback for L11. Land the recorder and the f64 transcription check before the Rust playback and retargeting code they check (4.2). | Two evaluators transcribed from the same equations can share a mistake. |
| 7 | Emit held `STEP` or bounded `LINEAR` channels for TRS and morph weights. Keep serialized non-glTF properties and inert events as metadata (2.5–2.6). | Each glTF target has one channel per animation. |
| 8 | Keep P2a's last-frame morph policy and four-influence skinning. Amend static3d decision 9 by assigning variable bone weights to the later slice in 2.6. | Layout and deformation still need recorded evidence. |
| 9 | Set the P2b batch scope for static3d decision 11. P2b adds GLB-only prefab export in spec 3 with manifest 3. The GLB's output key is its unit key (2.7). | E32 uses output-backed unit keys and the output's byte MD5. |
| 10 | Revise DESIGN section 5's P2b estimate to 7.3–10.0k non-test lines (section 6). Defer controller state/blend-tree evaluation, variable bone weights and sampled non-glTF properties (2.6). | The remaining work includes versioned playback, generic and humanoid collection, reference tooling and sealed GLB publication. |

## 1. Where it starts

- `unityrs-scene` already exposes `Unit::Prefab`, `Unit::Scene`, container
  selection, the ordered graph and `m_animation`/`m_animator` accessors.
  Graph nodes preserve stored names, file-scoped refs, local TRS and components.
- `unityrs-mesh` already supplies bind poses, joint palettes and `BlendShapes`
  with channel names, hashes, frame ranges and `fullWeights`.
- `unityrs-gltf` already maps graph nodes to scene nodes and can attach a skin
  to a separate identity-root node. Profile v1 currently refuses nonempty
  `clip_states`. The scene has no animation tables today.
- Batch spec 2 has the P3a image options and its own versioned schema. It has
  no prefab dispatch or 3D output kinds. There are no checked-in animation
  recordings for L11.

## 2. Crates, modules and APIs

### 2.1 Layout

| Crate | Dependencies on the new path | Responsibility |
|---|---|---|
| `unityrs-anim` (new) | core, scene | Clip reads, discovery, binding, Avatar reads and f64 sampling. Accept renderer channel facts keyed by scene component identity. |
| `unityrs-scene`, `unityrs-mesh` | P2a | Graph, units and source mesh data. |
| `unityrs-gltf` | P2a | The editable animation model, collector maps, morph channel facts and GLB writer. Define the collector callback over scene identities and collector-owned facts. |
| `unityrs-cli`, `unityrs-batch` and library callers | scene, glTF, anim on their animation paths | Caller-side glue adapts the channel table to anim's binding input and maps the returned samples to glTF's model. Reuse this path in standalone and sealed export. |
| `unityrs-testkit` | Existing fixture dependencies | Synthetic clips, controllers, Avatars and prefab units. |

Use the same checked allocation, work, Send + Sync and no-panic rules as P2a.
Follow `CLAUDE.md` for source and notices on any MIT port. The pinned AssetStudio
reader is format evidence. [AssetStudio AnimationClip v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/AnimationClip.cs)

### 2.2 Clip readers and playback

**Read.** `read_clip` takes an `ObjectRef` and meter. Retain timing, serialized
bindings, keys, events and issues. Read the explicit runtime position, rotation,
Euler, scale, float and `m_PPtrCurves` fields separately from the two authoring
arrays. Retain the serialized payloads and type information of non-glTF
properties for later use. Events are inert records.

**Playback source.** Per resolved target property, a populated, supported
compiled binding in `m_MuscleClip.m_Clip` owns the property on a runtime clip.
A malformed or unsupported compiled binding that claims a playback property
refuses it with the original cause. Valid metadata-only bindings follow 2.3.
On explicit/legacy layouts, use the runtime fields.
For rotation on such a layout, use `m_CompressedRotationCurves` when the
fixture-confirmed compression flag (`m_Compressed` where present) selects it
and supplies a channel for that path. Otherwise prefer explicit
`m_RotationCurves` to explicit `m_EulerCurves`. Accept supplemental runtime
properties alongside compiled data only where same-bundle fixtures demonstrate
that the property is genuinely unclaimed by the compiled table and played from
the explicit field. Treat the entire rotation as one property, even when
one source is Euler and another is quaternion. An ambiguous owner or a
malformed chosen source gives a typed refusal. Record the chosen field.

**Explicit interpolation and rotation.**

- Evaluate ordinary continuous values with time-scaled Hermite tangents
  (`outSlope` of the left key and `inSlope` of the right). Evaluate supported
  weighted tangents on the time/value Bézier segment using the time solve
  in 4.2. Keep held segments discrete. Require fixtures for versioned weighted
  or held encodings.
- For supported explicit quaternion curves, evaluate XYZW key components and
  their component derivatives in the same representation, then normalize the
  evaluated vector. Tangents are derivatives per second, with values and
  tangents kept in the serialized hemisphere for source evaluation. A
  fixture-backed antipodal rule, if needed, flips the key and both tangents
  together. Normalization and sign continuity of exported keys follow
  evaluation. Refuse invalid quaternions or an unconfirmed quaternion tangent
  mode.
- Evaluate explicit or compiled Euler components in degrees, preserving their
  authored revolutions until conversion. For the layout matching Unity's
  rotation-order enum, confirm `m_RotationOrder` values 0–5 as XYZ, XZY, YZX,
  YXZ, ZXY and ZYX in application order. For a layout without a stored order,
  use Z-X-Y only where the default is confirmed. Refuse an unknown order.
  Keep full-turn and off-key fixtures with both the runtime and authoring
  curves present. [Unity Quaternion.Euler](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Quaternion.Euler.html) · [Unity rotation-order enum](https://github.com/Unity-Technologies/UnityCsReference/blob/9778aaee88c7de7656c15184ba5c91ca66db7e94/Runtime/Transform/ScriptBindings/Transform.bindings.cs)
- For packed legacy rotations, unpack `m_Times` integer deltas, accumulate in a
  checked integer from zero in source time, and convert centiseconds to
  seconds. Read packed XYZW quaternions in `m_Values` and the packed float
  vector in `m_Slopes` separately. Only a
  fixture-confirmed slope layout may supply component in/out derivatives. Pin
  counts, key association, units and scaling against off-key playback and
  convert derivatives to per second before Hermite evaluation if that encoding
  is confirmed. Until then, refuse packed playback as
  `unsupported_packed_rotation_slope`, including when slope data is absent
  and its absence has no confirmed meaning. AssetStudio's exporter documents
  the time accumulation and leaves slope playback unverified. [AssetStudio ModelConverter v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudioUtility/ModelConverter.cs#L747-L767)

**Compiled channels.** Use streamed, dense and constant scalar slot ranges in
that order. Build the slot table before interpreting values (2.3).

- Decode the streamed words as typed frame times, key counts, slot indices
  and coefficient payloads. In the known sentinel layout, the initial
  sentinel may seed a held PPtr at the clip start, with each index validated
  against the mapping table. Preserve the seed until its first real key.
  Before the first real continuous key, clamp to that slot's first key value.
  A slot with neither a real key nor a supported initializer refuses playback.
  Distinguish a fixture-confirmed terminal dummy (such as a positive-infinity
  sentinel) from a real final key, check the dummy's shape, and exclude it from
  evaluation and duration. Evaluate the last key at or before the sample using
  its cubic at time since the key up to the next key. Hold the last real key
  afterward. Pin pre-roll, held boundaries and both sentinel rules against
  fixtures. [AssetRipper AnimationClipConverter 1.3.0, format reading only](https://github.com/AssetRipper/AssetRipper/blob/1.3.0/Source/AssetRipper.Processing/AnimationClips/AnimationClipConverter.cs)
- Dense samples are frame-major at `m_BeginTime + frame / m_SampleRate`.
  Check the frame × curve product, finite begin time and positive finite rate.
  Interpolate continuous scalar slots linearly between adjacent frames and
  clamp to the first or last sample outside the grid. A one-frame dense slot
  holds its single sample. Discrete slots hold the value of the preceding
  frame, with the first frame used before the grid. Evaluate a supported
  quaternion from its four components and normalize the result.
- Constant slots hold one value throughout the runtime interval, including for
  constant-only clips. For a Transform binding crossing storage ranges, allow
  slot-by-slot sampling only for a fixture-confirmed layout. Otherwise refuse
  the binding as `unsupported_mixed_transform_storage`.

**Time and result.** For compiled clips take finite, ordered `m_StartTime`
and `m_StopTime` in source seconds from `m_MuscleClip`, including for
constant-only clips. Dense grid bounds and streamed sentinels have separate
roles. For explicit-only layouts use the bounds defined by the layout or, when
no interval is stored, the first and last real runtime key times. A
metadata-only clip has no playable tracks. `sample_clip` takes a bound clip,
rest pose and an explicit schedule of source times within the closed interval.
Use the clip-level serialized `m_SampleRate` for the default schedule and include
both endpoints once. A zero-duration clip gets one sample. Refuse a non-finite
or non-positive serialized rate even when a schedule or rate override is
supplied. Refuse invalid intervals or schedules. Work in source seconds until
output, where the clock is `source_time - start`, also for nonzero starts. Fill
unanimated components from rest. Return f64 local TRS, resolved morph scalars
and the typed issues and source metadata. Export one pass, retaining wrap
metadata.

### 2.3 Discovery and binding

**Discovery.** Traverse the unit graph in stored order and each node's
components in component order. `Animation` contributes `m_Animation` then
`m_Animations`. `Animator` contributes the clip slots in its
`RuntimeAnimatorController`'s `m_AnimationClips`. Read compiled controller
tables for clip discovery and direct clip selection only, resolving supported
leaf indices to clip slots by layout and retaining table metadata. Follow each
PPtr from its owning file. Bound and cycle-check override chains. Apply
`AnimatorOverrideController` substitutions by resolved original identity,
innermost first, retaining the base slot's order. A missing or null override
retains the incoming clip. A non-null broken override records its typed error
and leaves that slot unresolved. Retain source-slot identity (owning object,
field and ordinal) with its original PPtr or override error, including on
outside-view slots. Deduplicate resolved clips by component/root identity and
resolved clip file identity plus signed path ID, keeping the first surviving
occurrence. Keep the same clip on different roots or components, even when
their names match.

**Paths and renderer targets.** Resolve explicit paths against case-sensitive,
slash-separated stored names relative to the Animation or Animator node.
The empty path is that node and `CRC32("") = 0` (IEEE). Check hashes of raw
relative path bytes against recorded bindings, including non-ASCII. Preserve
all collision candidates, including a nonempty path hashing to zero. Use
Avatar and controller TOS strings as evidence in their own domains and check
them against the graph. For explicit `blendShape.<name>` and supported hashes,
use the table of resolved mesh channels supplied by the collector for the
bound renderer. Ambiguous names or hashes, targets removed by filters and
missing attachments give typed issues.

**Versioned binding records.** Read
`m_ClipBindingConstant.genericBindings` and `pptrCurveMapping` from the
clip's serialized layout. Keep path, attribute, script PPtr, class and
`customType` together with the value flags.

| Layout evidence | Interpretation |
|---|---|
| Legacy value array | Use a fixture-confirmed adapter to the same typed slots. Unmapped layouts refuse playback. |
| GenericBinding before 5.6 where present, and 5.6 onward | Read the version's class field (`classID` or `typeID` as named by the tree), unsigned 16-bit before 5.6 and signed 32-bit from 5.6 in the documented reader layouts. `customType` is the layout's byte discriminator, including ordinary 0. `isPPtrCurve` is the object-reference flag. |
| Layouts with discrete integers | `isIntCurve` is a byte in the reader's 2022.1-and-later layouts. Verified older layouts lacking the field default it to false. |
| Layouts with managed reference flags | Read `isSerializeReferenceCurve` where the versioned tree defines it as the managed-reference flag. Default false only in a confirmed older layout without the field. Record the flag's presence, width and alignment in the fixture/version matrix before accepting a layout. Retain these bindings as metadata. |

Contradictory flags or a layout outside the verified table give a typed refusal.
For an ordinary supported Transform, attributes 1–4 consume respectively 3
translation, 4 quaternion, 3 scale and 3 Euler slots. Known scalar families,
including confirmed custom classes, consume their recorded width (one for
scalar morph weights, integer and PPtr slots). Preserve the recorded width
even when the property is unsupported, so subsequent bindings keep their slot
indices. Refuse an unknown width as `unknown_binding_width` before assigning
any following slots. Require the summed widths to match the storage counts.
Decode continuous values as finite floats. For a PPtr slot, validate the
layout's numeric index as finite, integral and in bounds, handling a sentinel
only where recorded, then take the PPtr from the clip-owned table. For a
bit-encoded discrete integer slot, reinterpret the word as the layout's
integer bits before float checks. It is neither a mapping index nor a floating
point sample. Discrete validation preserves the initial PPtr and later changes.
Keep valid material, integer and PPtr bindings as metadata only. They consume
their serialized slots and never count as refused export channels or cause a
clip failure. Sampling and resolving those properties for a consumer belongs
to 2.6.
[AssetStudio GenericBinding v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/AnimationClip.cs#L737-L824) · [Unity GenericBinding value kinds](https://docs.unity.com/en-us/engine/6000.0/script-reference/unityengine/animations/genericbinding)

### 2.4 Avatar and humanoid

Use the target `Avatar.m_Avatar` runtime data. Read `m_AvatarSkeleton`,
`m_AvatarSkeletonPose`, `m_DefaultPose`, skeleton name IDs, `m_Human`,
human-to-skeleton and reverse index arrays, and the root-motion data. Resolve
skeleton identities through `m_TOS` and the materialized graph. Validate
parent and axis indices and read the referenced axis records (pre/post
quaternions, signs, limits and axis type). Keep `HumanDescription` as
authoring/import evidence. The runtime tables control binding. Read the
versioned `m_IndexArray` mapping from humanoid DOF to `m_Clip` slot,
including its absent sentinel, and check it against the recorded DOF and
`HumanTrait` muscle-to-bone/axis order. Distinguish body, muscle, goal and
translation slots before binding. Refuse missing required bones, conflicting
mappings, unknown DOFs or an optimized hierarchy that cannot be reconstructed.
[AssetStudio Avatar v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/Avatar.cs) · [Unity HumanDescription](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanDescription.html) · [Unity HumanTrait](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanTrait.html)

| Feature | P2b behaviour and evidence |
|---|---|
| Muscle rotations | Use the documented normalized muscle range relative to neutral and axis limits. Apply the recorded runtime signs, reference pose and pre/post axis transforms in the order validated by single-muscle and noncommuting multi-axis recordings. Leave untouched components at rest. A missing axis convention gives `unsupported_humanoid_axes`. A conflicting generic writer gives a typed binding error. [Mecanim Humanoids](https://unity.com/blog/engine-platform/mecanim-humanoids) |
| Body position and rotation | `RootT` and `RootQ` describe the approximate centre-of-mass body pose in the Animator root frame. Position is divided by human scale. Recovering hips-local TRS requires the pose-dependent humanoid mass/body-orientation model and the clip's root-motion projection. Apply the body-bearing test below and refuse those clips as `unsupported_body_motion`. [Unity internal human pose](https://docs.unity.cn/2023.1/Documentation/ScriptReference/HumanPoseHandler.SetInternalHumanPose.html) · [Unity root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html) |
| Nonidentity Animator root | For any future supported body configuration, denormalize body position by the target Avatar's human scale, express the reconstructed hips in the Animator root frame, compose with its world transform, then convert through the inverse of the actual hips parent's world transform. Refuse that body case until this conversion passes a translated and rotated root fixture as well as an identity case. For muscle-only clips, accept a nonidentity root only on a recorded matching layout/configuration, otherwise return `unsupported_animator_root`. Singular or non-TRS conversions refuse explicitly. |
| Twist distribution | Read the runtime `m_Human` upper/lower arm and leg twist settings (`m_ArmTwist`, `m_ForeArmTwist`, `m_UpperLegTwist`, `m_LegTwist`) and record the import values. Refuse `unsupported_humanoid_twist` for a pose that needs twist redistribution until its joint order and split are confirmed on limb fixtures. The supported subset must show that redistribution is inactive for the recorded motion. [Unity HumanDescription](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanDescription.html) |
| Clip settings | Read the versioned clip settings, including `m_Mirror`, `m_CycleOffset`, loop time, `m_LoopBlend*`, and the rotation, Y and XZ bake-into-pose flags (including the corresponding runtime blend fields), original/feet basis flags, start-at-origin, height and orientation offsets. Pin the exact neutral settings and mapping in the fixture metadata. Mirror, nonzero cycle/height/orientation offsets and unverified loop-blend, bake or basis settings refuse as `unsupported_humanoid_clip_settings` with the offending field. Missing settings without a confirmed layout default refuse too. [Unity clip import settings](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/ModelImporterClipAnimation.html) · [Unity root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html) |
| Other pose features | Refuse unsupported locomotion extraction, translation DOFs, additive poses and IK with distinct causes. Missing Avatar or any humanoid refusal prevents a successful export of that selected animation. |

**Body-bearing test.** A clip is body-bearing if any `RootT` or `RootQ`
component maps to a populated runtime slot (streamed, dense or constant), or
the verified effective import settings require root projection or a
body-to-hips correction in local pose. A mapped constant or neutral value
counts as populated. An absent slot must be established by the versioned
`m_IndexArray` mapping and absent sentinel, or its verified layout equivalent.
Bake-into-pose and disabled root-motion application alone do not qualify a
clip. The recorder inventories the body and muscle slots and records the
effective import settings from the table above, with same-bundle root and
hips-local samples. To qualify a clip it must establish absent body slots and
that the recorded settings and playback require neither root projection nor
body-to-hips correction, with the other pose features also in supported scope.
Compare the sampled root and hips-local TRS to the reset rig with only the
known muscle-driven components applied, within the derived Editor bounds.
Unconfirmed settings or layouts produce their typed refusal.

PR 8 ships muscle-only playback for clips the recorder qualifies and typed
refusals for the rest. If no clips qualify on the two fixture rigs, PR 8 ships
the typed refusals and the qualification evidence. Positive L11 playback moves
to the deferred Humanoid body motion slice in 2.6. The parity row records
verified P2b coverage for the typed refusals once their tests pass.

### 2.5 glTF animation, collection and profile

**Model and collection.** Extend the editable glTF `Scene` with distinct
animation samplers and channels targeting local TRS or weights. The collector
resolves mesh channels once. Pass renderer identity, channel name and hash,
target slot, selected frame and full weight to the sampling callback. Preserve
graph-node-to-scene-node and renderer-to-attachment maps. TRS targets original
Transform or joint nodes, while weights target the actual mesh instance. For
a detached world-space static batch, refuse a selected clip with TRS on its
original renderer Transform or an ancestor that would move it as
`animated_static_batch`. Apply that rule to combined batches too. The
collector returns typed failures before creating a misleading channel.

Apply P2a's change of basis once, using translation `(-x, y, z)`, quaternion
`(x, -y, -z, w)` and unchanged scale components. Normalize quaternions and
choose consistent signs across exported samples. A zero or nonfinite
quaternion or f32 overflow is a typed refusal, including in raw mode. Use one
weight vector per animated attachment and time, in emitted target order. Fill
untouched slots from P2a's node weights or mesh defaults. Divide animated
weights by the selected last frame's `fullWeight`, and report the existing
in-between approximation.

**Write and baking.** Times are finite f32 SCALAR seconds with min/max and are
nonnegative and strictly increasing after rounding. Outputs are f32 VEC3 for
translation and scale, VEC4 in XYZW order for rotation, and SCALAR weights in
key-major order with `key_count × target_count` elements. Use `STEP` for
wholly held properties and `LINEAR` otherwise (glTF quaternion interpolation
for rotation). Include source key boundaries and schedule samples. Round
times to nearest f32, ties to even. Deduplicate coincident rounded keys only
when their values and transition semantics agree.
Validate dimensions, values, references and unique targets before writing.
Omit empty animations and stream the aligned BIN in first-reach order.

**Held jumps.** For a jump at exact source time `t`, let `j = t - start`,
`b = round_f32(j)` and `a` be the f32 predecessor of `b`. A mixed held and
continuous channel emits the source property's left limit at `t` on `a`
and its right-hand value at `t` on `b`, including the continuous components'
values at the jump. Compare the reconstructed values at those keys to those
exact sided references, even when `b` rounds down. For wholly held `STEP`
compare the value immediately before `b` and the value at `b` to the same
references. Let `s` be the f32 gap from `b` toward `j` (zero if exact).
Accept `|b - j| ≤ s/2` on the right and `|a - j| ≤ (b - a) + s/2` on the
left for a mixed channel. For `STEP`, both sides of the transition use the
same `s/2` timing bound at `b`. Within the transition window from `min(a,j)`
to `max(b,j)` for a mixed channel, or between `b` and `j` for `STEP`, use
these sided value and timing checks. Apply the continuous error check
outside the window. Refuse a jump if required keys fall outside the rounded
clip interval, another key changes its bracket, its transition window
overlaps another jump's, or its sides collapse or conflict after rounding.

For baked output, compare interpolation reconstructed from the emitted f32
accessors with the mirrored source pose or intended last-frame morph weights.
Measure absolute component error for translation (Unity units), scale and
weights, and sign-invariant normalized quaternion angle in radians (4.2).
Use a bound of the form `A + B × ulp32(M)` per component, where `M` bounds
the component magnitude over the source and reconstructed interval.
`ulp32(M)` is the local f32 spacing, including subnormal spacing at zero.
Derive the spacing term from rounding f32 key values and glTF accessor
storage. Count a single rounding where a rounded key is stored unchanged.
Bound timestamp rounding via the source and reconstructed rates of change
outside jump windows. Derive the absolute term from a stated approximation
allowance and an absolute arithmetic floor. Propagate component rounding
through quaternion normalization and glTF interpolation to derive the angular
bound, including a separate angular approximation allowance. PR 2 freezes
the constants, rate and value ranges, and the derivation in its reference
notes before any implementation comparison. These are export bounds distinct
from L11.

Include the first/last times, source breakpoints, off-key quarter/mid/three-quarter
points and extrema or conservative subdivision bounds that cover narrow spikes
and fast turns. Refine within the bake approximation's own subdivision and
sample caps. If those caps prevent meeting or establishing the bound, return
`animation_bake_error`. An engine budget refusal retains its original typed
cause, including `limit(export_work, <value>)` when `max_export_work` is
exhausted (2.7). Later, glTF `CUBICSPLINE` can carry compatible
unweighted Hermite and streamed polynomial segments with the same derivatives
and normalized quaternion results, subject to equivalent interpolation and
continuity tests. [Khronos glTF interpolation](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#appendix-c-animation-sampler-interpolation-modes)

**Profile.** Extend the strict v1 profile with `clips` and a finite bounded
sample-rate override. The `all_discovered` selector takes all resolved clips
discovered on the unit's own rigs, in discovery order and within its loaded
view. Discovery reports unresolved slots. An explicit selector uses unit-scoped
root Transform and Animation/Animator component identities plus either the
resolved clip identity or the discovery source-slot identity. Address objects by SerializedFile
occurrence in the loaded view and signed path ID. A selected unresolved slot
returns its original cause. Reuse discovery's addressing and refuse missing
or ambiguous matches. Display
names are optional labels, so two same-name clips on different roots can be
selected independently. In standalone export, missing or empty `clips`
selects static GLB. Batch's defaults and per-prefab selections are in 2.7.
Nonempty `clip_states` remains a typed refusal. CLI and batch share the
selector and collect/write path. Report typed conditions and fixture-backed
approximations. A selected clip with refused playback channels fails its
animation request. A valid metadata-only clip retains its metadata and emits
no glTF animation. For a selected empty clip with neither playable channels
nor valid metadata bindings, return a typed no-playable-channels cause.

### 2.6 Later slices

| Later slice | Preconditions and policy |
|---|---|
| Controller state and blend-tree evaluation | Start from P2b's discovery tables and direct-clip sampler. Require versioned layer/state-machine/motion and leaf-to-clip resolution, recorded state time, speed, cycle offset, mirror, write defaults and null overrides, plus child duration synchronization and missing-channel rules. Verify 1D, Simple Directional 2D, Freeform Directional 2D, Freeform Cartesian 2D and Direct separately before enabling each. Humanoid states also need verified humanoid playback. [Unity 2D blending](https://docs.unity3d.com/2022.3/Documentation/Manual/BlendTree-2DBlending.html) |
| Humanoid body motion | Establish the pose-dependent mass and body-orientation model, root-motion projection under recorded import settings, and the hips-local TRS derivation. Verify with scaled rigs and identity and translated/rotated Animator roots before lifting the body-bearing refusal. |
| Variable bone weights (eight influences) | Record serialized `m_VariableBoneCountWeights`, counts, palette indices and weights against Editor APIs by version and validate deformed positions and both joint/weight sets. Four stays the default. An explicit eight-influence request on an unverified layout fails. Remap into the palette and merge duplicate joints before ranking and truncation, breaking ties by first source occurrence. Normalize across both sets then pad in normal mode. For eight influences with `normalise: false`, refuse explicitly until a raw contract for merged rows and both sets is verified. [Unity Mesh.GetAllBoneWeights](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Mesh.GetAllBoneWeights.html) |
| Sampled non-glTF properties | Start from retained serialized keys, bindings, value kinds, PPtr mappings, authoring curves and events. Require fixtures for typed integer and PPtr hold semantics, reference validation, scalar interpolation and custom width, and a consumer API for these sampled values. Keep event execution and any glTF extension for those properties outside this slice. |

### 2.7 Batch prefab units

**Spec 3.** Add opt-in `outputs.glb` with a required `prefab_key_template` and
an optional inline `profile`. The default selector, including when the
profile is omitted, is `all_discovered` for each selected prefab's own rigs
in that unit's loaded view. An empty discovery list yields a static GLB.
The batch profile accepts `clips: "all_discovered"` or `clips: []` as the
default for all prefabs. Optional explicit selections in `clips_by_prefab`
are keyed by the batch unit key, the complete expanded GLB output key. Each
entry maps input IDs to selector lists, with an empty list selecting static
export for that prefab. The input ID keeps a same-key contestant in another
view from inheriting the list. Each selector resolves only in its addressed
prefab and input view, even when a different view has the same SerializedFile
occurrence and path ID. Prefabs without an override use the default.

The template expands the complete GLB key. Validate presence, sizes,
duplicate members, unknown fields, types and ranges in phase 1, with a
charged profile and template parse. `--check` validates the selection shape,
including known input IDs, and checks each literal `clips_by_prefab` key
against the output-key rules that apply before planning. These static
failures are `bad_spec` (exit 78). See BATCH_SPEC 1.1, 2.3, 9.1 and 10.

In phase 6, after key planning and before selecting winners, check each
override against the addressed input's planned prefab unit keys. If an input
fails before its plan can establish membership, preserve its typed input
failure. A missing key in a completed plan is a job-level data failure. Use
the phase-6 finisher to write the fenced final head as `incomplete`, with
`stop_reason: "unknown_prefab_key"` and the first unmatched selection's JSON
pointer in spec order as `stop_detail`. Skip artifact publication and exit 4
unless a higher-priority outcome applies. Resume repeats the membership
check, so the same missing key fails again. Correct the selection or inputs
and retry with `attempt + 1` after the fencing wait. See BATCH_SPEC 7.2–7.4
and 9.1–9.3.

Allow GLB-only jobs. Spec 1 and 2 reject `outputs.glb` as `unknown_field`.
Texture and image options apply to 2D outputs. GLBs embed
textures through the glTF writer. Keep loose prefab units in this slice.
The batch PR updates BATCH_SPEC, including the run-time validation outcome,
schemas, examples and CLI schema discovery.

**Row identity and key ownership.** Visit the input's own eligible
SerializedFiles in 4.4 file order and each file's stored container rows in
ordinal order. Identify a row by input, SerializedFile occurrence and
container-row ordinal, with path and resolved root as provenance. Two rows
targeting the same GameObject are separate selections. Ignore successfully
resolved non-GameObject rows. For failures retain the row identity and typed
cause. Dependents can supply data by PPtr within the private view.
Dependency rows and unmarked scene files supply no prefab selections.

Append GLBs after textures, sprites and atlases in the existing key plan.
Expand and validate keys before export, with container path and its safe form
as well as the existing name/path-ID fields. A valid expanded key is owned
by the first row or 2D output in plan order, even if that owner's export
fails. Later identical keys in the same input are `duplicate_key` skips, with
no promotion, merge or GLB candidate. When a broken row cannot supply the
fields needed to expand a valid key, record its pointer skip without a claim.
A broken row with a valid independently expanded key retains the claim.
Keep row identity independent of the string key and of the resolved object's
identity.

Within an input, emit plan skips first in plan order, then export skips in
export order. Each phase visits the existing 2D outputs before the appended
prefab rows. The manifest preserves this skip order.

**Sealing and winners.** A successful GLB copy-out and seal supplies one
candidate and one unit. Its unit key is exactly its GLB output key, with
`prefer.md5` comparing the artifact's byte digest. Use E31/E32/E61 and the
existing prefix and archive-key checks. Across inputs, a GLB and a 2D unit
with the same unit key first compete by E31 and E61. After that contest, only
the winning units and all units under E61-withheld unit keys reserve their
keys. An ordinary losing unit is ineligible. If a GLB key is also a member
key of a differently keyed 2D unit and both reserve it, the first unit key in
bytewise order keeps it. The other winning unit is withheld whole as
`unit_conflict`. E61-withheld unit keys retain `withheld`, and reservations
persist through withholding. A plain candidate at a reserved key loses by
`unit`. The existing archive membership kinds stay 2D.

**Failure policy.** Carry typed causes through standalone and batch collect
and write, preserving input or dependency provenance before deriving report
strings. For a normal end with no higher-priority outcome, apply this table.

| Cause | Prefab artifact | Record and input status | Exit |
|---|---|---|---|
| Broken container-row PPtr | No candidate | Unexpected `bad_pointer` skip (or the original `outside_view` or decode cause), input `complete_with_skips` | 3 |
| Caller-selected clip outside the view, including through an override | No candidate, even if static geometry is usable | Unexpected `outside_view` skip with clip identity, input `complete_with_skips` | 3 |
| Missing optional renderer resource, such as a texture with a usable factor-only material | Keep the degraded GLB when the selected animation and remaining scene are usable | `glb_degraded` diagnostic with output key, renderer/resource identity and typed cause. Input `complete` if there are no actual skips | 3 |
| Any dropped selected animation or selected animation channel, including an unavailable humanoid path or lost renderer target | No candidate for that prefab | Unexpected GLB skip with the typed cause, input `complete_with_skips` | 3 |
| Bake approximation caps exhausted before meeting or establishing the bound | No candidate for that prefab | Unexpected `animation_bake_error` GLB skip, input `complete_with_skips` | 3 |
| Engine `max_export_work` exhausted | Discard the input's attempt candidates | Failed input with `limit(export_work, <value>)` | 5 |
| Object-scoped input budget refusal | No candidate for that prefab | Existing budget skip, input `complete_with_skips` | 5 |
| `budget(dependency)` anywhere in the export, even on an optional resource | Discard the input's attempt candidates | Failed input with `budget(dependency)` | 5 |
| Other input-scoped container, dependency or budget failure | Discard the input's attempt candidates | Existing failed-input classifier | 4 or 5 |

This plan amends BATCH_SPEC 9.3 for new GLB scope reasons. PR 7 adds this
rule to the expected-skip test and records the new reasons in its closed list.

> For spec 3, an object skip caused by refusal of a caller-selected animation
> or one of its playback channels is unexpected (exit 3), including when the
> clip is selected by the profile default and the reason is a supported-scope
> limit such as `unsupported_body_motion`. Budget and failed-input causes
> keep their existing classifications.

PR 7 adds `glb_degraded` to manifest 3 diagnostics and to BATCH_SPEC 9.2's
exit-3 outcomes, including a diagnostic-only run's `complete_with_skips`
final status. The diagnostic leaves `skips` rows and counters untouched.
A missing required renderer resource that makes a prefab unusable has no
candidate and retains its typed object skip or input failure as applicable.
Preserve the existing sink and run-level outcomes for publication, spool,
allocation and internal failures. If an input fails and `publish_incomplete: false`, withhold
all otherwise eligible winners and nonempty archives under the existing rule.
With it true, E61 still withholds each key whose `prefer.input` names a failed
input, including the whole unit for a withheld unit key. Other eligible keys
from successful inputs can publish. Unexpected skips and GLB degradation
diagnostics alone leave the input eligible for publication and still set exit
3 (or 5 for an object budget). Reuse stop, resume, verification and exit
precedence.

**Versioned records.** Preserve BATCH_SPEC 7.3's one-to-one spec-to-manifest
mapping for running and final fenced heads, including spec 3 runs with no GLB
request or no 3D artifacts. Version 3 adds the GLB request provenance, closed
`outputs.kind` and `skips.output` value `glb`, bounded container-row and root
provenance, and typed skip details. A GLB has null `visible_hash` and `mode`.
Collision rows keep `pixels_differ` boolean. It is false when either visible
hash is absent, including collisions involving a GLB.
Update the reason, detail and count vocabulary in the manifest contract.
Raise the progress stream to `p: 2`. Section 10 bumps `p` for any new
value of a closed set, whatever the `spec_version`, and the `exit` line's
`reason` gains `unknown_prefab_key`. The batch PR's binary emits `p: 2` for
every job. Snapshot fields, counters and the `bad_spec` rule/pointer
vocabulary are otherwise unchanged. New per-prefab outcomes appear in
manifest rows, not on the stream. New readers accept v1, v2 and v3. Older readers follow 7.3 by ignoring
added fields and treating unknown closed values as other. Preserve fencing's
seven-field identity envelope for all versions. Test running and final heads,
including a zero-artifact job and resume from a v3 head.

## 3. Parity targets

### 3.1 UnityPy's existing surfaces

| Row in `PARITY.md` | Existing level | P2b use |
|---|---|---|
| `AnimationClip`, `Avatar`, `Animator`, `AnimatorController` (14.2) | Tier A, L2 | Retain the verified P0 dict result and compare nested serialized fields. |
| `Animation`, `AnimatorOverrideController`, `RuntimeAnimatorController` and generated clip, binding, Avatar and controller records (2 and Appendix A) | Existing API inventory and L2 | Check stored dicts and object-mode field shapes UnityPy exposes. |
| `GameObject.m_Animator`, `GameObject.m_Animation` (14.2, 19.2) and `PPtr.deref` (14.1) | Existing API and L1 (env), including recorded B cases | Reuse the component accessors and scoped dereference tests. |
| `PackedBitVector.unpack_ints` and `unpack_floats` (15.3) | Tier A as filed, L6 | Reuse shared packed scalar checks where the formats match. |

UnityPy 1.25.2 exposes the stored data through its generated classes and
component patches. [UnityPy 1.25.2](https://pypi.org/project/UnityPy/1.25.2/)

### 3.2 Tier X

The plan records `planned (P2b)` rows in `PARITY.md` section 24. Typed
decoding and bindings have their own checks. Discovery identity and order
use expected lists of source identities. L11 compares per-frame local TRS
and renderer morph weights from the same serialized clip and rig as the Editor recording. GLB and
batch have separate L6g (animation) and L9 (batch) checks. Advance each row
only when its own named reference and test cover the stated scope.

## 4. Verification

### 4.1 Field reads, discovery and bindings

Use synthetic serialized objects with embedded trees and the existing
UnityPy 1.25.2 L2 harness. Cover explicit and muscle forms, nested Avatar and
controller fields, and versioned binding flags and widths. Exercise supported
stripped objects with the runtime TPK and its existing no-write guard.

Separately pin discovery's ordered list of clip and rig identities across
files with repeated signed path IDs, duplicate names, null and broken
overrides, controller cycles and outside-view references. Check the exact
binding/slot table, including unsupported known widths before healthy
bindings, unknown widths, malformed counts, mixed Transform storage, distinct
PPtr and integer encodings, the streamed initial PPtr seed and terminal dummy.
Exercise non-finite and non-positive clip sample rates, a nonzero start,
a constant-only clip, sparse slots before their first real key, and dense
samples between frames and beyond both grid ends.

### 4.2 L11 and f64 transcription

**Equations and transcription.** PR 2 lands a standard-library f64 Python
evaluator, [`tools/parity/anim_reference.py`](../../tools/parity/anim_reference.py), and
[its notes](../anim_reference.md) before the Rust
playback and retargeting PRs. Use public descriptions only. For curve math,
cite the Khronos interpolation appendix for Hermite and normalized
quaternion results, Unity's Euler API for the documented rotation
convention and the Bézier formula for weighted segments. For humanoids,
cite Unity's Mecanim muscle-range description and the public HumanTrait and
HumanPose APIs for indexing and body-space semantics. Derive the candidate
equations from those descriptions and ordinary quaternion/matrix composition.
Document the source and assumptions of each equation, including the neutral
limit map and any joint-axis composition. Unity's native retargeting equations
are unpublished. Any convention the descriptions leave open needs an explicit
hypothesis guarded by Editor evidence, or a typed refusal. Read AssetRipper
(GPL) for understanding only. Copy no code. [glTF interpolation](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#appendix-c-animation-sampler-interpolation-modes) · [Bézier easing functions](https://www.w3.org/TR/css-easing-1/#cubic-bezier-easing-functions) · [Unity Quaternion.Euler](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Quaternion.Euler.html) · [Mecanim Humanoids](https://unity.com/blog/engine-platform/mecanim-humanoids) · [Unity HumanTrait](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanTrait.html) · [Unity internal human pose](https://docs.unity.cn/2023.1/Documentation/ScriptReference/HumanPoseHandler.SetInternalHumanPose.html) · [AssetRipper licensing](https://assetripper.github.io/AssetRipper/)

**Weighted Bézier time solve.** For an interior sample use normalized time
`(sample - left_time) / (right_time - left_time)`. Solve the time cubic with
endpoints 0 and 1 and the versioned control points. For a confirmed fractional
weight encoding the inner time controls are `outWeight` and `1 - inWeight`.
Require a finite, strictly increasing time curve
on the segment, allowing isolated zero derivatives. Return stored endpoint
values at or beyond the corresponding endpoint. Bisect the parameter bracket
`[0, 1]` for at most 64 iterations. At each iteration use
`mid = lo + (hi - lo) * 0.5`, evaluate time by de Casteljau with the lerp
`a + (b - a) * mid`, and move the lower bound if the result is below the
normalized sample time, otherwise move the upper bound. Return `mid`
immediately on equality or when it rounds to either bound. At the iteration
cap return the midpoint of the final bracket. Use f64 round-to-nearest, ties
to even, with operations in the stated order and separate multiply/add.
Evaluate the value cubic at that parameter with the same de Casteljau order.

**f64 transcription gates.** Give Python analytic cases and independently
recorded binding, axis, rest and curve/muscle inputs. Compare componentwise
absolute error for vectors and morph weights and normalized angular error
for rotations. Normalize both quaternions, choose their signs for a
nonnegative dot product, then measure `4 atan2(||q - r||, ||q + r||)`.
Derive the vector gates as an absolute floor plus a scale-dependent term
from the f64 operation counts and magnitude bounds for the stated polynomial,
linear interpolation, axis and pose algorithms. Include cancellation,
normalization conditioning and bounds on elementary functions where used.
For the time solve, include both the final parameter uncertainty from
bisection and the effect of normalization and time-cubic rounding on the
inverse. Enclose the possible parameters using the monotone time curve and
its evaluation error, including at a flat derivative or rounded equality.
Propagate that interval through the value cubic. Propagate quaternion error
through normalization and the angular metric to derive the rotation gate.
PR 2 freezes the resulting bounds and valid ranges in its reference notes
before any implementation comparison. Shared equations can reproduce the
same semantic error.
Include neutral/identity, single-axis, noncommuting multi-axis and asymmetric
limits, an optional missing bone, differing rig proportions, generic scale
and non-root translation.

**Editor playback.** The actual L11 reference is Editor playback using
the **same built bundle set, serialized clip and instantiated rig/Avatar**
that Rust reads. Land the recorder, replay manifest format and tolerance
procedure with the Python evaluator in their own PR. Record bundle digests
and the serialized identities of the prefab, clip, Animator and Avatar,
the exact Editor version and build target, import settings, rig transforms,
and playback configuration. Capture f32 local TRS and each renderer's
blend-shape weights at the recorded times and widen exactly.
A supplementary `HumanPoseHandler.SetHumanPose` recording applies known
muscle vectors on the reset rig and isolates muscle-to-pose evaluation.
Same-bundle playback supplies the separate evidence for clip decoding and
muscle binding. [Unity SetHumanPose](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPoseHandler.SetHumanPose.html)

**Replay and tolerances.** PR 2 defines the Editor tolerance derivation.
Bound f32 rounding along the documented playback path for the recorded
layout and ranges. Include rounding the time given to Unity, interpolation
and its local rate of change, pose and axis composition, quaternion
normalization, and f32 outputs. Propagate those errors into the component
and angle metrics above and into the weight units reported by the Editor.
At held boundaries, record which side the effective f32 sample time selects
and compare that side. Repeated recordings can test recorder repeatability.
Freeze the tolerances and derivation alongside each recording before comparing
any implementation results. Resolve discrepancies by investigating the
implementation, format or fixture.

PR 6 owns the generic and legacy Editor replay, including morph weights.
PR 8 owns humanoid replay and qualification. Rust replays the same sample
times and compares every expected node and renderer weight before glTF
conversion, reporting the worst errors by rig, bone or renderer, property
and source time. Run the generic and legacy comparison once the recordings
are available. L11 rows stay planned until same-bundle playback and the
Rust comparisons pass for their claimed layouts.

- PR 6 cross-checks bundle key-time completeness and sample off-key status, the source origin and `m_StartTime` where present, endpoint fields, actual stage ranges and each budget's `track_kind` against the bundle-derived playback path, and verifies a held probe distinguishes the clocks before generic or legacy L11 promotion.
- PR 8 cross-checks the same timing, endpoint, stage-range, track-kind and clock-probe obligations plus the body, muscle, goal and translation inventory, mapping, absent sentinels and storage against the serialized Avatar and clip before humanoid L11 promotion.

See the numbered [Rust-side cross-check obligations](../anim_reference.md#rust-side-cross-check-obligations).

### 4.3 GLB, skin and profile

- Run the pinned glTF-Validator and existing allowlist and orientation checks.
  Extend `tools/parity/gltf_check.py` to reconstruct accessors and interpolation
  independently and enforce section 2.5's baking bounds. Include nonlinear
  and held segments, jumps rounding up and down, a large translation with a
  nonrepresentable f32 endpoint, a fast turn, single-sample clips, a collapsed
  timestamp, malformed channels, valid metadata-only bindings alongside TRS,
  two morph instances sharing a mesh and a filtered node.
- In a non-rest four-influence skin fixture with animated joints, compute
  the Unity-space positions using all four joint transforms, inverse binds
  and weights. With `D = diag(-1, 1, 1, 1)`, independently reconstruct the
  glTF positions using the emitted animation and `D · M · D` for transforms
  and inverse binds. Compare to the mirrored Unity positions within the
  declared f32 tolerance. Include a separate skin attachment and a detached
  static-batch refusal.
- Compare writes under repeated calls and multiple caller threads. Exercise
  prefab and scene selection from the CLI, including same-name clips on
  different roots, source refusals and static profile compatibility. Keep
  the static GLB and OBJ baseline checks.

### 4.4 Batch

Replay synthetic GLB-only and mixed 2D/GLB jobs, with Animator, clip and
renderer resources crossing bundle boundaries in the private view. Compare
candidate GLB bytes with the standalone writer's. Cover two rows pointing
to one GameObject, two rows to one key with a failing first owner, a broken
row with and without an expandable key, same-name CABs, invalid keys, and
within-input and across-input 2D collisions. Include a member key held only
by an ordinary losing unit and one reserved by an E61-withheld unit key.
Test all-discovered defaults, two different rig selections, same-key
contestants with repeated view-local IDs, and a valid key absent from the
plan. Pin `--check` acceptance of that key, the final `unknown_prefab_key`
head and exit 4, and resume after correction. Test `prefer.md5` against byte
digests, prefix conflicts and withholding. For each per-prefab outcome in
2.7, pin candidate eligibility, typed row,
input status and exit together. Exercise both `publish_incomplete` settings
with another successful input, including a
dependency-budget refusal from an optional resource, a `max_export_work`
refusal distinct from a bake approximation cap and E61 prefer-withholding.
Check exact L9 manifest shapes, running and final heads for v1/v2/v3, a
zero-artifact v3 job, the progress protocol, interrupted publication and resume,
mixed-job skip order, and stable records with 1 and N workers.

## 5. Performance and memory

- Read and sample on the caller's thread. Meter counts, identity/path maps,
  Avatar arrays, track samples and output accessors before growth. Check
  sample count × tracks × arity and frame/target products. Charge traversal,
  polynomial evaluation, subdivision, pose composition, sorting and
  emitted bytes. Fuzz streamed counts, dense products, binding slots,
  controller cycles and Avatar indices.
- Cache decoded clips by file/object identity per unit. Key bindings
  and samples by clip and rig/component. Release scratch when each clip
  finishes. Measure coexistence of animation, geometry and embedded PNGs
  with the streamed BIN writer.
- Charge sealed export to the existing attempt, export-work and spool
  budgets and preserve dependency provenance on reads. Keep stop checks
  and copy-out digest accounting. Measure high curve counts, long clips and
  large morph vectors. Run squeeze, allocator refusal and zero-charge checks.

## 6. PRs

| # | PR | Contents | ~Non-test lines | Depends on |
|---|---|---|---|---|
| 1 | Plan | Plan, DESIGN estimate, static3d cross-reference and planned PARITY rows | docs | — |
| 2 | Reference and recorder | Public equation notes, Python f64 transcription check, frozen bake and transcription bounds, Editor recorder and f32 tolerance procedure, replay identity and time contract | 0.7–1.0k | 1 |
| 3 | Discovery and binding types | PPtrs, controller tables and overrides, versioned GenericBinding types and widths, graph path/CRC binding, renderer channel input and synthetic fixtures | 0.9–1.2k | 1 |
| 4 | Clip reader and bound sampler | Runtime/source precedence, explicit and packed curves, compiled slots, timing and generic/legacy TRS and morph sampling, L2 and synthetic replay | 1.6–2.1k | 2, 3 |
| 5 | glTF animation model and writer | Editable tables, validation, deterministic JSON/BIN, accessors, validator and reconstruction checks | 0.9–1.2k | 1 |
| 6 | Generic collection, profile and CLI | Collector callback and morph map, selection, mirrored TRS, baking, four-influence deformation test, standalone generic/legacy GLB and same-bundle Editor replay of TRS and renderer weights | 1.2–1.6k | 3, 4, 5 |
| 7 | Sealed GLB prefab batch | Spec/schema/manifest v3, row identity, key ownership, attempt/winner integration and typed failure L9 cases | 1.0–1.4k | 6 and P3a T3 (merged) |
| 8 | Avatar and humanoid integration | Runtime Avatar tables, qualification evidence, muscle-only retargeting for qualified clips, typed refusals for the rest, same-bundle L11 replay and collector integration for qualified clips | 1.0–1.5k | 2, 3, 4, 6 and section 8 recordings |

**Total estimate.** 7.3–10.0k non-test lines, including the recorder and
reference. PRs 2, 3 and 5 can start together. Merge PR 2 before the code it
checks and PR 3 before the bound sampler. Generic collection has a synthetic
integration gate. PR 6's Editor comparisons gate L11 status and run when the
recordings are available. The batch PR can prepare keys and schema once the
plan lands, and merge after generic collection without waiting for Editor
fixtures or humanoid retargeting. Once verified, humanoid uses the same
collection path.

## 7. Risks and gates

- A passing field read can still assign the wrong slots or select the wrong
  rotation source. Compare the chosen source and slot table separately and
  use off-key same-bundle playback for the pose.
- Runtime Avatar scope stays tied to recorded layouts, axes and effective
  clip settings. Body motion, twist or other unverified semantics produce
  the typed outcomes in 2.4.
- Bone, renderer and attachment identity survive filtering and reuse of
  meshes. Deformation and morph fixtures check the affected instance.
- A prefab can exceed a sealed attempt while following otherwise optional
  references. Preserve typed budget provenance through collection and output.
- Whole scenes in batch and 3D archive membership can be considered after
  loose prefab GLB has L9 coverage.

## 8. Editor fixture project (owner-built)

Extend the project in `docs/plans/static3d.md` section 8 with small original
assets. Record actual Unity 2022.3 and 6000.x patch releases, build target,
loaded bundle hashes, rig/asset identities and import settings. Use the
repository's fixture rules for the built assets and numeric records.

For each sample, instantiate a fresh rig from the loaded bundle or reset it
to the documented identical rest and Animator state. Use an explicit
one-clip evaluation at speed one, with no controller state blending,
transitions or accumulated delta. For Animator playback set
`Animator.applyRootMotion = false` before evaluation, disable foot IK and
playable IK, and leave `OnAnimatorMove` deltas unapplied. For legacy Animation,
record the direct legacy sampling API and playback state. Pin loop settings,
time origin and endpoint handling.
Log both the clip-local time given to the recorder and serialized source
time (including the nonzero `m_StartTime` offset), and the reset procedure.
Record local TRS and per-renderer blend-shape weights at the first/last times
and off-key times in Unity space. Key the weights by renderer identity and
mesh channel index and name, including when renderers share one mesh.

- A legacy explicit clip and generic runtime clips with recorded
  streamed, dense and constant layouts, including constant-only and nonzero
  start. Include precedence with authoring curves, quaternion tangents,
  Euler order/full turns, packed rotation slopes and between-key samples,
  initial and changing object references, discrete flags, and non-ASCII
  path hashes. Record whether mixed Transform storage is present.
- A legacy Animation and an Animator with a controller and overrides.
  Record direct clip identities and order, including null substitution,
  separately from poses. Use a direct clip for the pose recording.
- Two humanoid rigs with different proportions, asymmetric limits or axes
  and a materialized hierarchy, including a missing optional bone. Record
  candidate muscle-only playback and known-vector `SetHumanPose` cases.
  Record identity and nonidentity Animator roots. For every candidate record
  the serialized body and muscle slot inventory, absent sentinels and storage
  ranges, each effective import setting and its serialized counterpart, and
  root and hips-local comparison results for the 2.4 qualification rule.
  Record why each clip qualifies or which typed refusal it requires. Include
  body motion, twist and non-neutral clip settings as probe/refusal cases.
  Capture rest poses, mappings, axes and effective import settings from the
  Editor independently of the Rust decoder.
- Four-influence animated skinning and two instances of a mesh with
  animated blend shapes, including non-100 full weight and in-between
  frames. Extra-influence layout recordings belong to the later slice.

The recordings gate serialized-playback and humanoid L11 status. Synthetic
curve, binding, writer, generic collection and batch tests can run first.
