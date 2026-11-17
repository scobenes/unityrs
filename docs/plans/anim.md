# P2b plan: animation

**Status:** draft, 2026-11-17, with nine PRs (section 6) and the proposed decisions of section 0. The clip reader, discovery, the glTF writer and the fixture recorder can start in parallel. Humanoid verification and variable bone weights need the Editor fixtures (section 8).

**What it builds on.**

- The core's checked `ObjectRef::parse_as_dict`, embedded and provider typetrees, `Environment::deref` across files, and the allocation and work meter.
- P2a's `unityrs-scene` graph and units (static3d decision 3 and sections 2.3 and 2.5), `unityrs-mesh`'s geometry, skins, blend shapes and OBJ/MTL, and `unityrs-gltf`'s editable scene, collect, profile and GLB writer (static3d section 2.4).
- The sealed batch attempt, candidate records, winners and run manifest in `docs/BATCH_SPEC.md` and `docs/plans/batch.md`, with P3a's opt-in version 2 fields already built (`docs/plans/rest.md` decision 3 and section 2.2).

**What it delivers.** `unityrs-anim` reads clips, finds them by PPtr, binds them to a prefab or scene, and samples generic, legacy and humanoid animation. The library and `unityrs gltf` can write the resulting editable scene as an animated GLB with local TRS and morph-weight channels. Batch version 2 gains `glb` and `obj` outputs keyed on P2a's prefab container entries. OBJ is a static export with its MTL and texture files.

**How it is checked.** UnityPy 1.25.2 is the reference for L2 reads and P1's PPtr behaviour. `PARITY.md` section 14.2's animation row already has its L2 status. Sampling, retargeting and GLB are tier X. L11 compares local poses in f64 before glTF conversion, with a reference to build and Editor samples to record. The pinned glTF-Validator and an independent GLB reader check the written channels and accessors. Batch replays the rules as written at L9.

**Citations.** UnityPy behaviour is cited by `PARITY.md` section and item. The pinned 1.25.2 distribution has generated animation classes and the GameObject component patches. Its `export` package, legacy patches and extractor dispatch have no animation exporter, curve sampler, controller evaluator or humanoid retargeter. [UnityPy 1.25.2 release](https://pypi.org/project/UnityPy/1.25.2/)

---

## 0. Decisions

| # | decision | why | reversibility |
|---|---|---|---|
| 1 | Keep UnityPy parity at the existing typetree, object and PPtr surfaces and the packed scalar helpers where they apply. Add animation semantics to tier X in `PARITY.md` section 24. | The pinned release exposes stored animation data. L2 and L11 answer different questions. | More read fixtures can be added without changing either tier. |
| 2 | Put the readers, binding and f64 sampler in `unityrs-anim` on core and scene. Extend P2a's editable glTF `Scene` with animations and add a sampling entry point. | This uses the graph split in static3d decision 3 and the model extension in its section 2.4. Sampling also serves callers without a writer. | The static entry points keep their contracts. New clip forms fit behind the same sampler. |
| 3 | For each supported binding, use populated runtime muscle-clip data when present, and use explicit curves for legacy clips, clips with no runtime data and supplemental bindings confirmed by fixtures. Record the chosen source. | A clip may carry more than one representation of a curve. The binding and its samples must have a single owner. | Versioned format rules can be extended. A malformed runtime binding gets a reason, with any fallback requiring a fixture. |
| 4 | Discover clips per Animation or Animator root, in stored order, through the attempt's existing Environment. Provide explicit clip selection and a bounded offline controller-state sampler (Q2). | The same clip can animate different roots or Avatars. The controller's PPtrs are sufficient for discovery. State evaluation is a separate operation. | Selection can expand through the profile. The direct-clip API needs no controller. |
| 5 | Bind explicit paths exactly and runtime paths by CRC32 within the component's subtree. Resolve ambiguous paths, hashes and properties as issues. | Path ids are file-local, names can repeat and CRC32 can collide. Choosing the first match can animate the wrong bone. | Additional disambiguation must come from serialized identity or a fixture-backed rule. |
| 6 | Verify humanoid rotations and the resulting local TRS in f64 against an independent implementation to build. Also require Editor recordings for supported Avatar layouts (Q1, Q3). | UnityPy supplies the fields. The independent reference checks the math and the Editor checks the meaning of the fields. | The rig and clip matrix can grow. Each verified row names the reference and coverage it earned. |
| 7 | Emit baked `LINEAR` channels, exact `STEP` channels where the whole property is discrete, and one `weights` channel per animated renderer attachment. Keep events and other sampled properties in the editable model (Q4, Q5). | A node/property pair occurs once per glTF animation. Weighted tangents and humanoid remapping are evaluated in Unity space before export. | The writer can gain other encodings behind explicit profile choices. |
| 8 | Keep P2a's last-frame morph policy. Take on `m_VariableBoneCountWeights` and paired `JOINTS_1`/`WEIGHTS_1` from static3d decision 9 once the Editor fixtures pin the layout (Q6). | Morph weights need the same endpoint scale as P2a's static node weights. Extra skin influences need a confirmed order and palette. | The top-four path remains available. Enabling eight influences can be an explicit profile choice. |
| 9 | Add the two batch output kinds as version 2 opt-ins, as static3d decision 11 says. Use each own container entry whose PPtr resolves to a GameObject as one prefab export unit. Update the manifest contract for the new kinds (Q7, Q8). | The job spec and manifest have separate compatibility rules. P2a's prefab unit is the boundary for winner selection and OBJ sidecar ownership. | Jobs without the new options keep their existing manifests and output selection. |
| 10 | Budget about 10–14k non-test lines (section 6) against the 4–6k in `docs/DESIGN.md` section 5. | Curve decoding and collection alone omit humanoid semantics, the independent reference, controller sampling, extra skin influences and batch publication. | Each PR has its own acceptance. The plan records any slice that still waits on evidence. |

---

## 1. Where it starts

### 1.1 Done

- **`unityrs-scene`.** `Unit::Prefab`, `Unit::Scene`, `unit_from_container` and `Unit::graph`. `Graph` retains stored names, parent and child order, file-scoped object refs, local TRS, and renderer components. Animation and Animator already appear as `Component::Other`, and `accessors` has `m_animation` and `m_animator`.
- **`unityrs-mesh`.** `read_mesh` supplies bone hashes, bind poses, and `BlendShapes` with channel name hashes, frame ranges and `fullWeights`. `export_mesh_renderer` writes through `ObjSink` using the existing OBJ/MTL rules. Its corrected-oracle checks stay the L6 reference for renderer output.
- **`unityrs-gltf`.** `collect` and `collect_with_mapper` build a metered `Scene` with accessors and node-instance weights. The collector maps graph nodes to scene nodes and can make separate identity-root skin attachments. `to_glb` lays out and streams the GLB. `Profile` is strict version 1 JSON and currently refuses a nonempty `clip_states`.
- **Batch.** P3a's version 2 parser, schemas and golden jobs cover `outputs.formats`, `outputs.image` and `unitycn_key`. The attempt has its own dependency view and the copy-out path already records hashes, sizes, skips and unit membership.

### 1.2 Not done

- No animation crate, sampled pose reference or checked-in Editor recordings for L11. The existing `tools/parity/gltf_check.py` reads static GLBs.
- No animation tables in the glTF model, no retained collector map for animation targets, and no decoded variable bone weights in `unityrs-mesh`.
- No prefab dispatch or 3D output kinds in `BATCH_SPEC.md`.

---

## 2. Crates, modules and APIs

### 2.1 Layout

| crate | new? | depends on | holds |
|---|---|---|---|
| `unityrs-anim` | new | core, scene | `clip` (editor, packed rotation, streamed, dense, constant), `binding`, `discover`, `controller`, `avatar`, `humanoid`, `sample` |
| `unityrs-scene` | exists | core | the existing graph, raw node names, component refs and units |
| `unityrs-mesh` | exists | core, meshdecode, scene | the variable-weight reader and source influence rows, once verified |
| `unityrs-gltf` | exists | core, scene, mesh, texture, imageio; adds anim | the editable animation model, target mapping in collect, profile selection, layout and writer |
| `unityrs-testkit` | exists | — | clip, controller, Avatar and prefab builders alongside the scene fixtures |
| `unityrs-cli`, `unityrs-batch` | exist | scene, gltf, mesh on the new paths | CLI sampling through the glTF profile, and sealed batch unit export |

The new crate keeps P2a's rules (static3d section 2.1): `forbid(unsafe_code)`, no-panic lints, Send + Sync and waits tests, and files under the repository size limit. License a port with its source and notices as `CLAUDE.md` requires. AssetStudio's MIT readers give format evidence for the clip words and transform binding widths. They do not give L11's humanoid reference. [AssetStudio AnimationClip reader](https://github.com/Perfare/AssetStudio/blob/master/AssetStudio/Classes/AnimationClip.cs)

### 2.2 `unityrs-anim`: clips and samples

**Read.** `read_clip` takes an `ObjectRef` and meter. It returns source timing, typed bindings, key or sample data, events and issues. Legacy and editor-form reads cover position, quaternion, Euler and scale curves, scalar float curves and object-reference (`m_PPtrCurves`) keys. Read `m_EditorCurves` and `m_EulerEditorCurves` where the tree has them, and distinguish them from the ordinary explicit runtime fields. Use key times, tangents, weighted modes and rotation order to evaluate continuous curves, and retain held keys and typed PPtrs. Decode `m_CompressedRotationCurves` with checked time, quaternion and slope unpacking for the fixture-backed layouts. Share the existing packed scalar decoder if the field semantics match, with any shared code below `anim` and `meshdecode`.

**Runtime.** In `m_MuscleClip.m_Clip`, the streamed section is a word stream with frame times, key counts, scalar slot indices and polynomial coefficients. Decode words by their bit patterns. Classify the sentinel frames separately, retain the previous key for each sparse slot, and evaluate its polynomial at time since that key. The dense section is frame-major with `m_BeginTime + frame / m_SampleRate`; validate the frame × curve product and sample rate before reading. The constant section holds one value per remaining slot. Map the concatenated streamed, dense and constant slot ranges through the binding table, including a binding whose components sit in different ranges. Read the legacy value-array binding where fixtures confirm it. Check discrete flags and indices into `pptrCurveMapping` before dereferencing a PPtr from the clip's own file. Unknown custom or compressed forms get a typed issue; malformed counts, indices or times cannot become arbitrary values.

**Sample.** `sample_clip` takes a bound clip, an explicit time schedule and a rest pose. It returns f64 local TRS and scalar tracks, held object-reference tracks, active bindings and inert event records. The default schedule uses the clip's finite positive rate (with a documented fallback for absent rates), includes both ends once and samples in source seconds. Clip output uses a zero-based clock. Fill unanimated components from the bound rest pose, preserve exact held transitions, and keep wrap metadata for callers. The exported interval is one pass of the clip. A malformed rate or impossible interval is a typed refusal. Time, frame, component and quaternion conventions are pinned between keys as well as at keys.

### 2.3 Discovery and binding

- From each graph node's stored components, `Animation` (111) contributes `m_Animation` then `m_Animations`. `Animator` (95) gives its `m_Avatar` and `m_Controller`; `AnimatorController` (91), or a supported `RuntimeAnimatorController` layout, supplies `m_AnimationClips`. Follow these pointers with the file that owns each field. For `AnimatorOverrideController` (221), substitute clips by resolved original identity, report broken overrides, and bound and cycle-check the controller chain. Deduplicate on (component root and identity, clip file identity and signed path id). Preserve the same clip bound at distinct roots. Null slots, wrong classes and missing dependencies have distinct outcomes.
- Explicit paths are slash-separated, case-sensitive stored names relative to the Animation or Animator node. The empty path targets that node. For runtime bindings index those same raw-byte paths by Unity's CRC32 (IEEE, tested against serialized path/hash pairs, including non-ASCII). Path hash zero is the empty-path binding where the binding kind says so. `Avatar.m_TOS` and controller TOS entries supply hash-to-string evidence in their own domains. Check a candidate path against the graph and retain all matches until the target is unambiguous. A filename, node display name or globally hashed bone basename cannot substitute for a relative path.
- `GenericBinding` keys on path, attribute, class (`typeID` or `classID`), `customType`, script identity and discrete/PPtr flags. For supported Transform records, attributes 1, 2, 3 and 4 are translation (3), quaternion rotation (4), scale (3) and Euler rotation (3). Build the scalar-slot table once with checked widths. Class, custom type and flags decide the meanings of other attributes, including humanoid and blend-shape properties. Resolve explicit `blendShape.<name>` curves and supported hashed equivalents against the target renderer's own mesh channels, with collision checks. Retain other float and PPtr properties as typed tracks. Filtered nodes and dropped renderer attachments are reported during glTF collection.
- For `clip_states`, use the Animator's layer and state identity, controller clip indices and parameter defaults plus typed overrides. Evaluate the selected state's supported motion and time mapping at a bounded schedule. Validate state/clip indices, nested state and tree depth, parameter types and blend weights. PR 5 covers direct states and fixture-backed 1D/2D or direct blend trees as specified in Q2. Discovery still returns each referenced clip on a controller whose state features the sampler cannot evaluate.

### 2.4 Avatar and humanoid

**Read.** Resolve the target Avatar and validate its `m_TOS`, skeleton, default/reference poses, human-to-skeleton mappings, axes and limits. Build the humanoid slot table from the clip's muscle indices and the versioned DOF mapping. Bind each supported human bone through the target Avatar's skeleton identity and map it to the graph. Missing required bones, duplicate mappings and unreconstructable optimized hierarchies give a typed issue.

**Retarget.** Sample the muscle values in f64. Map signed normalized muscle values through the correct per-axis limits around neutral, compose the Avatar's joint-axis and rest-orientation transforms in fixture-pinned order, and derive local bone rotations in Unity space. Use the target rig's rest TRS for untouched components and apply the supported body/hips transform using the target Avatar's scale and reference data. Generic curves drive properties outside the retargeted set. Report an overlapping writer unless a fixture establishes precedence for that binding. Isolate unsupported root motion, translation DOFs, additive poses, or IK according to Q3. A missing Avatar can leave independent generic tracks usable.

### 2.5 `unityrs-gltf`: model, collect and profile

**The model.** Extend the existing editable `Scene` with animations, each with local animation sampler and channel tables. A sampler points to scene accessors. A channel points to a scene node and one of `translation`, `rotation`, `scale` and `weights`. Animation samplers are distinct from texture samplers. The sampling entry point can also return the bound events, active curves, STEP tracks and non-glTF properties for callers to inspect or edit before writing. Static collection and empty-animation GLB goldens still run.

**Collect.** Keep the graph-node to glTF-node map and a renderer-ref to attachment map from the collector. TRS targets the original transform or joint node. Weights target the actual mesh instance, including its separate skin attachment. Keep stable source order under filtering, and apply P2a's mirror once: translation `(-x, y, z)`, quaternion `(x, -y, -z, w)`, and scale as read. Normalize valid quaternion samples and choose consistent signs across keys. A zero or non-finite quaternion, or a value that overflows f32, gets a typed refusal for the affected channel or clip. These structural checks apply in raw mode too.

**Morphs.** Each attachment gets one weight vector per time, in the emitted mesh's target order. Baseline slots come from P2a's node weights, or the mesh defaults when applicable. Animated slots are the Unity weight divided by the selected last frame's `fullWeight`, just as P2a's static collector does. Use the union of member key times and the bake schedule when combining curves. Keep P2a's endpoint approximation for multi-frame shapes and report its use. Verify two renderers sharing geometry with different default weights and different animation.

**Write.** Animation times are finite f32 SCALAR seconds, non-negative and strictly increasing after rounding, with min and max. Outputs are f32 VEC3 for translation/scale, VEC4 in XYZW order for rotation and SCALAR for weights, flattened in key-major order with `key_count × target_count` elements. `LINEAR` uses glTF's quaternion interpolation for rotation. For mixed held and continuous components, bake at the transition and the preceding representable output time when it fits, with a reported approximation (Q4). Deduplicate only coincident rounded time/value pairs with the same transition semantics. Report an unrepresentable discontinuity instead of silently overwriting a key. The layout includes the new accessors in its aligned BIN first-reach order. Validate dimensions, references, values and duplicate targets before writing. Omit empty animations. [Khronos glTF 2.0 specification, Animations](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#animations)

**The profile.** Keep version 1's strict parser. Missing or empty `clip_states` keeps the static export. Add a typed selection for discovered clips and selected controller states with parameters, a finite bounded sample-rate override and the skin-influence choice once available. Use the same parsed profile in the CLI and batch. An unsupported source condition is a report entry. A bad profile shape is a validation error.

### 2.6 Variable skin weights

Confirm the version-specific storage behind `m_VariableBoneCountWeights` with counts, indices and weights recorded from the Editor. Expose source rows in `unityrs-mesh` and validate their ranges and palette references. For the eight-influence path, keep the top eight in a fixed order (source order breaks weight ties), normalize across both sets, then pad and remap both sets through P2a's joint palette handling. Count truncation and repairs. Check skinned positions at a non-rest pose as well as the two accessor pairs. Keep the top-four path for fixtures without a supported variable layout. The Editor API provides decoded counts and weights, and section 8 asks for the serialized layout beside them. [Unity Mesh.GetAllBoneWeights](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Mesh.GetAllBoneWeights.html)

### 2.7 Batch units

**Spec.** Proposed version 2 fields are opt-in `outputs.glb` (with an inline versioned glTF profile) and `outputs.obj`, under one shared prefab key template when either is set. Allow a 3D-only job. Validate in phase 1 with version 2's duplicate, unknown, type and range rules, and bound the profile and template against the spec budget. Version 1 rejects the new fields with `unknown_field`. Texture format and image options keep their 2D meanings, and GLB embeds textures with its own profile and writer. Add schemas, parser/golden cases and CLI discovery in the implementation PR (Q7).

**Selection and keys.** Walk the input's own eligible SerializedFiles and container rows in the batch attempt's existing order and scope (`BATCH_SPEC.md` 4.3–4.4). One GameObject-targeting row is one prefab instance, even when two rows resolve to the same object. Resolve geometry, clips and textures only through the private view. Dependency objects may supply data for a selected row. Batch does not enumerate dependency rows or infer scene units from unmarked files. Expose container path and a safe form for the new template, with the existing name/path-id conventions and section 2 checks. The template expands to a stable logical unit key and output base. Preflight within the attempt checks duplicate/bad keys in plan order and keeps each renderer's OBJ with the sidecars it references, in collision-free renderer subdirectories. Write the GLB and the OBJ family under that unit. The existing writer supplies OBJ/MTL text. Check every emitted relative reference and key before committing that family.

**Candidates and winners.** Stream to the existing attempt sinks and make candidate records eligible only after successful copy-out and seal. Use the same deterministic plan order, artifact digests and E31/E32/E61 winners, reservation and withholding rules (`BATCH_SPEC.md` 4.5–4.6). The 3D unit key identifies the group regardless of which outputs survived attempt-time skips. Extend E32's output-backed unit keys with an explicit anchor record for each nonempty 3D group. The anchor participates in unit selection and has no published object or member-key claim. Use `prefer[unit_key]` with input and an MD5 over a canonical key/size/digest list for group matching and flip detection, with published member hashes remaining byte digests (Q7). Specify the digest framing and disambiguate the anchor from a plain output with the same key in the batch PR. Every produced member of the winning attempt travels together through winner selection. A failed sidecar withholds its referencing OBJ family before selection. Publication is still per key, so a sink failure can leave an incomplete family, with per-key outcomes and a resumable run.

**Manifest and failures.** Keep section 7's fenced head and sorted row files at the existing locations. Propose `manifest_version: 3` for jobs that request either 3D kind while the job's `spec_version` is 2. Section 7.3 requires a new manifest version for closed values. Define `outputs.kind` and `skips.output` for `glb` and `obj`, using the latter for OBJ, MTL and required texture sidecars, with a role and root provenance for the family and null image-only fields in the manifest. Record recoverable dropped animation or renderer content with typed reasons and preserve the output when usable. Preserve the original failure provenance through the classifier: object failures and outside-view required reads are skips (exit 3 unless expected), budget refusals follow the existing object/input split and exit 5, container or dependency failures keep the existing input rules (4 or 5), publication failures exit 4, and spool/allocator/internal failures retain their run-level treatment (5, 74, 70). Reuse `publish_incomplete`, resume, verification, stop and precedence as written. Add the closed reason and detail vocabulary and progress version changes where section 10 requires them. Leave archives' existing membership kinds for the first PR (Q8).

---

## 3. Parity targets

### 3.1 UnityPy's rows P2b uses

| row (`PARITY.md` section) | tier | level | P2b work |
|---|---|---|---|
| `AnimationClip`, `Avatar`, `Animator`, `AnimatorController` (14.2) | A | L2 | Keep the existing verified P0 dict result. Exercise the classes on fixture reads, including nested clip and Avatar fields. |
| `Animation`, `AnimatorOverrideController`, `RuntimeAnimatorController`, clip/binding/Avatar/controller records (2 and Appendix A) | A for the existing read surfaces | API, L2 | Generated class and field inventory; compare the checked dicts and, where exposed, object-mode shapes through the existing harness. |
| `GameObject.m_Animator`, `GameObject.m_Animation` (14.2, 19.2) and `PPtr.deref` (14.1) | A and recorded B cases | API, L1 (env) | Reuse the existing component accessors and `deref` behaviour. Discovery of a clip set gets its own X row. |
| `PackedBitVector.unpack_ints` and `unpack_floats` (15.3) | A as filed | L6 | Reuse the pinned unpacking checks for any shared scalar format. Animation timing and packed quaternion evaluation are X. |
| Renderer OBJ/MTL (14.2, 15.4, 19.1–19.2) | A and C as filed | L6 | Reuse P2a writers and corrected-oracle cases. Batch selection, relative keys and unit winners get a separate X row. |

The current plan adds no rows to `PARITY.md`. The implementation PRs add their section 24 rows with named references and tests. The existing L2 row's P2b marker can point there. Only the result actually pinned by a test moves to verified.

### 3.2 Beyond UnityPy: tier X

| proposed section 24 row | level | reference |
|---|---|---|
| clip decoding and sampling | L11 for local TRS | generated known curves, the independent f64 evaluator, and Editor clip recordings for serialized semantics |
| clip discovery, binding and controller states | L11 for resulting local TRS | synthetic graphs and reference identities, recorded path CRCs, and Editor state and blend samples |
| Avatar and humanoid local TRS | L11 | independent f64 retargeter and Editor pose recordings on two rigs (sections 4.2 and 8) |
| animation channels and morph weights | L6g writer checks plus L11 source samples | Khronos validator, independent accessor reconstruction from L11 and the expected morph vectors, including the last-frame approximation |
| variable skin weights | L6g | recorded Editor rows, hand-computed skinning, and the glTF validator |
| batch prefab export | L9 | `BATCH_SPEC.md` as amended; candidate bytes against standalone writers, OBJ/MTL text checked at L6 |

---

## 4. Verification

### 4.1 L2 fields and references (CI)

Use synthetic serialized classes with embedded trees and the existing oracle harness. Include explicit versus muscle forms, `Animation`'s default and list, controller and Avatar PPtrs across files, and class/version differences in binding fields. Record canonical UnityPy 1.25.2 values for Rust replay. Run the supported stripped cases with TPK at run time under the existing no-write guard. No TPK-derived tree is committed. Use repeated signed path ids across files and null, bad and outside-view pointers to pin discovery's scope and order.

### 4.2 L11 per-frame local TRS in f64 (CI)

**To build.** `tools/fixtures/anim_reference.py`, a standard-library Python f64 evaluator and retargeter with independent equations and fixture input tables. It takes independently recorded bindings, rest/axis data and test curves or muscle samples, and computes expected local TRS. It consumes no Rust-produced poses or binding tables. Test identity and single-axis poses analytically, and include a noncommuting two-axis case, asymmetric limits, default/rest pose, missing optional bones, and the same motion on different proportioned rigs. Include generic scale and a non-root translation so a rotation-only check cannot pass L11.

**To record.** The Editor project (section 8) saves local poses for authored generic clips and humanoid clips, and saves explicit muscle vectors and corresponding local poses by applying them with `HumanPoseHandler.SetHumanPose` to the target hierarchy. Record rest pose, rig mapping, limits, sample times, Unity version and settings alongside the arrays. Editor TRS is f32 captured and widened exactly, with its own tolerance. The independent f64 reference must agree with it within that envelope before its fixture is used to verify the supported retargeting semantics. [Unity HumanPoseHandler.SetHumanPose](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanPoseHandler.SetHumanPose.html)

**Rust replay.** Compare every expected node and frame, including first/last and off-key times, in Unity space before f32 GLB conversion. Compare translations and scales componentwise and rotations by normalized quaternion angular distance, treating q and −q identically. Pin initial same-input f64 gates of `1e-9` absolute plus `1e-9` relative for vector components and `1e-8` radians for rotation, using a stable relative-quaternion angle. Set separate Editor tolerances from the fixtures before accepting L11 (Q1). Track the worst error with rig, bone, time and property. There is no independent f64 implementation or Editor recording in the tree today. The L11 semantic rows stay planned until both references exist and the comparisons pass.

### 4.3 GLB, skin and profile (CI)

- Run the existing pinned glTF-Validator and allowlist on static and animated fixtures. Extend `tools/parity/gltf_check.py` to reconstruct times, channels and values from GLBs, test their interpolation between samples, and check both source-to-glTF handedness and the expected f32 rounding. Include one sample, all-held and mixed segments, timestamp collapse, invalid channels, animated joints, a filtered node and two morph instances over one mesh.
- Pin `JOINTS_0`/`WEIGHTS_0` and `JOINTS_1`/`WEIGHTS_1` in the eight-influence case, normalization across sets and palette remaps. Keep the existing OBJ, static GLB and orientation gates.
- Write the same profiles and units twice and under multiple caller threads. Compare bytes and deterministic report fields. Exercise CLI selection through the normal prefab and scene options.

### 4.4 Batch (CI)

Replay a prefab whose Animator, clip and textures cross bundle boundaries, plus an OBJ family with MTL and PNG, a GLB-only job and a version 1 and pre-3D version 2 job. Compare the actual candidate bytes with standalone output. Test repeated container rows, same-name CABs, missing clips, duplicate and invalid keys, `prefer`, prefix conflicts, withheld units, tight budgets, and interrupted publication with resume. Check exact manifest shapes, null fields and exit outcomes at L9. Compare stable records with 1 and N workers as in batch's determinism check. Testkit mutations and fuzz seeds cover streamed key counts, dense products, binding slots, controller cycles and invalid Avatar indices.

---

## 5. Performance and memory

- **Charges.** Read and sample on the caller's thread. Before allocating, charge counts, paths, binding maps, Avatar tables, per-track samples, collector maps and output accessors. Count work for each traversal, polynomial evaluation, pose composition, sort and emitted byte. Check sample count × tracks × arity, and frame/target products, before growth. Declared sizes and controller depth cannot produce unbounded work.
- **Peak.** Cache decoded clips by file/object identity within the current unit. Store bindings and samples by rig/component as well as clip identity. Release source and retargeting scratch as clips finish and retain only model data needed for editing and writing. Measure peak with geometry, encoded PNGs and retained animation together, and keep GLB BIN streaming.
- **Batch.** Charges for export belong to the sealed attempt; reads across its view retain dependency provenance. Use the existing export-work and spool budgets, stop checks and copy-out hash accounting. Measure time, charged peak and spool work for high curve counts, long clips, large target vectors and OBJ families. Run the attempt's squeeze, allocator refusal and zero-charge checks on the new path.

---

## 6. PRs

About 1–2.3k non-test lines each, including the new reference and recorder tools.

| # | PR | contents | ~non-test lines | depends on |
|---|---|---|---|---|
| 1 | plan | this document | docs | — |
| 2 | clip model, readers and sampler | the new crate; editor and compressed rotation curves, streamed/dense/constant slots, PPtr and float tracks, events, checked time schedules, L2 recordings and known-curve fixtures | 1.8–2.3k | 1 |
| 3 | PPtr discovery and binding | Animation, Animator, controller and overrides; graph path/TOS/CRC32 matching, typed bindings, rig-scoped identities and fixtures | 1.0–1.4k | 1; joins 2 at sampled tracks |
| 4 | glTF animation model and writer | editable tables, channel/sampler validation, accessors, layout, deterministic JSON/BIN, validator and independent reader extensions | 1.0–1.4k | 1 |
| 5 | controller state sampling | selected layers/states, typed parameters, direct motions and supported blend trees, controller fixtures and refusals | 1.1–1.6k | 2, 3; supported cases need section 8 |
| 6 | Avatar, retargeting and L11 | Avatar and muscle data, f64 local pose, independent evaluator, Editor pose recorder and replay, L11 rows | 1.6–2.3k | 2, 3; verification needs section 8 |
| 7 | animated collection, profile and CLI | collection maps, mirrored TRS, morph vectors, held and baked interpolation, clip-state profile, CLI, animation rows in `PARITY.md` | 1.3–1.8k | 2, 3, 4; 5 and 6 for state and humanoid completion |
| 8 | variable skin influences | verified mesh decode, two joint/weight sets, palette and normalization, profile choice, skin fixtures | 0.8–1.2k | 1, section 8 layout recording; joins 7 at the profile |
| 9 | batch prefab units | v2 spec and schema, sealed OBJ/GLB writers, keys and group anchors, winners, manifest and failure mapping, L9 goldens | 1.5–2.1k | 7 and P3a's T3 (merged); 8 for eight-influence coverage |

**Total: about 10.1–14.1k non-test lines.** PRs 2, 3 and 4 can start together after the plan. The Editor fixture work starts then too. PR 7 can bring up generic animation before the controller and humanoid checks land. PR 9 depends on the completed profile and animated collection.

---

## 7. Risks and open questions

1. **Mixed curve encodings.** Sparse streamed slots and dense or constant companions can drive one TRS property. Fixtures pin the slot table, sentinel and step interpretation, and the priority where editor and runtime curves coexist.
2. **Binding collisions and skin attachments.** Hash collisions, duplicate paths, profile filtering and separate renderer attachments can silently send motion to the wrong target. Pin component-scoped identity and the collector maps independently of displayed names.
3. **Editor coverage.** An independent numerical implementation can confirm a wrong layout if both read the same mistaken data. Compare independently extracted fields and record poses on two rigs at two Unity releases.
4. **Sealed attempt amplification.** A single prefab can hold several texture sets and long clips. Measure coexistence, bound every growth and keep dependency refusals distinct from a recoverable animation skip.

| open question | recommended answer |
|---|---|
| **Q1. L11 fixtures and tolerances.** Which independent artifact and what precision can the Editor justify? | Build the f64 Python evaluator and record Editor TRS in section 8. Freeze the f64 gates in 4.2 and record separate Editor tolerances per property after the first fixture run. An initial target is `1e-5` Unity units/scale components and `1e-4` radians. Explain any change with the measured discrepancy. |
| **Q2. Controller evaluation.** How much of the state machine and blend tree does the profile promise? | Explicit single-state evaluation at fixed caller-supplied parameters, with direct clips and fixture-backed 1D, 2D and direct tree modes. Read state and tree tables and enumerate referenced clips even when a mode is unsupported. Leave transition simulation, triggers, layer mixing, masks and behaviour execution outside this slice. |
| **Q3. Humanoid scope.** How should root motion and optimized rigs be handled? | Verify muscle rotations and supported body/hips TRS on a materialized skeleton first. Preserve generic root TRS when it is authored directly. Keep locomotion extraction, IK, additive poses and unsupported translation DOFs out of the verified scope, with specific reasons. An optimized rig whose bones cannot be mapped refuses humanoid sampling. |
| **Q4. Sampling fidelity.** How should a continuous property with held components travel through glTF? | Use exact STEP when the entire target is held, and explicit LINEAR baking with both sides of each representable jump otherwise. Report collapsed transitions or approximations. Measure mid-interval errors separately from L11. Consider CUBICSPLINE only when its derivatives preserve the source evaluation. |
| **Q5. Morph in-betweens and other properties.** How much can GLB carry? | Keep P2a's last-frame shapes and their full-weight scaling. Report in-between approximation. Keep non-morph float properties, PPtr curves and events available from the sampled model. Core glTF channels carry TRS and weights. |
| **Q6. Extra influences.** When can eight be enabled? | Opt in after the serialized layout and both sets agree with recorded Editor rows. Keep top four as the default, and report unsupported layouts on eight-influence requests. |
| **Q7. Batch keys and manifest version.** What anchors an OBJ-only prefab and how does the reader recognize new kinds? | Use the explicit shared unit key and canonical group digest of 2.7, with byte digests on each artifact. Use manifest 3 only for specs that opt into the new kinds. Document the new head/row and preference semantics in the batch PR, and pin compatibility with old manifests and readers. |
| **Q8. Archives and scene units.** Should the new kinds also appear in ZIPs or select whole scenes? | Start with loose prefab units. Keep scene animation on the library/CLI path. Revisit 3D archive membership with a rule preserving OBJ/MTL relative references and whole-family inclusion. |

---

## 8. The Editor fixture project (owner-built)

Extend the project planned in `docs/plans/static3d.md` section 8. No animation bundles or pose recordings for it are checked in today. Build small original assets in 2022.3 and 6000.x and commit only the project fixtures, built bundles and numeric records the repository allows.

- An explicit legacy clip and a generic runtime clip with streamed, dense and constant curves, confirming which forms were actually serialized. Include compressed rotations, weighted and held keys, an object-reference change, an event, and a non-ASCII relative path with its recorded binding hash.
- A legacy Animation component, an Animator with a controller, an override and known direct and blend-tree states. Record clip identities, parameter values, state times and local poses with the same sample schedule used by L11.
- Two humanoid Avatars with different proportions and asymmetric axes or limits, including neutral, single-muscle and combined rotations, body/hips motion and an optional bone missing. Record both clip-driven local TRS and local TRS from known muscle inputs on reset rigs. Record the mapping, reference poses and axes independently of the Rust decoder.
- A mesh with more than four influences and a separate mesh instance with animated blend shapes, including non-100 full weight and in-between frames. Save per-vertex counts, indices and weights beside the serialized fields, and record local poses and shape weights at the chosen times.

What waits on it: verified L11 for serialized playback and humanoid retargeting, the supported controller blend modes, and the eight-influence layout. Synthetic curve, binding, writer and batch fixtures can proceed first.
