# P2b animation reference

The f64 transcription evaluates decoded inputs independently of Rust. Its
analytic cases check the equations below. L11 measures Unity-space playback
from the same built bundle, serialized clip and instantiated rig. The
[animation plan](plans/anim.md#42-l11-and-f64-transcription) assigns generic
and legacy replay to PR 6 and humanoid qualification and replay to PR 8.

## Playback equations

Times are in source seconds. For a compiled clip the closed interval is
`[m_MuscleClip.m_StartTime, m_MuscleClip.m_StopTime]`. For an explicit clip
use the interval of its layout, or the first and last real runtime keys if
the layout has no interval. A constant-only clip still uses its stored
interval. The default samples are `start + i / m_SampleRate` inside the
interval and the exact start and stop, each once. A zero-duration clip
has one sample. A schedule or rate override still requires a positive,
finite serialized clip sample rate. An explicit schedule supplies finite,
strictly increasing source times within the interval. Export writes one
pass at `source_time - start` and retains loop and wrap metadata. The last
sample belongs to that pass even when a runtime looping mode would wrap to
the first. Dense grid bounds and streamed markers have their own meanings.

Resolve one writer per property as in [plan section 2.2](plans/anim.md#22-clip-readers-and-playback).
Compiled bindings own their supported slots. Authoring curves are
metadata. A confirmed compressed rotation with a matching path precedes
explicit quaternion and then Euler on explicit layouts. Use rest values in
unanimated translation and scale components, and use the rest rotation if
there is no rotation writer. Blend-shape samples are scalar values for each
renderer and mesh channel, in Unity weight units.

### Compiled storage

The scalar slot order is streamed, dense, constant. Build the typed
binding table before indexing those ranges. Public readers support the
layout, coefficient and value field positions. Sampling behavior, sentinel
shapes and mixed-storage Transform bindings need same-bundle evidence.
[AssetStudio reader](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/AnimationClip.cs)
· [AssetRipper reader](https://github.com/AssetRipper/AssetRipper/blob/1.3.0/Source/AssetRipper.Processing/AnimationClips/AnimationClipConverter.cs)

- Streamed. For a slot, take the last real key at or before the sample.
  With `d = sample - key.time` and payload `(c3,c2,c1,c0)`, evaluate
  `((c3*d + c2)*d + c1)*d + c0` up to the next key. Before the first
  ordinary key and after the last real key, hold the first or last stored
  value respectively. An initial sentinel can seed a held PPtr in a
  confirmed layout. Use that seed up to the first real key, even if the
  first real key is later than the clip start. An empty slot needs a
  supported initializer. Validate the terminal dummy separately and
  exclude it from the real keys and duration.
- Dense. Frame `i` is at `g(i) = m_BeginTime + i / m_SampleRate`, using
  the dense layout's rate. Read `A[i*curve_count + slot]`. With
  `g(i) <= sample < g(i+1)`, let `u = (sample - g(i))*rate` and evaluate
  `A[i] + (A[i+1] - A[i])*u` for that slot. At a grid point use its stored
  value. Clamp outside the grid, and hold a one-frame slot. Discrete slots
  hold the preceding value with a right-hand change at a grid point.
- Constant. Read one value per slot and hold it over the whole playback
  interval.

PPtr indices and bit-encoded integer slots use the validated typed values
of the layout. Held streams change at the real key and keep the previous
value up to it. The Python functions take ordered real keys and typed data
after sentinel, layout and binding checks. A polynomial with a constant
segment alone does not establish a version's held encoding.

### Explicit curves

For source endpoints `t0 < t1`, let `h = t1-t0` and `u = (t-t0)/h`.
Ordinary Hermite uses the four basis terms in this order:

```text
(2*u³ - 3*u² + 1)*v0 + (u³ - 2*u² + u)*(h*outSlope0)
+ (-2*u³ + 3*u²)*v1 + (u³ - u²)*(h*inSlope1)
```

Tangents are derivatives per second. Use stored endpoints and their
clamps. A confirmed held segment keeps `v0` for `t < t1`, then uses
`v1` at `t1`. The Hermite bases and quaternion normalization follow the
public interpolation formula. [Khronos interpolation appendix](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#appendix-c-animation-sampler-interpolation-modes)

A confirmed fractional weighted segment uses these cubic Bézier controls:

```text
x = (0, outWeight0, 1-inWeight1, 1)
y = (v0, v0+(h*outSlope0)*outWeight0, v1-(h*inSlope1)*inWeight1, v1)
```

Use one-third for an unweighted side when the layout confirms ordinary
Hermite there. The normalized time is `u`. Solve `B(x,p)=u` for `p` and
return `B(y,p)`. Each evaluation uses three levels of de Casteljau with
`a + (b-a)*p`. Fractional weights are in `[0,1]`. The time derivative
is three times the quadratic Bézier with controls
`(outWeight0, 1-outWeight0-inWeight1, inWeight1)`. Check both endpoints and
the vertex if it is the interior minimum. Require a nonnegative derivative.
The fixed time endpoints ensure a strictly increasing cubic even with an
isolated zero derivative. Return stored endpoint values at or beyond the
segment ends. Bisect `[0,1]` for up to 64 steps using
`mid = lo + (hi-lo)*0.5`. Return `mid` on equality or when it rounds to
either bound. Move `lo` when the evaluated time is below `u`, otherwise
move `hi`. At the cap return the midpoint of the final bracket. Use
round-to-nearest-even f64 and separate multiply and add.
[W3C cubic Bézier easing](https://www.w3.org/TR/css-easing-1/#cubic-bezier-easing-functions)
· [Unity Keyframe](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Keyframe.html)

For packed legacy rotations, accumulate integer `m_Times` deltas from
zero with a checked sum and divide by 100. This yields source seconds
without adding a start offset. Packed XYZW values and packed slopes are
separate. Key association, slope units, scaling and missing slopes require
off-key playback evidence. Until then the playback refusal is
`unsupported_packed_rotation_slope`. [AssetStudio packed rotation export](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudioUtility/ModelConverter.cs#L747-L767)

### Rotations and candidate joint axes

For quaternion curves evaluate all four serialized XYZW components and
then divide the vector by its Euclidean norm. Keep source values and
derivatives together in the serialized hemisphere. A fixture-confirmed
antipodal adjustment must flip the key and both its tangents together.
Apply any output sign continuity after sampling. A zero or invalid raw
quaternion refuses. For the angular metric, normalize both quaternions
`q,r` and negate `r` when their dot product is negative. The difference
and sum norms are `2 sin(θ/4)` and `2 cos(θ/4)` for the rotation distance
`θ`, so use `θ = 4 atan2(||q-r||, ||q+r||)`. This also gives zero for
opposite quaternion signs and avoids loss of resolution near zero angle.
Use absolute component errors for translation, scale and weights.

For Euler values, sample degrees without folding revolutions. About axis
`i`, the unit quaternion is `(axis_i*sin(d_i*π/360), cos(d_i*π/360))`.
For application order `A,B,C`, compose `Q_C Q_B Q_A` using the Hamilton
product and normalize. The confirmed enum layout uses
`0..5 = XYZ, XZY, YZX, YXZ, ZXY, ZYX`. Where there is no stored order,
use ZXY only for a layout with confirmed default behavior.
[Unity Quaternion.Euler](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Quaternion.Euler.html)
· [Unity rotation-order enum](https://github.com/Unity-Technologies/UnityCsReference/blob/9778aaee88c7de7656c15184ba5c91ca66db7e94/Runtime/Transform/ScriptBindings/Transform.bindings.cs)

Unity documents normalized muscles and the public bone and muscle
indices. For a fixture hypothesis, let the neutral-relative degree
limits be `l <= 0 <= h` and let `d = sign*m` for `m` in `[-1,1]`.
Use `d*h` for `d >= 0`, or `d*(-l)` otherwise. The input limits must
already be relative to the confirmed neutral pose. A candidate local
rotation is `normalize(Q_rest Q_pre Q_axes Q_post)` with `Q_pre` and
`Q_post` normalized, mutually inverse bases and `Q_axes` composed in
the recorded order. At neutral this leaves rest unchanged. Leave local
positions, scales and bones without a muscle writer at rest. An absent
optional bone has no output writer.

The sign relative to the limits, the neutral pose, the multiplication
order and the interpretation of the stored axes are hypotheses for PR 8.
Known-muscle playback must distinguish them on asymmetric limits and
noncommuting rotations. The transcription exercises them without treating
them as Unity's unpublished retargeting equations. Unknown conventions
give `unsupported_humanoid_axes`. `RootT` and `RootQ` are body-space
quantities, with position scaled by the Avatar's human scale. Reconstructing
hips-local TRS would require the pose-dependent body model and root-motion
projection. Use the body-bearing test and the other refusals in
[plan section 2.4](plans/anim.md#24-avatar-and-humanoid).
[Unity Mecanim Humanoids](https://unity.com/blog/engine-platform/mecanim-humanoids)
· [Unity HumanTrait](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanTrait.html)
· [Unity internal human pose](https://docs.unity.cn/2023.1/Documentation/ScriptReference/HumanPoseHandler.SetInternalHumanPose.html)
· [Unity root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html)

## Frozen f64 transcription bounds

[anim_bounds.json](../tools/parity/anim_bounds.json) is the versioned numeric
contract. Treat the decoded inputs to each equation as exact. The envelope
is `|source_time| <= 1024` seconds, interval and dense grid steps at least
`2^-20` seconds, absolute Euler input at most 3600 degrees, operation
scale at most `2^40`, and raw quaternion norm at least 1/4 throughout the
comparison interval. Stay within the envelope and verify the predicates
that choose segments, keys and branches. A comparison outside it cannot
establish the gate.

Let `u64 = 2^-53` and `γ(n) = n*u64 / (1-n*u64)`. For a scalar compare to
an exact analytic expectation with `2^-900 + γ(n)M`. Use a sum of
absolute contributions, so cancellation near zero leaves the scale of
the operands in the budget. The floor covers f64 subnormal rounding
even with amplification by the interval, magnitude and norm bounds.
Use the sum of the two evaluation budgets when both sides have numerical
error. The rounded operation counts below include slack for computing
the scale. They budget separate multiply/add.

| Equation | `n` | Choose `M` at least |
|---|---:|---|
| Streamed | 16 | `abs(c3)*D³ + abs(c2)*D² + abs(c1)*D + abs(c0)`, with `D >= abs(d)`. Six Horner operations plus each occurrence of the rounded time difference. |
| Dense | 16 | `abs(a)+abs(b)+abs(b-a)*(1+rate*(abs(sample)+abs(begin)+abs(i/rate)))`. Scale for grid division and addition, subtraction, rate multiplication and lerp. Use the maximum adjacent scale at a grid boundary, including the neighbor if clock rounding can move the bracket. |
| Constant | 0 | The stored value widens exactly. Endpoint and held selection are identity checks. |
| Hermite | 64 | `16*(abs(v0)+abs(v1)+h*(abs(outSlope0)+abs(inSlope1)))`. Two subtractions and a division form time, then two powers, four basis terms, scaled slopes and four accumulated terms. The factor 16 bounds the absolute basis coefficients and their perturbations. |
| Weighted | 64 | `max(abs(v0),abs(v1))+h*(abs(outSlope0)+abs(inSlope1))` for control formation and six three-operation lerps. Add the inverse-time term below. |
| Muscle limit | 4 | `max(abs(l),abs(h))`, for sign and limit multiplication with an exact branch at zero. |
| Euler and joint axes | 256 | `8*(1+max(abs(degrees))*π/360)` with normalized basis and rest quaternions in the stated norm range. Form three half angles and six trig values, and compose at most six products of four signed products per component. The scale covers angle conversion, absolute signed contributions of unit quaternions and normalization conditioning of the basis inputs. |

For the weighted inverse, normalization uses two subtractions and a
division. Conversion of the inner time control and the six bounded
three-operation lerps add time-cubic error on `[0,1]`. Their combined
absolute error is at most `γ(64)` in normalized time. Enclose both roots
using `B(x,p)^{-1}` at `u ± γ(64)` clipped into `[0,1]`. The reference
brackets these inverses with 64 exact rational bisections and outward
endpoints, then includes `2^-52` for final parameter rounding and
termination. A rounded equality can occur anywhere in this interval.
This construction covers a flat derivative. The absolute derivative of
the value cubic is bounded by three times the largest adjacent control
difference. Multiply it by the parameter enclosure width and add the
value arithmetic bound. Widen the derivative bound by six times the
value arithmetic bound for rounded controls.

For quaternions, a per-component raw error `e` bounds its four-dimensional
norm by `2e`. For norm lower bound `N`, normalization moves the unit
endpoints apart by at most `4e/N` in chord length. The quaternion
rotation distance is bounded by `min(π, 4 asin(min(1,2e/N)))`. Add
`256*u64` for f64 normalization and evaluation of the angular metric.
This budget assumes elementary functions within two ulps over the
evaluated domains. Use the recorded norm lower bound for a complete
interval, including off-key values, so a near-zero raw quaternion cannot
hide under a generous angular tolerance.

## Frozen bake bounds

Baking compares reconstructed f32 accessors with the mirrored source
pose. Mirroring maps translation to `(-x,y,z)`, rotation to
`(x,-y,-z,w)`, and preserves scale. A morph target divides its Unity
weight by the selected last frame's `fullWeight`; the resulting glTF
weight units apply here. Reconstruct in f64 with glTF STEP, LINEAR and
short-arc SLERP using decoded accessor values.
[Khronos interpolation appendix](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#appendix-c-animation-sampler-interpolation-modes)

For a continuous component let `M` bound its magnitude on the source and
reconstructed intervals, `T` bound the local timestamps, and `R` bound the
sum of the source and reconstructed absolute rates. Use:

```text
A + B * ulp32(M)
A = approximation[property] + 2^-30 + 0.5 * ulp32(T) * R
B = 0.500001
```

Spacing is `2^(floor(log2(M))-23)` down to a minimum of `2^-149`,
including at zero. A key stored unchanged in the accessor rounds once
with at most half an ulp of error. Linear interpolation is a convex
combination of the rounded endpoints, so this same bound holds between
keys. The extra `0.000001` ulp covers f64 reconstruction arithmetic,
and the floor covers absolute f64 arithmetic including subtracting the
clock origin. A key-time displacement is bounded by half a local ulp.
On a continuous interval, align the two clocks at their endpoints;
interpolation of the endpoint displacements stays within that time
bound. Multiply by the rate bounds for the poses being compared.

For rotation use the same timestamp term with the sum of angular
rates, the angular approximation allowance and arithmetic floor, plus
`32 * ulp32(1)`. Component rounding of a unit endpoint is at most
`0.5*ulp32(1)`. Normalization and the chord bound above give about
`4*ulp32(1)` radians per endpoint. On the same selected short arc,
each endpoint's transverse interpolation sensitivity is at most one
on the unit sphere (the arc in quaternion space is at most `π/2`);
the along-arc factors are also at most one. Budget both endpoints
and the arithmetic with the factor 32. Certify arc selection across
rounding (a positive aligned dot greater than `8*ulp32(1)` suffices);
otherwise split and check the resulting arcs or refuse. Degenerate
equal orientations are constant on the exact short arc.

Allowances are 0.0001 Unity units for translation, 0.00001 for scale
and weights, and 0.0001 radians for rotation. They are approximation
budgets chosen for P2b, with rate caps of 4096 component units/s and
512 radians/s, magnitude cap `2^20` and local time cap 1024 seconds.
Use the source breakpoints, schedule, endpoints, quarter points,
midpoint and three-quarter points. Inspect extrema or use conservative
interval bounds to cover narrow spikes and fast turns. Refine up to depth
20 and 65,536 keys per channel; obtaining or certifying the bound past
these caps gives `animation_bake_error`. Report actual engine work
limits with their own cause.

For a held jump at `j = source_time-start`, let `b = round_f32(j)` and
`a = predecessor_f32(b)`. Let `s` be the spacing from `b` toward `j`
(zero for an exact hit). A mixed channel encodes the left limit on `a`
and the right value on `b`, with the continuous components evaluated
at the exact jump. For STEP, compare immediately before `b` and at
`b` to the same sides. The time error is at most `s/2` on the right
and `b-a+s/2` on the mixed left. Both STEP sides use `s/2` at `b`.
Use the sided value and time tests inside the transition window
(`[min(a,j),max(b,j)]` for mixed, and the interval between `b` and `j`
for STEP), even if `b` rounded down. Check continuous error outside it.
An invalid interval, an intervening key, overlapping windows or
conflicting/collapsed sides refuses the jump.

## Same-bundle recorder and replay contract

A replay document has an owner-prepared `spec` and recorder-generated
`capture` under a versioned [JSON Schema](../tools/parity/anim_replay.schema.json).
The [synthetic replay test](../tools/parity/tests/test_anim_replay.py) shows
both halves on a one-node prefab with two renderers sharing a mesh.

### Identity and evidence

- The ordered `bundles` list describes the loaded view, including
  dependencies. Each entry gives a relative bundle basename and SHA-256.
  The recorder checks the digest of the same bytes it gives to Unity.
- An object ID has a zero-based `bundle` and a `file_index` for the
  SerializedFile occurrence within that bundle's inspection view. Root and
  resource entries also consume file indices, as in [the CLI inspect
  contract](AGENTS.md#inspect-and-pagination). `file` is the exact stored
  name. `path_id` is a canonical signed 64-bit decimal string, which
  survives JSON number conversions.
- Inspect the exact hashed artifacts before recording. Check the bundle
  container PPtr for each selected prefab, clip and Avatar asset key.
  From the prefab GameObject, walk Transform child and GameObject
  component PPtrs in stored order to establish the address-to-ID crosswalk
  for the Transform nodes, Animation or Animator and SkinnedMeshRenderers.
  Follow the Animator's Avatar and each renderer's mesh PPtr. Retain this
  mapping in the spec as the built-ID evidence. Public playback APIs select
  objects by asset key and hierarchy without exposing built file/path IDs.
  Use one object of the needed type per asset key. Capture checks those
  lookups, the selected clip and node names, the bound Avatar object and
  shared mesh relationships.
- An address is a sequence of zero-based child indices separated by slashes
  from the prefab root. The empty address denotes the root. Component
  ordinals count components of the same type on that Transform. This
  preserves identity with duplicate or non-ASCII names. Pose arrays
  follow `spec.nodes` and `spec.renderers` in order. Each weight belongs to
  a renderer ID and its mesh channel index/name. Include every frame's
  `full_weights` in frame order. For example, two renderers can share
  channel 0, `Lift`, with frame thresholds 25 and 80 and yield different
  weights at the same instant.
- `evidence` labels the layout and justifies the clock with probe results.
  Attach independently inspected bindings, chosen runtime fields, slot
  ranges, packed and discrete encodings, sentinel shapes and path hashes as
  appropriate. Record discovery in stored order as owner ID, field,
  ordinal, original and resolved IDs and outcome. Include null, broken and
  override cases where relevant. This preserves order and provenance when
  an Editor controller API flattens the list.
- `import_settings` pairs effective values with the serialized fields.
  Record absence and justify any version-specific default. For human
  clips include rotation, Y and XZ bake and blend flags, original and
  feet bases, start at origin, mirror, offsets, loop, twist and the
  remaining [plan section 2.4](plans/anim.md#24-avatar-and-humanoid)
  settings. The recorder captures the built-clip settings reported by
  `AnimationUtility` and the human limits and twist from the Avatar.
  A matching clip setting name in the spec must agree with the captured
  effective value. Use distinct labels for an authoring setting and a
  built setting that report different quantities.

### Playback and clocks

The recorder accepts one selected clip on one rig per manifest. Use
nonlooping Animator clips for endpoint replay. The recorder removes the
controller from the fresh instance and evaluates a manual one-clip graph
at speed one and zero delta. It disables foot and playable IK, sets
`Animator.applyRootMotion = false` before evaluation, turns off animation
events and excludes fixture scripts so `OnAnimatorMove` cannot apply
deltas. The legacy path stops other states and uses `Animation.Sample` on
a single enabled state at weight and speed one with `ClampForever`.
Use active playback components on a fully materialized hierarchy.
The capture pins the API, wrap configuration, built loop flag, clip
length and frame rate. Each sample uses a fresh bundle prefab instance.
[Unity AnimationClipPlayable](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Animations.AnimationClipPlayable.html)
· [Unity Animation.Sample](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Animation.Sample.html)

For each requested local time `l`, record `local`, the f32 bits
`local_f32` given to Unity, the requested `source`, the modeled
`effective_source` and one side for each sorted `held_boundaries` entry.
The boundaries are source seconds for all held tracks. The recorder uses
`source = start + l`, substituting the exact serialized stop for the last
sample. The API receives `float(l)`. For `source_clock = widened`, add
that f32 value to the serialized start in f64. For `f32_add` round that
sum to f32 and require an exact f32 start. Clamp to the serialized
interval. At `float(l) == clip.length`, the endpoint convention selects
the exact stored stop.

The public playable or legacy time checks the supplied time. The
effective time and side labels express the selected fixture clock model.
The model selects `left` before a boundary and `right` on equality or
after it. Confirm both the endpoint behavior and the native clock with
independent Editor probes. Use recognizable held values on both sides,
including a time that changes sides after f32 rounding. Record the
observed values that justify the clock and side labels in `evidence`.
Unknown clock behavior needs more evidence before L11 comparison.

Record the first and last samples and off-key times for nonzero duration.
Include the cases in [plan section 8](plans/anim.md#8-editor-fixture-project-owner-built).
All local TRS and renderer weights are eight-digit IEEE 754 f32 bit
strings, widened exactly on read. Include the pose of the reset rig.
Quaternions are XYZW, transforms are in Unity space and weights keep
the Editor's units.

### Humanoid qualification

Record each candidate's complete body, muscle, goal and translation slot
inventory with `dof`, `kind`, serialized `index_value` and `storage`
(`streamed`, `dense`, `constant` or `absent`). Record the absent sentinel,
the storage ranges, and stored axis types, pre/post quaternions, signs
and limits. Use `RootT.x` through `RootT.z` and `RootQ.x` through
`RootQ.w` as the canonical body DOF labels. For axis fields exposed by
the loaded Avatar's Editor SerializedObject, list their property paths in `editor_axis_properties`
to capture integer, f32, Vector3 or quaternion values independently.
The recorder also captures the human bone-to-node map, optional
absences, public muscle-to-bone and axis map, limits and rest body pose.

For supplemental playback use `mode: "human_pose"` and place a known
muscle vector on each schedule point in `HumanTrait` order. The clocks
label these probes. The recorder converts the fresh rig's body pose from
normalized world space to normalized Animator-root space before applying
the muscles with `HumanPoseHandler.SetHumanPose`. It records the exact
vector bits applied. Use neutral, single-axis and noncommuting multi-axis
vectors across rigs of different proportions, with asymmetric limits,
an optional absent bone, and identity and nonidentity Animator roots.
Use the effective axes from those probes to interpret the runtime
records. [Unity GetHumanPose](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPoseHandler.GetHumanPose.html)
· [Unity SetHumanPose](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPoseHandler.SetHumanPose.html)

Start with `disposition: "pending"`. Qualification references the
known-vector or reset/muscle-only baseline by digest, root and hips
node indexes, and comparison result. Preserve the root and hips-local
component and angular residuals and their bounds with the evidence.
Use `qualified_muscle_only` only when the complete body inventory
is absent and the settings and playback require neither projection nor
body-to-hips correction. A mapped constant body slot counts as
present. Record the typed `unsupported_…` causes for refusal probes.

### Owner workflow

1. Build original fixture bundles in the [owner-built project](plans/static3d.md#8-the-editor-fixture-project-owner-built)
   with a documented 2022.3 or 6000.x patch and active target. Place the
   ordered bundle set beside the recording spec. Check the identity
   crosswalk, timing and settings against these exact artifacts.
2. Prepare the spec and include endpoints, off-key and held-boundary
   times. Run `python3 -I tools/parity/anim_replay.py validate-spec synthetic-spec.json`.
3. Copy [AnimReplayRecorder.cs](../tools/fixtures/AnimReplayRecorder.cs)
   into `Assets/Editor`. In Edit Mode choose **Tools → unityrs → Record
   animation replay**. Open the validated spec and save a new replay file.
   Verify the endpoint behavior and held probes. Run
   `python3 -I tools/parity/anim_replay.py validate synthetic-replay.json --bundle-dir .`
   with the bundle directory to recheck the artifact bytes.
4. Freeze the budgets and the resulting tolerance document as described
   below. Keep the original fixture bundles and numeric records under
   the repository's fixture rules. Record digest and project-relative
   keys without machine-specific paths.

### Per-recording f32 tolerances

Before comparing Rust output, bound each recorded property along its
layout-specific playback path. For each Transform give three budget rows
(`translation`, `rotation`, `scale`) with `target` equal to the node index
and `channel: -1`. Each weight budget targets a renderer index and its
channel index. Include held, constant and rest-valued properties. Use one
conservative envelope per row for the rest-to-output arithmetic and
scheduled continuous intervals. Preserve the path and range proof with
the budget. Repeat capture can check determinism independently of the
tolerance.

For each stage give the operation count `n`, a cancellation-safe
absolute scale `M`, downstream amplification `gain` and a bound for
additional elementary-function or solver error. The stage contributes
`(γ32(n)*M + n*2^-126 + elementary_error)*gain`, where
`γ32(n) = n*2^-24 / (1-n*2^-24)`. The absolute term allows underflow
flush-to-zero. Scale and gain must bound propagation of intermediate
errors to the stage output. Use the equations above to identify stages
for interpolation, axis and pose composition, and normalization. For
example, a streamed cubic has six Horner operations after time
subtraction, with scale bounded by
`abs(c3)*D³ + abs(c2)*D² + abs(c1)*D + abs(c0)`. If the native evaluator
rounds key-time subtraction, count that separately. For a weighted
segment near a flat inverse, enclose time parameters with the bounds
for the actual f32 solver and include the value enclosure. Justify the
native elementary-function bounds over the recorded ranges.

The per-sample clock displacement is `abs(effective_source-source)`.
Multiply by `rate`, the bound on the continuous derivative magnitude
over that displacement. For a cubic, use
`3*abs(c3)*D² + 2*abs(c2)*D + abs(c1)`. At a held crossing compare the
reference side selected by the effective time and budget only continuous
motion in the derivative term. Replaying all tracks at the effective time
also fits that budget. For an unbounded rate use an interval-based value
enclosure as a stage error. Establish how further native time rounding
affects the selected side before certifying the model.

Add the stages and time contributions. Apply `output_gain` for conversion
of internal weight units to Editor weight units, or use one for values
already in the output units. Add half an f32 ulp at the largest absolute
captured component for final storage. For rotation use raw component
error with a positive `min_norm` before normalization. Include the
arithmetic of native normalization and pose composition in the stage
proof. If the full path is bounded in unit quaternion space, use
`min_norm = 1`. Convert the final store separately with norm one,
sum the two angular bounds, and include the f64 angle metric floor.

Run `python3 -I tools/parity/anim_replay.py derive synthetic-replay.json --bundle-dir . > synthetic-tolerances.json`.
The result contains the manifest SHA-256 (sorted compact ASCII JSON),
ordered property, target and channel keys, and per-sample clocks, held
sides and bounds. Freeze it alongside the inputs before comparison.
Investigate discrepancies against the recorded layout, settings and
equations.
