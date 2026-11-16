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
python scripts/unittest-suites crates/unityrs-py/python/tests=197
```

Build a Linux wheel in the oracle container to test there; a macOS wheel cannot
be installed in Linux. In an oracle venv with UnityPy 1.25.2 available, install
the wheel, then run:

```sh
python tools/fixtures/py_bindings.py --check
cd tools/parity
python ../../scripts/unittest-suites bindings=365
```

The recorder writes 250 synthetic API records (34 Y1 records, 10 Y3 groups and
70 Y4 groups, 52 tools/module/loader groups, 36 Y6a groups and 48 Y6b groups),
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
The wheel suite has 197 tests and the oracle comparison has 365; CI enforces both
minimum counts. Exception translation and object-value handoff run on the calling
thread after the native worker joins. Other worker sites relay Python callbacks,
allocations and finalizers to that caller. Warning handlers run on that thread
before a fallback load is published, including handlers that re-enter the environment.
The Rust binding suite has 42 tests, including guarded finalization and unwind
regressions and replay of the synthetic API observations.

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
Conversion uses an explicit metered work stack. Object reads hand a flat, metered
construction plan to the caller; recursive native values are destroyed on the
parser worker, including failed `check_read` results. Files and objects retain
strong parent links, and a weak wrapper cache preserves identity without retaining
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
bundle no TPK or generated class data. Caller-supplied filesystems follow the recorded loader rules, including its
local path checks. fsspec remains optional; a local fallback is available.

Importing `unityrs.classes` generates records in memory. Set `UNITYRS_TPK` to a
TPK file path before importing it. The `unityrs.config` module's
`TPK` attribute accepts a path or bytes and takes precedence. Without an
explicit source the loader discovers the installed UnityPy resource without
importing UnityPy. If none is available, importing classes raises `ImportError`
with those source options; ordinary dict reading remains available.
Stripped-file schemas and the complete common-string table use an external TPK.
With no TPK available, embedded trees and explicit nodes remain usable for dict
reads, and common-string lookup uses the 108-entry MIT table. No TPK-derived
data is saved in the wheel or replay fixtures. The complete class map also
replays an oracle recording held only in memory during the Rust test.
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

The tools surface exposes `export_obj`, `extract_assets`, `exportTextAsset`,
`exportFont`, `exportMesh`, `exportShader`, `exportMonoBehaviour`, `exportAudioClip`,
`exportSprite`, `exportTexture2D`, `exportGameObject`, `crawl_obj`, `flatten` and
`EXPORT_TYPES` through `unityrs.tools` and `unityrs.tools.extractor`.
The facade uses the existing bounded class exporters. E4's native extractor has
intentional corrections to the oracle's graph and naming behavior; the Python
facade retains the recorded oracle behavior while using confined destination
writes. All public tools paths and names, class export arguments, environment save
arguments and asset-derived output names become exact strings before use. The
hostile-subclass matrix compares exact-string results and the surrounding
filesystem, including empty directories. Audio sample names retain scalar
formatting for `None` and integers, checked against the oracle. All nine direct exporters anchor the
destination before formatting and validate appended leaf names and extensions
before creating output. Hostile names and
existing symlink directories are refused, including audio sample names and
GameObject mesh/material names. Destination opening follows filters and dispatch;
ignored or filtered objects do not require an existing destination. Shared file
readers start at the actual post-load cursor, including eager AssetBundle reads,
and the environment lock covers their construction and publication. The same
reentrant lock covers lookup, construction and publication of file containers,
ObjectReader readers and decoded sprite textures. Concurrent first container
access shares one mutable helper. Container extraction accepts equivalent roots
such as `out`, `out/`, `out//`, `./out//`, `./out` and an absolute directory without
weakening the descriptor walk. Passing any falsy extension (including `None`,
`False`, `0` and `""`), positionally or by keyword, preserves the defaults or
ignored argument in eight direct exporters; Sprite retains
Pillow's unknown-extension error. Effective output names remain validated.
Truthy scalar extensions retain each exporter's formatting or ignored-argument
handling, including integers, booleans, floats, complex numbers and bytes.
The resulting output leaf is validated before writing. `export_obj` converts
the destination after dispatch and the parsed-object filter, so unsupported or
filtered objects accept `fp=None` and return `[]` as in UnityPy.
`extract_assets` loads and sorts the source before converting a destination.
Empty inputs and rejected objects leave `dst=None` or an unused PathLike
untouched; source-loading errors precede destination errors. Non-container
extraction also waits for dispatch and the parsed-object filter. An empty
destination exports into the current directory, with relative-name validation
and held-directory confinement intact. These cases compare with UnityPy in
scratch directories and replay through `binding_y5_round7`.
Standalone `ImportHelper.parse_file` parses without loader registration;
container constructors retain their native child CAB registration, including
streamed-resource lookup through duplicate nested container entries.
Oracle comparisons check `Environment.files` and `cabs` before and after,
including replacement names and a later load. All three behaviors are recorded
and replayed through `binding_y5_round6`.
Y6a's recorded forced-parser cases verify memory-reader `read(None)`, BytesIO
`read(-2)`, and publication of earlier child CABs when a later container child
fails. Successful and partial constructors retain the reader kind, cursor and
endian; these close Y5's standalone parsing follow-ups in PARITY section 22.

Failed image exports retain the previous file and remove their temporary sibling.
This is a documented safety divergence: Pillow can leave a partial destination
in UnityPy (33 bytes for a zero-height RGBA PNG). The oracle observation and our
preserved file both replay. Standalone material exports resolve current texture
names through Y6a’s bounded adapter; textured and untextured materials replay.
Replaced `CONV_TABLE` converters similarly refuse Switch-swizzled
and crunch inputs with a named refusal until Y6b supplies their preprocessing.

| behavior | UnityPy 1.25.2 source | PARITY.md rows |
|---|---|---|
| tool dispatch and extraction | tools/extractor.py:25-78, tools/extractor.py:81-157 | 15.7 |
| individual file exporters | tools/extractor.py:165-174, tools/extractor.py:177-187, tools/extractor.py:190-197, tools/extractor.py:200-207, tools/extractor.py:210-224, tools/extractor.py:227-247, tools/extractor.py:250-268, tools/extractor.py:271-281, tools/extractor.py:284-304 | 15.7 |
| graph helpers and dispatch tables | tools/extractor.py:307-324, tools/extractor.py:327-358, tools/extractor.py:361-368 | 15.7, 23 |
| file container and reader patch | files/File.py:123-125; files/ObjectReader.py:316-322 | 5.2, 8 |
| MonoBehaviour generator hooks | files/ObjectReader.py:325-350 | 8, 10, 14.3 |
| custom filesystem and loading | environment.py:37-73, environment.py:75-98, environment.py:100-174, environment.py:282-308 | 3, 19.2 |
| streams and module re-exports | streams/__init__.py:1-2; streams/EndianBinaryReader.py:335-349 | 2.1, 13, 23 |

The tools observations replay through `replays_unitypy_tools_observations`.
Recordings contain synthetic values and output bytes, never runtime TPK schemas.
Array-map recordings call every alias with empty and nonempty arrays, both
endians and both reader kinds, recording values and final cursors.
Generated MonoBehaviour rows compile on the caller before starting a worker.
Worker output allocation, exception lookup and formatting, and shared traversal
access relay to that caller; queued Python decrefs drain there too. Reentrancy
regressions cover node getters, GC callbacks, resource-error callbacks, snapshots,
configuration callbacks and buffer finalizers under the deadlock guard.
BytesIO reader policy is installed before synchronization publishes its wrapper,
including when a callback creates the lazy reader during synchronization. A
reader already created by a nested load receives the same policy.
Primitive string maps dispatch through the owning reader's decoding guard;
budget regressions cover the one-byte refusal boundary and every tuple-producing array-map
alias. Negative string sizes reserve the remaining input span through decoding;
negative and None-size byte, string and array reads also compare with the oracle.
Byte reads and the whole-file `bytes` property retain their spans in every
enclosing array guard until handoff. Boundary regressions cover aggregate byte
arrays, raw spans, fixed, aligned and NUL-terminated strings, every typed-array
alias and nested byte/string arrays, including charge cleanup after refusal
and success. The 64-entry array of 65,536-byte spans refuses a 200,000-byte
budget. `binding_y5_round8` records that safety divergence beside UnityPy's
result and replays values and cursors with sufficient space on memory, BytesIO
and path readers.
Empty GameObject component lists return no components, and legacy
`(classID, PPtr)` entries return their actual pointers. These documented Tier B
corrections replay beside the original oracle exceptions. Global decryption
keys are converted on the calling Python thread only when an encrypted bundle
needs them; list-valued keys accepted by the setter leave ordinary loads usable.
The oracle comparisons also check truncated-envelope error ordering, encrypted
key rejection and a successful retry after installing a valid key. Key staging
shares the active input's memory and work limits; repeated attempts do not
accumulate key-conversion work across inputs. Installing runtime configuration
without an explicit key override preserves a key set on the core file context;
explicit runtime keys take precedence. The synthetic core regression and binding
key comparisons cover these settings.
Font, TextAsset, MonoBehaviour and VideoClip have no class `export()` in 1.25.2;
their file exporters belong to the tools surface.

Y6b implements the standalone helper APIs listed after PARITY.md 2.1, live
codec callbacks and mutable reader parsing. Their synthetic observations replay
through `replays_unitypy_internal_helper_observations`; namespace checks alone
do not establish their parity. Standalone Brotli uses the core's bounded
inflater and stored-metablock writer: decoded bytes replay against UnityPy;
compressed bytes retain the existing Tier B difference from its quality-11 writer.

The helper review adds seven recorded groups in `binding_y6b_review`. Schema
lookup failures preserve the original reader cursor; truncated stream objects
stop at EOF in dict and object mode, including failed parses and `check_read`.
Scalar string and TypelessData reads dispatch live primitive-map callbacks on
the caller while retaining bounded builtin reads and direct string-array rules.
An empty shader blob still decompresses declared platform entries. LZ4 validates
the original signed-i32 argument before header mode, rejects oversized embedded
sizes as `ValueError`, and requires the decoded length to match the header.
Shader constructors inherit the native asset reader's traversal guard and share
it with nested subprograms until construction returns. The wheel checks one-byte
refusal/success boundaries, charge cleanup and 20,000 keywords with only 4 KiB
remaining. Successful native-reader construction is oracle-compared and replayed.
Replacement readers returning memoryview strings decode through charged bytes
staging and align their cursor; scalar and array reads record and replay ASCII,
multibyte UTF-8 and surrogate escapes. A budget regression refuses before the
string buffer is read. Each group is oracle-compared, replayed in Rust and checked
by the wheel suite.

Y6a's `replays_unitypy_residue_observations` replays 36 additional groups.
They cover the loader edge cases, synthetic object-mode residue and Transform,
damaged AssetInfo preloads, current material texture names, raw float WAVs and
forced helper parsing, detached saves and mutations, constructor reader state
and compressed-reader failures. Original oracle failures remain beside each
corrected reference; non-class module attributes are probed in isolated oracle children.
The input limits, charged staging and dependency confinement above apply to
these adapters too. No dependency or bundled runtime schema is added.

| behavior | UnityPy 1.25.2 source | PARITY.md rows |
|---|---|---|
| loader path, key and reader observations | helpers/ImportHelper.py:159-173; environment.py:100-160, environment.py:310-354; classes/PPtr.py:43-90 | 3, 19.2 |
| referenced arrays and non-class names | helpers/TypeTreeHelper.py:182-263, helpers/TypeTreeHelper.py:266-361; UnityPyBoost/TypeTreeHelper.cpp:458-583 | 9.2, 19.2 |
| synthetic Transform reader | files/ObjectReader.py:186-187, files/ObjectReader.py:217-218, files/ObjectReader.py:244-262 | 14.3 |
| damaged preload tables and external lookup | helpers/ContainerHelper.py:31-49; classes/PPtr.py:43-90 | 19.3 |
| standalone material texture names and renderer PNG write order | export/MeshRendererExporter.py:48-103, export/MeshRendererExporter.py:106-189 | 15.4, 19.1 |
| optional PCM float conversion | export/AudioClipConverter.py:13-49 | 16 |
| forced file-type parsing | helpers/ImportHelper.py:127-156 | 22 |

Forced parsers accept the default big-endian, zero-position reader state.
A nonzero parser cursor, or a non-default endian for Assets/Bundle parsing,
raises a recorded `NotImplementedError` before parsing; this prevents silently
parsing a different reader state. Raw-type selections preserve the original
reader and cursor. PARITY.md 19.3 records both refusals beside the oracle results.


Y6b adds standalone tree reads and writes, the accelerated callable, an explicit
optional generator backend, decryptor/key helpers, shader converter stages and
program classes, sprite geometry, and ASTC context/cache adapters. Object reader
replacement, endian changes and failure cursors use the same bounded parsing
policy. Codec map callbacks and compressor finalizers run on the calling Python
thread outside native locks; a payload retains its selected compressor and chunk
size while it is packed. Callback buffers are charged before copying.

Standalone traversal checks runtime value depth separately from schema depth
and charged scheduler frames. Accelerated reference lookup stops at the first
matching descriptor and compiles only its node, with attribute access relayed
to the calling Python thread. Decryptor block indices wrap modulo 2**64.
Shader stages call only the selected current subprogram's `Export`, retaining
fallback comments for mutated unknown types and leaving unused programs untouched.
Explicit `zClip=None` survives staging and omits ZClip in generated shader states
and their enclosing passes. Pure reference reads and writes compare class,
namespace and assembly in order, skipping later attributes on a nonmatch.
Switch dispatch compares the blob exponent directly; the standalone GOB-count
helper charges integer storage before shifting. Native layout validation still
refuses oversized exponents. Recorded boundary calls, full-budget allocation
regressions and shader/reference observations cover these paths.

| behavior | UnityPy 1.25.2 source | PARITY.md rows |
|---|---|---|
| codec maps and standalone failures | helpers/CompressionHelper.py:17-155, helpers/CompressionHelper.py:158-258; files/BundleFile.py:489-521 | 6 |
| reader replacement, endian and failure cursor | files/ObjectReader.py:244-262, files/ObjectReader.py:278-283 | 8 |
| standalone tree read/write and accelerated callable | helpers/TypeTreeHelper.py:123-179; UnityPyBoost/TypeTreeHelper.cpp:962-1056 | 9.1, 23 |
| optional generator backend and local discovery | helpers/TypeTreeGenerator.py:1-78 | 10 |
| standalone decryptor, key and search helpers | helpers/ArchiveStorageManager.py:12-143 | 12 |
| patched platform dispatch and ASTC contexts | export/Texture2DConverter.py:254-320, export/Texture2DConverter.py:356-397 | 15.1 |
| sprite geometry and matrices | export/SpriteHelper.py:29-346 | 15.2 |
| shader stages and mutable programs | export/ShaderConverter.py:58-689 | 15.5 |
| explicit TPK update entry point | cli/update_tpk.py:1-27 | 2.1 |

`unityrs.cli.update_tpk()` and the `unityrs update_tpk` command fetch only when
called. Download and extracted TPK sizes are bounded at 256 MiB. The raw TPK goes
to the caller's cache (or explicit destination), and class generation remains in
memory. No generated source, schema or downloaded TPK is included in the wheel.
Tests use a deterministic local HTTP peer with synthetic ZIP content.
Publication uses the shared directory sink: a sibling temporary file replaces
`lzma.tpk` atomically, preserving an existing link's former target. Every
destination directory component is opened without following symlinks.

ASTC contexts are Python proxy objects with the oracle's thread/block cache
policy; their native operations support all fourteen legal 2D footprints with U8
RGBA storage. Other storage types and swizzles raise named `NotImplementedError`
exceptions. Standalone sprite operations retain the renderer's singular-matrix
correction and its RGB/RGBA sheet bound. PARITY.md 19.2 records the exact refusals
beside the original oracle results. The threat model above applies unchanged.

Successful forced constructors apply their final cursor and endian to the input
reader, including stream short reads of oversized members. Failed constructors
retain endian assignments and publish earlier successful child CABs before
re-raising; nested bounds failures use the child reader kind and buffer size.
Compressed WebFiles keep the outer reader at its constructor state;
short-read errors use the decoded reader's kind and size. Detached files and
descendants expose `environment=None` while retaining a private synchronization
owner for saves, object edits and lazy node publication. These cases replay
`binding_y6_review` and compare with the pinned oracle; exception lookup
reentrancy also runs under the Rust `DEADLOCK_GUARD`.

Explicit readers passed to `Environment.load_file` keep their identity and
receive the detection reset and final constructor cursor and endian: short raw
inputs keep their cursor, a 20-byte raw stream resets to zero, and the synthetic
TextAsset finishes at 281. Nested bundle-signature errors retain a charged copy
of the rejected child's signature instead of rereading the outer container.
Both observations are oracle-compared and replayed in `binding_y6_review`.

Detection probes an explicit reader at its original cursor before resetting it:
the synthetic WebFile positioned at byte 2 stays the original raw reader at zero.
Probes past EOF retain the original cursor and each reader kind's exception.
Forced WebFile parsing distinguishes `BytesIO` reads from buffered-file reads;
a member size of -2 reads to EOF only for `BytesIO`, retaining its final cursor
and child CAB. Negative legacy-bundle offsets follow `struct.unpack_from`'s
normalization and range messages. These three observations are oracle-compared
and replayed in `binding_y6_review`; native bounds tests also compare directly
with `struct.unpack_from`. The Rust binding suite has 42 tests, including guarded
caller-thread finalization and callback-unwind regressions.

Renderer PNG writes preserve every `m_TexEnvs` entry in source order, including
duplicate property names and filename collisions; MTL references use the last
value for each property in first-key order. The duplicate `_MainTex` regression
keeps `old.png` and verifies the final pixels of `shared.png` against the
corrected 1.25.2 oracle, recorded and replayed in `binding_y6_exports`.

Y6b reader-meter audit: standalone tree reads select the supplied native reader
before an assets file or schema owner. An explicit `_guard` takes precedence;
explicit `limits=` or `budget=` retain the caller's opt-in policy. The selected
guard carries its meter and complete limits into accelerated parsing, node
compilation, reference callbacks and result construction. Python guard borrows
end before callbacks can re-enter it. With 32 KiB remaining, a detached schema
refuses a 20,000-element count after four bytes, before allocating the list.
Returned Python values still belong to Python after handoff, as specified above.

The following table lists the Y6b entry points and their account propagation.
"Caller values" means that the entry accepts no asset reader or owner: bytes,
images, matrices, fields or paths supplied after Python handoff have no retained
input provenance. These direct calls are outside the asset budget threat model;
their default native meters still enforce the documented core limits. They do
not infer an input account from a Python buffer. Calls from an asset adapter
that supply a reader or guard are listed separately.

| entry points | meter and route to the reader guard | boundary evidence |
|---|---|---|
| `TypeTreeHelper.read_typetree`, `read_value`, `read_value_array` | `_source_guard`: explicit guard, then native reader, native assets file, schema `_tree_source`, or explicit/default policy; `_ReadGuard` charges traversal, counts, values and primitive staging through handoff | `test_y6b_meter.py`: detached count, schema/file matrix, reader depth limits and a different schema owner |
| `read_typetree_boost` | Same assets-file/schema policy for direct calls; `read_typetree` passes its reader guard. `standalone_tree_read` copies the guard's meter and limits before compiling rows or parsing, and charges native buffers and construction. Fully detached bytes/schema without an owner are caller values | Read matrix checks every owned/detached schema and present/absent assets-file combination; each reachable combination checks its one-byte refusal/success edge and charge cleanup |
| `write_typetree`, `write_value` | Explicit guard, native assets file, schema owner, or explicit/default policy; detached schema with no assets file receives only caller values and writer. Writer position is caller-controlled. Traversal/output allowance stays charged until return | Write matrix checks both functions with all four schema/file combinations; the fully detached case explicitly supplies a budget |
| `get_ref_type_node` | `_source_guard` inherits the native assets file, including direct calls with no schema; reads/writes pass their existing traversal guard | Direct lookup and pure/read-value/accelerated/write/write-value reference boundaries in `test_y6b_entry_meters.py`; accelerated callbacks re-enter the same guard |
| Compression `decompress_lzma`, `compress_lzma`, `decompress_lz4`, `compress_lz4`, `decompress_brotli`, `compress_brotli`, `decompress_gzip`, `compress_gzip`, `decompress_lzham`, `chunk_based_compress` | Direct APIs receive caller byte buffers; native codecs use a default meter, gzip uses the caller's library, and LZHAM is a named refusal. They accept no asset reader | Outside the asset threat model as direct calls; codec failures and bytes are oracle-compared in `binding_y6b_codecs` |
| Asset loading/saving through `DECOMPRESSION_MAP`, `COMPRESSION_MAP`, `COMPRESSION_CHUNK_SIZE_MAP` | Builtins dispatch to core codecs with the input/output meter; `codec_bridge` carries that meter to the caller and charges callback arguments, result snapshots and copied output. Caller callback internals are outside the budget | `test_codec_loader_and_save_entry_boundaries` checks builtins and replacements; `test_y6b_codecs.py` checks callback block limits and snapshot refusal. Existing codec observations compare and replay callbacks and save output |
| `ArchiveStorageManager.read_vector`, `ArchiveStorageDecryptor.__init__` | Native reader `read_bytes`/`read_u_int` use `serialized_read` on the owning file meter. Each read's copy charge ends at Python handoff. The decryptor also passes its reader traversal guard into both `cn_key` calls, keeping AES snapshots, native output, decode work and output handoff on the owner meter. Reads, nibble expansion and tables are fixed size | Fixed-span and constructor boundary tests in `test_y6b_entry_meters.py` check cleanup and refuse AES staging with 288 bytes remaining after the header fits; `observe_decryptor_asset_meter` compares and replays memory/stream reader fields and cursors |
| `set_assetbundle_decrypt_key`, `decrypt_key`, `brute_force_key`, `ArchiveStorageDecryptor.decrypt_block`, `decrypt_byte`, `decrypt` | Direct key/data buffers, mutable views, indices or an explicit search path are caller values. Native key/block operations and bounded key-file search use default meters. Bundle decryption instead uses the core input meter and deferred key staging | Direct calls are outside the asset threat model; bundle-key budget/retry regressions and `binding_y6b_crypto_shader` cover the asset path and direct parity |
| `SpriteHelper.get_image_from_sprite`, `get_image`, `mask_sprite`, `render_sprite_mesh` | `_images._guard(sprite)` gets `object_reader._native.tree_traversal()`; texture reads use their own readers; `_sprite_render`/`_sprite_geometry` receive the sprite reader and use its meter. Python geometry/image staging retains the guard through handoff | Sprite sheet/image and both geometry entry boundaries in `test_y6b_entry_meters.py`; `test_y6b_images.py` checks cleanup and full-budget geometry refusal |
| `SpriteSettings`, `copy_triangle`, `transpose_matrix`, `get_matrix_minor`, `get_matrix_determinant`, `get_matrix_inverse`, `linalg_solve` | Scalar settings, PIL images, triangles and matrices are caller values; allocating standalone geometry/matrix operations use default guards and bounded native helpers | Outside the asset threat model as direct calls; geometry/matrix parity, singular refusal and factorial work limits have existing tests |
| `ShaderConverter.export_shader`, `ConvertSerializedShader` | `_images._guard(shader)` and native exporter context inherit `object_reader`. Empty-blob conversion now passes that guard through `_stage`, `_StageOutput` and native `shader_stage`, including nested properties, tags and state | Export/conversion entry boundaries plus `test_empty_shader_conversion_retains_owner_guard_for_nested_stages` check 32 KiB against both a long name and 1,000 properties |
| `ShaderProgram.__init__`, `ShaderSubProgram.__init__` | Native reader traversal guard, shared by nested subprograms until construction returns; retained keyword/list/code staging charges the same meter | Constructor one-byte boundaries, cleanup and 20,000-keyword refusal in `test_y6b_review.py` and constructor entry boundaries |
| `ConvertSerializedShaderParsedForm`, `ConvertSerializedSubShader`, `ConvertSerializedPass`, `ConvertSerializedSubPrograms`, `ConvertSerializedShaderState`, `ConvertSerializedShaderRTBlendState`, `ConvertSerializedTagMap`, `ConvertSerializedProperties`, `ConvertSerializedProperty`, `CheckGpuProgramUsable`, `GetPlatformString`, `ShaderProgram.Export`, `ShaderSubProgram.Export` | Standalone fields/programs/strings are caller values; default stage guards/native meters. Their internal calls from `ConvertSerializedShader` retain its owner guard as described above. Only selected current program exports are invoked | Direct stage/program parity replays `binding_y6b_crypto_shader`; attached empty-blob stages replay `binding_y6b_meter` |
| `Texture2DConverter.get_image_from_texture2d` and asset dispatch/preprocessing | `_images._guard(texture)` reaches the native object reader; both default decoding and replaced `CONV_TABLE` dispatch pass guard and reader to `_parse_image_data`, `_decode`, `_image_decode` or `_texture_preprocess`. Switch/crunch storage uses that meter before the selected callback; its buffers remain charged through handoff | Default/replaced converter entry boundaries, full-owner-budget Switch predicate regressions and patched platform oracle observations |
| `parse_image_data`, `pillow`, `atc`, `astc`, `pvrtc`, `etc`, `eac`, `half`, `rg`, `rgb9e5float`, `image_to_texture2d`, `compress_etcpak`, `compress_astc`, `swap_bytes_for_xbox`, `calculate_astc_compressed_size`, `pad_image`, `get_astc_context` and its compress/decompress proxies | Direct APIs receive caller buffers, images, dimensions and block sizes; no reader. Native implementations/default guards bound their own work. Asset preprocessing occurs under the preceding owner's guard before invoking a replaced converter | Outside the asset threat model as direct calls; ASTC/cache, conversion and platform observations and bounds tests remain in effect |
| TextureSwizzler `ceil_divide`, `deswizzle`, `swizzle`, `get_padded_texture_size`, `get_switch_gobs_per_block`, `is_switch_swizzled` | Direct buffers/dimensions are caller values. Asset dispatch compares the exponent without expanding an integer, then uses native checked layout/preprocessing on its input meter | Existing exponent/full-budget/allocation regressions in `test_y6b_images.py` |
| Optional generator discovery/backend and `cli.update_tpk` | Explicit caller paths/backend or explicit bounded download; no asset-reader bytes or inherited asset guard | Outside the asset threat model; bounded synthetic backend/download and confined publication tests remain in effect |

Five `binding_y6b_meter` observation groups compare and replay successful tree
schema/file combinations and 20,000-element reads, direct native-file reference
lookup, empty-blob shader conversion, and decryptor construction from native
memory/stream readers. The budget refusals above are safety
boundaries; UnityPy has no corresponding input budget.
