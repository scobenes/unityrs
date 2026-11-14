# unityrs-gltf

The crate supplies an editable glTF-space model, `collect`, a strict
`Profile`, `layout`, and `write_glb`. Collection uses core, scene, mesh and
meshdecode; it does not depend on the CLI or a material mapper. Material slots currently serialize their name, extras and glTF defaults.
Images are already-encoded PNG/JPEG owners; no image encoding dependency or
second encoding is needed. A bounded metadata walker mirrors Khronos
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
by skin inverse binds and images. Shared accessors are placed on first reach;
unreferenced accessors are omitted. Value checks run only for emitted data,
with POSITION/NORMAL/TANGENT analysed first to decide omissions. Each view
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
conversion described below. Material mapping is PR 7; the CLI is PR 8.

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


`collect(&env, &graph, &profile, &meter)` returns a `Scene`; write it with
`write_glb(&scene, &profile.options(), &meter, &mut sink)`. No material is bound
in this PR, so primitives use glTF's default material. Shader rules and material
allowlists are typed and retained for PR 7's mapper.

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
Invalid source geometry, including non-finite UV/color/morph values in
normalise mode, leaves a reported empty node with `unresolved_renderer`.
UV checks include overflow when rounding decoded values to f32.
Malformed batch ranges and decoded attribute counts differing from POSITION
likewise omit only their renderer. Missing mesh pointers are reported too.
Memory, allocation, work and other resource refusals still propagate.
Raw-mode duplicate palettes retain signed
weights while merging matching influences.
Morph-only renderers with no bones keep their ordinary hierarchy and have no
skin. Four vertex influences are used; variable-count weights remain P2b's.

Geometry, indices, sparse targets and inverse bind accessors are shared by
source file identity and path id. Mesh variants reference those same accessors
for different submesh selections. Nonempty submeshes each produce one
triangle-list primitive; empty submeshes produce none. Base vertices are added
with checked arithmetic. UV sets retain their indices; a missing set before a
present one is explicitly filled with zero UVs to satisfy glTF's contiguous
semantic requirement. Morph channels select their last frame; sorted sparse
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
| `shader_rules` | object with `drawable`, `additive`, `skip`, each an array of string name patterns; reserved for PR 7 | empty lists |
| `inactive` | `skip`, `include`, `include_hidden` | `skip` |
| `material_property_allowlist` | array of strings; reserved for PR 7 | `[]` |
| `scene_roots` | array of exact root names or slash-separated paths; an empty list selects every root | `[]` |
| `exclusions` | array of exact node names or slash-separated paths | `[]` |
| `builtin` | `placeholder`, `resolve`, `procedural` | `placeholder` |
| `extras_naming` | `unity` (source name, path id, layer, activity), `none` | `unity` |
| `export_metadata` | object with optional string `generator`, `source`, `input_sha256` (exactly 64 hex digits); saved as scene extras; asset generator remains the writer's identity | empty object |
| `lod` | `lod0`, `all` | `lod0` |
| `static_batch` | `unbatch`, `keep_combined` | `unbatch` |
| `normalise` | boolean; false may produce spec-invalid BIN values | true |
| `clip_states` | empty array; nonempty arrays require P2b | `[]` |

Built-in placeholders retain the numeric id in `unity_builtin`. `resolve`
requests the supplied resource through the Environment and diagnoses a missing
file/object. `procedural` supports approximate Cube, Quad and Plane geometry,
tagged `approximate`; unsupported built-ins and procedural skinned renderers
leave a reported empty renderer. Malformed resolved resources degrade only
their renderer; resource, meter, limit and allocation refusals propagate.
Repeated `unity_builtin` markers contain a sorted, unique set of numeric ids.
No Editor resource geometry is shipped. Metadata strings remain independently
charged. Invalid UTF-8 source names use replacement characters in glTF JSON.

`tools/parity/gltf_check.py` independently reads dense/strided/sparse accessors,
checks Unity-space expected arrays after f32 conversion, and checks orientation.
At least 99% of eligible triangles must agree with the mean vertex normal;
area must exceed 1e-12 times the squared POSITION-bounds diagonal. No normals or
no eligible triangles is reported as N/A. CI runs its Python self-tests and
Rust collect→GLB checks, including replay of pinned L6 position arrays. Material
and normal-map/image fixtures join in PR 7; Editor semantic rows remain planned.
