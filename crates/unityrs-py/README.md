# unityrs Python bindings

The bindings provide local-path and byte loading, eager files and object discovery,
`ObjectReader` dict reads and raw bytes. Y2 adds object mode and runtime classes.
Y3 adds mutable `TypeTreeNode` helpers,
explicit-node dict reads, `peek_name`, `ContainerHelper`, mutable loader config,
`UnityVersion` and the handwritten enums and lookup tables. It uses PyO3 0.29.3's Python 3.10+
abi3 interface and maturin 1.9.6. Y4 adds current-value images, samples, class
exports, texture editing and object/file/environment saves. PyO3's licence is already
accepted by the repository. The wheel includes `THIRD_PARTY_LICENSES`.

From the repository root, with maturin installed in a build venv:

```sh
maturin build --locked --release --manifest-path crates/unityrs-py/Cargo.toml --out /tmp/unityrs-wheels
python -m pip install --no-deps /tmp/unityrs-wheels/*.whl
python scripts/unittest-suites crates/unityrs-py/python/tests=61
```

Build a Linux wheel in the oracle container to test there; a macOS wheel cannot
be installed in Linux. In an oracle venv with UnityPy 1.25.2 available, install
the wheel, then run:

```sh
python tools/fixtures/py_bindings.py --check
cd tools/parity
python ../../scripts/unittest-suites bindings=157
```

The recorder writes 111 synthetic API records (34 Y1 records, 10 Y3 groups and
67 Y4 groups),
including exception text, all enum member names and values, version edge cases
and mutable helper behavior, plus 90 Y2 object-mode observations. Object
recordings select only fields from the synthetic input, never generated defaults
or a TPK schema.
`cargo test -p unityrs-py`
replays them through the Python facade. The object replay needs a TPK; it
discovers the oracle installation without importing UnityPy. The binding suite
compares the installed wheel directly with UnityPy and fails if either import
is missing. The ordinary oracle suite does not discover this separate suite.
The Y3 replay is `replays_unitypy_helper_observations`; its installed-wheel suite
also compares runtime common strings directly, without saving TPK-derived data.
The wheel suite has 61 tests and the oracle comparison has 157; CI enforces both
minimum counts. Warning handlers run on the calling Python thread before a
fallback load is published, including handlers that re-enter the environment.

```python
import unityrs

budget = unityrs.Budget(256 * 1024 * 1024)
limits = unityrs.Limits(max_input_bytes=64 * 1024 * 1024)
env = unityrs.Environment("input.assets", budget=budget, limits=limits)
for obj in env.objects:
    value = obj.parse_as_dict()
```

`Budget.used` is a Rust atomic counter. Limits and budgets apply to retained
Rust storage and the binding's conversion staging driven by asset files, streams
and bundles. Existing Python input buffers are retained without a Rust copy,
and Python owns returned values after handoff. Caller-constructed objects and
third-party libraries' work are outside this budget: this includes protocol
overrides such as `str` subclasses with lying `__len__` or custom `__add__`,
unsized or infinite `subset` iterables, caller-positioned writers (for example,
`Position = 8_000_000`), and Pillow's `Image.open()` and decoding of a caller's
image file passed to `set_image` (Pillow's own limits apply). Regardless of the
caller input, the binding must remain memory-safe and must never panic, abort
or deadlock. Every core operation releases the GIL; parser work runs on a scoped
worker using the core's
advertised value stack allowance plus container and tree parsing frames,
including when a Python thread has a small stack. Native container destruction
is iterative on the releasing thread, including wrapper failure cleanup.
The already charged entry tables form its work list: teardown allocates no
storage and creates no workers, even with a full budget or refused allocator.
Conversion uses an explicit metered work stack. Files and objects retain strong
parent links, and a weak wrapper cache preserves identity without retaining
replaced files forever. An environment lock serializes loads and wrapper
publication. Stream reads accumulate bounded chunks up to `max_input_bytes + 1` bytes, restore
the original position, and refuse oversized input before parsing. ZIP members
are decoded by the metered Rust loader for paths, bytes and streams.

Type-tree helpers use one charged traversal policy for parsing, list conversion,
graph walks and output. Helpers accept optional `limits=` and `budget=`; nodes
reconstructed from an object inherit its input's meter and depth limit. Every
repeated edge spends work and output memory again, cycles raise `ValueError`,
and staging charges stay live through reconstruction or output handoff. Lazy
node lookup and publication use the environment's reentrant lock.

No core call may be in flight in another thread at `fork()`. Use `spawn` or
`forkserver`, or fork before reading. There is no child-side lock reset.

Pillow is an optional runtime import for image access and editing, never a build
dependency. These calls propagate its import error when Pillow is unavailable.
Classes use the MIT-licensed attrs dependency for UnityPy-compatible slots,
constructors, equality and hashing. The bindings do not require UnityPy and
bundle no TPK or generated class data. Nonlocal
filesystems remain outside this local-input API.

Importing `unityrs.classes` generates records in memory. Set `UNITYRS_TPK` to a
TPK file path before importing it. The `unityrs.config` module's
`TPK` attribute accepts a path or bytes and takes precedence. Without an
explicit source the loader discovers the installed UnityPy resource without
importing UnityPy. If none is available, importing classes raises `ImportError`
with those source options; ordinary dict reading remains available.
Stripped-file schemas and the complete common-string table use an external TPK.
With no TPK available, embedded trees and explicit nodes remain usable for dict
reads, and common-string lookup uses the 108-entry MIT table. No TPK-derived
data is saved in the wheel or replay fixtures.
Uncompressed, LZ4 and Unity raw-LZMA TPKs are accepted. Generator inputs,
decoded payloads and decoder dictionaries are bounded at 256 MiB, tables at
four million records (class entries in aggregate). Invalid generated syntax
is rejected before execution. Python allocation failures raise `MemoryError`.

`reader.read()`, `reader.parse_as_object()` and
`reader.read_typetree(as_dict=False)` return objects; UnityPy's `wrap=True`
spelling is also supported. PPtrs retain their assets file and resolve through
its parent files and environment. Unknown records retain their node metadata;
`helpers.TypeTreeNode` supplies the metadata for `UnknownObject.get_type()`.
The core still parses detached from the GIL. Python builds objects with an
explicit work stack from the metered conversion, whose returned objects belong
to Python's allocator as in dict mode.

Generation follows HEAD tools/TpkClassGenerator.py:132-306 and
HEAD tools/TpkClassGenerator.py:309-367, preserved and blob-pinned under
`tools/parity/tests/head_417d595/`. Each release root contributes fields to its
named class; nested records merge by type name, node IDs deduplicate definitions,
field types form sorted unions, and the intersection of fields across shapes
is required. Other fields use `Optional[...] = None`. Required fields precede
optional ones, each sorted by its cleaned name. Bases follow class metadata,
classes without release roots add ABC, dependency order puts bases first, and
math records replace their TPK definitions. The decorator temporarily replaces
nontrivial bases when applying attrs (classes/generated.py:26-44).

The binding suite compares the entire generated class set, annotations,
field order, defaults, slots and class-ID map against 1.25.2 at test time.
It also checks every generated class with complete, missing and extra fields,
pointer resolution, explicit TPK sources, and malformed input. With the oracle's
TPK the HEAD algorithm reproduces the installed 1.25.2 introspection exactly;
no class/field list or ID map is persisted. The class map first uses the MIT
enum spelling, then current TPK metadata and legacy spelling normalization.

Object construction follows UnityPyBoost/TypeTreeHelper.cpp:458-583; zero-byte
objects follow helpers/TypeTreeHelper.py:233-258 and
helpers/TypeTreeHelper.py:314-357. Ports of Object, PPtr, UnknownObject and math
retain the original MIT notices via `LICENSE-UNITYPY` and `THIRD_PARTY_LICENSES`.

Y4's `replays_unitypy_export_observations` replays recorded class API results.
The installed-wheel comparisons cover current field edits, streamed resources,
sprite atlases and caches, 256 texture decode cases, raw target promotions and
filtered mip chains, shader fixtures, PCM/IMA/FADPCM banks, and save packers. Observations
never store generated class fields or defaults.

| behavior | UnityPy 1.25.2 source | PARITY.md rows |
|---|---|---|
| `Texture2D.image`, `get_image_data`, setter and `set_image` | classes/legacy_patch/Texture2D.py:8-11, classes/legacy_patch/Texture2D.py:14-60, classes/legacy_patch/Texture2D.py:63-75 | 14.2, 15.1, 18 |
| `Sprite.image`, array images | classes/legacy_patch/Sprite.py:4-7, classes/legacy_patch/Texture2DArray.py:12-43; export/SpriteHelper.py:44-127 | 14.2, 15.2 |
| `AudioClip.samples`, `extension` | classes/legacy_patch/AudioClip.py:5-6, classes/legacy_patch/AudioClip.py:9-12 | 14.2, 16 |
| `Mesh.export`, `Renderer.export`, `Shader.export` | classes/legacy_patch/Mesh.py:4-7, classes/legacy_patch/Renderer.py:4-7, classes/legacy_patch/Shader.py:4-7 | 14.2, 15.4, 15.5 |
| inherited `Object.save`, including Font, TextAsset and MonoBehaviour | classes/Object.py:29-33; files/ObjectReader.py:264-276 | 8, 14.1, 18 |
| file, bundle, WebFile and environment save | files/SerializedFile.py:389-491; files/BundleFile.py:188-487; files/WebFile.py:58-112; environment.py:176-184 | 3, 18 |

Exports use current Python values. `set_image` changes the class fields;
`Object.save()` installs their serialization, and file/environment save writes
it. ObjectReader reads keep returning the original bytes until reload, as in
UnityPy. Writer endian, position and alignment are honored. Core calls release
the GIL and use the owning input's meter; class snapshots reject cycles and
bound depth, value count and work. Returned PIL images and sample dictionaries
belong to Python. The existing `encode` feature is enabled; no new third-party
dependency is introduced.

PCM, IMA ADPCM and FADPCM use the core decoders. MPEG/Opus and unsupported FMOD
containers raise `NotImplementedError`; FMOD is never included. For Vorbis,
set `unityrs.config.AUDIO_VORBIS_HEADERS` to runtime setup-table bytes or a path,
or set `UNITYRS_VORBIS_HEADERS` to a path. No table is bundled; without supplied
headers these banks raise `NotImplementedError`. Passthrough Ogg, RIFF and M4A retain their
bytes. Core Tier B/C/HEAD corrections remain in effect, including integer array
formats, BC7 encoding, CAB registration and later file layouts; oracle equality
claims apply to the recorded Tier A cases.
Mode-packer failures retain their original oracle results beside the Tier C
reference (UnityPy writing the RGBA-converted input). Switch mip chains retain
the core's named refusal before changing texture fields.
Zero-size texture images and empty streamed audio clips retain the core's
Tier B results, empty RGBA and an empty samples dict, before resource lookup;
their original oracle exceptions are recorded beside those references.

Y5 still owns the `unityrs.tools` extractor surface: `export_obj`,
`extract_assets`, `exportTextAsset`, `exportFont`, `exportMesh`, `exportShader`,
`exportMonoBehaviour`, `exportAudioClip`, `exportSprite`, `exportTexture2D`,
`exportGameObject`, `crawl_obj`, `flatten` and the extractor dispatch table.
Font, TextAsset, MonoBehaviour and VideoClip have no class `export()` in 1.25.2;
their file exporters belong to that tools surface. Y5 also owns the remaining
package/module re-exports enumerated in PARITY.md 2.1 and 23, including streams
and export modules, codec maps, the decryption-key entry point and module
aliases/loop variables. Its final binding rows include GameObject component
accessors, `File.container` and file-iteration corrections, `ObjectReader.patch`,
provider/generator hooks, and the unclosed loader/filesystem cases from Y1.
Y4 does not mark those rows verified.
