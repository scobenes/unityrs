# unityrs-gltf

PR 4 supplies an editable glTF-space model, `layout`, and `write_glb`. The
workspace includes it through `crates/*`. Its only production dependency is
core. Material slots currently serialize their name, extras and glTF defaults.
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
Duplicate positive joint influences are refused, rather than silently merged.
Palette bounds use only the rows of surviving primitives, after omission
decisions and joint repair in normalise mode; raw mode checks the original
indices, including zero-weight slots. Maxima are cached once per emitted
JOINTS accessor, independently of node instances.

Normalise mode refuses skinned mesh nodes with a parent or a non-identity local
transform. The model exposes TRS transforms; matrix transforms must be resolved
by the caller before constructing a node. The collect pass (PR 6) will place
skinned mesh nodes at scene roots with identity transforms, as common exporters
do. Raw mode writes their parent relationships and local transforms as given.

Non-finite rounded positions omit their referencing primitives; invalid or zero
normals omit NORMAL and TANGENT. Invalid tangent xyz omits TANGENT. If that
leaves a morph target empty, its primitive is also omitted. A mesh left empty
is omitted and its nodes keep transforms and metadata. Other malformed values
or references are errors before output begins. `normalise: false` preserves
rounded payload bits, including NaN and infinity, and may fail validation;
JSON itself remains valid. Morph deltas are never unit-normalized.

The model already uses glTF coordinates and UVs. Collection will negate Unity
X, flip winding and V, and negate tangent w consistently. That conversion,
Unity input, material mapping, profiles and the CLI are later PRs.

Tests use hand-built scenes only. The committed empty, triangle and skinned
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
