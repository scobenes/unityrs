# P1 plan: unityrs-core

**Status:** the plan for P1's core, ready for parallel work. The interfaces in section 3 are committed as compiling stubs on this branch (`crates/unityrs-core`, `crates/unityrs-testkit`, `crates/unityrs-dump`). The stub files are the contract: where this text and a stub's signature differ, the stub wins and this text is fixed.

**What it delivers.** A library that reads Unity bundles, serialized files and typetrees exactly as UnityPy 1.25.2 with UnityPyBoost reads them, shaped for a batch engine and Python bindings. It ends at a gate: the canonical dump of the whole private corpus and of UnityPy's samples, at L0-L3, differs from the oracle's in zero records, with no shim.

**How it was chosen.** Three candidate designs were written against the same brief: parity first, performance first, and fork economy first. Each was scored, and their factual claims were checked against rabex 0.2.1's source and UnityPy 1.25.2's (the wheel, and the UnityPyBoost C++ in its sdist). This plan starts from the parity-first design and takes from the other two what survived that check (section 0).

**Citations.** UnityPy paths are relative to the installed `UnityPy/` package of 1.25.2; UnityPyBoost paths are `UnityPyBoost/…` in the 1.25.2 sdist; rabex paths are `src/…` of the 0.2.1 crate. Every rule the core ports cites its line. Repository paths are relative to the repository root.

**Reading order for a package agent.** Sections 0 and 1, your package in section 7, the API sections it lists, section 6 (what "done" is measured by), then the stub files you own.

---

## 0. Decisions at a glance

1. **The core is a port of UnityPy's read and write rules, line by line, on raw bytes.** Every format path (detection, bundles, SerializedFile, typetree nodes, both readers, the writer, the Environment) is written from the UnityPy function it mirrors, with a `// UnityPy <file>:<line>` comment on each rule. The P0 spike measured rabex at 99.98% of L1-L3 with one header fix, but the remaining differences sit in rabex's types, not in patchable lines: strict UTF-8 `String` fields where UnityPy surrogate-escapes, defaults where UnityPy keeps "absent", class-only cache keys, recursive node trees that refuse UnityPy's level jumps, version arithmetic that rejects UnityPy's versions. A zero-difference gate needs those facts as read.
2. **rabex stays vendored, as its own crate, pristine plus a ledger** (`crates/unityrs-rabex`, library name `rabex`). The first commit is the unmodified crates.io source; every later commit is one numbered patch with its class and reason in `UPSTREAM.md`; `scripts/fork-check.sh` proves the crate equals the pristine source plus the ledger. Code the core supersedes (serde_typetree, rabex's object lookup and read paths) stays in the fork **verbatim and fenced**: no unityrs crate may call it. In P1 the fork supplies the TPK parser to `unityrs-tpk`; in P3 the UnityCN decryptor and the builders. This keeps re-vendoring and upstream offers cheap without letting rabex's types into the core. `docs/DESIGN.md` section 4 records the result: the core is a port of UnityPy's rules (Q1, decided).
3. **Buffers are `bytes::Bytes`, shared, never copied.** The input, each decoded block run, every bundle entry, every `SerializedFile`, every `TypelessData` value, every `vector<UInt8>` value and every string read from a file (`RawStr` is a `Bytes`: metadata strings, node names, `Value::Str`) are slices of one allocation. A decoded buffer carries the memory-budget permit that paid for its whole capacity, and a decoder reserves before its output grows, so the books show exactly what is alive.
4. **Containers are lazy; the dump forces UnityPy's eager order.** A bundle reads its header, blocks-info and directory at open and decodes no data block. There is one layout, UnityPy's: the `b"".join` of every block's actual output. An entry's offset in it depends on the actual length of every block before it, so a lazy read first learns those lengths (a block not yet decoded is decoded with its output discarded, its length kept), then decodes the blocks it overlaps into a cached run (a block that two reads' spans share is decoded once per span, 3.6). Lazy reads are therefore byte-identical to eager ones on every bundle, malformed ones included. `BundleFile::load_all` is UnityPy's load (every directory entry parsed, a failing entry fails the bundle).
5. **Typetrees compile once per type into a flat table** (kind, shape, align, dict key, minimum size per node), `Arc`-shared by every object of the type, as UnityPyBoost precomputes per node. The walker never compares strings.
6. **`Value` is UnityPy's dict model, owned and self-describing.** Float width, packed primitive runs, tuple versus list, bytes versus str, invalid UTF-8 and dict order live in the value. Dict keys are node ids into the node lists a `Parsed` carries: the root's list, then one per ref-type tree a `ReferencedObject` read through. That last part fixes a flaw in the fork-first design, whose keys could only name the root tree.
7. **Two readers and one writer, each a port of its own UnityPy code.** UnityPyBoost's reader for `byte_size > 0`, UnityPy's pure-Python reader for `byte_size == 0`, and UnityPy's pure-Python writer (UnityPyBoost has no writer), whose per-element alignment does not mirror the reader. No serde in the walker.
8. **UnityPy mode is the default everywhere.** That covers the float32 signalling-NaN quieting, the writer's alignment, the eager AssetBundle read at load, and no version fallback. `FloatMode::Lossless` and `WriteRules::MirrorReader` are opt-in and listed in `PARITY.md`.
9. **The common-string table is the 112 strings UnityPy 1.25.2's blob parser resolves**, not the 108 of its MIT enum. Corpus blobs use two of the four extra offsets, so the 108-entry table (the performance-first design's choice) fails L1. rabex's 113th string stays out.
10. **No TPK in the core or the repository.** The core defines a provider chain keyed by `(class, packed version, script)`; `unityrs-tpk` loads a TPK from a path and implements it.
11. **No panics on untrusted input, bounded work, nothing global.** Clippy denies `unwrap`, `expect`, `panic`, `todo`, `unimplemented`, `unreachable` and indexing in the core (outside tests). Every input-chosen size passes a `Limits` bound or the budget, checked before the allocation; what parsing keeps (metadata records, typetree rows, decoded bytes) is bounded per input, however many files the input holds; every input-chosen count grows a table only as the bytes behind it are read, never sizes an allocation; arithmetic on input integers is widened or checked, as Python's integers never overflow (positions are signed `i128`); and no step is worse than linear in its input. The core spawns no threads and blocks on nothing a pool waits for. Every public type is `Send + Sync`, checked at compile time, and a test fails when a public type is added without the check.
12. **The canonical dump lives in `unityrs-dump`, on the public API only.** The core takes on no JSON, sha256 or gzip. If the dump needs a fact the API does not expose, the API is incomplete.
13. **Five work packages run in parallel after this scaffold** (section 7): A the fork, tables, TPK and licences; B the typetree compiler, readers and writer; C streams, versions, detection, codecs and files; D providers, objects, the container and the Environment; E the dump, the CLI and the gate.

---

## 1. Scope

**In P1:**
- Detection and `parse_file`'s naming rule, all of CANONICAL_DUMP section 4.2's L0 rules.
- UnityFS formats 6-8 (the UnityFS layout at format 6 included), the flag sets by engine version, blocks-info at the end, padding at the start, the 2019.4.15 alignment rule; codecs none, LZMA, LZ4, LZ4HC; LZHAM and unknown codecs raise as in UnityPy.
- `SerializedFile` formats 9-22 in both byte orders (below 9 by the same port, tested on synthetic files), every metadata gate, ref types, externals, script types.
- Typetrees in the old and blob formats; UnityPy's tree-building rule; the common-string table.
- Both readers, the writer, `check_read`, the AssetBundle container, `PPtr` dereference, resource lookup, the Environment in eager and lazy modes, the provider chain and a TPK provider.
- `unityrs dump` and `unityrs values`, and the gate of section 6.

**Not in P1** (refused with a named `Unsupported`, never a panic): UnityWeb and UnityRaw layouts outside format 6, `WebFile`, `.splitN`, zip/apk inputs as containers, UnityCN, UnityArchive, LZHAM, SerializedFile 23-26 and UnityFS 9 (the HEAD tier), writing whole files (L4), typed views, the batch engine itself (`unityrs-batch` builds on this API next), the Python bindings (P3; section 4.2 shapes the API for them).

---

## 2. Crates and layout

### 2.1 Workspace crates

| crate | role | licence | publish |
|---|---|---|---|
| `unityrs-core` | everything in section 3 | `(MIT OR Apache-2.0) AND MIT` (it ports MIT code) | yes, later |
| `unityrs-rabex` (new, package A) | rabex 0.2.1, vendored pristine plus the ledger; library name `rabex` | `MIT` (elected) | no (Q4) |
| `unityrs-tpk` (new, package A) | `TpkProvider`: a TPK loaded from a path, behind core's `TypeTreeProvider`; uses the fork's TPK parser | `(MIT OR Apache-2.0) AND MIT` | yes, later |
| `unityrs-testkit` (new) | synthetic builders and a seeded generator, bytes only, no core dependency | `MIT OR Apache-2.0` | no |
| `unityrs-dump` (new) | the canonical dump (`docs/CANONICAL_DUMP.md`) on core's public API | `MIT OR Apache-2.0` | no |
| `unityrs-cli` | `unityrs dump`, `unityrs values` | `MIT OR Apache-2.0` | yes, later |
| `unityrs-spike-rabex` | unchanged; removed once the gate passes | | no |
| `unityrs-bench-lzma` | unchanged | | no |

### 2.2 `unityrs-core` module tree

Owner letters refer to section 7. "Scaffold" means implemented in this branch and frozen: a package may add private helpers and tests to it but not change a public signature without the integration rule of section 7.1.

```
crates/unityrs-core/src/
  lib.rs            re-exports, the no-panic denies, Send+Sync assertions        scaffold
  raw.rs            RawStr: bytes-native, zero-copy strings                    scaffold
  limits.rs         Limits, Meter (per-input counts)                           scaffold
  budget.rs         MemoryBudget, Permit, Unbounded, Counting, budgeted_bytes,
                    DecodeBuf                                                  scaffold
  error.rs          Error, Unsupported, LimitError, NotFound                   scaffold
  options.rs        FileContext                                                scaffold
  tables.rs         Unicode Nd zeros and lower-case exceptions (generated)     A
  classid.rs        class names (generated)                                    A
  typetree/commonstring.rs  the 112 common strings (generated)                 A
  stream.rs         Reader: EndianBinaryReader on a slice; py_read,
                    py_unpack_from, py_align, py_string_to_null                C (the py_* helpers: scaffold)
  version.rs        UnityVersion, the two version chains                       C (from_packed, packed_u64,
                                                                                 ordering: scaffold)
  detect.rs         check_file_type, parsed_as                                 C
  codec.rs          Codec, BlockCodec, Codecs, BlockDecryptor                  C (registry: scaffold)
  files/mod.rs      File, RawFile, Parent, parse_file                          C
  files/bundle.rs   BundleFile: lazy blocks, directory, entries                C
  files/serialized.rs  SerializedFile, SerializedType, ObjectInfo, …           C (accessors: scaffold)
  files/web.rs      WebFile (P3: refuses in P1)                                C
  typetree/rows.rs  NodeRow, NodeList (UnityPy's tree-building rule)           scaffold
  typetree/parse.rs old-format and blob-format node rows                       C
  typetree/tree.rs  Tree::compile, Kind, Shape, NodeInfo                       B
  value.rs          Value, PrimArray, Map, Parsed                              scaffold
  walk/mod.rs       read_typetree dispatch + check_read; option and error types B (types: scaffold)
  walk/boost.rs     UnityPyBoost's reader                                      B
  walk/python.rs    the pure-Python reader                                     B
  walk/write.rs     UnityPy's writer                                           B
  pystr.rs          py_lower, ntpath basename/splitext, simplify_name          D
  provider.rs       TypeTreeProvider, ProviderChain                            D (construction: scaffold)
  object.rs         ObjectRef                                                  D (getters: scaffold)
  container.rs      Container, read_container                                  D
  env/mod.rs        Environment, EnvOptions, Source, CabResolver               D (options: scaffold)
  env/registry.rs   the CAB registry (dict order, lazy slots)                  D
  env/pptr.rs       PPtr, DerefError, external_index                           D (types, external_index: scaffold)
  env/resource.rs   resource candidates                                        D
crates/unityrs-core/tests/
  scaffold_fixtures.rs  replays tests/fixtures/scaffold/*.json               scaffold
  memory_model.rs       the measured memory model of 3.15                    scaffold
  decode_alloc.rs       a DecodeBuf pays before it allocates (3.6)           scaffold
  fixtures/<package>/   oracle fixtures, written by tools/fixtures/<package>.py   that package
tools/fixtures/scaffold.py  the scaffold's fixtures (reader rules, pointer indexes, LZ4)  scaffold (E maintains)
scripts/stub-check.sh   gate G6                                              scaffold (E maintains)
```

### 2.3 What happens to rabex 0.2.1

Line counts are the spike's `rabex_loc.py` library lines (`docs/spikes/rabex.md` section 11).

| rabex module | lines | fate | why | replacement |
|---|---:|---|---|---|
| `serde_typetree/*` | 1,528 | superseded, fenced | read rules (13 of 32 synthetic cases differ) and write rules (21 `todo!()`, 27 `unreachable!()`) | `walk/*` |
| `files/serializedfile/mod` | 1,115 | superseded, fenced | `String` metadata, defaults for absent fields, gates at 9/10/11, `types[type_id]` panics on a negative id (`:939`) | `files/serialized.rs` |
| `files/bundlefile/mod`, `reader` | 981 | superseded, fenced | the header byte (`mod.rs:616`), the decryptor-before-alignment order (`:638-645`), `UnityArchive` `panic!` (`:459`), `discard_front` `panic!` (`reader.rs:338-355`) | `files/bundle.rs`, `codec.rs` |
| `typetree/mod` | 374 | superseded, fenced | no `m_Level`/`m_Index` kept; the blob builder rejects UnityPy's level jumps; an unknown common offset is an error where UnityPy gives its decimal string; strict UTF-8 names | `typetree/*` |
| `unity_version` | 110 | superseded, fenced | rejects versions UnityPy accepts; no raw string | `version.rs` |
| `typetree/typetree_cache` | 99 | superseded, fenced | keyed by class alone | `provider.rs` |
| `read_ext`, `write_ext` | 128 | superseded, fenced | `io::Read` with up to 1 GiB pre-allocated for a hostile length | `stream.rs` |
| `objects/class_id`, `commonstring` | 996 | superseded, fenced | generated from AssetRipper's unlicensed dumps; the core's tables are generated from UnityPy | `classid.rs`, `commonstring.rs` |
| `tpk` | 465 | **used** (P1), patched | the TPK reader; panics on an unknown class (`tpk.rs:475`) | via `unityrs-tpk` |
| `archive_storage_manager` | 120 | used in P3 | UnityCN | a `BlockDecryptor` adapter |
| `files/*/builder` | 463 | used in P3 | creating files | |
| `objects/pptr`, the rest | 353 | fenced | core has its own types | |

**Fenced** means: present byte for byte, compiled (so `fork-check.sh` can test it), and unreachable from any unityrs crate. Enforcement: the fork's `lib.rs` keeps its modules public (that is upstream's API), and `crates/unityrs-tpk/clippy.toml` lists every superseded path under `disallowed-types` and `disallowed-methods` (repeating the root's `doc-valid-idents`, since clippy reads only the nearest `clippy.toml`); a test greps the workspace for `rabex::` uses outside `unityrs-tpk` and the spike and fails on any.

### 2.4 The fork's mechanics

- **F0, the pristine import.** The crates.io tarball's `src/`, `LICENSE-MIT.md` and `LICENSE-APACHE.md`, unchanged (tarball sha256 `d5f8c39f8069c0dc6d01d5435bd21153fda86b2e01af5fea9b5ddfe0716c3d58`). Only `Cargo.toml` is ours: package `unityrs-rabex`, `[lib] name = "rabex"`, `license = "MIT"`, `publish = false`, no `embed-tpk` feature and no `resources/` (the TPK and its dumps never enter the tree), the compression features the TPK reader needs, and its own `[lints]` table (it does not take the workspace's pedantic lints; upstream's style is kept).
- **The ledger**, `crates/unityrs-rabex/UPSTREAM.md`: one row per patch with id, commit, rabex lines touched, class, upstream status and, for a non-upstreamable patch, why. Classes: **U** upstreamable bug fix or additive API; **P** policy (licence, provenance); **C** UnityPy compatibility, not upstreamable. P1's patches:

  | id | change | class |
  |---|---|---|
  | F0 | pristine import | n/a |
  | F1 | `Cargo.toml`: no `embed-tpk`, no `resources/`, MIT; `[lints.rust] unexpected_cfgs = { level = "warn", check-cfg = ['cfg(feature, values("embed-tpk"))'] }`, so the pristine `#[cfg(feature = "embed-tpk")]` (`src/tpk.rs:201`) compiles without the feature and without a warning under `-D warnings` (checked on a temporary crate: the warning appears without the line and not with it) | P |
  | F2 | `TpkTypeTreeBlob::get_typetree_node` returns `None` for an unknown class instead of panicking (`tpk.rs:475`) | U |
  | F3 | the remaining `unwrap()`s reachable from a TPK file (`tpk.rs:306`, `:575`) become errors | U |
  | F4 | an additive accessor listing a class's `(version, class)` entries, so `unityrs-tpk` picks the version with core's `UnityVersion` (UnityPy's ordering) instead of rabex's | U |

- **The check**, `scripts/fork-check.sh`, in CI and the container: unpack the pinned tarball from the cargo registry cache (verifying its sha256) into a temporary directory; `git format-patch` the fork's history after F0 with `--relative=crates/unityrs-rabex` and `git am` the series onto it; `diff -r` against `crates/unityrs-rabex/src`; fail on any difference and on any ledger row without a commit.
- **Upstream.** Each U patch is a self-contained commit with its own test, offered upstream when the owner decides (Q5). If upstream merges one, re-vendoring drops it from the ledger.

### 2.5 Licences

- **Election.** rabex is `MIT OR Apache-2.0`; unityrs takes all vendored rabex code under **MIT**, so Apache-2.0 section 4(b)'s change notices do not apply. The crate's licence files travel with it.
- **`THIRD_PARTY_LICENSES` is generated** by cargo-about from `about.hbs` and `about.toml`, as on the `p0-ci` branch (`scripts/third-party-licenses`, whose `--check` fails on a hand edit, in CI). Cargo-about lists every crate's licence itself (the fork included, as a workspace member with `license = "MIT"`); the notices it cannot know live in `about.hbs`'s ported-code part, and package A owns `about.hbs` and `about.toml`:
  - UnityPy 1.25.2 and UnityPyBoost: MIT, `Copyright (c) 2019-2021 K0lb3`, the line in the 1.25.2 release's licence (later revisions read `2019-2026`; code ported from a later revision cites that line);
  - rabex 0.2.1: MIT elected; authors from its `Cargo.toml`, Rudolf Kolbe and Jakob Hellermann; its `LICENSE-MIT.md` verbatim, which has **no copyright line**, so none is invented;
  - CNStudio, credited by rabex's `archive_storage_manager.rs`: MIT, AssetStudio's lines (`Copyright (c) 2016 Radu`, `Copyright (c) 2016-2020 Perfare`);
  - data provenance: the common-string table's 108 MIT entries and 4 TPK-derived Unity type names (Q2); no TPK file anywhere.

  This branch still carries those notices as a hand-written `THIRD_PARTY_LICENSES`. Moving them into `about.hbs` and regenerating the file is deferred to the integration merge of `p0-ci` into this branch, which the main session does; package A then owns the generated pipeline.
- **File headers.** A ported file starts with `// Ported from UnityPy 1.25.2 (MIT): UnityPy/<path>. See THIRD_PARTY_LICENSES.` A generated file starts with `// @generated by tools/gen/gen_tables.py from <source>; do not edit.` The fork's files are not edited for headers (they are pristine); its `UPSTREAM.md` says where they came from.
- **`deny.toml`.** `about.toml`'s `accepted` list must equal `deny.toml`'s `[licenses] allow` (the `p0-ci` check), so `0BSD` (liblzma's xz sources, `docs/spikes/lzma.md`) is not added to the top-level list: it is a per-crate exception, `[[licenses.exceptions]] crate = "liblzma-sys", allow = ["0BSD"]`, matching `p0-ci`'s clarification for that crate. Ban `rabex` from crates.io for every crate but the spike, so the fork is the only rabex the product can link.
- **No TPK.** Three checks: no `include_bytes!` in the core outside generated tables; no crate in `cargo metadata` has a feature matching `*tpk*` enabled by default; no file under the repository matches `*.tpk`.

### 2.6 Generated tables

`tools/gen/gen_tables.py` runs in the oracle image (`scripts/dev python tools/gen/gen_tables.py [--check]`) and writes four tables into core. `--check` regenerates and diffs, and runs in CI where the oracle is present.

| table | file | source |
|---|---|---|
| `COMMON_STRINGS` | `typetree/commonstring.rs` | `UnityPy.helpers.TypeTreeNode.get_common_strings()` under 1.25.2: exactly 112 `(offset, bytes)` pairs, with a per-entry source comment (`CommonString.py` or the bundled TPK). Asserted: 1161 `Hash128`, 1169 `RenderingLayerMask`, 1188 `fixed_array`, 1200 `EntityId` present; 1209 absent. |
| `CLASS_NAMES` | `classid.rs` | `UnityPy.enums.ClassIDType`, 368 members |
| `ND_ZEROS` | `tables.rs` | Python 3.11 `unicodedata` (Unicode 14.0): the zero of every `Nd` run. Asserted: every `Nd` character is `zero + d` for `d` in 0-9. The scaffold seeds ASCII `0`. |
| `LOWER_EXCEPTIONS` | `tables.rs` | every code point whose `str.lower()` in Python 3.11 differs from Rust's `char::to_lowercase` (the generator emits Python's side; a Rust test checks the other side is what Rust does) |

---

## 3. The API

### 3.1 Principles

- **As read.** Every field UnityPy keeps is exposed as read: `Option` where UnityPy has no attribute, raw bytes where UnityPy keeps a surrogate-escaped `str`, the stored integer width. The dump is written from the public API, so a missing fact is a missing accessor.
- **Handles.** Files are `Arc`s of immutable parsed data (`File`, `Arc<BundleFile>`, `Arc<SerializedFile>`); an `ObjectRef` is an `Arc` plus a table row. Everything lazily computed sits behind a `OnceLock` (single flight under contention).
- **Context.** One `Arc<FileContext>` per load carries version policy, float mode, writer rules, providers, codecs, decryptor, budget and limits into every file of the load.
- **Errors, not panics.** Every fallible call returns `Result<_, Error>` or a narrower error with a structural kind; the dump never parses message text.

### 3.2 Strings

- `RawStr` (`raw.rs`, scaffold): the bytes UnityPy decoded with `surrogateescape`, kept as bytes, and a `Bytes`: a string read from a file is a zero-copy slice of the file's buffer (`RawStr::from_shared`), a common string is static, and only strings the core computes allocate. `to_str()` gives `Some` only for valid UTF-8 (a surrogate code point's three-byte encoding is invalid, as in Python). `Display` escapes. Metadata strings, node names, node types, container paths and `Value::Str` are all `RawStr`, so a typetree row that names a long local string costs a handle, however many rows name it.
- `pystr.rs` (D): `py_lower` is `str.lower()` on a surrogate-escaped string: valid UTF-8 runs lower with full case mapping (final sigma included) using Rust's tables except where `tables::lower_exception` gives Python 3.11's answer; invalid bytes pass through. `ntpath_basename` (after `splitdrive`: a drive letter or UNC prefix is dropped), `ntpath_splitext` (leading dots are not an extension), `simplify_name` (`basename(name).lower()`, `environment.py:357-363`) and `clean_external_path` (`classes/PPtr.py:68-73`).

### 3.3 The stream reader

`stream.rs` (C) is the memoryview `EndianBinaryReader` (`streams/EndianBinaryReader.py:230-285`) over a slice with its own cursor. **The position is signed (`i128`)**, as a Python `Position` is an unbounded integer that UnityPy moves with plain arithmetic: it goes negative or past the end, and the reads after it follow Python's rules, which the scaffold implements once as free functions and an oracle fixture pins (`tests/fixtures/scaffold/reader.json`, written by `tools/fixtures/scaffold.py`, replayed by `tests/scaffold_fixtures.rs`):
- `read(n)` is `py_read`: a Python slice, negative indexes from the end, a reversed range empty; the position moves by `n` whatever the slice held (size -1 sets it to the end, so `read(-1)` at 12 of 10 bytes leaves 10);
- every fixed-width read of 2, 4 or 8 bytes is `struct.unpack_from(view, Position)` (`:327-331`), `py_unpack_from`: a negative offset counts from the end and must leave the whole read before the end; the position moves only on success;
- a one-byte read (`read_u8`, `read_i8`, `read_bool`) is `unpack(fmt, read(1))` (`:91-95`): the `py_read` slice must hold one byte, and the position has moved even when it raises (at -1, `view[-1:0]` is empty);
- `align(n)` is `py_align`, Python's floor modulo from the start of the slice, with no bounds check (-7 aligns to -4);
- `read_string_to_null` is `py_string_to_null` (`:275-284`; the oracle feeds UnityPy bytes, `tools/parity/unitypy_dump.py:100`, `:446`): `re.search` clamps both bounds of the `STRING_TO_NULL_WINDOW = 32_767`-byte window into the buffer, so a negative start searches from 0; found, the string runs to the NUL, which is consumed, and the position is the absolute index after it; not found, it raises when `pos + 32_767 >= len` and otherwise returns `read(32_767)`, a slice from the position that may hold a NUL or nothing;
- a read UnityPy refuses returns `Error::OutOfBounds { what, pos, need, have }` (its `struct.error`/`EOFError`); `read_bytes` returns a borrowed slice; nothing pre-allocates from a length.

The positions that go negative, each a named test of its owner: `parse_blob` with a negative `node_count` or `stringbuffer_size` (3.8); a `SerializedFile` below format 9 whose `metadata_size` exceeds `file_size` (`SerializedFile.py:266`); blocks-info at the end with `compressedSize > Length`, read at `Length - compressedSize` (`BundleFile.py:135`, so `view[L-c : L]`); the pure-Python reader after a negative `TypelessData` length (3.10); a format-22 `byte_start + data_offset` below 0 or above `i64::MAX` (`ObjectInfo::byte_start` is `i128`).

**Counts.** UnityPy builds every metadata table with `range(count)` (`SerializedFile.py:282-308`, `BundleFile.py:145-165`), so a **negative count is zero records, not an error**, and a huge count fails only when the bytes run out. Parsers therefore grow tables record by record, reserving at most `min(count, remaining / min_record_bytes)` and never more than the input's meter still allows, so a table's length is bounded by the bytes that back it. `read_i32_array` is the exception: its `struct` format rejects a negative count (`EndianBinaryReader.py:258-264`), and it checks `4 * count <= remaining` before allocating. The bytes bound a table's length but not its memory, since a record can be one input byte (an external below format 5 is a lone NUL) and its parsed form up to `size_of::<SerializedType>()`, so every record of every file of one input is charged to the input's `Meter` before it is kept, against `Limits::max_metadata_records` (default 2^22, 3.15).

### 3.4 Versions

`version.rs` (C) ports `helpers/UnityVersion.py`.
- `UnityVersion::parse(&[u8])` matches `^(\d+)\.(\d+)\.(\d+)(.)?(\d+)?(.*)$` with Python's `re` (`\d` is any `Nd` digit via `tables::nd_digit`, DOTALL). The type character lowers to `a` 0, `b` 1, `c` 2, `f` 3, `p` 4, `x` 5, anything else 255; none at all is `f`. `packed` is `major<<48 | minor<<32 | build<<16 | type<<8 | number` as an **unbounded OR**: `Packed::Small(u128)` while it fits, `Packed::Big` beyond. Equality, hashing and ordering are on `packed`, as Python's `int` (two raw strings may pack equal). `raw()` keeps the string.
- **Python's digit limit.** Each group goes through Python 3.11's `int()`, which refuses a string of more than 4,300 digits (`sys.get_int_max_str_digits()`, `version::PY_INT_MAX_STR_DIGITS`; leading zeros and non-ASCII `Nd` digits count). `from_str` then raises `ValueError`, as for a string that does not match, so a group of 4,301 digits is `VersionError::NoMatch` in both chains (probe: 4,300 digits parse in every group, 4,301 raise). A 4,300-digit major, minor or build parses, but its packed integer has more than 4,300 digits and the oracle's `json.dumps` then raises, so no oracle dump can hold one (`docs/CANONICAL_DUMP.md` section 7); the core reports the exact integer.
- **From a TPK** (scaffold, implemented): `UnityVersion::from_packed(u64)` is UnityPy's `UnityVersion(int)` (`helpers/Tpk.py:464`), with an empty `raw()`, and `packed_u64` returns a packed value that fits; with the ordering this is what `unityrs-tpk` needs to pick an entry (`Tpk.py:472-481`) before C lands.
- **Chain 1, the bundle** (`files/BundleFile.py:523-536`), `resolve_bundle(engine, policy)`: the engine string; when it does not parse or its 16-bit major is 0, the fallback; no fallback is `VersionError::NoFallback`. It decides `flags_class` (`:113-121`) and `uses_block_alignment` (format 7 and up, or 2019.4.15 ≤ engine < 2020 at format 6; `:129-131`).
- **Chain 2, the SerializedFile** (`files/SerializedFile.py:351-360`), `resolve_serialized(own, parent_engine, policy)`: its own string (format 7 and up); when empty or `0.0.0`, the parent bundle's raw engine string (a `WebFile` parent gives none); when still empty or `0.0.0`, the fallback; then `parse`, which fails on whatever string that chain produced if it does not match.
- `VersionPolicy { fallback, force_bundle, force_serialized }`: `fallback` is UnityPy's `config.FALLBACK_UNITY_VERSION` (default `None`, as UnityPy); the two forces are unityrs options for broken headers and are reported in `ResolvedVersion::source`.

### 3.5 Detection and `parse_file`

`detect.rs` (C): `check_file_type(data, limits)` implements CANONICAL_DUMP section 4.2's eight rules in order (`helpers/ImportHelper.py:44-125`), including: a 20-byte file with no NUL raises; the U+00FA signature as UTF-8 bytes; zip only when byte 4 is NUL; gzip must inflate in full, every member, CRC and size checked, trailing zero bytes allowed, bounded by `Limits::max_inflate` (the spike's detector inflated only 20 bytes; that is wrong for rule 7). The full inflate is UnityPy's, not a unityrs policy, though `ImportHelper.py:77-83` asks for only 20 bytes: it reads them through a streaming `EndianBinaryReader` whose `read_string_to_null` tests `Position != Length` on every byte (`EndianBinaryReader.py:136-144`), and that `Length` is `seek(0, 2)` on the `GzipFile` (`:303-308`), which decompresses to the end of the last member. The oracle raises `BadGzipFile` on a bad CRC, `zlib.error` on a corrupt second member and `EOFError` on a truncated stream, each after a valid `UnityWebData` prefix. Detection inflates into a fixed scratch buffer and keeps only the first 20 bytes and the stream's length, so it reserves no budget; brotli at offset 0x20; the SerializedFile header read big-endian whatever its endian byte says. `parsed_as(ty, name, is_entry)` is `parse_file`'s naming rule (`ImportHelper.py:127-156`): an `AssetsFile` named `*.resS`, `*.resource`, `*.config`, `*.xml` or `*.dat` (case-sensitive) stays raw; a zip is `Zip` only for an input.

`files::parse_file(name, data, ty, parent, ctx)` (C) builds the `File` the type and name rule say. P1 refuses `WebFile` (`Unsupported::WebFile`).

### 3.6 Bundles and codecs

`files/bundle.rs` (C) ports `files/BundleFile.py`, lazily:
- `BundleFile::open` reads the header, blocks-info and directory only. `header()` exposes every field the dump's L1 writes (signature, format, both version strings raw, the resolved engine version, `data_flags`, `flags_class`, `block_alignment`, `encrypted`, `hash`/`crc` for the web/raw layout). `uncompressed_size()`, the sum of the **declared** block sizes, is known before any decode (the batch engine's admission); the actual decoded total can differ (LZMA ignores the declared size, LZ4 may decode short, a stored block yields its stored bytes, a truncated block yields what is there), and the limits and the budget follow the actual bytes (below).
- The rules, each with a test: the extra byte after the data flags only for non-UnityFS signatures (`BundleFile.py:104-106`; rabex reads it for UnityFS); the flag set by engine version (`:113-121`); the decryptor check **before** the 16-byte alignment (`:123-124`, then `:129-131`); blocks-info at the end read at `len - compressed` (`:134-137`); padding at the start only under the new flags with 0x200 (`:170-171`); a truncated block read short and decoded as it is, its actual length being the dump's `compressed`.
- **One layout: UnityPy's joined data.** UnityPy always decodes every block and reads entries from `b"".join` of their **actual** outputs (`:173-186`); there is no declared layout to fall back from. Block `b` starts at the sum of the actual lengths of blocks `0..b`, so an entry's bytes depend on every block before it, not only on the blocks it overlaps. The bundle keeps one `OnceLock<u64>` per block for its actual length. A length is known at open for a stored block (its bytes read) and otherwise only by running the block's codec.
- **Lazy reads.** Before `entry_bytes(i)` or `entry_range` returns bytes ending at joined offset `X`, every block that starts below `X` has a known length: a block still unmeasured that lies wholly before the read is decoded with its output discarded (a reused scratch buffer, its length kept), and the blocks the read overlaps are decoded into a run cached in `runs` (a run is `Bytes` with its permit; see **Runs** below for a block two spans share). A read whose bounds depend on the total length (a negative offset, a size of -1, a stop past the end) measures every block. Every entry is a zero-copy slice of its run. `entry(i)` parses once (`OnceLock`). There is no switch of layout after bytes were served, so a lazy read is byte-identical to `load_all` on every bundle, malformed ones included. On a well-formed bundle read in directory order the measuring costs nothing (the earlier runs are already cached); reading a later entry first costs one discarded decode of the blocks before it.
- **Entry slicing.** An entry is `reader.Position = offset; reader.read(size)` on the joined data (`files/File.py:78-81`), and the memoryview `read` is a Python slice (`EndianBinaryReader.py:248-255`): size 0 is empty, size -1 reads to the end, any other size is `data[offset : offset + size]` with negative indexes counting from the end and a reversed range empty. `stream::py_read` implements that arithmetic once (in `i128`, so no `i64` offset can overflow), and the bundle, `parse_blob` and the pure-Python reader use it.
- **Eager.** `load_all()` decodes every block in order and parses **every directory entry** in directory order, a path repeated in the directory included (its earlier entry is parsed and can fail, though the `files` dict keeps the later one), and returns the first error, which is UnityPy's behaviour (a failing entry fails the bundle).
- **Codecs** (`codec.rs`, C): **None** returns the stored bytes, as a zero-copy slice of the input; **LZMA** (`helpers/CompressionHelper.py:17-44`) is a raw LZMA1 decode of the 5 property bytes plus the rest, **run to the end of the input, the declared size ignored**, capped by `Limits::max_block_output` (fewer than 5 input bytes is `struct.error`); **LZ4/LZ4HC** (`:83-94`) is `lz4.block.decompress(data, uncompressed_size)`: the declared size is a **maximum**, a shorter output is accepted, a longer one raises (the performance-first design's "must equal" is wrong), and an empty input raises whatever the declared size; **LZHAM** and unknown ids raise (`:234-235`, `BundleFile.py:515-521`; `CompressionFlags` is an `IntFlag`, so any id constructs and the map lookup is what raises). `Codecs` is a 64-slot registry (the flags' low six bits) that a user may extend; only canonical ids name a slot (`Codec::Other(5..=63)`; `Other(1)` is not LZMA, and `set` hands a non-canonical value back); `BlockDecryptor` runs before decode (UnityCN: P3).
- **LZ4 acceptance is liblz4 1.9.4's** (`LZ4_decompress_safe` under python-lz4 4.4.4), not `lz4_flex`'s. They disagree both ways: liblz4's end-of-block conditions (a literal run ending within the last 12 bytes of the declared size or the last 8 bytes of input must be the last sequence and end exactly at the end of input; a match must end at least 5 bytes before the declared size; the length-extension guards) refuse blocks `lz4_flex` decodes, and liblz4 decodes a match at offset 0 (a run of zeros) that `lz4_flex` refuses. The oracle fixture `tests/fixtures/scaffold/lz4_blocks.json` (813 blocks around liblz4's end-of-block conditions, each decoded at up to 11 declared sizes around its real output, with liblz4's verdict and output; `tools/fixtures/scaffold.py` builds them) is the codec's acceptance test, and `tests/scaffold_fixtures.rs` pins that `lz4_flex` alone fails it. C checks the sequence stream against these conditions before (or while) `lz4_flex` decodes, and handles offset 0 itself; the dependency list stays as it is.
- **A codec writes into a `DecodeBuf`** (`budget.rs`, scaffold, implemented): `BlockCodec::decode(input: &Bytes, uncompressed, out: &mut DecodeBuf)` either appends its output (`extend_from_slice`, or `grow_zeroed` then `truncate`) and returns `Ok(None)`, or leaves `out` empty and returns `Ok(Some(slice))`, a zero-copy slice of its input, which the caller checks against the same caps and meter. Every growth of a `DecodeBuf` first checks its cap (`max_block_output`, `LimitKind::BlockOutput`) and, for a block's first decode (`with_meter`), the input's decoded-bytes meter (`max_bundle_uncompressed`), then reserves the new capacity from the budget, and only then grows; capacity grows geometrically up to the cap. So an LZMA block declared at 1 byte that decodes towards 1 GiB is refused at the first growth the budget or the limits do not allow, before the bytes exist (`tests/decode_alloc.rs` watches the heap with a counting allocator: a refused 64 MiB growth allocates nothing).
- **Limits on actual bytes, per input.** `max_block_output` caps one block's actual output, and `max_bundle_uncompressed` caps the decoded data of the whole input (every block and blocks-info of every bundle in it, nested ones included, each block counted once at its actual length): checked against the declared total at open (refused before any decode) and against the input's `Meter` as output grows. A block whose actual length is already known decodes again without the meter. Blocks-info's own decode is capped by `max_block_output` and the rows parsed from it grow with its bytes (3.3).
- **Budget.** Every decoded buffer, blocks-info, each run and each measuring decode's scratch, is a `DecodeBuf` reserved from `ctx.budget` (`Reservation::BundleData`) before it grows and turned into `Bytes` with `budgeted_bytes`, which shrinks the buffer, tops its permit up from the budget or trims it to the capacity, and releases it when the last slice drops; a refusal is `Error::Budget`. The budget pays for decoded and inflated buffers only: parsed metadata, typetree rows and values are bounded by `Limits` (3.15).
- **Runs.** A run covers the blocks one read overlaps, contiguous, and is cached by its block span. A read whose span a cached run covers slices it; any other read decodes its own span into a new run, so a block shared by two spans (a straddling read) is decoded twice and booked twice. `load_all` decodes every block into one run, so under `Eager` the runs hold exactly the joined data. `entry_range` (a `.resS` read) serves from a covering cached run and otherwise decodes its span into a buffer it returns without caching, so ranged reads add nothing for the bundle's lifetime. E's perf report counts re-decoded bytes and the peak of cached runs (R2, R3).
- **Parents.** An entry's `Parent` carries the input's `Meter` and, for a bundle, a `Weak` handle on it (the bundle is built with `Arc::new_cyclic`), which `PPtr.deref` scans for siblings (3.13).

### 3.7 SerializedFile

`files/serialized.rs` (C) ports `files/SerializedFile.py:206-360` and `files/ObjectReader.py:67-130`. The rules, each with a test:
- **Header** (`SerializedFile.py:255-269`): the four legacy `u32`s big-endian; from 9 the endian byte and 3 reserved bytes at offset 16, below 9 the endian byte at `file_size - metadata_size` (absolute); at 22 the 64-bit fields replace the legacy ones and `unknown` is the trailing `i64`.
- **Gates** (`:271-288`): the version string from 7, the target platform from 8, `enable_type_tree` from 13 (else true), `big_id_enabled` at 7-13 (else 0).
- **Type records** (`:117-148`): `is_stripped_type` from 16; `script_type_index` from 17 (else -1); from 13 `old_type_hash` always and `script_id` only for a ref type with `script_type_index >= 0`, a negative `class_id` below 16, or class 114 (`MonoBehaviour`) from 16; trees only when `enable_type_tree`, in the blob format at 10 and from 12, the old format otherwise; at 21 a ref type reads class, namespace and assembly names and a plain type its dependency list.
- **Objects** (`ObjectReader.py:73-130`): the path id as `i64` under bigID, `i32` below 14, else aligned `i64`; `byte_start` as `u32` (or `i64` from 22) plus `data_offset`, added in `i128` (`ObjectInfo::byte_start`: `2^63 - 1 + 144` loads in UnityPy, and a read from it is a Python slice); below 16 the type is the first record whose `class_id` equals the object's `type_id`, from 16 `types[type_id]` with Python indexing (a negative id counts from the end; out of range fails the file); at 11-16 the object's script index is written back into its type, the last object winning. The `objects` dict keeps a repeated path id at its first position with its last entry: `object_infos()` is file order, `object_order()` is dict order.
- **Script types and externals** with their gates (`temp_empty` from 6; `guid` and `type` from 5; the script identifier `i32` below 14, else aligned `i64`).
- **Eager AssetBundle.** With `ctx.eager_asset_bundle` (UnityPy's default), `parse` reads the container (`container()`, which calls D's `read_container`) of the first AssetBundle in dict order with `check_read`, and a failure fails the file (`SerializedFile.py:316-324`). UnityPy reads that object with `parse_as_object`, object mode (`ObjectReader.py:310-311`): keys are `_clean_name`s and the class is built with an `UnknownObject` fallback (`parse_class`, `UnityPyBoost/TypeTreeHelper.cpp:458-582`), and the container comes from that object. The core reads it in dict mode, so D's fixtures pin that the two modes agree on success, failure and `m_Container` for the trees they can differ on (two names that clean to one key, a tree without `m_Container`, a class that falls back to `UnknownObject`), and any case where they do not becomes a rule here. C's own tests run with it off; D and E test it on.
- **Bounds and strings.** Every record is charged to the input's `Meter` before it is kept (3.3), and every metadata string is a zero-copy slice of the file (3.2).
- `SerializedType::tree(&Limits)` compiles the record's `NodeList` with B's `Tree::compile` once (`OnceLock`); the walker reaches a ref type's tree through the `Limits` in its `ReadCtx`, so every reference reuses one compile.

### 3.8 Typetree nodes

- `typetree/rows.rs` (scaffold): `NodeRow` is one node as stored (`m_Level`, `m_Type`, `m_Name`, `m_ByteSize`, `m_Version`, `m_TypeFlags`, `m_VariableCount`, `m_Index`, `m_MetaFlag`, `m_RefTypeHash`, each `Option` where the format may lack it). `NodeList::from_rows` is UnityPy's stack rule (`helpers/TypeTreeNode.py:156-188`): a row nests under the nearest earlier row of lower level, so level jumps are kept as UnityPy keeps them; rows after a later top-level row are dropped, but are still processed for errors. Implemented and tested.
- `typetree/parse.rs` (C): `parse_old(r, data, format, meter, limits)` (`TypeTreeNode.py:78-109`, iterative, `Limits::max_tree_depth`) and `parse_blob(r, data, format, meter, limits)` (`:112-154`, `:331-347`): `m_Version` is read as `i16` where UnityPy reads it so; a string offset with the high bit set is looked up in `COMMON_STRINGS` and, when absent, becomes its decimal string (`:130`); a local offset reads to NUL within the buffer, as a zero-copy slice of it. The node area and the string buffer are `reader.read(n)` (`:116-117`), so a negative `node_count` or `stringbuffer_size` is a Python slice that moves the position back (`stream::py_read`, 3.3), not an error. **Each row is charged to the input's `Meter` (`typetree::parse::charge_row`, `Limits::max_tree_rows`, default 2^22) before it is pushed**, and the row vector grows with the rows read, never sized from `node_count`: a 24-byte blob row compresses about 7,000:1 under LZMA, so without the bound a 150 KB input could make the parser build 178 million rows (one 1 GiB block of them).
- `typetree/tree.rs` (B): `Tree::compile(Arc<NodeList>, &Limits)` precomputes per node what UnityPyBoost precomputes (`UnityPyBoost/TypeTreeHelper.cpp:1215-1216`): `Kind` from `m_Type` by exact match (`:1075-1103`) with the `PPtr<` prefix first (`:1105-1128`); `Shape` (what the walker does); `align` (`m_MetaFlag & 0x4000`); `key` (the first sibling with the same name) and `slot` (that key's ordinal among its parent's distinct names), so the dict collapse is O(1); `min_size` (the fewest bytes a successful read consumes, to bound pre-allocation and zero-size loops); `dup_names` and `has_registry` flags. `Shape::Vector` is only for a node whose kind the reader's switch does not handle itself (`Unknown`, `PPtr`, `Array`, `ReferencedObjectData`, `ManagedReferencesRegistry`) and whose first child's kind is `Array` (R9). Malformed shapes compile to `BadPair`/`BadVector` and fail only when a read reaches them (a `pair` run reaches its element once its length is read, even at length 0, R10); compilation fails only on depth.

### 3.9 Values

`value.rs` (scaffold) is the contract between the readers, the writer, the dump and (P3) Python:

| UnityPy | `Value` |
|---|---|
| `int` | `Int(i64)`, or `UInt(u64)` above `i64::MAX` |
| `bool` | `Bool` |
| `float` from a `float` node | `F32(bits)` |
| `float` from a `double` node | `F64(bits)` |
| `str` | `Str(RawStr)` |
| `bytes` (`TypelessData`) | `Bytes` (zero-copy) |
| `list` read as one primitive run | `Prims(PrimArray)`; `vector<UInt8>`/`char` is zero-copy `Bytes` |
| any other `list` | `List` |
| `tuple` (`pair`; the pure-Python path's aligned primitive arrays) | `Tuple` |
| `dict` | `Dict(Map)`: `(NodeId, Value)` entries in insertion order; `insert` is `PyDict_SetItem` (a repeated key keeps its first position and takes the last value) |

`Parsed { lists, value, bytes_read }` is the self-describing unit: `lists[0]` is the root tree's `NodeList`, and a `ReferencedObject` read through a ref type's tree appends that tree's list **once per distinct tree** (deduplicated by `Arc` pointer, so a list per ref type, not per reference); each `Map` records which list names its keys (`Map::list`, a `ListIx = u32`, tested past `u16`). `bytes_read` is an `i64`: UnityPyBoost's is never negative, the pure-Python reader's is after a negative `TypelessData` length (3.10). `Parsed::field`, `entries` and `get(path)` resolve names. `size_of::<Value>() <= 40` is tested.

### 3.10 Reading

`walk/mod.rs` dispatches (`helpers/TypeTreeHelper.py:123-158`): `byte_size > 0` reads `file[byte_start .. byte_start + byte_size]` (cut at the end of a truncated file) with `boost::read`; `byte_size == 0` reads with `python::read` from `byte_start` over the whole file; then with `check_read`, `bytes_read != byte_size` is `ParseError::CheckRead { expected, parsed }`, which carries the full value (it is what `check_read=False` returns).

**UnityPyBoost's rules** (`walk/boost.rs`, `UnityPyBoost/TypeTreeHelper.cpp`), each a named test:

| id | rule | source |
|---|---|---|
| R1 | kind from `m_Type` by exact match; `PPtr<` prefix; else Unknown, read as a class | `:1075-1128`, `:1215` |
| R2 | every bounds check is `pos + n > end`, evaluated even for `n = 0` (an empty run after an alignment past the end fails) | `:76`, `:88`, `:211`, `:296` |
| R3 | `align4` from the object's start, with no bounds check, so `bytes_read` may pass `byte_size` | `:67-72` |
| R4 | `bool` is a byte, non-zero true; `char` reads as `u8`; `Type*` as `u32`; `FileSize` as `u64` | `:74-84`, `:1075-1103` |
| R5 | `float` is widened to double; under `FloatMode::UnityPy` a signalling NaN gets bit 22 set, as the x86-64 widening does | `:172-175` |
| R6 | `string`: an `i32` length; a negative length is `negative_length` before the cursor moves; bounds; the bytes kept raw; **always** aligned after | `:288-305` |
| R7 | `TypelessData`: a length, bounds, the bytes (zero-copy); aligned only by its own flag | `:307-323`, `:877-880` |
| R8 | `pair` needs exactly 2 children, else `other` ("Pair node must have 2 children") | `:325-351` |
| R9 | the switch on the node's own kind comes first: only a kind it does not handle (`Unknown`, `PPtr`, `Array`, `ReferencedObjectData`, `ManagedReferencesRegistry`) is an array when its first child's type is exactly `Array` (a `string`, integer, `TypelessData` or `pair` node with an `Array` first child reads as its own kind); that `Array` needs 2 children; a negative length is `negative_length`; the `Array`'s flag aligns the whole | `:707-874`, `:812-861` |
| R10 | elements of integer, float, `bool` and `pair` kind read as one run and align once after if the element is flagged; a `pair` run checks the element's arity (R8) once after reading the length, so an empty array of malformed pairs fails | `:686-699`, `:839-860`, `:356-360` |
| R11 | other elements read one by one, each aligning itself | `:885-940` |
| R12 | a class reads its children in order into a dict, with `PyDict_SetItem` for repeated names | `:394-445` |
| R13 | `ManagedReferencesRegistry`: the first registry in a class's subtree sets the scope; later registries inside it are skipped; the scope resets when the class that set it ends | `:394-445` |
| R14 | `ReferencedObject`: children through the general reader; `ReferencedObjectData` resolves its tree, in this order: no assets file, `ref_types` absent or not a list, no `type` read yet, a missing `class`, `ns` or `asm` are each `ref_type`; then an empty class skips `data`; then the scan by class, namespace and assembly, where a match whose ref type has no tree (`node is None`) also skips `data`, and no match is `ref_type` | `:585-681`, `:751-811`, `:767` |
| R15 | after a value, `align4` when the node (for an array, its `Array`) is flagged | `:877-880` |
| R16 | error kinds are structural: `out_of_bounds`, `negative_length`, `ref_type`, `other`; a `Limits` bound maps to `other`, the kind UnityPy's `MemoryError` and `RecursionError` get | `walk/mod.rs` `L2Kind` |

**The pure-Python reader** (`walk/python.rs`, `TypeTreeHelper.py:182-361`) starts at `byte_start` in the whole file, reads with the stream rules of 3.3 at any position, and **aligns relative to the file**, not the object (`align_stream` rounds the absolute position: an object at byte 1 with `[UInt8 a (aligned), UInt8 c]` reads `c` at 4 and 4 bytes where Boost reads it at 5 and 5 bytes). It differs as CANONICAL_DUMP section 7 lists: aligned primitive arrays are tuples with `UInt16`/`SInt16` signedness swapped; an aligned `SInt8` array and an array of aligned `TypelessData` raise `TypeError` (`other`); a negative or out-of-range string length reads as `""`; in an array whose aligned element is a vector, a negative inner length reads as `[]`; arrays of aligned class elements align once and skip the registry check; a `TypelessData` length of -1 reads to the end of the file and other negative lengths follow Python slice semantics (so `bytes_read` can be negative); a read past the end of the file is `other`; and, probed on UnityPy 1.25.2:
- a `pair` with three or more children reads its first two (`:193-196`), where Boost raises;
- an aligned `pair` array reads its children through `FUNCTION_READ_MAP` without their own align flags (`:285-293`);
- a `ReferencedObject` reads `ref_object["type"]` before anything else (a missing key is `KeyError`, `other`), and raises "SerializedFile has no ref_types" for an empty or absent list *before* the empty-class skip (no `ref_types` attribute at all is an `AttributeError`, which the dump maps to `ref_type`), where Boost skips `data`;
- an empty array of malformed `pair`s reads as `[]` (the arity check is inside the loop);
- an aligned `ReferencedObject` array element whose class is empty or whose ref type has no tree raises (`read_value(None, …)`, `ReadDetail::RefTypeNoTree`, kind `other`); everywhere else both readers skip `data`.

**Floats.** `FloatMode::UnityPy` (default) quiets an f32 sNaN as above; `Lossless` keeps the bits. `F64` keeps every bit in both modes.

**Linear dicts.** A class whose children repeat a name (`dup_names`) is the one place a read could go quadratic: `Map::insert` scans, so a hostile tree with *n* children and late repeats would cost *n*² per object. The walker never calls `insert` per child. It keeps a per-class scratch from `slot` to entry position (cleared per class read, so a registry child skipped at run time, R13, cannot misplace a later key): a first sighting pushes, a repeat overwrites in place with `Map::replace_at`. The dict keeps first position and last value, as `PyDict_SetItem`, in time linear in the children read.

**Bounds.** A list first reserves `min(len, remaining / min_size, Limits::max_prealloc)` and then grows by `reserve_exact` in steps of at most `max_prealloc`, never past `len`, so its capacity never exceeds its final length and a dict entry costs `size_of::<(NodeId, Value)>()` (48 bytes) at most. Every `Value` built counts against `max_values_per_object`, the elements of a primitive run included; nesting (ReferencedObject data re-enters trees) counts against `max_value_depth`; elements with `min_size == 0` count against `max_zero_size_elements`.

### 3.11 Writing

`walk/write.rs` (B) ports UnityPy's `write_typetree` (`TypeTreeHelper.py:161-179`, `:381-458`) under `WriteRules::UnityPy`:
- W1 dispatch on the type name first, then `pair`, `ReferencedObject`, an `Array` first child, then a class;
- W2 **duck typing**: the writer takes any value Python's operations accept, not only the variant the reader makes for the node, which a duplicate-name object exercises (its dict holds one child's value under the other's node). Probed on UnityPy 1.25.2, per node kind: a `bool` node takes any value (`struct.pack('?')` uses truthiness; an empty `Str`, `Bytes`, `List`, `Tuple` or `Dict` is false); an integer node takes `Bool` and `Int`/`UInt` in range (a float, or an out-of-range int, is an error); a `float` or `double` node takes `Bool`, integers and floats (a finite float that overflows `f32` is an error); a `string` node only `Str`; `TypelessData` only `Bytes`; a `pair` node anything with items 0 and 1 (`List`, `Tuple`, `Prims` or `Bytes` of length 2 or more, a `Str` of 2 or more characters), each item written through its child's kind; an array any sized iterable (`List`, `Tuple`, `Prims`, `Bytes` as integers, `Str` as one-character strings, a `Dict` as its keys), each element under the same rules; a class only a `Dict`;
- W3 strings always align;
- W4 `TypelessData` writes its length and bytes and aligns only when flagged;
- W5 arrays write the length, then each element, and **each element aligns when its node is flagged** (`:435`), then the vector or `Array` flag aligns once: the writer does not mirror the reader;
- W6 a class writes `value[child.m_Name]` for every child: a missing key is `MissingKey`, an extra key is ignored, registry scope as on read;
- W7 `ReferencedObject` data needs non-empty ref types (`:99-100`, where the reader does not raise) and a ref type with a tree: an empty class, or a matched ref type with no tree, fails (`KeyError: 'data'` on the value the reader made, which left `data` out; an `AttributeError` on the `None` node when the value has one).

`WriteRules::MirrorReader` aligns once after a primitive or pair run and writes an empty-class `ReferencedObject` without `data`, so every value it reads round-trips. It is opt-in (`PARITY.md`, Tier C).

### 3.12 Providers and objects

- **Tree resolution** (`ObjectReader.py:285-307`), `ObjectRef::tree(nodes)` (D): an explicit `nodes=` tree; else the type record's own tree (`TreeSource::Embedded`); else `ctx.providers.lookup(TreeRequest { class_id, version, script: None })` (`TreeSource::Provider(name)`); else `Ok(None)`, which reads as `ParseError::NoTypeTree`.
- **The `MonoBehaviour` script hook** needs an Environment, so it is `Environment::object_tree(obj, nodes)` (D), not `ObjectRef::tree`: when the resolved tree's root type is `MonoBehaviour` and a provider in the chain accepts script requests (`TypeTreeProvider::accepts_scripts`, UnityPy's `env.typetree_generator`), the head is read with that tree (`check_read` off), `m_Script` is dereferenced through the Environment and parsed, and the provider is asked with its `ScriptIdent`. As in UnityPy (`ObjectReader.py:332-346`), only a `ValueError` there falls back to the class tree (a null `m_Script`, a value error in the head read, the provider's miss); a dereference failure (`FileNotFoundError`, `KeyError`, `IndexError`) is the object's error. With no such provider nothing is dereferenced (UnityPy raises its `ValueError` before any deref), so in P1, where no provider accepts scripts, `object_tree` equals `ObjectRef::tree`.
- **`ProviderChain`** (`provider.rs`, D) asks providers in order, first answer wins, and caches the answer, negative ones too, under `(class_id, packed version, script)`: UnityPy's `CLASSES_CACHE` key (`helpers/Tpk.py:16`, `:44`), where rabex keys by class alone and so answers every later version with the first version's tree (`docs/spikes/rabex.md` section 5).
- **`unityrs-tpk`** (A): `TpkProvider::open(path)` reads a TPK with the fork's parser, picks a class's entry for a version by core's `UnityVersion` ordering (UnityPy's), converts the fork's nodes to `NodeRow`s, compiles with `Tree::compile`, and answers `Ok(None)` for a class it does not have. Its `name()` is `"tpk"`.
- **`ObjectRef`** (D): `raw_data()` (zero-copy, cut at the end of the file), `read_typetree(nodes, check_read)`, `parse_as_dict(check_read)`, `write_typetree(value, nodes, rules)`.

### 3.13 The Environment

`env/` (D) ports `environment.py:27-363`, `classes/PPtr.py:43-90` and `helpers/ResourceReader.py:10-38`.
- **Registry** (`env/registry.rs`): keys are `simplify_name(name)`; dict semantics (a repeated key keeps its first position and takes the new value); every `SerializedFile` and raw entry of a bundle is registered by its entry name **as it is parsed** (`files/File.py:76-90`), and every file loaded at top level by its name (`environment.py:154-157`). **Partial registration:** under `Eager`, a bundle whose entry fails to parse fails `load_file`, which raises before the bundle enters `files`, but the CABs of the entries parsed before it stay registered and resolvable (probed on UnityPy 1.25.2 with a bundle `[good CAB, its .resS, bad CAB]`: `env.files` is empty, `env.cabs` holds the first two). Under `LoadMode::Lazy` a bundle registers every directory entry's name at open, in directory order (a repeated path keeps its first position and its last entry, as the eager dict does), and parses an entry on first `get` (`OnceLock`, single flight).
- **Load modes.** `Eager` is UnityPy (`load_all` at load); `Lazy` is the batch engine's. They give identical results for every read on a scene whose blocks all decode, whose entries all parse and none of which is itself a container. Everywhere else the differences are exactly these, and a test pins each: (a) **failures move**: an entry that fails to parse (or whose AssetBundle read fails) fails the whole bundle at load under `Eager` (with the partial registration above), and under `Lazy` fails only the `get` that parses it, so a sibling stays readable and an entry shadowed by a later one of the same path is never parsed; `BundleFile::load_all` under `Lazy` returns exactly `Eager`'s error; (b) **nested containers**: an entry that is itself a bundle is not a CAB under `Eager`, which registers its entries in place; `Lazy` cannot know that without decoding it, so the entry is registered under its own name until its first `get`, which answers `None` and registers its entries then, at the end of the order; `Eager` answers `None` for that key too, unless an earlier directory entry of the same path was a CAB, which `Eager` keeps under the key; (c) **block failures**: a block that fails to decode, or trips a limit, after the last byte any read needs fails the load under `Eager` (UnityPy decodes every block before any entry, `files/BundleFile.py:173-186`) and nothing under `Lazy`. The registry phase of a batch therefore reads headers and directories only (4.1). A test runs every Environment scene in both modes and compares, asserting identity on the well-formed scenes and exactly (a), (b) and (c) on the others; G5 also compares `Eager` with UnityPy on every scene (section 6).
- **`find_file`** (`environment.py:310-354`): `get_cab`, else the `CabResolver` (UnityPy walks `env.path`; the core never opens a path it was not given), loading what it returns as a dependency.
- **`deref`** follows `PPtr.py` in order (`env/pptr.rs` lists the rules): `m_PathID == 0` raises; `m_FileID == 0` is the own file; else `externals[m_FileID - 1]` by `pptr::external_index` (scaffold, implemented, pinned by `tests/fixtures/scaffold/pptr_external.json`): computed in `i64` (`i32::MIN - 1` must not wrap), the guard is only `>= len` (`FileNotFoundError`, `DerefError::FileIdPastEnd`), and then Python indexing, so a negative id counts from the end and one below `-len` is `IndexError` (`DerefError::FileIdBelowStart`); the container preload (`helpers/ContainerHelper.py:43-46`) swallows the first and propagates the second. **The file's container** is its bundle for a bundle entry, and for a file loaded at top level the Environment that loaded it (`environment.py:154` passes `self` as the parent, `files/File.py:36`): UnityPy's "no container is set" is raised only for a `SerializedFile` built outside any Environment, which `Environment::deref` never sees, so the core has no such refusal (probe: two loose files `a/CAB-A` and `b/CAB-B`, A's external `archive:/CAB-B/CAB-B`, resolve to B's object in UnityPy 1.25.2; the testkit scene `loose_external_pointer` asserts it). The cleaned external path is compared with each key of the container's `files` lower-cased (no basename), then `find_file`; a raw target, or a container under an Environment `files` key, has no objects; a missing path id is a `KeyError`. Each outcome is a `DerefError` variant. `from` need not have been loaded into this Environment (a batch job's own bundle handle, 4.1): its bundle siblings come through its `Parent`'s weak bundle handle (3.6), every other CAB from the Environment.
- **Resources**: the four candidates (`basename`, `name.resource`, `name.assets.resS`, `name.resS`), `get_cab` each, then load the file's externals and the candidates as dependencies and try again, then `NotFound::Resource`. Note: UnityPy calls `environment.load_file(dependency, True)`, whose second positional parameter is `parent`, not `is_dependency`, so those loads are not marked as dependencies; the port keeps that.
- **Container**: `Environment::container()` builds every CAB's preload index first (`environment.py:208-216`) and lists every non-dependency `SerializedFile`'s container rows, CABs in registry order. `container::read_container` reads the first AssetBundle in dict order (`m_Container` pairs in file order, repeated paths kept, and `m_PreloadTable`).
- **Dependencies** load into an LRU bounded by `EnvOptions::dep_cache_bytes` and reserved as `Reservation::Dependency`; files the caller loaded are never evicted. `EnvOptions::unitypy()` never evicts.

### 3.14 Errors

`error.rs` (scaffold, frozen) holds one `Error` with structural variants: `Detect`, `OutOfBounds`, `Malformed`, `Unsupported(Unsupported)`, `Decompress`, `Version`, `TypeIndex`, `Tree`, `AssetBundle(Box<ParseError>)`, `Entry { name, source }`, `Limit`, `Budget`, `NotFound`, `Io`. `LimitKind` names one variant per `Limits` field (`TreeRows` included) plus `Count`; `OutOfBounds::pos` is the signed reader position (3.3); `DerefError` separates `FileIdPastEnd` from `FileIdBelowStart` (3.13). `walk::ParseError` is an object's L2 outcome (`l2_kind()` maps to the dump's kinds); `walk::WriteError` is L3's. All are `Clone + Send + Sync`. A new variant goes through package E (section 7.1); the enums are `#[non_exhaustive]`.

### 3.15 Limits and no panics

- **`Limits` are per input**: each counts or caps what one input (a top-level file, every bundle nested in it and every `SerializedFile` in those) builds, through the input's `Meter` (3.6), so a bundle of many entries cannot multiply them. Defaults (scaffold): block output 1 GiB; bundle uncompressed 4 GiB (actual decoded bytes, 3.6); inflate 4 GiB (streamed, never held); metadata records 2^22 (3.3); typetree rows 2^22 (3.8); tree depth 256; value depth 512; values per object 2^26; zero-size elements 2^20; pre-allocation 2^16 elements. Exceeding one is `Error::Limit`/`ReadDetail::Limit`, never a crash, and a test trips each (`limits::tests::every_metered_bound_trips_and_keeps_nothing`, the `DecodeBuf` tests in `budget.rs`; packages B and C add one per limit they enforce). `PARITY.md` lists each as a safety divergence where UnityPy would exhaust memory or the stack.
- **What one input can hold**, from the scaffold's own types (`tests/memory_model.rs` measures each structure's size with a counting allocator and sums the defaults' worst case, asserting it stays at or below 10 GiB):

  | term | from | GiB |
  |---|---|---:|
  | decoded data held | `max_bundle_uncompressed` | 4.00 |
  | one block's regrowth | `max_block_output`: a `DecodeBuf` growing to its cap may hold its old and new buffers at once | 1.00 |
  | metadata records | `max_metadata_records` x the largest record (`SerializedType`, 200 bytes) | 0.78 |
  | typetree rows | `max_tree_rows` x 248 bytes (a row at the peak of `NodeList::from_rows`, 224 bytes, plus its compiled `NodeInfo`, 24) | 0.97 |
  | one object's values | `max_values_per_object` x `size_of::<(NodeId, Value)>()` (48 bytes) | 3.00 |
  | **one input, worst case** | | **9.75** |

  This holds because strings are not copied (a string is a slice of its file, 3.2), lists never hold spare capacity (3.10), tables grow with the bytes and the meter (3.3), and every decoded buffer is reserved before it grows (3.6). It assumes a run grows in place or by one copy at a time; a multi-block run whose buffer the allocator must move while it grows is the regrowth term, and E's R3 measures it. A caller with bigger inputs raises the limits; the batch engine bounds the sum over inputs with its budget (4.1), which the limits do not.
- **The budget is for decoded bytes.** `MemoryBudget` pays for decoded and inflated buffers and dependency files (3.6, 3.13). Parsed metadata, typetree rows and values are bounded by `Limits` alone; a caller that runs several inputs at once adds their limits to its plan (4.1).
- **No count sizes an allocation, no arithmetic wraps.** Table growth follows 3.3. Input integers are combined in a wider type (`i64` offsets in `i128`, `i32` ids in `i64`) or with checked operations, because Python's integers never overflow and a wrapped value is a wrong answer in release and a panic in the debug builds the no-panic tests run in.
- `lib.rs` denies `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable` and `indexing_slicing` outside tests. A stub module's `todo!()` is allowed only by its `STUB(X)` line, which the owner deletes with every `todo!()`; G6 (`scripts/stub-check.sh`) fails the gate while any stub, allow or `todo!(` remains anywhere under `crates/`.
- Every package adds seeded no-panic tests: its builders' output mutated by `testkit::Rng` (bit flips, truncation), 10,000 cases per module in CI, asserting `Result`, never a panic.

### 3.16 Send + Sync and threads

`lib.rs` asserts `Send + Sync` at compile time for **every** public struct, enum and trait object of the crate (`unityrs-dump`'s `lib.rs` does the same for its own), and a test (`every_public_type_is_asserted`) scans the sources and fails when a public type is missing from the list. The core keeps no global state (no statics with interior mutability), spawns no threads, and never blocks inside a call: `MemoryBudget::try_reserve` refuses instead of waiting. Locks are `OnceLock` (single flight), `RwLock` for the provider cache and the registry, and one `Mutex` for a bundle's decoded runs, none held across a decode of another file, and **no lock is held across a call into user code** (a `TypeTreeProvider`, `CabResolver`, `Fetch`, `MemoryBudget` or `BlockCodec`), so a callback may re-enter the core.

---

## 4. How the batch engine and the Python bindings use it

### 4.1 Batch

`unityrs-batch` (next, not P1) builds on the core like this. The rule that keeps it deadlock-free: **memory admission holds for a job is paid only by that job's own handles, and nothing a job waits on is owned by another job.**
1. **Registry.** Open every input bundle with `LoadMode::Lazy` under a registry `FileContext` whose budget is the dependency pool (`dep_cache_bytes`); only headers and directories are read, so CAB names and `uncompressed_size()` are known for all of them. The shared `Environment` holds these handles as dependencies: whatever it decodes for a dependency (a deref or a resource read from another job) is reserved from the dependency pool, never from admission, and evicted by its LRU.
2. **Admission, outside the pool.** For each bundle, block until `uncompressed_size()` (capped at `Limits::max_bundle_uncompressed`) fits `max_inflight_uncompressed_bytes`, then build a job `FileContext` whose budget is a prepaid `Counting` for exactly that amount. Blocking happens only here, before the job enters the rayon pool; inside it the core only ever calls `try_reserve`, which cannot block.
3. **Work.** The job opens its **own** bundle handle on the same `Bytes` with the job context (a second open reads the header and directory again, nothing else), then `entry()`, `objects()`, filter by `class_id()` from the table (no value read), `read_typetree` only for selected objects, payloads handed on as `Bytes` slices. Its runs and parsed entries live in that handle, so no other job ever waits on them. Pointers resolve through `Environment::deref`, which accepts a file not loaded in the Environment: its bundle siblings come through the job handle's `Parent`, every other CAB through the shared, dependency-paid registry.
4. **Refusal.** A job whose decodes outgrow its prepaid amount (a straddling read decodes a block twice, 3.6) gets `Error::Budget`, never a wait; the engine drops the job's handle, which returns its memory, and re-admits the bundle alone at `max_bundle_uncompressed`.
5. **Release.** At the end the job drops its handle: every permit returns as the last slice of each buffer drops, so admission's books equal what is alive. Slices a job hands on keep their permits until the consumer drops them, and the engine counts them as in flight.

Each input's parsed metadata, rows and values are bounded by its `Limits` (3.15), so the engine plans `jobs x 3.15's per-input bound` beside the decoded bytes admission counts.

### 4.2 Python (P3)

`unityrs-py` maps `Environment`, `BundleFile`, `SerializedFile` and `ObjectRef` to UnityPy's classes one to one; `load()` uses `LoadMode::Eager`, so UnityPy's load errors surface where UnityPy raises them. `Value` converts to Python objects without a typetree: `Str` with `surrogateescape`, `F32` through the double widening, `Tuple` to `tuple`, `Prims::U8` to `list[int]`, `Dict` keys through `Parsed`'s lists. Reads release the GIL (every type is `Send + Sync`). **`fork()`**: the core holds no threads or global state, but a read in another thread may hold a bundle's run `Mutex`, a registry or provider `RwLock`, or be inside a `OnceLock` initialisation, and a forked child would inherit that lock held by a thread that does not exist. The rule is therefore the one PyTorch and grpcio state: no core call may be in progress in another thread at `fork()`; the bindings document it, and a `multiprocessing` user picks the `spawn` or `forkserver` start method, or forks before the first read. A child-side reset (an `os.register_at_fork` hook re-creating every lock) was rejected: the locks live inside `Arc`s the core cannot enumerate, and a half-built `OnceLock` value cannot be repaired. **Back-references**: UnityPy's objects point back (`obj.assets_file`, `assets_file.parent`, `parent.environment`); the core's `ObjectRef` and `Parent` carry the file and container handles, and the bindings keep the Python wrappers they built for them (a dict keyed by the core handle) so `obj.assets_file is f` holds.

---

## 5. The dump and the CLI

`unityrs-dump` (E) writes `docs/CANONICAL_DUMP.md` format 1 from core's public API.
- **Order** (section 5 of the format): inputs in bytewise order of their relative path; each input depth-first: its `file` record, then a `SerializedFile`'s objects in ascending signed `path_id`, or a container's entries in `files` dict order, recursively. `-j N` dumps inputs in parallel (rayon) and writes them in order, so output does not depend on `N`.
- **A failing entry fails its input**: an input whose entry fails to detect or parse gets one `file` record with `parsed_as` `"error"` and `{"kind":"parse"}`, nothing else. The dump reads each input with `BundleFile::load_all` to get exactly that.
- **JSON** is written by hand, ported from the spike's `json.rs` (string escapes as Python's `json.dumps(ensure_ascii=True)`, the metadata-string rule), never through serde. Values encode from `Value` alone.
- **L3**: each object's value re-saved with `ObjectRef::write_typetree` under `WriteRules::UnityPy` and digested as the format says.
- **Context**: `FileContext::unitypy()` and nothing else: no shim, no fallback version, no override. A TPK path (`--tpk`, the oracle image's UnityPy TPK) feeds `unityrs-tpk` for stripped objects.
- **Output**: gzip level 9, mtime 0, written to `OUT.partial` and renamed when complete; the output path must lie outside the repository (as the oracle's).

**CLI** (`unityrs-cli`, E):
- `unityrs dump PATH... | --index FILE [--kind K] [--sample N] [--name GLOB] -o OUT [--levels L0,L1,L2,L3] [-j N] [--tpk PATH]`, the same selection flags as `tools/parity/unitypy_dump.py`;
- `unityrs values CASES.json`: the spike's value-case protocol (`{"name","endian","nodes","hex"}` in, one JSON line per case out), so `tools/parity/value_cases.py` (moved from the spike and pointed at `unityrs`) drives it.

---

## 6. The parity gate

The gate is what "P1 core done" means. Every row is run by package E at the end and must hold on the merged branch.

| id | what | levels | pass |
|---|---|---|---|
| **G1** | the whole private corpus (build A (4100) and build B (100056) inputs of `/corpus/index.json`) and UnityPy's samples, dumped by `unitypy_dump.py` and by `unityrs dump`, compared by `tools/parity/compare.py` | L0-L3 | **zero differences**, no shim, no `--levels` subset; `compare.py` format findings zero on both sides |
| G1b | `unityrs dump -j 1` and `-j 8` of the same corpus | all | byte-identical uncompressed output |
| G1c | the corpus dumped with every bundle opened `Lazy` and its entries read in reverse directory order before the dump's own pass | all | byte-identical uncompressed output to G1's |
| **G2** | the 32 value cases of `value_cases.py`, plus every case the packages add | L2, L3 | L2 identical for every case; L3 verdict `match` or `same_bytes` wherever the oracle re-saves |
| **G3** | the synthetic format matrix: testkit-built files for every format, codec, flag set, endian and gate in P1 scope, dumped by both sides (the files are written to a temporary directory outside the repository and fed to both dumpers) | L0-L3 | zero differences |
| G3b | every testkit builder's output under seeded mutations (bit flips, truncation, and each count and length field set to negative and to huge values; 200 per builder, seeds in the PR), written outside the repository and dumped by both sides | L0-L3 | zero differences, or each difference a `PARITY.md` safety divergence (a limit) |
| **G4** | each package's oracle-recorded fixtures (section 7) replayed in `cargo test` without Python, the scaffold's included (`tests/scaffold_fixtures.rs`) | per package | all pass; `tools/fixtures/*.py --check` reproduces every committed fixture |
| **G5** | the Environment probe: the spike's 41 cross-file `PPtr` and 20 `.resS` checks, plus every testkit scene, in both load modes, **and UnityPy run on every testkit scene** (`tools/fixtures/env.py` records it) | env | 41/41, 20/20; `Eager` equals UnityPy on every scene; `Lazy` equals `Eager` on well-formed scenes and differs by exactly 3.13's (a), (b) and (c) on the rest |
| G6 | `sh scripts/stub-check.sh`: no `STUB(` line, no `clippy::todo`/`clippy::unimplemented` allow and no `todo!(`/`unimplemented!(` anywhere under `crates/` | | exit 0 |

**Also measured, reported in the PR, not a gate:** objects/s and MB/s serial and at 8 threads against UnityPyBoost and against the spike's rabex numbers (`docs/spikes/rabex.md` section 5), peak RSS, and the decode share of the run. A result slower than UnityPyBoost serially is a blocker to investigate before merge.

**Fuzz plan (after P1, nightly):** `cargo-fuzz` targets for `check_file_type`, `BundleFile::open` plus `load_all`, `SerializedFile::parse`, `parse_blob`/`parse_old`, `boost::read` and `python::read` over a fuzzed tree and bytes, and write-after-read; seeds from the testkit; one hour per target per night; any panic, hang over 10 s or allocation over 2 GiB is a bug. P1's seeded no-panic tests (3.15) are the down payment.

---

## 7. Work packages

### 7.1 Rules for every package

- **Branching.** Each package works in its own worktree: `git worktree add .wt/p1-<x> -b p1-<x> p1-core` (x in a-e). Merge order into `p1-core`: **A, B, C, D, E**, each rebased on what merged before it. Every package must pass `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` at every commit it merges.
- **Ownership.** A package edits only the files it owns (2.2 and below). The scaffold files are frozen: a needed change to a shared signature, an `Error` variant or a `Limits` field is written as a short request in the package's PR description and made by E in one integration commit, which the asking package then rebases on.
- **Dependencies.** Core's dependency list is final, its one dev-dependency (`serde_json`, pinned, to replay fixtures) included. Only A changes `Cargo.lock` (new crates `unityrs-rabex`, `unityrs-tpk`), and E (the dump's and CLI's dependency on `unityrs-tpk`). Others resolve lock conflicts by taking the merged base's lock.
- **Tests are synthetic.** Inputs are built in code with the testkit; expected results come from the oracle. A package that records fixtures adds `tools/fixtures/<package>.py` (runs in the oracle image, builds inputs in code, runs UnityPy 1.25.2, writes the inputs as hex and UnityPy's results as small JSON under `crates/unityrs-core/tests/fixtures/<package>/`, one case per line, with a `--check` mode that regenerates and compares); `cargo test` replays them without Python. The scaffold's own fixtures are the model: `tools/fixtures/scaffold.py` writes `tests/fixtures/scaffold/{reader,pptr_external,lz4_blocks}.json` (the reader's position rules, `external_index`, and liblz4's verdict on boundary LZ4 blocks), and `tests/scaffold_fixtures.rs` replays them. No corpus byte, name or path enters a commit.
- **No stubs at merge.** A package deletes its `STUB(X)` allow and every `todo!()` in the files it owns; `scripts/stub-check.sh` lists what remains (G6).
- **Milestones.** Each package pushes its interface milestone first (listed below); other packages may merge that branch into theirs read-only to unblock tests, since files do not overlap.

### 7.2 Package A: the fork, tables, TPK, licences

- **Owns:** `crates/unityrs-rabex/**`; `crates/unityrs-tpk/**`; `tools/gen/**`; `scripts/fork-check.sh`; `about.hbs` and `about.toml` (cargo-about's template and configuration, from which `THIRD_PARTY_LICENSES` is generated, 2.5); `deny.toml`; the CI job for `fork-check.sh` and `gen_tables.py --check` in `.github/workflows/ci.yml`; in core, the table bodies of `tables.rs`, `classid.rs` and `typetree/commonstring.rs`.
- **Depends on:** the scaffold. `unityrs-tpk` picks a TPK entry with the scaffold's implemented `UnityVersion::from_packed`, `packed_u64` and ordering (3.4), so it does not wait for C; it uses B's `Tree::compile` (stubbed until B lands; its tests that compile trees are marked to run after the rebase on B).
- **Milestone (day 1):** the four generated tables committed.
- **Work:** F0-F4 (2.4); `UPSTREAM.md`; `fork-check.sh`; the fence (2.3); `gen_tables.py` with `--check`; `TpkProvider` (3.12); the UnityPy, rabex, CNStudio and common-string provenance notices in `about.hbs`, after the integration merge brings cargo-about in (2.5); `deny.toml` (the per-crate 0BSD exception, the rabex ban); the no-TPK checks (2.5).
- **Tests:** the generators' assertions (2.6); `tables` tests for the full `Nd` set and every lower exception; `fork-check.sh` clean; F2/F3 regression tests on synthetic TPKs built in code (an unknown class, a corrupt flag byte); a TPK node-list diff against UnityPy's `Tpk.py` over every class at a grid of versions (oracle-recorded, a fixture per class family), identical.
- **Done when:** the tests pass; `fork-check.sh` is clean; `UPSTREAM.md` gives every patch's class and reason; no `rabex::` use outside `unityrs-tpk` and the spike; `cargo deny check` passes.

### 7.3 Package B: compile, read, write

- **Owns:** `typetree/tree.rs`; `walk/mod.rs` (the `read_typetree` body); `walk/boost.rs`; `walk/python.rs`; `walk/write.rs`; `tools/fixtures/walk.py` and its fixtures.
- **Depends on:** the scaffold only (`NodeList`, `Value`, `Reader` is C's but B reads slices directly or through its own cursor).
- **Milestone (day 2):** `Tree::compile` and `boost::read` passing the 32 value cases.
- **Work:** 3.8 (compile), 3.10 (R1-R16, the pure-Python list, floats, bounds, list growth), 3.11 (W1-W7, `MirrorReader`), `check_read`.
- **Fixtures** (`tools/fixtures/walk.py`), each built in code and recorded from UnityPy 1.25.2, Boost and pure-Python where both apply: R9, a `string`, `int`, `TypelessData` and `pair` node each with an `Array` first child (read as its own kind), and an `Unknown`/`PPtr` node with one (read as an array); R14, each failure in order (no file, no or non-list `ref_types`, no `type`, a missing `class`/`ns`/`asm`), the empty-class skip with and without `ref_types`, a matched ref type with no tree (`data` skipped), no match; R10, an empty array of a 3-child `pair` (Boost raises, Python `[]`); the pure-Python list, each item of 3.10 recorded with `byte_size == 0`, the five probed items included and an object at an odd `byte_start` for the file-relative alignment; W2, every node kind against every `Value` variant (written bytes or the exception class); W7's two failures.
- **Tests:** one test per rule id (`r09_negative_array_length_is_negative_length`, …); the fixtures above; the 32 value cases as G4 fixtures (L2 and L3); a list of 2^16 + 1 elements whose capacity equals its length after the read (3.10); `FloatMode` both ways; a `ReferencedObject` whose data re-enters another ref type (two lists in `Parsed`); registry scope; each limit tripping to `other`; linear dicts: a class of 2^16 children whose names repeat late reads in a number of map steps linear in the children (a step counter, never a clock), and a repeated name after a registry child skipped at run time lands where `PyDict_SetItem` puts it; seeded no-panic tests over random trees and bytes; `size_of::<Value>() <= 40`.
- **Done when:** G2 passes through `unityrs values` (with E's harness, or the fixtures alone before E lands); every R and W rule has its test.

### 7.4 Package C: streams, versions, detection, codecs, files

- **Owns:** `stream.rs` (the `Reader` methods; the `py_*` helpers are the scaffold's), `version.rs` (`parse` and `from_str`; `from_packed`, `packed_u64` and the ordering are the scaffold's), `detect.rs`, `codec.rs`, `files/**`, `typetree/parse.rs`; `crates/unityrs-testkit/src/{blob,serialized,bundle}.rs`; `tools/fixtures/files.py` and its fixtures.
- **Depends on:** the scaffold; A's `ND_ZEROS` for non-ASCII versions (ASCII works from the seed); B's `Tree::compile` only through `SerializedType::tree`.
- **Milestone (day 1-2):** the testkit builders (blob, old tree, `SerializedFile` 9-22, UnityFS 6-8 with every codec), since D and E build their tests on them.
- **Work:** 3.3-3.7, `typetree/parse.rs` (3.8).
- **Tests:** the reader against `tests/fixtures/scaffold/reader.json` (every method at positions from below `-len` to past the end: `py_read`, `py_unpack_from`, one-byte reads, `py_align`, `py_string_to_null`); `UnityVersion` against oracle fixtures (types, `c`/`x`, builds above 255, spills, Unicode digits, the fallback chains and every `VersionError`), with a group of 4,300 and of 4,301 digits in each of major, minor, build and number, written with leading zeros and with non-ASCII `Nd` digits (4,300 parses, 4,301 is `NoMatch`); the negative-position sites of 3.3, each against the oracle (a negative `node_count` and `stringbuffer_size`, a format-8 header whose `metadata_size` exceeds `file_size`, blocks-info at the end with `compressedSize > Length`, a format-22 `byte_start` above `i64::MAX`); the input's `Meter`: `max_metadata_records` and `max_tree_rows` tripped across two `SerializedFile`s of one bundle, neither of which trips it alone; codecs: `lz4_blocks.json` replayed through the LZ4 codec with liblz4's verdict and output for every block (offset 0 included); detection: every L0 rule and quirk (oracle-recorded); codecs: LZMA over-long and truncated streams, fewer than 5 LZMA bytes, the output cap, LZ4 short output accepted, long output refused and an empty block refused, LZHAM and unknown ids refused; bundles: every flag set, blocks-info front and end, padding, both alignment rules, a truncated block, the declared-total limit refused at open and the actual-total limit tripped by an LZMA block that outgrows its declaration, the budget topped up for that block and released when the last slice drops, and a budget smaller than the block refusing it before the output outgrows the budget (`DecodeBuf`, 3.6); runs: a straddling read decoding the shared block a second time into its own run, `entry_range` returning without caching, and `load_all` leaving one run; `Parent`: an entry's weak handle upgrades while the bundle lives and fails after it drops; **one layout**: on bundles where a short LZ4 block, an over-long LZMA block, a stored block whose stored size differs from its declared size, or a truncated block lies *before* the entry read, every entry read lazily in every order (first, last, reverse, seeded random) equals `load_all`'s bytes; entry slicing: negative offsets, size -1, size 0 and stops past the end, against Python slices, and `stream::py_read` against a Python-recorded fixture (including `i64` extremes); counts: a negative count gives zero records and a huge one fails at the end of the bytes with no allocation from the count, for every table (types, objects, scripts, externals, ref types, blocks, directory), and `max_metadata_records` trips on one-byte externals; `read_string_to_null` with no NUL in the window, both sides of `pos + 32_767 >= len`; `SerializedFile`: every gate at its boundary versions both endians, negative and out-of-range type ids, script write-back, repeated path ids, non-UTF-8 metadata kept raw; `parse_blob`: level jumps, unknown common offsets, `i16` versions, a negative node count and string-buffer size; seeded no-panic tests on every builder.
- **Done when:** the tests pass; G3's L0-L1 rows have no differences on C's matrix (with E's dumper, or C's own fixtures before E lands).

### 7.5 Package D: providers, objects, container, Environment

- **Owns:** `pystr.rs`, `provider.rs`, `object.rs`, `container.rs`, `env/**`; `crates/unityrs-testkit/src/env.rs`; `tools/fixtures/env.py` and its fixtures.
- **Depends on:** C's builders (milestone) for scenes, B's reader for `read_typetree` and `read_container` (merge B's branch for tests).
- **Milestone (day 2):** `pystr` and `ProviderChain` complete.
- **Work:** 3.2's `pystr`, 3.12, 3.13, the eager AssetBundle check's container half.
- **Fixtures** (`tools/fixtures/env.py`), recorded from UnityPy 1.25.2 on testkit scenes: two loose files whose pointer resolves through the Environment (rule 4); a bundle `[good CAB, its .resS, bad CAB]` (load raises, `files` empty, the first two CABs registered and resolvable); a nested bundle after a CAB of the same path (difference (b)); a bundle whose last block fails after every entry (difference (c)); the container preload over `FileIdPastEnd` (swallowed) and `FileIdBelowStart` (raised).
- **Tests:** `py_lower` against Python on every code point in the BMP and astral samples (a generated fixture), invalid bytes, final sigma; `ntpath` cases (drives, UNC, trailing separators, dots); the provider cache key (two versions of one class get two trees; a negative answer is cached); tree resolution order, and `Environment::object_tree` equal to `ObjectRef::tree` with no script provider, and with a mock script provider dereferencing `m_Script` (a null script and a provider miss fall back; a dangling external fails); every `DerefError` (null, negative file ids, `i32::MIN` and `-len - 1` as file ids through the scaffold's `external_index`, loose files resolving, sibling match by lower-cased name without basename, `find_file`, raw target, missing id, `from` not loaded in the Environment); the four resource candidates and the dependency retry; registry dict semantics and partial registration; eager equals lazy on every well-formed scene, and each of 3.13's differences pinned (a failing entry, a failing entry shadowed by a later one of its path, a nested bundle, a nested bundle after a CAB of its path, a late block failure); the eager AssetBundle read in object mode against dict mode (3.7, oracle-recorded); LRU eviction never evicts a caller-loaded file, and every permit is released; the container with preload rows.
- **Done when:** the tests pass and G5 holds.

### 7.6 Package E: integration, the dump, the gate

- **Owns:** `crates/unityrs-dump/**`, `crates/unityrs-cli/**`, `tools/parity/value_cases.py` (moved from the spike), `PARITY.md`, this plan (status updates), the integration commits for shared-file requests (7.1).
- **Depends on:** everything, at the end. Before that it works against stubs and hand-built `Value`s.
- **Milestone (day 2):** the JSON writer and value encoder, tested against `tools/parity/canonical.py`'s encodings of the same values (a fixture of encoded strings).
- **Work:** section 5; the gate of section 6, G3b's mutator and G1c's reverse-order pass included; `PARITY.md` entries for every opt-in and safety divergence this plan names (the limits, `FloatMode::Lossless`, `WriteRules::MirrorReader`, the LZMA cap); the perf report (R2, R3: re-decoded bytes, the peak of cached runs, peak RSS against live permits).
- **Tests:** the encoder against `canonical.py` fixtures; order and the `-j` independence on testkit corpora; a failing entry failing its input; header and `end` records; `compare.py` reports zero format findings on a testkit dump; a version whose packed integer passes 4,300 digits dumps its exact integer (the oracle cannot, CANONICAL_DUMP section 7); `scripts/stub-check.sh` exits 0 on the merged branch.
- **Done when:** G1-G6 hold on the merged `p1-core`, with the commands and outputs recorded in the PR description (counts only, no corpus names), and the spike crate can be removed in a follow-up.

### 7.7 Rough sizes

| package | new lines (approx.) |
|---|---:|
| A | 1,200 (plus the 6.8k-line pristine import) |
| B | 2,600 |
| C | 3,000 |
| D | 1,800 |
| E | 1,500 |

---

## 8. Risks, and what to measure first

| id | risk | first measurement |
|---|---|---|
| R1 | the 32 value cases still miss a Boost rule the corpus uses | B's day-2 milestone through `unityrs values`; then a 20-bundle corpus sample through E's dumper before the full run |
| R2 | measuring the blocks before an entry makes a lazy read of a late entry cost a discarded decode of everything before it; a straddling read decodes a shared block again | C's one-layout tests check bytes; E's perf report counts discarded-decode and re-decoded bytes on the 20-bundle sample, read in directory order |
| R3 | memory: a decoded run held by one small `Bytes` slice keeps a large buffer alive; cached runs are never evicted while the bundle lives; a growing multi-block run may be moved by the allocator (3.15's regrowth term) | E's perf report includes peak RSS against the sum of live permits and the peak of cached runs per bundle; if a long-lived slice pins a big run, copy that value out (`Bytes::copy_from_slice`) above a size threshold |
| R4 | throughput below UnityPyBoost (owned `Value`, `Arc` handles) | E's perf report at the 20-bundle sample; the walker's run reads are the first place to look |
| R5 | `Packed::Big` versions or exotic digits in the corpus | C's tests plus a count of resolved-version sources in the G1 run |
| R6 | the eager AssetBundle read at load disagrees with UnityPy on a broken container | D's tests; G1 counts `parse` errors on both sides |
| R7 | `py_lower` drifts with Rust's Unicode version | the `LOWER_EXCEPTIONS` generator and D's all-code-point fixture; re-run on every toolchain bump |
| R8 | the four TPK-derived common strings are not acceptable for provenance | decided (section 9, Q2); the fallback, if that changes, is a user-supplied table loaded at runtime, at the cost of L1 parity on files that use those offsets |
| R9 | the LZ4 codec accepts or refuses a block liblz4 does not (3.6) | C replays `lz4_blocks.json` in full; the fuzz plan adds an LZ4 target compared against python-lz4 |

---

## 9. Questions for the owner

**Open:**
- **Q5. When to offer F2-F4 upstream.** The spike opened nothing upstream; the one outside pull request there is unanswered. Any offer needs the owner's approval first.

**Decided** (with the reason):
- **Q1. The base-layer decision is relabelled: yes.** The core supersedes rabex's container, SerializedFile, typetree and serde code, so `DESIGN.md` section 4 now says the core is a port of UnityPy's rules on raw bytes, with rabex vendored pristine plus a patch ledger for the TPK reader (P1) and UnityCN and the builders (P3).
- **Q2. The four TPK-derived common strings stay in the generated table**, provenance recorded in `about.hbs` and the generator: they are Unity's own type names, needed to read files that use them, and ship in UnityPy's MIT repository.
- **Q3. `LoadableReference` (offset 1209) reads as `"1209"`**, UnityPy 1.25.2's Tier A answer; naming it is a candidate opt-in for later, not in P1.
- **Q4. `unityrs-rabex` is not published in P1** (`publish = false`): publishing needs a name and a maintenance promise.
- **Q6. UnityPy mode stays the default in P1**; `Lossless`/`MirrorReader` remain opt-in, revisited when the public API is stabilised for the bindings (P3).
- **Q7. UnityWeb, UnityRaw and WebFile wait for P3**, refused in P1 with a named `Unsupported`.
- **Q8. `py_lower`'s Unicode table follows the oracle's Python** (3.11, Unicode 14.0), and moves with it.
- **Q9. The lowered defaults stay** (3.15): the largest corpus bundle is far below 4 GiB, and a caller with bigger inputs raises them explicitly. The fix pass lowered `max_metadata_records` from 2^24 to 2^22 and added `max_tree_rows` at 2^22, so the per-input worst case (9.75 GiB, `tests/memory_model.rs`) fits a 12 GiB process.
- **Q10. Lazy mode's difference on nested containers is accepted** (3.13, (b)): exactness would mean decoding every block before each entry at open and giving up the headers-only registry phase, and nested bundles are not expected in real builds.
- **Trusting declared block sizes as an opt-in: no**, not in P1: UnityPy ignores them for LZMA and treats them as a maximum for LZ4 (3.6), and an opt-in would be a second layout to test.

---

## 10. Reproducing

```sh
# Checks, in the dev container (from the repository root):
scripts/dev cargo fmt --all --check
scripts/dev cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/dev cargo test --workspace --locked
scripts/dev sh scripts/fork-check.sh                     # A
scripts/dev python tools/gen/gen_tables.py --check       # A
scripts/dev python tools/fixtures/scaffold.py --check    # the scaffold's oracle fixtures (3.3, 3.6, 3.13)
scripts/dev cargo test -p unityrs-core --test memory_model -- --nocapture   # 3.15's table
sh scripts/stub-check.sh                                 # G6; lists the stubs left

# The gate (E), outputs outside the repository:
scripts/dev python tools/parity/unitypy_dump.py --index /corpus/index.json -j 8 -o /tmp/p1/oracle.jsonl.gz
scripts/dev cargo run --release -p unityrs-cli -- dump --index /corpus/index.json -j 8 \
    --tpk <the oracle image's UnityPy TPK> -o /tmp/p1/unityrs.jsonl.gz
scripts/dev python tools/parity/compare.py /tmp/p1/oracle.jsonl.gz /tmp/p1/unityrs.jsonl.gz
scripts/dev python tools/parity/value_cases.py target/release/unityrs
sh scripts/stub-check.sh
```
