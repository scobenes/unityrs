# P3 plan: the rest of parity

**Status:** accepted, 2026-11-13, with the decisions of section 0, the three phases they cut P3 into (P3a, P3b and P3c, decision 2) and the PRs of section 3, in five waves. PR P3-0 is this document with the `PARITY.md` and `docs/DESIGN.md` changes it calls for: each P3 row of `PARITY.md` tagged with its phase (section 1.3), the P1 rows only a binding can test moved to P3c (decision 14), `docs/DESIGN.md` section 5's phase table split (decision 2), and the crate-versus-port rule written into its section 4 (decision 1). Wave 1 (T1, T2a, T2b, T3, T4a, T4b, C1 and C3) is the consumer path and starts at once. Three parts wait on the project owner, as additions to the one Editor fixture project `docs/plans/static3d.md` section 8 describes: crunch-compressed textures (decision 5), AudioClips (decision 8) and Unity 6000.4/6000.5 files for the HEAD tier (decision 15). No PR waits on them; the verification of the rows they cover does (section 6).

**What it builds on.**
- The core as merged in P1 (`docs/plans/core.md`): the containers and codecs P1 reads, the Environment, the memory meter (`MeteredVec`, `DecodeBuf`, `contain`), object re-save (L3, in `walk/write`), the TPK provider (`unityrs-tpk`, a port of UnityPy's `helpers/Tpk.py`, loaded from a path), and hooks P1 left unused: `codec::BlockDecryptor` for UnityCN, the `WebFile` type (whose `open` refuses with `Unsupported::WebFile`), and the detection of zip, gzip and brotli WebFiles and of UnityWeb/UnityRaw.
- The named refusals P3 lifts: `Unsupported::{WebRawLayout, WebFile, Encrypted, Zip, Split}`.
- P1b's crates: `unityrs-texture` (the five P1b formats, the selection and `plan`/`decode`, `MAX_PIXELS`, `contain`), `unityrs-imageio` (the Pillow-exact PNG writer), `unityrs-export` (`pyjson.rs`, Python-compatible JSON) and the batch engine (`docs/plans/batch.md`, spec version 1).
- The ETC and EAC decoders (`unityrs-texture`'s `src/etc.rs`, a port of texture2ddecoder 1.0.5's `etc.cpp`), Tier A but for two Tier C corrections, merged under P3 before this plan and the reason `verified P3` exists as a status. With them, 13 of the 78 TextureFormat rows decode verified.
- P2a's OBJ writer (`docs/plans/static3d.md`), which the extractor's `exportMesh` calls.

**What it delivers.** UnityPy 1.25.2 parity for every `PARITY.md` row still open, except the rows P2a, P2b and P4 own: every TextureFormat, the containers and codecs, UnityCN, lossy WebP and Pillow's other writers, the exporters and the extractor, audio, write-back and `set_image`, the TPK fallback, the HEAD tier, and the Python API (`unityrs-py`).

**How it is checked.** As in P1b and P2a: a row becomes `verified` only in the PR whose Rust test pins it, replaying what the oracle recorded (`tools/fixtures/<package>.py`, run in the amd64 oracle image, with a `--check` mode) where UnityPy's side was run. Tier B, C and Tol rows are held to the reference their rows name. P3c's rows are checked by a binding suite run against UnityPy 1.25.2 in the oracle image, at level API.

**Citations.** UnityPy behaviour is cited by its `PARITY.md` row (section and item), never by a line of UnityPy's source here; the rows and the ported files hold those lines. Corpus figures are aggregates from the private corpus, "the corpus" below, and nothing else of it is published.

---

## 0. Decisions

| # | decision | why |
|---|---|---|
| 1 | **Ports, not crates.** A decoder crate is adopted only if it is bit-exact with the oracle and faster than our port, passes the licence and deny checks, and can be wrapped by the memory meter; otherwise our own port is sped up. A comparison of candidate crates on the corpus's textures (about 44,000, all BC7, DXT1, DXT5, RGB24 or RGBA32) found none both bit-exact and faster across the formats tested, so no crate was adopted, and the decoders are ports of the code the oracle itself calls: Pillow for raw and BCn data, texture2ddecoder's C++ for ETC, EAC, ATC, PVRTC and crunch, astcenc for ASTC. C or C++ is vendored only where byte-equality needs that exact build: libwebp, liblz4's HC compressor, etcpak, and astcenc's encoder. `docs/DESIGN.md` section 4 states the rule. | The corpus holds no P3 format, so P3 applies the rule per format on synthetic oracle fixtures. The candidate for those formats, texture2ddecoder 0.1.2, differs from the code the oracle calls on known cases (ASTC LDR and HDR rounding, BC6H, ATC), can panic on bad ASTC data, and allocates infallibly outside the meter, which the CR10 rules (`docs/plans/batch.md` section 6) forbid after the batch engine's running head. Nothing of it is added, even as a development dependency. |
| 2 | **P3 is three phases.** P3a, consumer extraction parity: the texture formats and their gates, batch spec version 2, the containers and codecs read, UnityCN, lossy WebP and the fast PNG opt-in (waves 1 and 2, T7 and T8). P3b: the exporters and the extractor, Pillow's other writers, audio, write-back and `set_image`, the TPK fallback and providers, and the HEAD tier. P3c: the Python API. `docs/DESIGN.md` section 5 gives them about 14k, 15.5k and 6.5k non-test lines. | The bottom-up estimate (section 3) is about 36k lines against the 20k section 5 gave P3, and P2a's came in at about 2x too. Splitting keeps the consumer's path independent of the rest. |
| 2b | **Fast PNG is an opt-in only** (`image: png_fast`, decision 3), written with the `png` crate's Balanced setting. Its bytes are not Pillow's; its visible-pixel hash is. The default stays byte-equal with Pillow 9.5.0, and no golden spec uses the opt-in. | PNG encoding is about 97% of the decode-plus-encode CPU on the corpus, and Balanced measured about 2.6x faster there. |
| 3 | **Batch `spec_version` 2:** an explicit `outputs.textures.formats` list, defaulting to version 1's five formats (a format the binary cannot decode is `bad_spec`); `image: png | png_fast | webp{quality}`; and an optional UnityCN key given as `{"env": NAME}`, as `headers` takes secrets. No audio, text or shader output kinds. Split and zip inputs wait for a version 3, if needed. Version 1 is unchanged. | Version 1's golden specs pin `unsupported_format` skips and run against every later binary, so widening its formats would break them. With an explicit list, a later format joins it without another version bump. |
| 4 | **ASTC:** a pure-Rust port of astcenc's decompressor (metered, no `unsafe`), with FFI only for the encoder, behind `encode`. PR T2a first pins the astcenc version astc-encoder-py 0.1.12 bundles. Fixtures are recorded on amd64 only. | The oracle image's 12 ASTC_HDR failures of `test_texture_matrix.py` happen on arm and not on amd64; the reference host is amd64. |
| 5 | **Crunch fixtures:** an in-code builder that writes valid crunch streams of both variants, which UnityPy decodes in the oracle, now. Editor-built crunch fixtures (DXT and ETC, at 2017.2 and at 2019 or later) are added to the owner-built fixture project (section 6). | The oracle image has no crunch encoder, and the corpus has no crunch texture. |
| 6 | **libwebp:** vendored at exactly 1.3.0, the version Pillow 9.5.0's wheel bundles, as zlib 1.2.13 is vendored (sha-checked sources, `cc`, `unsafe` confined to one module). Its `malloc`, `calloc` and `free` are compiled through to Rust's global allocator. `scripts/third_party_licenses_check.py` learns a `NOTICE-` form for libwebp's PATENTS. | `libwebp-sys` builds a newer libwebp, whose bytes differ. Redirected allocation lets the counting allocator see libwebp and turns a refusal into `VP8_ENC_ERROR_OUT_OF_MEMORY`, which keeps it fallible under CR10. |
| 7 | **GIF, JPEG, MPO, PDF and JPEG2000 writers are P4's.** Until then unityrs refuses them with a named `Unsupported`: a temporary U whose target is P4. Reversible, and nothing exists yet to lower. `PARITY.md` section 15.8's tier column is printed by `tools/parity/pillow_tables.py writers` from what Pillow does and records the target (A), so PR W2 records the temporary U beside that table rather than in its generated cells. | GIF needs Pillow's quantiser (about 1.5k lines); JPEG, MPO and PDF need libjpeg-turbo 2.1.5.1, and JPEG2000 OpenJPEG 2.5.0. None is on the consumer's path. |
| 8 | **Audio fixtures** wait on the project owner: Editor-built AudioClips (Vorbis, ADPCM and PCM, for Android and iOS, MP3 if available) from synthetic tones of our own, checked in as Editor fixtures (section 6). A1 and A2 proceed on the banks FMOD accepts. FMOD stays a test oracle in the image, never bundled. | FMOD refuses the Vorbis, Opus, PCM8, PCM24, PCM32 and float banks built here. |
| 9 | **Audio provenance:** vgmstream's licence is verified before A2 ports its ADPCM and FADPCM decoders. The Vorbis setup-header table, data derived from FMOD, is never committed: it is loaded at runtime from a path the user gives, as the TPK is (core Q2). | Both are unverified provenance; the table is not ours to publish. |
| 10 | **Audio tolerances:** PCM exact; ADPCM and FADPCM exact, else at most 1 LSB; Vorbis the same sample count, per-channel SNR of at least 90 dB and at most 2 LSB (int16) against FMOD. Each `PARITY.md` Tol row states its threshold when A2 or A3 lands. | The `docs/DESIGN.md` section 3 Tol definition asks each row to state a metric and threshold. |
| 11 | **MPEG and Opus** are not built in P3, and report an unsupported optional feature. | symphonia is MPL-2.0, which is not accepted; libopus would be more vendored C. Revisited on request. |
| 12 | **UnityCN:** RazTools/Studio's licence is verified before C3 and its notice carried. No real key is ever committed; real samples wait for P4. | `docs/DESIGN.md` section 4 says P3 records that licence; UnityPyBoost's decryptor is based on its `UnityCN.cs`. |
| 13 | **`UnityPy.classes` in Python** is generated at import from a user-supplied TPK, by default the installed UnityPy's `lzma.tpk`, by a port of UnityPy's class generator. | `classes/generated.py` is generated from the TPK, so committing a port of it would break core Q2. The other option, depending on UnityPy at runtime for the classes and enums, was not taken; generating them is consistent with Q2. |
| 14 | **The 20 `impl P1` rows** only a binding can test (Environment, detection, version and class API) are P3c's, folded into Y1, Y3 and Y5; `PARITY.md` relabels them. | P1 built the Rust side; their remaining check is the binding's. |
| 15 | **HEAD reading** is implemented from HEAD's source and tested on synthetic files against HEAD's vendored reader; its rows stay a target until Unity 6000.4/6000.5 Editor fixtures exist, which wait on the project owner (section 6). | `docs/DESIGN.md` section 3: equality with HEAD's dicts is measured only on those fixtures. |
| 16 | **`set_image`'s encoders:** B3b opens with a probe of whether etcpak's and astcenc's output depends on the build or the instruction set. If it does, a scalar or invariant build is pinned, and the acceptance rule is equal after re-decode. | `docs/DESIGN.md` section 3, Writing: encoder exactness is a property of a build. |
| 17 | **LZ4HC** is vendored (liblz4's `lz4hc.c` at the version python-lz4 4.4.4 bundles, BSD-2-Clause, through `cc`, with a charged constant state), not ported. | Byte-equality with liblz4 HC level 9 is the target, and vendoring the exact source is the direct route to it, as it was for zlib. |
| 18 | **The TPK stays unembedded.** Tests read the oracle image's copy at test time. | Core Q2. |
| 19 | **Audio lives in a new crate, `unityrs-audio`.** No batch kind for audio, text or shaders in P3. | It keeps `unityrs-export` light and isolates lewton, which allocates infallibly; CR10 binds audio only if a batch kind is added. |
| 20 | **texture2ddecoder's quirks are Tier A:** ATC's palette wrap, and any PVRTC quirk, are kept. | They are UnityPy's output, and no normative reference contradicts them. |
| 21 | **The ETC 3DS formats** get the decided Tier U refusal in `select` and `plan`, and no decoder. | `PARITY.md` section 17 files them U. |

---

## 1. Where it starts

### 1.1 Done

- ETC and EAC: the eight formats decode as texture2ddecoder 1.0.5 does, Tier A but for two Tier C corrections (`PARITY.md` section 19.1). Version 1 of the batch engine still skips them as `unsupported_format`, because `docs/BATCH_SPEC.md` section 3 is closed for version 1.
- The five P1b formats (BC7, DXT1, DXT5, RGBA32, RGB24), with exact modes.
- Object re-save (L3); the TPK provider; the codec registry; the detection of every container P3 reads.

### 1.2 Not done

`unityrs-py`; any audio code; the ASTC, PVRTC, ATC and crunch decoders; the Switch and Xbox 360 gates; the Tier B and C formats; the Shader, Font, TextAsset, MonoBehaviour and VideoClip exporters, and the extractor; Pillow's writers other than PNG; WebP; file and bundle save; `set_image`; the HEAD-tier reader.

**P2a's, not P3's** (`docs/plans/static3d.md` decision 1): the OBJ and MTL rows, `Mesh.export`, `Renderer.export` and `Material`, the MeshRenderer Tier C rows, OBJ NaN, vertex-less mesh export, and the Rust side of the `GameObject` component accessors. P3 keeps the extractor's GameObject crawl and the binding of those accessors.

### 1.3 The rows, by phase

PR P3-0 tags every `PARITY.md` row whose unityrs side was P3's with its phase, as `impl P3a`, `impl P3b` or `impl P3c` (and section 17.1's references as `planned (P3a)`); a row that gives a part to the bindings names P3c for it. `verified P3` stays the status of the ETC and EAC work verified before the cut, which is P3a's. The assignment follows the areas of section 2:

| phase | rows | areas | `PARITY.md` sections |
|---|---|---|---|
| P3a | 153 | texture formats, their helpers and gates; containers and codecs read; UnityCN | 17 and 17.1 (all); 15.1's decoders, padding, flip and Xbox swap; 15.6; `Texture2DArray`'s rows (14.2, 19.2); 19.1's RGBAHalf and ASTC_HDR; 19.2's texture rows; 3's zip, split and `load_assets` rows; 4's zip and gzip detection; 5.1; 5.2's `WebFile`; 6's decompressors and magic numbers; 12 but its save; 19.2's gzip, zip and `.splitN` rows |
| P3b | 134 | write-back and `set_image`; exporters and extractor; audio; TPK and providers; HEAD | every L3 and L4 row (3, 5.2, 6, 7.2, 8, 9.1, 9.3, 12, 13, 18, 19.1, 19.2); 15.1's `image_to_texture2d` and encoders, and 19.2's `set_image` rows; 14.2's Shader, Font, TextAsset, VideoClip and AudioClip; 15.5; 15.7; 16; the Shader, extractor and audio rows of 19.1 and 19.2; 8's provider rows, 10 and `Environment.typetree_generator`; 7.1 and 20 |
| P3c | 106 | the Python API | the API-level rows of 2, 3, 5.2, 6 (the codec maps), 8, 9.1 to 9.3, 11, 13, 14.1 to 14.3, 19.2 and 23; the parts other rows give to the bindings; the 20 rows decision 14 moves from P1 |

Rows with no status column are tagged by the PR that lands them: section 15.8's writers (W1, W2 and decision 7), section 2.1's re-exports and section 9.2.1's classes (Y2 and Y5).

---

## 2. Areas

**Rules every area follows.**
- Fixtures are synthetic, built in code by `tools/fixtures/<package>.py`, which runs in the amd64 oracle image, records UnityPy's results as small JSON, and has a `--check` mode; a Rust test replays them. No game bytes are committed.
- Every rule a port follows cites UnityPy 1.25.2's `path:line` in the ported file, as `CLAUDE.md` says.
- No panics: the deny lints, and seeded mutation cases per module. New parsers get fuzz targets: WebFile, UnityWeb, split and zip, UnityCN, crunch, ASTC, PVRTC and FSB5.
- New crates link `LICENSE-MIT` and `LICENSE-APACHE` to the repository's.
- **Memory** (core sections 3.15 and 7.0): every input-sized buffer is charged before it is allocated, through `MeteredVec` or `DecodeBuf`; `plan()` gives a decode's charges before any data is read; decoding runs under `contain()`.
- **CR10** (`docs/plans/batch.md` section 6): P3 code reachable after the batch engine's running head allocates fallibly from its first commit, and a PR that adds an entry to the refusal sweep's inventory (`crates/unityrs-batch/tests/controller_support/refusals.rs`) fails review.

### 2.1 Texture formats (`unityrs-texture`, P3a)

Each format group is a module of its own, so the shared tables (`format.rs`, `select.rs`, `decode.rs`) stay small and merges stay simple.

| group | port of | tier, level | oracle |
|---|---|---|---|
| Pillow's raw and BCn remainder: Alpha8, ARGB4444, RGB565, BGR24, DXT3 (BC2), RGBA4444, BGRA32, BC4 (into L), BC5 (into RGB), R8, ARGB32 | Pillow 9.5.0's `Unpack.c` unpackers and `BcnDecode.c` BC2, BC4 and BC5 | A, L5 | Pillow through UnityPy |
| ASTC LDR (12 formats) | astcenc's decompressor as astc-encoder-py 0.1.12 bundles it, with `USE_DECODE_UNORM8`: integer sequence decoding, block modes, partitions, colour unquantisation, decimation and infill, dual plane, void extent | A, L5 | astc-encoder-py in the oracle |
| ASTC HDR (6 formats) | astcenc's HDR profile, then `PARITY.md` section 17.1's half-and-float rule | C, L5 | a divergence test against astcenc's HDR decode in the oracle image |
| PVRTC 2 and 4 bpp (4 formats), ATC RGB and RGBA (2) | texture2ddecoder 1.0.5's `pvrtc.cpp` and `atc.cpp` | A, L5, with ATC's palette wrap kept (decision 20) | texture2ddecoder through UnityPy |
| crunch: DXT1Crunched and DXT5Crunched (legacy), ETC_RGB4Crunched and ETC2_RGBA8Crunched (Unity's fork), split at 2017.3 | texture2ddecoder's crunch: `crn_decomp.h` and Unity's fork of it | A, L5 | texture2ddecoder through UnityPy, on decision 5's fixtures |
| Tier B raw formats: RHalf, RGHalf, RFloat, RGFloat, RGBFloat, RGBAFloat, RG16, R16, RG32, RGBA64, the eight signed formats, RGB9e5Float, YUY2 | section 17.1's closed forms, not a port | B, L5 | `test_texture_matrix.py`'s Tier B references, and divergence tests |
| Tier C raw formats: RGBAHalf, RGB48 | the closed forms | C, L5 | pinned divergence tests |
| BC6H | the Khronos Data Format Specification 1.3, section 20.2, then the half-and-float rule; neither Pillow nor texture2ddecoder is the reference | B, L5 | a divergence test |
| the Switch deswizzle, the Xbox 360 byte swap, `pad_image`, `Texture2DArray` (its GraphicsFormat map, and its B, C and U rows) | UnityPy's `helpers/TextureSwizzler.py`, `export/Texture2DConverter.py` and `classes/legacy_patch/Texture2DArray.py` | A; B, C and U per `PARITY.md` section 19.2 | UnityPy |
| Tier U: the ETC 3DS formats, ARGBFloat, odd-width YUY2, a Switch-swizzled texture of a `sw —` format | a refusal in `select` and `plan` (decision 21) | U | refusal tests |

**Licences.** ASTC adds Apache-2.0 (the crate's expression gains `AND Apache-2.0`, with a `LICENSE-ASTCENC` file and an `about.hbs` section). Crunch adds Zlib, Binomial's crunch's licence and, to be verified, that of Unity's fork (`AND Zlib`, `LICENSE-CRUNCH`, `LICENSE-UNITY-CRUNCH`). PVRTC and ATC are covered by the notices the crate already carries (texture2ddecoder, AssetStudio, mikunyan). The Tier B and C closed forms port no code.

**Memory.** Output goes through `plan()` and `MeteredVec`. Crunch's unpacked block buffer (8 or 16 bytes a block) and its Huffman tables, bounded by the header, are charged as further buffers. ASTC's decimation and partition tables are computed per decode and charged; T2a measures their size (about 1-2 MB per block size), against generating them with `tools/gen`. PVRTC needs no intermediate buffer. Work stays linear in the output.

**Size:** about 9.2k non-test lines: raw and BCn remainder 650, ASTC LDR 2,500, ASTC HDR with BC6H 1,100, PVRTC and ATC 800, crunch 2,400, Tier B and C raw formats 800, gates and `Texture2DArray` 900.

### 2.2 Batch exposure (`unityrs-batch`, `docs/BATCH_SPEC.md`, P3a)

`spec_version` 2 as decision 3 gives it. Checked by golden version 2 specs at L5, L5b and L9, the X-gate rows, and the nightly corpus run for BCn; the corpus has no ETC or ASTC, so those formats rest on testkit fixtures. The charges are version 1's (`docs/plans/extract.md` E59). About 1.0k lines.

### 2.3 Containers and codecs (`unityrs-core`, P3a)

**`.splitN`, zip and apk.** Ports of `environment.py`'s `load_assets`, `load_zip_file` and `_load_split_file`. The zip reader is our own central-directory reader, which may port CPython's `zipfile` (PSF-2.0, already accepted), with stored and deflate members inflated through flate2 into a `DecodeBuf`. Tier A, with `PARITY.md` section 19.2's B and C rows (a ZIP opened from a path, split members of a zip, `.splitN` before the end of a path). Joined split parts are one `Source`, charged against `max_input_bytes`. Checked at L0 to L2 against UnityPy on testkit zips and splits. About 1.1k lines.

**UnityWeb and UnityRaw formats 1 to 6, and WebFile (plain, gzip, brotli).** Ports of `files/BundleFile.py`'s `read_web_raw` and of `files/WebFile.py`. brotli through `brotli-decompressor` (BSD-3-Clause or MIT), whose allocator trait lets it be metered. gzip is Tier B, checked against the same file loaded uncompressed. Section 5.2's negative lengths follow their rows (−1 Tier A, below −1 U). Checked at L0 to L2 against UnityPy with brotli 1.2.0. About 1.1k lines.

**Memory.** Inflation goes into a `DecodeBuf` under `max_inflate`; the registry's charges are P1's.

### 2.4 UnityCN (`unityrs-core`, P3a)

A port of UnityPyBoost's `ArchiveStorageDecryptor.cpp` and UnityPy's `helpers/ArchiveStorageManager.py`: `decrypt_key`, `read_vector`, and `decrypt_block` with its LZ4-token walk, with `PARITY.md` section 12's Tier U row for a walk past the block. The key's AES-ECB uses the `aes` crate (MIT or Apache-2.0). It is exposed through the existing `BlockDecryptor`, a key API, and `set_assetbundle_decrypt_key`. Checked at L0 to L2 on synthetic encrypted bundles the testkit builds with a test key, which UnityPy and pycryptodome 3.23.0 decrypt (decision 12 for the licence and the keys). Decryption is in place and the key tables are constant. About 800 lines.

### 2.5 Lossy WebP and the image writers (`unityrs-imageio`)

**WebP (P3a).** Decision 6's vendored libwebp 1.3.0, charged a measured constant plus a per-pixel bound before encoding, behind a port of Pillow 9.5.0's `WebPImagePlugin._save` semantics (mode conversion, quality 80 and method 4 by default, `exact`). L5b byte-equality with Pillow at the defaults and at quality 95. Licence BSD-3-Clause, plus libwebp's PATENTS as a notice. About 900 lines.

**Fast PNG (P3a).** Decision 2b. About 300 lines.

**Pillow's other writers (P3b).** The writers built on Pillow's own encoders: BMP and DIB, TGA, PPM, PCX, SGI, IM, TIFF (raw), DDS, ICO and ICNS (PNG inside), and EPS. Each is Tier A where Pillow saves, L5b against Pillow 9.5.0, MIT-CMU. About 1.4k lines. GIF, JPEG, MPO, PDF and JPEG2000 are P4's (decision 7).

### 2.6 Exporters and the extractor (`unityrs-export` and the CLI, P3b)

**TextAsset, Font, MonoBehaviour and VideoClip.** TextAsset writes `m_Script` with surrogateescape; Font writes `.ttf` always, since UnityPy's OTTO test never holds; MonoBehaviour writes `json.dumps(indent=4, ensure_ascii=False)`, so `pyjson.rs` gains non-ASCII output and surrogateescape; VideoClip writes the bytes of `m_ExternalResources`, which UnityPy has no exporter for (tier X). Tier A, L7. About 800 lines.

**Shader.** A port of `export/ShaderConverter.py` (about 720 Python lines), Tier A up to 2021.3.10; then `PARITY.md` section 19.1's Tier C (`m_PlayerSubPrograms`, `m_ParameterBlobIndices`, the 6000.3 sub-program layout) and section 3's Tier U refusals (big-endian 5.3 and 5.4 blobs, `GpuProgramIndex`). Checked at L7 against UnityPy, and against a corrected-oracle run on the corpus's 980 Shaders (aggregates only). AssetStudio's MIT readers may be cited for layout facts; AssetRipper is read for knowledge only. About 1.4k lines for the port and 900 for the Tier C work.

**The extractor.** `extract_assets`, the export tree, file naming, and section 19.2's Tier C crashes (the GameObject crawl, MonoBehaviour, a missing `dst`, an ICO with a side below 16, images Pillow cannot save), and `unityrs extract` in the CLI, at L9. It depends on the writers, Shader, audio and P2a's OBJ. About 1.3k lines.

**Memory.** Output text is written into a metered sink, as `docs/plans/static3d.md`'s `MeteredWrite`.

### 2.7 Audio (`unityrs-audio`, P3b)

A port of `AudioClipConverter.py`'s dispatch: Ogg, RIFF and m4a pass through; the FSB5 header and subsounds; PCM 8, 16, 24, 32 and float, with fmod_toolkit's 44-byte header and float-to-int16 rule; Tier C for fewer than 4 data bytes, Tier U for a bank whose first subsound has no samples. IMA ADPCM and FADPCM are ported from vgmstream (decision 9); Vorbis decodes with lewton (MIT or Apache-2.0), plus the FSB5 setup-header rebuild following python-fsb5 and Fmod5Sharp (both MIT). MPEG and Opus: decision 11. Checked at L8 against FMOD through fmod_toolkit 0.1.3, in the oracle image only, within decision 10's tolerances, on decision 8's fixtures. The WAV buffers are charged; lewton allocates infallibly, so audio is library, CLI and Python only (decision 19). About 2.5k lines.

### 2.8 Write-back (`unityrs-core`, `unityrs-texture`, P3b)

**SerializedFile save.** Headers for v ≥ 9 and v < 9, `SerializedType.write`, blob trees; Tier C (writing) for pre-v10 and v11 trees, `SERIALIZED_FILE_PARSE_TYPETREE` and `PARITY.md` section 19.1's `dump_blob` offset 0; `mark_changed` and `set_raw_data`. L4: semantic equality and idempotence, and a UnityPy-layout mode byte-equal to `save(packer='none')`. About 1.4k lines.

**Bundle save and packers.** `save` and `save_fs`, `chunk_based_compress`, `compress_lzma` through liblzma's encoder (already linked); LZ4HC byte-equal at liblz4 HC level 9 (decision 17); `WebFile.save` with gzip and brotli, equal after decompression; `get_writeable_cab`; section 3's Tier C (writing) rows; and the HEAD tier's `lz4hc` and format-9 writes, against HEAD's vendored writer files. About 1.3k lines for bundle save and the packers, 900 for WebFile save, writeable CABs and the HEAD writes.

**`set_image`.** `image_to_texture2d`; Pillow 9.5.0's BICUBIC resize for mips (a port of `Resample.c`, MIT-CMU) and NEAREST padding; the Switch swizzle; section 18.1's modes, and the Tier C and U rows. The encoders sit behind an `encode` feature: etcpak at the source etcpak 0.9.14 wraps (BSD-3-Clause) and astcenc's encoder (Apache-2.0), vendored C++ in a `-sys`-style crate (decision 16). The acceptance rule is equal after re-decode; BC7 is Tier C, byte-equal to `compress_bc7`. About 1.2k lines plus 700, plus the vendored sources.

**Memory.** Writer output goes into a `DecodeBuf` on the object's meter (core 3.11); encoder states are charged as measured constants, as liblzma's decoder charge is.

### 2.9 TPK fallback and providers (P3b)

The provider is P1's. P3b adds a stripped-file L2 fixture read with the oracle image's `lzma.tpk` at test time, never committed (decision 18); the L2b node-list diff; the `nodes=` list through `TypeTreeNode.from_list`; `parse_monobehaviour_head` and `generate_monobehaviour_node`, with a provider trait for TypeTreeGeneratorAPI-style and Il2CppDumper-style dumps (`docs/DESIGN.md` section 4); and `config.get_fallback_version`. Tier A, at L2b, L2 and API. About 800 lines.

### 2.10 HEAD tier (P3b)

SerializedFile v23's per-type node info, v26's shared subtrees, and bundle format 9's 64-bit block offsets, read as HEAD `417d595` reads them, tested on synthetic files against HEAD's reader files vendored beside `tools/parity/tests/head_417d595/` (MIT). The status stays a target until decision 15's fixtures exist. Writing sits in the bundle-save PR (2.8). About 1.0k lines.

### 2.11 The Python API (`unityrs-py`, P3c)

PyO3 abi3, built with maturin, under core section 4.2's rules: the GIL is released on every call into the core, budgets are Rust counters, and `fork()` is safe only while no call is in flight in another thread. It covers `Environment`, the file classes and `ObjectReader`; object mode by UnityPyBoost's `parse_class` rule (`PARITY.md` sections 9.2 and 9.2.1); `UnityPy.classes` (decision 13); the enums, `TypeTreeNode`, `UnityVersion`, `ContainerHelper` and `config`; the export helpers (`Texture2D.image` as a PIL image, so Pillow is a runtime dependency, `Sprite.image`, `AudioClip.samples`, `Shader.export`, `set_image`, `save`); the extractor module and the package re-exports; and decision 14's rows. Checked by a binding suite against UnityPy 1.25.2 in the oracle image, at level API. About 6.4k lines over five PRs.

---

## 3. PRs

About 400-1,500 non-test lines each. Each depends on the PRs it names; the first PR of a wave does not wait for the previous wave to finish.

| # | PR | phase | contents | ~lines | depends on |
|---|---|---|---|---|---|
| P3-0 | plan and docs | — | this document; `PARITY.md`'s phase tags (1.3) and decision 14's relabelling; `docs/DESIGN.md`'s phase table and crate-versus-port rule | docs | — |
| **Wave 1: mobile textures and the batch** | | | | | |
| T1 | Pillow raw and BCn remainder | P3a | the 11 formats; `Unpack.c` and BC2, BC4, BC5; `tools/fixtures/texture_raw.py` | 650 | — |
| T2a | ASTC LDR, part 1 | P3a | the astcenc pin; block modes and ISE decoding, partitions, colour unquantisation; tables | 1,400 | — |
| T2b | ASTC LDR, part 2 | P3a | decimation and infill, dual plane, void extent, the UNORM8 store, `plan` and `select`, `texture_astc.py` fixtures (every block size and mode) | 1,100 | T2a |
| T3 | batch spec version 2 | P3a | decision 3: the `formats` list, the `image` option (PNG now; WebP and `png_fast` once W1 and W1b land), the UnityCN key field (wired when C3 lands), version 2 golden specs, X-gate rows | 1,000 | T1 |
| T4a | crunch, legacy | P3a | the crn unpacker for DXT1Crunched and DXT5Crunched; the in-code crunch builder | 1,400 | — |
| T4b | Unity crunch | P3a | the 2017.3 and later variant, ETC_RGB4Crunched and ETC2_RGBA8Crunched | 1,000 | T4a |
| C1 | split and zip | P3a | `.splitN`, `load_zip_file`, `load_assets`, the section 19.2 rows | 1,100 | — |
| C3 | UnityCN | P3a | the decryptor, the key API, synthetic encrypted fixtures | 800 | — |
| **Wave 2: the rest of mobile, and the web** | | | | | |
| T5 | PVRTC and ATC | P3a | the ports and their fixtures | 800 | — |
| C2 | UnityWeb, UnityRaw and WebFile | P3a | formats 1 to 6; plain, gzip and brotli; `brotli-decompressor` | 1,100 | C1 (shared load code) |
| W1 | lossy WebP | P3a | libwebp 1.3.0 vendored, Pillow's `_save`, L5b; T3's `webp` option | 900 | T3 |
| W1b | fast PNG | P3a | `png_fast` (decision 2b) | 300 | T3 |
| T6 | platform gates and `Texture2DArray` | P3a | the Switch deswizzle (`sw` table), the Xbox 360 swap for every format, `pad_image`, `Texture2DArray` and its section 19.2 rows | 900 | T1 |
| **Wave 3: correctness, exporters, audio** | | | | | |
| T7 | Tier B and C raw formats | P3a | half, float, integer, signed, RGB9e5Float, YUY2, RGB48, RGBAHalf; divergence tests | 800 | T1 |
| T8 | BC6H and ASTC HDR | P3a | Khronos section 20.2; the HDR profile, Tier C | 1,100 | T2b, T7 |
| E1 | simple exporters | P3b | TextAsset, Font, MonoBehaviour JSON, VideoClip | 800 | — |
| E2 | Shader, Tier A | P3b | the ShaderConverter port | 1,400 | — |
| E3 | Shader, Tier C and U | P3b | player sub-programs, the 6000.3 layout, refusals; corpus L7 aggregates | 900 | E2 |
| A1 | audio core | P3b | `unityrs-audio`: dispatch, passthrough, FSB5, PCM; the extension table | 1,000 | — |
| A2 | ADPCM and FADPCM | P3b | the decoders; fixtures against FMOD | 600 | A1, decision 9 |
| A3 | Vorbis | P3b | lewton and the header rebuild; the Tol harness | 900 | A1, decisions 8 and 9 |
| K1 | TPK and providers | P3b | the stripped fixture, the L2b diff, `from_list`, the MonoBehaviour generator hook, `get_fallback_version` | 800 | — |
| W2 | Pillow's writers | P3b | BMP, DIB, TGA, PPM, PCX, SGI, IM, TIFF, DDS, ICO, ICNS, EPS, and the Tier C PNG fallbacks; decision 7's refusals | 1,400 | — |
| **Wave 4: write-back, HEAD, the extractor** | | | | | |
| B1 | SerializedFile save | P3b | headers, types, blobs, Tier C trees, `dump_blob`, `mark_changed` | 1,400 | — |
| B2a | bundle save | P3b | `save` and `save_fs`, the packers, vendored LZ4HC, Tier C rows | 1,300 | B1 |
| B2b | WebFile save, writeable CABs, HEAD writes | P3b | `WebFile.save`, `get_writeable_cab`, `lz4hc` and format 9 against HEAD's writer | 900 | B2a, C2 |
| B3a | `set_image`, core | P3b | `image_to_texture2d`, BICUBIC, padding, the swizzle, section 18.1's modes, Tier C and U | 1,200 | B1, T6 |
| B3b | encoders | P3b | the `encode` feature: vendored etcpak and astcenc's encoder; the probe (decision 16); BC7 Tier C | 700 | B3a |
| H1 | HEAD reading | P3b | SerializedFile v23 and v26, bundle format 9; HEAD's reader vendored for tests | 1,000 | — |
| E4 | the extractor | P3b | `extract_assets`, naming, the Tier C crashes, `unityrs extract`, L9 | 1,300 | E1 to E3, A1, W2, P2a's OBJ PR |
| **Wave 5: Python** (Y1 may start in wave 2) | | | | | |
| Y1 | `unityrs-py` scaffold | P3c | the crate, maturin abi3, its CI job, `Environment`, the files and `ObjectReader`, `Value` to Python, errors, the GIL | 1,500 | — |
| Y2 | object mode and classes | P3c | the `parse_class` rule, the class map and generator (decision 13), the base and math classes, `PPtr` | 1,500 | Y1 |
| Y3 | helpers API | P3c | `TypeTreeNode`, `UnityVersion`, `ContainerHelper`, the enums, `config`, `peek_name` | 1,200 | Y1 |
| Y4 | export API | P3c | `image`, `samples`, `export`, `set_image`, `save` | 1,200 | Y2, and the T, A, E and B PRs |
| Y5 | extractor and re-exports | P3c | UnityPy's `tools` equivalent, section 2.1's re-exports, decision 14's rows, the binding suite | 1,000 | Y4, E4 |

**Totals:** P3a about 14.4k non-test lines, P3b about 15.6k, P3c about 6.4k; about 36k in all, which `docs/DESIGN.md` section 5 now gives (decision 2).

**The consumer path.** T1, then T3, with T2a, T2b, T4a and T4b, is the shortest path to consumer value: once they land, a version 2 spec exports every mobile format, and C1 and C3 widen which inputs load.

**Merge order.** The T PRs share `format.rs`, `select.rs` and the plan tables, so they merge one at a time and rebase on each other. C1 and C2 run serially; C3, B1 and H1 touch disjoint modules.

---

## 4. Verification

- **Oracle fixtures.** Each PR's `tools/fixtures/<package>.py` records UnityPy's side in the oracle image on amd64 (decision 4), and CI's parity job checks it is current, as for P1b's fixtures. Its Rust test replays it, and the row's status becomes `verified` there.
- **Divergence tests.** Tier B and C rows compare unityrs with their stated reference (section 17.1's closed forms, astcenc's HDR decode, Khronos for BC6H, a corrected-oracle run for the Shader and the extractor), and pin UnityPy's own answer beside it.
- **Tol.** Audio rows state their metric and threshold (decision 10) in the PR that lands them.
- **The corpus** (aggregates only): BCn through batch version 2 in the nightly run, and the 980 Shaders' L7 comparison. It holds no ETC, ASTC, crunch, UnityCN, WebFile or audio, so those rest on synthetic and Editor fixtures until P4's coverage matrix, and the consumer's first version 2 run on ETC2 or ASTC bundles is the effective L10 check.
- **Binding suite** (P3c): the API rows, against UnityPy 1.25.2 in the oracle image.
- **Fuzzing:** a target per new parser (section 2's rules), seeded from the testkit as the existing targets are.

---

## 5. Risks

1. **Size overrun.** The estimate is about 1.8x `docs/DESIGN.md`'s old figure, and P2a's was 2x. Mitigations: the phase split (decision 2), the 1,500-line PR cap, and a consumer path (wave 1) independent of the rest.
2. **ASTC exactness.** Bit-exact output depends on astcenc's version, its UNORM8 rounding, the HDR float path and the decimation tables, and an arm-versus-amd64 divergence has already been seen. Fixtures are recorded on amd64 only, with a case per mode, partition count, dual plane, void extent and block size.
3. **Crunch.** A large bitstream port in two variants with a version split, with no encoder in the oracle image: everything hinges on decision 5's builder, and the corpus gives no real-world check.
4. **Vendored C and C++** (libwebp, liblz4 HC, etcpak, astcenc's encoder): the `unsafe` surface, allocator routing for CR10, output that may depend on the instruction set, builds on four targets (Linux and macOS, x86-64 and aarch64), and the cargo-about and deny bookkeeping. Mitigation: zlib's vendoring pattern of sha-checked sources and a reference differential test.
5. **Batch contract drift.** Widening formats without a version bump would silently change version 1's outputs; decision 3's explicit list prevents it, and version 1's golden specs stay green.
6. **CR10 regressions.** Every new decoder, encoder or loader reachable after the running head allocates fallibly; lewton and other crates' internals stay off the batch path, and the refusal sweep's inventory does not grow.
7. **Python scope.** Decision 13 shapes Y2 to Y5; abi3 and PIL interoperation add CI surface.
8. **Licences and provenance.** The FMOD-derived Vorbis table, RazTools/Studio, vgmstream, Unity's crunch fork and libwebp's PATENTS all need verifying. A failure blocks the PR that depends on it, not the phase.
9. **Thin real-world evidence.** Section 4's corpus bullet.
10. **Merge contention** in `unityrs-texture`'s shared tables and the core's `env/load`: per-format modules, and section 3's merge order.
11. **Known red tests.** The parity suite's ASTC_HDR failures on arm and its lz4 failures are understood in T2a and B2a, so they do not mask new regressions.

---

## 6. The Editor fixtures (owner-built)

P3's Editor-built fixtures are part of the one fixture project `docs/plans/static3d.md` section 8 describes, which the project owner is to build with a licensed Unity Editor; no date is set. P3 adds to it:
- crunch-compressed DXT and ETC textures, built at 2017.2 and at 2019 or later (decision 5);
- AudioClips from synthetic tones of our own: Vorbis, ADPCM and PCM (MP3 if available), for Android and for iOS (decision 8);
- Unity 6000.4 and 6000.5 files for the HEAD tier (decision 15).

Their bundles are checked in, as `CLAUDE.md` allows for Editor-built fixtures. What waits on them: `verified` on the crunch rows beyond the in-code builder's streams, on the Vorbis and other FMOD-refused audio rows, and on the HEAD tier's reading rows, which stay a target until then. No PR waits on them.
