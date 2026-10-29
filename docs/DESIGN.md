# unityrs design

**Status:** accepted plan, 2026-10-28. It went through two adversarial review rounds (Codex plus a Claude panel), and each finding was addressed or recorded.

This is the plan for a Rust Unity asset extractor that aims at full UnityPy parity. It is built to be reused across games.

## 1. Why

Python Unity pipelines built on UnityPy tend to fan out one task per bundle. That is slow in ways the per-file decoding time hides:
- every task re-parses the shared dependency bundles;
- uploads run serially, one file at a time;
- archives are rebuilt by re-downloading their own outputs;
- memory grows per worker.

On a live mobile-game corpus, measured:
- 11.5k bundles, Unity 6000.3, 1.3-1.8 GB per build;
- about half of all task time went to re-parsing dependencies;
- PNG encoding cost 30x the texture decode;
- UnityPy's own parsing was a small share.

unityrs is the alternative, built around three things:
- **Batching:** each bundle and dependency is parsed once, with work decoded and encoded in parallel under explicit memory budgets.
- **A typetree-driven core** that keeps working on new Unity versions.
- **The 3D export UnityPy does not have:** skins, Mecanim animation, humanoid retargeting, scenes, glTF.

## 2. Goals, non-goals, anti-goals

**Goals**
1. **Full parity with UnityPy**, pinned to a version and its native stack, per the tiers and inventory in section 3. Also the newer formats UnityPy's development head reads: SerializedFile v23-26 and bundle format 9.
2. **A batch engine:** one call takes N bundles plus dependencies, outputs and resource budgets, and parallelizes internally. It streams outputs to sinks: a directory, archives, S3-compatible object stores.
3. **3D export:** the prefab and scene graph, skins, blend shapes, Mecanim clips, Avatar/humanoid retargeting, and GLB.
4. **Any game:**
   - UnityPy's version-detection order, with opt-in reported overrides;
   - a TypeTreeProvider chain for stripped files;
   - hooks for altered headers, encryption and custom codecs;
   - every platform's texture formats and the old containers.
5. **A library, a CLI and Python bindings** (PyO3 abi3, GIL released, fork-safe).

**Non-goals (v1)**
- **No game-specific logic in the core.** Conventions arrive as job specs and profiles.
- **No byte-identical re-save of Unity's original file layout.** UnityPy does not do it either. See section 3's Writing table.
- **No FBX.** It needs Autodesk's proprietary SDK; GLB is the 3D format.
- **No native IL2CPP/Mono typetree generator.** A provider hook takes external dumps.
- **FMOD is never bundled.** A user-supplied libfmod is optional and outside the guarantee.

**Anti-goal:** silent output drift. A pipeline that hashes decoded pixels, or keys on PNG MD5s, must see identical results when it moves from UnityPy to unityrs. So the pixel output, output mode and PNG bytes are held to exact parity (Tier A, L5/L5b).

## 3. Parity

**The oracle** is a pinned stack, run from a versioned oracle image:

| package | version |
|---|---|
| UnityPy (with UnityPyBoost) | 1.25.2 |
| Pillow | 9.5.0 |
| texture2ddecoder | 1.0.5 |
| astc-encoder-py | 0.1.12 |
| etcpak | 0.9.14 |
| fmod_toolkit | 0.1.3 |
| liblz4 / liblzma | as installed |

A second **HEAD oracle** is UnityPy git `417d595`. Re-pinning either one is a documented step.

**Reading**
- **Tier A: bit-exact** wherever UnityPy is right.
  - Typetree dicts equal `parse_as_dict()` with Boost semantics: lists for arrays, tuples for pairs, bytes for TypelessData, surrogate-escaped strings, float32 widened exactly.
  - Object mode builds each class as UnityPyBoost's `parse_class` does: a node with a field its class lacks becomes an `UnknownObject` when the class's `__slots__`, read through the MRO, is a non-empty tuple or its instances have no `__dict__`, and the class with the field as an attribute otherwise. `PARITY.md` section 9.2.1 lists every class of `UnityPy.classes` by that rule, from what `parse_class` does with each. The pure-Python path's different rule is kept where UnityPy calls it (`PARITY.md` section 9.2).
  - `obj.container` and dependency resolution match, the case of file names included, with one exception in Tier B. `PPtr.deref` looks a record up by its last component lower-cased. `load_file`'s case-insensitive fallback resolves almost nothing: `find_sensitive_path` matches a two-component path, and the name `load_file` hands it is joined to `Environment.path`. `find_file`'s suffix match on lower-cased basenames finds a file whose basename on disk is all lower case, whatever the record's case, and never one whose basename holds an upper-case letter. unityrs adds no case-insensitive lookup of its own, which would load files UnityPy does not (`PARITY.md` section 19.2).
  - Decoded pixels **and output mode** match: BCn per Pillow `BcnDecode.c`; ETC/EAC/ATC/PVRTC/crunch per texture2ddecoder C++; ASTC via ARM astcenc.
  - OBJ/MTL text matches.
  - An image file the extractor saves under a container's extension is byte-equal with what Pillow 9.5.0's writer for that extension saves at its defaults to the same path, for every writer Pillow registers and every mode, size and file name it saves; a PDF's two dates, which hold the time of the save, aside (`PARITY.md` section 15.8).
  - `save_typetree(parse())` returns the raw bytes.
  - Every patch to vendored decoders is labelled either Tier A (keeps the reference behaviour) or Tier C.
  - An observable quirk that callers can depend on and that is not wrong data (a file name, a dict key, a `str()` form, a docstring the code contradicts) is Tier A: unityrs keeps UnityPy's behaviour.
- **Tier B: UnityPy raises on a valid request.** We produce correct output, checked against an independent reference, and `PARITY.md` names a pinned divergence test for each:
  - 21 TextureFormats: BC6H, the half and float formats but ARGBFloat, the 8 signed formats, RG16/RG32, R16, RGBA64, RGB9e5, YUY2. `PARITY.md` section 17.1 names each one's reference: a closed form (the half and float, integer, shared-exponent and BT.601 rules), or for BC6H the decoding of the Khronos Data Format Specification 1.3, section 20.2, then the half and float rule. Neither BC6H decoder in the oracle image follows that section (texture2ddecoder 1.0.5 unquantizes the wrong endpoint value to 0xFFFF; Pillow 9.5.0 interpolates without rounding), so neither is the reference;
  - a gzip-compressed WebFile, which UnityPy detects and then fails to parse;
  - a streamed Texture2DArray, whose slices all raise;
  - a Texture2DArray in one of the seven 8-bit integer formats that UnityPy maps to a 16-bit texture format that raises;
  - `GameObject.m_Components` on an empty list or the old pair form, and the component accessors (`m_Transform` and the others), which raise because they read it;
  - `Texture2D.image` of an empty 0×0 texture;
  - an empty AudioClip, whose empty resource path UnityPy fetches;
  - a ZIP opened from a path, which UnityPy loads only from bytes or a buffered stream;
  - a `bytearray` passed with no name, which UnityPy's own `FileSourceType` lists and which raises computing its default key (the reference is the same bytes passed as `bytes`);
  - a record that `PPtr.deref` resolves to a file on disk whose basename holds an upper-case letter: UnityPy lower-cases the record and so never finds the file, even when the record names it in its own case (the reference is the file whose basename equals the record's last component);
  - a name read through a non-local fsspec filesystem, which `load_file` tests and walks on the local disk instead, where that raises (the local disk cannot list `Environment.path`, as with the default `''`);
  - `File.get_objects` and `File.get_filtered_objects` of a file holding a bundle or WebFile, which they iterate as `objects`, an attribute neither has;
  - a class node whose type names an attribute of `UnityPy.classes` that is not a class, where UnityPyBoost raises (`List`, `Tuple`): unityrs builds it as a type the module lacks (`PARITY.md` section 19.2);
  - `set_image` from an image whose mode Pillow cannot pack for the target (an indexed or grey PNG, or an RGB image into Alpha8): unityrs converts it with Pillow's `convert("RGBA")` first (`PARITY.md` section 18.1).
- **Tier C: UnityPy is wrong.** It returns wrong data that an independent reference shows is wrong, or one of its exporters or helpers crashes. We produce correct output, and `PARITY.md` lists each case with a pinned divergence test and, for a crash, what correct output means:
  - RGBAHalf returns garbage;
  - RGB48 decodes as packed 5-6-5;
  - ASTC_HDR is wrong in 1.25.2;
  - ETC2_RGBA1 keeps a transparent punch-through pixel's colour, where the Khronos Data Format Specification makes it (0, 0, 0, 0);
  - Texture2DArray decodes R8G8B8_UInt as RGB48;
  - the MeshHelper.py:504 UV offset;
  - UNorm/SNorm values are not normalised;
  - mesh endianness is never read from the file;
  - the compressed-mesh 4th bone weight;
  - the ReferencedObject array path does not skip None;
  - the pure-Python object-mode reader raises `AttributeError` on an extra field of a class without a `__dict__`;
  - UnityPyBoost crashes the process, or returns some other value (a function, a `str`, an `int`, `None`), on a class node whose type names something `getattr` finds on `UnityPy.classes` that is not one of its classes: an attribute of the module (`math`, `__name__`, `attrs_define`) or of the module's type, which `dir` leaves out (`__class__`, `__repr__`, `__hash__`): unityrs builds it as a type the module lacks;
  - dump_blob's common-string offset 0;
  - two bundles holding a CAB of one name share one registry entry;
  - a nameless input's `files` key, which UnityPy takes from Python's hash, randomised per process for bytes and an address for a stream, so no output can equal it: unityrs keys it by a stable rule (`PARITY.md` section 19.2);
  - a name read through a non-local fsspec filesystem where `load_file` returns `None` with no error, because the local disk can list `Environment.path` but lacks the name (the reference is the same as Tier B's: the file loaded as a stream opened from the filesystem);
  - `File.get_filtered_objects` with no types, or on a SerializedFile, and `File.get_assets` on a SerializedFile yield nothing, where each means to yield the value its generator `return`s or the file's objects (`PARITY.md` section 19.2);
  - `load_dependencies` loads dependencies as main files;
  - `ContainerHelper.path_dict` ignores an entry's file id;
  - `ObjectReader.peek_name` caches a peek node by the root's name, type and version alone, so a later tree of another layout reads the wrong field;
  - the Sprite cache key ignores the FileID;
  - a sprite triangle whose destination corners are collinear makes `copy_triangle`'s affine matrix singular, and `LinAlgError` escapes `Sprite.image`: the triangle copies nothing;
  - MeshRendererExporter fails on Unity 6;
  - ShaderConverter fails from 2021.3.10 and on 6000.3: it reads a pass's programs only from `m_SubPrograms`, while from 2021.3.10, 2022.1.13 and 2022.2 they can be in `m_PlayerSubPrograms`, as all of the corpus game's are (correct output reads those, with `m_ParameterBlobIndices`), and it cannot read 6000.3's sub-program layout;
  - 3.x submeshes are all read as strips;
  - a Mesh before 3.5 and a sprite's render data before 5.6 fail `MeshHandler.process`'s assert;
  - Shader export before 5.5 raises `TypeError` on `bytes(m_Script)`, and from 5.5 on a Shader with an empty `compressedBlob` on `bytes(None)`;
  - `ShaderSubProgram` decodes GL, GLES, console and Metal code as strict UTF-8, so code that is not UTF-8 raises: unityrs writes the comment UnityPy writes for a program type it cannot disassemble;
  - `export_mesh_renderer` gives each material to a submesh `firstSubMesh` too early, and writes no texture;
  - the extractor's crashes: GameObjects (each reachable object written once, its name ending in its path id where UnityPy's would end in `_None`), MonoBehaviours, sprites without a container, meshes without vertices, images Pillow cannot save under the container's extension, or saves as an ICO holding no image (a side below 16), and the no-container mode's missing `dst`.
- **Unsupported until a sample and a reference exist.** unityrs refuses these with a named `Unsupported` error:
  - LZHAM, where UnityPy raises too;
  - a UnityArchive parsed as a bundle, where UnityPy raises too. Loading one is Tier A: UnityPy's detector does not know the signature and loads the file as a raw ResourceFile, and so does unityrs;
  - ARGBFloat, where UnityPy raises too: its name says A, R, G, B, UnityPy's table reads it as B, G, R, A, and no sample or specification of the layout decides;
  - YUY2 of odd width: no sample shows how the last pixel is stored;
  - `set_image` from an image mode Pillow cannot convert to RGBA (BGR;15, BGR;16, BGR;24, La), where UnityPy raises too;
  - ETC_RGB4_3DS and ETC_RGBA8_3DS. UnityPy decodes them as plain ETC1, but no 3DS sample or reference decoder shows what they should decode to, so the refusal is a documented divergence, not parity;
  - a Texture2DArray read from a Switch or XBOX360 file. UnityPy decodes its slices with no platform handling, and the class carries no platform blob to deswizzle with;
  - `set_image` with mips on a Switch-swizzled texture. UnityPy swizzles the top level and writes the mips linearly; no sample shows how a mip chain is laid out;
  - a Switch-swizzled texture in a format the Switch texel-block table lacks (the matrix's `sw —`), read, or written by `set_image` (the seven ETC targets), where UnityPy raises `KeyError`: no Switch sample shows the texel block such a format is swizzled with;
  - `set_image` on XBOX360 to a format UnityPy's reader byte-swaps. UnityPy writes without swapping, so the texture reads back wrong.
  - a 5.3-5.4 Shader whose sub-program blob does not parse big-endian, the byte order UnityPy reads it in. UnityPy raises too (`struct.error`, or `TypeError` once the blob parses), and no sample shows the byte order Unity writes.
- **Tolerance:** the Sprite affine-triangle path, and pure-Rust audio against FMOD. Each has a stated metric and threshold.
- **HEAD tier:** SF v23-26 and bundle format 9, equal to HEAD's dicts on Unity 6000.4/6000.5 fixtures; and the `lz4hc` packer, which 1.25.2 refuses and HEAD writes (Writing, below).

**Writing**

Writing has tiers of its own. Where UnityPy writes, unityrs follows the rule in the table below (Tier A for writing). Where UnityPy fails to write (it raises on a valid request, or writes data its own reader misreads), that is **Tier C (writing)**: unityrs writes, and the row states the acceptance rule the written data must meet.

| what | rule |
|---|---|
| Object re-save | byte-equal |
| File / bundle save | semantic equality plus idempotence. An optional UnityPy-layout mode is byte-equal to UnityPy's `save(packer='none')` |
| Compressed packers | equal after decompression; byte-equal LZ4HC only with liblz4 HC level 9 (pinned) |
| `lz4hc` packer (HEAD tier) | HEAD's `save_fs(195, 3)`: header flags `0xC3`, every block flagged 3, under the compressed-packer rule above; 1.25.2 writes the same for the tuple packer `(195, 3)`, which the rule is pinned against |
| `File.get_writeable_cab` | UnityPy's naming rule |
| `set_image` | equal after re-decode: Pillow 9.5 BICUBIC mips, NEAREST padding, pinned etcpak/astcenc |

| Tier C (writing) | acceptance |
|---|---|
| Pre-v10 and v11 typetree writing (UnityPy raises), and a save below format 13 with `SERIALIZED_FILE_PARSE_TYPETREE` false (UnityPy drops the trees the format requires, and cannot read the file back) | the saved file reloads with the same objects |
| An object holding a ReferencedObject of empty class (UnityPy raises `KeyError`) | byte-equal to its original bytes |
| `set_image` to BC7 (UnityPy raises with the pinned etcpak) | byte-equal to what etcpak 0.9.14's `compress_bc7` writes with a default `BC7CompressBlockParams()` on the data UnityPy hands it (flipped, padded to whole 4×4 blocks, RGBA); on a Switch-swizzled texture, padded to the swizzle's size instead, and the compressed bytes swizzled |
| `set_image` to ETC_RGB4Crunched (UnityPy tags plain ETC1 as crunched) | written as ETC_RGB4, equal to `set_image` to ETC_RGB4 |
| `SerializedFile.get_writeable_cab` from a bundle or WebFile (UnityPy raises `TypeError` after making the parent's writer, and registers no external) | returns the writer, and registers the external once with the fields UnityPy means to write: `temp_empty` empty, a 16-byte GUID of the `uuid1().urn[-16:]` shape, type 0, path `archive:/{name}/{cab}`; the saved file reloads with that external, and the saved parent with the writer's entry |
| `WebFile.save` of a writer entry (UnityPy raises) | the saved WebFile reloads with the writer's bytes |
| `BundleFile.save` with `original` and no blocks (UnityPy raises); `save_fs` encryption flags (UnityPy's output reads back as encrypted) | the saved bundle reloads, unencrypted, with the same entries |

**Inventory.** `PARITY.md` (P0) maps every public UnityPy API and behaviour to a tier, a verification level and a test. It includes a per-TextureFormat matrix: decoder, output mode, conversion, platform/version gates, and UnityPy's own result. "Full parity" means the inventory is green. Audio lists exact formats: Ogg/RIFF/m4a passthrough; FSB5 PCM, Vorbis, ADPCM and FADPCM in pure Rust; MPEG and Opus optional; anything else reported unsupported.

**3D (beyond UnityPy).** Golden comparisons against a Python reference exporter: GLB JSON structure, accessor data within tolerances, and decoded-image equality. Per-frame local TRS is compared in f64 (L11). glTF-Validator runs, plus a normal-orientation check.

## 4. Architecture

A Cargo workspace. Licences are declared per crate.

| crate | contents |
|---|---|
| `unityrs-core` | **Containers:** UnityFS 6-9 (ArchiveFlags vs ArchiveFlagsOld by engine version, blocks-info at the end, padding-at-start, alignment), UnityWeb/UnityRaw, WebFile (gzip/brotli), `.splitN`, zip/apk, folders, ImportHelper detection and name rules. **Compression:** `liblzma` static (benchmarked against `lzma-rust2` in P0: `docs/spikes/lzma.md`), `lz4_flex` (liblz4 HC for writing), `brotli`, `flate2`, `zstd` behind a feature. **SerializedFile** v1-26 in both endians (big-endian best-effort until fixtures exist). **Typetrees** in old, blob and mhtt formats, read into our own dynamic `Value`, which holds invalid UTF-8, pairs versus sequences, and TypelessData. Typed views sit on top for hot classes. **TypeTreeProvider chain:** embedded, then TPK, then a user provider (a Rust trait or a Python callable; loaders for TypeTreeGeneratorAPI and Il2CppDumper-style dumps), plus `nodes=` overrides. With no schema, an object is still readable as raw bytes. **Environment:** a case-insensitive CAB registry built from bundle directories. The directory sits in the small blocks-info, so it is readable without decompressing the data. A dependency is loaded only when a PPtr crosses into it. Also `.resS`/`.resource` lookup and container paths. **Version detection** uses two chains in UnityPy's order: at bundle level, flags and alignment come from the engine string; at SerializedFile level, its own version, then the bundle's if empty or `0.0.0`, then an override. **Hooks:** a stream transform before sniffing, a block decryptor (UnityCN built in), and a codec registry, each with a Python adapter. **Write-back** per section 3. |
| `unityrs-tpk` | The class database for stripped typetrees. Optional, and loaded from a path at runtime. It is not embedded until its provenance is settled (it comes from AssetRipper's TypeTreeDumps, which has no licence file). |
| `unityrs-meshdecode` | Vertex-data unpack for every VertexFormat table, CompressedMesh/PackedBitVector, index formats and topologies, `get_triangles`. In f64 for parity. Sprites and meshes share it. |
| `unityrs-texture` | TextureFormats per the P0 matrix: Pillow-semantics BCn; vendored texture2ddecoder for ETC/EAC/ATC/PVRTC/crunch; ARM astcenc for ASTC LDR/HDR; Switch deswizzle; the Xbox360 swap; Texture2DArray. **Sprites:** atlas render data, alpha merge, banker's-rounding crop, rotation and flips, tight-mesh masks (a port of Pillow's `polygon_generic`), and the affine path. Encoders for `set_image` sit behind a feature. Decoders run under `catch_unwind`. |
| `unityrs-imageio` | A mode-aware PNG encoder byte-identical to Pillow 9.5.0: colour types 0/2/4/6; filters none/up/sub/paeth by minimum abs-sum; `libz-sys` static zlib, with CI asserting no zlib-ng; IDAT chunks of max(65536, 4*w). Also lossy WebP via `libwebp-sys`, a visible-pixel hash, and a fast PNG mode. In P3, the other writers `PARITY.md` section 15.8 makes Tier A, each byte-identical to Pillow 9.5.0's. |
| `unityrs-mesh` | MeshHelper parity, skins, blend shapes, Unity 6 LOD info, OBJ/MTL. |
| `unityrs-anim` | AnimationClip (streamed, dense and constant, generic bindings, legacy), Avatar/TOS, humanoid muscles, AnimatorController state machines and blend trees. |
| `unityrs-gltf` | **Two-phase:** `sample_*()` returns an editable `AnimatedScene` (nodes, per-frame TRS, events, active curves, STEP channels), and `to_glb(scene, profile)` writes it. Callers can edit the scene in between. A **material-mapping trait** with unlit+extras and core PBR (normal, alpha, `KHR_texture_transform`) implementations. Morph targets. The **profile schema** covers root naming, layer skips, drawable/additive shader rules, inactive-root handling, material-property allowlists, clip-state lists with parameter overrides, scene roots and exclusions, built-in resource handling, extras naming and export metadata. |
| `unityrs-export` | Font, TextAsset, MonoBehaviour JSON (a Python-compatible JSON writer), Shader text (fixed for 6000.x), VideoClip, AudioClip per the inventory, and an NGUI atlas cutter. Its sheet scope is either `own_files_by_name` or `follow_pptr`. Collisions are handled per atlas, and the atlas winner also wins its sheet texture. |
| `unityrs-batch` | **The engine.** A JSON job spec carries the inputs (paths or URLs with headers and retry policy), outputs, `run_id`, and resource budgets: `threads`, `download_concurrency`, `max_inflight_uncompressed_bytes`, `max_decoded_bytes`, `dep_cache_bytes`, spool dir and size. These are required; autodetection is only a capped fallback. Each bundle and dependency is parsed once (Arc'd, LRU by bytes). A rayon pool runs per call. **Sinks:** a directory, zip (spooled, stored members), and S3-compatible stores via `object_store`, with static or session credentials from the environment only and **no** IMDS fallback. Content-Type is set on every PUT, files go up single-part, and archives go up multipart. **Failure model:** object-level skips are recorded, but bundle-level failures (download, container, parse) make the run `incomplete`. The NDJSON progress and the run manifest (written **last**, the previous one deleted first, stamped with `run_id`) carry per-output status. Writes are idempotent, and a re-run skips outputs already present with the right MD5. **Selection hooks:** `prefer` winners for same-named outputs; emit only changed pixels against a previous manifest. |
| `unityrs-cli`, `unityrs-py` | The CLI, and PyO3 abi3 wheels that mirror the UnityPy API per the inventory. |

**Base layer: rabex** (MIT OR Apache-2.0). The P0 spike decides the route by these rules:

| route | when |
|---|---|
| **Use upstream, pinned** | L1-L3 pass 100% on the corpora; the Environment, cross-file PPtr, `.resS` and container-path probes pass or layer on top cleanly; it supports a raw-bytes `Value` (or offers an extension point); throughput is at least UnityPyBoost's; no fuzz panics in one hour, or upstream accepts the fixes. |
| **Vendor a fork** | L1 is sound, but we need invasive changes (the `Value` representation, the save layout) that upstream has not accepted (we offer them upstream and revisit if they merge). |
| **Port UnityPy's core** | The failures are structural, or more than half of rabex's core would be rewritten anyway. |

The spike records LOC kept versus rewritten, whether upstream takes patches, objects/s and MB/s against UnityPyBoost, peak memory, Send+Sync under rayon, the format matrix (SF 5-26, UnityFS 6-9, flags rules, big-endian), save against UnityPy, and a TPK node-list diff. `unity-rs-core` (an MIT AssetStudio port) is a reference and a second oracle only.

**Licensing**
- Crates carry accurate SPDX expressions, for example `(MIT OR Apache-2.0) AND MIT-CMU` for Pillow ports.
- `cargo-deny` checks crates; `cargo-about` with overrides scans vendored C (libwebp BSD-3 plus PATENTS, zlib, xz).
- `THIRD_PARTY_LICENSES` carries every notice: UnityPy and UnityPyBoost (MIT), AssetStudio (MIT), texture2ddecoder, astcenc (Apache-2.0), Pillow, libwebp.
- The provenance of bundled data is recorded.
- GPL/AGPL code is never read into the repo.
- **Game data is never committed.** Corpora are private manifests of URLs plus sha256. Only synthetic and Unity-Editor-built fixtures are checked in.

## 5. Phases

| phase | delivers | rough size |
|---|---|---|
| **P0 Foundations** | CI (fmt, clippy, test, deny, about; L0-L5b on checked-in fixtures). The oracle image and a long-lived dev container. Corpus manifests with a nightly full run. `PARITY.md`. The rabex spike plus decision. The LZMA benchmark. A minimal Unity 6000.4/6000.5 fixture set for the HEAD tier. **L2 in P0** works through a tagged canonical JSON dump from the Rust CLI, compared by a Python normaliser in the oracle image against `parse_as_dict()`; no binding needed yet (format: `docs/CANONICAL_DUMP.md`; oracle and comparer: `tools/parity/`). | 2-3k LOC |
| **P1 Fast batch extraction (2D)** | The core for real-world shapes, `meshdecode`, the most common formats (BC7/DXT1/DXT5/RGBA32/RGB24) with exact modes, sprites, the NGUI cutter, PNG/WebP, `batch` with every sink and the failure model, the CLI. The first production cutover runs on this. | 10-13k |
| **P2a Static 3D** | `mesh`, the scene graph, `gltf` (two-phase, material trait, profile), scenes, built-in meshes. | 4-5k |
| **P2b Animation** | `anim`, humanoid retargeting (L11 in f64), clips by PPtr, batch unit export. | 4-6k |
| **P3 The rest of parity** | The remaining Tier-A formats (ETC/EAC/ATC/PVRTC/crunch, ASTC), Tier B/C formats, audio, the Shader/Font/TextAsset/VideoClip/MonoBehaviour/OBJ exporters, UnityWeb/Raw/WebFile/split/zip, UnityCN, the TPK fallback, write-back plus `set_image`, the full Python API, and the HEAD tier. | about 20k |
| **P4 Multi-game hardening** | A coverage matrix with per-row pass criteria: UnityPy's samples; Editor-built fixtures (5.x to 6000.5; typetrees on and off; every packer; multi-block; blocks-info at the end; split; WebGL; Android/iOS/Switch texture sets); stripped MonoBehaviours through the provider hook; UnityCN samples; big-endian if a source exists. Also `binary2text` as an oracle, fuzzing, docs, releases. | ongoing |

## 6. Verification

| level | checks |
|---|---|
| L0 | detection |
| L1 | container metadata |
| L2 | typetree dicts |
| L2b | TPK nodes |
| L3 | object re-save |
| L4 | file save |
| L5 | pixels, mode, visible hash |
| L5b | image file bytes against Pillow 9.5.0's writers: PNG first, then every writer `PARITY.md` section 15.8 makes Tier A |
| L6 | mesh numbers (with an expected-divergence mask) and OBJ text |
| L7 | text exports |
| L8 | audio |
| L9 | extractor tree |
| L10 | end-to-end consumer shadow runs |
| L11 | 3D per-frame TRS in f64 |

CI runs L0-L5b on checked-in fixtures. The nightly runs the full private corpora. Every run records the oracle versions. Also: glTF-Validator, fuzz targets on every parser, and benchmarks.

## 7. Risks

- **Output drift** (pixels, mode, PNG bytes): the L5/L5b gates and consumer shadow runs.
- **Small-maintainer dependencies** (rabex, texture2ddecoder, fsbex, lewton, gltf): vendoring at crate boundaries.
- **Format drift:** a typetree-driven core, the HEAD tier, and Editor fixtures.
- **A thin public corpus** (UnityPy ships 4 Android SF v17 bundles): Editor-built fixtures in P0 (minimal) and P4.
- **TPK provenance:** a separate crate, loaded at runtime.
- **Humanoid retargeting has no permissive reference** other than a Python implementation; L11 must pass before any tuning.
