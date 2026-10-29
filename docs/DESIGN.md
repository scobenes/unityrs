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
| numpy | 2.4.6 (not a UnityPy dependency, but UnityPy uses it when present: sprite rendering differs without it) |
| liblz4 / liblzma | as installed |

`tools/parity/requirements-oracle.txt` pins the whole stack, dependencies included, as hash-checked wheels; the oracle dumper refuses to run when any of them, or the UnityPyBoost binary, differs. A second **HEAD oracle** is UnityPy git `417d595`. Re-pinning either one is a documented step.

**Reading**
- **Tier A: bit-exact** wherever UnityPy is right.
  - Typetree dicts equal `parse_as_dict()` with Boost semantics: lists for arrays, tuples for pairs, bytes for TypelessData, surrogate-escaped strings, float32 widened exactly.
  - `obj.container` and dependency resolution match.
  - Decoded pixels **and output mode** match: BCn per Pillow `BcnDecode.c`; ETC/EAC/ATC/PVRTC/crunch per texture2ddecoder C++; ASTC via ARM astcenc.
  - OBJ/MTL text matches.
  - `save_typetree(parse())` returns the raw bytes.
  - Every patch to vendored decoders is labelled either Tier A (keeps the reference behaviour) or Tier C.
- **Tier B: UnityPy raises.** 22 TextureFormats: BC6H, half and float formats, the 8 signed formats, RG16/RG32, R16, RGBA64, RGB9e5, YUY2. We produce correct output, checked against an independent reference. UnityArchive and LZHAM stay **unsupported (they raise)** until a sample and a reference exist.
- **Tier C: UnityPy is wrong.** We produce correct output, and `PARITY.md` lists each case with a pinned divergence test:
  - RGBAHalf returns garbage;
  - ASTC_HDR is wrong in 1.25.2;
  - the MeshHelper.py:504 UV offset;
  - UNorm/SNorm values are not normalised;
  - mesh endianness is never read from the file;
  - the compressed-mesh 4th bone weight;
  - ETC_RGB4_3DS and ETC_RGBA8_3DS;
  - the ReferencedObject array path does not skip None;
  - dump_blob's common-string offset 0;
  - MeshRendererExporter fails on Unity 6;
  - ShaderConverter fails on 6000.3;
  - the Sprite cache key ignores the FileID.
- **Tolerance:** the Sprite affine-triangle path, and pure-Rust audio against FMOD. Each has a stated metric and threshold.
- **HEAD tier:** SF v23-26 and bundle format 9, equal to HEAD's dicts on Unity 6000.4/6000.5 fixtures.

**Writing**

| what | rule |
|---|---|
| Object re-save | byte-equal |
| File / bundle save | semantic equality plus idempotence. An optional UnityPy-layout mode is byte-equal to UnityPy's `save(packer='none')` |
| Compressed packers | equal after decompression; byte-equal LZ4HC only with liblz4 HC level 9 (pinned) |
| `get_writeable_cab` | UnityPy's naming and GUID rule |
| `set_image` | equal after re-decode: Pillow 9.5 BICUBIC mips, NEAREST padding, pinned etcpak/astcenc |
| Pre-v10 typetree writing | broken in UnityPy, so Tier C |

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
| `unityrs-imageio` | A mode-aware PNG encoder byte-identical to Pillow 9.5.0: colour types 0/2/4/6; filters none/up/sub/paeth by minimum abs-sum; `libz-sys` static zlib, with CI asserting no zlib-ng; IDAT chunks of max(65536, 4*w). Also lossy WebP via `libwebp-sys`, a visible-pixel hash, and a fast PNG mode. |
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
| L5b | PNG bytes against Pillow 9.5.0 |
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
