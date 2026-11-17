# unityrs-gltf

The crate supplies an editable glTF-space model, `collect`, a strict
`Profile`, `layout`, and `write_glb`. Collection uses core, scene, mesh and
meshdecode, texture and imageio; it does not depend on the CLI.
`collect` uses `UnlitExtras`; `collect_with_mapper` accepts an injected
`MaterialMapper`, including `CorePbr`. Images are charged PNG/JPEG owners.
Collected Texture2Ds are decoded and encoded once per source identity. A bounded metadata walker mirrors Khronos
glTF-Validator 2.0.0-dev.3.10 in both modes, without allocating or decoding
pixels. PNG chunks before IDAT must fit, including the CRC slots; a complete
13-byte IHDR must precede IDAT. CRC values and IHDR field values are not
validated by that decoder. It stops at the first IDAT header, so the IDAT body,
IEND, and trailing bytes are not checked. JPEG walks segment lengths and
standalone markers through a complete SOFn (C0–CF except C4, C8 and CC) or DHP
(DE) segment, with the six header fields the decoder reads and one or three
color channels. It stops there, without requiring scan data or EOI. Both
formats require at least 14 bytes for recognition. PNG custom color information
and non-square pixels are refused to match the validator's unsupported-feature
warning. The caller keeps the encoded bodies charged until write completes.

All variable-length fields use `MeteredVec`. `array` and `text` charge before
fallible allocation and count copy work. Further edits must also count their
work on the caller's meter. Each field retains its charge when moved out of the
scene. A file is a suitable output sink; an in-memory sink must charge its own
growth or use previously reserved storage. A failed write can leave partial
output. No operation starts threads, waits for work, or requires seeking.

Accessors are shared by scene-table index. A mesh contains material bindings;
two meshes can reference the same geometry. Morph weights belong to nodes, so
instances do not overwrite one another. Sparse targets have strictly increasing
u32 indices and no dense allocation. Their bounds include implicit zeroes,
except when every element is explicitly supplied. An empty sparse set emits
an implicit all-zero accessor with no bufferView or sparse property. Inverse
bind MAT4 values are column-major. Triangle indices accept glTF's u8, u16 and u32 types; the
caller chooses their width. Floating-point attributes are f64 owners, converted directly to little-endian
f32 in fixed 64 KiB stack chunks. Joint and index owners retain their integer
component types.

Layout reaches meshes in table order, their primitives in order, attributes
in semantic order, indices, then morph POSITION/NORMAL/TANGENT slots, followed
by skin inverse binds, animation sampler inputs and outputs, and images. Shared accessors are placed on first reach;
unreferenced accessors are omitted. Value checks run only for emitted data,
with POSITION/NORMAL/TANGENT, colours and UVs analysed to decide omissions. Each view
starts at a multiple of four.
View lengths exclude padding; BIN padding is zero and JSON padding is space.
An empty scene has no BIN chunk. Checked lengths refuse GLBs larger than u32.

JSON has fixed schema key order, ordered tables, and ordered metadata keys;
there is no hash iteration. Extras keys and primitive attributes must be
strictly ordered and unique. TEXCOORD sets start at zero with no gaps;
COLOR, JOINTS and WEIGHTS currently support only set zero. Extras are
string-valued metadata slots in this PR. Rust's locale-independent shortest round-trip f64 `Display` on the pinned
toolchain is the number rule, with negative zero canonicalized to 0 in JSON.
Payload conversion is IEEE f64 to f32, nearest ties-to-even;
NaNs retain their sign and high payload bits and are explicitly quieted for
host-stable raw-mode output. Accessor bounds are computed from those f32 values, not the original doubles. JSON is measured
and emitted by the same serializer into an exactly sized charged buffer.

`Options::default()` normalizes normal and tangent xyz, sets tangent w to ±1,
clamps negative/non-finite rounded weights to zero and rescales weights to
sum to one. An all-zero weight row becomes [1,0,0,0] with the first joint
index set to palette index zero. All four joint indices are zero in that
fallback row; every index whose emitted weight is zero is also cleared.
Changes count differing f32 scalar bits
once per emitted shared accessor, with zero-weight rows counted separately.
Nonzero node rotations are renormalized in f64 and clamped to [-1,1];
raw rotations refuse components outside that range. Rotation changes count
f64 scalar bits once per node in `Report::rotation_components`; asset extras
include `rotation_components` when nonzero (absence means zero).
Finite colour components clamp to [0,1] before rounding; non-finite colours
omit the colour attribute, and non-finite UVs (including f32 overflow) omit only
their set, per primitive. Surviving UVs compact in source order. Texture info
uses the first surviving set; if none remains, only that primitive uses a
material variant without texture references. `color_components` counts differing
emitted scalars once per shared accessor; `dropped_color_attributes` and
`dropped_texcoord_attributes` count attributes per retained primitive;
`dropped_texture_references` counts lost slots per retained primitive.
The returned report and asset extras carry these counts and primitive issues.
Issue `mesh` and `primitive` indices refer to output arrays, both null for
omitted primitives; `source_mesh` and `source_primitive` retain model indices.
Joint accessors shared with different weight accessors must need identical
full-row joint repairs; conflicting repairs require separate joint accessors.
The writer refuses duplicate positive joint influences in hand-built scenes.
In normalise mode, `collect` merges repeated vertex indices even when the bone
palette is unique, reporting `duplicate_skin_influence` per merged entry.
Palette weight repairs retain their original accessor in
`Scene::collected_weight_sources` so the report and asset extras count all
changed weight components and zero rows once per emitted accessor.
Palette bounds use only the rows of surviving primitives, after omission
decisions and joint repair in normalise mode; raw mode checks the original
indices, including zero-weight slots. Maxima are cached once per emitted
JOINTS accessor, independently of node instances.

Normalise mode refuses skinned mesh nodes with a parent or a non-identity local
transform. The model exposes TRS transforms; matrix transforms must be resolved
by the caller before constructing a node. The collect pass places
skinned mesh nodes at scene roots with identity transforms. Raw mode writes their parent relationships and local transforms as given.

Non-finite rounded positions omit their referencing primitives; invalid or zero
normals omit NORMAL and TANGENT. Invalid tangent xyz omits TANGENT. If that
leaves a morph target empty, its primitive is also omitted. A mesh left empty
is omitted and its nodes keep transforms and metadata. Other malformed values
or references are errors before output begins. `normalise: false` preserves
rounded payload bits, including NaN and infinity, and may fail validation;
JSON itself remains valid. Morph deltas are never unit-normalized.

The model already uses glTF coordinates and UVs. `collect` performs the Unity
conversion described below. Material mapping is described below.

Add an `Animation` to `Scene::animations` with sampler and channel tables.
Channels reference scene nodes and animation-local samplers. Samplers reference
dense f32 accessors in the scene table and specify `STEP` or `LINEAR`. Supply
baked keys in glTF space with finite, nonnegative clip-local seconds and
strictly increasing times after rounding. Use `AnimationVec3` for translation
and scale, unit XYZW `AnimationRotation` keys with continuous signs, and
`AnimationWeights` scalars in key-major order with one row of weights per
key, in the target mesh's morph order. TRS channels target transform and joint
nodes. Morph channels target the node that instantiates the mesh.

The writer validates rounded animation keys in both modes and emits the
rounded values directly. It keeps the order of clips, channels and referenced
samplers, omits clips without channels, and writes time bounds from the actual
f32 payload. Optional names and string extras use the existing metadata model.
`tools/parity/gltf_check.py` can independently decode keys and reconstruct
STEP, LINEAR and quaternion interpolation.

Writer tests use hand-built scenes; collector tests use `testkit::scene3d`. The committed empty, triangle and skinned
sparse-morph GLBs are byte goldens; `UNITYRS_GLTF_UPDATE_GOLDENS=1 cargo test
-p unityrs-gltf --test writer` explicitly regenerates them. The pixel is a
synthetic 1x1 PNG from the existing imageio fixture generator. No semantic
Editor-fixture verification is claimed.

Set `UNITYRS_GLTF_VALIDATOR` to Khronos glTF-Validator 2.0.0-dev.3.10 to run
structural tests. Without it they print a skip notice unless
`UNITYRS_GLTF_VALIDATOR_REQUIRED` is set, in which case they fail. CI's
`gltf` job fetches
the Linux release archive, verifies its SHA-256 before extracting or running,
and sets `UNITYRS_GLTF_VALIDATOR_REQUIRED=1` for these tests. Accepted scenes
must have zero errors and zero warnings; `tests/validator-allowlist.json` is
empty. Coverage includes attribute continuity, zero-weight joint repair,
rotation range, empty sparse morphs, image chunk/segment framing and decoder
stopping points, effective joint palette bounds after primitive omissions,
and skinned mesh placement. Root, identity skinned scenes
pass without warnings; raw parented and transformed scenes retain their expected
validator warnings. Refused image bodies also reach the real validator in
independently framed test GLBs, with their expected diagnostic codes asserted.
Other invalid inputs are refused before emission, and the raw-mode fixture must
fail validation.
The validator is a CI tool, never a library dependency or release artifact.


Select an export unit with `unityrs_scene::Unit::Scene(file)` for a complete
scene file (`levelN` or a scene bundle's `BuildPlayer-<name>` CAB), or
`unit_from_object(&object)` for a prefab GameObject or a SceneAsset marker
(class 1032). A SceneAsset selects its owning file without parsing its payload.
`unit_from_container(&env, &file, &row)` dereferences one container entry,
returning a prefab for a GameObject, a scene for a SceneAsset, and `None` for
other classes. Each prefab entry is its own unit, including entries targeting
the same root. Broken selected pointers return their error. Call
`unit.graph(&env, &GraphOptions::default(), &meter)` before collecting.

A scene unit includes all root Transforms/RectTransforms with null fathers in
that file, in `SerializedFile::objects()` order rather than signed path-id
order. A prefab unit includes only its chosen GameObject subtree; its parent
outside that subtree is not imported. Child order follows `m_Children`.
Load the scene's `.sharedAssets` and other dependency files into the same
Environment (or supply a `CabResolver`) for cross-file references; dependency
files' independent roots do not join the selected scene. `scene_roots` selects
exact root names or paths; `exclusions` removes exact node names or paths and
their descendants during collection. The root-order policy and built-in ids
still await Editor-fixture confirmation.

`collect(&env, &graph, &profile, &meter)` returns a `Scene`; write it with
`write_glb(&scene, &profile.options(), &meter, &mut sink)`. Pass `&CorePbr` to
`collect_with_mapper` for core PBR mapping. A missing material keeps the
primitive with glTF's default material and a counted diagnostic.

Collection mirrors X on positions, normals, tangent xyz and morph deltas,
reverses every triangle from (a,b,c) to (c,b,a), flips every UV to (u,1-v),
and negates tangent w. Tangent w normalisation takes the sign bit, including
signed zero and NaN, so reflection and sign normalisation commute. Reversing the winding compensates for the reflection,
keeping normals and front faces consistent. Quaternion (x,y,z,w) becomes
(x,-y,-z,w); scales stay unchanged. Bind matrices are conjugated by
`diag(-1,1,1,1)` and transposed from Unity row-major storage to column-major
MAT4 payloads. The writer applies its existing counted normalisation rules;
report counters and asset extras describe emitted shared accessors once.

The original Transform hierarchy is retained. Every node with a skin and mesh
is a **separate scene root with identity TRS**, including in raw mode. Joints
remain in their original hierarchy and supply world transforms; renderer TRS
must not be applied to the mesh again. Original nodes remain available as
joints and parents. Required joint ancestors survive inactive/layer/root
filters as transform-only nodes. Unresolved joints or joints outside the graph
leave the renderer as an empty node with an `unresolved_skin` reason and an
`unresolved_skins` scene count. A root bone is used as `skeleton` only when it
is an ancestor-or-self of every joint; otherwise the closest common ancestor
is used. Only when one skin spans several original scene roots, one synthetic
identity root wraps all original roots, with `unityrs_synthetic_root: "true"`
in its extras. All skinned mesh attachments remain separate identity roots.

A renderer's invalid skin data degrades that renderer instead of failing the
scene. Bind-pose count mismatches and conflicting binds for a duplicate joint
leave an empty node with an `unresolved_skin` reason. Invalid joint influences,
non-finite blend weights, and invalid bind matrices are also diagnosed before
attachment. Missing vertex weights or
indices export the mesh unskinned at its original renderer node. When several
ordinary, morph-only or downgraded renderers share a Transform, the first uses
that node and each additional renderer uses an identity child, preserving the
original hierarchy, renderer transform and independent morph weights. Repeated
Transforms in a bone palette are deduplicated, with inverse binds selected in
that order and per-skin JOINTS/WEIGHTS accessors remapping and merging their
influences; other instances keep their original accessors. Short blend-weight
arrays are padded with zero and long arrays are truncated. Each repair or
degradation is recorded in node extras and in `collection_issues` in both
`Report` and asset extras, with original model node, reason, and repaired-entry
count (one for a degraded renderer). Unresolved joints are also included there.
Invalid source geometry, including non-finite morph values in normalise mode,
leaves a reported empty node with `unresolved_renderer`. Bad colours and UVs
follow the primitive-local writer rules above. A processed Mesh with neither
positions nor indices skips the renderer as `empty_mesh`, distinct from
`mesh_decode_failed`, counted in report, asset extras and node extras.
Malformed batch ranges and decoded attribute counts differing from POSITION
likewise omit only their renderer. Missing mesh pointers are reported too.
Memory, allocation, work and other resource refusals still propagate.
Raw-mode duplicate palettes retain signed
weights while merging matching influences.
Morph-only renderers with no bones keep their ordinary hierarchy and have no
skin. Four vertex influences are used; one- or two-wide indices with matching
weights pad with joint 0 / weight 0. One-wide indices without weights get
weight 1 on their joint. Width padding applies even with `normalise: false`,
because VEC4 is structural; `padded_skin_rows` counts rows once per collected
source mesh in collection issues, before any weight normalization.
Variable-count weights remain P2b's.

Geometry, indices, sparse targets and inverse bind accessors are shared by
source file identity and path id. Mesh variants reference those same accessors
for different submesh selections. Nonempty submeshes each produce one
triangle-list primitive; empty submeshes produce none. Base vertices are added
with checked arithmetic. UV sets retain their indices; a missing set before a
present one is explicitly filled with zero UVs to satisfy glTF's contiguous
semantic requirement. When an invalid source set is dropped, these generated
fillers are excluded from fallback selection and only real surviving source
sets are compacted. Raw mode preserves the gap fillers and original sets.
Morph channels select their last frame; sorted sparse
indices retain implicit zero rows, including entirely zero targets. Duplicate
or out-of-range sparse indices and invalid frame ranges are refused. Channel
frame ranges and full frame weights are retained as string extras. Renderer
blend weights are per node, divided by the selected frame's full weight (normally
100). This preserves the authored endpoint even when a multi-frame channel ends
at another weight. Zero, missing or non-finite endpoint weights are refused;
in-between interpolation is the documented last-frame approximation. Unity's
[frame-weight API](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Mesh.AddBlendShapeFrame.html)
describes the per-frame weight range.

Static-batched renderers select their combined mesh's submesh range and attach
it to an identity scene root because the combined vertices are already in
world space. The original node keeps its TRS and `static_batched` extra.
An unusable optional root bone does not invalidate resolved joints: collection
reports `skin_root_fallback` and uses their common ancestor (or the existing
synthetic root for joints across roots). In normalise mode, finite bind-pose
last rows within 1e-6 of [0,0,0,1] are snapped exactly, reported as
`bind_pose_rows_snapped` per source matrix; larger deviations degrade the
renderer. Repeated repair extras on the same source node sum their counts;
the collection report retains each renderer's record in traversal order.
Repeated `unresolved_skin` and `unresolved_renderer` markers contain a sorted,
unique semicolon-separated set of reasons; per-reason extras sum their counts.

Legacy nonempty `m_SubsetIndices` lists use the same batching rules as modern
static-batch ranges. `keep_combined` emits the union of submesh ranges of the
selected renderers once per source, also at an identity root. Renderers excluded
by LOD, activity, layers, exclusions, roots or `m_Enabled=false` contribute no
submeshes to that union. Under `include_hidden`, that root is hidden only when every
selected combined instance is inactive; visibility applies to the whole mesh.
Ordinary instances never suppress that combined root. Enabled
LODGroups select renderers in level zero by default, suppressing renderers
found only at later levels. `all` retains every level without runtime switching;
disabled LODGroups do not filter renderers. Unity 6 mesh LOD metadata is read by
the mesh model; this policy selects LODGroup renderers, not alternate index
ranges inside a Mesh.

Effective activity includes ancestors. `skip` removes inactive subtrees;
`include` retains them; `include_hidden` additionally tags inactive nodes with
`hidden`. Layers and exclusions remove matching subtrees. Scene roots follow
the graph's object-table order, with synthetic mesh attachments appended in
renderer order; child order remains the graph's stored order. This ordering
and semantic placement still await Editor-fixture confirmation.

Version-1 profile JSON has the following schema. Every key is optional except
`profile_version`; unknown or duplicate keys are errors at every object level.
Strings support UTF-8 and JSON escapes, including paired UTF-16 surrogates.
Parsing uses charged fallible owners and has no input-sized serde allocation.

| key | type / accepted values | default |
|---|---|---|
| `profile_version` | integer, exactly 1 | required |
| `root_naming` | `name`, `path_id` (GameObject id for root names) | `name` |
| `layer_skips` | array of integer Unity layers, 0–31 | `[]` |
| `shader_rules` | object with `drawable`, `additive`, `skip`, each an array of bytewise glob name patterns (`*` and `?`) | empty lists |
| `inactive` | `skip`, `include`, `include_hidden` | `skip` |
| `material_property_allowlist` | array of exact saved-property names | `[]` |
| `scene_roots` | array of exact root names or slash-separated paths; an empty list selects every root | `[]` |
| `exclusions` | array of exact node names or slash-separated paths | `[]` |
| `builtin` | `placeholder`, `resolve`, `procedural` | `placeholder` |
| `extras_naming` | `unity` (source name, path id, layer, activity), `none` | `unity` |
| `export_metadata` | object with optional string `generator`, `source`, `input_sha256` (exactly 64 hex digits); saved as scene extras; asset generator remains the writer's identity | empty object |
| `lod` | `lod0`, `all` | `lod0` |
| `static_batch` | `unbatch`, `keep_combined` | `unbatch` |
| `normalise` | boolean; false may produce spec-invalid BIN values | true |
| `clip_states` | empty array; nonempty arrays require P2b | `[]` |

Built-in placeholders retain a sorted, unique array of structured identities in
`extras.unity_builtin`, for example
`[{"file":"unity default resources","id":10210,"name":"Quad"}]`.
`file` is the canonical resource basename; unknown ids retain `file` and `id`
with `name:null`. The editable model stores these in `Node::builtins`.
`resolve` uses the same basename and case recognition as built-in detection,
accepting both `/` and `\`, then requests the supplied
resource through the Environment and its existing `CabResolver`; a missing
file/object is diagnosed. Placeholder mode never resolves a built-in, even
when its resource is already loaded. `procedural` supports approximate Cube, Quad and Plane geometry,
tagged `approximate`; unsupported built-ins and procedural skinned renderers
leave a reported empty renderer. Malformed resolved resources degrade only
their renderer; resource, meter, limit and allocation refusals propagate.
No Editor resource geometry is shipped. Metadata strings remain independently
charged. Invalid UTF-8 source names use replacement characters in glTF JSON.

`tools/parity/gltf_check.py` independently reads dense/strided/sparse accessors,
checks Unity-space expected arrays after f32 conversion, and checks orientation.
At least 99% of eligible triangles must agree with the mean vertex normal;
area must exceed 1e-12 times the squared POSITION-bounds diagonal. No normals or
no eligible triangles is reported as N/A. CI runs its Python self-tests and
Rust collect→GLB checks, including replay of pinned L6 position arrays. Material and normal-map/image fixtures check the mappings and image orientation;
Editor semantic rows remain planned.


## Materials

Material mapping keeps base colour factors and shader metadata. `UnlitExtras`
emits `KHR_materials_unlit` and saves every present allowlisted colour, float,
integer and texture property as a string extra. Texture extras retain the PPtr,
scale and offset. A repeated saved key takes its last value, including a null
texture. FastPropertyName keys use their `name`. Empty allowlists copy no saved
properties. Mapping reasons remain visible regardless of `extras_naming`.

`CorePbr` recognizes `Standard`, `Standard (Specular setup)` and
`Universal Render Pipeline/Lit`. Standard prefers `_Color`/`_MainTex`/`_Glossiness`;
URP prefers `_BaseColor`/`_BaseMap`/`_Smoothness`, with aliases as fallback.
Null texture pointers fall through to populated aliases. Scalar metallic defaults
to zero, smoothness to 0.5; roughness is `1 - smoothness`.

Saved `_Color`, `_BaseColor` and `_EmissionColor` RGB values are sRGB encoded.
Factors use the exact EOTF: `c / 12.92` for `c <= 0.04045`, otherwise
`((c + 0.055) / 1.055)^2.4`. Alpha remains linear and unchanged. Texture
`m_ColorSpace` is 0 for linear, 1 for sRGB. Base-colour and emissive images are
encoded as sRGB for glTF; all other slots are linear. When source and slot
encodings differ, RGB is converted with that EOTF or its exact inverse, rounded
to the nearest 8-bit value; alpha is unchanged. An absent flag assumes the
slot's native encoding. Image cache identity includes source file/path id,
conversion usage (including opacity-alpha override), smoothness channel/scale,
metallic override and any albedo-alpha
source, so incompatible slot conversions cannot share pixels.

Normal maps use Unity's `UnpackNormalmapRGorAG`: `x = (R*A)*2-1`, `y = G*2-1`,
`z = sqrt(max(0,1-x*x-y*y))`, then re-encode XYZ to linear RGB. This handles
DXT5nm, BC5 and RGB maps (implicit alpha 1). Unity and glTF both use +Y tangent
normals: green's direction stays unchanged; the V flip is paired with tangent w.
Standard scales XY by `_BumpScale`/`_NormalScale` before reconstructing Z,
then bakes the resulting direction into RGB with `normalTexture.scale` 1.
The baked scale participates in the image cache identity. URP reconstructs Z
before scaling XY, matching glTF's `normalTexture.scale`, so its scale stays
in that field.

A `_MetallicGlossMap` becomes a linear metallicRoughness image: B is map R,
G is `1 - map.A * _GlossMapScale` (URP uses `_Smoothness`). With
`_SmoothnessTextureChannel == 1`, albedo alpha supplies smoothness instead
(default white alpha 1 when the base texture is null, dangling or undecodable;
failures are still counted and the valid metallic channel is retained).
URP enables this channel only for opaque surfaces, even if the saved selector
survives a switch to transparent. When enabled, the exported base-colour image
has alpha 1: opacity comes only from the material colour alpha.
Both factors are 1 when this map is present. Packing uses a grid of
max(width) × max(height) across both sources, sampling each with its own wrap
and filter. The output uses the metallic map's sampler. Different source wrap
modes cannot both be preserved outside [0,1]; `packed_sampler_conflict` counts
this loss. Packing and filtering a product into one image still approximates
continuous sampling between texel centres. Standard Specular setup
and URP `_WorkflowMode == 0` record `specular_workflow`, keep metallic 0 and
roughness from gloss, and preserve `_SpecColor`/`_SpecGlossMap` as raw extras;
no metallic approximation of the specular colour is attempted.
Occlusion copies source G to output R in a separately cached linear image and
uses `_OcclusionStrength` as `strength`.

Emission is enabled only by the exact `_EMISSION` keyword in `m_ShaderKeywords`
or the 2021+ `m_ValidKeywords` array. After colour linearization, HDR emission
is divided by its largest component when above 1; that maximum becomes
`KHR_materials_emissive_strength.emissiveStrength`, declared in `extensionsUsed`.
Components are never individually clipped to 1. Nonfinite values use defaults
with `invalid_material_value`; bounded factors are clamped with
`material_value_clamped`.

Standard `_Mode` 0 selects OPAQUE, 1 MASK, 2/3 BLEND. URP `_Surface == 1`
selects BLEND; otherwise `_AlphaClip == 1` selects MASK, otherwise OPAQUE.
Transparent URP with `_AlphaClip == 1` exports BLEND and counts
`alpha_clip_dropped`, preserving `_Cutoff` (default 0.5) in material extras;
glTF cannot combine BLEND with a cutoff.
`_Cutoff` is only the threshold (default 0.5), and a queue override does not
change either shader's coverage mode. `_Cull == 0` is double-sided.
Case-sensitive shader rules keep their precedence: `skip` omits the submesh,
`additive` selects BLEND and permits PBR, and `drawable` permits custom PBR.
Unknown or missing/malformed shaders fall back to unlit with `unknown_shader`.
Built-in Unlit/Texture, Unlit/Transparent and Unlit/Transparent Cutout use a
white factor, ignoring stale saved `_Color` while preserving it in extras.
Unlit/Color continues to use `_Color`.

ST comes from `m_Scale`/`m_Offset`, with explicit `_MainTex_ST`/`_BaseMap_ST`
overrides. Standard and URP Lit share the base ST across normal, occlusion,
emission and metallic slots, even when the base texture is null; custom shaders
retain per-slot ST. `KHR_texture_transform` keeps scale `(sx,sy)` and sets offset
`(ox,1-sy-oy)` after `v = 1-v`, including negative scale. All slots use
TEXCOORD_0; missing UVs omit maps with `missing_texture_uv`. Materials used on
both UV-bearing and UV-less geometry get separate bindings.

Samplers are shared per `(wrap U, wrap V, filter)`: Repeat 10497, Clamp 33071,
Mirror 33648. MirrorOnce exports as Mirror with counted `mirror_once`.
Point uses NEAREST (NEAREST_MIPMAP_NEAREST with source mips); Bilinear uses
LINEAR (LINEAR_MIPMAP_NEAREST with mips); Trilinear uses LINEAR_MIPMAP_LINEAR.
The exporter embeds mip 0; viewers generate the mip chain when needed.
Legacy `m_WrapMode` is supported when per-axis wrap fields are absent.

Vertex colours under Standard, Standard (Specular setup), URP Lit, URP Simple
Lit, URP Unlit, Unlit/Texture, Unlit/Color, Unlit/Transparent and
Unlit/Transparent Cutout are retained as `_COLOR_0`, with a counted
`vertex_colors_ignored` per affected primitive. These shaders do not consume
vertex colours. Known consumers (for example Sprites/Default) and unknown
shaders keep COLOR_0: an unknown shader provides no evidence for suppressing it.
Accessor storage still shares across materials with different colour semantics.

Modern batched submesh i uses `m_Materials[i-firstSubMesh]`. Legacy
`m_Materials[k]` pairs with `m_SubsetIndices[k]`, including permutations:
subsets [2,0,1] and materials [A,B,C] map submesh 2 to A. Under `keep_combined`,
the first selected renderer owning a submesh supplies its binding. Submeshes
beyond the material list remain as default-material geometry with
`submesh_without_material`, matching UnityPy's exporter retention; Unity itself
does not draw these submeshes. Null/unresolved material pointers instead count
`missing_material`. Procedural builtins apply the same binding policy.

Texture fetching is shared with NGUI: inline bytes, own bundle rows (inner to
outer, candidate order, last matching row first), then Environment resources.
Transient candidate names and normalized lookup keys use charged fallible Vecs
and borrowed lookup APIs, avoiding `Bytes::from_owner` for lookup work (CR10).
Successful resolver loads promote their existing name vector into a fallible
shared owner (`triomphe`); computed names and their slices retain charges
without an infallible `Bytes` owner allocation.
The enabled `streamed_texture_host_allocation_refusals` sweeps host refusals as
well as budget refusals, including suffixed candidate names.

Every format in `Formats::DECODED` is eligible. Unsupported formats/platforms,
missing or malformed data omit only the affected slot with a counted reason.
Selection uses the texture's own file version. Planning and decoding use its
platform fields too: Switch GOB deswizzling, RGB promotion and crop, and Xbox
byte swapping before unpacking. `tests/material_platform.rs` checks the
embedded pixels and malformed-layout omissions.
Before any output allocation/work charge, the decode plan checks data length,
signed dimensions, padded pixel count, raw row width and crop bounds. A header
beyond 2^28 padded pixels or the decoder's static row/crop caps is implausible
source data (`texture_too_large`), even with only five data bytes. Empty data
counts `missing_texture`; short/invalid data counts `texture_malformed`.
These static bounds are not runtime resource refusals. After a valid plan, a
meter/work/allocator refusal during decode, conversion or PNG encoding fails
the unit and retains its provenance; fetching/parsing resource refusals also
propagate. One renderer's, material's or texture's malformed data never fails
the unit. Malformed materials roll back their newly embedded resources.

Shader references: [UnityStandardInput.cginc](https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/CGIncludes/UnityStandardInput.cginc),
[UnityCG.cginc](https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/CGIncludes/UnityCG.cginc),
[Standard normal scaling](https://github.com/TwoTailsGames/Unity-Built-in-Shaders/blob/master/CGIncludes/UnityStandardUtils.cginc),
[URP normal scaling](https://github.com/Unity-Technologies/Graphics/blob/master/Packages/com.unity.render-pipelines.core/ShaderLibrary/Packing.hlsl),
[URP metallic/specular sampling](https://github.com/Unity-Technologies/Graphics/blob/master/Packages/com.unity.render-pipelines.universal/Shaders/Utils/MetallicSpecGloss.hlsl),
[URP base UVs](https://github.com/Unity-Technologies/Graphics/blob/master/Packages/com.unity.render-pipelines.universal/Shaders/LitForwardPass.hlsl),
and the [glTF material specification](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#materials).

PNGs use the existing imageio encoder. The output allocation and its charge
transfer directly into the image table; decoded pixels and conversion scratch drop before the next
texture. Images are named `{texture name or property key}.png`. There is no
whole-BIN staging or uncharged image copy. Source caches retain mapping reasons
and replay them once for each affected renderer, including later cache hits.
Reasons appear in collection reports, asset extras and node extras; material
extras also hold mapping skips when a material exists.

The synthetic material tests cover collect → GLB → the pinned validator,
independent PNG unfiltering and pixels, ST lookup after the V flip, normal-map
tangent handedness, alias precedence, shader rules, missing/malformed sources,
cross-file identity and sharing, batch offsets and per-allocation refusals.
Validator-dependent checks print a skip when the executable is not configured;
CI requires it. Editor-built Standard/URP semantic verification remains pending.
