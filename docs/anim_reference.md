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
interval and the exact start and stop, each once. Drop an interior sample
when its f32 local time repeats a preceding time or reaches the f32 local
stop. The remaining local times increase strictly after rounding. A
zero-duration clip has one sample. A schedule or rate override still requires a positive,
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
binding table before indexing those ranges. Where the stream has a
`discreteCurveCount`, zero included, the streamed range holds `curveCount +
discreteCurveCount` slots in serialized binding order, and its discrete
bindings must number exactly `discreteCurveCount`. Dense and then constant
slots follow. A built 2022.3 capture confirms a stream of continuous slots
followed by a discrete PPtr slot. Discrete slots beside dense or constant
storage follow the format and are unverified by a real capture. Public readers support the
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
  first real key is later than the clip start. Built 2022.3 clips seed
  every slot, continuous ones with zero slopes and their first key's
  value. Accept any finite seed, since samples at and after the first real
  key do not read it. Before the first real key, hold a continuous seed
  only in that flat, first-key-equal pattern (signed zeros compare equal),
  or when it is flat and the slot has no real key. Refuse any other
  continuous seed there as unverified: a cubic from `-FLT_MAX` has no
  confirmed playback value. Refuse a seed for a slot outside the stream, a
  repeated seed slot and a non-finite coefficient. An empty slot needs a
  supported initializer. Validate the terminal dummy separately and
  exclude it from the real keys and duration.
- Dense. Frame `i` is at `g(i) = m_BeginTime + i / m_SampleRate`, using
  the dense layout's rate. Read `A[i*curve_count + slot]`. With
  `g(i) <= sample < g(i+1)`, let `u = min(1, (sample - g(i))*rate)` and evaluate
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

Tangents are derivatives per second. Use the stored value at each key.
For a short explicit curve, use that side's `m_PreInfinity` or
`m_PostInfinity` code outside the key range. The boundary rule is
**serialized 2 = clamp; others refused until confirmed by fixture**.
An out-of-range sample with any other code raises `unsupported_curve_wrap`.
[Unity AnimationCurve pre-wrap](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AnimationCurve-preWrapMode.html) · [Unity AnimationCurve post-wrap](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AnimationCurve-postWrapMode.html)

For both Hermite and weighted segments, an infinite left `outSlope`
or right `inSlope` holds `v0` until the right key, even if the other
slope is finite. Use `v1` at the right key. Refuse NaN slopes. The
one-sided infinity convention is to confirm by fixture. The Hermite
bases and quaternion normalization follow the public interpolation
formula. [Khronos interpolation appendix](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#appendix-c-animation-sampler-interpolation-modes)

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
isolated zero derivative. Use the same key and wrap checks as for Hermite.
Bisect `[0,1]` for up to 64 steps using
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
use ZXY only for a layout with confirmed default behavior. A compiled
Euler binding (Transform attribute 4) stores its order in `customType`,
in the same enum; refuse other values. Built 2022.3 generic clips store
4, and their replays match ZXY and no other order. A compiled
blend-shape binding of a built 2022.3 clip stores the CRC32 of the bare
channel name. AssetStudio's model converter hashes the
`blendShape.<name>` property instead. Accept either, and refuse a hash
that names two channels under the two conventions as ambiguous.
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

### Synthetic sampler replay

The testkit emits explicit and compiled clips with a two-second source
origin. The Rust replay binds their Transform and morph channels and
compares 18 sampled poses against the independent f64 evaluator. The
fixtures cover fractional scalar weights, an infinite one-sided tangent,
stored Euler order and authored turns, sparse cubic streams, frame-major
dense quaternions and morph weights, and constants. The serialized field
fixtures and stripped TPK check exercise the read path separately.

Regenerate the expected values and inverse-time bounds from the repository
root with:

```sh
python3 -I tools/parity/tests/test_anim_synthetic.py --generate
```

The [generation test](../tools/parity/tests/test_anim_synthetic.py) checks
the JSON for drift. The [Rust replay](../crates/unityrs-anim/tests/clip/replay.rs)
compares stored Hermite endpoints exactly and uses the raw quaternion norm
of each rotation sample, with the frozen norm as its floor. Same-bundle
Editor playback remains the L11 gate.

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

A completed replay document has a `spec` and recorder-generated `capture`
under a versioned [JSON Schema](../tools/parity/anim_replay.schema.json).
The [synthetic replay test](../tools/parity/tests/test_anim_replay.py) shows
both halves on a one-node prefab with two renderers sharing a mesh.

### Trust model

The replay manifest is trusted provenance from our own recorder and
digest-pinned bundles. The Python validator checks internal consistency
to catch recorder and operator mistakes. Deliberate manifest forgery is
outside this threat model.

#### Rust-side cross-check obligations

PR 6 (generic and legacy) and PR 8 (humanoid) must read those same
digest-pinned bundles and perform the applicable checks before promoting
any L11 row.

1. Derive the complete key-time inventory from the bundle and compare it
   with the manifest. Use the bundle keys to determine the off-key status
   of each requested and effective sample time, including after f32
   rounding, and confirm the required interior probe.
2. Verify the source origin against the clip layout, including serialized
   `m_StartTime` where present.
3. Verify the recorded body, muscle, goal and translation slot inventory,
   mapping, absent sentinels and storage counts and ranges against the
   serialized Avatar and clip.
4. Verify the identities and values of the start and stop fields used
   for endpoint replay, or the endpoint derivation for layouts that use
   key bounds.
5. Verify that every recorded stage range, including rotation output
   boxes, encloses the clip's actual values along the declared playback path.
6. Verify that each budget's `track_kind`, and so its fixed stage path,
   matches the playback path derived from the bundle for that track.

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
- Inspect the exact hashed artifacts before completing a recording. Check the bundle
  container PPtr for each selected prefab, clip and Avatar asset key.
  From the prefab GameObject, walk Transform child and GameObject
  component PPtrs in stored order to establish the address-to-ID crosswalk
  for the Transform nodes, Animation or Animator and SkinnedMeshRenderers.
  Follow the Animator's Avatar and each renderer's mesh PPtr. Retain this
  mapping in the spec as the built-ID evidence. Public playback APIs select
  objects by asset key and hierarchy without exposing built file/path IDs.
  Use one prefab root, clip or Avatar of the needed type per asset key.
  Capture checks those lookups, the selected clip and node names, the
  bound Avatar object and shared mesh relationships.
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
The capture pins the API, wrap configuration, built loop and human-motion
flags, clip length and frame rate. Each sample uses a fresh bundle prefab instance.
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
interval. At `float(l) == float(stop - start)`, the endpoint convention
selects the exact stored stop.

Unity 2022.3 reports `AnimationClip.length` for a Mecanim clip whose
interval starts late (`start > 0`) as `stop`, measured from zero, while
its built `m_MuscleClip` keeps the interval and its keys in source
seconds. Playback still starts at the interval start. An Editor fixture
with stepped keys at 0.5 and 0.75 on the interval 0.5 to 1.5, sampled at
playable times from 0 to 1.5, read the left value only at playable 0 and
the right value from playable 0.5 on. Reading playable time as source
time would have kept the left value up to 0.75. The capture records the
reported length: `float(stop)` for such a clip, and `float(stop - start)`
for a legacy clip or one starting at zero. Validation refuses any other
length. For a late-start Animator clip, a held probe must also fall on
the other side from where a zero origin would put it, so the probes
either side of playable `boundary - start` are required.

The public playable or legacy time checks the supplied time. Use the
`source_clock` enum to declare the clock model. Inspect the source
endpoints independently and record their source seconds and field names
in `evidence.clock.start` and `stop`. Include all inspected key times
inside the source interval, held boundaries and both endpoints in
`key_times`. The Editor handoff also preserves its authored times in
`authored_key_times`. For a constant clip use the interval endpoints as the
comparison keys. Supply the first and last samples and at least one
effective f32 interior sample strictly between distinct keys. The
sample must remain off-key under the selected source clock.

Each endpoint also carries a probe with the schedule index, property,
target, channel, component and independently known f32 value. Humanoid
fixtures retain the observed component bits after checking the full local
Spine rotation against the authored endpoint pose. For a
Transform use `channel: -1` and its zero-based component index. For a
weight use the channel index and component zero. Include one `held` row
per boundary with its source time and probes at the last effective sample
before the jump and the first one at or after it. A jump at the clip
start needs only the right probe. Use a held component with distinct,
independently known values on its two sides. The validator compares the
expected values with the captured pose and predicts the held sides under
both clock models. A recording is clock-independent when both candidate
clocks give every sample a bit-identical effective source time, as when
each `start + f32(local)` is exact in f32 (a zero start, for one). Then
`derive` sets `clock_discriminated` to `"independent"`, and no held probe is
needed to choose between the models. Otherwise, when no held probe changes
sides between the models, `validate` reports `clock not discriminated` for
clip playback and `derive` sets `clock_discriminated` to false. Promotion
requires a clock-independent recording, or at least one held probe whose
independently observed side distinguishes the models, and the matching
`"independent"` or true `clock_discriminated` in the frozen tolerances used
as clock evidence. The Rust comparison recomputes both clocks per sample
and refuses a recording whose clocks differ anywhere without a
discriminating probe. A late-start Mecanim interval still needs its own
held probe against a zero origin.

Include the cases in [plan section 8](plans/anim.md#8-editor-fixture-project-owner-built).
All local TRS and renderer weights are eight-digit IEEE 754 f32 bit
strings, widened exactly on read. Include the pose of the reset rig.
Quaternions are XYZW, transforms are in Unity space and weights keep
the Editor's units.

### Humanoid qualification

Record each candidate's complete body, muscle, goal and translation slot
inventory with `dof`, `kind`, serialized `index_value` and `storage`
(`streamed`, `dense`, `constant` or `absent`). Use `storage_counts`
for the consecutive streamed, dense and constant slot ranges. Record the
absent sentinel and stored axis types, pre/post quaternions, signs
and limits. Use `RootT.x` through `RootT.z` and `RootQ.x` through
`RootQ.w` as the canonical body DOF labels. Include all 95 muscle slots
in the supported HumanTrait order, including absent slots. The validator
matches their names to the recorded inventory and checks runtime axes for
mapped muscles. Both the goal and translation kinds require explicit rows,
using the absent sentinel and `storage: "absent"` for each unmapped slot.
For axis fields exposed by
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

Start with `disposition: "pending"`. For qualification, identify the
distinct Animator root and mapped hips node indexes on an observed
human-motion clip. Record a separate `human_pose` replay on the same
built rig and Editor version with the same f32 sample schedule. Run
`python3 -I tools/parity/anim_replay.py capture-id known-vector-replay.json`
to get its ID, the SHA-256 of the complete standalone recording as sorted
compact UTF-8 JSON. Put this ID in
`evidence.humanoid.root_hips_comparison.baseline_capture_id` in the clip
spec. Once the clip is recorded, attach the complete known-vector replay
document as an entry in the clip manifest's top-level `baseline_captures`
array. Keep the original recorder output with the fixture.

The validator resolves the ID, checks `SetHumanPose` playback, rig
identities and sample times, and reads the root and hips-local poses
directly from the referenced capture. It measures componentwise and
angular residuals against the frozen replay budgets. Equal root and
hips-local samples across the two captures give a zero residual.

The `qualified_muscle_only` profile requires absent body slots (including
mapped constant slots as present), only muscle writers, and neutral
settings. Supply both effective and serialized values for the built and
authoring settings in the profile. The implemented profile requires zero
rotation, height and cycle offsets and twist, and disables loop, bake,
blend, original/feet bases, start-at-origin, mirror, translation DOFs and
additive playback. Clip and Avatar values exposed by the recorder must
match the corresponding effective entries. Keep settings with unconfirmed
layout defaults or twist behavior pending until the rig fixtures establish
them. Record the typed `unsupported_…` causes for refusal probes.
[Unity clip import settings](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/ModelImporterClipAnimation.html)

### Owner workflow

Follow the [Windows Editor steps](../tools/fixtures/unity/README.md). Run
**Tools → unityrs → Build and record all fixtures** first in a new Unity
2022.3 LTS 3D (Built-in) project and later in a separate new Unity 6000.x
project. The button uses the active build target, which defaults to
`StandaloneWindows64` on the owner's PC. Send each complete
version-and-target folder to the maintainer's fixture inbox. If the run
stops with **unityrs fixture build failed**, send the log from the separate
`-failed-<id>` folder shown in the dialog.

The button generates both humanoid rigs and their clips. Each rig has
skinned boxes in a T-pose and a procedurally built Avatar. The taller rig
has asymmetric Spine limits and omits UpperChest. Each rig has an
identity and a translated, rotated Animator root. The recorder pairs a
candidate muscle-only clip with a known-vector `SetHumanPose` replay on
each prefab and uses the same five sample times for the pair. A separate
clip on the compact rig adds `RootT` and `RootQ` body curves. An invalid
generated Avatar is reported in the log and generation continues.

Recordings use asset keys, hierarchy addresses and component ordinals.
The recorder selects endpoints, off-key samples and held probes from the
authored keys and records the Editor-visible clip settings and curve
bindings. Repo-side completion fills in the built file and path IDs.

#### Repo-side completion

The Unity project owner sends the version folder. Point `complete` at the
target directory within it (`<version>/<target>/`), which contains `run.json`,
`build.log`, `bundles/` and `replays/`. For example, after unpacking the
`2022.3.62f3` folder under `returned/`, run the following command.

```sh
python3 -I tools/parity/anim_replay.py complete "returned/2022.3.62f3/StandaloneWindows64/" --unityrs target/debug/unityrs
```

Omit `--unityrs` to use `unityrs` on PATH. Completion requires the inventory
in `run.json` and a recorded result for every required case, including the
static bundles and raw known-vector recordings. It reports missing and failed
cases before writing completed replays. The [fixture contract](../tools/parity/anim_fixture_contract.py)
specifies each required bundle, asset key, playback route and evidence.
Completion matches the run ID, Editor version and build target across the
manifest, log, recordings and built bundles. Each bundle stores the stamp as
a TextAsset or, for the scene, as an empty GameObject. The built clip curves,
prefab components and scene references supply the required fixture evidence.
Static asset keys are checked against the inspected AssetBundle container.
A scene's container entry names its `.unity` path and has a null object
pointer. [Unity AssetBundle format](https://github.com/Unity-Technologies/UnityDataTools/blob/main/Documentation/assetbundle-format.md#scenes-in-assetbundles)

Optional coverage can be skipped with
a reason. These sections are URP materials, packed rotation slopes,
authored/runtime precedence, verified mixed Transform storage, integer discrete
bindings, humanoid twist and non-neutral setting refusals, humanoid slot/axis
inventory and the root/hips comparison. `build.log` and `run.json` give the
reason for uncovered sections and fixture errors. The `*.pending.json` files use
the separate `unityrs.anim_replay_pending` format, version 1, and stay
beside the completed files. Each pending spec has one bundle basename
without a digest, uses integer mesh groups where renderers share a mesh,
and omits built identities. The capture pins the digest of the bytes
Unity loaded. Completion reads `inspect` for file occurrences and class
headers, then uses `show` for the actual container entries, Transform and
component PPtrs, mesh and Avatar PPtrs, controller slots, and clip settings.
It inventories explicit and packed keys, streamed frame times and the dense
grid from the serialized clip. A packed time array represented only by its
length and digest leaves the inventory incomplete, so completion refuses it.
The recording must include an interior sample
off both the authored and built key times after f32 rounding.
The automated fixtures keep these PPtrs within one bundle. Completion
verifies the digest, writes the existing `unityrs.anim_replay` version 1
format and runs its validator. The validator refuses pending recordings.

For a saved inspection, pass `--inspect-dir DIR`. Completion trusts sidecars
produced by our own unityrs tooling. The bundle digest catches stale or
mismatched sidecars and provides no protection against forgery. Use an unfiltered,
complete `unityrs inspect BUNDLE --limit 10000 --max-bytes 1048576 --external-paths` response
named `BUNDLE.inspect.json`. Run inspection with the bundle basename
from inside `bundles/`. Save each needed
`unityrs show BUNDLE --file-index 0:FILE_INDEX --path-id PATH_ID --value canonical --max-items 4096 --max-bytes 4194304`
response as `BUNDLE.FILE_INDEX.PATH_ID.show.json`.
For the four-influence skin meshes in `static-objects` and `anim-generic`,
use `--value natural --bytes full` in place of `--value canonical` to preserve
the vertex bytes. Add a top-level
`bundle_sha256` field to the inspect response and each show response, using
the SHA-256 of the bundle bytes inspected at the time of capture. Preserve
exact file occurrences and signed path IDs. Completion refuses cut values,
nonzero show truncation counters and missing or mismatched digests.

The raw `human_pose` recordings are under `replays/known-vectors/`.
Each contains the inputs in HumanTrait order, the exact applied f32
bits and the independently captured rig facts. Completion checks their
exact five-point schedule, muscle vectors, and agreement with the corresponding
clip replay's inspected assets, rest pose and Editor capture facts. To complete a known-vector baseline, preserve a
copy of the raw recording and add the inspected `spec.evidence.humanoid` to
the pending file under `replays/known-vectors/` for its selected
clip with `disposition: "pending"` and the slot and axis evidence
described in [Humanoid qualification](#humanoid-qualification).
Run completion on that copy, then use the standalone result to compute
the baseline ID and attach it to the candidate clip recording as
described above.

On the first run, confirm that `StaticBatchingUtility.Combine` persists
the shared mesh and submesh ranges in the scene bundle, and that
`LoadAssetWithSubAssets<GameObject>` returns the prefab root and hierarchy.
In the animation bundles, check how `AnimationClip.SetCurve`,
`AnimationUtility.SetAnimationClipSettings`, `EditorCurveBinding.PPtrCurve`
and `AnimatorOverrideController.ApplyOverrides` serialize the authored
curves, nonzero interval, object-reference binding, and null substitution. Check
the native playback endpoints and held probes for the actual build.
Confirm `AvatarBuilder.BuildHumanAvatar` accepts the generated T-poses
and public `HumanDescription` fields and that the built Avatars retain
their mappings, limits and optional absences. Confirm the Animator
muscle property names and `RootT`/`RootQ` produce the expected
`humanMotion` clips. The recorder compares built local Spine endpoint rotations
to `SetHumanPose` evaluations of the authored muscle vectors on the source rig.
[Unity AvatarBuilder](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AvatarBuilder.BuildHumanAvatar.html)
· [Unity HumanDescription](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanDescription.html)
· [Unity AnimationClip.SetCurve](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AnimationClip.SetCurve.html)

Unity chooses the serialized curve layout. Check compiled storage,
packed rotations and the complete runtime key inventory using the
same-bundle cross-checks. Humanoid qualification starts pending until the
built slot inventory, axes, serialized settings and baseline comparison
have been checked. Freeze the budgets and derive the tolerances below
before comparing Rust output. Keep original built fixtures and their
numeric records under the repository's fixture rules, with no
machine-specific paths.

For a manual case, copy the same [Editor folder](../tools/fixtures/unity/Editor).
Put the validated spec beside its bundle set and choose **Tools → unityrs
→ Record animation replay**. Open the spec and save a new replay file.
The manual and automated paths share
[`AnimReplayRecorder.cs`](../tools/fixtures/unity/Editor/AnimReplayRecorder.cs).
`anim_replay.py validate-spec` checks the input and `anim_replay.py validate
REPLAY --bundle-dir BUNDLE_DIR` checks the result and artifact bytes.
**Tools → unityrs → Test animation replay prefab loading** builds a
temporary three-node prefab bundle and checks root lookup and a nested
address. [`AnimReplayRecorderSmokeTest.cs`](../tools/fixtures/unity/Editor/AnimReplayRecorderSmokeTest.cs)

### Per-recording f32 tolerances

Before comparing Rust output, bound each recorded property along its
layout-specific playback path. For each Transform give three budget rows
(`translation`, `rotation`, `scale`) with `target` equal to the node index
and `channel: -1`. Each weight budget targets a renderer index and its
channel index. Include held, constant and rest-valued properties. Use one
conservative envelope per row for the rest-to-output arithmetic and
scheduled continuous intervals. Identify each step by its `stage` enum
and supply the inclusive `[lo, hi]` intervals in `range` in the table's
order. Derive the ranges from the inspected inputs and rest pose across
the complete playback path, including all covered components and both
clock times. Retain the inspected sources used to establish coverage.

Set `track_kind` on every budget row. Each kind fixes one ordered stage
sequence, with each stage appearing once.

| Track kind | Stage sequence |
|---|---|
| `rest`, `constant`, `held`, `streamed`, `dense`, `hermite`, `weighted` | The single stage of the same name. |
| `euler_constant`, `euler_held`, `euler_streamed`, `euler_dense`, `euler_hermite`, `euler_weighted` | The named input stage, then `joint`, for rotation. |
| `muscle_constant`, `muscle_streamed`, `muscle_dense` | The named input stage, then `muscle`, then `joint`, for rotation. |

A streamed track whose key intervals mix held (zero `c3`, `c2` and `c1`)
and streamed segments has track kind `streamed`. A held segment is the
streamed cubic with zero slopes, so the streamed stage bounds it, and its
value lies in the `c0` range. Other mixed-stage inputs need a documented
track kind and a matching budget derivation before promotion.

`tools/parity/anim_replay_budgets.py REPLAY --bundle-dir DIR` writes the
budget rows of a completed generic or legacy replay without them. It reads
the replay's digest-pinned bundles through `unityrs show`: the serialized
rest pose, the compiled or explicit curves and the playback interval,
never the observed poses. It walks each interval between the start, the
stop and every key strictly inside them, and records every coefficient,
key-relative time, Hermite control and clamped or rest value the path
reads. It supports rest, constant, held and streamed compiled slots,
clamped explicit Hermite curves and Euler input through `joint`, and
refuses dense, weighted and humanoid paths until they have an author.
Its choices, provisional until review:

- Each recorded range is padded outward by `2^-40` of its magnitude plus
  `2^-140`, and a nonnegative bound stays nonnegative. The tool computes
  Hermite values, Euler quaternions and derivative pads in f64. `2^-40`
  covers about 4096 f64 roundings of that arithmetic, yet is `2^-16` of an
  f32 ulp, so the derived bounds do not grow measurably. The absolute
  term keeps exact zeros, where the relative term vanishes, strictly
  inside their range. It is below f32's smallest normal, so it adds
  nothing an f32 output can show.
- Rotation `norm_ranges` split each key interval into 64 equal pieces,
  and box each piece by its endpoint quaternions widened by the piece's
  width times a derivative bound. The derivative bound is the stage's,
  or `π/360` per degree per second summed over the Euler axes. 64 is a power of
  two, so the pieces fall on the midpoint subdivision the Rust audit uses
  to fit its own pieces into a box. With 64 pieces, each box spans at most
  1/64 of the interval's motion, so the norm floor stays near one for
  ordinary clips, at the cost of 64 boxes per interval and rotation target.

| Stage | Recorded ranges in order | Continuous derivative bound |
|---|---|---|
| `rest`, `constant`, `held` | Value | Zero on the selected side. |
| `streamed` | `c3, c2, c1, c0, d` with nonnegative key-relative time `d` | `3*abs(c3)*D² + 2*abs(c2)*D + abs(c1)` with `D = max(d)`. |
| `dense` | Adjacent values `a, b`, grid rate, sample time, grid begin, grid offset `i/rate` | `max(abs(b-a))*max(rate)`. |
| `hermite` | `v0, v1, outSlope, inSlope, h`, with `min(h) >= 2^-126` | `1.5*max(abs(v1-v0))/min(h) + max(abs(outSlope)) + max(abs(inSlope))` from the derivatives of the Hermite bases. |
| `weighted` | Hermite ranges followed by `outWeight, inWeight` | Use the time inverse enclosure below. |
| `muscle` | Normalized muscle value and signed degree limits | Forward component error and rate with gain `max(abs(limit))`. |
| `joint` | Euler angles in degrees | Forward the maximum input component error and rate with gain `3π/360` into raw quaternion components. |

For each curve stage take the operation count and cancellation-safe
scale from the transcription table, using maxima over the ranges
and the largest difference between opposite interval endpoints. Use
`γ32(n) = n*2^-24 / (1-n*2^-24)` and add `n*2^-126` for
flush-to-zero, scaled by conservative propagation through the stage
for time subtraction, Horner multiplies, grid interpolation or Hermite
time division. Hermite and weighted budgets require normal f32 segment
durations so their divisors survive flush-to-zero. The exact rest,
constant and held stages contribute zero. Multiply each continuous rate
by the clock displacement `abs(effective_source-source)`. At a held
crossing compare the reference on the effective side and budget
continuous motion only.

For a confirmed weighted solver, add its normalized time uncertainty
to clock displacement divided by the shortest segment. The budget
allows twice the 64-operation f32 time bound plus `2^-23` for solver
termination and parameter rounding. For fractional weights, a parameter
change of width `w` changes the normalized time cubic by at least `w³`.
For fixed parameters the time difference is affine in the weights, so it
suffices to check the four corners of the weight square. The two endpoint
cubes and the midpoint-flat cube attain this bound. The remaining
smoothstep cubic gives its smallest change over an interval of width
`w` at either endpoint of the parameter domain and gives
`w²(3-2w) >= w³`. Bound the parameter displacement by the cube root of
the time uncertainty, capped at one. Where all three derivative
controls admit a positive lower bound, also divide the time uncertainty
by that bound and use the tighter estimate. Multiply the result by
`3*(max(abs(v1-v0)) + max(h)*(max(abs(outSlope))+max(abs(inSlope))))`,
which bounds the value derivative by the largest adjacent Bézier
control difference. Confirm the native f32 solver fits this time
enclosure before using the stage for L11.

Forward the input stage error through any muscle and joint stages in the
track kind. Add their local roundoff and the joint elementary
error (six trig evaluations at up to two f32 ulps each with conservative
propagation). The joint formula assumes normalized rest and axis
quaternions whose multiplication preserves Euclidean length. Each of
the three Euler factors contributes at most a half-angle of quaternion
displacement. Set `weight_units` to `editor` or `fraction_of_100` to
select the conversion gain of one or 100. The percent multiply has
an additional `(100+1)*2^-126` allowance for input and result flushing.

For rotation, put `norm_ranges` on the final stage of that same budget
path. The four-component XYZW boxes cover that stage's raw quaternion
output, including off-key values. Subdivide the path if a single box
includes zero.
Within each box the largest guaranteed absolute component is a lower
bound on the norm. Use the smallest such bound across the path.
The boxes must establish a floor of at least `2^-60` and an upper
component magnitude no greater than `2^60` to keep f32 length
arithmetic in range. For a `joint` output each box must admit a unit
quaternion, and the floor is capped at one. Convert propagated raw
component error with this norm floor. Budget normalization with 32
operations in unit space. Add half an f32 ulp at the largest captured
component for final storage. For rotation, convert the normalization and storage
errors with norm one, sum the two angular budgets and include the f64
angle metric floor. Refuse any rotation tolerance that reaches π.

Run `python3 -I tools/parity/anim_replay.py derive synthetic-replay.json --bundle-dir . > synthetic-tolerances.json`.
The result contains the manifest SHA-256 (sorted compact UTF-8 JSON),
`clock_discriminated` (true, false or `"independent"`), ordered property, target and channel keys, and
per-sample clocks, held sides and bounds. Freeze it alongside the inputs
before comparison.
Investigate discrepancies against the recorded layout, settings and
equations.

After deriving and freezing the tolerances, run the Rust comparison from
the repository root:

```sh
UNITYRS_ANIM_REPLAY=synthetic-replay.json \
UNITYRS_ANIM_BUNDLES=. \
UNITYRS_ANIM_TOLERANCES=synthetic-tolerances.json \
cargo test -p unityrs-export --features gltf --test anim_replay \
  recorded_generic_or_legacy -- --ignored --nocapture
```

The same-bundle comparator checks the captured rig, clip and renderer
identities, derives clocks and ranges from serialized playback, and reports
worst Unity-space errors by rig, node or renderer, property and source time.
The ungated synthetic tests exercise both clip modes with testkit bundles.
L11 status requires the owner-built Editor recordings.
