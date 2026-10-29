# UnityPy parity inventory

**Status:** P0, 2026-10-28. **Oracle:** UnityPy 1.25.2 with UnityPyBoost, in the oracle image (`DESIGN.md` section 3).

This is the inventory `DESIGN.md` section 3 asks for. It maps every public UnityPy API and behaviour to a tier, a verification level and a test, and gives a per-`TextureFormat` matrix. "Full parity" means every row here is green. It was built by reading UnityPy 1.25.2 as installed in the oracle image, and the claims about what UnityPy does with a texture format come from running it there (section 17).

`tools/parity/inventory.py check` fails when a UnityPy module, class, module-level function, API method, UnityPyBoost export or `TextureFormat` is missing from this file (section 21).

## 0. How to read the tables

**Citations.** `file.py:N` is a line of UnityPy 1.25.2, relative to the `UnityPy/` package. `HEAD file.py:N` is UnityPy git `417d595`, the HEAD oracle. Where a table says "lines are in X", a bare `:N` is a line of X.

**Tier** (`DESIGN.md` section 3):

| tier | meaning |
|---|---|
| A | bit-exact with UnityPy wherever UnityPy is right |
| B | UnityPy raises; unityrs produces correct output, checked against an independent reference |
| C | UnityPy is wrong; unityrs produces correct output, with a pinned divergence test (section 19) |
| Tol | compared within a stated metric and threshold (Sprite affine path, audio against FMOD) |
| HEAD | equal to UnityPy git `417d595` on Unity 6000.4/6000.5 fixtures (section 20) |
| U | unsupported: UnityPy raises and unityrs raises too, until a sample and a reference exist |
| gap | a deliberate `DESIGN.md` gap: UnityPy produces output (through FMOD) and unityrs reports the input unsupported (section 16) |
| proposed | a defect found while writing this file and not yet in `DESIGN.md` (section 19.2) |
| A, C | the item has parts in each tier; the row's notes and section 19 say which |
| — | not a behaviour of its own: a Python helper whose effect the named row covers, or Python-only tooling with no unityrs counterpart. The Python bindings (P3) decide whether to mirror it. |

**Level** is a verification level from `DESIGN.md` section 6: L0 detection, L1 container metadata, L2 typetree dicts, L2b TPK nodes, L3 object re-save, L4 file save, L5 pixels, mode and visible hash, L5b PNG bytes, L6 mesh numbers and OBJ text, L7 text exports, L8 audio, L9 extractor tree, L10 consumer shadow runs, L11 3D TRS. **L1 (env)** marks metadata at Environment scope (container paths after the preload pass, cross-file PPtrs): the canonical dump's format 1 leaves it out (`CANONICAL_DUMP.md` section 8), so it needs a format bump. **API** marks a Python-binding behaviour (a return type, an exception, a side effect) with no output of its own; it is tested through `unityrs-py` in P3.

**Status:**
- **verified P0 (Ln):** the oracle (`tools/parity/unitypy_dump.py`) records UnityPy's behaviour at level Ln in the canonical dump, `compare.py` diffs it, and the named self-test pins the oracle's side. The rabex spike, the candidate for the base layer, matched it on every record of the corpus game and UnityPy's samples with its one-hunk header fix (`docs/spikes/rabex.md` section 3). The production crate lands in the phase given after the semicolon.
- **planned (Pn):** nothing checks it yet; it lands in phase Pn of `DESIGN.md` section 5.
- **n/a:** tier —.

**Test** names a self-test in `tools/parity/tests/` (module and test without its `test_` prefix), or says what the planned test compares.

## 1. The oracle stack

The pins are `DESIGN.md` section 3's. The image also carries these libraries, whose versions decide UnityPy's behaviour on the rows named (from `pip list` in `unityrs-dev`, 2026-10-28):

| library | version | decides |
|---|---|---|
| lz4 | 4.4.4 | LZ4/LZ4HC blocks (section 6), `CompressionHelper.compress_lz4` bytes |
| brotli | 1.2.0 | brotli WebFiles |
| pycryptodome | 3.23.0 | UnityCN key decryption (`ArchiveStorageManager.decrypt_key`) |
| numpy | 2.4.6 | the Sprite affine solve (`SpriteHelper.copy_triangle`), FMOD float-to-int16 conversion |
| pyfmodex | 0.7.2 | FMOD bindings under fmod_toolkit 0.1.3, which ships its own `libfmod` |
| fsspec | 2025.5.1 | `Environment(fs=...)` |
| attrs | 26.1.0 | the generated classes and `ObjectReader` |

TypeTreeGeneratorAPI is not installed, so `TypeTreeGenerator` raises `ImportError` in the oracle (section 10).

## 2. Package and entry points

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `UnityPy.load` | alias of `Environment` (`__init__.py:8`) | A | API | planned: binding | planned (P3) |
| `UnityPy.AssetsManager` | backward-compatible alias of `Environment` (`__init__.py:11`) | A | API | planned: binding | planned (P3) |
| `UnityPy.__version__` | `"1.25.2"` (`__init__.py:1`) | — | — | — | n/a |
| `UnityPy.set_assetbundle_decrypt_key` | re-export of `ArchiveStorageManager.set_assetbundle_decrypt_key` (`__init__.py:4-6`) | A | API | planned: binding | planned (P3) |
| `python -m UnityPy` | runs `cli.main` (`__main__.py:1-4`) | — | — | — | n/a |
| `cli.main` | argparse front end with one sub-command, `update_tpk` (`cli/__init__.py:6-24`) | — | — | — | n/a |
| `update_tpk.update_tpk` | downloads AssetRipper's TPK over HTTP and regenerates `classes/generated.py` (`cli/update_tpk.py:12-24`). unityrs loads a TPK from a path instead (`DESIGN.md` section 4, `unityrs-tpk`). | — | — | — | n/a |
| package re-exports | `files/__init__.py`, `helpers/__init__.py`, `streams/__init__.py`, `export/__init__.py`, `enums/__init__.py`, `classes/__init__.py`, `classes/legacy_patch/__init__.py`; `export/__init__.py` leaves `ShaderConverter` out of `__all__` | — | — | — | n/a |
| empty packages | `resources/__init__.py` (holds `lzma.tpk`), `tools/__init__.py` | — | — | — | n/a |

## 3. Environment and loading

Lines are in `environment.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Environment` constructor | each argument: a path that is a file with extension `.apk`/`.zip` (case-sensitive) goes to `load_zip_file`; another file sets `path` to its directory and loads it (a `.splitN` name through `load_files`); a directory sets `path` and loads it recursively; a string that is neither is ignored without an error; any other argument loads as a stream (`:54-70`). `path` defaults to the cwd for a local fs. | A | L0 | planned: loader suite | planned (P1) |
| `Environment.file` | set only when exactly one file loaded (`:72-73`) | A | API | planned: binding | planned (P3) |
| `Environment.fs` | any fsspec filesystem (`:40`); `load_file` still tests local paths with `os.path` (`:119-128`) | A | API | planned: binding | planned (P3) |
| `Environment.load_files` | `load_assets` with `open` (`:75-77`) | A | L0 | planned: loader suite | planned (P1) |
| `Environment.load_folder` | every file under the folder, through `fs.walk` order (`:79-81`) | A | L0 | planned: loader suite | planned (P1) |
| `Environment.load` | loads each existing path and keys `files` by basename (`:83-87`); `load_file` has already keyed it by the full path, so each file sits in `files` twice and `Environment.objects` lists its objects twice | A | L0 | planned: loader suite | planned (P1) |
| `Environment.load_file` | a `.splitN` path merges its parts (`:111-116`); a relative path resolves against `path`, then `{file}.split0`, then a case-insensitive walk (`find_sensitive_path`); a miss returns `None` silently (`:117-133`). Detects, parses, keys `files` by the name as given (else the stream's `name`, else its hash), and registers a SerializedFile or raw reader as a CAB under its simplified name (`:139-160`). | A | L0 | planned: loader suite | planned (P1) |
| `Environment.load_zip_file` | opens a zip/apk and loads every member through `load_assets` (`:162-174`); raises `TypeError` on other inputs | A | L0 | planned: zip fixture | planned (P3) |
| `Environment.load_assets` | merges `.splitN` members once per base path, loads the rest by name (`:282-308`) | A | L0 | planned: split fixture | planned (P3) |
| `.splitN` merging | `split0`, `split1`, … concatenated up to the first missing part after `split0` (a missing `split0` is skipped, not a stop), at most 999 parts (`:89-98`); the regex `environment.reSplit`, `(.*?([^\/\\]+?))\.split\d+` (`:25`) | A | L0 | planned: split fixture | planned (P3) |
| `Environment.save` | writes each file with `is_changed` to `out_path/basename`, through its `save(packer=pack)` (`:176-184`) | A | L4 | planned: save suite | planned (P3) |
| `Environment.objects` | every object of every non-dependency SerializedFile, depth-first in `files` order (`:186-206`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.container` | runs every CAB's preload pass once (`_build_container_index`, `:208-215`), then joins the containers of non-dependency SerializedFiles (`:217-226`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.assets` | non-dependency SerializedFiles, depth-first (`:228-247`) | A | API | planned: binding | planned (P3) |
| `Environment.get` | `getattr` with a default (`:249-250`) | — | API | — | n/a |
| `Environment.register_cab` | keys `cabs` by `simplify_name` and invalidates the container index (`:252-264`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.get_cab` | case-insensitive basename lookup (`:266-280`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.find_file` | CAB registry, then `path/name`, then a walk of `path` built once, matched by exact path, then by simplified name (dead: it compares a bare name with the walk's full paths, `:338`, `:342`), then by plain suffix with no separator boundary, first on the paths and then on the simplified paths; raises `FileNotFoundError` (`:310-354`) | A | L1 (env) | planned: env dump | planned (P1) |
| `environment.simplify_name` | `ntpath.basename(name).lower()`; the docstring says it drops the extension, and it does not (`:357-363`) | A | L1 (env) | planned: env dump | planned (P1) |
| `Environment.typetree_generator` | `None` by default; `ObjectReader.generate_monobehaviour_node` reads it (`:34`) | A | L2 | planned: provider suite | planned (P3) |

## 4. File types and detection

Lines are in `helpers/ImportHelper.py`. `CANONICAL_DUMP.md` section 4.2 (`l0`) is the full rule list.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `FileType` | `AssetsFile` 0, `BundleFile` 1, `WebFile` 2, `ResourceFile` 9, `ZIP` 10 (`enums/FileType.py:4-9`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| `ImportHelper.check_file_type` | under 20 bytes `ResourceFile`; signature to NUL within 20 bytes; bundle, web and `PK\x03\x04` signatures; under 128 bytes `ResourceFile`; gzip and brotli WebFiles; else a big-endian SerializedFile header sanity check (`:44-124`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| eight-`0xFA` signature | compared as eight U+00FA characters, so the raw bytes never match (`:63`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| zip detection | only when the fifth byte is NUL (`:69`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P3 |
| gzip WebFile detection | decompresses the whole stream; a bad stream raises (`:77-83`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P3 |
| `ImportHelper.parse_file` | builds `SerializedFile`/`BundleFile`/`WebFile` by type; an `AssetsFile` named `.resS`, `.resource`, `.config`, `.xml` or `.dat` stays a raw reader; a failure prints and re-raises (`:127-156`) | A | L0 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L0); impl P1 |
| `ImportHelper.find_sensitive_path` | case-insensitive path resolution, one `os.listdir` per part (`:159-173`) | A | API | planned: loader suite | planned (P1) |
| `ImportHelper.file_name_without_extension` | path without its extension (`:16-17`); unused inside UnityPy | — | — | — | n/a |
| `ImportHelper.list_all_files` | `os.walk` skipping `.git` (`:20-29`); unused inside UnityPy | — | — | — | n/a |
| `ImportHelper.find_all_files` | `list_all_files` filtered by substring (`:32-41`); unused inside UnityPy | — | — | — | n/a |

## 5. Containers

### 5.1 BundleFile

Lines are in `files/BundleFile.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `BundleFile` header | signature, u32 format, player and engine strings (`:36-39`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P1 |
| `UnityArchive` | raises `NotImplementedError` (`:41-42`) | U | L0 | planned: error record | planned (P1) |
| unknown signature | raises `NotImplementedError` (`:50-51`) | A | L0 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L0); impl P1 |
| `BundleFile.read_web_raw` | UnityWeb/UnityRaw other than format 6: hash and crc from format 4, level table, LZMA for UnityWeb with an 8-byte size prefix (`:55-94`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures (UnityRaw 5 and 7); UnityWeb: planned fixture | verified P0 (L1) for UnityRaw; impl P3 |
| UnityWeb/UnityRaw format 6 | read with the UnityFS layout plus one byte after the flags (`:44-45`, `:104-106`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P3 |
| `BundleFile.read_fs` | UnityFS 6-8: sizes and flags (`:96-102`), blocks info compressed by the flags' codec (`:139-141`), block table and directory (`:144-165`) | A | L1 | `test_dump.py` samples (format 6), corpus dump (format 8) | verified P0 (L1); impl P1 |
| `ArchiveFlags` vs `ArchiveFlagsOld` | old below 2020, and below 2020.3.34, 2021.3.2 and 2022.1.1 on those lines (`:113-121`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `BlocksInfoAtTheEnd` | blocks info read from the file's end, data from after the header (`:134-137`) | A | L1 | planned: fixture (none in the samples or the corpus) | planned (P1) |
| 16-byte alignment after the header | format ≥ 7, or a 2019 engine at 2019.4.15 or later (`:126-131`) | A | L1 | corpus dump (format 8); the 2019.4.15 format-6 case: planned fixture | verified P0 (L1) at format ≥ 7; impl P1 |
| `BlockInfoNeedPaddingAtStart` | 16-byte alignment before the data, new flags only (`:170-171`) | A | L1 | corpus dump (flags `0x243`) | verified P0 (L1); impl P1 |
| `BlockInfo`, `DirectoryInfoFS` | namedtuples for the block table and directory (`:13-14`) | — | L1 | — | n/a |
| `BundleFile.decompress_data` | codec from `flags & 0x3F`; the decryptor runs on compressed blocks with flag `0x100` (`:489-521`) | A | L1 | `test_dump.py` samples; decryptor: section 12 | verified P0 (L1); impl P1 |
| `BundleFile.parse_version` | `UnityVersion.from_str(version_engine)`, falling back to `config.FALLBACK_UNITY_VERSION` when it does not match or its major is 0 (`:523-536`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| entries | each directory entry is detected and parsed in turn; a failing entry fails the whole bundle (`files/File.py:76-90`) | A | L0 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L0); impl P1 |
| `BundleFile.save`, `BundleFile.save_fs`, `BundleFile.save_web_raw` | section 18 | | | | |

### 5.2 WebFile, File and resources

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `WebFile` | gzip by magic, brotli by `brotli` at 0x20, else plain (`files/WebFile.py:20-37`); signature must start `UnityWebData` or `TuanjieWebData` (`:40-43`); entry names decoded as strict UTF-8 (`:53`). The dump records it as `kind: web`. | A | L1 | planned: WebFile fixtures (gzip, brotli, plain) | planned (P3) |
| `WebFile.save` | section 18 | | | | |
| `File` | base of the three containers: `files` dict, `is_changed`, `cab_file` `CAB-UnityPy_Mod.resS`, `parent`, `environment`, `name` as basename (`files/File.py:16-39`) | A | API | planned: binding | planned (P3) |
| `DirectoryInfo` | `(path, offset, size)` namedtuple (`files/File.py:13`) | — | L1 | — | n/a |
| `File.read_files` | slices each entry, parses it, registers SerializedFiles and raw readers as CABs, copies directory flags onto the child (`files/File.py:76-90`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P1 |
| `File.get_assets` | SerializedFiles below the file, depth-first (`files/File.py:41-50`) | A | API | planned: binding | planned (P3) |
| `File.get_objects` | objects of every child; a raw-reader child is yielded itself (`files/File.py:65-74`) | A | API | planned: binding | planned (P3) |
| `File.get_filtered_objects` | `get_objects` filtered by `obj.type` (`files/File.py:52-63`) | A | API | planned: binding | planned (P3) |
| `File.container` | dict union of the children's containers (`files/File.py:123-125`) | A | L1 (env) | planned: env dump | planned (P1) |
| `File.get`, `File.keys`, `File.items`, `File.values` | dict-style access to `files` (`files/File.py:127-140`) | — | API | — | n/a |
| `File.mark_changed` | marks the file and every parent (`files/File.py:145-148`) | A | L4 | planned: save suite | planned (P3) |
| `File.get_writeable_cab` | section 18 | | | | |
| `ResourceReader.get_resource_data` | looks up the CAB by basename, then `{name}.resource`, `{name}.assets.resS`, `{name}.resS`; on a miss calls `SerializedFile.load_dependencies` with those names (which loads the file's externals as non-dependencies, section 7.2) and looks again; raises `FileNotFoundError` (`helpers/ResourceReader.py:10-32`) | A | L5 | planned: `.resS` texture fixture | planned (P1) |
| `.resS` read | `reader.Position = offset; read_bytes(size)` on the CAB (`helpers/ResourceReader.py:36-38`) | A | L5 | planned: `.resS` texture fixture | planned (P1) |

## 6. Compression codecs

Lines are in `helpers/CompressionHelper.py`. The bundle codec is `flags & 0x3F` as `CompressionFlags` (`enums/BundleFile.py:4-9`).

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| NONE (0) | identity (`:239`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P1 |
| LZMA (1) | 5-byte props, raw LZMA1, no size check: UnityFS ignores the stored size (`:17-44`, `:240`). See `docs/spikes/lzma.md` for truncated and over-long blocks. | A | L1 | `test_dump.py` samples; `docs/spikes/lzma.md` fixtures | verified P0 (L1); impl P1 |
| LZ4 (2), LZ4HC (3) | `lz4.block.decompress` to the stored size (`:83-94`, `:241-242`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| LZHAM (4) | raises `NotImplementedError` (`:234-235`, `:243`) | U | L1 | planned: error record | planned (P1) |
| other codec values | raise `ValueError` "Unknown compression" (`files/BundleFile.py:515-521`) | A | L1 | planned: error record | planned (P1) |
| gzip | `gzip.decompress` (`:133-142`); WebFile only. Detection already decompresses the whole stream (section 4). | A | L0, L1 | `test_dump.py` l0_detection_and_name_rules (L0); WebFile fixture (L1) | verified P0 (L0); impl P3 |
| brotli | `brotli.decompress` (`:109-118`); WebFile only | A | L1 | planned: brotli WebFile fixture | planned (P3) |
| `CompressionHelper.decompress_lzma` | `read_decompressed_size` skips an 8-byte size, for UnityWeb (`:43`) | A | L1 | `test_dump.py` samples (UnityFS); UnityWeb: planned fixture | verified P0 (L1) for UnityFS; impl P1 |
| `CompressionHelper.decompress_lz4` | as LZ4 above | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `CompressionHelper.decompress_brotli`, `CompressionHelper.decompress_gzip` | as above | A | L1 | as above | as above |
| `CompressionHelper.decompress_lzham` | as LZHAM above | U | L1 | planned: error record | planned (P1) |
| `CompressionHelper.compress_lzma` | props `0x5D`, dictionary 8 MiB, lc 3 lp 0 pb 2, BT4, nice_len 123, optional u64 size (`:47-79`) | A | L4 | planned: save suite | planned (P3) |
| `CompressionHelper.compress_lz4` | `lz4.block.compress` HC level 9, no size prefix (`:97-105`); used for LZ4 and LZ4HC | A | L4 | planned: save suite | planned (P3) |
| `CompressionHelper.compress_gzip`, `CompressionHelper.compress_brotli` | library defaults (`:121-155`): brotli at quality 11, and gzip at level 9 with the current time as the header's mtime, so two gzip saves of one file differ | A | L4 | planned: save suite (after decompression) | planned (P3) |
| `CompressionHelper.DECOMPRESSION_MAP`, `CompressionHelper.COMPRESSION_MAP`, `CompressionHelper.COMPRESSION_CHUNK_SIZE_MAP` | the codec tables by `CompressionFlags` (`:238-258`): LZ4HC reads and writes as LZ4 HC, chunks are 128 KiB for LZ4 and LZ4HC and `0xFFFFFFFF` otherwise. They are looked up on every block (`files/BundleFile.py:515-518`), so adding an entry is how a UnityPy user registers a custom codec; unityrs's codec registry is the counterpart (`DESIGN.md` section 4) | A | API | planned: codec-registry binding | planned (P3) |
| `CompressionHelper.GZIP_MAGIC`, `CompressionHelper.BROTLI_MAGIC` | `1f 8b` at offset 0 and `brotli` at `0x20`, the WebFile sniffs (`:12-13`, `files/WebFile.py:20-37`) | A | L1 | planned: WebFile fixtures | planned (P3) |
| `CompressionHelper.chunk_based_compress` | LZ4/LZ4HC in 128 KiB chunks, LZMA in one; a chunk that grows is stored with its codec bits cleared (`:158-231`, sizes `:253-258`) | A | L4 | planned: save suite | planned (P3) |

## 7. SerializedFile

### 7.1 Version gates

Every branch on `header.version`. Lines are in `files/SerializedFile.py` unless named. The canonical dump records each one at L1 (`CANONICAL_DUMP.md` section 4.2). P0 exercises format 17 (UnityPy's samples), format 22 (the corpus game) and the synthetic formats 15, 16 and 22 that `tests/synth.py` builds; a gate only those cannot reach is planned for P4's Editor-built fixtures, though the oracle already records it.

| version | gate | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|---|
| all | header | four big-endian u32: metadata size, file size, version, data offset (`:32-38`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| < 9 | endian byte | read at `file_size - metadata_size`, at the end (`:265-267`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 9 | endian, reserved | endian byte and 3 reserved bytes after the header (`:257-259`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 22 | large header | u32 metadata size, i64 file size, i64 data offset, i64 unknown (`:260-264`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 7 | version string | read and passed to `set_version` (`:271-273`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| < 7 | no version string | `unity_version` stays `"2.5.0f5"` (`:239`) and `version` is never set, so anything that reads it raises `AttributeError` | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 8 | target platform | i32 as `BuildTarget`, which keeps unknown values (`:275-277`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 13 | `enable_type_tree` | bool (`:279-280`); below 13 every type carries a tree | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 16 | type `is_stripped_type` | bool (`:122-123`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 17 | type `script_type_index` | i16 (`:125-126`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 13 | `script_id`, `old_type_hash` | 16-byte script id for ref types with a script index, class id < 0 below 16, or class 114 from 16; then the 16-byte type hash (`:128-135`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| 10, ≥ 12 | blob typetree | `TypeTreeNode.parse_blob` (`:138-139`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| < 10, 11 | old typetree | `TypeTreeNode.parse`, field by field (`:140-141`) | A | L1 | planned: Editor fixture | planned (P4) |
| 2 | old node `variable_count` | read only at version 2 (`helpers/TypeTreeNode.py:98`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≠ 3 | old node `index`, `meta_flag` | absent at version 3 (`helpers/TypeTreeNode.py:99`, `helpers/TypeTreeNode.py:102`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 19 | blob `ref_type_hash` | node struct `hBBIIiii` gains `Q` (`helpers/TypeTreeNode.py:331-347`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| ≥ 21 | type dependencies, ref type names | plain types read an i32 array; ref types read class, namespace and assembly (`:143-149`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| 7-13 | `big_id_enabled` | i32 (`:286-288`) | A | L1 | planned: Editor fixture | planned (P4) |
| < 14 | path id | i32, or i64 with `big_id_enabled` (`files/ObjectReader.py:73-76`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 14 | path id | align 4, then i64 (`files/ObjectReader.py:77-79`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| ≥ 22 | byte start | i64, else u32; plus `data_offset` (`files/ObjectReader.py:81-86`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| < 16 | object type | the i32 type id is followed by a u16 class id; the type is the first whose class id equals the i32, and the object's class id is the u16 (`files/ObjectReader.py:89-97`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| ≥ 16 | object type | `types[type_id]` with Python indexing (`files/ObjectReader.py:98-101`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| < 11 | `is_destroyed` | u16 (`files/ObjectReader.py:106-107`) | A | L1 | planned: Editor fixture | planned (P4) |
| 11-16 | object script index | i16 written back into the shared type (`files/ObjectReader.py:109-112`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| 15, 16 | `is_stripped` | signed byte (`files/ObjectReader.py:115-116`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| ≥ 11 | script types | i32 count of `LocalSerializedObjectIdentifier` (`:298-300`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 14 | script identifier | align 4, then i64 (`:50-52`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| < 14 | script identifier | i32 (`:48-49`) | A | L1 | planned: Editor fixture | planned (P4) |
| ≥ 6 | external `temp_empty` | string (`:79-80`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 5 | external `guid`, `type` | 16 bytes and i32 (`:81-83`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 20 | `ref_types` | count and ref `SerializedType`s (`:306-308`); below 20 the attribute is never set | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| ≥ 5 | `userInformation` | string (`:313-314`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| 23-26 | HEAD only | section 20 | HEAD | L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| < 6, < 5, < 8, < 13 | absent fields | `temp_empty`, `guid`/`type`, platform and the tree flag keep their defaults (`:79-84`, `:240-241`) | A | L1 | planned: Editor fixture | planned (P4) |
| any | big-endian files | `header.endian` drives every read after the header (`:269`) | A | L1, L2 | `test_unitypy_boost.py` big_endian_reads_the_same_values (values only) | verified P0 (L2) for values; files planned (P4) |

### 7.2 Other SerializedFile behaviour

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `SerializedFile` | parses header, types, objects, scripts, externals, ref types and user information (`files/SerializedFile.py:235-324`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `SerializedFileHeader` | attrs class for the header (`files/SerializedFile.py:23-38`) | — | L1 | — | n/a |
| `LocalSerializedObjectIdentifier` | script type entry; `write` mirrors the read (`files/SerializedFile.py:41-60`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `FileIdentifier` | external entry; `name` is the path's basename (`files/SerializedFile.py:63-94`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `SerializedType` | type record (`files/SerializedFile.py:97-149`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `SerializedType.nodes` | alias of `node` (`files/SerializedFile.py:200-203`) | — | API | — | n/a |
| `SerializedType.write` | section 18 | | | | |
| object table collapse | `objects` is a dict by path id: a repeated id keeps its first position and last entry (`files/SerializedFile.py:291-295`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `SerializedFile.files` | alias of `objects` (`files/SerializedFile.py:225-233`) | — | API | — | n/a |
| `SerializedFile.set_version` | empty or `0.0.0` takes the parent bundle's `version_engine`, then the fallback (`files/SerializedFile.py:351-360`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| AssetBundle at load | the first AssetBundle object is parsed while loading and builds the container; a broken one fails the file (`files/SerializedFile.py:316-324`) | A | L1 | `test_dump.py` samples (container); the failing path: planned fixture | verified P0 (L1); impl P1 |
| `SerializedFile.container` | the file's `ContainerHelper` (`files/SerializedFile.py:326-328`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `config.SERIALIZED_FILE_PARSE_TYPETREE` | `False` turns `_enable_type_tree` off only after the types and ref types, trees included, were read (`files/SerializedFile.py:284`, `:308`, `:310-311`, `config.py:12`), so despite its docstring it saves no load time and reading is unchanged (`ObjectReader` still uses each type's tree, `files/ObjectReader.py:296-297`); its one effect is on `SerializedFile.save`, which then writes the tree flag as false and no trees (`files/SerializedFile.py:179`, `:405`) | A | L4 | planned: config suite | planned (P3) |
| `SerializedFile.load_dependencies` | loads every external by path, then the optional extra names, ignoring `FileNotFoundError` for the extras (`files/SerializedFile.py:330-349`); `load_file(path, True)` passes `True` as `parent`, not `is_dependency` (`environment.py:100-106`), so what it loads counts as a non-dependency and joins `Environment.objects` and `Environment.container`; section 19.2 | A | L1 (env) | planned: env dump | planned (P1) |
| `SerializedFile.read_string` | common-string lookup from `enums/CommonString.py`; never called (`files/SerializedFile.py:496-503`) | — | — | — | n/a |
| `SerializedFile._cache` | per-file image cache for sprites (`files/SerializedFile.py:246-250`); see Tier C, Sprite cache key | C | L5 | planned: divergence test | planned (P1) |
| `SerializedFile.get_writeable_cab`, `SerializedFile.save` | section 18 | | | | |

## 8. ObjectReader

Lines are in `files/ObjectReader.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `ObjectReader` | attrs record of one object table entry; `type` is a `ClassIDType`, extendable to unknown ids (`:34-50`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `ObjectReader.from_reader` | the object table row, per version (section 7.1; `:67-130`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `ObjectReader.version`, `ObjectReader.version2`, `ObjectReader.platform` | the file's packed engine version, its format version and its `BuildTarget` (`:52-62`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.parse_as_dict` | `read_typetree(wrap=False)` (`:313-314`): the dict model of `CANONICAL_DUMP.md` section 3 | A | L2 | `test_unitypy_boost.py` (all); `test_dump.py` samples | verified P0 (L2); impl P1 |
| `ObjectReader.parse_as_object` | `read_typetree(wrap=True)` (`:310-311`): object mode, section 9.2 | A | API | planned: object-mode suite | planned (P3) |
| `ObjectReader.read_typetree` | node from `nodes=` or the type or the TPK; reads from `byte_start`; UnityPyBoost when `byte_size > 0`; `check_read` raises `ValueError` on a size mismatch (`:244-262`, `helpers/TypeTreeHelper.py:146-156`) | A | L2 | `test_unitypy_boost.py` error_kinds, check_read_values | verified P0 (L2); impl P1 |
| `ObjectReader.read` | `read_typetree(wrap=True)` (`:217-218`) | A | API | planned: object-mode suite | planned (P3) |
| `nodes=` override | a `TypeTreeNode`, or a list of node dicts built with `TypeTreeNode.from_list`; anything else raises `ValueError` (`:285-294`) | A | L2 | planned: provider suite | planned (P3) |
| TPK fallback | no embedded tree: `Tpk.get_typetree_node(class_id, version)` (`:296-299`) | A | L2b | planned: stripped fixture | planned (P3) |
| no tree at all | `TypeTreeError` "There are no TypeTree nodes" (`:305-306`) | A | L2 | `test_unitypy_boost.py` error_kinds | verified P0 (L2); impl P1 |
| `ObjectReader.save_typetree` | writes the value with `write_typetree` in the file's endian, then `set_raw_data` (`:264-276`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |
| `ObjectReader.patch` | alias of `save_typetree` (`:316-322`) | A | L3 | as above | as above |
| `ObjectReader.set_raw_data` | replaces the data and marks the file changed (`:181-184`) | A | L4 | planned: save suite | planned (P3) |
| `ObjectReader.get_raw_data` | `byte_size` bytes from `byte_start`, position restored (`:278-283`) | A | L3 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L3); impl P1 |
| `ObjectReader.write` | the object table row for `save`; data from `set_raw_data` or the original bytes (`:132-179`) | A | L4 | planned: save suite | planned (P3) |
| `ObjectReader.peek_name` | reads the tree up to the first `m_Name`/`name` child; the peek node is cached process-wide by the root's `(m_Name, m_Type, m_Version)` (`:189-197`, `helpers/TypeTreeNode.py:190-211`) | A | API | planned: peek suite | planned (P3) |
| `ObjectReader.container` | builds the Environment's container index, then looks the object up by path id alone in its own file's `path_dict` (`:199-204`) | A | L1 (env) | planned: env dump | planned (P1) |
| `ObjectReader.get_class` | `ClassIDTypeToClassMap.get(type)`, `None` for ids without a class (`:186-187`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.dump_typetree_structure` | `TypeTreeNode.dump_structure` text (`:236-242`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.Position`, `ObjectReader.reset`, `ObjectReader.get` | reader position, seek to `byte_start`, `getattr` with default (`:206-221`) | — | API | — | n/a |
| unknown attributes | `__getattr__` forwards to the reader and returns `None` for anything else (`:223-225`) | A | API | planned: binding | planned (P3) |
| `ObjectReader.parse_monobehaviour_head` | reads the TPK's MonoBehaviour tree without `check_read` (`:325-330`) | A | L2 | planned: provider suite | planned (P3) |
| `ObjectReader.generate_monobehaviour_node` | dereferences `m_Script` and asks `Environment.typetree_generator` for `{namespace}.{class}`; raises `ValueError` without a generator or a node, which `_get_typetree_node` swallows (`:332-350`, `:300-304`) | A | L2 | planned: provider suite | planned (P3) |

## 9. Typetrees

### 9.1 Reading and writing values

Lines are in `helpers/TypeTreeHelper.py` unless named. `CANONICAL_DUMP.md` sections 3 and 7 give the full dict model.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TypeTreeHelper.read_typetree` | UnityPyBoost for `byte_size > 0`, else the pure-Python reader from the reader's position (`:123-158`) | A | L2 | `test_unitypy_boost.py` (all) | verified P0 (L2); impl P1 |
| `UnityPyBoost.read_typetree` | the C reader: lists for arrays, tuples for pairs, bytes for TypelessData, surrogate-escaped strings, float32 widened, signalling NaN quieted (`CANONICAL_DUMP.md` section 7) | A | L2 | `test_unitypy_boost.py` float_bits, shapes | verified P0 (L2); impl P1 |
| `UnityPyBoost.TypeTreeNode` | the C node type `helpers/TypeTreeNode.py` subclasses when Boost is present (`helpers/TypeTreeNode.py:25-49`) | — | L1 | — | n/a |
| `TypeTreeHelper.read_value` | pure-Python reader: primitives, pairs as tuples, ReferencedObject, vectors, classes, registry scope (`:182-263`) | A | L2 | `test_unitypy_boost.py` pure_python_path_returns_tuples | verified P0 (L2); impl P1 |
| `TypeTreeHelper.read_value_array` | pure-Python arrays of aligned elements: tuples, 16-bit signedness swap, per-array alignment (`:266-361`) | A | L2 | `test_unitypy_boost.py` pure_python_path_swaps_16_bit_signedness | verified P0 (L2); impl P1 |
| ManagedReferencesRegistry scope | the first registry child starts a scope that skips every other registry until its class ends (`:225-231`) | A | L2 | `test_unitypy_boost.py` registry_scope_is_the_class_that_holds_it | verified P0 (L2); impl P1 |
| ReferencedObject | `data` typed by `ref_types` on `(class, ns, asm)`; empty class omits `data` (`:197-206`) | A | L2 | `test_unitypy_boost.py` referenced_objects_and_the_registry_scope | verified P0 (L2); impl P1 |
| `TypeTreeHelper.get_ref_type_node` | raises `ValueError` without `ref_types` or on an unknown type, `None` for an empty class (`:88-109`) | A | L2 | `test_unitypy_boost.py` error_kinds | verified P0 (L2); impl P1 |
| `TypeTreeHelper.write_typetree` | the pure-Python writer, dict or object (`:161-179`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |
| `TypeTreeHelper.write_value` | aligns after every element whose node carries the align flag, which its reader does not mirror (`:407-458`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |
| `TypeTreeHelper.metaflag_is_aligned` | `meta_flag & 0x4000` (`:364-365`) | — | L2 | — | n/a |
| `TypeTreeConfig` | `as_dict`, `assetsfile`, `has_registry` (`:78-85`) | — | L2 | — | n/a |
| `TypeTreeHelper.get_annotation_keys` | a class's annotation names (`:112-120`) | — | API | — | n/a |
| `TypeTreeHelper.clean_name` | `(int&)` prefix and `?` suffix dropped, ` .:-[]` to `_`, `pass`/`from` get `_`, a leading digit gets `x` (`:368-378`); unlike the next row it fails on `""` | A | API | planned: object-mode suite | planned (P3) |
| `TypeTreeNode.clean_name` | the same, returning `""` for `""`; kept in sync with UnityPyBoost (`helpers/TypeTreeNode.py:350-363`) | A | API | planned: object-mode suite | planned (P3) |

### 9.2 Object mode

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| class lookup | `getattr(classes, m_Type, UnknownObject)` per class node; `PPtr<...>` nodes become `PPtr` (`helpers/TypeTreeHelper.py:233-258`) | A | API | planned: object-mode suite | planned (P3) |
| attribute names | `clean_name` of each child (`helpers/TypeTreeHelper.py:231`) | A | API | planned: object-mode suite | planned (P3) |
| missing fields | an `UnknownObject` carrying the node (`helpers/TypeTreeHelper.py:244-249`) | A | API | planned: object-mode suite | planned (P3) |
| extra fields | set as attributes after construction (`helpers/TypeTreeHelper.py:250-256`) | A | API | planned: object-mode suite | planned (P3) |
| pure-Python class arrays | the same rules per element (`helpers/TypeTreeHelper.py:314-357`) | A | API | planned: object-mode suite | planned (P3) |

### 9.3 TypeTreeNode

Lines are in `helpers/TypeTreeNode.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TypeTreeNode` | node with `m_Level`, `m_Type`, `m_Name`, `m_ByteSize`, `m_Version`, `m_Children`, `m_TypeFlags`, `m_VariableCount`, `m_Index`, `m_MetaFlag`, `m_RefTypeHash` (`:52-64`, `:70`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P1 |
| `TypeTreeNode.parse` | old format, stack-based, version gates of section 7.1 (`:78-109`) | A | L1 | `test_dump.py` tables_nodes_and_entries_follow_unitypy | verified P0 (L1); impl P4 |
| `TypeTreeNode.parse_blob` | blob rows under a hidden root; only the root's first child is kept; offsets with the high bit index the TPK's whole common-string table, and an offset missing from it becomes its decimal text (`:111-154`, `:129`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |
| `TypeTreeNode.from_list` | builds a tree from node dicts (needs `m_Level`, `m_Type`, `m_Name`; byte size and version default to 0) or nodes (`:156-188`) | A | API | planned: provider suite | planned (P3) |
| `TypeTreeNode.traverse` | pre-order (`:71-76`) | A | API | planned: binding | planned (P3) |
| `TypeTreeNode.get_name_peek_node` | see `ObjectReader.peek_name` (`:190-211`) | A | API | planned: peek suite | planned (P3) |
| `TypeTreeNode.dump` | old-format writer; writes the root's fields for every node (`:213-235`); see section 18, pre-v10 writing | C | L4 | planned: divergence test | planned (P3) |
| `TypeTreeNode.dump_blob` | blob writer; common strings by offset, except the one at offset 0 (`:237-278`) | A, C | L4 | planned: save suite; divergence test | planned (P3) |
| `TypeTreeNode.dump_structure` | AssetRipper-style text dump (`:280-288`) | A | API | planned: binding | planned (P3) |
| `TypeTreeNode.to_dict`, `TypeTreeNode.to_dict_list` | non-`None` fields, and the pre-order list (`:290-299`) | A | API | planned: binding | planned (P3) |
| equality | fields and children (`:301-302`) | — | API | — | n/a |
| `TypeTreeNode.get_common_strings` | the TPK's common strings, all of them for `version=None` (the only call), offsets by cumulative length (`:308-328`) | A | L1 | `test_dump.py` type_indices_and_blob_trees_follow_python | verified P0 (L1); impl P1 |

## 10. TPK, typetree providers and config

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `resources/lzma.tpk` | AssetRipper's TPK, LZMA, loaded at import (`helpers/Tpk.py:20-39`, `:484`). unityrs loads it from a path (`DESIGN.md` section 4, `unityrs-tpk`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `Tpk.init` | reads the bundled file (`helpers/Tpk.py:20-39`) | — | L2b | — | n/a |
| `Tpk.get_typetree_node` | `ClassInformation[class_id]` (a `KeyError` for unknown ids), versioned class (raises `ValueError` when absent), cached by `(class_id, version)` (`helpers/Tpk.py:42-55`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `Tpk.generate_node` | the release root node, pre-order, `m_Index` counted, no type flags (`helpers/Tpk.py:58-88`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `Tpk.get_item_for_version` | the last entry whose version ≤ the target; an entry holding `None` raises `ValueError` (`helpers/Tpk.py:472-481`) | A | L2b | planned: TPK node-list diff | planned (P3) |
| `TpkFile` | magic `TPK*`, version 1, NONE/LZ4/LZMA/brotli (`helpers/Tpk.py:145-197`) | A | L2b | planned: TPK loader test | planned (P3) |
| `TpkCompressionType`, `TpkDataType`, `TpkUnityClassFlags` | TPK enums (`helpers/Tpk.py:98-135`); only the type-tree blob is used | — | L2b | — | n/a |
| `TpkDataBlob`, `TpkTypeTreeBlob`, `TpkCollectionBlob`, `TpkFileSystemBlob`, `TpkJsonBlob` | TPK blob readers (`helpers/Tpk.py:207-279`) | — | L2b | — | n/a |
| `TpkUnityClass`, `TpkClassInformation`, `TpkUnityNode`, `TpkUnityNodeBuffer`, `TpkStringBuffer`, `TpkCommonString` | TPK records (`helpers/Tpk.py:289-429`) | — | L2b | — | n/a |
| `Tpk.read_string`, `Tpk.read_data`, `Tpk.read_version`, `Tpk.read_versions` | varint-length UTF-8, i32-length bytes, u64 versions (`helpers/Tpk.py:445-469`) | — | L2b | — | n/a |
| `TypeTreeGenerator` | wraps TypeTreeGeneratorAPI; without it, construction raises `ImportError` (`helpers/TypeTreeGenerator.py:6-25`) | A | L2 | planned: provider suite | planned (P3) |
| `TypeTreeGeneratorBase` | the stand-in base when TypeTreeGeneratorAPI is missing (`helpers/TypeTreeGenerator.py:9-17`) | — | API | — | n/a |
| `TypeTreeGenerator.load_local_game` | IL2CPP (`GameAssembly.dll` + metadata) or Mono `Managed/*.dll` (`helpers/TypeTreeGenerator.py:27-39`) | A | L2 | planned: provider suite | planned (P3) |
| `TypeTreeGenerator.load_local_dll_folder` | every `.dll` in a folder (`helpers/TypeTreeGenerator.py:41-48`) | A | L2 | planned: provider suite | planned (P3) |
| `TypeTreeGenerator.get_nodes_up` | appends `.dll`, builds the tree with byte size and version 0 and the generator's meta flags, cached by `(assembly, fullname)` (`helpers/TypeTreeGenerator.py:50-75`) | A | L2 | planned: provider suite | planned (P3) |
| `TpkClassGenerator.generate_classes`, `TpkClassGenerator.generate_field_type`, `TpkClassGenerator.implement_node_class` | regenerate `classes/generated.py` from the TPK (`tools/TpkClassGenerator.py:198-376`) | — | — | — | n/a |
| `NodeClass`, `NodeClassField` | the generator's records (`tools/TpkClassGenerator.py:132-195`) | — | — | — | n/a |
| `config.FALLBACK_UNITY_VERSION` | `None`; used when a file has no usable version (`config.py:5`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `config.get_fallback_version` | raises `UnityVersionFallbackError` unless the fallback is a `str`; warns once (`config.py:21-40`) | A | API | planned: config suite | planned (P3) |
| `TypeTreeError` | raised for a missing tree, carries `nodes` (`exceptions.py:1-4`) | A | API | planned: binding | planned (P3) |
| `UnityVersionFallbackError`, `UnityVersionFallbackWarning` | `exceptions.py:7-14` | A | API | planned: binding | planned (P3) |

## 11. Versions

Lines are in `helpers/UnityVersion.py`. `CANONICAL_DUMP.md` section 4.2 gives the packing rule.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `UnityVersion` | an `int`, `major<<48 \| minor<<32 \| build<<16 \| type<<8 \| number`; compares with ints and tuples (`:23`, `:112-142`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `UnityVersion.from_str` | the DOTALL pattern (`:7-10`), one type character, `a b c f p x` else 255, no type packs `f`; spill-over of wide fields (`:63-88`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `UnityVersion.from_list` | packs five fields (`:57-60`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `UnityVersion.major`, `UnityVersion.minor`, `UnityVersion.build`, `UnityVersion.type_number`, `UnityVersion.as_tuple` | field extraction, 16/16/16/8/8 bits (`:28-54`, `:109-110`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `UnityVersion.type` | `UnityVersionType` of the type byte; raises `ValueError` for bytes 6-254 (`:40-42`) | A | L1 | `test_dump.py` version_strings_pack_as_unitypy_packs_them | verified P0 (L1); impl P1 |
| `UnityVersion.type_str`, `UnityVersion.postfix` | the original type character and trailing text (`:44-50`) | A | API | planned: binding | planned (P3) |
| `UnityVersionType` | `a b c f p x u` = 0-5, 255 (`:13-20`) | A | L1 | `test_unitypy_boost.py` version_packing | verified P0 (L1); impl P1 |
| `str(UnityVersion)` | above 5.x drops the build and its dot: `2019.4.15f1` prints `2019.4f1`, and `2018.4.36` (no type) prints `2018.4f0`; 5.x and below drop the type and its number: `5.6.7f1` prints `5.6.7` (`:90-94`); section 19.2 | A | API | planned: binding | planned (P3) |

## 12. UnityCN and the decryptor

Lines are in `helpers/ArchiveStorageManager.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| encryption flag | `UsesAssetBundleEncryption` is `0x200` in `ArchiveFlagsOld` and `0x1400` in `ArchiveFlags` (`enums/BundleFile.py:12-26`); a set flag builds a decryptor (`files/BundleFile.py:123-124`) | A | L1 | `test_dump.py` bundle_layouts_names_and_child_failures | verified P0 (L1); impl P3 |
| `ArchiveStorageManager.set_assetbundle_decrypt_key` | a 16-byte key, `str` encoded with surrogateescape; else `ValueError` (`:16-22`) | A | API | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageDecryptor` | u32 unknown, data/key vectors and signature vectors (`:62-72`); no key (`ArchiveStorageManager.DECRYPT_KEY`) raises `LookupError` (`:74-85`); the AES-ECB signature must equal `ArchiveStorageManager.UNITY3D_SIGNATURE`, `#$unity3dchina!@`, else a plain `Exception` (`:12`, `:87-89`); index and substitute tables from the decrypted key (`:91-94`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageDecryptor.decrypt_block` | UnityPyBoost's `decrypt_block` when present (`:96-98`), else the Python loop (`:100-107`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `UnityPyBoost.decrypt_block` | the C version of the loop | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageDecryptor.decrypt`, `ArchiveStorageDecryptor.decrypt_byte` | the LZ4-token-aware byte decryption (`:109-143`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageManager.decrypt_key` | AES-ECB-encrypt the key vector, XOR with data (`:33-37`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageManager.read_vector` | 16 + 16 bytes and one skipped byte (`:25-30`) | A | L1 | planned: UnityCN samples | planned (P3) |
| `ArchiveStorageManager.brute_force_key` | tries every 16-word-character window of a file as the key (`:40-59`) | — | — | — | n/a |
| encrypted save | `save_fs` clears the encryption flag; UnityPy never writes encrypted bundles (`files/BundleFile.py:303-307`) | A | L4 | planned: save suite | planned (P3) |

## 13. Streams

Lines are in `streams/EndianBinaryReader.py` and `streams/EndianBinaryWriter.py`. They are Python I/O plumbing; their observable rules sit in the rows that use them, and these are the ones that matter.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `EndianBinaryReader` | picks a memoryview, stream or local-file reader by input type; default endian `>` (`streams/EndianBinaryReader.py:33-69`) | — | — | — | n/a |
| `EndianBinaryReader_Memoryview`, `EndianBinaryReader_Streamable`, `EndianBinaryReader_Streamable_LocalFile` | the three readers (`streams/EndianBinaryReader.py:231-332`) | — | — | — | n/a |
| `EndianBinaryReader.memory_read_func`, `EndianBinaryReader.stream_read_func` | the generated primitive readers (`streams/EndianBinaryReader.py:335-349`) | — | — | — | n/a |
| strings to NUL | surrogate-escaped UTF-8, at most 32767 bytes; with no NUL in reach the memory reader returns 32767 bytes without consuming a NUL, or, when the window reaches the end, raises a plain `Exception` "String not terminated" (`streams/EndianBinaryReader.py:275-284`; `EndianBinaryReader.reNot0` is the search) | A | L1 | `test_dump.py` l0_detection_and_name_rules | verified P0 (L1); impl P1 |
| aligned strings | a length outside `0 < n ≤ remaining` reads `""` (`streams/EndianBinaryReader.py:266-273`) | A | L2 | `test_unitypy_boost.py` error_kinds | verified P0 (L2); impl P1 |
| `EndianBinaryWriter` | default endian `>`; `write_string_to_null` and `write_aligned_string` encode with surrogateescape (`streams/EndianBinaryWriter.py:9-130`) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |

## 14. Classes

Every class in `classes/generated.py` is listed in Appendix A. They are attrs records built from the TPK by `TpkClassGenerator`; their behaviour is the typetree's (section 9). This section covers the classes UnityPy adds behaviour to, and the classes the export helpers read.

### 14.1 Base classes

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Object` | base of every generated class; fields arrive as keyword arguments; `copy.copy` rebuilds through the annotations (`classes/Object.py:11-54`) | A | API | planned: object-mode suite | planned (P3) |
| `Object.set_object_reader` | `read_typetree(wrap=True)` attaches the reader (`classes/Object.py:17-18`) | A | API | planned: object-mode suite | planned (P3) |
| `Object.assets_file` | the reader's SerializedFile, or `None` (`classes/Object.py:23-27`) | A | API | planned: object-mode suite | planned (P3) |
| `Object.save` | `object_reader.save_typetree(self)`; raises `ValueError` without a reader (`classes/Object.py:29-33`) | A | L3 | planned: object-mode resave | planned (P3) |
| `UnknownObject` | a class the generated module lacks, or one whose fields do not match; keeps the node in `__node__` (`classes/UnknownObject.py:7-14`) | A | API | planned: object-mode suite | planned (P3) |
| `UnknownObject.get_type` | the node's `m_Type`, or `None` (`classes/UnknownObject.py:16-17`) | A | API | planned: object-mode suite | planned (P3) |
| `PPtr` | `m_FileID`, `m_PathID`, `assetsfile`; truthy when the path id is not 0; equality and hash on the two ids (`classes/PPtr.py:15-19`, `:98-107`) | A | API | planned: object-mode suite | planned (P3) |
| `PPtr.file_id`, `PPtr.path_id` | aliases of the two ids (`classes/PPtr.py:21-29`) | — | API | — | n/a |
| `PPtr.type` | `deref().type` (`classes/PPtr.py:31-33`) | A | L1 (env) | planned: env dump | planned (P1) |
| `PPtr.deref` | path id 0 raises `ValueError`; file id 0 is the pointer's own file; otherwise `externals[file_id - 1]`, its path stripped of `archive:/` and `assets/`, its last component lower-cased, matched against the parent container's entry names, else `Environment.find_file`; raises `FileNotFoundError`; a missing path id raises `KeyError` (`classes/PPtr.py:43-90`) | A | L1 (env) | planned: env dump | planned (P1) |
| `PPtr.deref_parse_as_object`, `PPtr.deref_parse_as_dict` | `deref()` then parse (`classes/PPtr.py:92-96`) | A | L2 | planned: env dump | planned (P1) |
| `PPtr.read`, `PPtr.read_typetree` | backward-compatible aliases of the two above (`classes/PPtr.py:35-41`) | — | API | — | n/a |
| `Vector2f`, `Vector3f`, `Vector4f` | attrs records with defaults 0 (`classes/math.py:10-37`) | A | API | planned: object-mode suite | planned (P3) |
| `float3`, `float4` | aliases of `Vector3f` and `Vector4f` (`classes/math.py:40-41`) | A | API | planned: object-mode suite | planned (P3) |
| `Quaternionf` | a `Vector4f` (`classes/math.py:44-47`) | A | API | planned: object-mode suite | planned (P3) |
| `Matrix3x4f`, `Matrix4x4f` | attrs records of named elements (`classes/math.py:50-83`) | A | API | planned: object-mode suite | planned (P3) |
| `ColorRGBA` | `r g b a` floats, or built from a packed `rgba` u32 (older trees) (`classes/math.py:86-111`) | A | API | planned: object-mode suite | planned (P3) |
| `generated.unitypy_define` | applies attrs `define` with temporarily rewritten bases (`classes/generated.py:26-44`) | — | — | — | n/a |
| `ClassIDType` | an `ExtendableEnum` of 368 class ids; an unknown id becomes `Unknown (N)` instead of raising (`enums/ClassIDType.py:5`, `enums/ExtendableEnum.py:6-19`) | A | L1 | `test_dump.py` object_records_follow_the_table_and_the_parse | verified P0 (L1); impl P1 |
| `ExtendableEnum` | the base that adds unknown members (`enums/ExtendableEnum.py:6-19`) | — | — | — | n/a |
| class map | `classes/ClassIDTypeToClassMap.py` maps 358 of the 368 class ids to generated classes (`classes/ClassIDTypeToClassMap.py:720`); `ObjectReader.get_class` reads it | A | API | planned: binding | planned (P3) |

### 14.2 Classes with special handling

| class | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Texture2D.image` | getter `Texture2DConverter.get_image_from_texture2d`; setter `set_image` (`classes/legacy_patch/Texture2D.py:8-11`, `:78`) | A | L5 | planned: L5 goldens per format (section 17) | planned (P1) |
| `Texture2D.get_image_data` | `image_data` when set, else `m_StreamData` through `ResourceReader.get_resource_data`, else `ValueError` (`classes/legacy_patch/Texture2D.py:63-75`) | A | L5 | planned: `.resS` texture fixture | planned (P1) |
| `Texture2D.set_image` | section 18 | | | | |
| `Texture2DArray.images` | `GraphicsFormat(m_Format)` (an unknown value raises `ValueError`) through `GraphicsFormat.GRAPHICS_TO_TEXTURE_MAP` (unmapped raises `NotImplementedError`); data from `image_data`, and from `m_StreamData` only when `image_data is None`, which a parsed array never is: a streamed array's empty TypelessData reads as `b""`, so every slice raises `ValueError` "Texture2D has no image data" (checked in the oracle on 2026-10-28; section 19.2); `m_Depth` slices of `m_DataSize // m_Depth`; decoded with platform 0 and no blob, so no Switch or Xbox handling (`classes/legacy_patch/Texture2DArray.py:12-43`, `:20-21`, `enums/GraphicsFormat.py:179-277`) | A, C | L5 | planned: array fixture | planned (P3) |
| `Sprite.image` | `SpriteHelper.get_image_from_sprite` (`classes/legacy_patch/Sprite.py:4-10`); section 15.2 | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteAtlas` | no patch: `SpriteHelper.get_image_from_sprite` dereferences `m_SpriteAtlas`, or finds an atlas in the same file whose peeked name equals `m_AtlasTags[0]`, then takes the first `m_RenderDataMap` entry whose key equals `m_RenderDataKey`; a key the atlas lacks raises `StopIteration` (`export/SpriteHelper.py:63-81`, `:78`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteRenderData` | the sprite's own render data when there is no atlas; with or without an atlas, the tight-packing mesh always comes from the sprite's own `m_RD` (`export/SpriteHelper.py:80-81`, `:117`) | A | L5 | planned: sprite goldens | planned (P1) |
| `Mesh.export` | `MeshExporter.export_mesh(self, format)` (`classes/legacy_patch/Mesh.py:4-8`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `Renderer.export` | `MeshRendererExporter.export_mesh_renderer(self, dir)`, inherited by `MeshRenderer` and `SkinnedMeshRenderer` (`classes/legacy_patch/Renderer.py:4-8`) | C | L6 | planned: divergence test | planned (P3) |
| `AudioClip.extension` | `AUDIO_TYPE_EXTEMSION.get(m_CompressionFormat, ".audioclip")`; the dict mixes `AudioType` and `AudioCompressionFormat` keys, so equal values collide and the later one wins (`classes/legacy_patch/AudioClip.py:5-6`, `enums/Audio.py:33-56`) | A | API | planned: binding | planned (P3) |
| `AudioClip.samples` | `AudioClipConverter.extract_audioclip_samples(self)` (`classes/legacy_patch/AudioClip.py:9-12`); section 16 | A, Tol | L8 | section 16 | planned (P3) |
| `Shader.export` | `ShaderConverter.export_shader(self)` (`classes/legacy_patch/Shader.py:4-7`) | A, C | L7 | planned: shader text goldens | planned (P3) |
| `GameObject.m_Components` | `[]` for `None`; `ComponentPair.component` for each pair; an empty list raises `IndexError`, and old `(classID, PPtr)` tuples raise `AttributeError` (`classes/legacy_patch/GameObject.py:9-15`); section 19.2 | A | API | planned: binding | planned (P3) |
| `GameObject.m_Animator`, `GameObject.m_Animation`, `GameObject.m_Transform`, `GameObject.m_MeshRenderer`, `GameObject.m_SkinnedMeshRenderer`, `GameObject.m_MeshFilter` | the first component whose dereferenced type matches, else `None` (`classes/legacy_patch/GameObject.py:18-33`) | A | L1 (env) | planned: binding | planned (P3) |
| `Transform` | no special handling; reached through `GameObject.m_Transform`; read through its typetree | A | L2 | corpus dump | verified P0 (L2); impl P1 |
| `Material` | no patch; `MeshRendererExporter.export_material` reads `m_SavedProperties` (`export/MeshRendererExporter.py:106-189`) | A, C | L6 | planned: MTL goldens | planned (P3) |
| `MonoBehaviour` | without an embedded tree: the TPK's head plus `Environment.typetree_generator` (`files/ObjectReader.py:300-304`); the extractor writes it as JSON (section 15.7) | A | L2, L7 | corpus dump (embedded trees) | verified P0 (L2); impl P1 |
| `MonoScript` | read to build `{namespace}.{class}` for the generator (`files/ObjectReader.py:339-346`) | A | L2 | corpus dump | verified P0 (L2); impl P1 |
| `AssetBundle` | parsed while loading for `m_Container` and `m_PreloadTable` (`files/SerializedFile.py:316-324`, `helpers/ContainerHelper.py:21-49`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `Font` | no patch; the extractor writes `m_FontData` as `.ttf`, `.otf` when it starts with `OTTO` (`tools/extractor.py:177-187`) | A | L7 | planned: font fixture | planned (P3) |
| `TextAsset` | no patch; the extractor writes `m_Script` re-encoded with surrogateescape (`tools/extractor.py:165-174`) | A | L7 | planned: text fixture | planned (P3) |
| `VideoClip` | no special handling and no exporter in UnityPy: the dict only. unityrs-export's VideoClip export goes beyond UnityPy. | A | L2 | planned: Editor fixture | planned (P3) |
| `AnimationClip`, `Avatar`, `Animator`, `AnimatorController` | no special handling: the dict only. Sampling, retargeting and GLB go beyond UnityPy (L11). | A | L2 | corpus dump | verified P0 (L2); impl P2b |

### 14.3 ContainerHelper

Lines are in `helpers/ContainerHelper.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `ContainerHelper` | from an AssetBundle (`m_Container`, `m_PreloadTable`) or a list; `container_dict` path→PPtr (last wins); `path_dict` path id→path (last wins, file id ignored) (`:12-29`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `ContainerHelper.parse_preload_table` | for each container entry with a valid preload range, dereferences each preload PPtr and sets its path in the target file's `path_dict` if absent; dereference failures are skipped; runs once (`:31-49`) | A | L1 (env) | planned: env dump | planned (P1) |
| `ContainerHelper.items` | `(path, PPtr)` in container order, repeats kept (`:51-52`) | A | L1 | `test_dump.py` samples | verified P0 (L1); impl P1 |
| `ContainerHelper.keys`, `ContainerHelper.values` | lists built from sets: deduplicated, in no defined order (`:54-58`) | A | API | planned: binding | planned (P3) |
| mapping protocol | `[]`, `in`, `len` (entries, repeats counted), iteration over `keys()`; attribute access by path; assignment and deletion raise `NotImplementedError` (`:60-85`) | A | API | planned: binding | planned (P3) |

## 15. Export helpers

### 15.1 Texture2DConverter

Lines are in `export/Texture2DConverter.py`. Section 17 has the per-format detail.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `Texture2DConverter.get_image_from_texture2d` | `parse_image_data` with the texture's data, size, format, the reader's version (else `(0, 0, 0, 0)`) and platform, and `m_PlatformBlob` (`:237-251`) | A | L5 | planned: L5 goldens | planned (P1) |
| `Texture2DConverter.parse_image_data` | zero width or height gives an empty RGBA image; empty data raises `ValueError`; an unknown format number raises `ValueError`; then the Xbox swap, the Switch deswizzle or block padding, crunch unpacking, the decoder, a crop back to size, and a vertical flip (`:254-320`) | A | L5 | planned: L5 goldens | planned (P1) |
| output flip | `flip=True` by default: every decoded image is flipped top to bottom (`:317-318`) | A | L5 | planned: L5 goldens | planned (P1) |
| `Texture2DConverter.get_compressed_image_size` | pads to the block: ASTC by name, `DXT`/`BC`/`ETC`/`EAC` 4×4, PVRTC 8×4 (2 bpp) or 4×4; no padding for any other name, ATC included (`:22-50`) | A | L5 | planned: NPOT goldens | planned (P1) |
| `Texture2DConverter.pad_image` | pads with the last column and row (NEAREST) and the corner pixel (`:53-80`) | A | L5 | planned: `set_image` round trip | planned (P3) |
| `Texture2DConverter.swap_bytes_for_xbox` | swaps byte pairs for `Texture2DConverter.XBOX_SWAP_FORMATS` on XBOX360, first of all, so a crunched texture's crunch stream is swapped before it is unpacked (`:274-275`, `:323-328`, `:569`) | A | L5 | planned: Xbox fixture | planned (P3) |
| `Texture2DConverter.pillow` | `Image.frombytes(mode, size, data, codec, args)`, then an optional channel reorder (`:331-344`) | A | L5 | planned: L5 goldens | planned (P1) |
| `Texture2DConverter.atc` | texture2ddecoder ATC, BGRA to RGBA (`:347-353`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.astc` | astcenc into `U8`; data shorter than the block count raises `ValueError` (`:356-366`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.get_astc_context` | one LDR context per thread and block size, quality 100, `USE_DECODE_UNORM8` (`:369-387`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.calculate_astc_compressed_size` | 16 bytes per block (`:390-397`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.pvrtc` | texture2ddecoder PVRTC, 2 or 4 bpp, BGRA to RGBA (`:400-402`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.etc` | texture2ddecoder ETC1, ETC2, ETC2 A1, ETC2 A8 (`:405-417`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.eac` | texture2ddecoder EAC R, R signed, RG, RG signed (`:420-432`) | A | L5 | planned: L5 goldens | planned (P3) |
| `Texture2DConverter.half` | `int(half * 256)` per value, truncated toward zero, `len(codec)` values per pixel, and `Texture2DConverter.CONV_TABLE` always makes `codec` the string `"raw"`, so three; a value whose result falls outside 0-255 (1.0 and above, -1/256 and below) or a NaN raises `ValueError`, an infinity `OverflowError`, and data that runs out `struct.error` (`:435-447`) | B, C | L5 | section 17 | planned (P3) |
| `Texture2DConverter.rg` | declared `(data, w, h, mode, codec, args)`, but `CONV_TABLE` passes `("RGB", "raw", <codec>)`, so `codec` is `"raw"` and `Texture2DConverter.RG_PADDING_MAP["raw"]` raises `KeyError` (`:450-468`) | B | L5 | section 17 | planned (P3) |
| `Texture2DConverter.rgb9e5float` | assigns floats into a `bytearray`, which raises `TypeError` (`:471-484`) | B | L5 | section 17 | planned (P3) |
| `Texture2DConverter.image_to_texture2d` | maps the target format to the format it writes (section 17, `set_image` column), flips, pads, compresses or packs, and swizzles for Switch (`:118-234`) | A | L5 | planned: `set_image` round trip | planned (P3) |
| `Texture2DConverter.compress_etcpak` | etcpak per format; BC7 passes `None` parameters, which etcpak 0.9.14 rejects (`:83-103`, `:95`) | A, C | L5 | planned: `set_image` round trip | planned (P3) |
| `Texture2DConverter.compress_astc` | astcenc with the cached LDR context (`:106-115`) | A | L5 | planned: `set_image` round trip | planned (P3) |

### 15.2 SpriteHelper

Lines are in `export/SpriteHelper.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `SpriteSettings` | `settingsRaw` bits: packed (0), packing mode (1), rotation (2-5), mesh type (6) (`:29-40`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpritePackingMode`, `SpritePackingRotation`, `SpriteMeshType` | the `SpriteSettings` fields (`enums/SpritePackingMode.py`, `enums/SpritePackingRotation.py`, `enums/SpriteMeshType.py`); a rotation value from 5 to 15 raises `ValueError` while the settings are read | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.get_image` | decodes the texture unflipped; with an alpha texture, merges its first band as alpha; caches in the SerializedFile's `_cache` by path id only (`:44-59`) | A, C | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.get_image_from_sprite` | atlas lookup (section 14.2); crop by `textureRect`, which Pillow rounds half to even; flips and rotations when packed; tight packing goes through the mesh; a final vertical flip (`:62-127`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.mask_sprite` | tight sprites whose UVs are all zero: a mode `1` polygon mask from the vertices, offset to the minimum and scaled by `m_PixelsToUnits`; composite for RGBA, `putalpha` otherwise (`:130-170`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.render_sprite_mesh` | tight sprites with UVs: drops the constant axis (raises for 3D), rounds positions and UVs (half to even), copies each triangle of the first submesh only, returning inside the loop; no triangles raises `ValueError` (`:173-224`, `:222`) | A | L5 | planned: sprite goldens | planned (P1) |
| `SpriteHelper.copy_triangle` | a triangle of the same shape is pasted through a polygon mask; any other goes through an affine transform solved by numpy (`:227-301`) | A, Tol | L5 | planned: sprite goldens, affine metric | planned (P1) |
| `SpriteHelper.linalg_solve`, `SpriteHelper.get_matrix_inverse`, `SpriteHelper.get_matrix_determinant`, `SpriteHelper.get_matrix_minor`, `SpriteHelper.transpose_matrix` | the solver used without numpy (`:304-346`); the oracle has numpy | — | — | — | n/a |

### 15.3 MeshHelper and PackedBitVector

Lines are in `helpers/MeshHelper.py` unless named.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `MeshHandler` | the version from the argument or the Mesh's reader (a SpriteRenderData needs one); endianness defaults to `<` and no caller passes another (`:71-108`) | A, C | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.process` | channels and streams by version (below 4, 4, 5 and up); `m_StreamData` through `ResourceReader`; 16- or 32-bit indices; vertex data from 3.5; compressed mesh from 2.6 (`:110-195`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.copy_from_mesh` | fills unset arrays from the Mesh's own vertex, normal, tangent, UV, colour and skin arrays (`:197-233`) | A | L6 | planned: mesh numbers | planned (P2a) |
| `MeshHandler.copy_from_spriterenderdata` | index buffer, vertices and UVs from the sprite's render data (`:235-251`) | A | L6 | planned: sprite goldens | planned (P1) |
| `MeshHandler.get_streams` | derived streams, offsets aligned to 16 (`:256-281`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.get_channels` | the pre-4 fixed channel layout (`:283-321`) | A | L6 | planned: Editor fixture | planned (P4) |
| `MeshHandler.read_vertex_data` | per channel through `UnityPyBoost.unpack_vertexdata`; pre-2018 colour forced to 4 components (`:323-393`) | A, C | L6 | planned: mesh numbers | planned (P1) |
| `UnityPyBoost.unpack_vertexdata` | the C strided gather, with byte swapping (`:366-376`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.assign_channel_vertex_data` | channel index to attribute, for 2018 and later and for earlier versions; an unknown channel raises (`:395-449`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.get_channel_dtype` | `VertexChannelFormat` below 2017, `VertexFormat2017` below 2019, `VertexFormat` after (`:451-462`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.get_channel_component_size` | struct size of that type (`:464-466`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHandler.decompress_compressed_mesh` | vertices, UVs, bind poses (below 5), normals, tangents, float colours (5 and up), skin, triangles, packed colours (`:468-628`) | A, C | L6 | planned: mesh numbers | planned (P2a) |
| `MeshHandler.get_triangles` | per submesh: triangles; strips (below 4, or strip topology) skipping degenerates and alternating winding, and rewriting `indexCount`; quads as two triangles; lines and points raise `ValueError` (`:630-683`) | A | L6 | planned: mesh numbers | planned (P1) |
| `MeshHelper.vector_list_to_tuples`, `MeshHelper.lists_to_tuples`, `MeshHelper.zeros` | list helpers (`:38-60`) | — | — | — | n/a |
| `MeshHelper.normalize` | unit vector, zeros below length 1e-5 (`:63-68`) | A | L6 | planned: mesh numbers | planned (P2a) |
| `PackedBitVector.unpack_ints` | little-endian bit stream, `m_BitSize` bits per item from `start` (`helpers/PackedBitVector.py:20-65`) | A | L6 | planned: mesh numbers | planned (P1) |
| `PackedBitVector.unpack_floats` | bit size 0 repeats `m_Start`; else `x * range / (2^bits - 1) + start` in f64 (`helpers/PackedBitVector.py:68-85`) | A | L6 | planned: mesh numbers | planned (P1) |
| `PackedBitVector.reshape` | rows of 1D or 2D shape (`helpers/PackedBitVector.py:7-17`) | — | — | — | n/a |
| `VertexChannelFormat`, `VertexFormat2017`, `VertexFormat` | struct codes per format in `VertexFormat.VERTEX_CHANNEL_FORMAT_STRUCT_TYPE_MAP`, `VertexFormat.VERTEX_FORMAT_2017_STRUCT_TYPE_MAP` and `VertexFormat.VERTEX_FORMAT_STRUCT_TYPE_MAP`; UNorm, SNorm and the byte colour formats read as raw integers (`enums/VertexFormat.py:4-80`) | A, C | L6 | planned: mesh numbers | planned (P1) |
| `MeshTopology`, `GfxPrimitiveType` | topology enums (`enums/MeshTopology.py`, `enums/GfxPrimitiveType.py`) | A | L6 | planned: mesh numbers | planned (P1) |

### 15.4 MeshExporter and MeshRendererExporter

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `MeshExporter.export_mesh` | `"obj"` only, else `NotImplementedError` (`export/MeshExporter.py:11-14`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `MeshExporter.export_mesh_obj` | `g name`, optional `mtllib`; `v` with x negated, `vt`, and `vn` with x negated, every number as `{:.9G}`: upper-case `1E-10`, `-0` for a negated zero, and `NAN`/`INF`, because the `.replace("nan", "0")` meant for NaN never matches the upper-case text (checked in the oracle on 2026-10-28; section 19.2); per submesh `g name_i`, optional `usemtl`, faces `c/c/c b/b/b a/a/a` 1-based; returns `False` without vertices (`export/MeshExporter.py:17-56`, `:33`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `MeshRendererExporter.get_mesh` | `SkinnedMeshRenderer.m_Mesh`, or the GameObject's MeshFilter mesh (`export/MeshRendererExporter.py:25-45`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `MeshRendererExporter.export_mesh_renderer` | materials per submesh offset by `m_StaticBatchInfo.firstSubMesh` or `min(m_SubsetIndices)`; writes each material texture as `{name or key}.png`, the `.obj` and the `.mtl` (`export/MeshRendererExporter.py:48-103`) | C | L6 | planned: divergence test | planned (P3) |
| `MeshRendererExporter.export_material` | `newmtl`; `Ka`/`Kd`/`Ks` from `_SColor`/`_Color`/`_SpecularColor` with defaults, `Tr`, `Ns`, all `{:.4f}`; texture maps by key (`export/MeshRendererExporter.py:106-189`). Only the defaults ever print: a material holding any of the three colours raises `AttributeError`, because `clt` reads `color.R` and `ColorRGBA` has `r g b a` (`export/MeshRendererExporter.py:110`; `classes/math.py:86-91`), and one holding any texture raises `ValueError` (`export/MeshRendererExporter.py:168`); both checked in the oracle on 2026-10-28 | A, C | L6 | planned: MTL goldens | planned (P3) |

### 15.5 ShaderConverter

Lines are in `export/ShaderConverter.py`. `export/__init__.py` does not import it; `Shader.export` does.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `ShaderConverter.export_shader` | 5.3-5.4 `m_SubProgramBlob` through LZ4, read big-endian (the reader's default), with `GpuProgramIndex N` substituted; 5.5 and up `compressedBlob`, read little-endian (`:84`); else `m_Script` as UTF-8; every result starts with `ShaderConverter.HEADER`, the `NOTE: This is *not* a valid shader file` banner (`:30-55`) | A, C | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShader` | one LZ4 slice per platform, nested-list entries take their first item, stops at the shorter length list (`:58-89`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShaderParsedForm` | `Shader`, properties, subshaders, fallback, custom editor (`:92-113`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedSubShader` | LOD, tags, passes (`:116-130`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedPass` | Pass, UsePass, GrabPass; state and the five program kinds (`:133-181`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedSubPrograms` | groups by blob index and program type; the first usable platform per group (`:184-227`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShaderState` | name, LOD, tags, cull, ZTest, ZWrite, colour mask, blend, stencil, fog (`:230-292`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedShaderRTBlendState` | an empty string; its caller is commented out (`:295-298`) | — | L7 | — | n/a |
| `ShaderConverter.ConvertSerializedTagMap` | `Tags { "k" = "v" }` (`:301-311`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.ConvertSerializedProperties`, `ShaderConverter.ConvertSerializedProperty` | property block; a property type other than Color, Vector, Float, Range and Texture raises `ValueError`, Int (5, from Unity 2021.1) included (`:314-372`, `:369`; the enum stops at 4, `enums/SerializedPropertyType.py:4-9`) | A, C | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.CheckGpuProgramUsable` | platform/program-type table; NaCl, Flash and PSM raise `NotImplementedError`, and so does every platform the table lacks: PS3, `kShaderCompPlatformNone` and anything above 20 (`:375-493`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderConverter.GetPlatformString` | platform names, `unknown` otherwise (`:496-540`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderProgram` | sub-program table, entry size 12 from 2019.3 and 8 before (`:543-568`) | A | L7 | planned: shader text goldens | planned (P3) |
| `ShaderSubProgram` | sub-program versions 201509030 to 202012090: keywords, local keywords in `[201806140, 202012090)`, code; GLSL, Metal and console code as text, DX9 and DX11 as a `// shader disassembly not supported on DXBC` comment, SPIR-V as nothing, any other type as a `//shader disassembly not supported on {type}` comment (`:571-689`); an unknown program type raises `ValueError` (`:589`) | A, C | L7 | planned: shader text goldens; divergence test | planned (P3) |
| `PassType`, `SerializedPropertyType`, `ShaderCompilerPlatform`, `ShaderGpuProgramType`, `TextureDimension` | enums the converter reads (`enums/PassType.py`, `enums/SerializedPropertyType.py`, `enums/ShaderCompilerPlatform.py`, `enums/ShaderGpuProgramType.py`, `enums/TextureDimension.py`); an unknown program type raises `ValueError` (`:589`) | A | L7 | planned: shader text goldens | planned (P3) |

### 15.6 TextureSwizzler

Lines are in `helpers/TextureSwizzler.py`.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `TextureSwizzler.is_switch_swizzled` | Switch, a platform blob of at least 12 bytes, and more than one GOB per block (`:121-127`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.get_switch_gobs_per_block` | `1 << u32le(blob[8:12])` (`:117-118`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.deswizzle`, `TextureSwizzler.swizzle` | GOB walk over 16-byte texels (`:17-69`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.get_padded_texture_size` | pads to whole GOB blocks (`:106-114`) | A | L5 | planned: Switch fixture | planned (P3) |
| `TextureSwizzler.ceil_divide` | integer ceiling (`:13-14`) | — | — | — | n/a |
| Switch format table | 29 formats with a texel block; any other format raises `KeyError` when swizzled (`:73-103`) | A | L5 | planned: Switch fixture | planned (P3) |

### 15.7 The extractor

Lines are in `tools/extractor.py`. Level L9 is its output tree.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| `extractor.extract_assets` | loads the source; by container (default) in `extractor.EXPORT_TYPES` priority (a stable sort), paths split on `/` minus `ignore_first_container_dirs`, every entry exported with no de-duplication, so a later entry with the same path overwrites an earlier one (a Texture2D its Sprite's PNG); or every object with its name appended, skipping what an earlier export returned. Nothing catches a failure: the first export that raises ends the run (`:81-157`). | A, C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.export_obj` | export function by type (unknown types through `exportMonoBehaviour` when asked); the filter sees the parsed object; the path's own extension (the container's, or one in the object's name) is split off and handed to the export function, which keeps it; `append_path_id` appends `_None`, not the path id, because it reads `m_PathID` from the `ObjectReader`, whose attribute is `path_id` (`:25-78`, `:75`; `files/ObjectReader.py:223-225`); checked in the oracle on UnityPy's samples on 2026-10-28 | A, C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.exportTextAsset` | `{fp}.txt` (or the container's extension) from `m_Script` re-encoded with surrogateescape (`:165-174`) | A | L7 | planned: text fixture | planned (P3) |
| `extractor.exportFont` | `m_FontData` as `.ttf`, or `.otf` for `OTTO`; nothing without data (`:177-187`) | A | L7 | planned: font fixture | planned (P3) |
| `extractor.exportMesh` | `obj.export()` as UTF-8 text to `{fp}.obj`, or to the container's extension (`.asset` in UnityPy's samples), so a vertex-less mesh writes `False` and raises `TypeError` (`:190-197`) | A | L6 | planned: OBJ goldens | planned (P3) |
| `extractor.exportShader` | `obj.export()` as UTF-8 text (`:200-207`) | A | L7 | planned: shader text goldens | planned (P3) |
| `extractor.exportMonoBehaviour` | `json.dumps(parse_as_dict(), indent=4, ensure_ascii=False)` encoded with surrogateescape, else the raw bytes as `.bin` (`:210-224`). Through `export_obj` it never returns: for a MonoBehaviour it writes the JSON and then raises `AttributeError` on `obj.path_id` (`:224`), and for any other type (`export_unknown_as_typetree`) the parsed object has no `parse_as_dict` or `get_raw_data`, so it raises `AttributeError` before writing; checked in the oracle on 2026-10-28 | A, C (proposed, section 19.2) | L7 | planned: MonoBehaviour JSON goldens | planned (P3) |
| `extractor.exportAudioClip` | one sample written as `{fp}.wav` whatever its format; several into a folder `{fp}/` as `{name}.wav`, where each name already ends in `.wav`, so `x.wav.wav` and `x-1.wav.wav` (`:227-247`) | A | L9 | planned: extractor tree | planned (P3) |
| `extractor.exportSprite` | `sprite.image` saved as `{fp}{extension}`, Pillow choosing the format by extension, with no default: an extension-less path (every sprite in the no-container mode) raises `ValueError` "unknown file extension" (checked in the oracle on UnityPy's samples on 2026-10-28). Returns the sprite and its texture, keyed by the pointer's own file; the alpha texture never, because it reads `assets_file` from a `PPtr`, whose attribute is `assetsfile` (`:250-268`, `:264-267`) | A, C (proposed, section 19.2) | L5b | planned: PNG bytes | planned (P1) |
| `extractor.exportTexture2D` | `obj.image` saved when `m_Width` is non-zero, as `{fp}.png` without an extension, else with the container's extension, which picks Pillow's format: `.tga` writes a TGA, `.psd` raises `KeyError`, `.jpg` raises `OSError` for RGBA (checked against Pillow 9.5.0 on 2026-10-28) (`:271-281`) | A | L5b | planned: PNG bytes | planned (P1) |
| PNG bytes | Pillow 9.5.0 `Image.save` defaults, for the paths that end in `.png` or get it by default: the encoder `unityrs-imageio` matches (`DESIGN.md` section 4) | A | L5b | planned: PNG bytes | planned (P1) |
| `extractor.exportGameObject` | meant to crawl every reachable PPtr and export each non-GameObject into a folder, printing failures (`:284-304`), but it always raises `AttributeError`: `crawl_obj` reads `m_PathID` from the parsed GameObject, which has none (`:350`). In container mode GameObjects sort first, so a container holding one exports nothing. Checked in the oracle on 2026-10-28. | C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.crawl_obj`, `extractor.flatten` | the crawl: PPtrs by path id alone, nested lists, tuples and dicts flattened (`:327-368`); a parsed object raises at `obj.m_PathID` (`:350`), and a null pointer would raise at `obj.m_Index`, which `PPtr` lacks (`:336`) | C (proposed, section 19.2) | L9 | planned: extractor tree | planned (P3) |
| `extractor.MONOBEHAVIOUR_TYPETREES` | an empty dict nothing reads (`:324`) | — | — | — | n/a |

## 16. Audio

`DESIGN.md` section 3 fixes the format list: Ogg, RIFF and m4a pass through; FSB5 PCM, Vorbis, ADPCM and FADPCM decode in pure Rust; MPEG and Opus are optional; anything else is reported unsupported. FMOD is never bundled (`DESIGN.md` section 2). Lines are in `export/AudioClipConverter.py`; fmod_toolkit is 0.1.3. **gap** in the tier column marks a deliberate `DESIGN.md` gap: UnityPy produces output through FMOD, and unityrs reports the format unsupported.

| format | UnityPy 1.25.2 | unityrs | tier | level | test | status |
|---|---|---|---|---|---|---|
| data source | `m_AudioData` (old clips), else `m_Resource` through `ResourceReader`; neither raises `ValueError` (`:21-33`) | same | A | L8 | planned: audio fixtures | planned (P3) |
| Ogg (`OggS`) | returned unchanged as `{name}.ogg` (`:35-37`) | passthrough | A | L8 | planned: audio fixtures | planned (P3) |
| RIFF | returned unchanged as `{name}.wav` (`:38-39`) | passthrough | A | L8 | planned: audio fixtures | planned (P3) |
| m4a (`ftyp` at offset 4) | returned unchanged as `{name}.m4a` (`:40-41`) | passthrough | A | L8 | planned: audio fixtures | planned (P3) |
| anything else | `fmod_toolkit.raw_to_wav` with `m_Channels or 2` and `m_Frequency or 44100`; one WAV per subsound, `{name}.wav` or `{name}-{i}.wav` (`:43-49`) | FSB5 parser | A | L8 | planned: audio fixtures | planned (P3) |
| FSB5 PCM8, PCM16, PCM24, PCM32 | FMOD's samples in a 44-byte RIFF header, as they are | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 PCM float | FMOD's floats converted to int16 with numpy, `(v * 32768).clip(-32768, 32767)` truncated toward zero (fmod_toolkit 0.1.3, `fmod.py` lines 195-198) | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 Vorbis | FMOD decode to PCM | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 IMA ADPCM | FMOD decode to PCM | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 FADPCM | FMOD decode to PCM | pure Rust | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 MPEG | FMOD decode to PCM | optional feature | Tol | L8 | planned: against FMOD | planned (P3) |
| FSB5 Opus | FMOD decode to PCM | optional feature | Tol | L8 | planned: against FMOD | planned (P3) |
| any other codec (XMA, ATRAC9, VAG, HEVAG, GC ADPCM, …) | whatever the bundled `libfmod` decodes; a non-PCM result raises `TypeError`, not the `NotImplementedError` fmod_toolkit means to raise, because its message concatenates a `str` and a `SOUND_FORMAT` enum (fmod_toolkit 0.1.3, `fmod.py` line 139; checked in the oracle on 2026-10-28) | reported unsupported | gap | L8 | planned: error record | planned (P3) |
| tracker and legacy audio (IT, MOD, S3M, XM, AIFF, pre-5 MP3) | FMOD | reported unsupported | gap | L8 | planned: error record | planned (P3) |
| `AudioClipConverter.extract_audioclip_samples` | the dispatcher above (`:13-49`) | same | A | L8 | planned: audio fixtures | planned (P3) |
| `AudioType`, `AudioCompressionFormat` | enums (`enums/Audio.py:4-31`) | same | A | API | planned: binding | planned (P3) |
| `Audio.AUDIO_TYPE_EXTEMSION` | extension table (`enums/Audio.py:33-56`); see `AudioClip.extension` | same | A | API | planned: binding | planned (P3) |

## 17. TextureFormat matrix

One row per `TextureFormat` value (78; 37-40 are unassigned). Lines are in `export/Texture2DConverter.py` unless named; the conversion table is `Texture2DConverter.CONV_TABLE` (`:487-566`).

**Columns.**
- **decoder** and **conversion**: what `parse_image_data` runs and how the bytes become pixels. Pillow's `bcn` is `BcnDecode.c` in Pillow 9.5.0; texture2ddecoder (1.0.5) returns BGRA, which Pillow's `BGRA` raw mode turns into RGBA; ASTC goes through astcenc (astc-encoder-py 0.1.12) with the LDR profile and `USE_DECODE_UNORM8` (`:369-387`). The pixel values quoted were checked in the oracle on 2026-10-28.
- **mode**: the Pillow mode UnityPy returns. Every image is then flipped top to bottom (`:317-318`).
- **gates**: `pad W×H` is the block padding before decoding and the crop after (`Texture2DConverter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE`, `:22-50`, `:314-315`); `no pad` means none. `sw W×H` is the Switch texel block (`TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP`, `helpers/TextureSwizzler.py:73-103`), used when `TextureSwizzler.is_switch_swizzled` holds, in place of the block padding; `sw —` means a swizzled texture of that format raises `KeyError`. `xbox` swaps byte pairs on XBOX360, before anything else, crunch unpacking included (`:274-275`, `:569`). `crunch` unpacks with `unpack_unity_crunch` from 2017.3 and for the two ETC crunched formats at any version, and with `unpack_crunch` before 2017.3 (`:297-306`).
- **UnityPy 1.25.2**: the result of `parse_image_data` on an 8×8 texture of `0x30` bytes, as many as a real 8×8 texture of the format holds, StandaloneWindows64, version 2021.3.1f3. `works` and `wrong` return an image in the listed mode; `wrong` means the pixels are wrong (section 19). `raises` names the exception, module-qualified when it is not a builtin. `crunch` rows need valid crunch data and were not decoded.
- **`set_image` writes**: the format `image_to_texture2d` writes for that target from an RGBA image (`:118-234`).
- **tier** and **status**: section 0. Tier A rows get L5 goldens against the oracle, Tier B rows an independent reference, Tier C rows a pinned divergence test; `DESIGN.md` section 5 puts BC7, DXT1, DXT5, RGBA32 and RGB24 in P1 and every other format in P3.

`test_inventory.py` checks the mode, UnityPy and `set_image` columns of every row against the oracle (texture_matrix_matches_the_oracle).

| value | format | decoder | mode | conversion | gates | UnityPy 1.25.2 | `set_image` writes | tier | status |
|---|---|---|---|---|---|---|---|---|---|
| 1 | `Alpha8` | Pillow raw `A` | RGBA | (0, 0, 0, A) | no pad; sw 16×1 | works (`:489`) | `Alpha8` | A | planned (P3) |
| 2 | `ARGB4444` | Pillow raw `RGBA;4B` | RGBA | bands reordered (2, 1, 0, 3) | no pad; sw 8×1 | works (`:490`) | `RGBA32` | A | planned (P3) |
| 3 | `RGB24` | Pillow raw `RGB` | RGB | as stored | no pad; swizzled data is read as `RGBA32`, so mode RGBA (`:279-283`) | works (`:491`) | `RGB24` | A | planned (P1) |
| 4 | `RGBA32` | Pillow raw `RGBA` | RGBA | as stored | no pad; sw 4×1 | works (`:492`) | `RGBA32` | A | planned (P1) |
| 5 | `ARGB32` | Pillow raw `ARGB` | RGBA | alpha moved last | no pad; sw 4×1 | works (`:493`) | `RGBA32` | A | planned (P3) |
| 6 | `ARGBFloat` | Pillow raw `RGBAF` | — | — | no pad; sw 1×1 | raises `ValueError`, unknown raw mode (`:494`) | `RGBA32` | B | planned (P3) |
| 7 | `RGB565` | Pillow raw `BGR;16` | RGB | red from the top 5 bits; each field widened as `v * 255 / 31` (green `/ 63`), truncated: red 16 gives 131, where bit replication gives 132 | no pad; sw 8×1; xbox | works (`:495`) | `RGB24` | A | planned (P3) |
| 8 | `BGR24` | Pillow raw `BGR` | RGB | blue and red swapped | no pad; swizzled data is read as `BGRA32`, so mode RGBA (`:279-283`) | works (`:496`) | `RGB24` | A | planned (P3) |
| 9 | `R16` | Pillow raw `R;16` into RGB | — | — | no pad; sw 8×1 | raises `ValueError`, unknown raw mode (`:498`) | `R8` | B | planned (P3) |
| 10 | `DXT1` | Pillow `bcn` 1 | RGBA | 1-bit alpha | pad 4×4; sw 8×4; xbox | works (`:500`) | `DXT1` | A | planned (P1) |
| 11 | `DXT3` | Pillow `bcn` 2 | RGBA | explicit 4-bit alpha | pad 4×4; sw — | works (`:501`) | `RGBA32` | A | planned (P3) |
| 12 | `DXT5` | Pillow `bcn` 3 | RGBA | interpolated alpha | pad 4×4; sw 4×4; xbox | works (`:502`) | `DXT5` | A | planned (P1) |
| 13 | `RGBA4444` | Pillow raw `RGBA;4B` | RGBA | bands reordered (3, 2, 1, 0) | no pad; sw 8×1 | works (`:503`) | `RGBA32` | A | planned (P3) |
| 14 | `BGRA32` | Pillow raw `BGRA` | RGBA | blue and red swapped | no pad; sw 4×1 | works (`:504`) | `RGBA32` | A | planned (P3) |
| 15 | `RHalf` | `half` into Pillow mode `R` | — | — | no pad; sw — | raises `struct.error`: `half` reads three halves a pixel for the one stored, so real-sized data runs out; a value outside [0, 1) raises `ValueError` first, and Pillow's missing mode `R` (`ValueError` too) is reached only with three times the data (`:505`, `:446`) | `R8` | B | planned (P3) |
| 16 | `RGHalf` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:506`, `:459-468`) | `RGBA32` | B | planned (P3) |
| 17 | `RGBAHalf` | `half` into Pillow `RGB` | RGB | three halves per pixel for four stored, each `int(h * 256)`: alpha dropped, pixels misaligned (`(.5, .25, .125, .75)` repeated gives `(128, 64, 32)`, `(192, 128, 64)`, `(32, 192, 128)`, …) | no pad; sw — | wrong; a value at 1.0 or above, at -1/256 or below, or NaN raises `ValueError`, an infinity `OverflowError` (`:507`, `:435-447`) | `RGBA32` | C | planned (P3) |
| 18 | `RFloat` | Pillow raw `RF` into RGB | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:508`) | `R8` | B | planned (P3) |
| 19 | `RGFloat` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:509`) | `RGBA32` | B | planned (P3) |
| 20 | `RGBAFloat` | Pillow raw `RGBAF` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:510`) | `RGBA32` | B | planned (P3) |
| 21 | `YUY2` | none: the entry is commented out (`:511`) | — | — | no pad; sw — | raises `NotImplementedError` (`:308-309`) | `RGBA32` | B | planned (P3) |
| 22 | `RGB9e5Float` | `rgb9e5float` | — | — | no pad; sw — | raises `TypeError`, floats into a bytearray (`:512`, `:482`) | `RGB24` | B | planned (P3) |
| 23 | `RGBFloat` | none: no entry | — | — | no pad; sw — | raises `NotImplementedError` (`:308-309`) | `RGBA32` | B | planned (P3) |
| 24 | `BC6H` | Pillow `bcn` 6 | — | — | pad 4×4; sw 4×4 | raises `ValueError`, bad image mode (`:515`) | `RGBA32` | B | planned (P3) |
| 25 | `BC7` | Pillow `bcn` 7 | RGBA | as decoded | pad 4×4; sw 4×4 | works (`:516`) | raises `ValueError` (`:95`); section 19.2 | A | planned (P1) |
| 26 | `BC4` | Pillow `bcn` 4 | L | one channel | pad 4×4; sw 8×4 | works (`:513`) | `BC4` | A | planned (P3) |
| 27 | `BC5` | Pillow `bcn` 5 | RGB | (R, G, 0) | pad 4×4; sw 4×4 | works (`:514`) | `BC5` | A | planned (P3) |
| 28 | `DXT1Crunched` | crunch, then Pillow `bcn` 1 | RGBA | 1-bit alpha | pad 4×4; sw —; xbox; crunch | crunch (`:517`) | `DXT1` | A | planned (P3) |
| 29 | `DXT5Crunched` | crunch, then Pillow `bcn` 3 | RGBA | interpolated alpha | pad 4×4; sw —; xbox; crunch | crunch (`:518`) | `DXT5` | A | planned (P3) |
| 30 | `PVRTC_RGB2` | texture2ddecoder PVRTC 2 bpp | RGBA | BGRA to RGBA | pad 8×4; sw — | works (`:519`) | `RGB24` | A | planned (P3) |
| 31 | `PVRTC_RGBA2` | texture2ddecoder PVRTC 2 bpp | RGBA | BGRA to RGBA | pad 8×4; sw — | works (`:520`) | `RGBA32` | A | planned (P3) |
| 32 | `PVRTC_RGB4` | texture2ddecoder PVRTC 4 bpp | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:521`) | `RGB24` | A | planned (P3) |
| 33 | `PVRTC_RGBA4` | texture2ddecoder PVRTC 4 bpp | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:522`) | `RGBA32` | A | planned (P3) |
| 34 | `ETC_RGB4` | texture2ddecoder ETC1 | RGBA | alpha 255 | pad 4×4; sw — | works (`:523`) | `ETC_RGB4` | A | planned (P3) |
| 35 | `ATC_RGB4` | texture2ddecoder ATC RGB | RGBA | alpha 255 | no pad; sw — | works (`:524`) | `RGB24` | A | planned (P3) |
| 36 | `ATC_RGBA8` | texture2ddecoder ATC RGBA | RGBA | BGRA to RGBA | no pad; sw — | works (`:525`) | `RGBA32` | A | planned (P3) |
| 41 | `EAC_R` | texture2ddecoder EAC R | RGBA | (R, 0, 0, 255) | pad 4×4; sw — | works (`:526`) | `R8` | A | planned (P3) |
| 42 | `EAC_R_SIGNED` | texture2ddecoder EAC R signed | RGBA | (R + 127, 0, 0, 255), -128 clamped to -127: base 100 gives 227, -100 gives 27 | pad 4×4; sw — | works (`:527`) | `R8` | A | planned (P3) |
| 43 | `EAC_RG` | texture2ddecoder EAC RG | RGBA | (R, G, 0, 255) | pad 4×4; sw — | works (`:528`) | `RGBA32` | A | planned (P3) |
| 44 | `EAC_RG_SIGNED` | texture2ddecoder EAC RG signed | RGBA | (R + 127, G + 127, 0, 255) | pad 4×4; sw — | works (`:529`) | `RGBA32` | A | planned (P3) |
| 45 | `ETC2_RGB` | texture2ddecoder ETC2 | RGBA | alpha 255 | pad 4×4; sw — | works (`:530`) | `ETC2_RGB` | A | planned (P3) |
| 46 | `ETC2_RGBA1` | texture2ddecoder ETC2 A1 | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:531`) | `ETC2_RGBA8` | A | planned (P3) |
| 47 | `ETC2_RGBA8` | texture2ddecoder ETC2 A8 | RGBA | BGRA to RGBA | pad 4×4; sw — | works (`:532`) | `ETC2_RGBA8` | A | planned (P3) |
| 48 | `ASTC_RGB_4x4` | astcenc LDR | RGBA | as decoded | pad 4×4; sw 4×4 | works (`:533`) | `ASTC_RGB_4x4` | A | planned (P3) |
| 49 | `ASTC_RGB_5x5` | astcenc LDR | RGBA | as decoded | pad 5×5; sw 5×5 | works (`:534`) | `ASTC_RGB_5x5` | A | planned (P3) |
| 50 | `ASTC_RGB_6x6` | astcenc LDR | RGBA | as decoded | pad 6×6; sw 6×6 | works (`:535`) | `ASTC_RGB_6x6` | A | planned (P3) |
| 51 | `ASTC_RGB_8x8` | astcenc LDR | RGBA | as decoded | pad 8×8; sw 8×8 | works (`:536`) | `ASTC_RGB_8x8` | A | planned (P3) |
| 52 | `ASTC_RGB_10x10` | astcenc LDR | RGBA | as decoded | pad 10×10; sw 10×10 | works (`:537`) | `ASTC_RGB_10x10` | A | planned (P3) |
| 53 | `ASTC_RGB_12x12` | astcenc LDR | RGBA | as decoded | pad 12×12; sw 12×12 | works (`:538`) | `ASTC_RGB_12x12` | A | planned (P3) |
| 54 | `ASTC_RGBA_4x4` | astcenc LDR | RGBA | as decoded | pad 4×4; sw 4×4 | works (`:539`) | `ASTC_RGBA_4x4` | A | planned (P3) |
| 55 | `ASTC_RGBA_5x5` | astcenc LDR | RGBA | as decoded | pad 5×5; sw 5×5 | works (`:540`) | `ASTC_RGBA_5x5` | A | planned (P3) |
| 56 | `ASTC_RGBA_6x6` | astcenc LDR | RGBA | as decoded | pad 6×6; sw 6×6 | works (`:541`) | `ASTC_RGBA_6x6` | A | planned (P3) |
| 57 | `ASTC_RGBA_8x8` | astcenc LDR | RGBA | as decoded | pad 8×8; sw 8×8 | works (`:542`) | `ASTC_RGBA_8x8` | A | planned (P3) |
| 58 | `ASTC_RGBA_10x10` | astcenc LDR | RGBA | as decoded | pad 10×10; sw 10×10 | works (`:543`) | `ASTC_RGBA_10x10` | A | planned (P3) |
| 59 | `ASTC_RGBA_12x12` | astcenc LDR | RGBA | as decoded | pad 12×12; sw 12×12 | works (`:544`) | `ASTC_RGBA_12x12` | A | planned (P3) |
| 60 | `ETC_RGB4_3DS` | texture2ddecoder ETC1 | RGBA | plain ETC1 blocks in plain order; any 3DS tiling or byte order is ignored (section 19.1) | pad 4×4; sw — | wrong (`:545`) | `ETC_RGB4_3DS` holding plain ETC1 | C | planned (P3) |
| 61 | `ETC_RGBA8_3DS` | texture2ddecoder ETC1 | RGBA | the first half of the 16-byte-per-block data read as ETC1 blocks, alpha 255; the alpha is never decoded (section 19.1) | pad 4×4; sw — | wrong (`:546`) | `RGBA32` | C | planned (P3) |
| 62 | `RG16` | `rg` | — | — | no pad; sw 8×1 | raises `KeyError` `'raw'` (`:499`) | `RGBA32` | B | planned (P3) |
| 63 | `R8` | Pillow raw `R` into RGB | RGB | (R, 0, 0) | no pad; sw 16×1 | works (`:497`) | `R8` | A | planned (P3) |
| 64 | `ETC_RGB4Crunched` | crunch, then texture2ddecoder ETC1 | RGBA | alpha 255 | pad 4×4; sw —; crunch | crunch (`:547`) | `ETC_RGB4Crunched` holding plain ETC1; section 19.2 | A | planned (P3) |
| 65 | `ETC2_RGBA8Crunched` | crunch, then texture2ddecoder ETC2 A8 | RGBA | BGRA to RGBA | pad 4×4; sw —; crunch | crunch (`:548`) | `ETC2_RGBA8` | A | planned (P3) |
| 66 | `ASTC_HDR_4x4` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile | pad 4×4; sw — | wrong (`:549`, `:371-377`) | `ASTC_RGBA_4x4` (`ASTC_RGB_4x4` from an RGB image) | C | planned (P3) |
| 67 | `ASTC_HDR_5x5` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile | pad 5×5; sw — | wrong (`:550`) | `ASTC_RGBA_5x5` (`ASTC_RGB_5x5` from an RGB image) | C | planned (P3) |
| 68 | `ASTC_HDR_6x6` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile | pad 6×6; sw — | wrong (`:551`) | `ASTC_RGBA_6x6` (`ASTC_RGB_6x6` from an RGB image) | C | planned (P3) |
| 69 | `ASTC_HDR_8x8` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile | pad 8×8; sw — | wrong (`:552`) | `ASTC_RGBA_8x8` (`ASTC_RGB_8x8` from an RGB image) | C | planned (P3) |
| 70 | `ASTC_HDR_10x10` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile | pad 10×10; sw — | wrong (`:553`) | `ASTC_RGBA_10x10` (`ASTC_RGB_10x10` from an RGB image) | C | planned (P3) |
| 71 | `ASTC_HDR_12x12` | astcenc LDR | RGBA | HDR blocks decoded with the LDR profile | pad 12×12; sw — | wrong (`:554`) | `ASTC_RGBA_12x12` (`ASTC_RGB_12x12` from an RGB image) | C | planned (P3) |
| 72 | `RG32` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:555`) | `RGBA32` | B | planned (P3) |
| 73 | `RGB48` | Pillow raw `RGB;16` | RGB | Pillow reads `RGB;16` as packed 5-6-5, two bytes per pixel for six stored | no pad; sw — | wrong (`:556`) | `RGBA32` | C (proposed, section 19.2) | planned (P3) |
| 74 | `RGBA64` | Pillow raw `RGBA;16` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:557`) | `RGBA32` | B | planned (P3) |
| 75 | `R8_SIGNED` | Pillow raw `R;8s` into mode `R` | — | — | no pad; sw — | raises `ValueError`, unrecognized image mode (`:558`) | `RGBA32` | B | planned (P3) |
| 76 | `RG16_SIGNED` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:559`) | `RGBA32` | B | planned (P3) |
| 77 | `RGB24_SIGNED` | Pillow raw `RGB;8s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:560`) | `RGBA32` | B | planned (P3) |
| 78 | `RGBA32_SIGNED` | Pillow raw `RGBA;8s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:561`) | `RGBA32` | B | planned (P3) |
| 79 | `R16_SIGNED` | Pillow raw `R;16s` into mode `R` | — | — | no pad; sw — | raises `ValueError`, unrecognized image mode (`:562`) | `RGBA32` | B | planned (P3) |
| 80 | `RG32_SIGNED` | `rg` | — | — | no pad; sw — | raises `KeyError` `'raw'` (`:563`) | `RGBA32` | B | planned (P3) |
| 81 | `RGB48_SIGNED` | Pillow raw `RGB;16s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:564`) | `RGBA32` | B | planned (P3) |
| 82 | `RGBA64_SIGNED` | Pillow raw `RGBA;16s` | — | — | no pad; sw — | raises `ValueError`, unknown raw mode (`:565`) | `RGBA32` | B | planned (P3) |

The Tier B count is 22, as `DESIGN.md` section 3 says: `BC6H`; the half and float formats `RHalf`, `RGHalf`, `RFloat`, `RGFloat`, `RGBAFloat`, `ARGBFloat`, `RGBFloat`; the eight `_SIGNED` raw formats; `RG16`, `RG32`, `R16`, `RGBA64`, `RGB9e5Float` and `YUY2`. `RGBAHalf` and `RGB48` return an image, so they are not Tier B.

Two more behaviours apply to every format: a zero width or height returns an empty RGBA image, and empty data raises `ValueError` (`:265-269`); a format number outside the enum raises `ValueError` (`:271-272`).

## 18. Writing

`DESIGN.md` section 3 sets the rules: object re-save byte-equal; file and bundle save semantically equal and idempotent, with an optional UnityPy-layout mode byte-equal to `save(packer='none')`; compressed packers equal after decompression, byte-equal LZ4HC only with liblz4 HC level 9; `get_writeable_cab` with UnityPy's naming and GUID rule; `set_image` equal after re-decode; pre-v10 typetree writing Tier C.

| item | UnityPy 1.25.2 | tier | level | test | status |
|---|---|---|---|---|---|
| object re-save | `ObjectReader.save_typetree` (section 8) | A | L3 | `test_dump.py` resave_aligns_after_every_aligned_array_element | verified P0 (L3); impl P3 |
| `SerializedFile.save` | metadata: version string (v ≥ 7), platform (v ≥ 8), typetree flag (v ≥ 13), types, `big_id_enabled` (7 to 13), objects sorted by path id with each data block aligned to 8 (`files/SerializedFile.py:415-419`), script types (v ≥ 11), externals, ref types (v ≥ 20), user information (v ≥ 5); `packer` is ignored (`files/SerializedFile.py:389-440`) | A | L4 | planned: save suite | planned (P3) |
| header, v ≥ 9 | big-endian header; data offset aligned to 16; v ≥ 22 writes 0 into the legacy metadata size, file size and data offset (the version field keeps the version), then the endian byte, the reserved bytes, the u32 metadata size, the 64-bit file size and data offset, and the file's own unknown i64 back (`files/SerializedFile.py:443-478`) | A | L4 | planned: save suite | planned (P3) |
| header, v < 9 | data right after the 16-byte header, then the endian byte and the metadata, with 32 in the data offset field (`files/SerializedFile.py:480-491`), although the data starts at 16 and the reader adds that field to every object's start (`files/ObjectReader.py:86`). Reachable only with `config.SERIALIZED_FILE_PARSE_TYPETREE` false, since below v10 the tree writer raises (next rows), and then the file lacks the trees a pre-13 reader expects. Part of pre-v10 writing. | C | L4 | planned: divergence test | planned (P3) |
| `SerializedType.write` | class id, stripped flag (v ≥ 16), script index (v ≥ 17), script id and hash (v ≥ 13), the tree, then class names (ref types) or dependencies (v ≥ 21) (`files/SerializedFile.py:151-197`) | A | L4 | planned: save suite | planned (P3) |
| tree as a blob (v ≥ 12, v = 10) | `TypeTreeNode.dump_blob`; the common string at offset 0 (`AABB`) is written as a local string (`helpers/TypeTreeNode.py:237-278`, `:250`) | A, C | L4 | planned: divergence test | planned (P3) |
| pre-v10 and v11 typetree writing | calls `serialized_file.dump`, which does not exist, so it raises `AttributeError` (`files/SerializedFile.py:182-184`); below v13 every file carries trees (`files/SerializedFile.py:241`), so every save of such a file raises unless `config.SERIALIZED_FILE_PARSE_TYPETREE` is false; `TypeTreeNode.dump` itself writes the root's fields for every node (`helpers/TypeTreeNode.py:213-235`) | C | L4 | planned: divergence test | planned (P3) |
| `ObjectReader.write` | object table row; data from `set_raw_data` or the original bytes (`files/ObjectReader.py:132-179`) | A | L4 | planned: save suite | planned (P3) |
| `BundleFile.save` | header, then: `UnityArchive` raises `NotImplementedError`; UnityWeb and UnityRaw format 6 use `save_fs(64, 64)`, other formats `save_web_raw`; UnityFS by packer: `none` (default) `save_fs(64, 64)`, `original` the file's own flags, `lz4` `(194, 2)` (blocks info LZ4 at the end, LZ4 blocks), `lzma` `(65, 1)`, a tuple passed as `(data_flag, block_info_flag)` although the docstring says the reverse, anything else `NotImplementedError` (`files/BundleFile.py:188-236`) | A | L4 | planned: save suite | planned (P3) |
| `BundleFile.save_fs` | `data_flag` is the header's flags (layout bits and the blocks-info codec) and `block_info_flag` the data blocks' codec, despite the names; directory and data rebuilt from `files`; the encryption mask of the file's flag set is XORed out of both flags when any of its bits is set (`files/BundleFile.py:303-307`), so with `ArchiveFlags` (`0x1400`) a flag holding only `0x1000` comes out holding `0x400`, which reads back as encrypted, and the data blocks' own `0x100` bit is never cleared (section 19.2); `CompressionHelper.chunk_based_compress` (`files/BundleFile.py:309`); `0x40` is required (`files/BundleFile.py:325-326`); 16-byte alignment after the header when the read set `_uses_block_alignment` (format ≥ 7, or a 2019.4.15+ engine), then the blocks info before or after the data (`0x80`) with 16-byte padding for `0x200`; the size patched in (`files/BundleFile.py:238-393`) | A, C (proposed, section 19.2) | L4 | planned: save suite | planned (P3) |
| `BundleFile.save_web_raw` | formats 1 to 3 only; above 3 raises `NotImplementedError` (`files/BundleFile.py:395-487`, `:408-409`) | A | L4 | planned: Editor fixture | planned (P4) |
| compressed packers | `CompressionHelper.compress_lz4` is liblz4 HC level 9, so `lz4` output is byte-equal only with that level (section 6) | A | L4 | planned: save suite | planned (P3) |
| `WebFile.save` | `UnityWebData1.0` table and data, then `gzip`, `brotli` or none; the default `packer="none"` never falls back to the file's own packer (`files/WebFile.py:61`, `:67-68`); the signature is the argument's, whatever the file's was (`TuanjieWebData` included); an `EndianBinaryWriter` entry (from `get_writeable_cab`) raises `AttributeError` (`files/WebFile.py:58-112`, `:71`) | A | L4 | planned: save suite | planned (P3) |
| `File.get_writeable_cab` | default name `cab_file`; returns an existing writer, raises `ValueError` for an existing non-writer; copies `flags` and endian from the first `.resS`, else flags 0 (`files/File.py:92-121`) | A | L4 | planned: save suite | planned (P3) |
| `SerializedFile.get_writeable_cab` | `None` unless the parent is a bundle or web file; path `archive:/{name}/{cab}`; registers an external once, GUID `uuid1().urn[-16:]` as ASCII (the last three hex digits of the clock sequence, a hyphen and the 12-digit node id), type 0, empty temp path (`files/SerializedFile.py:362-387`) | A | L4 | planned: save suite (the GUID rule, not its bytes) | planned (P3) |
| `Texture2D.set_image` | the target defaults to `m_TextureFormat`; paths and streams are opened with Pillow; `image_to_texture2d` with the reader's platform and `m_PlatformBlob`; mips halve until a side would drop below 4, each resized BICUBIC from the previous level and written with platform 0, so never Switch-swizzled; sets `m_Width`, `m_Height`, `m_MipMap`, `m_MipCount`, `image_data`, `m_CompleteImageSize` (all levels), `m_TextureFormat`, and empties `m_StreamData` (`classes/legacy_patch/Texture2D.py:14-60`) | A | L5 | planned: `set_image` round trip | planned (P3) |
| `Environment.save`, `File.mark_changed`, `ObjectReader.set_raw_data` | sections 3, 5.2 and 8 | A | L4 | planned: save suite | planned (P3) |

## 19. Divergences

### 19.1 Tier C (`DESIGN.md` section 3)

| divergence | UnityPy 1.25.2 | unityrs | level | test | status |
|---|---|---|---|---|---|
| RGBAHalf returns garbage | three halves read per pixel for four stored, `int(h * 256)` truncated, alpha dropped; a value at 1.0 or above, at -1/256 or below, or NaN raises `ValueError`, an infinity `OverflowError`, so most HDR data raises (`export/Texture2DConverter.py:507`, `:435-447`) | half floats to RGBA | L5 | planned: divergence test | planned (P3) |
| ASTC_HDR is wrong | HDR blocks decoded with the LDR profile into UNORM8 (`export/Texture2DConverter.py:549-554`, `:371-377`) | HDR profile | L5 | planned: divergence test | planned (P3) |
| the UV offset | `uvSrcOffset = uvDim * m_VertexCount` assigns instead of adding, so the third and later UV channels of a compressed mesh start at the wrong offset (`helpers/MeshHelper.py:504`) | `+=` | L6 | planned: divergence test (expected-divergence mask) | planned (P2a) |
| UNorm/SNorm not normalised | UNorm and SNorm channels, and the byte colour formats (`kChannelFormatColor`, `kVertexFormatColor`), are read as raw integers (`enums/VertexFormat.py:46-47`, `:54-58`, `:70-73`, `helpers/MeshHelper.py:392`), while a compressed mesh's colours come out divided by 255 (`helpers/MeshHelper.py:618-628`) | normalised to [0, 1] and [-1, 1] | L6 | planned: divergence test | planned (P1) |
| mesh endianness | `MeshHandler` defaults to `<` and no caller passes the file's endianness, so big-endian vertex data is swapped wrongly (`helpers/MeshHelper.py:73`, `:99`, `:363`; callers `export/MeshExporter.py:18`, `export/SpriteHelper.py:117`) | the file's endianness | L6 | planned: divergence test | planned (P4) |
| the 4th bone weight | `1 - sum` with `sum` counted in 31sts, so the fourth weight is far below zero (`helpers/MeshHelper.py:604`, `:587`, `:591`) | `(31 - sum) / 31` | L6 | planned: divergence test | planned (P2a) |
| ETC_RGB4_3DS, ETC_RGBA8_3DS | decoded as plain ETC1 in plain block order; for ETC_RGBA8_3DS only the first half of its 16-byte-per-block data is read and the alpha is never decoded (`export/Texture2DConverter.py:545-546`). That the 3DS formats store another tiling or byte order is `DESIGN.md`'s premise; no 3DS sample or reference decoder exists here to show it. | 3DS layout and alpha | L5 | planned: divergence test, once a 3DS sample settles the layout | planned (P3) |
| ReferencedObject arrays do not skip None | the array path reads `ReferencedObjectData` with no `None` check, unlike the single-value path (`helpers/TypeTreeHelper.py:300-301` against `:201-203`); the pure-Python reader only | skips | L2 | planned: divergence test (pure-Python path) | planned (P3) |
| dump_blob's common-string offset 0 | `if common_offset:` treats offset 0 (`AABB`) as absent and writes it locally (`helpers/TypeTreeNode.py:250`) | offset 0 is common | L4 | planned: divergence test | planned (P3) |
| MeshRendererExporter fails on Unity 6 | `hasattr` is always true on the merged attrs classes, so an unbatched renderer reaches `min(None)` (`TypeError`) and a renderer without `m_StaticBatchInfo` reads `None.subMeshCount` (`AttributeError`) (`export/MeshRendererExporter.py:56-59`); `export_material` raises `AttributeError` for any material holding `_Color`, `_SColor` or `_SpecularColor`, because `clt` reads `color.R` and `ColorRGBA` has `r g b a` (`:110`), and iterates the dict `texEnvs` as pairs, so any material with a texture property raises `ValueError` (`:117`, `:168`); all three checked in the oracle on 2026-10-28 | exports | L6 | planned: divergence test | planned (P3) |
| ShaderConverter fails on 6000.3 | `ShaderSubProgram` knows sub-program versions up to 202012090, local keywords only in `[201806140, 202012090)` (`export/ShaderConverter.py:577-605`, `:598`), and a property of type Int (5, Unity 2021.1 and later) raises `ValueError` (`export/ShaderConverter.py:369`). On the corpus game (Unity 6000.3; 980 Shader objects, all of their 89,868 sub-programs at blob version 202012090, no Int properties), 946 exports return text and 34 raise `struct.error` reading keywords (`export/ShaderConverter.py:596`), run in the oracle on 2026-10-28; whether the 946 texts are right was not checked. | 6000.3 layout | L7 | planned: divergence test | planned (P3) |
| pre-v10 typetree writing (`DESIGN.md` section 3, Writing) | below v10 and at v11, `SerializedType.write` calls `serialized_file.dump`, which does not exist (`files/SerializedFile.py:182-184`); `TypeTreeNode.dump` writes the root's fields for every node (`helpers/TypeTreeNode.py:218-233`); the v < 9 header names data offset 32 for data at 16 (`files/SerializedFile.py:488-489`); section 18 | writes the old format | L4 | planned: divergence test | planned (P3) |
| the Sprite cache key ignores the FileID | the decoded texture is cached in the sprite's file by `path_id` alone, so two textures with one path id in different files share an entry (`export/SpriteHelper.py:48`, `:55`) | keyed by file and path id | L5 | planned: divergence test | planned (P1) |

### 19.2 Found while writing this inventory

These are not in `DESIGN.md` section 3. Each has a proposed tier; `DESIGN.md` decides.

| defect | UnityPy 1.25.2 | proposed | level | status |
|---|---|---|---|---|
| RGB48 decodes wrong | Pillow's `RGB;16` is packed 5-6-5, two bytes per pixel, so a pure red `(0xFFFF, 0, 0)` decodes as white (`export/Texture2DConverter.py:556`) | C | L5 | planned (P3) |
| `set_image` to BC7 raises | `etcpak.compress_bc7(data, w, h, None)`; etcpak 0.9.14 rejects `None` parameters with `ValueError` (`export/Texture2DConverter.py:95`) | B | L5 | planned (P3) |
| `set_image` to ETC_RGB4Crunched | writes plain ETC1 tagged `ETC_RGB4Crunched`, which the reader then sends to crunch (`export/Texture2DConverter.py:160-165`) | C | L5 | planned (P3) |
| Texture2DArray 8-bit integer formats | `GRAPHICS_TO_TEXTURE_MAP` maps the 8-bit UInt and SInt graphics formats to the 16-bit texture formats (`enums/GraphicsFormat.py:192-199`) | C | L5 | planned (P3) |
| Texture2DArray platform | slices are decoded with platform 0 and no blob, so Switch and Xbox arrays are not deswizzled or swapped (`classes/legacy_patch/Texture2DArray.py:33-40`) | C | L5 | planned (P3) |
| `set_image` mips | mips are written with platform 0, so a Switch texture gets a swizzled top level and linear mips (`classes/legacy_patch/Texture2D.py:44`) | C | L5 | planned (P3) |
| `str(UnityVersion)` | drops the build number above 5.x, and the type and its number at 5.x and below (`helpers/UnityVersion.py:90-94`) | A (API) | API | planned (P3) |
| `GameObject.m_Components` | an empty list raises `IndexError`; the old tuple form reads `m_GameObject` from each component pointer (`classes/legacy_patch/GameObject.py:9-15`) | C | API | planned (P3) |
| `WebFile.save` | an `EndianBinaryWriter` entry raises `AttributeError`; the default packer never reuses the file's own (`files/WebFile.py:61`, `:67-68`, `:71`) | C | L4 | planned (P3) |
| `BundleFile.save` tuple | the docstring says `(block_info_flag, data_flag)`, the code takes `(data_flag, block_info_flag)` (`files/BundleFile.py:193-194`, `:232-233`) | A (code) | L4 | planned (P3) |
| vertex-less mesh export | `export_mesh_obj` returns `False` (`export/MeshExporter.py:22-23`, `:29-30`), so `extractor.exportMesh` raises on `f.write(False)` | A | L6 | planned (P3) |
| one audio sample | `extractor.exportAudioClip` writes a single sample as `.wav` whatever its format (`tools/extractor.py:239-241`) | A | L9 | planned (P3) |
| `ContainerHelper.path_dict` | keyed by path id alone, last entry wins (`helpers/ContainerHelper.py:28`, `:47`) | A | L1 | planned (P1) |
| Texture2DArray streamed data | `image_data` falls back to `m_StreamData` only when it `is None`, but a parsed array's empty TypelessData is `b""`, so a streamed array never reads its `.resS` and every slice raises `ValueError` (`classes/legacy_patch/Texture2DArray.py:20-21`; `Texture2D.get_image_data` tests truthiness, `classes/legacy_patch/Texture2D.py:64`); checked in the oracle on 2026-10-28 | C | L5 | planned (P3) |
| OBJ NaN and infinity | `{:.9G}` prints `NAN` and `INF`, so `.replace("nan", "0")` never fires and the OBJ holds `NAN` (`export/MeshExporter.py:33`, `:39`, `:45`); checked in the oracle on 2026-10-28 | A (the text) | L6 | planned (P3) |
| `BundleFile.save_fs` encryption flags | the flag set's whole encryption mask is XORed out when any bit of it is set, so an `ArchiveFlags` header with only `0x1000` is saved with `0x400`, which UnityPy reads back as encrypted; the data blocks' `0x100` bit survives the `original` packer (`files/BundleFile.py:303-307`, `files/BundleFile.py:123-124`) | C | L4 | planned (P3) |
| `SerializedFile.load_dependencies` | `load_file(path, True)` sets `parent`, not `is_dependency`, so dependencies it loads (a `.resS` lookup miss triggers it) join `Environment.objects` and `Environment.container` (`files/SerializedFile.py:340`, `:347`; `environment.py:100-106`) | C | L1 (env) | planned (P1) |
| `config.SERIALIZED_FILE_PARSE_TYPETREE` | promises faster loading but is applied after the trees are parsed; its only effect is that `save` drops every tree and writes the flag false (`files/SerializedFile.py:310-311`, `:179`, `:405`) | A (save) | L4 | planned (P3) |
| extractor crashes | `exportGameObject` always raises `AttributeError` in `crawl_obj` (`tools/extractor.py:350`), and GameObjects sort first, so a container holding one exports nothing; `exportMonoBehaviour` writes its JSON and then raises `AttributeError` on `obj.path_id` (`tools/extractor.py:224`), and with `export_unknown_as_typetree` it raises before writing; `exportSprite` has no default extension, so every sprite of the no-container mode raises `ValueError` (`tools/extractor.py:250-259`); nothing in `extract_assets` catches, so the first raise ends the run. Checked in the oracle, on UnityPy's samples where they apply, on 2026-10-28. | C | L9 | planned (P3) |
| extractor file names | `append_path_id` appends `_None` (`tools/extractor.py:75`); images, meshes, shaders and text keep the container's extension, which picks Pillow's image format (`.tga` writes TGA, `.psd` raises); several audio samples are written `x.wav.wav`; a later container entry with the same path overwrites an earlier one; the alpha texture is never returned as exported (`tools/extractor.py:264-267`) | A (the names), C (the raises) | L9 | planned (P3) |

## 20. HEAD tier

The HEAD oracle is UnityPy git `417d595`. Lines below are in that tree. unityrs matches HEAD's dicts on Unity 6000.4/6000.5 fixtures (`DESIGN.md` section 3).

| item | UnityPy 1.25.2 | HEAD `417d595` | level | test | status |
|---|---|---|---|---|---|
| SerializedFile v23 | not read | per-type `TypeTreeNodeInfoHandler` in place of the blob (`files/SerializedFile.py:141-144`, `helpers/TypeTreeNode.py:342-394`) | L1, L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| SerializedFile v24, v25 | not read | no gates of their own | L1, L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| SerializedFile v26 | not read | shared subtrees after the ref types, resolved by hash (`files/SerializedFile.py:331-338`) | L1, L2 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| bundle format 9 | not read | a u64 offset per block, used to seek each block (`helpers/CompressionHelper.py:158-172`, `files/BundleFile.py:485-491`) | L1 | planned: 6000.4/6000.5 fixtures | planned (P3) |
| `lz4hc` packer | absent | `save_fs(195, 3)` (`files/BundleFile.py:228-229`) | L4 | planned: save suite | planned (P3) |

## 21. Keeping this file complete

`tools/parity/inventory.py` lists UnityPy's public surface and checks that this file names all of it:

```sh
scripts/dev python tools/parity/inventory.py list [--json]
scripts/dev python tools/parity/inventory.py check
```

| group | what | named here as |
|---|---|---|
| modules | every `.py` file under `UnityPy/` (78) | its path in a code span, alone or with `:line`; a path names its module and nothing else |
| classes | every public class defined in a module (955, the generated classes included) | its bare name in a code span |
| functions | every public module-level function (126) | `Module.function` in a code span |
| methods | public members of the API classes, and every member a `classes/legacy_patch` module adds (133) | `Class.member` in a code span |
| boost | every public name of UnityPyBoost (4) | `UnityPyBoost.name` in a code span |
| texture formats | every `TextureFormat` member (78) | a table row with the cells `` `Name` `` and its value |

`check` exits 1 and lists what is missing. `tools/parity/tests/test_inventory.py` runs it against this file in the oracle image, checks the TextureFormat matrix against the oracle on real-sized data (section 17), and tests the matching rules with the standard library alone.

## 22. Module index

Every module of UnityPy 1.25.2, with the sections that cover it.

| module | what | sections |
|---|---|---|
| `__init__.py` | `load`, `AssetsManager`, `__version__`, `set_assetbundle_decrypt_key` | 2, 12 |
| `__main__.py` | `python -m UnityPy` | 2 |
| `classes/__init__.py` | re-exports the classes | 2, 14 |
| `classes/ClassIDTypeToClassMap.py` | class id to class | 14.1 |
| `classes/Object.py` | `Object` | 14.1 |
| `classes/PPtr.py` | `PPtr` | 14.1 |
| `classes/UnknownObject.py` | `UnknownObject` | 14.1 |
| `classes/generated.py` | the TPK-built classes | 14, Appendix A |
| `classes/math.py` | vectors, quaternion, matrices, colour | 14.1 |
| `classes/legacy_patch/__init__.py` | applies the patches | 14.2 |
| `classes/legacy_patch/AudioClip.py` | `AudioClip.extension`, `AudioClip.samples` | 14.2, 16 |
| `classes/legacy_patch/GameObject.py` | `GameObject.m_*` | 14.2 |
| `classes/legacy_patch/Mesh.py` | `Mesh.export` | 14.2 |
| `classes/legacy_patch/Renderer.py` | `Renderer.export` | 14.2 |
| `classes/legacy_patch/Shader.py` | `Shader.export` | 14.2 |
| `classes/legacy_patch/Sprite.py` | `Sprite.image` | 14.2 |
| `classes/legacy_patch/Texture2D.py` | `Texture2D.image`, `set_image`, `get_image_data` | 14.2, 18 |
| `classes/legacy_patch/Texture2DArray.py` | `Texture2DArray.images` | 14.2 |
| `cli/__init__.py` | the console entry | 2 |
| `cli/update_tpk.py` | downloads a TPK | 2, 10 |
| `config.py` | version fallback, serialized-file parse flag | 10 |
| `environment.py` | `Environment` | 3 |
| `exceptions.py` | the exception classes | 10 |
| `enums/__init__.py` | re-exports the enums | 2 |
| `enums/Audio.py` | audio enums and extensions | 16 |
| `enums/BuildTarget.py` | platforms | 7, 15.1 |
| `enums/BundleFile.py` | archive and block flags | 5.1 |
| `enums/ClassIDType.py` | class ids | 14.1 |
| `enums/CommonString.py` | a fixed common-string table, read only by the never-called `SerializedFile.read_string` | 7.2 |
| `enums/ExtendableEnum.py` | enum base with unknown members | 14.1 |
| `enums/FileType.py` | detection result | 4 |
| `enums/GfxPrimitiveType.py` | primitive types | 15.3 |
| `enums/GraphicsFormat.py` | graphics formats and the array map | 14.2, 19.2 |
| `enums/MeshTopology.py` | topology | 15.3 |
| `enums/PassType.py` | shader passes | 15.5 |
| `enums/SerializedPropertyType.py` | shader properties | 15.5 |
| `enums/ShaderCompilerPlatform.py` | shader platforms | 15.5 |
| `enums/ShaderGpuProgramType.py` | shader program types | 15.5 |
| `enums/SpriteMeshType.py` | sprite mesh type | 15.2 |
| `enums/SpritePackingMode.py` | sprite packing mode | 15.2 |
| `enums/SpritePackingRotation.py` | sprite rotation | 15.2 |
| `enums/TextureDimension.py` | texture dimension | 15.5 |
| `enums/TextureFormat.py` | texture formats | 17 |
| `enums/VertexFormat.py` | vertex formats and struct codes | 15.3, 19.1 |
| `export/__init__.py` | re-exports the exporters | 2 |
| `export/AudioClipConverter.py` | audio samples | 16 |
| `export/MeshExporter.py` | OBJ | 15.4 |
| `export/MeshRendererExporter.py` | OBJ and MTL for renderers | 15.4, 19.1 |
| `export/ShaderConverter.py` | shader text | 15.5 |
| `export/SpriteHelper.py` | sprites | 15.2 |
| `export/Texture2DConverter.py` | texture decode and encode | 15.1, 17 |
| `files/__init__.py` | re-exports the file classes | 2 |
| `files/BundleFile.py` | bundles | 5.1, 18 |
| `files/File.py` | the container base | 5.2, 18 |
| `files/ObjectReader.py` | objects | 8 |
| `files/SerializedFile.py` | serialized files | 7, 18 |
| `files/WebFile.py` | `UnityWebData` files | 5.2, 18 |
| `helpers/__init__.py` | package marker | 2 |
| `helpers/ArchiveStorageManager.py` | UnityCN | 12 |
| `helpers/CompressionHelper.py` | codecs | 6 |
| `helpers/ContainerHelper.py` | containers | 14.3 |
| `helpers/ImportHelper.py` | detection and split files | 4 |
| `helpers/MeshHelper.py` | mesh decoding | 15.3, 19.1 |
| `helpers/PackedBitVector.py` | packed bits | 15.3 |
| `helpers/ResourceReader.py` | `.resS` data | 5.2 |
| `helpers/TextureSwizzler.py` | Switch swizzling | 15.6 |
| `helpers/Tpk.py` | the TPK | 10 |
| `helpers/TypeTreeGenerator.py` | MonoBehaviour trees | 10 |
| `helpers/TypeTreeHelper.py` | typetree values | 9 |
| `helpers/TypeTreeNode.py` | typetree nodes | 9.3, 18 |
| `helpers/UnityVersion.py` | versions | 11 |
| `resources/__init__.py` | bundled resources | 2, 10 |
| `streams/__init__.py` | re-exports the streams | 2, 13 |
| `streams/EndianBinaryReader.py` | readers | 13 |
| `streams/EndianBinaryWriter.py` | writer | 13 |
| `tools/__init__.py` | package marker | 2 |
| `tools/TpkClassGenerator.py` | builds `classes/generated.py` | 10, 14 |
| `tools/extractor.py` | the extractor | 15.7 |

## Appendix A. Generated classes

Every class in `classes/generated.py` (869), with its bases and the class ids `classes/ClassIDTypeToClassMap.py` maps to it. They are read through the typetree: Tier A at L2 through the dict (verified in P0 wherever the samples and the corpus hold them), and at the API level in object mode (planned P3, section 9.2).

| class | bases | class ids |
|---|---|---|
| `AnnotationManager` | Object | 1049 |
| `AssetDatabaseV1` | Object | 1004 |
| `AssetMetaData` | Object | 1028 |
| `AssetServerCache` | Object | 1037 |
| `AudioBuildInfo` | Object | 641289076 |
| `BlockShaderSourceArtifact` | Object | — |
| `BuiltAssetBundleInfoSet` | Object | 668709126 |
| `ContentSummary` | Object | — |
| `Derived` | Object | 1091556383 |
| `SubDerived` | Derived | 367388927 |
| `DifferentMarshallingTestObject` | Object | — |
| `EditorBuildSettings` | Object | 1045 |
| `EditorExtension` | Object, ABC | 18 |
| `Component` | EditorExtension | 2 |
| `Behaviour` | Component | 8 |
| `Animation` | Behaviour | 111 |
| `Animator` | Behaviour | 95 |
| `ArticulationBody` | Behaviour | 171741748 |
| `AudioBehaviour` | Behaviour | 180 |
| `AudioListener` | AudioBehaviour | 81 |
| `AudioSource` | AudioBehaviour | 82 |
| `AudioFilter` | Behaviour | 181 |
| `AudioChorusFilter` | AudioFilter | 166 |
| `AudioDistortionFilter` | AudioFilter | 170 |
| `AudioEchoFilter` | AudioFilter | 168 |
| `AudioHighPassFilter` | AudioFilter | 165 |
| `AudioLowPassFilter` | AudioFilter | 169 |
| `AudioReverbFilter` | AudioFilter | 164 |
| `AudioReverbZone` | Behaviour | 167 |
| `Camera` | Behaviour | 20 |
| `ScriptableCamera` | Camera | 488575907 |
| `Canvas` | Behaviour | 223 |
| `CanvasGroup` | Behaviour | 225 |
| `Cloth` | Behaviour | 183 |
| `InteractiveCloth` | Cloth | 160 |
| `SkinnedCloth` | Cloth | 163 |
| `CloudServiceHandlerBehaviour` | Behaviour | — |
| `Collider2D` | Behaviour | 53 |
| `BoxCollider2D` | Collider2D | 61 |
| `CapsuleCollider2D` | Collider2D | 70 |
| `CircleCollider2D` | Collider2D | 58 |
| `CompositeCollider2D` | Collider2D | 66 |
| `CustomCollider2D` | Collider2D | — |
| `EdgeCollider2D` | Collider2D | 68 |
| `PolygonCollider2D` | Collider2D | 60 |
| `PolygonColliderBase2D` | Collider2D | — |
| `TilemapCollider2D` | Collider2D | 19719996 |
| `ConstantForce` | Behaviour | 75 |
| `Effector2D` | Behaviour | 248 |
| `AreaEffector2D` | Effector2D | 249 |
| `BuoyancyEffector2D` | Effector2D | 253 |
| `PlatformEffector2D` | Effector2D | 251 |
| `PointEffector2D` | Effector2D | 250 |
| `SurfaceEffector2D` | Effector2D | 252 |
| `FlareLayer` | Behaviour | 124 |
| `GUIElement` | Behaviour | 133 |
| `GUIText` | GUIElement | 132 |
| `GUITexture` | GUIElement | 131 |
| `GUILayer` | Behaviour | 92 |
| `GridLayout` | Behaviour | 1742807556 |
| `Grid` | GridLayout | 156049354 |
| `Tilemap` | GridLayout | 1839735485 |
| `Halo` | Behaviour | 122 |
| `HaloLayer` | Behaviour | 125 |
| `IConstraint` | Behaviour | 285090594 |
| `AimConstraint` | IConstraint | 895512359 |
| `LookAtConstraint` | IConstraint | 1183024399 |
| `ParentConstraint` | IConstraint | 1773428102 |
| `PositionConstraint` | IConstraint | 1818360608 |
| `RotationConstraint` | IConstraint | 1818360609 |
| `ScaleConstraint` | IConstraint | 1818360610 |
| `Joint2D` | Behaviour | 230 |
| `AnchoredJoint2D` | Joint2D | 229 |
| `DistanceJoint2D` | AnchoredJoint2D | 232 |
| `FixedJoint2D` | AnchoredJoint2D | 255 |
| `FrictionJoint2D` | AnchoredJoint2D | 256 |
| `HingeJoint2D` | AnchoredJoint2D | 233 |
| `SliderJoint2D` | AnchoredJoint2D | 234 |
| `SpringJoint2D` | AnchoredJoint2D | 231 |
| `WheelJoint2D` | AnchoredJoint2D | 235 |
| `RelativeJoint2D` | Joint2D | 254 |
| `TargetJoint2D` | Joint2D | 257 |
| `LensFlare` | Behaviour | 123 |
| `Light` | Behaviour | 108 |
| `LightProbeGroup` | Behaviour | 220 |
| `LightProbeProxyVolume` | Behaviour | 259 |
| `MonoBehaviour` | Behaviour | 114 |
| `NavMeshAgent` | Behaviour | 195 |
| `NavMeshObstacle` | Behaviour | 208 |
| `NetworkView` | Behaviour | 148 |
| `OffMeshLink` | Behaviour | 191 |
| `ParticleSystemForceField` | Behaviour | 330 |
| `PhysicsUpdateBehaviour2D` | Behaviour | 246 |
| `ConstantForce2D` | PhysicsUpdateBehaviour2D | 247 |
| `PlayableDirector` | Behaviour | 320 |
| `Projector` | Behaviour | 119 |
| `ReflectionProbe` | Behaviour | 215, 216 |
| `Skybox` | Behaviour | 45 |
| `SortingGroup` | Behaviour | 210 |
| `StreamingController` | Behaviour | 1542919678 |
| `Terrain` | Behaviour | 218 |
| `VideoPlayer` | Behaviour | 328 |
| `VisualEffect` | Behaviour | 2083052967 |
| `WindZone` | Behaviour | 182 |
| `CanvasRenderer` | Component | 222 |
| `Collider` | Component | 56 |
| `BoxCollider` | Collider | 65 |
| `CapsuleCollider` | Collider | 136 |
| `CharacterController` | Collider | 143 |
| `MeshCollider` | Collider | 64 |
| `RaycastCollider` | Collider | 140 |
| `SphereCollider` | Collider | 135 |
| `TerrainCollider` | Collider | 154 |
| `WheelCollider` | Collider | 146 |
| `FakeComponent` | Component | 1803986026 |
| `Joint` | Component | 57 |
| `CharacterJoint` | Joint | 144 |
| `ConfigurableJoint` | Joint | 153 |
| `FixedJoint` | Joint | 138 |
| `HingeJoint` | Joint | 59 |
| `SpringJoint` | Joint | 145 |
| `LODGroup` | Component | 205 |
| `MeshFilter` | Component | 33 |
| `MultiplayerRolesData` | Component | — |
| `OcclusionArea` | Component | 192 |
| `OcclusionPortal` | Component | 41 |
| `ParticleAnimator` | Component | 12 |
| `ParticleEmitter` | Component | 88 |
| `EllipsoidParticleEmitter` | ParticleEmitter | 15 |
| `MeshParticleEmitter` | ParticleEmitter | 87 |
| `ParticleSystem` | Component | 198 |
| `Pipeline` | Component | 17 |
| `Renderer` | Component | 25 |
| `BillboardRenderer` | Renderer | 227 |
| `ClothRenderer` | Renderer | 161 |
| `LineRenderer` | Renderer | 120 |
| `RendererFake` | LineRenderer | 646504946 |
| `MeshRenderer` | Renderer | 23 |
| `PanelRenderer` | Renderer | — |
| `ParticleRenderer` | Renderer | 26 |
| `ParticleSystemRenderer` | Renderer | 199 |
| `RenderAs2D` | Renderer | — |
| `SkinnedMeshRenderer` | Renderer | 137 |
| `SpriteMask` | Renderer | 331 |
| `SpriteRenderer` | Renderer | 212 |
| `SpriteShapeRenderer` | Renderer | 1971053207 |
| `TilemapRenderer` | Renderer | 483693784 |
| `TrailRenderer` | Renderer | 96 |
| `UIRenderer` | Renderer | — |
| `VFXRenderer` | Renderer | 73398921 |
| `Rigidbody` | Component | 54 |
| `Rigidbody2D` | Component | 50 |
| `TextMesh` | Component | 102 |
| `Transform` | Component | 4 |
| `RectTransform` | Transform | 224 |
| `Tree` | Component | 193 |
| `WorldAnchor` | Component | 362 |
| `WorldParticleCollider` | Component | 76 |
| `GameObject` | EditorExtension | 1 |
| `NamedObject` | EditorExtension, ABC | 130 |
| `AnimatorState` | NamedObject | 1102 |
| `AnimatorStateMachine` | NamedObject | 1107 |
| `AnimatorTransitionBase` | NamedObject | 1111 |
| `AnimatorStateTransition` | AnimatorTransitionBase | 1101 |
| `AnimatorTransition` | AnimatorTransitionBase | 1109 |
| `AssetBundle` | NamedObject | 142 |
| `AssetBundleManifest` | NamedObject | 290 |
| `AssetImportInProgressProxy` | NamedObject | 369655926 |
| `AssetImporter` | NamedObject, ABC | 1003 |
| `ASTCImporter` | AssetImporter | 1053 |
| `AndroidAssetPackImporter` | AssetImporter | — |
| `AssemblyDefinitionImporter` | AssetImporter | 1766753193 |
| `AssemblyDefinitionReferenceImporter` | AssetImporter | 294290339 |
| `AudioImporter` | AssetImporter | 1020 |
| `BlockShaderImporter` | AssetImporter | — |
| `BuildArchiveImporter` | AssetImporter | — |
| `BuildInstructionImporter` | AssetImporter | — |
| `BuildMetaDataImporter` | AssetImporter | — |
| `C4DImporter` | AssetImporter | — |
| `ComputeShaderImporter` | AssetImporter | 1008 |
| `DDSImporter` | AssetImporter | 1046 |
| `DefaultImporter` | AssetImporter | 1030 |
| `IHVImageFormatImporter` | AssetImporter | 1055 |
| `KTXImporter` | AssetImporter | 1054 |
| `LibraryAssetImporter` | AssetImporter | 1038 |
| `LocalizationImporter` | AssetImporter | 1027052791 |
| `ModelImporter` | AssetImporter, ABC | 1040 |
| `FBXImporter` | ModelImporter | 1041 |
| `Mesh3DSImporter` | ModelImporter | 1005 |
| `SketchUpImporter` | ModelImporter | 1124 |
| `MonoImporter` | AssetImporter | 1035 |
| `MovieImporter` | AssetImporter | 1044 |
| `MultiArtifactTestImporter` | AssetImporter | 1223240404 |
| `NativeFormatImporter` | AssetImporter | 1034 |
| `PVRImporter` | AssetImporter | 1052 |
| `PackageManifestImporter` | AssetImporter | 1896753126 |
| `PluginImporter` | AssetImporter | 1050 |
| `PrefabImporter` | AssetImporter | 468431735 |
| `PreviewImporter` | AssetImporter | — |
| `RayTracingShaderImporter` | AssetImporter | 747330370 |
| `ReferencesArtifactGenerator` | AssetImporter | 1114811875 |
| `RoslynAdditionalFileImporter` | AssetImporter | — |
| `RoslynAnalyzerConfigImporter` | AssetImporter | — |
| `RuleSetFileImporter` | AssetImporter | — |
| `ScriptedImporter` | AssetImporter | 2089858483 |
| `ShaderImporter` | AssetImporter | 1007 |
| `ShaderIncludeImporter` | AssetImporter | — |
| `SpeedTreeImporter` | AssetImporter | 1110 |
| `SpriteAtlasImporter` | AssetImporter | 1210832254 |
| `StyleSheetImporter` | AssetImporter | — |
| `SubstanceImporter` | AssetImporter | 1112 |
| `TextScriptImporter` | AssetImporter | 1031 |
| `TextureImporter` | AssetImporter | 1006 |
| `TrueTypeFontImporter` | AssetImporter | 1042 |
| `VideoClipImporter` | AssetImporter | 1127 |
| `VisualEffectImporter` | AssetImporter | 2058629510 |
| `AudioContainerElement` | NamedObject | — |
| `AudioMixer` | NamedObject | 240 |
| `AudioMixerController` | AudioMixer | 241 |
| `AudioMixerEffectController` | NamedObject | 244 |
| `AudioMixerGroup` | NamedObject | 273 |
| `AudioMixerGroupController` | AudioMixerGroup | 243 |
| `AudioMixerSnapshot` | NamedObject | 272 |
| `AudioMixerSnapshotController` | AudioMixerSnapshot | 245 |
| `AudioResource` | NamedObject | — |
| `AudioRandomContainer` | AudioResource | — |
| `SampleClip` | AudioResource | 271 |
| `AudioClip` | SampleClip | 83 |
| `Avatar` | NamedObject | 90 |
| `AvatarMask` | NamedObject | 319 |
| `AvatarSkeletonMask` | NamedObject | — |
| `BaseAnimationTrack` | NamedObject, ABC | 110 |
| `NewAnimationTrack` | BaseAnimationTrack | 118 |
| `BillboardAsset` | NamedObject | 226 |
| `BlobObject` | NamedObject | — |
| `BlockShaderContainer` | NamedObject | — |
| `BlockShaderErrors` | NamedObject | — |
| `BlockShaderSyntaxTree` | NamedObject | — |
| `BuildReport` | NamedObject | 1125 |
| `CachedSpriteAtlas` | NamedObject | 214 |
| `CachedSpriteAtlasRuntimeData` | NamedObject | 644342135 |
| `ComputeShader` | NamedObject | 72 |
| `D3D12DeviceFilterLists` | NamedObject | — |
| `DefaultAsset` | NamedObject | 1029 |
| `BrokenPrefabAsset` | DefaultAsset | — |
| `SceneAsset` | DefaultAsset | 1032 |
| `EditorProjectAccess` | NamedObject | 426301858 |
| `Flare` | NamedObject | 121 |
| `Font` | NamedObject | 128 |
| `GameObjectRecorder` | NamedObject | 1268269756 |
| `GraphicsStateCollection` | NamedObject | — |
| `HumanTemplate` | NamedObject | 1105 |
| `ImportLog` | NamedObject | — |
| `LightProbes` | NamedObject | 197, 258 |
| `LightingDataAsset` | NamedObject | 1120 |
| `LightingDataAssetParent` | NamedObject | 1325145578 |
| `LightingSettings` | NamedObject | 850595691 |
| `LightmapParameters` | NamedObject | 1113 |
| `LocalizationAsset` | NamedObject | 2083778819 |
| `Material` | NamedObject | 21 |
| `ProceduralMaterial` | Material | 185 |
| `Mesh` | NamedObject | 43 |
| `Motion` | NamedObject, ABC | 207 |
| `AnimationClip` | Motion | 74 |
| `PreviewAnimationClip` | AnimationClip | 1108 |
| `BlendTree` | Motion | 206 |
| `NavMeshData` | NamedObject | 238 |
| `NavMeshObsolete` | NamedObject | 194 |
| `OcclusionCullingData` | NamedObject | 363 |
| `PhysicsMaterial` | NamedObject | — |
| `PhysicsMaterial2D` | NamedObject | 62 |
| `PreloadData` | NamedObject | 150 |
| `Preset` | NamedObject | 181963792 |
| `RayTracingShader` | NamedObject | 825902497 |
| `RoslynAdditionalFileAsset` | NamedObject | — |
| `RoslynAnalyzerConfigAsset` | NamedObject | — |
| `RuntimeAnimatorController` | NamedObject | 93 |
| `AnimatorController` | RuntimeAnimatorController | 91 |
| `AnimatorOverrideController` | RuntimeAnimatorController | 221 |
| `Shader` | NamedObject | 48 |
| `ShaderVariantCollection` | NamedObject | 200 |
| `SpeedTreeWindAsset` | NamedObject | 228 |
| `Sprite` | NamedObject | 213 |
| `SpriteAtlas` | NamedObject | 687078895 |
| `SpriteAtlasAsset` | NamedObject | 612988286 |
| `SubstanceArchive` | NamedObject | 184 |
| `TerrainData` | NamedObject | 156 |
| `TerrainLayer` | NamedObject | 1953259897 |
| `TextAsset` | NamedObject | 49 |
| `AssemblyDefinitionAsset` | TextAsset | 1152215463 |
| `AssemblyDefinitionReferenceAsset` | TextAsset | 662584278 |
| `MonoScript` | TextAsset | 115 |
| `PackageManifest` | TextAsset | 1896753125 |
| `RuleSetFileAsset` | TextAsset | — |
| `ShaderInclude` | TextAsset | — |
| `Texture` | NamedObject, ABC | 27 |
| `BaseVideoTexture` | Texture, ABC | 237 |
| `WebCamTexture` | BaseVideoTexture | 158 |
| `CubemapArray` | Texture | 188 |
| `LowerResBlitTexture` | Texture | 1480428607 |
| `MovieTexture` | Texture | 152 |
| `ProceduralTexture` | Texture | 186 |
| `RenderTexture` | Texture | 84 |
| `CustomRenderTexture` | RenderTexture | 86 |
| `SparseTexture` | Texture | 171 |
| `Texture2D` | Texture | 28 |
| `Cubemap` | Texture2D | 89 |
| `Texture2DArray` | Texture | 187 |
| `Texture3D` | Texture | 117 |
| `VideoClip` | NamedObject | 329 |
| `VisualEffectObject` | NamedObject, ABC | 2059678085 |
| `VisualEffectAsset` | VisualEffectObject | 2058629509 |
| `VisualEffectSubgraph` | VisualEffectObject, ABC | 994735392 |
| `VisualEffectSubgraphBlock` | VisualEffectSubgraph | 994735404 |
| `VisualEffectSubgraphOperator` | VisualEffectSubgraph | 994735403 |
| `VisualEffectResource` | NamedObject | 2058629511 |
| `VulkanDeviceFilterLists` | NamedObject | — |
| `EditorExtensionImpl` | Object | 1002 |
| `EditorSettings` | Object | 159 |
| `EditorUserBuildSettings` | Object | 1051 |
| `EditorUserSettings` | Object | 162 |
| `EmptyObject` | Object | 277625683 |
| `GUIDSerializer` | Object | 1027 |
| `GameManager` | Object, ABC | 9 |
| `GlobalGameManager` | GameManager, ABC | 6 |
| `AnimationManager` | GlobalGameManager | — |
| `AudioManager` | GlobalGameManager | 11 |
| `BuildSettings` | GlobalGameManager | 141 |
| `CloudWebServicesManager` | GlobalGameManager | 301 |
| `ClusterInputManager` | GlobalGameManager | 236 |
| `CrashReportManager` | GlobalGameManager | 304 |
| `DelayedCallManager` | GlobalGameManager | 98 |
| `GraphicsSettings` | GlobalGameManager | 30 |
| `InputManager` | GlobalGameManager | 13 |
| `MasterServerInterface` | GlobalGameManager | 155 |
| `MonoManager` | GlobalGameManager | 116 |
| `MultiplayerManager` | GlobalGameManager | — |
| `NavMeshProjectSettings` | GlobalGameManager | 126 |
| `NetworkManager` | GlobalGameManager | 149 |
| `NotificationManager` | GlobalGameManager | — |
| `PerformanceReportingManager` | GlobalGameManager | 305 |
| `Physics2DSettings` | GlobalGameManager | 19, 51 |
| `PhysicsManager` | GlobalGameManager | 55 |
| `PlayerSettings` | GlobalGameManager | 129 |
| `QualitySettings` | GlobalGameManager | 47 |
| `ResourceManager` | GlobalGameManager | 147 |
| `RuntimeInitializeOnLoadManager` | GlobalGameManager | 300 |
| `ShaderNameRegistry` | GlobalGameManager | — |
| `StreamingManager` | GlobalGameManager | 1403656975 |
| `TagManager` | GlobalGameManager | 78 |
| `TimeManager` | GlobalGameManager | 5 |
| `UnityAdsManager` | GlobalGameManager | 292 |
| `UnityAnalyticsManager` | GlobalGameManager | 303 |
| `UnityConnectSettings` | GlobalGameManager | 310 |
| `VFXManager` | GlobalGameManager | 937362698 |
| `LevelGameManager` | GameManager, ABC | 3 |
| `HaloManager` | LevelGameManager | 127 |
| `LightmapSettings` | LevelGameManager | 157 |
| `NavMeshSettings` | LevelGameManager | 196 |
| `OcclusionCullingSettings` | LevelGameManager | 29 |
| `RenderSettings` | LevelGameManager | 104 |
| `HierarchyState` | Object | 1026 |
| `InspectorExpandedState` | Object | 1048 |
| `MarshallingTestObject` | Object | — |
| `MemorySettings` | Object | — |
| `NScreenBridge` | Object | 280 |
| `NativeObjectType` | Object | 1977754360 |
| `PackedAssets` | Object | 1126 |
| `PlatformModuleSetup` | Object | 877146078 |
| `PluginBuildInfo` | Object | 382020655 |
| `Prefab` | Object | — |
| `PrefabInstance` | Object | 1001 |
| `PresetManager` | Object | 1386491679 |
| `PropertyModificationsTargetTestObject` | Object | 1111377672 |
| `RenderPassAttachment` | Object | 1571458007 |
| `SceneRoots` | Object | — |
| `SceneVisibilityState` | Object | 1154873562 |
| `ScenesUsingAssets` | Object | 156483287 |
| `SerializableManagedHost` | Object | 1995898324 |
| `SerializableManagedRefTestClass` | Object | 76251197 |
| `ShaderContainer` | Object | — |
| `SiblingDerived` | Object | 334799969 |
| `SpriteAtlasDatabase` | Object | 638013454 |
| `TestObjectVectorPairStringBool` | Object | 1628831178 |
| `TestObjectWithSerializedAnimationCurve` | Object | 478637459 |
| `TestObjectWithSerializedArray` | Object | 478637458 |
| `TestObjectWithSerializedMapStringBool` | Object | 1981279845 |
| `TestObjectWithSerializedMapStringNonAlignedStruct` | Object | 342846651 |
| `TestObjectWithSpecialLayoutOne` | Object | 293259124 |
| `TestObjectWithSpecialLayoutTwo` | Object | 1392443030 |
| `TilemapEditorUserSettings` | Object | — |
| `VersionControlSettings` | Object | 890905787 |
| `VideoBuildInfo` | Object | — |
| `AABB` | object | — |
| `AddedComponent` | object | — |
| `AddedGameObject` | object | — |
| `AndroidDeviceFilterData` | object | — |
| `AnimationClipBindingConstant` | object | — |
| `AnimationClipOverride` | object | — |
| `AnimationCurve` | object | — |
| `AnimationEvent` | object | — |
| `AnimatorCondition` | object | — |
| `Annotation` | object | — |
| `ArticulationDrive` | object | — |
| `AspectRatios` | object | — |
| `AssemblyJsonAsset` | TextAsset | — |
| `AssemblyJsonImporter` | AssetImporter | — |
| `Asset` | object | — |
| `AssetBundleFullName` | object | — |
| `AssetBundleInfo` | object | — |
| `AssetBundleScriptInfo` | object | — |
| `AssetDatabase` | Object | — |
| `AssetDatabaseMetrics` | object | — |
| `AssetImporterHashKey` | object | — |
| `AssetImporterLog` | NamedObject | 41386430 |
| `AssetImporter_ImportError` | object | — |
| `AssetInfo` | object | — |
| `AssetLabels` | object | — |
| `AssetStats` | object | — |
| `AssetTimeStamp` | object | — |
| `AttachmentIndexArray` | object | — |
| `AttachmentInfo` | object | — |
| `AudioImporterOutput` | object | — |
| `AudioMixerConstant` | object | — |
| `AudioMixerLiveUpdateBool` | ABC | 100009 |
| `AudioMixerLiveUpdateFloat` | ABC | 100008 |
| `AutoOffMeshLinkData` | object | — |
| `AvatarBodyMask` | NamedObject | — |
| `AvatarConstant` | object | — |
| `AvatarSkeletonMaskElement` | object | — |
| `Axes` | object | — |
| `BitField` | object | — |
| `Blend1dDataConstant` | object | — |
| `Blend2dDataConstant` | object | — |
| `BlendDirectDataConstant` | object | — |
| `BlendShapeData` | object | — |
| `BlendShapeVertex` | object | — |
| `BlendTreeConstant` | object | — |
| `BlendTreeNodeConstant` | object | — |
| `BoneInfluence` | object | — |
| `BoneWeights4` | object | — |
| `BranchWindLevel` | object | — |
| `BufferBinding` | object | — |
| `BuildReportFile` | object | — |
| `BuildReportPackedAssetInfo` | object | — |
| `BuildReportScenesUsingAsset` | object | — |
| `BuildStepInfo` | object | — |
| `BuildStepMessage` | object | — |
| `BuildSummary` | object | — |
| `BuildTargetSettings` | object | — |
| `BuildTextureStackReference` | object | — |
| `BuiltAssetBundleInfo` | object | — |
| `BuiltinShaderSettings` | object | — |
| `CGProgram` | TextAsset | 109 |
| `CachedAssetMetaData` | object | — |
| `Channel` | object | — |
| `ChannelInfo` | object | — |
| `CharacterInfo` | object | — |
| `Child` | object | — |
| `ChildAnimatorState` | object | — |
| `ChildAnimatorStateMachine` | object | — |
| `ChildMotion` | object | — |
| `ClampVelocityModule` | object | — |
| `ClassInfo` | object | — |
| `ClassMethodInfo` | object | — |
| `Clip` | object | — |
| `ClipAnimationInfo` | object | — |
| `ClipAnimationInfoCurve` | object | — |
| `ClipMuscleConstant` | object | — |
| `ClothAttachment` | object | — |
| `ClothConstrainCoefficients` | object | — |
| `ClothSphereColliderPair` | object | — |
| `ClusterInput` | object | — |
| `CollabEditorSettings` | object | — |
| `Collision` | ABC | 100004 |
| `Collision2D` | ABC | 100007 |
| `CollisionModule` | object | — |
| `ColorBySpeedModule` | object | — |
| `ColorModule` | object | — |
| `ComponentPair` | object | — |
| `CompressedAnimationCurve` | object | — |
| `CompressedMesh` | object | — |
| `ComputeBufferCounter` | object | — |
| `ComputeShaderBuiltinSampler` | object | — |
| `ComputeShaderCB` | object | — |
| `ComputeShaderKernel` | object | — |
| `ComputeShaderKernelParent` | object | — |
| `ComputeShaderParam` | object | — |
| `ComputeShaderPlatformVariant` | object | — |
| `ComputeShaderResource` | object | — |
| `ComputeShaderVariant` | object | — |
| `Condition` | object | — |
| `ConditionConstant` | object | — |
| `ConfigSetting` | object | — |
| `ConstantBuffer` | object | — |
| `ConstantClip` | object | — |
| `ConstraintSource` | object | — |
| `ControllerConstant` | object | — |
| `CrashReportingSettings` | object | — |
| `CustomDataModule` | object | — |
| `D3D12DeviceFilterData` | object | — |
| `D3D12GraphicsJobsDeviceFilterData` | object | — |
| `DataTemplate` | NamedObject | — |
| `DateTime` | object | — |
| `DefaultPreset` | object | — |
| `DefaultPresetList` | object | — |
| `DeletedItem` | object | — |
| `DenseClip` | object | — |
| `DetailDatabase` | object | — |
| `DetailPatch` | object | — |
| `DetailPrototype` | object | — |
| `DeviceNone` | object | — |
| `DirectorGenericBinding` | object | — |
| `DirectorPlayer` | Behaviour | — |
| `EffectConstant` | object | — |
| `EmbeddedNativeType` | object | — |
| `EmissionModule` | object | — |
| `EnlightenRendererInformation` | object | — |
| `EnlightenSceneMapping` | object | — |
| `EnlightenSystemAtlasInformation` | object | — |
| `EnlightenSystemInformation` | object | — |
| `EnlightenTerrainChunksInformation` | object | — |
| `EntityId` | object | — |
| `Error` | object | — |
| `ExpandedData` | object | — |
| `ExposedReferenceTable` | object | — |
| `Expression` | object | — |
| `ExtensionPropertyValue` | object | — |
| `ExternalForcesModule` | object | — |
| `FalloffTable` | object | — |
| `FastPropertyName` | object | — |
| `FlareElement` | object | — |
| `FloatCurve` | object | — |
| `ForceModule` | object | — |
| `GISettings` | object | — |
| `GLTextureSettings` | object | — |
| `GUID` | object | — |
| `GenericBinding` | object | — |
| `GfxBlendState` | object | — |
| `GfxDepthState` | object | — |
| `GfxRasterState` | object | — |
| `GfxRenderTargetBlendState` | object | — |
| `GfxStencilState` | object | — |
| `Google` | object | — |
| `Gradient` | object | — |
| `GradientNEW` | object | — |
| `GraphicsStateInfo` | object | — |
| `GroupConnection` | object | — |
| `GroupConstant` | object | — |
| `Hand` | object | — |
| `HandPose` | object | — |
| `Handle` | object | — |
| `Hash128` | object | — |
| `HeightMeshBVNode` | object | — |
| `HeightMeshData` | object | — |
| `Heightmap` | object | — |
| `HeightmapData` | object | — |
| `HierarchicalSceneData` | object | — |
| `HoloLens` | object | — |
| `Human` | object | — |
| `HumanBone` | object | — |
| `HumanDescription` | object | — |
| `HumanGoal` | object | — |
| `HumanHandle` | object | — |
| `HumanLayerConstant` | object | — |
| `HumanPose` | object | — |
| `HumanPoseMask` | object | — |
| `Image` | object | — |
| `ImportLog_ImportLogEntry` | object | — |
| `InheritVelocityModule` | object | — |
| `InitialModule` | object | — |
| `InputAxis` | object | — |
| `InputImportSettings` | object | — |
| `InsightsSettings` | object | — |
| `IntPoint` | object | — |
| `Item` | object | — |
| `JointAngleLimit2D` | object | — |
| `JointAngleLimits2D` | object | — |
| `JointDrive` | object | — |
| `JointLimits` | object | — |
| `JointMotor` | object | — |
| `JointMotor2D` | object | — |
| `JointSpring` | object | — |
| `JointSuspension2D` | object | — |
| `JointTranslationLimits2D` | object | — |
| `Keyframe` | object | — |
| `LOD` | object | — |
| `LODRenderer` | object | — |
| `LayerConstant` | object | — |
| `LayoutDataOne` | object | — |
| `LayoutDataThree` | object | — |
| `LayoutDataTwo` | object | — |
| `LeafInfoConstant` | object | — |
| `LibraryRepresentation` | object | — |
| `LifetimeByEmitterSpeedModule` | object | — |
| `LightBakingOutput` | object | — |
| `LightProbeData` | object | — |
| `LightProbeOcclusion` | object | — |
| `LightmapBakeMode` | object | — |
| `LightmapData` | object | — |
| `LightmapSnapshot` | NamedObject | — |
| `LightsModule` | object | — |
| `Limit` | object | — |
| `LineParameters` | object | — |
| `LodSelectionCurve` | object | — |
| `Lumin` | object | — |
| `MaterialImportOutput` | object | — |
| `MaterialInstanceSettings` | object | — |
| `MatrixParameter` | object | — |
| `MdFour` | object | — |
| `MeshBlendShape` | object | — |
| `MeshBlendShapeChannel` | object | — |
| `MeshBlendShapeVertex` | object | — |
| `MeshLodInfo` | object | — |
| `MeshLodRange` | object | — |
| `MeshLodSubMesh` | object | — |
| `MinMaxAABB` | object | — |
| `MinMaxCurve` | object | — |
| `MinMaxGradient` | object | — |
| `MipmapLimitSettings` | object | — |
| `Module` | object | — |
| `MonoAssemblyImporter` | AssetImporter | — |
| `MonoObject` | ABC | 100003 |
| `MotionNeighborList` | object | — |
| `MultiModeParameter` | object | — |
| `NameToObjectMap` | object | — |
| `NativeType` | object | — |
| `NavMesh` | NamedObject | — |
| `NavMeshAreaData` | object | — |
| `NavMeshAreas` | GlobalGameManager | — |
| `NavMeshBuildDebugSettings` | object | — |
| `NavMeshBuildSettings` | object | — |
| `NavMeshLayerData` | object | — |
| `NavMeshLayers` | GlobalGameManager | — |
| `NavMeshParams` | object | — |
| `NavMeshTileData` | object | — |
| `NetworkViewID` | object | — |
| `Node` | object | — |
| `NoiseModule` | object | — |
| `NonAlignedStruct` | object | — |
| `ObjectRolePair` | object | — |
| `OcclusionScene` | object | — |
| `Oculus` | object | — |
| `OffsetPtr` | object | — |
| `Output` | object | — |
| `PPtrCurve` | object | — |
| `PPtrKeyframe` | object | — |
| `PackedBitVector` | object | — |
| `PackingSettings` | object | — |
| `Parameter` | object | — |
| `ParserBindChannels` | object | — |
| `ParticleSystemEmissionBurst` | object | — |
| `ParticleSystemForceFieldParameters` | object | — |
| `PerLODSettings` | object | — |
| `PerformanceReportingSettings` | object | — |
| `PhysicMaterial` | NamedObject | 134 |
| `PhysicsJobOptions2D` | object | — |
| `PhysicsShape` | object | — |
| `PhysicsShapeGroup2D` | object | — |
| `PlatformSettings` | object | — |
| `PlatformSettingsData` | object | — |
| `PlatformShaderDefines` | object | — |
| `PlatformShaderSettings` | object | — |
| `PluginImportOutput` | object | — |
| `Polygon2D` | ABC | 100010 |
| `PrefabModification` | object | — |
| `PresetType` | object | — |
| `PreviewData` | object | — |
| `ProbeSetIndex` | object | — |
| `ProbeSetTetrahedralization` | object | — |
| `ProceduralMaterialInformation` | object | — |
| `ProceduralTextureAssignment` | object | — |
| `PropertyModification` | object | — |
| `PropertyModificationsTargetTestNativeObject` | object | — |
| `QualitySetting` | object | — |
| `QuaternionCurve` | object | — |
| `RationalTime` | object | — |
| `RayTracingShaderBuiltinSampler` | object | — |
| `RayTracingShaderConstantBuffer` | object | — |
| `RayTracingShaderFunctionDesc` | object | — |
| `RayTracingShaderID` | object | — |
| `RayTracingShaderParam` | object | — |
| `RayTracingShaderPlatformVariant` | object | — |
| `RayTracingShaderReflectionData` | object | — |
| `RayTracingShaderResource` | object | — |
| `RayTracingShaderResources` | object | — |
| `RayTracingShaderVariant` | object | — |
| `Rectf` | object | — |
| `RenderManager` | GlobalGameManager | — |
| `RenderPassInfo` | object | — |
| `RenderStateBlock` | object | — |
| `RenderStateInfo` | object | — |
| `RendererData` | object | — |
| `ResourceManager_Dependency` | object | — |
| `RippleGroup` | object | — |
| `RootMotionData` | ABC | 100006 |
| `RotationBySpeedModule` | object | — |
| `RotationModule` | object | — |
| `SBranchWindLevel` | object | — |
| `SParams` | object | — |
| `SWindGroup` | object | — |
| `SampleSettings` | object | — |
| `SamplerParameter` | object | — |
| `Scene` | LevelGameManager | — |
| `SceneDataContainer` | object | — |
| `SceneIdentifier` | object | — |
| `SceneObjectIdentifier` | object | — |
| `SceneSettings` | LevelGameManager | — |
| `SceneVisibilityData` | object | — |
| `ScriptMapper` | GlobalGameManager | 94 |
| `SecondarySpriteTexture` | object | — |
| `SecondaryTextureSettings` | object | — |
| `SelectorStateConstant` | object | — |
| `SelectorTransitionConstant` | object | — |
| `SerializedCustomEditorForRenderPipeline` | object | — |
| `SerializedPass` | object | — |
| `SerializedPlayerSubProgram` | object | — |
| `SerializedProgram` | object | — |
| `SerializedProgramParameters` | object | — |
| `SerializedProperties` | object | — |
| `SerializedProperty` | object | — |
| `SerializedShader` | object | — |
| `SerializedShaderDependency` | object | — |
| `SerializedShaderFloatValue` | object | — |
| `SerializedShaderRTBlendState` | object | — |
| `SerializedShaderState` | object | — |
| `SerializedShaderVectorValue` | object | — |
| `SerializedStencilOp` | object | — |
| `SerializedSubProgram` | object | — |
| `SerializedSubShader` | object | — |
| `SerializedTagMap` | object | — |
| `SerializedTextureProperty` | object | — |
| `ShaderBindChannel` | object | — |
| `ShaderInfo` | object | — |
| `ShadowSettings` | object | — |
| `ShapeModule` | object | — |
| `SizeBySpeedModule` | object | — |
| `SizeModule` | object | — |
| `Skeleton` | object | — |
| `SkeletonBone` | object | — |
| `SkeletonBoneLimit` | object | — |
| `SkeletonMask` | object | — |
| `SkeletonMaskElement` | object | — |
| `SkeletonPose` | object | — |
| `SketchUpImportCamera` | object | — |
| `SketchUpImportData` | object | — |
| `SketchUpImportScene` | object | — |
| `SnapshotConstant` | object | — |
| `SoftJointLimit` | object | — |
| `SoftJointLimitSpring` | object | — |
| `SortingLayerEntry` | object | — |
| `SourceAssetIdentifier` | object | — |
| `SourceTextureInformation` | object | — |
| `SpeedTreeWind` | object | — |
| `SpeedTreeWindConfig8` | object | — |
| `SpeedTreeWindConfig9` | object | — |
| `SphericalHarmonicsL2` | object | — |
| `SplashScreenLogo` | object | — |
| `SplatDatabase` | object | — |
| `SplatPrototype` | object | — |
| `SpriteAtlasAssetData` | object | — |
| `SpriteAtlasData` | object | — |
| `SpriteAtlasEditorData` | object | — |
| `SpriteBone` | object | — |
| `SpriteCustomDataEntry` | object | — |
| `SpriteCustomMetadata` | object | — |
| `SpriteData` | object | — |
| `SpriteMetaData` | object | — |
| `SpriteRenderData` | object | — |
| `SpriteSheetMetaData` | object | — |
| `SpriteTilingProperty` | object | — |
| `SpriteVertex` | object | — |
| `State` | NamedObject | — |
| `StateConstant` | object | — |
| `StateKey` | object | — |
| `StateMachine` | NamedObject | — |
| `StateMachineBehaviourVectorDescription` | object | — |
| `StateMachineConstant` | object | — |
| `StateRange` | object | — |
| `StaticBatchInfo` | object | — |
| `StreamInfo` | object | — |
| `StreamedClip` | object | — |
| `StreamedResource` | object | — |
| `StreamingInfo` | object | — |
| `StructParameter` | object | — |
| `SubCollider` | object | — |
| `SubEmitterData` | object | — |
| `SubMesh` | object | — |
| `SubModule` | object | — |
| `SubPassDescriptor` | object | — |
| `SubstanceEnumItem` | object | — |
| `SubstanceInput` | object | — |
| `SubstanceValue` | object | — |
| `TakeInfo` | object | — |
| `Tetrahedron` | object | — |
| `TextureImportInstructions` | object | — |
| `TextureImportOutput` | object | — |
| `TextureImporterPlatformSettings` | object | — |
| `TextureParameter` | object | — |
| `TextureParameters` | object | — |
| `TextureSettings` | object | — |
| `TicksPerSecond` | object | — |
| `TierGraphicsSettings` | object | — |
| `Tile` | object | — |
| `TileAnimationData` | object | — |
| `TilemapRefCountedData` | object | — |
| `TrailModule` | object | — |
| `TransformMaskElement` | object | — |
| `Transition` | NamedObject | — |
| `TransitionConstant` | object | — |
| `TreeInstance` | object | — |
| `TreePrototype` | object | — |
| `TriggerModule` | object | — |
| `TypeStats` | object | — |
| `UAVParameter` | object | — |
| `UVAnimation` | object | — |
| `UVModule` | object | — |
| `UnityAdsSettings` | GlobalGameManager | — |
| `UnityAnalyticsSettings` | object | — |
| `UnityPropertySheet` | object | — |
| `UnityPurchasingSettings` | object | — |
| `UnitySceneHandle` | object | — |
| `UnityTexEnv` | object | — |
| `UpdateZoneInfo` | object | — |
| `VFXCPUBufferData` | object | — |
| `VFXCPUBufferDesc` | object | — |
| `VFXEditorSystemDesc` | object | — |
| `VFXEditorTaskDesc` | object | — |
| `VFXEntryExposed` | object | — |
| `VFXEntryExpressionValue` | object | — |
| `VFXEventDesc` | object | — |
| `VFXExposedMapping` | object | — |
| `VFXExpressionContainer` | object | — |
| `VFXField` | object | — |
| `VFXGPUBufferDesc` | object | — |
| `VFXInstanceSplitDesc` | object | — |
| `VFXLayoutElementDesc` | object | — |
| `VFXLayoutOffset` | object | — |
| `VFXMapping` | object | — |
| `VFXMappingTemporary` | object | — |
| `VFXPropertySheetSerializedBase` | object | — |
| `VFXRendererSettings` | object | — |
| `VFXShaderSourceDesc` | object | — |
| `VFXSystemDesc` | object | — |
| `VFXTaskDesc` | object | — |
| `VFXTemplate` | object | — |
| `VFXTemporaryGPUBufferDesc` | object | — |
| `VRSettings` | object | — |
| `ValueArray` | object | — |
| `ValueArrayConstant` | object | — |
| `ValueConstant` | object | — |
| `ValueDelta` | object | — |
| `VariableBoneCountWeights` | object | — |
| `VariantInfo` | object | — |
| `Vector3Curve` | object | — |
| `VectorParameter` | object | — |
| `VelocityModule` | object | — |
| `VertexData` | object | — |
| `VertexLayoutInfo` | object | — |
| `VideoClipImporterOutput` | object | — |
| `VideoClipImporterTargetSettings` | object | — |
| `VisualEffectInfo` | object | — |
| `VisualEffectSettings` | object | — |
| `VulkanGraphicsJobsDeviceFilterData` | object | — |
| `WheelFrictionCurve` | object | — |
| `bitset` | object | — |
| `int2_storage` | object | — |
| `int3_storage` | object | — |
| `xform` | object | — |
