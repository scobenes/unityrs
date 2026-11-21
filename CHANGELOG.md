# Changelog

All notable changes to unityrs are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html): the public
API of the published crates and of the `unityrs` Python package.

## [Unreleased]

## [1.0.0] - 2026-11-20

The first release: a Rust library, the `unityrs` command-line tool and the
`unityrs` Python package (one abi3 wheel per platform, Python 3.10+), all
measured against UnityPy 1.25.2 with UnityPyBoost. `PARITY.md` lists every
behaviour with its tier and its test; `docs/DESIGN.md` section 3 defines the
tiers.

### Added

- **Reading.** Asset bundles (UnityFS and the older formats), serialized
  files, typetrees, WebFile, UnityWeb, split files and ZIP archives, with
  UnityPy's detection and load rules; LZMA and LZ4/LZ4HC blocks, gzip and
  Brotli UnityWeb;
  UnityCN decryption; the TPK typetree provider for stripped files; and the
  newer formats UnityPy's development head reads (SerializedFile v23-26,
  bundle format 9).
- **Writing.** Object, file and bundle saves, and `set_image`.
- **Textures.** Every TextureFormat UnityPy decodes (BCn, ETC/EAC, ATC,
  PVRTC, crunch and ASTC among them): pixels and modes equal to UnityPy's
  where it is right (Tier A), and corrected against an independent
  reference where it is wrong (Tier C, such as signed EAC and ETC2
  punch-through alpha; `PARITY.md` section 19.1);
  the Switch and Xbox platform gates and Texture2DArray; PNG bytes identical
  to Pillow 9.5.0's; lossy WebP; and the other Pillow image writers.
- **Other exports.** Sprites (with tight-mesh rendering), the NGUI atlas
  cutter, meshes as UnityPy's OBJ/MTL, audio (FSB5 PCM, ADPCM and Vorbis to
  WAV), Shader, Font, TextAsset, VideoClip and MonoBehaviour exports, and
  UnityPy's extractor tree (`unityrs extract`).
- **3D beyond UnityPy.** glTF/GLB export of scenes and prefabs (skins, morph
  targets, materials, built-in meshes, static batching) and generic and
  legacy animation, checked with the Khronos glTF-Validator.
- **Batch extraction.** `unityrs batch`: a JSON job spec (versions 1 to 3)
  run in parallel under explicit memory budgets, writing to a directory,
  archives or S3-compatible object stores, with a run manifest and typed
  failure reasons.
- **Bounded input.** The storage unityrs retains or stages while it reads
  and decodes an asset input is charged to that input's budget before it is
  allocated, and size, work and depth limits refuse hostile input with a
  typed error. The budget covers that work only: values once handed to the
  caller (Python owns them), objects the caller builds, and third-party
  work such as Pillow's decoding of a caller's image for `set_image` are
  outside it (`crates/unityrs-py/README.md` states the threat model).
- **Python.** The `unityrs` package follows UnityPy's API (`Environment`,
  object mode, the generated classes from a TPK, the helpers and the
  exporters), with the GIL released while the core works.
- **One crate.** `unityrs` re-exports the library crates, each behind a
  feature (`batch` and the texture encoders are opt-in).
- **Agents and scripts.** `describe`, `inspect`, `show`, the canonical dump
  and the JSON protocols of `docs/AGENTS.md`.

### Known limitations

- Humanoid body motion is refused with a typed reason, and so is positive
  humanoid playback; it is planned for a later minor release. Controller
  state and blend-tree evaluation, eight-influence bone weights and sampled
  non-glTF properties are later slices too (`docs/plans/anim.md` section
  2.6).
- Where UnityPy raises on a valid request, unityrs returns output checked
  against an independent reference instead (Tier B): BC6H, decoded as the
  Khronos Data Format Specification says, and 20 other texture formats,
  among them the half and float formats. BC6H's reserved modes are refused.
- Where no reference can be named, unityrs refuses with a typed error
  (Tier U): ARGBFloat textures, for one.
- Each batch input decodes its own declared dependencies; a shared,
  pre-decoded dependency cache is planned.
- Vorbis audio needs a user-supplied setup-header table
  (`--vorbis-headers`); FMOD is never bundled.
- glTF materials and scene root order await comparison with Editor-built
  fixtures.
- No FBX export, and no IL2CPP/Mono typetree generator (a provider hook
  takes external dumps).
- Wheels are built for Linux (x86_64, aarch64) and macOS.

[Unreleased]: https://github.com/scobenes/unityrs/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/scobenes/unityrs/releases/tag/v1.0.0
