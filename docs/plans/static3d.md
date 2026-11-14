# P2a plan: static 3D

**Status:** accepted, 2026-11-13, with its eight PRs (section 6) and the sixteen decisions of section 0. PR 1 is this document with the `PARITY.md` and `docs/DESIGN.md` changes it calls for. PRs 2, 3 and 4 can start at once, in parallel; the critical path is 2 and 3, then 6, then 8. Review round 1 (Codex, one HIGH, eight MEDIUM, one LOW) is folded in: the metered OBJ sinks (2.2), mesh instances and UVs (2.4), non-finite data (decision 8), root order (decision 13), containment and the PNG peak (section 5), the orientation threshold (3.2) and the static-batching row (`PARITY.md` section 24). One part waits on the project owner: the Editor-built fixture project (decision 6, section 8), on which no PR but the semantic glTF rows' verification waits.

**What it builds on.**
- The core as merged in P1 (`docs/plans/core.md`), through its public API: `ObjectRef::parse_as_dict` and `Parsed` (`Parsed::field` in dict mode, `Parsed::attr` for object-mode clean names), `Environment::deref` across files with UnityPy's resolution, `Environment::resource`, the externals, object and container listings of `SerializedFile`, the meter (`Meter`, `Charge`, `MeteredVec`, `count_decode`, `heap_cost`), `pyfloat::py_float_repr`, and the class ids P2a reads (GameObject 1, Transform 4, Material 21, MeshRenderer 23, MeshFilter 33, Mesh 43, SkinnedMeshRenderer 137, LODGroup 205, RectTransform 224). A need the API does not cover is a request to the core, not an assumption.
- `unityrs-meshdecode` as merged in P1b and after it (section 1.1): every vertex array of a Mesh, the compressed mesh, and `get_triangles`, each against the oracle.
- `unityrs-texture` (P1b's five formats, and ETC and EAC) and `unityrs-imageio` (the Pillow-exact PNG writer), for material textures.

**What it delivers.** The 3D export of a prefab or a scene, as GLB, from the library and the CLI (`unityrs gltf`), with skins, blend shapes, materials, textures and the profile `docs/DESIGN.md` section 4 describes; and UnityPy's OBJ and MTL export (`unityrs obj`), held to UnityPy's text. Animation, and the batch engine's glTF and OBJ output kinds, are P2b's.

**How it is checked.** Where UnityPy 1.25.2 has the behaviour (OBJ and MTL text, the component accessors, the mesh arrays), the oracle is the reference, at level L6 (`docs/DESIGN.md` section 6), and the rows are `PARITY.md`'s, which now give them to P2a (section 3.1). Where UnityPy has none (glTF, scenes, built-in meshes), `PARITY.md` section 24 files the rows under tier X at level L6g, and each names its own reference (section 3.2). A status becomes "verified P2a" only in the PR whose Rust test pins it, as in P1b.

**Citations.** UnityPy behaviour is cited by its `PARITY.md` row (section and item), never by a line of UnityPy's source; the rows hold those lines. Corpus figures are aggregates from the private corpus, "the corpus" below, and nothing else of it is published.

---

## 0. Decisions

| # | decision | why |
|---|---|---|
| 1 | UnityPy's OBJ and MTL rows (`MeshExporter`, `MeshRendererExporter`, `Material`, `Mesh.export`, `Renderer.export`) and the `GameObject` component accessors are P2a's, not P3's. The extractor's GameObject crawl (`extractor.exportGameObject`, `extractor.crawl_obj`) stays P3, with the rest of the extractor. | OBJ is thin on top of the mesh model; it is the only Tier A text check of the geometry glTF reuses; and it shares `get_mesh` and the accessors with the scene graph. `docs/DESIGN.md` puts OBJ and MTL in `unityrs-mesh`, and level L6 is "mesh numbers and OBJ text". |
| 2 | `MeshHandler.copy_from_mesh`'s vertex arrays and `MeshHandler.copy_from_spriterenderdata`'s `vertices` are P4's, with `PARITY.md` section 19.2's "meshes before 3.5.0f5, sprites before 5.6" row (Tier C). `meshdecode` keeps refusing them (`Unsupported::MeshArray`, `Unsupported::SpriteVertices`). | Those arrays exist only before 3.5.0f5 and 5.6, where `process` fails its assert first; the Tier C row that reads them is P4's. With them re-filed, `meshdecode` is complete for P2a. |
| 3 | A new crate, `unityrs-scene`, depending on the core only, holds the scene graph. | `mesh`, `gltf` and P2b's `anim` all need the GameObject and Transform graph without depending on each other. |
| 4 | The object-mode view (`meshdecode`'s `view`, now `pub(crate)`) moves to `unityrs_core::objmode` in PR 2. It is used where UnityPy's object-mode behaviour is the reference (the renderer exporter, `Material`); graph facts are read in dict mode (`Parsed::field`). | Other crates need UnityPyBoost's `parse_class` semantics, and one copy of them. |
| 5 | glTF is not Tier B, which means "UnityPy raises on a valid request": UnityPy has no glTF request at all. `PARITY.md` section 24, "3D beyond UnityPy", files it under a new tier, **X**, at a new level, **L6g**: clean under the pinned glTF-Validator, every accessor equal to L6's arrays under the stated transform (2.4), and the normal-orientation check. | A tier for work with no UnityPy counterpart keeps A and B meaning what they mean; each X row states its reference instead. |
| 6 | The semantic oracle `docs/DESIGN.md` section 3 names for 3D, Editor-built fixtures, is a planned project the owner builds with a licensed Unity Editor (section 8). PRs 2 to 6 do not wait for it; "verified P2a" on the glTF semantic rows does. | Hierarchy, TRS, skins, morphs and material semantics are known by construction only from such a project. Its bundles are checked in, as `CLAUDE.md` allows for Editor-built fixtures. |
| 7 | Built-in meshes: `placeholder` by default (an empty node with extras and a report entry); `resolve` derefs as usual, through a file the user supplies via the existing `CabResolver`; `procedural`, opt-in, generates Cube, Quad and Plane tagged approximate. Their file ids are confirmed on the Editor fixtures. | UnityPy cannot resolve them, and the Editor's resource file cannot ship (2.6). |
| 8 | glTF is spec-valid by default: normals renormalised, tangent w set to ±1, joint weights summed to 1 with negatives clamped, and an all-zero weight row made weight 1 on joint 0. Data no rescaling can repair is handled by rule, never written: a primitive with a non-finite position is refused, with a reported reason; a non-finite or zero normal drops NORMAL (and TANGENT, which needs it) for that primitive, reported. Each rule is documented, and every value it touches is counted in the report and the asset's extras. Profile `normalise: false` writes raw values, documented as possibly spec-invalid, and accepts the validator's errors. OBJ is always raw (Tier A). | Unity data often fails the validator as read; a silent change would be drift, a counted one is not. |
| 9 | A multi-frame blend-shape channel exports its last frame as the morph target, with the frame list in extras. Skins take the top four bone weights from the vertex data. `JOINTS_1`/`WEIGHTS_1` from `m_VariableBoneCountWeights` are P2b's, after the Editor fixtures confirm the layout. | glTF has no in-between frames; the variable-weight layout is not confirmed in any MIT source. |
| 10 | A static-batched renderer is un-batched by default into a world-space node at the scene root, the original node keeping its TRS and `extras.static_batched`; profile `static_batch: keep_combined` is the alternative. | A combined mesh's vertices are in world space; exported under the original node they would be transformed twice. |
| 11 | No batch integration in P2a: the library and the CLI only. P2b's batch unit export adds `glb` and `obj` output kinds under `spec_version` 2. | `docs/DESIGN.md` gives batch unit export to P2b; P2a's crates are built to drop into it (section 5). |
| 12 | P2a's size is about 10k non-test lines (section 6), against the 4-5k `docs/DESIGN.md` section 5 gave it; the table now says so. Materials (PR 7) stay in P2a. | An honest estimate from the PR breakdown; cutting materials would save about 1.4k and leave glTF with factors only. |
| 13 | A scene's root order is the object table's order (`SerializedFile::objects`, never `objects_by_path`, which sorts by signed path id), documented, until the Editor fixtures show the order Unity keeps; it is decided again on them. | Player builds store no root order in a field known today. |
| 14 | Unity's left-handed Y-up becomes glTF's right-handed Y-up by negating X, and Unity's bottom-left UV origin becomes glTF's top-left by `v = 1 - v` (2.4). | UnityPy's OBJ negates X, so OBJ and glTF geometry agree by construction; so do the common Unity glTF exporters. Images stay as the existing PNG output writes them, top row first, which is glTF's image orientation. |
| 15 | A Shader's name is read by a full, metered parse of the Shader, cached per unit. A core partial read ("build only this field path") is requested for P3 only if the corpus measurement shows the cost matters. | The name sits after the large sub-shader arrays, and the core has no partial read. |
| 16 | A texture in a format `unityrs-texture` cannot decode leaves its material with factors only and a report reason, never a unit failure. More formats arrive with P3 without a change here. | P1b's five formats and ETC/EAC are what decode today. |

---

## 1. Where it starts

### 1.1 Done

**`unityrs-meshdecode`** (P1b's E16, then the compressed mesh and UNorm/SNorm normalisation):
- `MeshHandler::process`: channels and streams by version, `m_StreamData` through the Environment's resources, 16- and 32-bit indices;
- the three vertex-format tables and `unpack_vertexdata`;
- CompressedMesh and PackedBitVector (`compressed.rs`, `packed.rs`), with `PARITY.md` section 19.1's UV offset and fourth-weight rows corrected, and `normalize`'s square;
- UNorm, SNorm and colour normalisation (Tier C);
- `get_triangles` for lists, strips and quads.

Every array is charged to the meter, work is counted, and nothing panics. `tests/oracle.rs` replays `tools/fixtures/meshdecode.py`, with an expected-divergence mask built from a corrected-oracle run (UnityPy's source with the Tier C lines fixed), and the same cases run again on UnityPy's own TPK trees with `--tpk`, under the no-write guard (`PARITY.md` section 15.3). So the "rest of meshdecode" the phase table once gave P2a is merged; `docs/DESIGN.md` section 5 now says so.

### 1.2 Not done

- Nothing reads Mesh-level structure: submesh records, `m_LocalAABB`, `m_BindPose`, `m_BoneNameHashes`, `m_RootBoneNameHash`, `m_Shapes`, and `m_VariableBoneCountWeights` (whose layout is to be confirmed). No crate has an OBJ or MTL writer, a GameObject, Transform or component reader, or glTF code.
- `meshdecode`'s own refusals stay: a Mesh's own vertex arrays and a render data's own `vertices` (decision 2), `FixedChannels` below 4 and the mesh's endianness (both P4).
- The object-mode view is private to `meshdecode` (decision 4).
- The batch spec has no mesh or GLB output kind (decision 11).

---

## 2. Crates, modules and APIs

### 2.1 Layout

| crate | new? | depends on | holds |
|---|---|---|---|
| `unityrs-meshdecode` | exists | core | unchanged, but `Processed` gains an owned form (2.2) |
| `unityrs-scene` | new (decision 3) | core | the GameObject and Transform graph across PPtrs, the component accessors (UnityPy's `GameObject` patch), `get_mesh`, scenes, built-in recognition |
| `unityrs-mesh` | new | core, meshdecode, scene | the Mesh model (submeshes, skin, blend shapes, bounds, Unity 6 LOD info), OBJ and MTL (`MeshExporter`, `MeshRendererExporter`), and Python's number formats (`{:.9G}`, `{:.4f}`) |
| `unityrs-gltf` | new | core, scene, mesh, texture, imageio | the scene model, collect, the GLB writer, the material trait, the profile |
| `unityrs-testkit::scene3d` | new module | — | hand-built trees and files for 3D scenes |
| `unityrs-cli` | exists | gltf, mesh | `unityrs obj` and `unityrs gltf` |

Every new crate keeps `meshdecode`'s rules: `#![forbid(unsafe_code)]` and the no-panic clippy denies; the `public_types` Send + Sync test and the `waits` test (no thread, no wait); a UnityPy-ported file's `Ported from UnityPy 1.25.2` header, with the crate's `LICENSE-UNITYPY` link and its `(MIT OR Apache-2.0) AND MIT` licence; and no file over 800 lines (`scripts/file-size-check.sh`).

### 2.2 `unityrs-mesh`: the Mesh model

```rust
pub struct Mesh {                         // owned; charged to the meter while alive
    pub name: RawString,                  // m_Name as read
    pub processed: ProcessedOwned,        // meshdecode's arrays: f64, with Python's int/float tags
    pub submeshes: Vec<SubMesh>,          // firstByte, indexCount, topology, baseVertex, firstVertex, vertexCount, localAABB
    pub triangles: Triangles,             // get_triangles, per submesh
    pub bounds: Option<Aabb>,             // m_LocalAABB
    pub bind_poses: Vec<[f32; 16]>,       // m_BindPose, or the compressed mesh's below 5
    pub bone_name_hashes: Vec<u32>,
    pub root_bone_name_hash: Option<u32>,
    pub blend_shapes: BlendShapes,        // m_Shapes: sparse deltas, shapes, channels (name, hash, frames), full weights
    pub lod: Option<MeshLods>,            // Unity 6 mesh LOD data, its field names confirmed on 6000.x trees first
    charge: Charge,
}
pub fn read_mesh(obj: &ObjectRef, env: &Environment, meter: &Arc<Meter>) -> Result<Mesh, MeshError>;
pub mod obj {   // PARITY.md section 15.4
    pub fn export_mesh_obj(m: &Mesh, material_names: Option<&[Option<&RawStr>]>, out: &mut dyn MeteredWrite)
        -> Result<Written, MeshError>;          // Written::False: UnityPy's False, nothing written
    pub fn export_mesh_renderer(r: &RendererRef, env: &Environment, meter: &Arc<Meter>, sink: &mut dyn ObjSink)
        -> Result<RendererExport, MeshError>;   // section 19.1's rules; each file through a MeteredWrite of the sink's
    pub fn export_material(m: &MaterialView, out: &mut dyn MeteredWrite) -> Result<(), MeshError>;
    pub fn export_mesh_obj_owned(m: &Mesh, material_names: Option<&[Option<&RawStr>]>, meter: &Arc<Meter>)
        -> Result<Option<ChargedBytes>, MeshError>;   // the convenience form: a charged byte owner
}
pub mod pyfmt { pub fn g9(x: PyNum, buf: &mut PyFmtBuf) -> &str; pub fn f4(x: f64, buf: &mut PyFmtBuf) -> &str; }
```

No OBJ or MTL text is a bare `String`. It is written into a caller-supplied metered sink, `MeteredWrite`, an `io::Write` whose every growth is charged first and fallible (core 3.15), so a refusal is an error, never an abort; the convenience form returns a `ChargedBytes` that keeps its charge for as long as the bytes live. On an error, what the sink holds is the caller's to discard. `export_mesh_obj` decides UnityPy's `False` (no vertices, or no positions) before it writes anything. `pyfmt` formats into a fixed stack buffer (`PyFmtBuf`, sized for `{:.4f}` of the largest double, under 320 bytes), so it allocates nothing. `Processed` borrows its `Parsed`; it gains `into_owned()` (or the Mesh keeps the `Parsed` in an `Arc` and borrows again). The owned arrays stay **f64 with the int/float tag**: OBJ text depends on it, since UnityPy negates x, so a compressed normal left as an `int` zero prints `0` where a float prints `-0`. glTF converts to f32 per accessor as it streams (section 5).

### 2.3 `unityrs-scene`: the scene graph

```rust
pub struct Graph { nodes: Vec<Node>, roots: Vec<NodeId>, issues: Vec<Issue>, charge: Charge }
pub struct Node {
    pub game_object: ObjectRef, pub transform: ObjectRef, pub kind: TransformKind, // Transform or RectTransform
    pub name: RawString, pub active: bool, pub layer: u32, pub tag: u16,
    pub local: Trs,                        // m_LocalPosition, m_LocalRotation, m_LocalScale, f32, Unity's space
    pub parent: Option<NodeId>, pub children: Vec<NodeId>,          // m_Children's order
    pub components: Vec<Component>,                                 // m_Component's order
}
pub enum Component { MeshFilter { mesh: Option<PPtrTarget> }, MeshRenderer(RendererInfo),
                     SkinnedMeshRenderer(SkinInfo), LodGroup(LodInfo), Other(i32, ObjectRef) }
pub enum PPtrTarget { Object(ObjectRef), Builtin(BuiltinRef), Unresolved(DerefErrorKind) }

pub fn graph_from_game_object(env: &Environment, root: &ObjectRef, opts: &GraphOptions, meter: &Arc<Meter>)
    -> Result<Graph, SceneError>;      // a prefab: a container entry
pub fn graph_from_scene_file(env: &Environment, file: &Arc<SerializedFile>, opts: &GraphOptions, meter: &Arc<Meter>)
    -> Result<Graph, SceneError>;      // a scene
pub mod accessors {                     // PARITY.md sections 14.2 and 19.2: Tier A, B for an empty list and 5.4's pairs
    pub fn components(go: &Parsed) -> Result<Vec<PPtr>, SceneError>;
    pub fn m_transform(/* .. */);      // and m_mesh_filter, m_animator, m_animation, m_mesh_renderer, m_skinned_mesh_renderer
    pub fn get_mesh(renderer: &ObjectRef, env: &Environment, meter: &Arc<Meter>) -> Result<Option<ObjectRef>, SceneError>;
}
```

`RendererInfo` holds the materials, the static-batch range (`firstSubMesh`, `subMeshCount`), `m_SubsetIndices`, `m_Enabled` and the static-batch root; `SkinInfo` adds the mesh, `m_Bones`, `m_RootBone` and `m_BlendShapeWeights`.

**Rules.**
- The walk starts at the root Transform and follows `m_Children`. Each child's `m_Father` is checked against its parent, and a mismatch is an `Issue`.
- A visited set keyed by (file identity, path id) makes a cycle an `Issue`, never a loop. Depth is bounded by `Limits::max_tree_depth`, and every node is charged.
- A PPtr that does not resolve (a missing dependency, a built-in) becomes `PPtrTarget::Unresolved` or `Builtin`, with its reason. Only a root that fails is an error.
- Graph facts are read in dict mode. The accessors that have UnityPy rows follow `classes/legacy_patch/GameObject.py`, with section 19.2's Tier B answers; the Python binding of them stays P3.

### 2.4 `unityrs-gltf`: two phases, the material trait, the profile

**The model.** An editable scene between the two phases; P2b extends it with animation.

```rust
pub struct Scene { pub nodes: Vec<GNode>, pub roots: Vec<usize>, pub meshes: Vec<GMesh>, pub skins: Vec<GSkin>,
                   pub materials: Vec<GMaterial>, pub images: Vec<GImage>, pub extras: Extras, pub report: Report }
// GNode: name, TRS in glTF's space, mesh, skin, children, extras. GMesh: primitives (attribute arrays as f32, u16
// or u8 views over charged buffers), sparse morph targets, target names. GSkin: joints, inverse bind matrices, skeleton.
```

**Collect:** `collect(env, Graph, &Profile, &dyn MaterialMapper, meter) -> Result<Scene, GltfError>`.
- Meshes, materials and textures are resolved and de-duplicated per unit by (file, path id). **Mesh instances:** a source Mesh's geometry buffers (its accessors) are written once per unit and shared; a glTF mesh is keyed by (source Mesh, material bindings), so two renderers that share a Mesh with different materials get two glTF meshes over the same accessors.
- **Coordinates** (decision 14): positions, normals and tangents become (-x, y, z); rotations (x, -y, -z, w); winding flips to (c, b, a), as the OBJ's faces do; bind poses are conjugated by diag(-1, 1, 1, 1).
- **UVs** (decision 14): every TEXCOORD_n becomes (u, 1 - v), and images are embedded as the existing PNG output writes them, top row first. A `KHR_texture_transform` from Unity's `m_Scale` (sx, sy) and `m_Offset` (ox, oy) keeps its scale and takes offset (ox, 1 - sy - oy), the same lookup in glTF's flipped space. Tangent w is negated (after decision 8's ±1). Unity and glTF both rebuild the bitangent as w × cross(N, T), and both read a normal map's green channel as up in the image. The image keeps its content, and the V flip only readdresses the same texels, so the bitangent must stay the mirror image of Unity's. Negating x flips cross(N, T), and only negating w restores it. An asymmetric textured fixture, with a normal map, pins the image's orientation, the offset and w's sign (4.3).
- **Skins:** `m_Bones` become the joints, `m_BindPose` the inverse bind matrices, `m_RootBone` the skeleton; weights and indices come from the vertex data, the top four (decision 9). Every skinned mesh attachment is an identity-TRS scene root, separate from its original Transform node. The original hierarchy keeps joint world transforms; applying renderer TRS again would double-transform the skin. This also satisfies the writer's normalise-mode refusal of parented or transformed skinned mesh nodes.
- **Blend shapes:** sparse morph targets from `m_Shapes`' deltas, a multi-frame channel's last frame with the frame list in extras (decision 9); `m_BlendShapeWeights` are per renderer, so they go in the node's `weights`, not the shared mesh's.
- **Static batching** (decision 10): a renderer with `m_StaticBatchInfo` and `subMeshCount > 0` reads that range of the combined mesh, whose vertices are in world space.
- **LODGroup:** the profile picks `lod0` or `all`.

**Write:** `to_glb(&Scene, &Profile, sink: &mut dyn Write) -> Result<GlbStats, GltfError>`, and optionally `to_gltf` with separate JSON and BIN sinks.
- **Deterministic bytes:** the writer fixes the JSON key order, writes floats by shortest round trip, orders buffer views by first reach, and aligns the BIN to 4 bytes.
- A layout pass computes every buffer view's offset and the total length first, which the GLB header needs, then streams JSON and BIN, so the BIN is never built whole. An embedded image's length is only known once it is encoded, so every PNG is encoded before the layout pass and kept, charged, until it is written (section 5); none is encoded twice.
- POSITION accessors carry min and max. Indices are u16 where the largest index allows, else u32.
- Every value written is spec-valid by decision 8's rules (or the primitive or attribute is refused or dropped, by those rules), and the count of values each rule touched goes into the report and the asset's extras.

**The material trait.**

```rust
pub trait MaterialMapper: Send + Sync {
    fn map(&self, m: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<GMaterial, GltfError>;
}
// MaterialInput: name, shader name (decision 15), render queue, m_SavedProperties (TexEnvs with m_Scale and m_Offset,
// Floats, Colors, Ints), keywords. MapCx::texture(ptr) -> Option<ImageId> decodes through unityrs-texture and encodes
// a PNG through unityrs-imageio, de-duplicated; a format it cannot decode is a Report entry (decision 16).
pub struct UnlitExtras; // KHR_materials_unlit: base colour from _MainTex/_BaseMap and _Color/_BaseColor, every
                        // allowlisted property in extras
pub struct CorePbr;     // base colour; normalTexture (_BumpMap/_NormalMap, scale); metallic and roughness from
                        // _Metallic, _Glossiness, _Smoothness; emissive; alphaMode (MASK on _Cutoff/_AlphaClip,
                        // BLEND on a queue of 3000 or more or _Surface 1); doubleSided (_Cull 0);
                        // KHR_texture_transform from m_Scale and m_Offset
```

**The profile.** Versioned JSON (`profile_version: 1`), parsed strictly: an unknown key is refused, as the batch spec refuses one. Its fields are those `docs/DESIGN.md` section 4 lists: `root_naming`, `layer_skips`, `shader_rules` (drawable, additive and skip, by name pattern), `inactive` (`skip`, `include`, or `include_hidden` with extras), `material_property_allowlist`, `scene_roots` with `exclusions`, `builtin` (2.6), `extras_naming`, `export_metadata` (the generator, the source, and optionally the input's SHA-256), `lod`, `static_batch` and `normalise`. `clip_states` is P2b's: it parses, and is refused while it is not empty.

### 2.5 Scenes

**Scene files.** A scene is a SerializedFile, a player's `levelN` or a scene bundle's `BuildPlayer-<name>` CAB, with its `.sharedAssets` sibling. Its roots are the Transforms whose `m_Father` is null, in the object table's order, `SerializedFile::objects` (decision 13); children follow `m_Children`. The profile's `scene_roots` and `exclusions` filter roots by name or path.

**Prefabs.** Each container entry of an AssetBundle whose PPtr targets a GameObject is one unit, rooted at that GameObject's Transform. P2b's batch export keys on this unit.

**Not exported.** Cameras, lights, terrain and particle renderers appear only as profile-gated `extras.unity` metadata.

### 2.6 Built-in meshes

UnityPy does nothing special with them. A renderer or MeshFilter PPtr whose external is `Library/unity default resources` (or `Resources/unity_builtin_extra`, for the default material) goes through `PPtr.deref`, which finds no loaded file, so `get_mesh` raises `FileNotFoundError`; it resolves only if the user loads a copy of the Editor's own resource file, which cannot ship. The core does the same (a `DerefError`).

`unityrs-scene::builtin` recognises the two externals by their path (the basename rule of `PARITY.md` section 7.2) and names the known file ids: Cube 10202, Cylinder 10206, Sphere 10207, Capsule 10208, Plane 10209, Quad 10210, Default-Material 10303, each to be confirmed on the Editor fixtures before its row is verified. The profile's `builtin` takes decision 7's three modes; `procedural` tags its nodes `extras.approximate = true`, since they are not Unity's vertex sets.

### 2.7 Core APIs P2a needs

| need | status |
|---|---|
| `parse_as_dict`, `Parsed::field` and `Parsed::attr`, `deref`, `resource`, the externals and object listings, the meter | exist |
| an object-mode view shared beyond `meshdecode` | moves to `unityrs_core::objmode` in PR 2 (decision 4) |
| a Shader's name without a full parse | a full metered parse, cached per unit (decision 15) |
| a texture's data (`image_data`, or `m_StreamData` through `Environment::resource`) | the logic exists in `unityrs-export`'s NGUI reader; it is lifted to a shared helper, not copied |
| `unityrs_texture::contain` (`catch_unwind`) | exists, for a closure that reserves nothing; collect and write wrap only such steps (section 5) |

---

## 3. Parity targets

### 3.1 UnityPy's rows P2a owns

Decision 1 moves these `PARITY.md` rows to P2a:

| row (`PARITY.md` section) | tier | level | P2a work |
|---|---|---|---|
| `MeshExporter.export_mesh` (15.4) | A | L6 | the dispatch: `"obj"` only |
| `MeshExporter.export_mesh_obj` (15.4) | A | L6 | an exact port, with a port of Python's `{:.9G}` |
| OBJ NaN and infinity (19.2) | A | L6 | the same text, `NAN` and `INF` |
| vertex-less mesh export (19.2) | A (the `False`); C (the extractor) | L6, L9 | `None`; the extractor's part stays P3 |
| `Mesh.export` (14.2) | A | L6 | the library method and the CLI |
| `MeshRendererExporter.get_mesh` (15.4) | A | L6 | `scene::accessors::get_mesh` |
| `MeshRendererExporter.export_mesh_renderer`, `Renderer.export` (15.4, 14.2) | C | L6 | section 19.1's rules: submesh `i` takes `m_Materials[i - firstSubMesh]`, and textures are written as `{name or key}.png` |
| `MeshRendererExporter.export_material`, `Material` (15.4, 14.2) | A, C | L6 | the defaults exact (A); colours and texture maps written (C) |
| MeshRendererExporter fails on Unity 6, renderer materials before `firstSubMesh` (19.1, 19.2) | C | L6 | divergence tests |
| `GameObject.m_Components` and the six accessors (14.2, 19.2) | A; B for an empty list and 5.4's pairs | API, L1 (env) | Rust accessors with Tier B's answers; the binding stays P3 |

Decision 2 moves `MeshHandler.copy_from_mesh`'s vertex arrays and `MeshHandler.copy_from_spriterenderdata`'s `vertices` (15.3) to P4. Mesh endianness, 3.x strips and `get_channels` stay P4; the extractor's GameObject crawl stays P3; `Transform` stays verified at L2.

**HEAD tier.** P2a owns no row of `PARITY.md` section 20. It reads 6000.x Mesh, GameObject and Transform trees through the existing embedded and TPK paths.

### 3.2 Beyond UnityPy: tier X, level L6g

`PARITY.md` section 24 files glTF, scenes and built-ins under tier X (decision 5). Each row states its reference, from these:

| reference | what it checks |
|---|---|
| **Khronos glTF-Validator**, pinned | structure: zero errors, and warnings only from a committed allowlist of codes |
| **the L6 arrays** | accessor content: each POSITION, NORMAL, TANGENT, TEXCOORD_n, COLOR_0, JOINTS_0, WEIGHTS_0 and index accessor equals UnityPy's `MeshHandler` arrays (the corrected oracle where Tier C applies) under 2.4's transform, rounded from f64 to f32 to nearest, after decision 8's rules. So glTF content is anchored to the Tier A oracle, not to an exporter of our own |
| **the OBJ** | the same positions, normals and faces as `export_mesh_obj`'s text, by construction: one transform. UVs differ by the stated V flip, which the OBJ, bottom-left like Unity, does not apply |
| **the normal-orientation check** | per primitive, at least 99% of the eligible triangles agree: a triangle's winding normal has a positive dot product with the mean of its three vertex normals. Eligible means the primitive has normals and the triangle is not degenerate: its area is above 1e-12 times the square of the diagonal of the primitive's POSITION bounds. A primitive with no eligible triangle is N/A, recorded and not counted as a pass. It catches handedness and winding flips |
| **Editor-built fixtures** | `docs/DESIGN.md` section 3's 3D oracle: hierarchy, TRS, skins, morphs and materials known by construction (decision 6, section 8) |

Level **L6g** (`docs/DESIGN.md` section 6) is the first four together: validator clean, accessors equal to L6 under the stated transform, and the orientation check.

**Third-party glTF readers** do not join the oracle stack: `oracle_env.py --exact` refuses unpinned packages, and trimesh brings a numpy of its own. A standard-library GLB reader, `tools/parity/gltf_check.py`, reads what the checks need (a GLB is a 12-byte header, a JSON chunk and a BIN chunk). Import into Blender is a manual smoke test, outside CI.

---

## 4. Verification

### 4.1 L6 numbers and OBJ text, against the oracle (CI)

P1b's pattern: the recorder runs in the oracle image on inputs built in code, its JSON is committed, and Rust replays it without Python.
- **`tools/fixtures/mesh_export.py`** builds Meshes, GameObjects, renderers and Materials on hand-built trees, extending `meshdecode_trees.py` from UnityPy's MIT field lists, with format facts (order, alignment) cited from AssetStudio's MIT readers. No TPK-derived tree is committed (`docs/plans/core.md` section 9, Q2). It records:
  - `obj.json`: `export_mesh_obj`'s text, or `False`, for NaN, infinities, -0, 1e-10, a compressed mesh's `int` zero normals, a missing UV0 or normal, every topology, material lists with `None` and short ones, and names that are not UTF-8;
  - `renderer.json`: UnityPy's exceptions, and a corrected-oracle run, `MeshRendererExporter`'s source with section 19.1's lines fixed and compiled in memory as `meshdecode.py`'s fixes are, which gives the Tier C rows a UnityPy-derived reference and an expected-divergence mask;
  - `accessors.json`: the `GameObject` accessors, the Tier B cases included;
  - `pyfmt.json`: Python's `format(x, '.9G')` and `'{:.4f}'` over a grid of doubles (random bits, the boundaries at exponents -5, -4, 8 and 9, ties, subnormals, ±0, NaN and the infinities).
- **`--tpk`:** each case runs again on UnityPy's TPK trees at run time, under the existing no-write guard.
- **Rust replays:** `crates/unityrs-mesh/tests/obj_oracle.rs`, `renderer_oracle.rs` and `pyfmt.rs`; `crates/unityrs-scene/tests/accessors_oracle.rs`; and the divergence tests `PARITY.md`'s rows name, as `test_parity_rows.py` every_status_agrees_with_its_test_column requires.
- **UnityPy's samples**, which CI's `parity-py` job fetches: a mesh dump over every Mesh in them compares L6 arrays and OBJ text, committing no output.

### 4.2 glTF structure (CI)

- A Khronos glTF-Validator release binary is pinned by version and SHA-256 and run in a new `gltf` CI job (actions pinned by SHA, a `timeout-minutes`, as `scripts/tests/test_ci_workflow.py` checks). It is a tool, not a dependency: it stays out of the release artefacts and the licence notices.
- It checks the GLBs `crates/unityrs-gltf/tests/*` write into testkit scratch. The Rust test skips with a printed notice unless `UNITYRS_GLTF_VALIDATOR` is set, which the job sets.
- The gate: no error, and warnings only from a committed allowlist of codes, each with its reason (expected: unused objects beside extras-only nodes, and degenerate triangles UnityPy keeps).
- `tools/parity/gltf_check.py` checks accessors against the oracle's `MeshHandler` arrays (4.1) and runs the orientation check.
- Determinism: each scene is written twice, and from 1 and from N threads of the caller, and the GLBs are byte-equal.

### 4.3 Fixtures

**Exist:** `meshdecode`'s hand-built trees and cases; the testkit's SerializedFile, bundle and Environment builders; the NGUI Texture2D and material builders; UnityPy's pinned samples (few meshes, and game data, never committed).

**To build:**
1. `unityrs-testkit::scene3d`: GameObject, Transform, RectTransform, MeshFilter, MeshRenderer, SkinnedMeshRenderer, Material, LODGroup and Mesh writers at the 2019.4, 2022.3 and 6000.x layouts, and scenes built from them: a prefab in bundle A with its mesh in B and its material in C (cross-file PPtrs through `deref`); a skinned mesh with bones, bind poses and blend shapes, multi-frame ones included; a static-batched combined mesh; a scene file with several roots, whose object table's order differs from its path ids' order (decision 13); an asymmetric texture on a quad, with a normal map, whose orientation shows after the V flip (2.4); built-in externals; and hostile inputs (an `m_Father`/`m_Children` cycle, a parent mismatch, a dangling PPtr, huge counts, a negative file id).
2. The Editor fixture project (section 8).
3. Fuzz targets `scene_collect` (testkit scenes, mutated) and `obj_text`, seeded from the testkit as the existing ones are (`fuzz/`, CI's `fuzz-build`).

### 4.4 Corpus checks (aggregates only)

- **L6:** `tools/parity/mesh_dump.py` (the oracle) and a mesh mode of `unityrs-dump` write one JSONL record per Mesh: array digests over the f64 bits and type tags, a triangles digest, the OBJ text's SHA-256, or the exception's class; they are compared under the mask. The corpus is Unity 6000.3, where UnityPy's renderer exporter fails (Tier C), so renderers compare with the corrected-oracle run.
- **glTF:** every container prefab and every scene of the corpus is written as GLB. The gates: no validator error and no accessor difference; the counts of normalised values, unresolved PPtrs, built-ins and skipped textures (by reason), and peak charged bytes, are reported.
- Only aggregate counts are published.

---

## 5. Performance and memory

- **Charges.** Every buffer whose size an input chooses is charged before it is allocated, as `docs/plans/extract.md`'s E59 and core 3.15 require of every crate: graph nodes, the owned mesh arrays (already charged in `meshdecode`), f32 accessor staging, sparse morph arrays, JSON text, decoded texture pixels and PNG output. Work is counted with `count_decode` before each loop. An overrun is `LimitKind::InputMemory` or `DecodeWork`.
- **Peak.** Within a unit, roughly the decoded meshes (f64, about 8 bytes a component), the sum of the encoded PNGs plus the largest texture decode, and the JSON. Textures are decoded and encoded one at a time, each decode freed once its PNG is made; the PNGs stay charged until the BIN writes them, since the layout needs their lengths (2.4). Re-encoding instead of keeping them would double the encode cost. The BIN streams from the meshes, converting f64 to f32 per accessor in fixed 64 KiB chunks, and is never built whole.
- **Caches.** Meshes, materials and textures are de-duplicated per unit by (file, path id). A cross-unit LRU (prefabs of one bundle sharing meshes) is bounded by a byte budget charged to the meter, and off by default.
- **Reuse in batch.** The crates run on the caller's thread with no waits (the testkit's `waits` scan), are Send + Sync, and contain panics only around steps that reserve nothing: a unit's decode and collect steps are each wrapped in `contain`, and every reservation on the meter happens outside any contained closure, so a caught panic cannot leave bytes charged. So P2b drops them into the engine's sealed attempts unchanged, dependency bundles coming through the view's Environment within `dep_cache_bytes`. An X12-style counting-allocator test (heap at most the charges plus a slack) covers collect and write, and a squeeze test runs in a child (`unityrs_testkit::child`).
- **Measured first:** decode time and peak for the corpus's 50 largest Meshes; the Shader parse's cost per material, which decides decision 15's request; GLB write throughput.

---

## 6. PRs

About 1-2.5k non-test lines each.

| # | PR | contents | ~non-test lines | depends on |
|---|---|---|---|---|
| 1 | plan and PARITY | this document; `PARITY.md`: the OBJ, renderer, Material and accessor rows to P2a (decision 1), the old vertex arrays to P4 (decision 2), section 24 with tier X and level L6g (decision 5); `docs/DESIGN.md`'s phase, crate and level tables (decisions 3, 5, 12) | docs | — |
| 2 | `unityrs-mesh` model | `objmode` in the core (decision 4), `Processed`'s owned form, `read_mesh`, submeshes, bounds, bind poses, blend shapes, bone hashes, LOD info, charges and work; the testkit's Mesh writers | 1.4-1.8k | — |
| 3 | `unityrs-scene` graph | the crate, `Graph`, `graph_from_game_object` and `graph_from_scene_file`, the accessors (A and B), `get_mesh`, built-in recognition, issues, the cycle and depth bounds; `testkit::scene3d`; the accessors part of `tools/fixtures/mesh_export.py` | 1.3-1.7k | — |
| 4 | glTF model and GLB writer | `Scene`, the layout pass, deterministic JSON, the streamed BIN, accessors with min and max, sparse morphs, skins; the validator's CI job and allowlist; tests on hand-built `Scene`s, with no Unity input | 1.2-1.6k | — |
| 5 | OBJ and MTL | `pyfmt`, `export_mesh_obj`, `export_mesh_renderer` and `export_material` (Tier A, and C corrected), the `obj.json`, `renderer.json` and `pyfmt.json` fixtures with corrected-oracle masks, the divergence tests; `unityrs obj`; the samples' L6 check in `parity-py` | 1.3-1.8k | 2, 3 |
| 6 | glTF collect and profile | Unity to `Scene`: the coordinate conversion, a primitive per submesh, skins, morph targets, static batching, LOD, inactive nodes, layers, extras; the strict profile parser; `gltf_check.py`'s accessor and orientation checks | 1.8-2.3k | 2, 3, 4 |
| 7 | materials | `MaterialMapper`, `UnlitExtras`, `CorePbr`, shader rules, `KHR_texture_transform`, the shared texture-fetch helper, PNGs through `unityrs-imageio`, skip reasons | 1.2-1.6k | 6 |
| 8 | scenes, built-ins, CLI, corpus | scene and prefab units, root order, the `builtin` modes (procedural Cube, Quad and Plane), `unityrs gltf`, the fuzz targets, `mesh_dump.py` and the dump's mesh mode, the X12 and squeeze tests | 1.3-1.8k | 6 (7 optional) |

**Total: about 9.5-12.5k non-test lines**, which `docs/DESIGN.md` section 5 now gives as about 10k (decision 12). PRs 2, 3 and 4 run in parallel from the start, and 5 overlaps 6.

---

## 7. Risks

1. **Python's number formatting.** `{:.9G}`, `int` against `float`, `-0` and NaN all change OBJ bytes. The `pyfmt` grid fixture, and Python's type tags kept through `ProcessedOwned`.
2. **A lost int/float tag.** A refactor that turns `Values::Mixed` into plain f64 silently changes `vn` lines (`0` against `-0`); a fixture case pins it.
3. **Static batching.** Most built scenes use combined meshes in world space, which a naive export transforms twice. Decision 10's rule waits on the Editor fixtures for confirmation, and `PARITY.md` section 24's static-batching row is verified only on them.
4. **No Editor fixtures yet.** Until they exist, glTF content is as right as L6 and the transform, and hierarchy, TRS and material semantics are unverified (section 8).
5. **The validator against Unity data.** Non-unit normals, weights that do not sum to 1, negative or more than four influences, NaN and degenerate triangles are validator errors. Normalisation changes values, so it is documented, counted and reported, never silent (decision 8).
6. **Cross-file PPtrs in batch.** A missing dependency is an `Issue`, never a failed unit; each dependency's charge sits under the attempt's `D`, and a large one can take a unit past `M + D`.
7. **Shader names** cost a full Shader parse (megabytes) per material without a partial read: cached, and measured on the corpus (decision 15).
8. **Scope.** The estimate doubled (decision 12).
9. **Unity 6 mesh LOD and variable bone weights.** Their field names and layouts are not confirmed in the MIT sources; they are confirmed on the corpus's trees (aggregates only) before they are coded.
10. **Determinism across threads and hosts.** f64 to f32, `{:.9G}` and normalisation must not depend on libm: only correctly rounded operations, as `normalize`'s square fix already uses.
11. **The validator binary** is a third-party download in CI: pinned by SHA-256, and kept out of the release and the licences.

---

## 8. The Editor fixture project (owner-built)

`docs/DESIGN.md` section 3 names Unity-Editor-built fixtures as the 3D oracle, and none exist yet. The project owner is to build a minimal project with a licensed Unity Editor; no date is set. It holds:
- the primitives (Cube, Cylinder, Sphere, Capsule, Plane, Quad) and the default material;
- a skinned mesh with blend shapes, in-between frames included;
- a static-batched scene;
- a LODGroup;
- Built-in and URP materials;
- built at 2022.3 and at 6000.x.

Its bundles are checked in, as `CLAUDE.md` allows for Editor-built fixtures. What waits on it: "verified P2a" on section 24's semantic rows (hierarchy and TRS, skins and morphs, materials, scene root order, built-in file ids), decision 9's `m_VariableBoneCountWeights` layout, decision 10's static-batching rule, and decision 13's root order. PRs 2 to 6 do not.
