# Canonical dump format

**Format:** `unityrs-canonical-dump`, version 1. **Status:** P0, 2026-10-28.

The canonical dump is how unityrs is compared with UnityPy at verification levels L0-L3 (`DESIGN.md` sections 5 and 6) before any Python binding exists. Two producers write it:
- `tools/parity/unitypy_dump.py`, the oracle, runs UnityPy 1.25.2 with UnityPyBoost in the oracle image;
- the unityrs CLI writes the same records from Rust.

`tools/parity/compare.py` diffs the two. `tools/parity/canonical.py` is the Python reference for every encoding rule below.

The format makes two promises:
- **Lossless against UnityPy.** Every distinction UnityPy's `parse_as_dict()` makes on the UnityPyBoost path survives: list versus tuple, bytes versus str, a str holding invalid UTF-8, int versus bool versus float, float32 versus float64 down to the bit, dict key order, and ReferencedObject data.
- **Canonical.** A value has exactly one encoding and a record exactly one serialisation. Two producers that agree write byte-identical lines, so equal lines need no parsing, and a dump's sha256 identifies its content.

## 1. Stream

- JSON Lines: UTF-8, one JSON object per line, each line ended by a single `\n`, with no blank lines and no byte-order mark.
- A path ending in `.gz` holds the same stream gzip-compressed. Writers set the gzip mtime to 0, so equal dumps compress to equal bytes. The canonical content is always the uncompressed stream.
- The first line is the `dump` header record and the last is the `end` record. A stream without its `end` record is truncated.

## 2. JSON serialisation

Each line is exactly what Python's `json.dumps(record, ensure_ascii=False, separators=(",", ":"))` writes. That is also what serde_json's compact writer emits when members are serialised in the listed order (use struct field order, or `serde_json` with `preserve_order`).
- **Whitespace:** none outside strings.
- **Member order:** exactly as each record's table below lists it. Every listed member is present, with `null` where it does not apply. The one exception is `note`, which is optional and always comes last.
- **Strings:**
  - Escape `"` as `\"` and `\` as `\\`.
  - Escape U+0008, U+0009, U+000A, U+000C and U+000D as `\b`, `\t`, `\n`, `\f` and `\r`.
  - Escape every other code point below U+0020 as `\u00xx`, with lowercase hex.
  - Write everything else as literal UTF-8: `/`, U+007F, U+2028/U+2029 and non-BMP characters included.
  - Every emitted string is valid Unicode. Strings that are not valid UTF-8 are carried as hex (section 3).
- **Integers:** plain decimal, with `-` only when negative, no leading zeros, no `+`, no fraction and no exponent. Values span i64 and u64. The dump never contains a JSON float: floats are always hex-tagged, so the two producers never meet a float-formatting difference.
- **Hashes and fixed byte fields** (sha256, GUIDs, type hashes, reserved bytes) are lowercase hex strings.

## 3. Values (L2)

A typetree value is the Python object `ObjectReader.parse_as_dict()` returns, encoded as follows. Bare JSON carries only the types JSON cannot confuse. Every JSON object inside a value is a tagged value whose first member is `t`. A dict is tagged too, so a typetree field named `t` can never be mistaken for a tag.

| Python value | encoding |
|---|---|
| `None` | `null` |
| `bool` | `true` / `false` |
| `int` | JSON integer |
| `str` that is valid UTF-8 | JSON string |
| `str` holding invalid UTF-8 | `{"t":"str","hex":H}` or `{"t":"str","len":N,"sha256":S}` (the raw-bytes rule) |
| `bytes` (TypelessData) | `{"t":"bytes","hex":H}` or `{"t":"bytes","len":N,"sha256":S}` |
| `float` from a `float` node | `{"t":"f32","v":"3f800000"}`, the float32 bit pattern as 8 hex digits |
| `float` from a `double` node | `{"t":"f64","v":"3ff0000000000000"}`, 16 hex digits |
| `list` | JSON array of encoded elements, except the packed forms below |
| non-empty `list` from `vector<float>` | `{"t":"f32s","v":H}`: the elements' 8-digit bit patterns, concatenated |
| non-empty `list` from `vector<double>` | `{"t":"f64s","v":H}`: 16 digits per element |
| non-empty `list` from `vector<UInt8>` or `vector<char>` | `{"t":"u8s","hex":H}` or `{"t":"u8s","len":N,"sha256":S}` |
| `tuple` | `{"t":"tuple","v":[...]}` |
| `dict` | `{"t":"dict","v":[[K,V],...]}`, in the dict's own order |

The rules behind the table:

- **Raw-bytes rule.** `bytes`, invalid-UTF-8 strings and `u8s` share one rule. At 64 bytes or fewer they are written inline as `hex` (lowercase, two digits per byte). Above 64 bytes they are written as `len` plus the `sha256` of the bytes. Inline hex keeps small differences readable. The summary keeps texture, font and vertex blobs from dwarfing the dump, and still tells any two different payloads apart.
- **Strings.**
  - Typetree strings are bytes that UnityPy decodes as UTF-8 with `surrogateescape`. The producer re-encodes them with `encode("utf-8", "surrogateescape")`, which recovers the file's exact bytes, and emits a JSON string only when those bytes are valid UTF-8.
  - Validity is strict UTF-8, which Python and Rust's `std::str::from_utf8` agree on: overlong forms and encoded surrogates (`ED A0 80`) are invalid.
  - An empty string is `""`.
- **Dict keys** (child names) are never summarised. A valid name is a JSON string; anything else is `{"t":"str","hex":H}` at any length.
- **Float width comes from the typetree, not the value.** Python holds both widths as a float64, so the producer reads the width from the node that typed the value.
  - `f32` is the float32 the file held, exactly: UnityPyBoost widens it losslessly, and narrowing back is exact.
  - NaN payloads, the sign of NaN and `-0.0` all survive, apart from one oracle exception (section 7).
  - `float`/`double` are the only float node types. A float anywhere else is an encoding error.
- **Packing is mandatory and by node type.**
  - A list is packed when it is non-empty and its vector's element node (`children[0].children[1]`) has type `float`, `double`, `UInt8` or `char`.
  - An empty list is always `[]`. Tuples are never packed.
  - `SInt8`, `bool` and every wider integer stay JSON arrays.
  - A packed `u8s` decodes to the same list of ints, so it never collides with `bytes`.
- **Pairs** are UnityPy's tuples (a `pair` node). A `map` is therefore a list of `tuple`s.
- **ReferencedObject** values are dicts like any other. Their `data` entry is typed by the node UnityPy resolves through `SerializedFile.ref_types` by `(class, ns, asm)`. When `type.class` is empty, UnityPy leaves `data` out of the dict, and so does the dump. **ManagedReferencesRegistry** follows UnityPy too: only the first registry in a subtree is read, and later ones are absent.
- **Repeated child names.** The dict keeps the first position and the last value (`PyDict_SetItem` overwrites), and the last child of that name types the value.

A worked example. This is the value UnityPyBoost reads from a synthetic tree. The f32 signalling NaN in the file, `7f800001`, appears quieted (section 7). The `d_snan` f64 signalling NaN survives. `s_bad` holds the bytes `61 ff 62`. `m` is a `map<string,SInt64>`. `dup` names an `int` and then a `float`.

```json
{"t":"dict","v":[["f_snan",{"t":"f32","v":"7fc00001"}],["f_qnan_payload",{"t":"f32","v":"ffc12345"}],["f_negzero",{"t":"f32","v":"80000000"}],["d_snan",{"t":"f64","v":"7ff0000000000001"}],["b2",true],["s_bad",{"t":"str","hex":"61ff62"}],["blob",{"t":"bytes","hex":"0001"}],["fv",{"t":"f32s","v":"3fc000007f800000"}],["u8v",{"t":"u8s","hex":"007fff"}],["m",[{"t":"tuple","v":["k",-5]}]],["dup",{"t":"f32","v":"40000000"}]]}
```

## 4. Records

Each record's first member is `rec`.

### 4.1 `dump`: the header

| member | value |
|---|---|
| `format` | `"unityrs-canonical-dump"` |
| `version` | `1` |
| `levels` | the levels the dump holds, in order: a subset of `["L0","L1","L2","L3"]` |
| `producer` | free-form object (tool name, versions). Never compared. |

### 4.2 `file`: one per file and per container entry

| member | value |
|---|---|
| `file` | the file's identity, a list of names: `[input]` for an input, `[input, entry]` for a bundle or WebFile entry, and so on down. |
| `l0` | detection (below) |
| `l1` | container metadata by kind (below), or `null` when the file parsed as raw, zip or error, or when the dump has no L1 |
| `error` | `null`, or `{"kind":K}` with `K` either `"detect"` (`check_file_type` raised) or `"parse"` (the parser raised). An optional `note` gives the message. |

**Names.**
- `input` is the input's path relative to the dump root, with `/` separators:
  - the file name for a single-file input;
  - the path below the folder for a folder input;
  - the entry's `path` for a corpus index.
- An entry name is the bundle directory's `path` string.
- Names are written like dict keys (always inline, hex when not UTF-8).

**`l0`** (member order `type`, `parsed_as`, `size`, `sha256`):

- `type` is UnityPy's `ImportHelper.check_file_type` verdict on the bytes, as its `FileType` name (`AssetsFile`, `BundleFile`, `WebFile`, `ResourceFile`, `ZIP`), or `"error"` when it raises. For an entry, the bytes are the entry's decompressed data. The rules, in order:
  1. Fewer than 20 bytes is `ResourceFile`.
  2. The signature is read up to a NUL within the first 20 bytes and decoded as UTF-8 with surrogateescape. With no NUL, the read raises if the file is exactly 20 bytes; otherwise the signature is the 20 bytes.
  3. A signature equal to `UnityWeb`, `UnityRaw`, `UnityFS` or eight U+00FA characters is `BundleFile`.
  4. A signature starting with `UnityWebData` or `TuanjieWebData` is `WebFile`.
  5. A signature equal to `PK\x03\x04` is `ZIP`.
  6. Fewer than 128 bytes is `ResourceFile`.
  7. A gzip stream (`1f 8b`) whose decompressed signature starts with `UnityWebData`/`TuanjieWebData` is `WebFile`. So is `brotli` at offset 0x20.
  8. Otherwise the SerializedFile header is read **big-endian**: `metadata_size`, `file_size`, `version` and `data_offset` as u32. At version ≥ 22, 4 bytes are skipped, then come u32 `metadata_size`, i64 `file_size`, i64 `data_offset` and an i64. The file is `AssetsFile` unless `version` < 0 or > 100, any of the four is < 0 or larger than the file, or `file_size` < `metadata_size` or < `data_offset`. Then it is `ResourceFile`.
- `parsed_as` is what UnityPy built: `SerializedFile`, `BundleFile`, `WebFile`, `raw` (it kept a reader), `zip`, or `error`. **The name rule:** an `AssetsFile` whose name ends in `.resS`, `.resource`, `.config`, `.xml` or `.dat` stays `raw`.
- `size` and `sha256` describe the bytes `type` was decided on.

**`l1` for `kind: "bundle"`**:

| member | value |
|---|---|
| `kind` | `"bundle"` |
| `signature`, `format`, `version_player`, `version_engine` | the header, as read |
| `engine_version_packed` | `BundleFile.parse_version()` as UnityPy packs it: `major<<48 \| minor<<32 \| build<<16 \| type<<8 \| number`, a plain OR, so a number above 255 spills into the type byte. `type` is the lower-cased type string's value (a 0, b 1, c 2, f 3, p 4, x 5, anything else 255). `null` where it raises. |
| `flags` | the UnityFS data flags as an integer, or `null` for UnityWeb/UnityRaw below format 6 |
| `flags_class` | `"ArchiveFlags"` or `"ArchiveFlagsOld"`, UnityPy's choice by engine version, or `null`. It is old below 2020, and within the 2020, 2021 and 2022 lines below 2020.3.34, 2021.3.2 and 2022.1.1. |
| `block_alignment` | whether the 16-byte alignment after the header applied: format ≥ 7, or a 2019 engine at 2019.4.15 or later |
| `encrypted` | whether the UnityCN encryption flag made UnityPy build a decryptor |
| `hash`, `crc` | UnityWeb/UnityRaw format ≥ 4 header fields, else `null` |
| `blocks_info` | `[uncompressed, compressed, flags]` of the blocks-info section, or `null` |
| `blocks` | `[[uncompressed, compressed, flags], ...]`, or `null` for UnityWeb/UnityRaw below format 6 |
| `directory` | `[[offset, size, flags, path], ...]` in file order (`flags` is `null` for UnityWeb/UnityRaw) |

UnityFS's total-size field and its uncompressed-data hash are left out, because UnityPy reads and discards them.

**`l1` for `kind: "web"`:** `kind`, `signature`, `packer` (`"none"`, `"gzip"` or `"brotli"`), and `directory` with `flags` `null`.

**`l1` for `kind: "serialized"`**:

| member | value |
|---|---|
| `kind` | `"serialized"` |
| `header` | `{metadata_size, file_size, version, data_offset, endian, reserved, unknown}`. `endian` is `"<"` or `">"`. `reserved` is the 3 hex bytes from version 9, else `null`. `unknown` is the version-22 trailing i64, else 0. The version-22 header's own fields replace the legacy u32s. |
| `unity_version` | the version string as read (`"2.5.0f5"` below format 7) |
| `version_packed` | `SerializedFile.version` packed as above, after UnityPy's fallback: an empty or `0.0.0` string takes the parent bundle's engine version. `null` below format 7, where UnityPy never sets it. |
| `target_platform` | the raw integer (format ≥ 8), else `null` |
| `enable_type_tree` | bool |
| `big_id_enabled` | integer (formats 7-13), else 0 |
| `types` | type records, in file order |
| `object_order` | path_ids in UnityPy's `objects` dict order (section 7) |
| `script_types` | `[[file_index, identifier], ...]` |
| `externals` | `[[temp_empty, guid, type, path], ...]`. `guid` is hex. Members a format lacks are `null`. |
| `ref_types` | type records (format ≥ 20), else `null` |
| `user_information` | string (format ≥ 5), else `null` |
| `container` | the AssetBundle's `m_Container`: `[[path, preload_index, preload_size, file_id, path_id], ...]` in order. `[]` without an AssetBundle. |

**Type record:** `class_id`, `is_stripped_type`, `script_type_index`, `script_id` (hex or `null`), `old_type_hash` (hex or `null`), `nodes`, `type_dependencies` (list or `null`), `class_name`, `namespace`, `assembly` (names, or `null` outside ref types).

**Typetree nodes:**
- `nodes` is the tree in pre-order, one row per node: `[level, type, name, byte_size, version, type_flags, variable_count, index, meta_flag, ref_type_hash]`.
- `null` marks a field the format lacks.
- `nodes` itself is `null` when the type carries no tree.

### 4.3 `object`: one per object

| member | value |
|---|---|
| `file` | the owning SerializedFile's identity |
| `path_id` | i64 |
| `l1` | `{type_id, class_id, byte_start, byte_size, is_destroyed, is_stripped}`. `byte_start` is absolute within the SerializedFile. The last two are `null` where the format lacks them. `null` when the dump has no L1. |
| `l2` | `{root_type, source, error, value}`, or `null` when the dump has no L2 |
| `l3` | `{raw_size, raw_sha256, resave, resave_size, resave_sha256, first_diff}`, or `null` when the dump has no L3 |

**`l2`:**
- `root_type`: the typetree root's `m_Type` (the class name, for example `Texture2D` or `MonoBehaviour`).
- `source`: `"embedded"` for the file's own tree, `"tpk"` for UnityPy's bundled class database. Both are `null` when no tree exists.
- `error`: `null` on success, else `{"kind":K,"bytes_read":N}` (optionally followed by `note`). `bytes_read` is an integer only for `check_read`. The kinds:

| kind | meaning | `value` |
|---|---|---|
| `no_typetree` | no node: none embedded, and none in the TPK | `null` |
| `check_read` | the tree read fewer bytes than `byte_size` | the dict from `parse_as_dict(check_read=False)` |
| `out_of_bounds` | the tree needs more bytes than the object has | `null` |
| `negative_length` | a negative array or string length | `null` |
| `ref_type` | a ReferencedObject type could not be resolved | `null` |
| `encode` | the value does not fit its tree (a producer bug, never expected) | `null` |
| `other` | anything else | `null` |

- `value`: the encoded value (section 3).

**`l3`:**
- `raw_size` and `raw_sha256` describe `get_raw_data()`. `raw_size` can be below `byte_size` only for a truncated file.
- `resave` is `"equal"` when `TypeTreeHelper.write_typetree(value, node)` (what `save_typetree` writes, without its side effect of marking the object changed) returns the raw bytes exactly.
- It is `"differs"` otherwise, and then `resave_size`, `resave_sha256` and `first_diff` (the first differing offset, or the shorter length) are filled in. They are `null` for every other outcome.
- It is `"error"` when the writer raises, with an optional `note` after `first_diff`.
- It is `"skipped"` when `l2` has an error. The write is not attempted, because UnityPy's writer only ever sees full parses.

### 4.4 `end`

`{"rec":"end","files":F,"objects":O}`: the number of `file` and `object` records written.

## 5. Order

The dump is deterministic: the same input and producer always give the same bytes.
- Inputs come in ascending order of the bytewise UTF-8 encoding of their relative path, which is what `os.fsencode` gives in Python and `OsStr` byte order gives in Rust.
- Each input is written depth-first:
  1. its `file` record;
  2. for a SerializedFile, its `object` records in ascending signed `path_id`;
  3. for a BundleFile or WebFile, each entry in the order of UnityPy's `files` dict (directory order, with a repeated path kept at its first position, holding the last entry), recursively.

## 6. Comparison

`compare.py` takes a reference dump (UnityPy) and a candidate (unityrs).
- **Matching.** It matches records by key: `file` by `file`, `object` by (`file`, `path_id`). It compares the levels both headers list, or the subset `--levels` names.
- **What it ignores.** `note` members and `producer` are never compared.
- **How it compares.**
  - Values are compared after decoding. Packed lists unpack, and an inline payload and its summary compare by length and sha256.
  - Types must match exactly: `true` is not `1`, a tuple is not a list, an `f32` is not an `f64`.
  - A line whose values match but whose bytes differ is reported as a **format** finding: not the canonical serialisation of itself, a different member order, or another encoding of the same value. This holds a producer to section 2.
  - A missing `end` record, or one whose counts disagree with the records in its own dump, is also a **format** finding.
- **Report.** Differences are counted by level and class (`root_type`), then grouped by field path, with list indices folded to `[]`, and a few concrete examples per group.
- **Levels.** L0: detection, and missing or extra files. L1: container and object-table metadata, load errors, and missing or extra objects. L2: values and parse outcome. L3: resave.

## 7. UnityPy behaviours the oracle records

These are UnityPy 1.25.2 facts that a unityrs producer must reproduce to match, or record as a deliberate divergence in `PARITY.md`.

**L0 detection.**
- The eight-`0xFA` bundle signature is compared as eight U+00FA characters. The surrogate-escaped bytes `FA` never match, so such a file reads as `ResourceFile`, while the UTF-8 bytes `C3 BA` repeated eight times do match.
- A zip is detected only when its fifth byte is NUL, because the signature runs to the first NUL and must equal `PK\x03\x04`. An ordinary zip, with version-needed 20 (byte `0x14`), reads as `ResourceFile`.
- A 20-byte file with no NUL makes detection raise (`l0.type` `"error"`).
- The SerializedFile check reads its header big-endian, whatever the file's endian byte says.

**Floats.**
- UnityPyBoost widens a float32 to a Python float in C (`PyFloat_FromDouble`). On x86-64 that conversion quiets a signalling NaN: the file's `7f800001` reaches the dict as `7fc00001`.
- The oracle dump therefore shows the quieted bits. A producer that reports the file's bits differs here, and `compare.py` labels such pairs `f32_snan_quieted` so they stand apart from real errors.
- `save_typetree` writes the quieted bits back, so such objects resave as `differs`.
- float64 values pass through unconverted and keep every bit.

**Booleans.** Any non-zero byte reads as `true`. `save_typetree` writes `1`, so a stored `2` resaves as `differs`.

**Parsing path.**
- Every object with `byte_size` > 0 is read by UnityPyBoost. Its arrays are lists, and its strings are always followed by a 4-byte alignment.
- With `byte_size` 0, UnityPy uses its pure-Python reader instead. That reader returns a tuple for an array whose primitive element node carries the align flag (encoded as `tuple`, never packed), and reads an out-of-range string length as `""` instead of failing.

**ReferencedObject.** UnityPyBoost skips the `data` of an empty class name in arrays as well. The pure-Python path does not; that bug is not reachable on the Boost path.

**Object table.**
- `objects` is a dict keyed by `path_id`, so a repeated `path_id` keeps its first position with the last entry. `object_order` and the `object` records show that collapsed view.
- Below format 16, an object's type is the first type whose `class_id` equals the object's `type_id`.
- For formats 11-16, loading writes each object's script index back into that shared type. A type's `script_type_index` is therefore the last such object's value.

**Load-time parses.**
- A SerializedFile parses its AssetBundle object while loading, so a broken AssetBundle fails the whole file (`error.kind` `"parse"`).
- An unparseable engine version with no `UnityPy.config.FALLBACK_UNITY_VERSION` raises.

## 8. Not covered by format 1

- **Zip/apk members and `.splitN` merging.** Inputs are dumped one by one. A detected zip gets a `file` record with `parsed_as` `"zip"` and no children.
- **Object mode** (`parse_as_object`, `clean_name` attribute names) and the classes built from it.
- **`peek_name()`.** UnityPy caches its peek node globally by the root's `(m_Name, m_Type, m_Version)`, so the answer can depend on which file a process read first, and an order-independent dump cannot pin it. The name is in the L2 value.
- **Environment-wide state:** `obj.container` after the preload pass, and cross-file PPtr resolution. These belong to a later Environment level.
- **L2b TPK node lists** for stripped files, L4 file save, and every level from L5 up.

## 9. Changing the format

- A change that alters any line for the same input bumps `version`.
- `compare.py` refuses to mix versions.
- Re-pinning UnityPy does not change the format, but it can change the oracle's records. The header's `producer` says which UnityPy wrote a dump.
