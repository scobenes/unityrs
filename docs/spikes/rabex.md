# rabex spike

**Status:** P0, 2026-10-28. **Subject:** rabex 0.2.1 from crates.io (pinned `=0.2.1`, default features off). **Oracle:** UnityPy 1.25.2 with UnityPyBoost (`tools/parity/unitypy_dump.py`). **Code:** `crates/unityrs-spike-rabex` (`publish = false`).

This spike decides how unityrs builds its base layer (`DESIGN.md` section 4, "Base layer: rabex"). The decision applies that section's three rules to measurements taken on the private corpus (a live mobile-game corpus) and on UnityPy's samples. Section 14 names the command behind every number.

## Recommendation: vendor a fork now

As `DESIGN.md` section 4 was first written, no route's conditions were fully met. The fork row asked for changes "that upstream won't take", and that is unproven: the one outside change offered upstream is still unanswered (section 12). The fork row now reads "that upstream has not accepted (we offer them upstream and revisit if they merge)". Against the rules as amended (section 13):
- **"Use upstream, pinned" fails today** on two conditions: L1-L3 are not at 100% (UnityPy's format-6 samples fail on one header bug), and fuzz-or-accept is unmet (no fuzz run, 5 panics reproduced through the public API, and no fix accepted upstream).
- **"Vendor a fork" applies.** L1 is sound: every record of the corpus game passes L0-L3 with rabex unmodified (403,009 L1 records, 399,883 objects), and a 1-hunk header fix brings every level to 100% on every input. The changes we need are invasive (below), and upstream has accepted none: its one outside pull request, the header fix, has had no review or comment since 2026-10-23.
- **"Port UnityPy's core" does not apply.** The rewrite is `serde_typetree`, 22.7% of rabex's library code, and 37.5% with the two generated tables the fork regenerates (`class_id`, `commonstring`): below half (section 11).

So: vendor the fork now, offer each fix upstream, and revisit "use upstream, pinned" for the container layer if upstream merges them. Opening pull requests or issues upstream is the repository owner's call; this spike opened none.

**What makes the changes invasive.** Not the `Value` representation as such. `value.rs` builds UnityPy's dict model through rabex's public serde API and reaches 100% L2 and L3 on all 399,936 corpus objects, so serde is a usable extension point for the model (section 4). The triggers are:
- read-rule bugs in `serde_typetree/de.rs`: alignment, negative lengths, truncated arrays, and node kinds it cannot read (section 4);
- write-rule bugs in `serde_typetree/ser.rs`: 21 live `todo!()`, 27 `unreachable!()`, and unaligned non-UTF-8 strings (sections 4 and 8);
- bounding: rabex reads an object unbounded in the whole file and accepts a short read, and it allocates up to 1 GiB for a hostile length before the read fails (sections 1 and 4);
- panics reachable from untrusted input (section 10);
- the save layout (section 8).

What the fork changes first, in order:
1. The UnityFS header byte, and UnityPy's format-6 alignment rule, which applies when 2019.4.15 ≤ version < 2020.
2. `serde_typetree`'s read rules, rewritten to UnityPyBoost's `read_typetree`, and its write rules to UnityPy's pure-Python `write_typetree`, which is what the L3 oracle re-saves with (UnityPyBoost has no writer; section 4). On read: alignment, negative lengths, the truncated-array masking, ReferencedObject (which needs the file's ref types inside the reader) and `Type*` nodes. On write: alignment after non-UTF-8 strings, and UnityPy's per-element alignment, which its own reader does not mirror; the fork reproduces that or lists it in `PARITY.md` as a deliberate divergence (section 4). Each read is bounded to the object's bytes, and the bytes consumed are checked; the spike does both outside rabex today (section 1). Whether the rewrite keeps serde as its interface or walks into a raw-bytes `Value` is open: no measured case needs a raw-bytes `Value` (section 4).
3. Panics become errors: an unknown TPK class, UnityArchive and UnityWeb signatures, malformed maps, the bundle reader's `discard_front`, and every `unwrap()` and `todo!()` on a read or write path untrusted input can reach (the SerializedFile writer's `unwrap()`s below version 14 among them, section 8). Add bounds on recursion depth and allocation.
4. The SerializedFile metadata gates for versions below 14, in the reader and the writer alike (the two disagree today, section 8), the raw Unity version string, and the bundle-version fallback. For the HEAD tier: SerializedFile 23-26 and UnityFS format 9 (section 7).
5. `TypeTreeCache` keyed by `(ClassId, UnityVersion)`, not by `ClassId` alone (section 5).
6. liblzma instead of lzma-rs. On every LZMA block of the corpus, rabex's lzma-rs call takes 24.9 s and liblzma 0.4.9 `static` (xz 5.8.4) takes 10.3 s: lzma-rs is 2.41x slower, and it is 89% of rabex's serial run (section 5).
7. The writer: data order, UnityPy's header flags, and errors instead of `todo!()` and `unwrap()` (section 8).
8. Licence and provenance (section 9): delete `resources/lz4.tpk` and the `embed-tpk` feature; regenerate `commonstring.rs` from UnityPy's MIT `CommonString` and `objects/class_id.rs` from UnityPy's MIT `ClassIDType` (both are generated from AssetRipper's unlicensed TypeTreeDumps), or record their provenance; name the authors, Rudolf Kolbe and Jakob Hellermann, in `THIRD_PARTY_LICENSES`, and carry the notices of the outside code two vendored files credit (CNStudio's for the UnityCN decryptor, UnityPy's for the TPK node walk); take the code under MIT.

## 1. What was built

`rabex-dump` (in `crates/unityrs-spike-rabex/src/main.rs`) uses rabex for every byte it reads:
- `dump` writes the canonical dump (`docs/CANONICAL_DUMP.md`) at L0-L3, so `tools/parity/compare.py` scores rabex exactly as it will score unityrs;
- `bench` measures throughput, serially and with rayon (`--phase` runs one side alone);
- `probe` runs the Send/Sync check, the Environment, cross-file PPtr and `.resS` probes, the TPK rows, the TPK cache's two-version reproduction, and the crafted panics;
- `values` reads the synthetic typetree cases;
- `save` re-saves bundles through rabex's writers;
- `lzma` decodes every LZMA block with rabex's codec call and with liblzma.

The scripts in `scripts/` are the UnityPy side of each comparison, plus `mem_peak.py` (memory), `rabex_loc.py` (section 11), and `commonstring_compare.py` and `classid_compare.py` (section 9). Their unit tests are in `scripts/tests/`.

rabex has no raw value walker. `SerializedFile` hands out values only through `serde_typetree::Deserializer`, which drives a serde `Visitor`. A plain visitor cannot tell a struct from a `pair`, or `vector<UInt8>` from `TypelessData`. So `value.rs` seeds every visit with the node rabex is reading, and rebuilds UnityPy's dict model (tuple versus dict versus list, bytes versus a u8 list, float width) from the node. What rabex itself reads (alignment, lengths, the kinds of node it supports) cannot be changed from outside. That is what the corpus and section 4 measure.

**Layered on top, because rabex does not provide it.** The results that depend on these layers are the spike's, not rabex's:
- L0 detection, a port of UnityPy's `check_file_type` (`detect.rs`);
- the bundle fields rabex parses but does not expose: data flags and the blocks-info row;
- the node level and pre-order index columns;
- the container rows, read from the AssetBundle object's typetree;
- **bounded reads.** The spike reads each object from a slice cut to the object's own bytes (`dump.rs`, `object_bytes`) and checks the bytes consumed against the object's size (`check_read`). rabex's own `SerializedFile::read` and `ObjectRef::read` (`serializedfile/mod.rs`) seek in the whole file and read unbounded. Through rabex's API, a tree that over-reads continues into the next object and returns a value, and one that under-reads is accepted silently. The `check_read` and object-boundary `out_of_bounds` outcomes in section 4 come from this layer;
- **L1 members the spike writes itself.** L1 passing on these tests the spike's code, not rabex's (`dump.rs`):
  - a bundle record's `encrypted` (`false`), `hash` and `crc` (both `null`), written as constants;
  - `flags_class`, the spike's copy of the engine-version rule, because rabex's own `use_new_archive_flags` (`bundlefile/mod.rs:572`) is private;
  - `block_alignment`, written as "format 7 or later": a description of rabex's rule, not a value rabex reports;
  - `big_id_enabled`, always 0, because rabex reads the field and discards it (section 7);
  - `engine_version_packed` and `version_packed`, packed by the spike's `pack_version`; a SerializedFile without its own Unity version takes the parent bundle's, a fallback rabex lacks (section 7);
  - the type records' absent members: rabex fills a field the version does not have with a default, and the spike writes `null` there by its own version gates (`type_record`), as UnityPy leaves it `None`;
  - `object_order`, and which object records are written: UnityPy's `objects` dict, one object per path ID in first-appearance order with the last entry winning (`object_dict`), not rabex's object list;
- **the TPK** (`tpk.rs`), loaded from a path (`--tpk`) rather than compiled in, behind a cache keyed by class and version (section 5).

`--shim-fs6` works around the header bug (section 3) by inserting the byte rabex wrongly consumes. Every subcommand leaves it off by default, so a default run measures rabex as published; the runs below say when it was on.

## 2. Corpus

| part | inputs | content |
|---|---|---|
| corpus game, build A (4100) | 551 bundles | Unity 6000.3.17f1, UnityFS format 8, SerializedFile version 22 |
| corpus game, build B (100056) | 545 bundles | the same engine line |
| UnityPy samples (commit `417d595`) | 5 files | 4 UnityFS format 6 bundles (5.6.7f1, 2017.4.37f1, 2018.4.4f1, 2018.4.11f1; SerializedFile version 17) and 1 file UnityPy reads as raw (an OBJ mesh) |

That is 1,101 inputs, 470,133,684 bytes, 3,139 file records and 399,936 objects. The oracle dump is 191.0 MB gzipped (1.81 GB of JSON).

The corpus covers one modern engine line and four old sample bundles. Every other row of the format matrix comes from reading rabex's source (section 7) and from synthetic cases (section 4).

## 3. Parity, L0-L3

Pass rates are from `scripts/pass_rates.py`, which counts a record as passing a level when the candidate has it and that level's members match the oracle.

| part | level | rabex 0.2.1 as published | with `--shim-fs6` |
|---|---|---|---|
| all | L0 | 3,127 / 3,139 (99.618%) | 3,139 / 3,139 (100%) |
| all | L1 | 403,010 / 403,075 (99.984%) | 403,075 / 403,075 (100%) |
| all | L2 | 399,883 / 399,936 (99.987%) | 399,936 / 399,936 (100%) |
| all | L3 | 399,883 / 399,936 (99.987%) | 399,936 / 399,936 (100%) |
| corpus game | L0-L3 | 100% at every level (3,126 files, 403,009 L1, 399,883 objects) | 100% |
| UnityPy samples | L0 / L1 / L2 / L3 | 1/13, 1/66, 0/53, 0/53 | 13/13, 66/66, 53/53, 53/53 |

With the shim, `compare.py` reports the two dumps identical: 403,076 records, no differences.

**Failure classes:**

| class | records | cause |
|---|---|---|
| UnityFS format 6 header | 4 bundles: 12 L0, 65 L1 and 53 L2/L3 records (61 of them missing from the candidate) | `read_unityfs_info` (`src/files/bundlefile/mod.rs:616`) reads one byte when the signature *is* UnityFS; UnityPy reads it only for the other signatures. Every format-6 bundle is one byte off. Format 7 and later is off by 16 when the header ends 16-aligned, and otherwise the `align(16)` that follows hides the byte. The blocks-info read then fails with "the offset to copy is not contained in the decompressed buffer". |
| everything else | 0 | n/a |

Upstream PR #1 fixes the inverted condition. It also adds a zero-padding guess for 2019.4, where UnityPy uses an explicit version rule (2019.4.15 ≤ version < 2020, `BundleFile.py`). The corpus has no 2019.4 bundle to separate the two.

## 4. Value semantics beyond the corpus

The corpus game exercises one engine line, so `scripts/value_cases.py` builds 32 typetrees and their bytes. Each one aims at a place where rabex's `serde_typetree/de.rs` reads differently from UnityPyBoost's `TypeTreeHelper.cpp`. The oracle side reads with UnityPyBoost's `read_typetree` and re-saves with UnityPy's pure-Python `write_typetree` (UnityPyBoost exports no writer); the rabex side uses `rabex-dump values`. L2 compares what the canonical dump's L2 record holds: the error kind, `bytes_read` (an integer only for `check_read`) and the value.

**L2: 18 of 32 match, plus 1 by-design difference.** L3 compares the re-saved bytes, with each other and with the input, and only for the cases both sides re-saved. **L3: in 8 of 12, both re-saves equal the input.** In `bool_byte_2` the two re-saves are the same bytes (`01000000`), but neither equals the input (`02000000`): the two agree, and neither round-trips. The 3 whose re-saves differ from each other are `f32_signalling_nan` (by design), `invalid_utf8_string` and `invalid_utf8_dirty_pad` (below). The other 20 say nothing about L3:
- in 7, only UnityPy re-saved, because rabex could not read the case (L2 counts these): the five alignment cases, `array_first_child_with_siblings` and `type_named_node`. In two of them, UnityPy's own re-save differs from the input (below);
- in 13, neither side re-saved, because both stopped at a read error or both writers refused: the two negative lengths, the six truncations, `string_past_end`, `short_read`, `empty_struct_align_last`, `referenced_object_registry` (UnityPy's writer refuses it) and `duplicate_child_names` (both writers refuse it).

In the table, † marks an outcome produced by the spike's bounded read (section 1). With rabex's API alone, such a read runs on into the next object or ends early without an error.

| case | UnityPyBoost | rabex 0.2.1 |
|---|---|---|
| `typeless_data_align` | aligns after flagged `TypelessData` | never aligns, so the next field reads 3 bytes early (`check_read`†) |
| `empty_struct_align` | `{}`, aligned when flagged | `Kind::Empty`: unit, no align (`de.rs:147`), so the read ends early (`check_read`†) |
| `empty_struct_align_last` | the same struct as the last field, before 4 spare bytes: `check_read` after 4 bytes | `check_read` after 1 byte†, with the same value: only `bytes_read` tells the two apart |
| `map_array_align` | the Array child's flag aligns a map | checks the map or pair flag instead (`check_read`†) |
| `primitive_element_align` | flagged primitive elements align **once**, after the array | aligns after every element (`out_of_bounds`†) |
| `pair_element_align` | same rule for pair elements | aligns per element (`out_of_bounds`†) |
| `negative_string_length` | raises `negative_length` | reads 2^32−1 as the length and refuses it as above its 1 GiB read limit ("other") |
| `negative_vector_length` | raises `negative_length` | reads 2^32−1, runs out of bytes on the third element, and masks that as "did not exhaust sequence" ("other") |
| `vector_u8_truncated` | `out_of_bounds` | the same masking: "did not exhaust sequence" ("other") |
| `vector_float_truncated` | `out_of_bounds` | the same masking ("other") |
| `referenced_object_registry` | reads `ManagedReferencesRegistry` (SerializeReference data, 2019.3+) | `unimplemented: ManagedReferencesRegistry` (`de.rs:157`) |
| `array_first_child_with_siblings` | an array when the first child is `Array` | "invalid type: Array, expected a list" |
| `type_named_node` | reads the node | `Kind::TodoType` reads nothing (`de.rs:160`), so the read ends early (`check_read`†) |
| `f32_signalling_nan` (by design) | quiets `7f800001` to `7fc00001` | keeps the bits; rabex is the lossless one |

Cases where the two agree:
- alignment of structs and of the Array node;
- strings past the end and short reads, with the same error kinds;
- truncated arrays whose last element is the one that fails (`int`, struct, `string` and map elements);
- a pair outside a map, maps with string keys, and duplicate child names;
- nested vectors, `UInt64` max, `FileSize`, `Type*`, scalar `char` and `SInt8`;
- an empty float vector and big-endian data;
- a bool byte of 2, which both read as true and both re-save as `01`, so neither round-trips.

**The truncation masking.** rabex's `SeqDeserializer` counts the elements down before reading each one. When an element fails, `deserialize_seq` sees elements still to come and returns "did not exhaust sequence", which replaces the real end-of-data error (`de.rs:305-323`). So a truncated array reads as `out_of_bounds` only when its last element is the one that fails; otherwise it reads as `other`. `negative_vector_length` is this masking as well: rabex reads −1 as a length of 2^32−1, runs out of bytes on the third element, and reports the masked error. `negative_string_length` attempts no 4 GiB allocation: `read_bytes_sized` refuses any length above its 1 GiB `MAX_READ_BYTES_LEN` (`read_ext.rs:60-63`), and allocates any length up to that before the read fails. The walker the fork writes must follow UnityPyBoost's rules here: a negative length is its own error, and the end of data is reported as such.

**UnityPy's writer does not mirror its reader.** `write_typetree` aligns after every element whose node carries the align flag (`helpers/TypeTreeHelper.py`, `write_value`), where the reader aligns once, after the array. So UnityPy fails its own L3 on `primitive_element_align` and `pair_element_align`, which only UnityPy could re-save: for `primitive_element_align` it writes `02000000 01000000 02000000 07000000` from the input `02000000 01020000 07000000`. The oracle records `resave: "differs"` for any such object. A fork writer that mirrors its reader round-trips it, and so disagrees with the oracle's L3 record; the fork reproduces UnityPy's writer here or lists the difference in `PARITY.md` (change 2).

**L3:** `invalid_utf8_string` reads equal, but rabex's resave is wrong. rabex serialises a string holding invalid UTF-8 through `serialize_bytes`, which writes no alignment (`ser.rs:281-293`); `serialize_str` aligns. `invalid_utf8_dirty_pad` is the same string with its alignment byte set to `01`. Both sides read it equal; UnityPy re-saves a zero pad and rabex no pad, so neither re-save equals the input, and the two differ from each other. Comparing only whether each side "differs" from the input would have scored it as agreement.

None of these shapes occurs in the corpus game. `ManagedReferencesRegistry` is what Unity writes for `[SerializeReference]` fields (2019.3+). Negative lengths and truncation are what corrupt or hostile input looks like. The alignment rules are UnityPyBoost's, so any tree that reaches one of them reads differently in rabex. Other games and engine lines are in scope (the HEAD tier and the multi-game goal), and these cases are the reason for rewriting `serde_typetree`.

**What a raw-bytes `Value` is still needed for.** Checked case by case, nothing measured:
- **Non-UTF-8 strings** reach the dict model through serde. rabex's `deserialize_string` hands invalid UTF-8 to the visitor as bytes (`visit_byte_buf`, `de.rs:237-240`), `value.rs` keeps them (`Val::BadStr`), and `invalid_utf8_string` matches at L2. Its resave failure is the `ser.rs` rule above, not the representation.
- **Lossless L3** is reached on the corpus: the dump re-saves every object from the dict model through rabex's serializer (`value.rs`, `Ser`), and L3 is 100% with the shim (section 3). The synthetic L3 differences are `ser.rs` rules. rabex keeps a signalling NaN's bits, so it is the lossless side of `f32_signalling_nan`.
- **ReferencedObject** needs the SerializedFile's ref types inside the reader, which `serde_typetree::Deserializer` does not have. That is a change to the reader's inputs, not to the value it produces.

A raw-bytes `Value` therefore stays a design choice for the rewrite, not a parity requirement. Speed is not a reason either: values take 1.78 s of rabex's 27.9 s serial run (section 5).

## 5. Throughput, memory, Send+Sync, the TPK cache

The workload is all 1,101 inputs, parsed and with every object's value read. MB/s counts input bytes (compressed, as on disk) per wall second. Ranges span repeated runs. rabex runs with `--shim-fs6`, so both sides read all 399,936 objects. Each side's serial and parallel phases ran as separate processes, so each peak is that phase's alone.

**Memory** is measured the same way on both sides: `scripts/mem_peak.py` samples the process and every process it starts every 50 ms and sums their PSS (`/proc/<pid>/smaps_rollup`). PSS charges a page shared by n processes 1/n to each, so the copy-on-write pages a fork pool inherits from its parent count once, and rabex's threads and UnityPy's forked workers are measured alike.

| | serial | 8 threads (rabex) / 8 forked workers (UnityPy) |
|---|---|---|
| **rabex 0.2.1** | 27.8-27.9 s, 14,328-14,374 obj/s, 16.8-16.9 MB/s | 3.92-4.04 s, 99,006-101,950 obj/s, 116-120 MB/s |
| **UnityPyBoost** | 58.8 s, 6,798 obj/s, 8.0 MB/s | 8.22-8.29 s, 48,217-48,677 obj/s, 56.7-57.2 MB/s |
| **ratio (rabex / UnityPy, throughput)** | 2.1x | 2.0-2.1x |
| **peak memory, rabex (summed PSS)** | 110 MB | 337 MB (one process) |
| **peak memory, UnityPy (summed PSS)** | 349 MB | 1.27-1.40 GB (9 processes) |
| **memory ratio (UnityPy / rabex)** | 3.2x | 3.8-4.1x |

For contrast, the old UnityPy figure, each worker's `ru_maxrss` summed, is 2.29-2.38 GB for the same parallel runs: it counts the parent's pages once per worker.

**Where the time goes:**
- **rabex (serial CPU):** containers 25.3 s, SerializedFile parsing 0.52 s, values 1.78 s.
- **UnityPy (serial CPU):** load 54.0 s, values 4.8 s.
- **LZMA, head to head** (`rabex-dump lzma`, single-threaded, from memory): 1,097 LZMA blocks from 1,100 bundles (the 1,101st input is the raw sample), 1,345.9 MB out, 0 outputs that differ. rabex's call into lzma-rs 0.3.0 (`lzma_decompress_with_options` with `UnpackedSize::UseProvided(Some(size))`) takes 24.9 s (54.1 MB/s). liblzma 0.4.9 `static`, which links xz 5.8.4, takes 10.3 s (130.3 MB/s). lzma-rs is 2.41x slower.
- **What that means for rabex.** lzma-rs's 24.9 s is 89% of the 27.9 s serial run. With liblzma the run would take about 27.9 − 24.9 + 10.3 ≈ 13.3 s. That is an estimate, from two separate runs, and it assumes the rest of the run is unchanged. That is why the fork moves to liblzma.

The full canonical dump (L0-L3, with encoding, resave check and gzip) takes 26.8 s at `-j 8`, peaking at 876-911 MB summed PSS (one process). The oracle, `tools/parity/unitypy_dump.py`, forks a fresh process for every input, so that no input sees what UnityPy cached for another; it takes 111-115 s at `-j 8`, peaking at 2.26-2.46 GB summed PSS (9 processes). Each was run twice, and each pair of outputs is byte-identical.

**Send+Sync** (an autoref-specialisation probe in `probe.rs`):

| type | Send | Sync |
|---|---|---|
| `BundleFile`, `BundleFileReader<Cursor<&[u8]>>`, `SerializedFile`, `TypeTreeNode`, `TpkTypeTreeBlob`, `ObjectRef`, both `Error` types | yes | yes |
| `typetree_cache::sync::TypeTreeCache<TpkTypeTreeBlob>` | yes | yes |
| `typetree_cache::TypeTreeCache` (unsync, elsa `FrozenMap`) | yes | **no** |
| the spike's `tpk::Tpk` (`VersionedCache`, elsa `sync::FrozenMap`) | yes | yes |

The bench shares one `tpk::Tpk` across every rayon thread.

**The TPK cache key (a fork fix).** Both of rabex's `TypeTreeCache` variants look up `get(&class_id)` and use the Unity version only on a miss (`typetree/typetree_cache.rs:36-53` and `:95-112`). The first Unity version to ask for a class fixes that class's tree for every later version in the process. So a stripped file (one with no embedded tree) read after a file of another version decodes against the wrong layout, silently or as a spurious `check_read`. UnityPy keys by class and version (`helpers/Tpk.py`). The P4 Editor fixtures (5.x to 6000.5, typetrees on and off, in one test process) and any `unityrs-py` session that opens files of two games would hit it.
- **Reproduced.** `rabex-dump probe`'s `cache` section gives each of classes 1, 21, 28, 48, 114, 115 and 128 a fresh cache, asks it for 5.6.7f1 and then 6000.3.17f1, and compares with the TPK's own tree for each version. The TPK's two trees differ for all 7 classes (node counts at the two versions: 15/15, 76/107, 30/46, 1,137/2,306, 12/12, 36/35, 66/67). rabex's `sync::TypeTreeCache` returns the 5.6.7f1 tree for 6000.3.17f1 in all 7; `tpk::VersionedCache` returns each version's own tree in all 7.
- **Not reached by the corpus.** Every object record's `l2.source` in the oracle dump is `"embedded"` (399,936 records, 0 `"tpk"`), so neither section 3 nor the bench ever read a tree from the TPK.
- **The fix**, as the fork will make it, is `tpk::VersionedCache`, keyed by `(ClassId, version)`. The spike reads through it. The unit tests `tpk::tests::rabex_cache_is_keyed_by_class_only` and `versioned_cache_keys_by_class_and_version` pin both behaviours.

## 6. Probes: Environment, cross-file PPtr, `.resS`, container paths

These ran on build A's 551 bundles, selected from `index.json` (`--build 4100`) and taken as sources in path order, and `scripts/verify_probe.py` checked each result against UnityPy's `Environment` loaded with the same bundles.

| probe | result | UnityPy agrees |
|---|---|---|
| CAB registry, built from bundle directories only (no block decompression) | 551 bundles, 1,020 entries, 0 failures, 0.5 ms | n/a |
| Cross-file PPtrs into other bundles of the build (targets: Texture2D 23, Shader 10, MonoBehaviour 7, Material 1) | 41 resolved, 0 failed, 9 skipped as built-ins (`Library/unity default resources`) | 41/41 (`deref()` type and name) |
| Streamed Texture2D: the `m_StreamData` path, offset and size, read from the `.resS` entry | 20/20 | 20/20 (sha256 of `get_resource_data`) |
| AssetBundle container paths | the container is read from the AssetBundle object's typetree | L1 container rows 100% (section 3) |

**What rabex provides:**
- `PPtr::file_identifier` and `resolve`, which map a FileID to the external path;
- `BundleFileReader`, which reads a directory without decompressing and an entry with only the blocks it needs. That is what makes the registry cheap.

**What we layer on top:**
- the CAB-name to bundle registry;
- PPtr dereferencing across files and bundles;
- skipping built-in resources;
- the `.resS` lookup with UnityPy's fallback chain (basename, `.resource`, `.assets.resS`, `.resS`);
- the container helper.

All of it layered cleanly on the public API, with no change to rabex. It is the Environment part of `unityrs-core` in `DESIGN.md`.

## 7. Format matrix (code reading, rabex 0.2.1 against UnityPy 1.25.2, and UnityPy HEAD `417d595` for the HEAD tier)

Rows marked *corpus* were exercised by section 3. The rest are read from the source.

| area | rabex 0.2.1 | UnityPy | gap |
|---|---|---|---|
| UnityFS format 6 | the header byte bug (section 3) | correct | **fix** (*corpus*) |
| UnityFS format 6, 2019.4.15 ≤ version < 2020 | TODO comment; never aligns | aligns the blocks info when the engine is 2019.4.15 or later within 2019 (`version[0] == 2019`) | **fix** |
| UnityFS 7-8 | reads; `align(16)` at ≥7; bug only when the header ends 16-aligned | correct | **fix** (*corpus*: format 8) |
| UnityFS 9 (HEAD tier) | read with format-8 rules: each blocks-info row is `u32, u32, u16`, with no 64-bit offset | HEAD reads a `u64` offset after each row (`CompressionHelper.BlockInfo`, `version >= 9`); 1.25.2 does not | **fix** (HEAD) |
| ArchiveFlags vs ArchiveFlagsOld | the same version rule as UnityPy; an unknown version is an Error | the same rule, with a default | minor |
| Blocks-info at the end (0x80) | read | read | none |
| Hash, CRC, data flags | parsed, not exposed | exposed | layer (done in the spike) |
| UnityArchive | `panic!` (`bundlefile/mod.rs:459`) | unsupported (raises) | **panic** |
| UnityWeb / UnityRaw | `BundleFile` only; `BundleFileReader` calls `unimplemented!()` | read | **panic** |
| WebFile, detection | absent (upstream removed WebFile) | read | layer (detection done in the spike) |
| LZHAM | reads: an error (`Unsupported`, `bundlefile/mod.rs:761-764`); writes: `todo!()` (`:424`) | unsupported (raises) | writer only |
| SerializedFile below version 9 | seeks `Current(file_size - metadata_size)`, not the absolute position | absolute | **fix** |
| SerializedFile header gates | reader: `unity_version` at ≥9 (UnityPy ≥7), `target_platform` at ≥10 (≥8), `enable_type_tree` at ≥11 defaulting to false (≥13, defaulting to true); the writer uses UnityPy's gates, so it panics on versions 9-13 (section 8) | | **fix** |
| `bigIDEnabled` (versions 7-13) | read and discarded, so path IDs misread when it is set | honoured | **fix** |
| SerializedFile below version 16 | always takes trees from the TPK | first type with a matching class ID | **fix** |
| SerializedFile 23-25 (HEAD tier) | no version gate above 22 (`LARGE_FILES_SUPPORT`), so read with version-22 rules: a type's tree is parsed as a version-12+ blob | HEAD reads each type's tree through `TypeTreeNodeInfoHandler` (a hash, a size, then an `mhtt` blob with ref hashes); 1.25.2 does not | **fix** (HEAD) |
| SerializedFile 26 (HEAD tier) | as 23-25, and the shared-subtree table after the ref types is not read, so the user-information string is read from the wrong place | HEAD reads the shared subtrees (a count, then node-info handlers by hash) | **fix** (HEAD) |
| Metadata strings | `read_cstr` fails on invalid UTF-8 | surrogate-escaped | **fix** |
| Unity version string | rejects `c`/`x` types and builds above 255; the raw string is dropped; `0.0.0` becomes `None`, with no fallback to the bundle's version | kept raw, with a fallback | **fix** |
| Big-endian | generic `ByteOrder` throughout | read | none (`big_endian_mix` matches) |
| Old-format typetree blobs | parsed recursively, with no depth bound | iterative | **bound** |

## 8. Save and write

Bundles were re-saved with `rabex-dump save --shim-fs6 --tpk <rabex's lz4.tpk>`: `write_serialized` for each SerializedFile entry, with object bytes unchanged, then `write_bundle`, uncompressed. The shim was on: as published, rabex reads none of the 4 sample bundles (each fails with "the offset to copy is not contained in the decompressed buffer"). `scripts/save_compare.py` compared the result with the originals and with UnityPy. The set was 72 bundles: an even sample of 68 corpus-game bundles (`--sample`), and the 4 sample bundles. The fifth sample is not a bundle; rabex's bundle reader rejects it, and it is left out.

| check | rabex | UnityPy |
|---|---|---|
| Bundle bytes equal to UnityPy `save(packer="none")` | 0/72. The header flags differ on all 68 corpus-game bundles: the originals carry `0x243`, UnityPy writes `0x40`, and rabex writes `0x240`, keeping the `0x200` flag (padding at the start of blocks-info). The 4 sample bundles get `0x40` from both writers and still differ | n/a |
| On reload by UnityPy: entries, header fields (the reserved bytes and the version-22 unknown field included), types and ref types with their type dependencies, ref-type names and trees (each node's `m_Index` and `m_RefTypeHash` included), script types, externals, the object table and every object's raw bytes equal the original's ("semantically equal") | 72/72 | n/a |
| The same, plus the layout: metadata size, data offset, file size and each object's offset in the data | 45/72 | n/a |
| SerializedFile re-save byte-equal to the original | 45/72 | 6/72 (`SerializedFile.save()`) |

**Why rabex's SerializedFile re-saves differ.** In all 27 that differ, the original's object table is not in data order: Unity stored the objects' data in a different order from the table. In all 45 that are equal, it is in order. rabex writes object data in table order, so it moves the objects (14,659 of the 16,118 objects in the 68 corpus-game bundles, all in those 27 files) and rewrites their offsets in the object table. The first difference is therefore in the metadata, before the data offset, in all 27; in one of them the metadata size or the data offset changes too. On reload nothing else differs: the types, externals, object table and object bytes are equal (the semantic row). UnityPy's `SerializedFile.save()` rewrites more (6/72 byte-equal), so rabex is the closer of the two to the original.

No SerializedFile in the save set has a type dependency or a ref type (`save_compare.py`'s `original_sf_with_type_dependencies` and `original_sf_with_ref_types` are 0), so those members are equal here by absence. rabex's writer could not take a ref type anyway (the `todo!()` below).

**The writer panics below version 14.** rabex's SerializedFile reader and writer gate the metadata differently. The reader takes the Unity version from version 9, the target platform from 10 and `enable_type_tree` from 11 (section 7), and reads `bigIDEnabled` at 7-13 only to discard it (`serializedfile/mod.rs:773-778`). The writer uses UnityPy's gates, 7, 8 and 13, and unwraps the target platform at 8 and later (`:1187`) and `bigIDEnabled` at 7-13 (`:1206`). So `write_serialized` panics on every file of versions 9-13 that rabex has just read, before any `todo!()` below. `rabex-dump probe`'s `sf_writer` section re-saves an object-less, type-less file of each version from 9 to 22, laid out to rabex's own reader gates: versions 9-13 panic at an `unwrap()` ("called `Option::unwrap()` on a `None` value"), and 14-22 re-save byte-equal. The corpus and the save set are all version 17 or 22.

**Limits found by reading the code:**
- `write_serialized` calls `todo!()` for non-empty `ref_types`, headers below version 9, `is_destroyed` below version 11, and typetree blobs at version 11 or below 10;
- `write_bundle` writes UnityFS only, and blocks-info at the end and encryption are `todo!()`;
- LZMA writes go through lzma-rs, and LZ4HC through C lz4 at level 12, where `DESIGN.md` pins HC 9;
- chunks are 128 KiB, and objects align to 8, or 16 from 6000;
- `serde_typetree/ser.rs` has 21 live `todo!()` (a 22nd is commented out) and 27 `unreachable!()`, the `todo!()`s including vectors, arrays, strings and ReferencedObject under some node kinds;
- the version-22 header writer writes three zero bytes where it read the reserved bytes (`serializedfile/mod.rs:153-161`). Every original in the save set has zeros there, so it changes nothing measured here;
- the type-tree writer writes each node's pre-order position as its `m_Index`, not the index it read (`typetree/mod.rs:112,191,276`). In the save set every stored index is its pre-order position, so this too changes nothing measured here.

Matching UnityPy's `save(packer="none")` byte for byte needs its header flag and data-order choices. That is the "save layout" change `DESIGN.md` names as a reason to fork.

## 9. TPK, licence and provenance

| | rabex 0.2.1 (`resources/lz4.tpk`) | UnityPy 1.25.2 (`lzma.tpk`) |
|---|---|---|
| classes | 412 | 409 |
| versions | 1,393, from 3.4.0f5 to 6000.6.0a2 | 1,334, from 3.4.0 to 6000.5a5 |

`scripts/tpk_compare.py` built 15 classes at 6 versions on both sides: classes 1, 4, 21, 28, 43, 48, 49, 74, 83, 114, 115, 128, 142, 213 and 687078895, at 5.6.7f1, 2018.4.11f1, 2020.3.48f1, 2022.3.0f1, 6000.0.50f1 and 6000.3.17f1.
- 89 of the 90 pairs give the same node list (level, type, name, byte size, version, meta flag). Only `type_flags` differs: UnityPy leaves it `None` and rabex fills it in.
- The 90th pair is SpriteAtlas (687078895) at 5.6.7f1, which is absent on both sides (UnityPy raises; rabex returns `None`).

`TpkTypeTreeBlob::get_typetree_node` **panics** on a class ID the blob does not hold (a map index).

**The spike compiles no TPK in.** rabex embeds its blob by default: `embed-tpk` is a *default* feature, and the crate ships `resources/lz4.tpk`, built with AssetRipper/Tpk from AssetRipper's TypeTreeDumps, which has no licence file. `DESIGN.md` keeps the TPK out of the repository and the binary until its provenance is settled (`unityrs-tpk`). So the spike depends on rabex with `default-features = false` (only `compression-lzma` and `compression-lz4`), and loads a TPK at runtime with `--tpk PATH`. The runs here pointed it at rabex's own `lz4.tpk` in the cargo registry, outside the repository, so the measured blob is the one rabex embeds. The unit test `tpk::tests::rabex_is_built_without_the_embedded_tpk` fails if `embed-tpk` comes back anywhere in the workspace.

**The fork plan, licence and provenance:**
- **Licence election.** rabex is MIT OR Apache-2.0 (its `Cargo.toml`). The fork takes the vendored code under MIT. Under Apache-2.0, section 4(b) would require a change notice in every modified file.
- **Attribution.** rabex's `LICENSE-MIT.md` has no copyright line, and upstream deleted its COPYRIGHT file on 2026-06-08 ("remove stale COPYRIGHT file"), so there is no rabex notice to carry over. `THIRD_PARTY_LICENSES` names the authors from rabex's `Cargo.toml`, Rudolf Kolbe and Jakob Hellermann, with the MIT text.
- **Outside code inside rabex.** Two vendored files credit code from elsewhere, and its notices do carry over:
  - `archive_storage_manager.rs`, the UnityCN block decryptor, which the fork keeps as is, opens with "credits to: https://github.com/Razmoth/CNStudio/blob/master/AssetStudio/CNUnity.cs". That link is dead; the code is a port of the same repository's `AssetStudio/UnityCN.cs` (the `#$unity3dchina!@` signature check, the key decryption, and the index and substitution tables). CNStudio is MIT, "Copyright (c) 2016 Radu, Copyright (c) 2016-2020 Perfare", and archived.
  - `tpk.rs`'s node walk in `get_typetree_node` is "adapted from" UnityPy's `helpers/Tpk.py` (MIT, K0lb3).

  `DESIGN.md`'s `THIRD_PARTY_LICENSES` already carries AssetStudio's and UnityPy's notices. The fork's entry for rabex points at them, and adds CNStudio's two copyright lines, so the provenance is recorded where the code is.
- **The TPK blob.** The fork deletes `resources/lz4.tpk` and the `embed-tpk` feature. Vendored as shipped, the blob would sit in this public repository, and, as a default feature, `embed-tpk` compiles it into every crate that depends on the fork with default features.
- **`commonstring.rs`.** It starts `//! @generated … from the embedded TPK`, so it is data derived from the same blob. The fork regenerates it from UnityPy's MIT `UnityPy.enums.CommonString` (copyright K0lb3). `scripts/commonstring_compare.py`: UnityPy 1.25.2's table has 108 entries, all equal to rabex's at the same offsets; rabex's has 113, the 5 more being newer engine strings (`Hash128`, `RenderingLayerMask`, `fixed_array`, `EntityId` and `LoadableReference`). The fork adds those from the TPK once its provenance is settled, or records their provenance.
- **`objects/class_id.rs`.** It is generated straight from AssetRipper's TypeTreeDumps: upstream's `utils/class_generator/generate_class_id.py` merges every `Classes/*.json` of a TypeTreeDumps checkout into one ID-to-name table, and the file carries that generator's 25 "Renamed from" / "Reassigned from" notes. The crate also ships the dumps' README under `utils/class_generator/TypeTreeDumps/`. The fork regenerates it from UnityPy's MIT `UnityPy.enums.ClassIDType`. `scripts/classid_compare.py`: rabex's table has 413 IDs and UnityPy's 368; 356 IDs carry the same name on both sides, 4 are named differently (the generator keeps the newest engine's name, for example rabex's `PhysicsMaterial` for UnityPy's `PhysicMaterial`), 53 are only in rabex and 8 only in UnityPy. The fork takes the 53 from a licensed source, or records their provenance.

## 10. Panics and fuzzing

**Reproduced through the public API** (`rabex-dump probe`; 5 panics). On read, the `panics` section, 4 of 4:
- `BundleFile::from_reader` on a UnityArchive signature (`panic!`);
- `BundleFileReader::from_reader` on a UnityWeb signature (`unimplemented!()`);
- `TpkTypeTreeBlob::get_typetree_node` on an unknown class;
- `serde_typetree` reading a malformed `map` node whose Array lacks its pair (an index panic).

On write, the `sf_writer` section: `write_serialized` on any SerializedFile of versions 9-13 that rabex has just read, at an `unwrap()` (section 8).

**Found by reading the code, not reproduced:**
- the write-path `todo!()`s in section 8;
- `BundleFileReader`'s `discard_front` `panic!` (`reader.rs:346`);
- unbounded loops over a vector of empty structs with a huge count, which do no I/O per element;
- recursive parsing of old-format typetrees, which can overflow the stack.

**Fuzzing was not run.** The container has no nightly toolchain: `/opt/cargo` holds only stable and 1.99.1, and has no `cargo-fuzz`. Installing nightly into the shared toolchain was out of scope for this spike. Upstream's repository has `fuzz/` targets for `bundlefile` and `serializedfile`, which the crate does not ship. The "no fuzz panics in one hour" condition is therefore unmet, and a fuzzer would soon reach the four read panics above, so it would fail anyway.

## 11. Code kept versus rewritten

`scripts/rabex_loc.py` counts rabex 0.2.1's `src/` code lines, without blanks or comments: 6,852 in all, 120 of them in test-only items (`#[cfg(test)]`, or `#[cfg(all(test, …))]` as in `tpk.rs`), so 6,732 of library code. Each module's fate, and why, is a table in the script.

| fate | lines | modules |
|---|---|---|
| **kept as is** | 1,064 (15.8%) | `files/serializedfile/builder` 370, `objects/pptr` 229, `archive_storage_manager` 120, `read_ext` 103, `files/bundlefile/builder` 93, `files/bundlefile/config` 36, `typetree/provider` 36, `files/unityfile` 27, `write_ext` 25, `lib` 13, `objects/mod` 7, `files` 5 |
| **kept with targeted fixes** | 3,144 (46.7%) | `files/serializedfile/mod` 1,115 (header gates, bigID, version strings, writer), `files/bundlefile/mod` 694 (header byte, alignment, panics, writer flags), `tpk` 465 (unknown class), `typetree/mod` 374 (depth bound), `files/bundlefile/reader` 287 (UnityWeb `unimplemented!()`, `discard_front` `panic!`), `unity_version` 110, `typetree/typetree_cache` 99 (the cache key) |
| **regenerated** | 996 (14.8%) | `objects/class_id` 874 and `commonstring` 122, both generated from AssetRipper's TypeTreeDumps (section 9) |
| **rewritten** | 1,528 (22.7%) | `serde_typetree/ser` 797, `serde_typetree/de` 665, `serde_typetree/error` 60, `serde_typetree/mod` 6 |

The port rule counts rewrites: `serde_typetree` is 22.7% of the library code. With the two regenerated tables it is 37.5%. Either way it is below the "more than half" threshold for porting. The targeted fixes touch modules holding 46.7% of the lines, but each is a bounded change in place (the reasons are in the script's table), not a rewrite.

## 12. Upstream

Checked with `gh api` on 2026-10-28 (section 14).
- **Repository:** `jakobhellermann/RustyAssetBundleEXtractor`, a GitHub fork of `K0lb3/RustyAssetBundleEXtractor`. It has 34 commits from 2026-06-08 to 2026-09-26: 33 by Jakob Hellermann, and Rudolf Kolbe's "discontinuation notice" of 2026-06-18, shared with the predecessor's history. `main` is at "release v0.2.1" (2026-09-26). Recent commits include serde reader fixes of the kind the fork needs, such as "serde: exhaust remaining ignored fields in structs" (2026-09-26).
- **Issues:** the tracker is disabled (`has_issues: false`), so no one outside can file an issue there.
- **Pull requests:** one ever, #1 by an outside contributor (2026-10-23), the UnityFS header fix. It is open, with no review or comment as of 2026-10-28.
- **The predecessor**, `K0lb3/RustyAssetBundleEXtractor`, is archived (`archived: true`), so nothing can merge there now. It did merge an outside pull request: #1 by perceptron8, "Fix reader alignment", on 2025-05-28. The current maintainer's own pull requests there (#2-#7 and #10-#15) were closed unmerged on 2025-10-27, and an outside LZMA fix (#8, "Fix lzma decompress option") has been open since 2025-06-12.

So upstream is active but small. Whether it accepts outside fixes is untested: the one outside fix is unanswered after five days, and the predecessor's evidence concerns a different maintainer.

## 13. The decision against `DESIGN.md`'s rules

**Use upstream, pinned**, condition by condition:

| condition | measured | met |
|---|---|---|
| L1-L3 pass 100% on the corpora | 99.984% / 99.987% / 99.987%; UnityPy samples 1/66, 0/53, 0/53 (section 3) | **no** (yes only with a patch) |
| The Environment, cross-file PPtr, `.resS` and container probes pass or layer on top cleanly | 41/41, 20/20, 100% container rows, all through the public API (section 6) | yes |
| A raw-bytes `Value` or an extension point | serde is the extension point: `value.rs` builds the dict model through it, 100% L2/L3 on the corpus, and no measured case needs a raw-bytes `Value`. The read rules behind it are wrong in 13 of 32 synthetic cases, which the L1-L3 row and the fork's change 2 cover (section 4) | yes, as an extension point |
| Throughput at least UnityPyBoost's | 2.1x serial, 2.0-2.1x at 8 threads, at a quarter to a third of UnityPy's peak memory (section 5) | yes |
| No fuzz panics in one hour, or upstream accepts the fixes | no fuzz run; 5 panics reproduced; the one outside fix is unreviewed (sections 10, 12) | **no** |

So this route fails today on two conditions: L1-L3, and fuzz-or-accept (no fuzz run, 5 panics reproduced, nothing accepted upstream).

**Vendor a fork**, under the amended rule ("invasive changes … that upstream has not accepted"): L1 is sound, at 100% on the corpus game and 100% everywhere with a 1-hunk fix. The changes are invasive: `de.rs` and `ser.rs` read and write rules, bounding, panics, and the save layout (sections 1, 4, 8, 10). Upstream has accepted none of them yet. **This route applies.** Under the rule as first written ("won't take") it did not, because refusal is unproven; that is why the rule was amended rather than the route assumed.

**Port UnityPy's core:** the failures are not structural. The container and SerializedFile layers pass every corpus record once the header is fixed, and the rewrite is 22.7% of rabex (37.5% with the regenerated tables), short of half. **This route does not apply.**

## 14. Reproducing

All of this runs in the dev container (`scripts/dev`), from the worktree. Dumps and re-saved bundles go outside the repository; `dump` and `save` refuse a path inside it.

```sh
scripts/dev sh -c 'cd /work/.wt/p0-rabex-spike && cargo build --release -p unityrs-spike-rabex'
# then, inside the container, from the worktree:
D=/tmp/unityrs-dumps/spike; B=target/release/rabex-dump; S=crates/unityrs-spike-rabex/scripts
T=$(ls $CARGO_HOME/registry/src/*/rabex-0.2.1/resources/lz4.tpk)    # rabex's TPK, outside the repo
M="python $S/mem_peak.py --out"                                       # summed-PSS sampler

# sections 2-3: dumps, compare, pass rates; oracle size. Section 5 times the two mem_peak runs (each run twice).
$M $D/mem-oracle.json -- python tools/parity/unitypy_dump.py --index /corpus/index.json -j 8 -q -o $D/oracle-full.jsonl.gz
$B dump /corpus/index.json --index -j 8 -o $D/rabex-full.jsonl.gz --stats $D/rabex-full.stats.json
$M $D/mem-dump-shim.json -- $B dump /corpus/index.json --index -j 8 --shim-fs6 -o $D/rabex-full-shim.jsonl.gz --stats $D/rabex-full-shim.stats.json
python tools/parity/compare.py $D/oracle-full.jsonl.gz $D/rabex-full.jsonl.gz
python tools/parity/compare.py $D/oracle-full.jsonl.gz $D/rabex-full-shim.jsonl.gz
python $S/pass_rates.py $D/oracle-full.jsonl.gz $D/rabex-full.jsonl.gz
python $S/pass_rates.py $D/oracle-full.jsonl.gz $D/rabex-full-shim.jsonl.gz
stat -c %s $D/oracle-full.jsonl.gz; zcat $D/oracle-full.jsonl.gz | wc -c
python -c "import json, collections; print(collections.Counter((f['kind'], f['build']) for f in json.load(open('/corpus/index.json'))['files']))"
zcat $D/oracle-full.jsonl.gz | grep -o '"format":[0-9]*,"version_player":"[^"]*","version_engine":"[^"]*"' | sort | uniq -c
zcat $D/oracle-full.jsonl.gz | grep -o '"version":[0-9]*,"data_offset"' | sort | uniq -c    # SerializedFile versions
zcat $D/oracle-full.jsonl.gz | grep -o '"source":"[a-z]*"' | sort | uniq -c    # section 5, TPK cache

# section 4
python $S/value_cases.py $B --json $D/value-cases.json

# section 5: throughput and memory, one phase per process
$M $D/mem-rabex-serial.json   -- $B bench /corpus/index.json --index -j 8 --repeat 2 --phase serial   --shim-fs6
$M $D/mem-rabex-parallel.json -- $B bench /corpus/index.json --index -j 8 --repeat 2 --phase parallel --shim-fs6
$M $D/mem-unitypy-serial.json    -- python $S/bench_unitypy.py /corpus/index.json -j 8 --phase serial
$M $D/mem-unitypy-parallel.json  -- python $S/bench_unitypy.py /corpus/index.json -j 8 --phase parallel   # run twice
$B lzma /corpus/index.json --index --shim-fs6

# sections 5, 6, 8, 9, 10: probes (section 8's writer versions are the `sf_writer` section)
$B probe /corpus/index.json --index --build 4100 --tpk $T > $D/probe.json
python $S/verify_probe.py $D/probe.json /corpus
python $S/tpk_compare.py $D/probe.json
python $S/commonstring_compare.py $(ls -d $CARGO_HOME/registry/src/*/rabex-0.2.1/src)   # section 9
python $S/classid_compare.py $(ls -d $CARGO_HOME/registry/src/*/rabex-0.2.1/src)        # section 9

# section 8
$B save /corpus/index.json --index --kind bundle --sample 68 --shim-fs6 --tpk $T -o $D/save-a
$B save /corpus/index.json --index --kind unitypy-sample --shim-fs6 --tpk $T -o $D/save-b
$B save /corpus/index.json --index --kind unitypy-sample --tpk $T -o $D/save-b-plain   # as published
python $S/save_compare.py $D/save-a /corpus      # and save-b, save-b-plain

# section 11; the ser.rs todo!()/unreachable!() counts in the recommendation and section 8
R=$(ls -d $CARGO_HOME/registry/src/*/rabex-0.2.1/src)
python $S/rabex_loc.py $R
grep -v '^\s*//' $R/serde_typetree/ser.rs | grep -c 'todo!'           # live uses only
grep -v '^\s*//' $R/serde_typetree/ser.rs | grep -c 'unreachable!'
grep -c 'Renamed from\|Reassigned from' $R/objects/class_id.rs                   # section 9
grep -rn 'credits to\|adapted from' $R                                            # section 9, outside code

# section 10: the toolchains on hand
rustup toolchain list; ls $CARGO_HOME/bin

# section 12 (from the host)
gh api repos/jakobhellermann/RustyAssetBundleEXtractor --jq '{has_issues, fork, archived}'
gh api 'repos/jakobhellermann/RustyAssetBundleEXtractor/pulls?state=all'
gh api 'repos/jakobhellermann/RustyAssetBundleEXtractor/commits?since=2026-06-01T00:00:00Z&per_page=100'
gh api repos/jakobhellermann/RustyAssetBundleEXtractor/contents/utils/class_generator/generate_class_id.py --jq .content | base64 -d | head   # section 9
gh api repos/K0lb3/RustyAssetBundleEXtractor --jq .archived
gh api repos/Razmoth/CNStudio --jq '{license: .license.spdx_id, archived}'                      # section 9
gh api repos/Razmoth/CNStudio/contents/LICENSE --jq .content | base64 -d | head -4
gh api 'repos/K0lb3/RustyAssetBundleEXtractor/pulls?state=all&per_page=100'

# the spike's own tests
cargo test -p unityrs-spike-rabex
python -m unittest discover -s crates/unityrs-spike-rabex/scripts/tests
```

Watch `cat /sys/fs/cgroup/memory.current` during the parallel runs: the container is capped at 12 GB.
