# Canonical dump format

**Format:** `unityrs-canonical-dump`, version 1. **Status:** P0, 2026-10-28.

The canonical dump is how unityrs is compared with UnityPy at verification levels L0-L3 (`DESIGN.md` sections 5 and 6) before any Python binding exists. Two producers write it:
- `tools/parity/unitypy_dump.py`, the oracle, runs UnityPy 1.25.2 with UnityPyBoost in the oracle image, and refuses to run on any other stack;
- the unityrs CLI writes the same records from Rust.

`tools/parity/compare.py` diffs the two. `tools/parity/canonical.py` is the Python reference for every encoding rule below.

The format makes two promises:
- **Lossless against UnityPy.** Every distinction UnityPy's `parse_as_dict()` makes on the UnityPyBoost path survives: list versus tuple, bytes versus str, a str holding invalid UTF-8, int versus bool versus float, float32 versus float64 down to the bit, dict key order, and ReferencedObject data.
- **Canonical.** A value has exactly one encoding and a record exactly one serialisation. Two producers that agree write byte-identical lines, apart from the header's `producer` and the free-text `note` members (section 2), so lines that are equal need no field-by-field comparison.

## 1. Stream

- JSON Lines: UTF-8, one JSON object per line, each line ended by a single `\n`, with no blank lines and no byte-order mark.
- A path ending in `.gz` holds the same stream gzip-compressed.
- **Equality is defined on the uncompressed stream, header line excluded.** Two dumps hold the same content when their uncompressed lines after the header are byte-identical once every `note` member (the optional last member of an `error` or `l3` object) is removed; the sha256 of those lines, notes removed, identifies the content. Notes are free text that two producers may word differently for the same outcome, so they are not content, and `compare.py` never compares them. The header differs between producers (`producer`), and the compressed bytes depend on the gzip writer, so neither is canonical. For example, the oracle's `GzipFile` compresses at level 9 and writes the header `1f8b0800 00000000 02ff`, whose `02` (XFL) names that level; a writer at another level or with another deflate implementation writes other bytes for the same lines. The oracle sets the gzip mtime to 0 only so that its own runs are reproducible.
- The first line is the `dump` header record and the last is the `end` record. A stream without its `end` record is truncated.

## 2. JSON serialisation

Each line is exactly what Python's `json.dumps(record, ensure_ascii=False, separators=(",", ":"))` writes. That is also what serde_json's compact writer emits when members are serialised in the listed order (use struct field order, or `serde_json` with `preserve_order`).
- **Whitespace:** none outside strings.
- **Member order:** exactly as each record's table below lists it. Every listed member is present, with `null` where it does not apply. The one exception is `note`, which is optional and always comes last.
- **Notes** are free text for people and are never compared. Like every other string they are valid Unicode: the oracle writes the bytes a message quotes that are not valid UTF-8 as Python backslash escapes (`\udcff`).
- **Strings:**
  - Escape `"` as `\"` and `\` as `\\`.
  - Escape U+0008, U+0009, U+000A, U+000C and U+000D as `\b`, `\t`, `\n`, `\f` and `\r`.
  - Escape every other code point below U+0020 as `\u00xx`, with lowercase hex.
  - Write everything else as literal UTF-8: `/`, U+007F, U+2028/U+2029 and non-BMP characters included.
  - Every emitted string is valid Unicode. Strings that are not valid UTF-8 are carried as hex (section 3).
- **Integers:** plain decimal, with `-` only when negative, no leading zeros, no `+`, no fraction and no exponent. Values span i64 and u64, except the two packed engine versions, which can be wider (section 4.2) and are written exactly. The dump never contains a JSON float: floats are always hex-tagged, so the two producers never meet a float-formatting difference.
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
- **Dict keys** (child names) follow the metadata-string rule (section 4.2): never summarised. A valid name is a JSON string; anything else is `{"t":"str","hex":H}` at any length.
- **Float width comes from the typetree, not the value.** Python holds both widths as a float64, so the producer reads the width from the node that typed the value.
  - `f32` is the float32 the file held, exactly: UnityPyBoost widens it losslessly, and narrowing back is exact.
  - NaN payloads, the sign of NaN and `-0.0` all survive, apart from one oracle exception (section 7).
  - `float`/`double` are the only float node types. A float anywhere else is an encoding error.
- **Packing is mandatory and by node type.**
  - A list is packed when it is non-empty and its vector's element node (`children[0].children[1]`) has type `float`, `double`, `UInt8` or `char`.
  - An empty list is always `[]`. Tuples are never packed.
  - `SInt8`, `bool` and every wider integer stay JSON arrays.
  - A `u8s` is raw bytes of its own kind (section 6). It equals neither a JSON array of the same ints nor `bytes` holding the same bytes, so a UInt8 or char vector written as ints is an L2 `type` difference.
- **Pairs** are UnityPy's tuples (a `pair` node). A `map` is therefore a list of `tuple`s.
- **ReferencedObject** values are dicts like any other. Their `data` entry is typed by the node UnityPy resolves through `SerializedFile.ref_types` by `(class, ns, asm)`. When `type.class` is empty, UnityPy leaves `data` out of the dict, and so does the dump.
- **ManagedReferencesRegistry** follows UnityPy's scope, which is the class that holds a registry as a direct child. Reading that registry starts the scope. Until that class ends, every other registry is skipped and absent from the dict: the class's later registry children, any registry inside its later children's subtrees, and any registry inside the registry being read. The scope ends with that class, so a registry in a later sibling of the class is read. In `{a:{r1}, b:{r2}, r3, c:{r5}, r4}`, `r1`, `r2` and `r3` are read, while `r5` and `r4` are absent.
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
| `levels` | the levels the dump holds, in order: `"L0"`, then any of `"L1"`, `"L2"`, `"L3"`. Every file record carries `l0`, so every dump holds L0. |
| `producer` | free-form object (tool name, versions). Never compared. |

### 4.2 `file`: one per file and per container entry

| member | value |
|---|---|
| `file` | the file's identity, a list of names: `[input]` for an input, `[input, entry]` for a bundle or WebFile entry, and so on down. |
| `l0` | detection (below) |
| `l1` | container metadata by kind (below), or `null` when the file parsed as raw, zip or error, or when the dump has no L1 |
| `error` | `null`, or `{"kind":K}` with `K` one of `"detect"` (`check_file_type` raised), `"parse"` (the parser raised) and `"crash"` (the process dumping the input died of a signal). An optional `note` gives the message. Only an input's own record carries an error (see "A failing entry" below). |

**Names.**
- `input` is the input's path relative to the dump root, with `/` separators:
  - the file name for a single-file input;
  - the path below the folder for a folder input;
  - the entry's `path` for a corpus index.
- An entry name is the bundle directory's `path` string.
- An input's name comes from its path's bytes, as given, decoded as UTF-8 whatever the process locale, so bytes that are not UTF-8 reach the metadata-string rule below unchanged. The path is not resolved: a symlink is named after itself, in either input mode, and the name rule (under `l0` below) tests that name.

**The metadata-string rule.** Every string the dump takes from a file or a file name, other than a typetree value, is written as a name. That covers input and entry names, `signature`, `version_player`, `version_engine`, `unity_version`, `user_information`, directory, external and container paths, `temp_empty`, typetree node types and names, the class, namespace and assembly of a type record, and `root_type`. A name is a JSON string when its bytes are valid UTF-8, and otherwise `{"t":"str","hex":H}` at any length. It is never summarised: the raw-bytes rule of section 3 applies only inside values.

**A failing entry fails its input.** UnityPy reads a bundle's or WebFile's entries with no error handling. An entry whose detection or parse raises (a SerializedFile that does not parse, or a broken AssetBundle object inside one) therefore fails the whole top-level input. That input gets one `file` record, with `l0.parsed_as` `"error"`, `l1` `null` and `{"kind":"parse"}`, and no records for any of its entries. So an entry record never carries an `error`, and `l0.type` is `"error"` only on an input's own record.

**A crash fails its input.** The oracle dumps each input in a process of its own. When that process dies of a signal (UnityPyBoost aborts or segfaults, section 7, or the memory cap kills it), before or while it hands back its records, the input gets one `file` record, with `l0` describing the input's bytes as detected, `l0.parsed_as` `"error"`, `l1` `null` and `{"kind":"crash"}`, and no other records. The `note` names the signal. A kill by the memory cap (`SIGKILL`) depends on the machine and the number of jobs, not on the input. A raise in the oracle's own code, or a process that ends without its records and without a signal, is no outcome of the input: the oracle stops and writes no dump.

**`l0`** (member order `type`, `parsed_as`, `size`, `sha256`):

- `type` is UnityPy's `ImportHelper.check_file_type` verdict on the bytes, as its `FileType` name (`AssetsFile`, `BundleFile`, `WebFile`, `ResourceFile`, `ZIP`), or `"error"` when it raises. For an entry, the bytes are the entry's decompressed data. The rules, in order:
  1. Fewer than 20 bytes is `ResourceFile`.
  2. The signature is read up to a NUL within the first 20 bytes and decoded as UTF-8 with surrogateescape. With no NUL, the read raises if the file is exactly 20 bytes; otherwise the signature is the 20 bytes.
  3. A signature equal to `UnityWeb`, `UnityRaw`, `UnityFS` or eight U+00FA characters is `BundleFile`.
  4. A signature starting with `UnityWebData` or `TuanjieWebData` is `WebFile`.
  5. A signature equal to `PK\x03\x04` is `ZIP`.
  6. Fewer than 128 bytes is `ResourceFile`.
  7. A file that starts with `1f 8b` is read as gzip, and the whole stream must decompress: every member, with its CRC and size checked, where zero bytes may pad the end. If it does not (a bad header, corrupt or truncated data, a bad CRC, a non-zero byte after the last member), detection raises (`"error"`). The file is `WebFile` when the decompressed bytes start with `UnityWebData` or `TuanjieWebData` and go on for at least one more byte: UnityPy's streaming reader stops at the end of the stream without keeping the last byte it read. A stream that holds exactly `UnityWebData` is therefore not a WebFile. Otherwise detection goes on.
  7b. A file whose 6 bytes at offset 0x20 are `brotli` is `WebFile`.
  8. Otherwise the SerializedFile header is read **big-endian**: `metadata_size`, `file_size`, `version` and `data_offset` as u32. At version ≥ 22, 4 bytes are skipped, then come u32 `metadata_size`, i64 `file_size`, i64 `data_offset` and an i64. The file is `AssetsFile` unless `version` < 0 or > 100, any of the four is < 0 or larger than the file, or `file_size` < `metadata_size` or < `data_offset`. Then it is `ResourceFile`.
- `parsed_as` is what UnityPy built: `SerializedFile`, `BundleFile`, `WebFile`, `raw` (it kept a reader), `zip`, or `error`. `zip` is for an input only: UnityPy's parser has no zip branch, so an entry detected as `ZIP` is `raw`. **The name rule:** an `AssetsFile` whose name ends in `.resS`, `.resource`, `.config`, `.xml` or `.dat` stays `raw`. The match is a case-sensitive suffix test (`str.endswith`), so `.RESS` does not match.
- `size` and `sha256` describe the bytes `type` was decided on.

**`l1` for `kind: "bundle"`**:

| member | value |
|---|---|
| `kind` | `"bundle"` |
| `signature`, `format`, `version_player`, `version_engine` | the header, as read |
| `engine_version_packed` | `BundleFile.parse_version()` as UnityPy packs it (the version-packing rule below), or `null` where it raises. Only the UnityFS layout (below) calls it while loading. |
| `flags` | the UnityFS data flags as an integer, or `null` for UnityWeb/UnityRaw outside format 6 |
| `flags_class` | `"ArchiveFlags"` or `"ArchiveFlagsOld"`, UnityPy's choice by engine version, or `null` outside the UnityFS layout. It is old below 2020, and within the 2020, 2021 and 2022 lines below 2020.3.34, 2021.3.2 and 2022.1.1. UnityPy compares the tuple `(major, minor, build, type, number)` of the packed value, each field cut to its slot (16 bits for the first three, 8 for the last two). So `133091.1.1f1` compares as 2019 and is old. A type byte other than 0-5 and 255, the bytes that name types, makes the comparison raise and the input fail (`parse`), as when a `number` above 255 spills into it: `2020.3.34f1024` has type byte 7. |
| `block_alignment` | whether the 16-byte alignment after the header applied. Only the UnityFS layout applies it: at format ≥ 7, or with a 2019 engine at 2019.4.15 or later (compared as `flags_class` compares). It is `false` on the web/raw layout, format 7 and up included. |
| `encrypted` | whether the UnityCN encryption flag made UnityPy build a decryptor |
| `hash`, `crc` | the header fields of UnityWeb/UnityRaw at format 4 or above, except format 6; else `null` |
| `blocks_info` | `[uncompressed, compressed, flags]` of the blocks-info section, or `null` |
| `blocks` | `[[uncompressed, compressed, flags], ...]`, or `null` for UnityWeb/UnityRaw outside format 6. In both, `compressed` is the number of bytes UnityPy read for the section or block, which is below the header's size when the file is truncated. |
| `directory` | `[[offset, size, flags, path], ...]` in file order (`flags` is `null` for UnityWeb/UnityRaw outside format 6) |

UnityFS's total-size field and its uncompressed-data hash are left out, because UnityPy reads and discards them.

**UnityWeb and UnityRaw format 6** are read with the UnityFS layout (UnityPy's `read_fs`, with one extra byte after the data flags). So they have `flags`, `flags_class`, `blocks_info`, `blocks` and directory flags, and their `hash` and `crc` are `null`. Every other UnityWeb/UnityRaw format takes the web/raw layout, which has a 16-byte hash and a u32 crc from format 4.

**The version-packing rule** (`engine_version_packed`, and `version_packed` below) is UnityPy's `UnityVersion.from_str`:
- The string must match `^(\d+)\.(\d+)\.(\d+)(.)?(\d+)?(.*)$`, using Python `re` with DOTALL. So `\d` is any Unicode decimal digit (category Nd), and the groups are `major`, `minor`, `build`, the type, `number` and an ignored rest.
- The type is exactly one character, the first after `build`'s digits. `number` is the digits right after it, or 0 when there are none.
- The type's value is that character lower-cased: `a` 0, `b` 1, `c` 2, `f` 3, `p` 4, `x` 5, anything else 255. A string with nothing after `build` packs as `f` (3).
- The packed value is `major<<48 | minor<<32 | build<<16 | type<<8 | number`. It is a plain OR of unbounded integers, so a field wider than its slot spills into the next one: a `number` above 255 lands in the type byte.
- Examples: `2021.3.2ff1` packs type `f` with number 0. `2019.4.15 f1` packs type 255 with number 0. `5.6.7` packs type `f` with number 0.
- A major of 65536 or more makes the packed value wider than 64 bits: `133091.1.1f1` packs to 37461786129692951297. The dump writes the exact integer.
- `parse_version` raises, and `engine_version_packed` is `null`, when the string does not match or its major, cut to 16 bits, is 0 (`0.5.1f1`, `65536.1.1f1`). In those cases UnityPy wants `config.FALLBACK_UNITY_VERSION`, which the oracle leaves unset. A bundle on the UnityFS layout calls `parse_version` while loading, so there the input fails (`parse`) instead.

**`l1` for `kind: "web"`:** `kind`, `signature`, `packer` (`"none"`, `"gzip"` or `"brotli"`), and `directory` with `flags` `null`. The oracle never writes `"gzip"`: UnityPy detects a gzip-compressed WebFile and then fails to parse it, so the oracle's record of one is a `parse` error (`PARITY.md` section 4, gzip WebFile detection, Tier B). unityrs loads it and writes `"gzip"`.

**`l1` for `kind: "serialized"`**:

| member | value |
|---|---|
| `kind` | `"serialized"` |
| `header` | `{metadata_size, file_size, version, data_offset, endian, reserved, unknown}`. `endian` is `"<"` or `">"`. `reserved` is the 3 hex bytes from version 9, else `null`. `unknown` is the version-22 trailing i64, else 0. The version-22 header's own fields replace the legacy u32s. |
| `unity_version` | the version string as read (`"2.5.0f5"` below format 7) |
| `version_packed` | `SerializedFile.version` packed by the version-packing rule, with no 16-bit cut, so a major of 0 packs like any other. An empty or `0.0.0` string takes the parent bundle's `version_engine` (a WebFile is no such parent). Without a usable string UnityPy wants the fallback, so the input fails (`parse`). A string that does not match the rule fails the input whatever the fallback holds, because UnityPy packs it directly. `null` below format 7, where UnityPy never sets it. |
| `target_platform` | the raw integer (format ≥ 8), else `null` |
| `enable_type_tree` | bool, read from format 13. Below it the value is `true`, and every type carries a tree. |
| `big_id_enabled` | integer (formats 7-13), else 0 |
| `types` | type records, in file order |
| `object_order` | path_ids in UnityPy's `objects` dict order (section 7) |
| `script_types` | `[[file_index, identifier], ...]` |
| `externals` | `[[temp_empty, guid, type, path], ...]`. `guid` is hex. Members a format lacks are `null`. |
| `ref_types` | type records (format ≥ 20), else `null` |
| `user_information` | string (format ≥ 5), else `null` |
| `container` | the `m_Container` of the first AssetBundle object in `objects` dict order (section 7), as `[[path, preload_index, preload_size, file_id, path_id], ...]` in order. `[]` without an AssetBundle. |

**Type record:** `class_id`, `is_stripped_type`, `script_type_index`, `script_id` (hex or `null`), `old_type_hash` (hex or `null`), `nodes`, `type_dependencies` (list or `null`), `class_name`, `namespace`, `assembly` (names, or `null` outside ref types). Each member is what the format reads, or UnityPy's default:
- `is_stripped_type` is a bool from format 16, else `null`.
- `script_type_index` is read from format 17. Below it the type holds -1, unless loading wrote an object's index back (section 7).
- `script_id` and `old_type_hash` are read from format 13.
- `nodes`, `type_dependencies` (format 21, plain types) and the three names (format 21, ref types) are read only when `enable_type_tree` is true, and are `null` otherwise.

**Typetree nodes:**
- `nodes` is the tree in pre-order, one row per node: `[level, type, name, byte_size, version, type_flags, variable_count, index, meta_flag, ref_type_hash]`.
- A blob tree (format 10, and 12 on) unpacks each row as UnityPy's `hBBIIiii` struct, plus `Q` from format 19. So `version` is i16, `level` and `type_flags` u8, `byte_size`, `index` and `meta_flag` i32, and `ref_type_hash` u64. The blob has no `variable_count`, so it is `null`. Older trees are read field by field, as UnityPy's `TypeTreeNode.parse` reads them.
- A blob string offset with the high bit set indexes UnityPy's whole common-string table, whatever the engine version. An offset the table lacks becomes the offset's decimal string (`0x7FFFFFF0` gives `"2147483632"`).
- Blob rows become a tree as UnityPy's `parse_blob` builds it, under a hidden root at level -1. A row whose level is above the previous row's becomes a child of the previous row, however far above. A row at the same level becomes its sibling. A row below it becomes a child of the nearest ancestor of the previous row whose level is below its own. Only the hidden root's first child is kept: a later top-level row and its subtree are dropped, from `nodes` and from the value. So rows at levels `0, 2, 1` give a root with two children, and `nodes` lists the kept tree in pre-order with each row's level as stored.
- `null` marks a field the format lacks.
- `nodes` itself is `null` when the type carries no tree.

### 4.3 `object`: one per object

| member | value |
|---|---|
| `file` | the owning SerializedFile's identity |
| `path_id` | i64 |
| `l1` | `{type_id, class_id, byte_start, byte_size, is_destroyed, is_stripped}`. `byte_start` is absolute within the SerializedFile. The last two are `null` where the format lacks them. Both are integers, never bools: `is_destroyed` a u16 (below format 11), `is_stripped` a signed byte (formats 15 and 16). `null` when the dump has no L1. |
| `l2` | `{root_type, source, error, value}`, or `null` when the dump has no L2 |
| `l3` | `{raw_size, raw_sha256, resave, resave_size, resave_sha256, first_diff}`, or `null` when the dump has no L3 |

**`l2`:**
- `root_type`: the typetree root's `m_Type` (the class name, for example `Texture2D` or `MonoBehaviour`).
- `source`: `"embedded"` for the file's own tree, `"tpk"` for UnityPy's bundled class database. Both are `null` when no tree exists.
- `error`: `null` on success, else `{"kind":K,"bytes_read":N}` (optionally followed by `note`). `bytes_read` is an integer only for `check_read`. A read that raises gives every kind but `check_read`; `check_read` is decided only after the whole tree was read. The kinds:

| kind | meaning | `value` |
|---|---|---|
| `no_typetree` | no node: none embedded, and none in the TPK | `null` |
| `check_read` | the tree's read ended `bytes_read` bytes after `byte_start`, and `bytes_read != byte_size`, in either direction (see the reading paths below) | the dict from `parse_as_dict(check_read=False)` |
| `out_of_bounds` | on the UnityPyBoost path, a read other than an alignment needs bytes past the object's end | `null` |
| `negative_length` | a negative array length, or on the UnityPyBoost path a negative string or byte-array length | `null` |
| `ref_type` | a ReferencedObject's `(class, ns, asm)` is not in `ref_types`, or the file has no `ref_types` | `null` |
| `encode` | the value does not fit its tree (a producer bug, never expected) | `null` |
| `other` | anything else: a malformed tree (a `pair` without two children), and on the pure-Python path a read past the SerializedFile's end or an aligned `SInt8` array | `null` |

**The two reading paths** decide `bytes_read` and which failures raise (UnityPy's `TypeTreeHelper.read_typetree`):
- **`byte_size` > 0: UnityPyBoost** reads the object's `byte_size` bytes and nothing else. `bytes_read` includes the 4-byte alignment after the last field. UnityPyBoost moves the position for an alignment without a bounds check, so a trailing alignment can carry `bytes_read` past `byte_size` without failing. Example: `[int i, bool b (aligned)]` over 5 bytes reads `{i, b}` and gives `check_read` with `bytes_read` 8.
- **`byte_size` 0: the pure-Python reader** starts at `byte_start` in the whole SerializedFile and reads on into the bytes after the object. Any tree that reads at all is therefore a `check_read`, and its value comes from the bytes that follow. A tree that reads nothing is no error. This reader reads a negative or out-of-range string length as `""`, and its other differences are listed in section 7.

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
- **Matching.** It matches records by key: `file` by `file`, `object` by (`file`, `path_id`). A second record with a key already read is a `duplicate` finding, and the first one is the one compared.
- **Order.** The order is content too (section 1), so it is checked twice. Each dump is held to section 5, and a record out of place is an `order` format finding: an input whose name's bytes sort before the previous input's, an entry that does not follow its container's records, an object that does not follow its file's record or whose `path_id` is below the previous one, and an entry that comes before an entry its container's `directory` lists earlier (at its first position). When neither dump has an `order` finding, the records both hold must come in the same order, or the first pair that does not is an `order_differs` finding. That covers the entry order of a dump without L1, which has no `directory` to check against.
- **Levels.** It compares the levels the reference's header lists, or the subset `--levels` names. A compared level that the candidate's header lacks is a `missing_level` difference at that level. A `--levels` name outside L0-L3, a level the reference lacks, and an empty selection are refused (exit 2).
- **What it ignores.** `note` members and `producer` are never compared. A reference written by `unitypy_dump.py` must record `boost` true and UnityPy 1.25.2, or the comparison is refused.
- **How it compares.**
  - Values are compared after decoding. `f32s` and `f64s` unpack to their floats. `u8s`, `bytes` and hex-tagged strings stay raw bytes, each kind apart from the others, and an inline payload and its summary compare by length and sha256.
  - Types must match exactly, at any depth: `true` is not `1`, a tuple is not a list, an `f32` is not an `f64`, also inside a struct inside an array.
- **Format findings.** Every line of both dumps is checked on its own, byte-identical pairs and the header and `end` records included. Each of these is a **format** finding:
  - a line that is not the canonical serialisation of itself, or that holds a JSON float;
  - members that are not each record's list (section 4) in order;
  - a non-null section for a level its header does not declare, and a `null` one for a level it declares: an object's `l1`, `l2` or `l3`, and a file's `l1` when `l0.parsed_as` is `SerializedFile`, `BundleFile` or `WebFile`;
  - a file `l1` whose `kind` is not the one its `parsed_as` gives (`serialized`, `bundle`, `web`), or that is there for `raw`, `zip` or `error`; an `error` that is not `null` exactly when `parsed_as` is `"error"`; an `l2.value` that is not there exactly when `l2.error` is `null` or `check_read`;
  - a name (the metadata-string rule, section 4.2, at every position it lists, and the `file` members) that is neither a JSON string nor `{"t":"str","hex":H}` with lowercase, even-length `H`, or whose hex bytes are valid UTF-8;
  - a value encoding that breaks a rule a value shows on its own: a JSON array of tagged floats (it should have been packed), inline hex above 64 bytes, a summary of 64 bytes or fewer, uppercase or odd-length hex, an empty packed list, a `str` or hex-tagged dict key whose bytes are valid UTF-8, or a repeated dict key;
  - a pair of lines whose compared values agree but whose bytes differ for a reason the rules above do not name;
  - a record after the `end` record, a missing `end` record, or one whose counts are not the integers its own dump holds.
- **Unreadable input** exits 2: a line that is not JSON, not UTF-8 or not valid Unicode (a lone surrogate escape), `NaN` or `Infinity`, a truncated or corrupt gzip stream, a header that is not this format and version or whose `levels` are not L0 followed by any of L1-L3 in order, a second header, or a record without a valid key (`file` not a list of names, `path_id` not an integer).
- **Report.** Differences are counted by level and class (`root_type`), then grouped by field path, with list indices folded to `[]`, and a few concrete examples per group.
- **What each level covers.** L0: detection, and missing or extra files. L1: container and object-table metadata, load errors, and missing or extra objects. L2: values and parse outcome. L3: resave.

## 7. UnityPy behaviours the oracle records

These are UnityPy 1.25.2 facts that a unityrs producer must reproduce to match, or record as a deliberate divergence in `PARITY.md`.

**L0 detection.**
- The eight-`0xFA` bundle signature is compared as eight U+00FA characters. The surrogate-escaped bytes `FA` never match, so such a file reads as `ResourceFile`, while the UTF-8 bytes `C3 BA` repeated eight times do match.
- A zip is detected only when its fifth byte is NUL, because the signature runs to the first NUL and must equal `PK\x03\x04`. An ordinary zip, with version-needed 20 (byte `0x14`), reads as `ResourceFile`.
- A 20-byte file with no NUL makes detection raise (`l0.type` `"error"`).
- The SerializedFile check reads its header big-endian, whatever the file's endian byte says.

**Floats.**
- UnityPyBoost widens a float32 to a Python float in C (`PyFloat_FromDouble`). On x86-64 that conversion quiets a signalling NaN: the file's `7f800001` reaches the dict as `7fc00001`.
- The oracle dump therefore shows the quieted bits. A producer that reports the file's bits differs here, and `compare.py` labels such pairs `f32_snan_quieted` so they stand apart from real errors. The label requires the candidate to be a signalling NaN: all exponent bits set, the quiet bit (bit 22) clear and a non-zero mantissa, with the reference the same bits plus bit 22. A quiet NaN against an infinity also differs only in bit 22, but it is an ordinary `value` difference.
- `save_typetree` writes the quieted bits back, so such objects resave as `differs`.
- float64 values pass through unconverted and keep every bit.

**Booleans.** Any non-zero byte reads as `true`. `save_typetree` writes `1`, so a stored `2` resaves as `differs`.

**Array alignment.** Alignment is to 4 bytes, counted from the start of the object's data.
- UnityPy's writer (`write_typetree`, so L3) writes an array element by element. After each element it aligns when the element node carries the align flag (`0x4000`), and after the array it aligns once more when the vector node or its `Array` node carries it.
- UnityPyBoost reads an array of integer, float, `bool` or `pair` elements as one run, and aligns once after the run when the element node carries the flag. It reads elements of any other type (`string`, `TypelessData`, classes, vectors) one at a time, each aligning after itself as the writer does.
- So when a run-read element with the flag, other than the last, ends off a 4-byte boundary, the writer adds padding the file does not have. The elements after it move, and the object resaves as `differs` unless the moved bytes happen to match. Three aligned `UInt8` elements `010203` in a 12-byte object resave as 20 bytes, first differing at offset 5. Two aligned `UInt32` elements after a lone `UInt8` keep the object's size and differ from offset 9.
- A flag on the vector or `Array` node alone aligns once on both sides, and so does a one-element array, so those resave as `equal`.
- A producer's own writer matches L3 only when it pads after each aligned element in the same way.

**Parsing path.**
- Every object with `byte_size` > 0 is read by UnityPyBoost. Its arrays are lists, and its strings are always followed by a 4-byte alignment.
- With `byte_size` 0, UnityPy uses its pure-Python reader instead (section 4.3 says where it starts and how far it reads). It differs from UnityPyBoost in these ways:
  - It returns a tuple for an array whose primitive element node carries the align flag. The tuple is encoded as `tuple` and never packed.
  - In such an array, 16-bit elements swap signedness. `UInt16` and `unsigned short` read as signed, and `SInt16` and `short` read as unsigned, because UnityPy's `read_u_short_array` unpacks `h` and `read_short_array` unpacks `H`. A `UInt16` 65535 reads as -1.
  - An aligned `SInt8` array raises a `TypeError` (`other`).
  - An array of aligned `TypelessData` elements raises a `TypeError` (`other`), because the array reader calls `read_bytes()` without a length. UnityPyBoost reads each element's length and bytes.
  - In an array whose aligned element is itself a vector, a negative inner length reads as an empty list. UnityPyBoost raises `negative_length`.
  - It reads a negative or out-of-range string length as `""` instead of failing.
  - In an array of aligned class elements it aligns once after the array, where UnityPyBoost aligns after each element. Two elements `{UInt8 x}` stored as `01000000 02000000` read as `x` 1 and 0 here, and the field after the array reads the second element's bytes.
  - Its array reader takes an array whose element node carries the align flag. For class elements it reads every child with no registry check, so the registry scope of section 3 does not hold there: an element `{r1, r2}` of two registries reads as `{r1}` on UnityPyBoost and `{r1, r2}` here. Everywhere else both paths apply the same scope.
  - It reads a TypelessData length of -1 as the rest of the SerializedFile, so the next read fails (`other`). Any other negative length `L` at position `p` reads the Python slice `[p:p+L]` of the SerializedFile's bytes and moves the position to `p+L`. The slice is empty unless `p+L` is negative, which counts from the end. The fields after it then re-read earlier bytes. UnityPyBoost raises `negative_length` for both.

**ReferencedObject.**
- UnityPyBoost skips the `data` of an empty class name in arrays as well. The pure-Python path does not; that bug is not reachable on the Boost path.
- UnityPy's writer needs every ReferencedObject's `data` entry, which the reader leaves out for an empty class name. An object holding one therefore resaves as `error`: `KeyError: 'data'`, or `ValueError: SerializedFile has no ref_types` when the file has none. A writer that round-trips the reference gets `equal` there.
- UnityPyBoost drops one reference to `None` for each empty-class ReferencedObject it reads: `get_ref_type_node` returns `Py_None` without a new reference, and its caller releases one. On Python 3.11 a process that reads as many of them as `None` has references aborts (`none_dealloc`), wherever it is. The oracle pins `None` in each input's process, so no record shows the leak.

**UnityPyBoost crashes.** UnityPyBoost builds typetree nodes with `PyUnicode_AsUTF8` and uses the result unchecked. A node type or name that is not valid UTF-8 therefore kills the process while the SerializedFile loads: a bad name aborts (`SIGABRT`) and a bad type segfaults (`SIGSEGV`). Such an input gets the crash record (section 4.2), so no reference exists for its contents. A unityrs producer that dumps the file, with hex-tagged names (section 4.2), records the difference as a deliberate divergence.

**Object table.**
- `objects` is a dict keyed by `path_id`, so a repeated `path_id` keeps its first position with the last entry. `object_order` and the `object` records show that collapsed view.
- Below format 16, an object's type is the first type whose `class_id` equals the object's `type_id`.
- From format 16 it is `types[type_id]`, indexed as Python indexes: a negative `type_id` counts from the end (`-1` is the last type), and one outside the list fails the input (`parse`, `IndexError`). The object's `class_id` is that type's.
- For formats 11-16, loading writes each object's script index back into that shared type. A type's `script_type_index` is therefore the last such object's value.

**Load-time parses.**
- A SerializedFile parses its AssetBundle object while loading, so a broken AssetBundle fails the whole top-level input (`error.kind` `"parse"`), even when the SerializedFile is a bundle entry (section 4.2).
- UnityPy prints every load error to stdout, quoting the file's bytes raw (`Invalid version string: ` and the surrogate-escaped string). The oracle discards that print, because with a strict stdout encoding the print itself raises and replaces the error, and it escapes the note (section 2).
- A bundle on the UnityFS layout packs its engine version while loading, so one that `parse_version` cannot pack fails the input (section 4.2's version-packing rule). A SerializedFile's own version string fails the input when it does not match the rule, and when it is empty or `0.0.0` with no parent bundle to take a version from.

## 8. Not covered by format 1

- **Zip/apk members and `.splitN` merging.** Inputs are dumped one by one. An input detected as a zip gets a `file` record with `parsed_as` `"zip"` and no children; an entry detected as one is `raw` (section 4.2).
- **Object mode** (`parse_as_object`, `clean_name` attribute names) and the classes built from it.
- **`peek_name()`.** UnityPy caches its peek node globally by the root's `(m_Name, m_Type, m_Version)`, so the answer can depend on which file a process read first, and an order-independent dump cannot pin it. The name is in the L2 value.
- **Environment-wide state:** `obj.container` after the preload pass, and cross-file PPtr resolution. These belong to a later Environment level.
- **L2b TPK node lists** for stripped files, L4 file save, and every level from L5 up.

## 9. Changing the format

- A change that alters any line for the same input bumps `version`.
- `compare.py` refuses to mix versions.
- Re-pinning UnityPy does not change the format, but it can change the oracle's records. The header's `producer` says which UnityPy wrote a dump.
