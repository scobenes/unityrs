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

**PR 8's outcome.** `tools/parity/anim_humanoid_qualify.py FOLDER --unityrs BIN -o OUT`
reads a completed run's humanoid clip recordings, their known-vector
baselines and the same built bundles through `unityrs show`. The clip set is
the run's: every required humanoid clip and every optional coverage clip
`run.json` marks recorded. A missing replay, or an empty or partial set,
fails. Per clip it writes the slot inventory, the crosswalk against the
recorder's `HumanTrait` order (each built binding at its generic width), the
muscles of absent optional bones, the generic Transform writers on human
skeleton nodes, the root and hips-local residuals against the reset rig and
the baseline, and the muscle-to-pose residual of the candidate equation in
`muscle_rotation`.

It then gives three independent verdicts:

- **Clip settings.** Each non-basis setting (`m_Mirror`, `m_CycleOffset`,
  `m_Level`, the loop flags and the orientation offset) at its neutral
  value.
- **Root basis.** The keep-original orientation, Y, XZ and height-from-feet
  flags off and `m_StartAtOrigin` on. The Editor's
  `AnimationClipSettings` has no field for `m_StartAtOrigin`, and every
  built clip of the basis matrix stores it on, whatever its XZ flag, so the
  neutral profile has it on and the XZ flag off. The Rust reader refuses an
  unverified basis as `unsupported_humanoid_root_basis(field)`, apart from
  the non-neutral settings.
  [Root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html),
  [AnimationClipSettings](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AnimationClipSettings.html)
- **Residual.** The root and hips-local samples against the reset rig,
  within the bound `anim_replay.py derive` gives an unanimated (rest)
  track of that node: the rows `anim_replay_budgets.py` authors for a
  Transform property no curve reads, through `budget_model` and `_bound`.
  That is half an f32 ulp at each rest component's magnitude (the
  subnormal half ulp at zero), widened by `1 + 2^-45` as every derived
  bound is; for rotation the box's raw error and that storage error plus
  the f32 unit roundoff map to an angle with `angular_bound`. A hips residual over its bound
  is `unsupported_body_motion(hips_correction)`, a root residual
  `unsupported_body_motion(root_projection)`.

The evidence lists every typed cause a clip has, unordered. A clip
qualifies only with no cause and a baseline within its bound: the
known-vector `SetHumanPose` recording of the same prefab, clip, Animator,
Avatar, hierarchy and reset rig, whose own root stays at its rest and which
the clip's root and hips match within the rest-track bounds. A root basis
matrix clip has no known-vector recording, so its baseline is its own
prefab's reset rig, which the residual verdict compares. A qualifying
clip's verdict is `qualified_pending_playback`. The Rust reader cannot see
recordings, so it never reports a residual cause; it returns one static
cause, in the partial order the plan implies: identity before
the rig (Avatar, hierarchy, required bones), the rig before the Animator
root and conflicting generic writers, those before the slots it plays
(locomotion, IK, translation DOFs, additive pose, body slots, muscle axes),
the slots before the Avatar settings that act on them (twist and stretch,
only where the clip animates a bone the setting redistributes or
lengthens), and those before the clip's own settings and its root basis.
A clip with none of these refuses as `unsupported_humanoid_playback`,
since positive playback is not built.
The Rust test `recorded_humanoid_refusals` (`UNITYRS_ANIM_HUMANOID`,
`UNITYRS_ANIM_BUNDLES`, with `run.json` beside the bundles folder) reads
the same bundles, checks the run's clip set, the inventory, settings,
Avatar twist and stretch, the dropped muscles and the neutral twin's
values, runs `humanoid::body_avatar` on each Avatar (its masses, slot paths
and rest body must pass, or its topology refuse as outside the body model's
scope), binds each clip to its own completed replay (bundle digests,
prefab, clip, Animator and Avatar) and the `-offset` clips to a translated
and rotated Animator rest. Its refusal must be one of the listed causes, a
refusal probe's carrying the probe's code, or
`unsupported_humanoid_playback` for a qualified clip or one whose causes
are all residual. The neutral twist and stretch are
the values on the recorded probes' neutral Avatar, not an assumed zero.

On the 2022.3 and 6000 fixture runs no clip qualified, and no muscle clip's
hips residual is within its bound: absent body slots leave the hips at the
Animator root origin, the baseline's hips less the Avatar's `m_RootX`
translation. The body-motion clip maps body slots. The builder's muscle
clips also keep the original Y position, which refuses first as a root
basis. The candidate equation matched the known-vector Spine poses on both
rigs. The optional `humanoid-root-basis-*` coverage matrix records the
compact rig's muscle clip in each combination of the three keep-original
flags under the same playback, every other setting neutral, so the basis
and the residual can be told apart on one rig. A run older than the matrix
has no rows for it in `run.json` and still completes. Positive playback waits for the Humanoid body motion slice.

### Owner workflow

Follow the [Windows Editor steps](../tools/fixtures/unity/README.md). Run
**Tools → unityrs → Build and record all fixtures** first in a new Unity
2022.3 LTS 3D (Built-in) project and later in a separate new Unity 6000.x
project. The button uses the active build target, which defaults to
`StandaloneWindows64` on the owner's PC. Send each complete
version-and-target folder to the maintainer's fixture inbox. If the run
stops with **unityrs fixture build failed**, send the log from the separate
`-failed-<id>` folder shown in the dialog.

Two more menu items build variant runs with no replays. **Build URP
material fixtures** runs only in a Universal 3D project and writes
`<version>/URP-<target>/` with the `urp-materials` bundle: URP Lit and
Unlit materials with textures, keywords and properties. **Build Android
fixtures** needs Android Build Support and writes `<version>/Android-variant/` with
the static bundles and an `android-textures` bundle whose textures carry
explicit ASTC and ETC/ETC2 Android overrides. Their `run.json` names the
variant (`urp` or `android`), and `complete` checks it against
[`anim_fixture_variants.py`](../tools/parity/anim_fixture_variants.py):
shader names, keywords and properties for URP, and each override's texture
format, data size and `.resS` stream for Android, with each texture decoded by
`unityrs extract` at its declared size. The Android folder is
separate from `<version>/Android/`, where a Built-in run publishes when
Android is the active target. A Built-in run keeps `urp-materials` as
`not_covered`.

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
reason for uncovered sections and fixture errors. The button generates two of them in a separate bundle build: a
humanoid rig with non-neutral Avatar twist that ramps the twist muscles, and a muscle clip whose one non-neutral
setting is a height offset. Integer discrete bindings stay not covered on Built-in. Unity keeps an integer as a
discrete curve only on a script field: `EditorCurveBinding.DiscreteCurve` accepts only an enum or an int with
`[DiscreteEvaluation]`, and `DiscreteEvaluationAttribute` targets fields
([UnityCsReference](https://github.com/Unity-Technologies/UnityCsReference/blob/2022.3/Editor/Mono/Animation/EditorCurveBinding.bindings.cs)).
Built-in components bind their integers as float curves, and the fixture prefabs carry no scripts. The run's reason
lists the discrete candidates that `GetAnimatableBindings` reported on the probe's built-in components. A case is not covered only when an explicit check
shows Unity cannot produce it, with the reason; any other error fails it. A case recorded in `run.json` needs its
pending recording, which completion checks against the [coverage contract](../tools/parity/anim_fixture_coverage.py);
any other status must leave none. Completion lists the optional cases not recorded. Its `REQUIRED_COVERAGE` table
names, per Editor version, the cases a run must record; add a case there once a real run of that version has
recorded it and completion has accepted it. Real 2022.3 and 6000 runs recorded both humanoid refusal probes, so
both versions now require them, and a run of either version without them is refused. The `*.pending.json` files use
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
budget rows of a completed generic or legacy replay without them, and
`--verify` refuses stored rows that differ from the ones it writes. It
checks each bundle's SHA-256 pin, then reads the bundles through
`unityrs show --value canonical`, refusing any truncated document or one
for another file or object, and
takes the tagged f32 bits of infinite tangents as they are. It reads the
serialized rest pose, the compiled or explicit curves and the playback
interval, never the observed poses. It walks each interval between the
start, the stop and every key strictly inside them, and records every
coefficient, key-relative time, Hermite control and clamped or rest value
the path reads. A translation or scale row must also give each component
its own ranges in `component_ranges`, in the stage's layout, with `[0, 0]`
for a range the component never reads. The schema, `derive` and the Rust
audit refuse a translation or scale row without them, and any other row
with them; no shared envelope ever bounds a vector component. The derivation then bounds each
component from its own ranges, so a large clamped Y never widens X's
tolerance, and the Rust audit checks each component against them. The
author supports rest, constant, held and streamed compiled slots, clamped
unweighted explicit Hermite curves and Euler input through `joint`, and
refuses dense, weighted and humanoid paths until they have an author.
Its choices, provisional until review:

- Every computed enclosure is widened by `2^-40` of the magnitudes of the
  terms that produced it, never of the result, so cancellation and trig
  zeros keep their margin. The tool computes in f64, where each rounding
  errs by at most `2^-53` of its operands' scale. `2^-40` covers about
  4096 such roundings, yet is `2^-16` of an f32 ulp, so the derived bounds
  do not grow measurably. Exact f32 inputs are widened by `2^-40` of their
  own magnitude. A further absolute `2^-1022`, the smallest normal f64,
  covers the f64 underflow of the tool's own arithmetic, whose absolute
  error is at most `2^-1075` per operation. It is far below the smallest
  f32 subnormal, `2^-149`.
- An Euler box widens each half angle by `2^-40` of its magnitude before
  the trig. That covers the rounding of the degree conversion and the
  difference between the f64 constant and π, below `2^-52` of π. Sine
  and cosine are enclosed over the widened interval, including any
  extremum inside it, and the axis quaternions are multiplied as
  intervals. A constant angle whose half falls on a trig zero, such as
  3060°, still lies inside its box.
- The rotation `norm_ranges` hold a point box at every walked time, using
  that time's own segment, so a held jump at a key or at the stop is
  enclosed. Each interval is halved at its midpoint, as the Rust audit
  halves its pieces, until every box's guaranteed largest component is at
  least half its largest possible one. A unit quaternion's largest
  component is at least one half, so the floor stays at least one
  quarter. A path that gets this close to the origin, such as a raw
  quaternion whose W changes sign beside an X of `2^-14`, is refused as
  `rotation path too close to the origin to bound` after 16 halvings, as
  is a box holding the origin, a rest pose included.
- The Rust audit halves its own pieces up to 20 times. It expands each
  piece's chord by a curvature bound, per component, and for a `joint`
  output clamps it to `[-1, 1]`, since a normalized quaternion lies there.
  With at most one turning Euler axis, the other two vector components are
  exactly zero and get no expansion, so a Z-only turn the author accepts,
  such as 0° to 3000°, passes the audit (a test runs that case through
  both). A piece that still fits no box is refused as `rotation path too
  close to the origin to bound` when its chord's norm is below every box's
  guaranteed floor, and as `bundle rotation output boxes` otherwise.

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
operations in unit space. Add half an f32 ulp for final storage at the
largest value the stage ranges admit, never at a captured value: one for
a normalized quaternion, else the stage's value magnitude times the weight
gain plus the arithmetic error. For rotation, convert the normalization and storage
errors with norm one, sum the two angular budgets and include the f64
angle metric floor. Refuse any rotation tolerance that reaches π.

Run `python3 -I tools/parity/anim_replay.py derive synthetic-replay.json --bundle-dir . > synthetic-tolerances.json`.
The result contains the manifest SHA-256 (sorted compact UTF-8 JSON),
`clock_discriminated` (true, false or `"independent"`), ordered property, target and channel keys, and
per-sample clocks, held sides and bounds. A translation or scale row has
one bound per component, compared component by component; a rotation or
weight row has one bound. Freeze it alongside the inputs before comparison.
Investigate discrepancies against the recorded layout, settings and
equations.

After deriving and freezing the tolerances, run the Rust comparison from
the repository root:

```sh
UNITYRS_ANIM_CLI=target/release/unityrs \
UNITYRS_ANIM_REPLAY=synthetic-replay.json \
UNITYRS_ANIM_BUNDLES=. \
UNITYRS_ANIM_TOLERANCES=synthetic-tolerances.json \
cargo test -p unityrs-export --features gltf --test anim_replay \
  recorded_generic_or_legacy -- --ignored --nocapture
```

The gated test first runs `anim_replay_budgets.py --verify` with that CLI,
so it refuses budget rows that differ from the ones the bundles give.
The same-bundle comparator checks the captured rig, clip and renderer
identities, derives clocks and ranges from serialized playback, and reports
worst Unity-space errors by rig, node or renderer, property and source time.
The ungated synthetic tests exercise both clip modes with testkit bundles.
L11 status requires the owner-built Editor recordings.

## Humanoid body fixtures and B1 tolerances

B1 of the [humanoid body motion plan](plans/humanoid-body.md) adds its
Editor fixtures, recorder rows and tolerances. All of it is evidence for
the plan's hypotheses. None of it is a playback claim.

### Body fixtures

The main menu item builds 932 optional `body-…` cases, each kind in its own
bundle build after the main and coverage builds. Their tables are in
`UnityrsBodyFixtures.cs` and `UnityrsBodyClips.cs`, mirrored by
[`anim_fixture_body.py`](../tools/parity/anim_fixture_body.py).

| Cases | Plan | Contents |
|---|---|---|
| `body-probes-<rig>` (19) | F1, F6 | Known-vector `SetHumanPose` on each rig. The rigs are the two P2b rigs, each F6 topology, a noncoplanar rig and the hold-out rig. The vectors are one per probe muscle of each mapped bone, then three combined vectors over every probe muscle. Twist and stretch stay out. |
| `body-probes-tall-2` (1) | F1 | 26 extra known vectors on the tall rig, in a bundle of their own. Each moves two of the trunk, shoulder, upper-arm and upper-leg bones, the poses the adequacy checkpoint predicts it needs (below). |
| `body-probes-twist-{compact,tall}` (2) | F2 | Twist sweeps on the two P2b rigs, each in a bundle of its own: 84 known vectors, the shoulder-hip twist swept by the Spine and Chest twist muscles under an upright and three tilted body targets (below) |
| `body-probes-range-{compact,tall,noncoplanar}` (3) | F8 | Range sweeps on three training rigs, each in a bundle of its own: 263 known vectors filling every mapped spine bone's muscle cube jointly under the twist sweeps' tilts (below) |
| `body-probes-band-{compact,tall,noncoplanar}` (3) | F9 | Lateral band sweeps on the same three rigs, each in a bundle of its own: 296 known vectors crossing combined spine Left-Right values up to ±3/8 with tilts and small arm-leg yaw gaps, and the side bends (below), all required once the run has them |
| `body-targets-{compact,tall}[-offset]` (4) | F3 | The three combined vectors with nonidentity body targets on identity and offset roots |
| `body-clip-<variant>[-offset]` (78) | F4 | 39 body-motion clips on the compact rig, each on an identity and an offset root |
| `body-chain-<f5>-{targets,clip}` (18) | F5 | A scaled Animator node, a larger skeleton, an intermediate scaled, moved and turned `Armature` node, and a non-unit hips scale; then, for E5, the scaled Animator at sizes 1.5 and 0.82, the hips scale at 1.2 and 0.7 and the armature at 1.8, a size scaling every offset below the Animator (the armature's translation too). Each plays F3's targets and the all-original F4 clip with changing `MotionT` and `MotionQ` curves, whose residual body translation and tilt change through the chain and whose motion the original-root rule keeps. Completion requires its built motion slots; if Unity drops them, that clip case is not covered. |
| `body-clip-<rig>-<variant>[-offset]` (798) | E5 | The 21 variants B4 admits (freeze v8's E4 passing set, each with its `-start` twin where it has one) on 19 more rigs, each rig in a bundle `body-clips-<rig>`: F5's large rig, the tall rig and a plain `small` rig (size 0.82), at the top, middle and bottom of B4's human-scale range, then every admitted training topology but the compact rig's. Never a hold-out rig. |

**Rig geometry.** One parameterized layout gives every rig. Unchanged, it
repeats the P2b offsets in the same sibling order. An omitted bone's
children keep their place: its offset is added to theirs. The noncoplanar
rig moves the upper legs to `z = .03` and the shoulders to `z = -.025`.
The no-Chest rig does the same, because Spine's three children are its
own edges only there, and only off one plane can each be separated. The
hold-out rig scales the arms by 1.3 and the legs by 0.8, and lowers the
hips to keep the feet on the ground.

**Schedules.** Each known-vector case keeps the P2b anchors at 0, .5 and 1
(neutral, single-axis, noncommuting), so the Spine endpoint checks still
hold. The probes fill each half at `.5 (k + 391/1024) / n`, rounded to a
`2^-16` grid, so each time is exact in JSON and as an f32. A probe sets one
muscle to 1, -.9 or .8 by its place in its group, near the muscle's limits,
so each probe turns its edges as far as it can. The combined vectors cycle
five values per probe muscle.

**Extra probes.** An extra probe case (`ExtraProbes`, mirrored by
`EXTRA_PROBES`) gives each listed bone the muscles of one of its rig's own
probe vectors: single probe `k`, or combined vector `c`. Every other
muscle is zero. Each bone's local rotation is then one its rig's probe case
records, so the case can be predicted before it is recorded. Its schedule
keeps the anchors and the grid. It builds its own bundle, so a rerun
leaves the other cases' bundles unchanged. A run with body rows but none
for the extra cases predates them and completes. Once the rig's own probe
case records, its extra case is required: in the builder, Unity refusing
it is an error rather than not covered, and completion refuses a run where
it is not recorded.

**Twist sweeps.** A twist sweep (`TwistProbes`, mirrored by `TWIST_RIGS`)
sets the Spine and Chest twist muscles together to `degrees / 80`, so with
HumanTrait's default limits (±40° each) the upper body turns about the
spine by 0, ±2, ±4, ±6, ±8, ±10, ±15, ±20, ±30, ±45 and ±60°. Every other
muscle is zero. Each twist runs under four body targets without offset:
upright, and the pitch and roll pairs (12°, 7°), (−9°, −11°) and
(12°, −11°) from F3's tilts, with no yaw. The anchors stay upright. That is
84 vectors plus the anchors, so the read-back frame's turn about `up` can
be measured against the twist and the tilt (2.2). The sweeps are a later
generation than the extra probes, required the same way once their rig's
probe case records, and only on training rigs. The adequacy checkpoint
does not read them.

**Range sweeps.** A range sweep (`RangeProbes`, mirrored by `RANGE_RIGS`)
moves every mapped spine bone's three muscles (Front-Back, Left-Right and
Twist of Spine, Chest and UpperChest) over the whole cube `[-1, 1]^3`
together. The vectors are the first 256 points of the unscrambled 9-d Sobol
sequence with Joe and Kuo's direction numbers, each coordinate `x` as
`2 x - 1`, so every value is exact in f32. A rig without UpperChest takes
the first six coordinates. Then come the seven joint corners the sequence
lacks, every mapped spine bone at the same corner; `(-1, -1, -1)` is the
sequence's first point. Every other muscle is zero. The body target takes
the twist sweeps' four tilts, each over an aligned block of 64 consecutive
Sobol points. Such a block is itself a net that fills the whole cube, where
every fourth point would keep Spine Front-Back and Left-Right in one
quadrant. The corners cycle through the tilts, and the anchors stay
upright. The design comes from the cube alone, never from recorded or real
animation. With the anchors that is 266 samples per case, on the compact,
tall and noncoplanar rigs. The sweeps are fitted with the probes, and each
is required once its rig records. The three rigs with UpperChest absent and
both, the left or the right Toes absent came with them, in the same later
generation. Those rigs are required whenever a run has that generation: an
Avatar Unity refuses fails the run instead of going uncovered.

**Lateral band sweeps.** A band sweep (`BandProbes`, mirrored by
`BAND_RIGS`) validates the frame past its lateral domain. B2's checker
refuses a spine Left-Right value beyond the largest one at which every
training sample passes E2 and E3a. Past it, 2.2's frame leaves Unity's
read-back by an unexplained second-order yaw of up to 0.08°. The vectors are
the range sweeps' 256 Sobol points, rescaled per bone: Front-Back over
`[-1, 1]`, Left-Right over `±3/8` and Twist over `±1/32`. Each is `2 x - 1`
times a power of two, so exact in f32. The twists stay small, so most poses
stay inside the frame's shoulder-hip twist scope, and they open an arm-leg
yaw gap of a few degrees. The tilts are the range sweeps'. Then come 40
side bends (`BandBends`), every mapped spine bone alike with Front-Back
zero, under each tilt. Four are twisted: Left-Right and Twist at
`±(15/128, 1/32)`, `±(9/64, -1/32)`, `±(11/64, 1/32)` and
`±(13/64, -1/32)`. They open yaw gaps of either sign in the band's low
bins, from just past the validated domain, which the Sobol points' largest
Left-Right reaches only sparsely. The fifth is pure, Left-Right `±3/8`
with Twist zero: the oppositely tilted arm and leg laterals at a near-zero
yaw gap, the shape no lateral-weighting rule fits. With the anchors that
is 299 samples per case. A run that has the band sweeps requires all
three, whatever their rigs' own probe cases did: Unity refusing one is an
Editor error. They are a later generation of their own. They are evaluated (E2, E3a, and their
margins in the adequacy checkpoint) but never fitted, so they validate
the fit without moving it. The design comes from the cube alone.

**F3 targets.** The targets alternate two requests. One moves the rest
body by `(.05, -.03, .04)` and tilts it by `Euler(12, -20, 7)`. The other
moves it by `(-.04, .02, -.06)` and tilts it by `Euler(-9, 35, -11)`. The
anchors carry them too.

**F4 clips.** Every F4 clip has `RootT` with XZ and Y motion, and a `RootQ`
whose yaw turns back and forth. Both are on linear segments, with keys at
0, .25, .5, .75 and 1. The variants are:

- the eight keep-original combinations with bake off;
- the seven other bake combinations with keep-original off;
- height from feet, `level = .125` and `orientationOffsetY = 30`, each alone;
- each of those 18 again from a nonzero start, its whole path turned 50°
  about +Y and moved by `(.6, 0, -.4)`;
- separate `MotionT`/`MotionQ` curves on the all-original combination,
  with a `MotionT` Y that starts above zero and changes, unlike a zero or
  start-height rule;
- height from feet with alternating knee bends, with and without foot
  goal curves.

The builder confirms each variant's settings after authoring and after the
build. If Unity changes a setting, or drops the motion or goal curves while
authoring, that variant is not covered and the rest still record.

Before an F4 or F5 clip case records, the builder also checks the built
clip's compiled tracks: `RootT` and `RootQ` (Animator attributes 7–13),
and, where the variant relies on them, `MotionT` and `MotionQ` (0–6), the
knee muscles (42 onward) and the foot goals (14–27). Each must be in
`m_ClipBindingConstant`'s runtime track table, bound to the Animator root.
Where Unity exposes `m_MuscleClip.m_IndexArray` on the bundle-loaded clip,
the attribute must index that same slot. A missing or moved track fails the
case with the tracks named, and it is not recorded. `complete` then checks
the slots' values.

### Recorder rows

A body case's editor evidence holds:

- `body_case`: the kind, rig, variant, root and clip settings;
- `body_samples`, one row per sample:
  - F2's `GetHumanPose` read-back, converted to `A` as the rest body is (an
    inverse Animator transform of `bodyPosition · humanScale`, divided by
    `humanScale`);
  - the raw world read-back and its muscle array (`readback_muscles`, raw
    bits);
  - `nodes`: Unity's own evaluation of every node, its world position,
    rotation and `lossyScale`, and about the hips its
    `hips.InverseTransformPoint` position and `hips.rotation⁻¹` rotation;
  - during clip playback, the getter values `Animator.rootPosition` and
    `rootRotation` (`root_*`). Playback runs with `applyRootMotion` off, so
    they may be the Animator node's own transform rather than the clip's
    root projection. `Animator.bodyPosition` and `bodyRotation` are not
    read: outside `OnAnimatorIK` Unity warns on every call, and
    `GetHumanPose` already gives the body. Recordings that hold them keep
    them, unread;
  - for a clip, a second playback with `applyRootMotion` on (`motion_*`):
    a fresh instance each sample, the graph set to 0, evaluated once at 0
    and once by the sample time. It records the Animator node's resulting
    local position and rotation, and `deltaPosition` and `deltaRotation`
    as Unity reports them;
- `body_targets`: each known vector's requested offset and tilt, and the
  body pose `SetHumanPose` received;
- `human_bones`: HumanTrait's 55 bones with `GetParentBone`, `RequiredBone`
  and `GetBoneDefaultHierarchyMass`, for E0b.

Every body value is raw f32 bits, nonfinite included, so a case records
whatever Unity returns. After each case the recorder logs one warning per
field that held nonfinite values, with the count and the first value
(`Nonfinite <field> in <case>: <count> of <n> values (NaN)`).
`complete` refuses a nonfinite value, naming the field, wherever the
analysis reads it: the read-back pose, every node row, the body targets,
the root-motion transform and the body slots' masses. The read-back
muscles, the getter rows, the root-motion deltas and the other bones'
masses are kept as recorded.

The recorder samples from its own copy of the spec. It keeps the
schedule's exact times there, because 2022.3's `JsonUtility` read some
16-digit times on the 2^-16 grid one ulp off. `complete` restores a
recording made before that fix only where a sample's `local` and
`source` are the schedule's time one double ulp away, at exactly the
scheduled f32 sample time and effective source.

Analysis uses them as follows:

- The read-back is E2's and the adequacy checkpoint's target for a clip.
- `nodes` checks B2's forward kinematics and hips-local positions against
  Unity's own, sample by sample, before any fit.
- The getter rows are evidence of what those getters return with root motion
  off. They are not taken as the clip's root projection.
- The root-motion rows are the projection: B2's root-motion model (keep
  original, bake and the offsets) must reproduce the Animator node's
  transform from its rest transform.

While recording, every body sample also checks that the built Avatar maps
exactly the bones the case's schedule assumes (`GetBoneTransform`). It also
checks the F5 chain at that sample: the Animator node's scale, the armature
and its scale, the sampled hips scale, and every other mapped bone's
authored offset. Any difference fails the case.

Known-vector body cases keep the pending human_pose schema, as the P2b known
vectors do.

### Completion

`complete` treats the body cases as one optional group:

- A run with no `body-…` row completes as before.
- A run with any body row must list every body case.
- E5's cases (round 11: the clip rigs and the five chain rigs added for E5)
  are a generation of their own. A run without them completes as before;
  a run with any of them must list and record every one. Each one's
  recorded human scale must lie in B4's range, from the armature rig's
  `0x3f47f9bb` to the large rig's `0x3fb7a311`. B2 never reads them.
- A recorded case needs `replays/body/<case>.pending.json` and its bundle.
- A failed case fails the run.

For each recording `complete` checks:

- the case row, against the builder's tables;
- the bone and mass table;
- that the captured mapped bones are the rig's topology;
- read-back rows for every sample, with unit rotations, the read-back
  muscles, and root and root-motion rows exactly in clip playback, each
  finite where the analysis reads it;
- a node row per node per sample, with unit rotations and the hips at the
  origin of their own frame;
- the schedule's times and muscles, exactly;
- each body target, as requested (zero for probes) and as applied, within
  `1e-5`;
- the F5 chain in the rest pose;
- for a clip, the authored body, knee, motion and goal curves, and the
  captured and built root settings;
- for a knee, motion or goal variant (and F5's clip), that the built clip
  keeps each of those slots (Animator attributes 0–6, 14–27 and 42 onward)
  where playback reads them: the DOF's `m_IndexArray` entry is present (not
  -1) and names a slot of `m_MuscleClip`'s data bound to that attribute,
  with the authored value at every key, sampled as playback samples it (a
  dense slot linearly between its frames). A surviving `m_EditorCurves` or
  `m_FloatCurves` copy does not count. Otherwise the variant does not
  count as covered.

It copies the built `m_MuscleClip` root fields and the compiled Animator
attributes into `evidence.body_settings` for B2. Any other case that
claims body rows is refused.

### B1 tolerances

[`anim_body_tolerance.py`](../tools/parity/anim_body_tolerance.py) derives
section 2.5's bounds, and its docstring holds the derivation. The
operation model is the one the per-recording tolerances above use, applied
operation by operation at its actual operands:

- A value computed through `n` dependent roundings errs by at most
  `γ(n) = n·u / (1 − n·u)` of the absolute values of its operands, with
  `u = 2^-24` for Unity's f32 and `2^-53` for the model's f64.
- Each operation adds `2^-126` for flush-to-zero.
- Recorded f32 values decode exactly. Each rounding is counted once.

All bounds are on a vector's Euclidean norm. The tests check each one by
hand, and the forward kinematics against chains emulated in f32.

| Operation | Depth | Rounds at |
|---|---|---|
| `rotate` | 6 | Unity's `v + w t + u × t`, `t = 2(u × v)`: the absolute values of each cross product's operands |
| `compose` | 4 | A quaternion product: its sixteen products |
| `matrix` | 3 | One entry of a 3×3 product or matrix-vector product: its products |
| `quaternion`, `unquaternion` | 4, 8 | A quaternion's rotation matrix; Unity's matrix-to-quaternion conversion and its f32 store |
| `midpoint`, `lateral`, `cross`, `normalize` | 2, 2, 2, 6 | 2.2's up and lateral vectors at the four points, each cross product at its operands, and each normalization |
| scale product, add, subtraction, division | 1 | Their results |

**Rotations.** An error `|dq|` in a quaternion moves Unity's rotation of `v`
by at most `κ(q) |dq| |v|`, with `κ = 2 √(|u|² + (|w| + 2|u|)²)`.

**Forward kinematics.** One link sets `p' = p + s R t`, `q' = q q_link`
and `s' = s s_link`. The offset's rotation rounds at `t`, the scale product
and the add at their results, the rotation product at its products, and
the scale product once. The parent's errors carry over, its rotation's
through `κ`. A chain is walked from an exact identity at its base:

- the hips, for 2.3's hips-local pose, so where the hips sit in `A` does
  not enter;
- the Animator node, for the hips parent `(t_p, R_p, σ_p)`;
- the world, for Unity's read-back.

**Centre of mass.** `c = Σ_i m_i (p_i + Σ_j a_ij (p_j − p_i))`. Under
`|a_ij| ≤ 1` (B2 refits and checks it), input errors reach `c` as
`Σ_i m_i (|dp_i| + Σ_j |d(p_j − p_i)|)`. An edge difference's error walks
the links from `i` to `j` again from `i`'s state with no position error, so
a common translation cancels, plus the subtraction at its result. The sum's
own rounding is `γ(n_max + 2N + 2)` of `M = Σ_i m_i (|p_i| + Σ_j |p_j − p_i|)`.
The sums run in HumanBodyBones order.

**The frame.** Each normalization multiplies its vector's error by
`2 / |v|`, for the up vector and for `lateral × û`, so the bound grows
without limit as that cross product vanishes. `H_A = M(B_A) F(T) F_hᵀ`
rounds at its operands.

**`ε`.** Unity's world centre of mass and the recorder's conversion:

- GetHumanPose's stored `w = c / h` and the recorder's `h w` round at the
  world value;
- the subtraction of the Animator's world position rounds at the
  difference `d`, and the inverse rotation at `d`;
- the division by the Animator's scale, and the stored division by `h`, at
  the result.

The Animator's own tracked errors carry through, and `ε = E_c / s_root +`
the conversion's bound.

**`E_unity`.** Unity's own error in the recorded hips-local translation:

- **The target.** A known vector's is the exact f32 body pose the recorder
  passed to SetHumanPose, so only `h T_A` rounds and `B_A` is exact. A
  clip's stands in by its read-back. The position carries `ε`. The
  rotation carries Unity's world frame and T-pose frame, its
  matrix-to-quaternion conversion and store, and the recorder's product
  with the inverse Animator rotation.
- **The hips-local centre of mass** `c_h` (`E_ch`, magnitude `M_h`), and
  both frames: `|dH| ≤ |dF_h| + |dF_T| + |dB_A|` plus `H_A`'s rounding.
- **The solve** `P_A = h T_A − k H_A c_h`: `k (E_ch + |dH| M_h)`, the
  matrix-vector product at `|H_A| M_h`, the product by `k` and the
  subtraction at its result.
- **The local conversion** `σ_p⁻¹ R_p⁻¹ (P_A − t_p)`: `t_p`'s error, the
  subtraction at its result `d` (`σ_p R_p` times the recorded hips
  translation), `R_p`'s error acting on `d`, the inverse rotation at `d`
  and the division at its result. `σ_p`'s error cancels in
  `k / σ_p = s_h` and acts only on `h T_A − t_p`.

**`E_eval`.** The model's own error at a sample, with the fitted
coefficients. B2's f64 reference runs the same path from the recorded f32
inputs taken exactly, at `u = 2^-53`. Storing the hips translation as f32
adds `2^-24` of a bound on the model's pre-cast translation,
`(h|T_A| + k M_h + |t_p|) / σ_p` plus the path's own error. These
roundings are the model's, not Unity's, so `E_unity` does not count them.

**`T`.** E3's comparison tolerance is `T_s = E_unity,s + u_s + E_eval,s`
(section 2.5).

These are B1's bounds. B2 freezes its own and reruns the check if they
differ.

### Adequacy checkpoint

After the owner's first run of these fixtures, run the checkpoint on each
version's folder:

```sh
python3 -I tools/parity/anim_body_adequacy.py <folder> --json report.json
```

`--design-only` predicts before a run. It adds every extra probe case the
folder lacks, built from its rig's recorded probe case by
[`anim_body_design.py`](../tools/parity/anim_body_design.py), then runs the
same checkpoint, and the report lists the predicted cases.

- Each planned bone takes the local rotation Unity recorded for the same
  muscles, bit for bit. The pose's hips-local kinematics, its columns (up
  to a turn of the block, which leaves `u_s` unchanged) and its `J_s` are
  then exact. Its body target is the recorder's known vector, the same in
  every probe sample.
- Unity's solved hips transform and world read-back are not known before a
  run, and they set `ε` and `E_unity,s`. They are bounded by an envelope:
  every recorded sample of the rig's identity-root probe and target cases
  lends the planned pose its hips transform and read-back.
- Each planned sample takes the largest `ε` and `E_eval,s` and the
  smallest `E_unity,s` over the envelope. A larger `ε` weights its block
  less, so it can only raise `u_s` elsewhere. A smaller `E_unity,s` lowers
  the sample's own bound. The predicted margin is therefore conservative.

A recorded run of the planned case stays the checkpoint of record.

[`anim_body_adequacy.py`](../tools/parity/anim_body_adequacy.py) builds a
row block from every body read-back sample of the training rigs. The
hold-out rig takes no part: its recordings are never read, so they cannot
change the verdict. Only the post-freeze evaluation, E3b, reads them. Each sample's three equations are weighted by `1/ε`.
Each coefficient `a_ij`, keyed by edge, has the column `m_i (p_ij − p_i)`
in `A`. Edges come from the recorded hierarchy: a mapped body-slot bone and
its nearest mapped ancestor by node address. Masses are the E0b-folded
defaults from `human_bones`. The checkpoint reports:

1. **Rank and condition** of `A_w`. A Householder QR is followed by a
   one-sided Jacobi SVD of its triangle. The numerical rank uses
   `σ_max · max(R, N) · 2^-52`, and the condition limit is `1e6`. Each null
   direction is listed as a free combination.
2. **`u_s ≤ E_unity,s`**, per component of the hips translation, on every
   training body sample and every P2b humanoid recording:
   the coefficients' worst-case uncertainty within the recording's own
   noise floor. The report lists each failure, every sample's `E_unity,s`,
   `u_s`, `E_eval,s` and `T_s`, and the distribution of the margins
   `E_unity,s / u_s`.
   - `u_s` sums, over the blocks, the 2-norm of each block of `J_s A_w⁺`.
   - `J_s` is the full sensitivity `−σ_p⁻¹ R_p⁻¹ (H_A H_rec⁻¹) m_i
     (p_ij − p_i)` with the solved hips orientation of 2.3:
     `H_A H_rec⁻¹ = B_A F(T) F(pose)⁻¹`. `B_A` is the rotation the
     recorder passed to SetHumanPose for a known vector, the read-back
     body rotation for a clip, and the model's own otherwise.
   - `A_w⁺ = V Σ⁻² Vᵀ A_wᵀ`, over the kept singular values.
   - There is no per-sample `E_unity` against `ε` veto (section 2.5).
3. **The coefficient inventory.**
   - Each sample's recorded edges must be the edges HumanTrait's parents
     give its mapped bones.
   - Every edge of the F1 and F6 training rigs must have fitting rows.
   - A sample whose edges no fitting row trains is listed apart.
4. **Topology coverage.** A training rig counts only through its own
   `body-probes-` case, with read-back rows and that rig's mapped bones.
5. **The extra probe plan.** Every scheduled sample of every extra probe
   case must be recorded with read-back. A missing case or sample is named.

Rank and `u_s` do not depend on column scale, so the masses matter only to
the condition number. The right-hand side is never needed. The script exits
0 when every item passes, 1 when any is short (B1 then extends the fixtures
before B2 starts) and 2 for unreadable input.
