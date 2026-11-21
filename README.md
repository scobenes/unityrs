# unityrs

Fast Unity asset extraction in Rust: asset bundles, serialized files,
typetrees, textures, sprites, meshes, animation and glTF export, as a library,
a command-line tool and Python bindings.

**Status: pre-alpha.** The core reads asset bundles, serialized files and
typetrees as UnityPy 1.25.2 reads them. The CLI provides the canonical dump,
value-case protocol, batch extraction, OBJ/MTL and static scene/prefab GLB export.
The CLI also exports generic and legacy animation to GLB and runs
[GLB-only prefab batch jobs](docs/plans/anim.md#0-decisions).
Positive humanoid playback is [deferred](docs/plans/anim.md#26-later-slices).
The plan and phases are in [`docs/DESIGN.md`](docs/DESIGN.md).

Agents and scripts: see [the agent interface guide](docs/AGENTS.md) for `describe`, `inspect`, `show`, and the JSON protocols.

## Goals

- **Parity with [UnityPy](https://github.com/K0lb3/UnityPy).** Every behaviour is
  tested against UnityPy, pinned to an exact version and native stack, and sorted
  into tiers: bit-exact where UnityPy is right, correct where it raises or is
  wrong, and documented either way.
- **Speed through batching.** The work is decoded and encoded in parallel, and
  memory stays bounded. In the first version each input decodes what it reaches
  of its own declared dependencies, so no output depends on what runs beside it;
  a shared pre-decoded dependency cache is planned.
- **Any game.** Version detection, a typetree-provider chain for stripped files,
  and hooks for encryption, altered headers and custom compression.
- **3D export.** Prefabs, skins, Mecanim animation and scenes to glTF/GLB.

## Performance

<!-- bench:begin -->
unityrs against UnityPy 1.25.2 with UnityPyBoost, the same Python script on both, on public synthetic fixtures, on GitHub's ubuntu-24.04 runner (Intel(R) Xeon(R) 6975P-C), commit b4b4443667bd, 2026-11-21. Median of 7 rounds, each a fresh process with a warm file cache. API rows time the operation after imports; the extraction row times the whole process, imports included.

| operation (timer contains) | inventory | UnityPy | unityrs | speedup |
|---|---|---|---|---|
| load and list objects | objects-lz4 (20,000 objects), objects-lzma (20,000 objects), objects-2019 (20,000 objects), many-blocks (64.0 MiB entry) | 497.3 ms | 404.8 ms | ×1.2 |
| read every typetree | 40,000 objects in 2 layouts | 687.3 ms | 811.7 ms | ×0.85 |
| read and decode textures | 15 formats, 113.2 Mpixel | 1.11 s | 812.8 ms | ×1.4 |

Breakdown, memory, unityrs-only rows and how to rerun: [BENCHMARKS.md](BENCHMARKS.md).
<!-- bench:end -->

## Static 3D export

`unityrs gltf` writes one scene or prefab unit as a GLB. Load dependencies,
including a scene's `.sharedAssets` sibling, as additional inputs:

```sh
unityrs gltf level0 level0.sharedAssets --scene level0 -o scene.glb --report scene-report.json
unityrs gltf prefab.bundle geometry.bundle --prefab assets/example.prefab -o prefab.glb
unityrs gltf input.assets --path-id 123 -o root.glb --profile profile.json --mapper pbr
```

Select exactly one unit with `--scene FILE` (SerializedFile name), `--prefab PATH`
(exact container path targeting a GameObject), or `--path-id ID` (a GameObject
subtree or a SceneAsset's owning scene file). Ambiguous selectors fail. A scene's
roots follow its object-table order, pending Editor confirmation; children keep
`m_Children` order. Additional dependency roots never join the selected unit.
The library exposes the same selections through `unityrs_scene::Unit`.

`--profile FILE` accepts the strict version-1 [export profile](crates/unityrs-gltf/README.md).
The default mapper is unlit with saved-property extras; `--mapper pbr` selects
CorePbr. Built-ins default to empty nodes with `unity_builtin` extras and a counted
report reason. Set `"builtin":"resolve"` in the profile and supply
`--builtin-file FILE` for resolution through `CabResolver`, or opt into
`"builtin":"procedural"` for approximate Cube, Quad and Plane geometry.
Unsupported procedural shapes and missing resource objects degrade with reasons.
No Editor resource file is included. `--tpk FILE` supplies stripped schemas.

`--max-input-memory`, `--max-input-bytes`, `--max-decode-work` and
`--max-tree-depth` set the core's per-input bounds (bytes except depth).
Graph, collect and writer buffers charge the selected input's meter before
allocation. The report records repairs, omissions and peak charged bytes;
writer/collect details also appear in GLB asset extras. `--report FILE` adds
JSON with graph reasons and the invocation's aggregate peak. One renderer's,
material's or texture's bad data degrades with a reason; resource refusals fail.
Raw `normalise:false` output can fail glTF validation.

Output files and symlinks are overwritten by sibling-temp rename after successful
export. All destinations are checked for directories before publication.
GLB/report destinations in the same canonical directory whose final names are
equal under Unicode case folding are rejected as an argument error on every
filesystem, before either output is published. A
publish failure leaves earlier complete outputs in place and removes only this
invocation's remaining temps; an interrupted run may leave `.unityrs-gltf-*.tmp`
files, safe to delete. Exit status is 0 for success/help and 1 for any export or
argument error, following `unityrs obj`. Run `unityrs gltf --help` for all options.

`unityrs extract INPUT... -o DIR` exports the UnityPy extractor tree. It uses
container paths by default; `--no-container` exports objects by name.
`--ignore-first-container-dirs N`, `--append-path-id` (UnityPy's `_None`),
`--export-unknown-as-typetree` and repeated `--asset-filter CLASS` select its
behavior. A filter accepts Unity class names or numeric ids. `--tpk FILE`
supplies stripped schemas. The Rust API also accepts a filter that inspects
metadata and then the parsed object.

`--vorbis-headers FILE` supplies the runtime VST1 setup-header table for
FSB5 Vorbis WAV decoding. There is no default path and the table is never
fetched. It is loaded against the extraction budget before any input; a
missing or invalid table ends the run with exit 1. Without the flag, Vorbis
samples remain recorded object failures and a completed run exits 0.
Batch never calls the extractor or audio exporter.

`--max-input-memory N` defaults to 8589934592 charged bytes per loaded input
and, separately, one shared extraction budget for exporter buffers, paths,
the retained report and its JSON. `--max-input-bytes N` defaults to 4294967296
source bytes per input; `--max-decode-work N` to 34359738368 work bytes per
input and separately per extraction; `--max-tree-depth N` to 256. These use
the `gltf` limit spellings and defaults. A memory or work budget refusal is
a typed resource failure and ends extraction with exit 1, including a refusal
retained during an object export. Completed files can remain after a refusal.

JSON on stdout records completed writes, including overwritten files, and
object failures; `--report PATH` also saves it at a relative path beneath DIR.
Object failures are recorded and extraction continues. Exit status is 0 for a
completed extraction/help, including reported object failures, and 1 for an
argument, input, traversal, output-root or report error that prevents completion,
as for `gltf`. GameObject crawls use actual path ids and visit each file/id once.
MonoBehaviours use JSON or raw `.bin`; vertex-less meshes create no file.
Zero-width textures are skipped; other zero-side images pass through their
writers, retaining SGI and ICNS output, and produce no file where Pillow
raises or writes an ICO holding no image. Unsafe paths and symlink parents
are refused. Complete files replace earlier leaves by rename without following
leaf symlinks or modifying outside hard links.

Confinement is to directory inodes reached from the held `-o` descriptor by
safe relative opens, each component opened against its held parent without
following symlinks or `..`. It is established when those directories are opened,
not checked against their later pathnames. A concurrent rename moves a held
directory and its files together, and writes continue in that inode, just as
if the rename happened after extraction. A directory moved outside the `-o`
pathname can therefore receive subsequent writes through its held descriptor;
this does not grant access to a directory the renamer could not write to.
Interrupted runs may leave
`.unityrs-extract-*.tmp` files, safe to delete. The Python tools facade exposes
the nine exporters and extraction helpers with the same destination confinement.
It validates names and extensions before creating output, opens destinations only
when dispatch and filters reach an export, and preserves existing files on failed
image exports through atomic publication. Its 366 oracle comparisons and 197 wheel
tests include hostile path, shared-reader cursor and concurrency regressions,
primitive-map decoding budgets, empty and legacy GameObject components, and
decryption-key conversion deferred until encrypted bundle processing. Generated
node getters, result allocation, error callbacks and finalization run on the
calling thread; negative string reads reserve decoding storage for the remaining
input span. Array-map recordings include values and cursors for every alias.
Every enclosing reader array retains byte-span charges through handoff,
including whole-file copies and nested arrays. One-byte boundary tests cover
raw bytes, strings, aligned reads and typed arrays; recorded oracle comparisons
preserve successful values and cursors and document the aggregate budget refusal.
Concurrent first access shares file containers, readers and decoded sprite
textures under the environment lock. Container extraction accepts equivalent
destination spellings, including repeated trailing slashes. Public tool paths,
class export arguments and asset-derived output names become exact strings before
use; hostile subclass regressions check the surrounding filesystem too. Direct
exporters preserve defaults or ignored arguments for all falsy extensions,
retain scalar extension formatting, and validate effective output names.
Empty sources, unsupported objects and filtered objects skip unused destination
conversion; source-loading errors take precedence. Non-container extraction
accepts an empty destination as the current directory while validating names.
These cases have oracle comparisons and recorded replay. Standalone
SerializedFile parsing leaves the environment's file and CAB mappings unchanged.
Y6a’s recorded forced-parsing cases close PARITY section 22’s stream semantics
(memory-reader `read(None)` and BytesIO `read(-2)`) and earlier-child CAB retention
after a later container failure, with reader identity, cursor and endian checks.
Runtime configuration retains a context's
decryption key unless it supplies an override.
Standalone textured materials resolve current texture names through Y6a’s bounded
adapter. Y6b adds standalone helpers and Switch/crunch preprocessing for replacement
texture converters. Seven helper review recordings cover schema-error cursors,
truncated stream-object cursors, live primitive callbacks on the caller, empty
shader blobs with declared platforms, signed LZ4 arguments and embedded sizes,
shader construction from native asset readers, and memoryview string decoding
with cursor alignment. Shader constructors retain the owning reader's traversal
guard through construction; one-byte budget boundaries and the 20,000-keyword
case with 4 KiB remaining have wheel regressions.
Detached-schema tree reads inherit the native reader's meter and limits through
accelerated parsing. Direct reference lookup, decryptor AES staging and
empty-blob shader stages also retain their owning input account. The helper audit in the binding README lists
every Y6b byte-consuming entry, guard route and small-budget boundary test;
five new oracle groups replay successful tree, reference, decryptor and shader
behavior.
The 250 API fixtures and 90 object observations replay in 42 Rust binding tests;
the threat model in `crates/unityrs-py/README.md` applies unchanged.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT licence](LICENSE-MIT) at your option. Code ported from other projects keeps
its original notices. [`THIRD_PARTY_LICENSES`](THIRD_PARTY_LICENSES) collects
them, with the licences of the crates unityrs builds on.

Crunch Library Copyright (c) 2010-2016 Richard Geldreich, Jr.

unityrs is not affiliated with or endorsed by Unity Technologies. "Unity" is a
trademark of Unity Technologies.

The glTF material collector converts saved sRGB colours to linear factors and
textures to each slot's encoding, unpacks Unity normal maps (+Y), repacks
metallic/gloss and occlusion channels, preserves sampler settings and base ST,
and represents HDR emission with emissive strength. See the
[material mapping rules](crates/unityrs-gltf/README.md#materials) for supported
shaders, counted omissions, colour spaces and resource-refusal boundaries.
