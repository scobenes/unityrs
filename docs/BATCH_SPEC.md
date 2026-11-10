# The batch job contract, version 1

**Status: draft, design round 3 (2026-11-09), not reviewed.** This file is the exact contract of `unityrs batch`: the job spec, keys and templates, identity and order, the NGUI cutter, the spool, the manifest, the zip bytes, the exit codes and the progress stream. `docs/plans/extract.md` states the requirements (E1 onward) and cites this file for their exact form; `docs/plans/batch.md` designs the engine behind them. Where this file and a requirement disagree, the review rounds fix one of them.

**Conventions.**
- *bytes*: an integer count of bytes. *ms*: an integer count of milliseconds.
- *Bytewise order*: the order of the strings' UTF-8 bytes, compared as unsigned bytes, a shorter prefix first.
- *Refused at validation*: the binary exits 78 before any work (section 9).
- `M`, `D` and `I` abbreviate `max_decoded_bytes`, `dep_cache_bytes` and `max_inflight_uncompressed_bytes`.
- Every integer field is at most 2^53 - 1, so every JSON parser reads it exactly. The engine computes in `u64` with checked arithmetic, and the ranges below keep every sum it forms below 2^63.

---

## 1. The job spec

A spec is one JSON object, at most 16 MiB, UTF-8. An unknown field at any level, a duplicate member in any object, a value of the wrong type and a value out of its range are refused at validation. `docs/batch/spec-v1.schema.json` (JSON Schema 2020-12) is written by hand from these tables; it covers each field's type and range, and the cross-field rules are the parser's alone (`extract.md` section 3, item 4).

### 1.1 Top level

| field | type | required | default | allowed values and meaning |
|---|---|---|---|---|
| `spec_version` | integer | yes | | `1` |
| `run_id` | string | yes | | 1-64 bytes of `[A-Za-z0-9._-]`, not `.` or `..`. Names one logical run; its attempts share it (section 7) |
| `attempt` | integer | yes | | 1 to 2^31-1. The caller raises it for each attempt of one `run_id` |
| `deadline` | string or null | no; yes with an object store sink | `null` | an RFC 3339 date-time with `Z` or an offset; `null` sets none (E45) |
| `shutdown_grace_ms` | integer | no | 20000 | 5,000 to 600,000 (E45) |
| `stop_on_stdin_eof` | boolean | no | `false` | E46 |
| `parent_pid` | integer or null | no | `null` | 2 to 2^22: the caller's pid, checked at start (E46) |
| `threads` | integer | yes | | 1 to 1,024: the most attempts that run at once |
| `download_concurrency` | integer | yes | | 1 to 256: the most stages and transfers at once (E8) |
| `max_inflight_uncompressed_bytes` | integer | yes | | `I`, 64 MiB to 2^50: what the attempts in flight may reserve together, and phases 6 and 7's working set (`batch.md` 2.2) |
| `max_decoded_bytes` | integer | yes | | `M`, 1 MiB to 2^40: every meter's `max_input_memory` and `max_input_bytes`, and each attempt's Input budget |
| `dep_cache_bytes` | integer | yes | | `D`, 1 MiB to 2^40: each attempt's Dependency budget and its private dependency cache (`batch.md` 2.2) |
| `max_view_bundles` | integer | no | 1024 | 1 to 65,536: the most bundles one input's view may hold |
| `max_attempt_evictions` | integer | no | 1024 | 0 to 2^20: the most dependency evictions one attempt may cause (`batch.md` 2.6 step 7); more fails the input, `limit(attempt_evictions, <value>)` |
| `spool` | Spool | yes | | section 1.3 |
| `http` | Http | no | every default | section 1.4 |
| `inputs` | array of Bundle | yes | | 1 to 100,000 items |
| `dependencies` | array of Bundle | no | `[]` | bundles that are read but not extracted |
| `outputs` | Outputs | yes | | section 1.5 |
| `prefer` | object | no | `{}` | output key → Prefer (E31) |
| `publish_incomplete` | boolean | no | `true` | E61 |
| `resume` | boolean | no | `false` | E43 |
| `sink` | Sink | yes | | section 1.6 |
| `archives` | array of Archive | no | `[]` | section 1.7; 0 to 256 items |
| `manifest_fencing` | string | no | `"conditional"` | `"conditional"` or `"none"` (section 7.2) |

**Cross-field rules**, each refused at validation:
- `M <= D` and `M + D <= I` (`batch.md` 2.2).
- Every `id` is unique across `inputs` and `dependencies`.
- Every id in a `deps` list names a bundle of the spec.
- No view (section 4.2) holds more than `max_view_bundles` bundles.
- With a directory sink, `spool.dir` and `sink.root`, compared as paths after removing a trailing `/`, neither equal nor contain one another (preflight compares their real paths again, section 6.2).
- `deadline` is set when `sink.kind` is `"object_store"` (the time fence, section 7.2).
- Every template parses and passes its literal checks (section 2.1), and every literal key (`prefer`'s keys, `archives`' keys, the sink's prefix) passes section 2.3's rules but `prefix_conflict` and `archive_conflict`, which phase 6 decides.
- `archives`' keys are unique, and none is an ancestor of another (`a` of `a/b.zip`). In one archive, each member `kind` appears once.

### 1.2 Bundle (an item of `inputs` or `dependencies`)

| field | type | required | default | allowed values and meaning |
|---|---|---|---|---|
| `id` | string | yes | | 1-1,024 bytes of UTF-8, no control character. The bundle's identity (section 4.1) |
| `path` | string | one of `path`, `url` | | an absolute path of a local regular file |
| `url` | string | one of `path`, `url` | | an `http` or `https` URL with no userinfo |
| `headers` | object | no | `{}` | only with `url`. Header name → a string, or `{"env": NAME}` read from the environment at preflight |
| `size` | integer | no | | 0 to 2^53-1: the expected byte count |
| `sha256` | string | no | | 64 lowercase hex digits: the expected digest |
| `deps` | array of string | no | `[]` | the ids this bundle needs (E11). A dependency may list its own |

Every bundle is staged into the spool before extraction (`batch.md` 2.5), and every read of it reads the staged copy. A `size` or `sha256` that does not match the staged bytes fails the bundle (`integrity_mismatch`). A local path that does not exist, is not a regular file or cannot be opened fails it (`source_missing`). Every bundle in a view must be an AssetBundle file; a member that is not fails its inputs (`dependency_failed(<id>, not_a_bundle)`).

### 1.3 Spool

| field | type | required | allowed values and meaning |
|---|---|---|---|
| `dir` | string | yes | an absolute path of an existing directory on a local filesystem, dedicated to unityrs spools (section 6.2) |
| `size_bytes` | integer | yes | 1 MiB to 2^53-1: the cap on what the run writes there |
| `reserve_bytes` | integer | yes | 0 to 2^53-1: the free space, on the spool's filesystem, that every claim leaves |

### 1.4 Http

| field | type | default | allowed values and meaning |
|---|---|---|---|
| `idle_timeout_ms` | integer | 30000 | 1,000 to 600,000: a try fails (`"timeout"`) when no byte arrives for this long |
| `min_rate_bytes` | integer | 1024 | 1 to 2^30: a try also fails (`"timeout"`) when fewer than this many bytes arrive in a period of `idle_timeout_ms`, the periods counted back to back from the request's send. There is no whole-request timeout, so a download slowed by its neighbours takes longer and fails only below this floor. A staged bundle's bytes are claimed from `spool.size_bytes`, so with the floor every try ends |
| `connect_timeout_ms` | integer | 10000 | 100 to 60,000 |
| `attempts` | integer | 3 | 1 to 10: tries per download, the first included |
| `backoff_initial_ms` | integer | 500 | 0 to 60,000 |
| `backoff_max_ms` | integer | 30000 | at least `backoff_initial_ms`, at most 600,000 |
| `retry_on` | array of string | all five | a subset of `"connect"`, `"timeout"`, `"status_5xx"`, `"status_429"`, `"truncated"` |
| `redirects` | string | `"same_origin"` | `"same_origin"` or `"none"` |

- The k-th retry waits `min(backoff_max_ms, backoff_initial_ms x 2^(k-1))`, with no jitter. A `Retry-After` header (seconds, or an HTTP date) raises that wait to its value, still capped at `backoff_max_ms`.
- No retry starts when its wait would pass the stop cutoff (the deadline less the grace, E7). A request in flight at a stop is cancelled (`batch.md` 2.11).
- `"same_origin"` follows at most 5 redirects whose scheme, host and port equal the request's, with its headers. A redirect anywhere else, or past the fifth, fails the download (`redirect_refused`), so a header never reaches another origin. `"none"` fails at the first redirect.
- A body shorter than its `Content-Length` is `"truncated"`.

### 1.5 Outputs

| field | type | required | meaning |
|---|---|---|---|
| `textures` | `{"key": Template}` | no | one PNG per selected Texture2D (E66) |
| `sprites` | `{"key": Template}` | no | one PNG per Sprite (E17) |
| `atlases` | Atlases | no | NGUI atlases (section 5) |

At least one of the three is present. **Atlases:**

| field | type | required | default | meaning |
|---|---|---|---|---|
| `sprite_key` | Template | yes | | one PNG per atlas sprite |
| `json_key` | Template | yes | | the atlas JSON (E24) |
| `script_classes` | array of string | no | `["NGUIAtlas"]` | the `m_ClassName`s that make a MonoBehaviour an atlas (section 5) |
| `skip_name_suffixes` | array of string | no | `[]` | an atlas whose `m_Name` ends with one of them is skipped as a reference atlas (E22) |

**Prefer:** `{"input": id or null, "md5": 32 lowercase hex digits or null}`, at least one not null (E31).

### 1.6 Sink

One sink per job. Outputs are written to it as loose objects, and each archive (section 1.7) is written to it as one more object.

| `kind` | fields |
|---|---|
| `"directory"` | `root` (required): an absolute path of an existing directory |
| `"object_store"` | `bucket` (string, required); `region` (string, required); `prefix` (string, default `""`, else ending in `/`); `endpoint` (an `http` or `https` URL or null, default null: AWS; an `http` endpoint allows plain HTTP to that endpoint only); `path_style` (boolean, default `false`); `timeout_ms` (integer, default 30000, 1,000 to 300,000, per request); `attempts` (integer, default 3, 1 to 10) |

Every key is written at the sink's root (directory) or after its prefix (object store). Credentials come from the environment only (E37): `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` when set. Either of the first two missing exits 69 (`credentials_missing`).

### 1.7 Archive

| field | type | required | meaning |
|---|---|---|---|
| `key` | string | yes | the archive's key in the sink |
| `members` | array of `{"kind": string, "path": Template}` | yes | 1 to 4 items, one per output kind: `"texture"`, `"sprite"`, `"atlas_sprite"` or `"atlas_json"`. Every published output of that kind is a member of the archive, at the path the template expands to with that output's variables (section 4.7) |

A member is also published as a loose object. Which outputs are members is decided in phase 6, from the winners alone (section 4.7). An archive left with no member is not written (`empty`).

---

## 2. Templates and keys

### 2.1 Templates

```
template = 1*( literal / variable )
literal  = 1*( any character but "{" and "}" / "{{" / "}}" )
variable = "{" name "}"
```

`{{` and `}}` write one brace. An unknown variable, or one not allowed for the template's kind, is refused at validation.

| variable | value | allowed in |
|---|---|---|
| `input`, `safe_input` | the input's `id` | every template |
| `type` | the object's class name: `Texture2D` or `Sprite` | `textures`, `sprites`, and archive paths of kind `texture` or `sprite` |
| `name`, `safe_name` | the object's `m_Name` | as `type` |
| `path_id` | the object's path id, signed decimal, no leading zero | every template; an atlas's is the MonoBehaviour's |
| `atlas`, `safe_atlas` | the atlas MonoBehaviour's `m_Name` | `sprite_key`, `json_key`, and archive paths of kind `atlas_sprite` or `atlas_json` |
| `sprite`, `safe_sprite` | the `mSprites` element's `name` | `sprite_key`, and archive paths of kind `atlas_sprite` |

A plain variable is written verbatim: a name's stored bytes. Its `safe_` form is `safe()` of the same value (2.2). There is no `{container}` variable in version 1 (E27).

**Literal checks at validation.** What the literals fix is checked on the template: each `/`-separated segment made only of literals passes section 2.3's segment rules (`empty_segment`, `dot_segment`, `segment_too_long`, `reserved_name`); a template that starts with a literal passes `leading_slash` and `reserved_prefix` on it; one that ends with a literal passes `trailing_slash`. Everything else is checked on each expansion.

### 2.2 `safe()`

1. Decode the bytes as UTF-8, each invalid byte becoming one code point of its own (UnityPy's `surrogateescape` gives the same count).
2. Replace every code point outside `[A-Za-z0-9._-]` with `_`.
3. Strip every leading and trailing `.` (Python's `str.strip(".")`).
4. An empty result becomes `unnamed`.

### 2.3 Key rules

A key is the template's expansion. It is checked as given, then after the sink's prefix is put before it. The first rule it breaks names the refusal:

| rule | refused when |
|---|---|
| `empty` | the key is empty |
| `not_utf8` | it is not valid UTF-8 |
| `control_char` | it holds a code point below U+0020, or U+007F |
| `leading_slash`, `trailing_slash` | it starts or ends with `/` |
| `empty_segment` | it holds `//` |
| `dot_segment` | a segment is `.` or `..` |
| `segment_too_long` | a segment is over 255 bytes (`NAME_MAX`) |
| `too_long` | the full key is over 1,024 bytes |
| `reserved_prefix` | it starts with `_unityrs/` (section 7) |
| `reserved_name` | a segment starts with `.unityrs-tmp-` (the directory sink's temp files, E62) |
| `prefix_conflict` | it is an ancestor of another published key (section 4.6) |
| `archive_conflict` | it equals an archive's key, is an ancestor of one, or lies under one (section 4.6) |

A literal key that breaks a rule is refused at validation. An expanded key that does is the output's object-level skip, `unwritable_key(<rule>)`. The rules are the same for every sink, and an archive member path takes every rule but `reserved_prefix`, `reserved_name` and `archive_conflict` (section 4.7), so a key is writable in one sink exactly when it is writable in all of them.

---

## 3. Selection

What one input exports (E66):
- **Textures**, when `outputs.textures` is set: every Texture2D object of the input's own SerializedFiles. A sheet section 5 cuts is exported too, as the unit's texture (section 4.5).
- **Sprites**, when `outputs.sprites` is set: every Sprite object of the input's own SerializedFiles. The sheet a sprite renders from may be in a dependency (`batch.md` 2.6).
- **Atlases**, when `outputs.atlases` is set: section 5.

A texture is exported only in the five formats of E15, and only where UnityPy 1.25.2 neither unswizzles nor byte-swaps it:
- any other format is skipped, `unsupported_format(<name>)`;
- a texture for which UnityPy's `is_switch_swizzled` holds (`helpers/TextureSwizzler.py`: platform Switch, platform blob of at least 12 bytes, more than one GOB per block) is skipped, `unsupported_platform(switch)`;
- an Xbox 360 texture of a format UnityPy byte-swaps (`XBOX_SWAP_FORMATS`; of the five, DXT1 and DXT5) is skipped, `unsupported_platform(xbox360)`. An Xbox 360 RGBA32 or RGB24 texture is exported like any other.

A texture with a side of 0 is skipped, `empty_image`. UnityPy's extractor skips a zero width and raises on a zero height; the oracle records both as `empty_image` (`extract.md` section 5).

---

## 4. Identity and order

### 4.1 Bundles

- A bundle's identity is its spec `id`.
- A job declares its whole dependency graph. A name in an input's AssetBundle object's `m_Dependencies` is compared, exactly (no case folding, no path or variant rule), with the ids of the bundles of its view. A name that matches none is reported, never fetched or opened: a `diagnostics` row (`undeclared_dependency`, the input, the name) and the run's `undeclared_dependencies` count. A caller who spells ids as `m_Dependencies` spells them gets no such row for a declared bundle.
- Wherever bundles are ordered, it is by bytewise order of their ids.

### 4.2 Views

`view(X)` is the smallest set that holds `X` and the `deps` of every bundle it holds. No `m_Dependencies` list ever adds to it.

### 4.3 Resolution inside an attempt

- The attempt's private Environment registers `view(X)` less `X` in bytewise order of id, then `X` last (`batch.md` 2.6).
- **A pointer** is dereferenced by the core's `Environment::deref` from the file that holds it. The reading file's own bundle answers first (its siblings, through `Parent`; Tier A). Then the registry answers with the registration of greatest rank: `X` when it holds the CAB, else the greatest id among the view's bundles that hold it.
- **A resource** (`.resS`) read takes UnityPy's candidates in order. Every candidate is looked for in the reading file's own container chain first; only when none is found there are they looked for through `Environment::resource` on the private Environment (`batch.md` 2.6).
- A CAB no bundle of the view holds is the reading object's skip, `outside_view(<cab>)`.

### 4.4 Objects and candidates

- **Object order** in a bundle: its directory's entries in directory order, the SerializedFiles among them, and in each SerializedFile its object table's order.
- **One input's names** (E29). Of two objects of one class and one `m_Name`, the first in object order takes the name, even when its export then fails. The later one is skipped, `duplicate_name`.
- **One input's keys.** Of two outputs of one input with one key, the first in (kind, object order) is the candidate, kinds ordered `textures`, `sprites`, `atlases`, and within one atlas its JSON before its sprites, the sprites in `mSprites` order; the later one is skipped, `duplicate_key`. The exception: inside one atlas, of two sprites with one key, the later one wins (E30).
- **Sprite groups.** Sprites are grouped by their sheet texture (bundle id, path id), and the groups run in the order of each group's first sprite.

### 4.5 Winners across inputs

E31 to E33 and E61 as written, with these orders and units:
- "first by bytewise input id" everywhere;
- E31 rule 1 applies when the preferred input has a candidate for the key;
- **Units** (E32). With `outputs.textures` set, a unit is an input's sheet texture with every atlas of that input whose sheet it is (section 5), their crops and their JSONs; its **unit key** is the texture's key. Without `outputs.textures`, a unit is one atlas with its crops and JSON; its unit key is the JSON key. A unit's **keys** are its unit key and its members' keys. Its `prefer` entry is its unit key's, and E31 rule 2 compares the MD5 of its unit key's output.
- The steps, in order (`batch.md` 2.9):
  1. Each unit key's winning unit is decided by E31 and E61 among the units with that unit key.
  2. Every key of every winning unit is **reserved**. Of the winning units that share a key, the first by unit key keeps it, and every other is withheld whole (`unit_conflict`). A key stays reserved even when every unit claiming it is withheld; this step is not repeated.
  3. A plain candidate for a reserved key loses (rule `unit`). Every other key is decided by E31 and E61.
  4. Section 4.6 runs once on the keys steps 1 to 3 would publish.
- So phase 6 never publishes a unit in part: a unit is published whole or withheld whole. An attempt-time skip of one of its outputs (`unwritable_key`, `duplicate_key`) removes that output only.

### 4.6 Prefix conflicts

Let `P` be the keys section 4.5's steps 1 to 3 would publish: withheld keys, `unit_conflict` keys and losers are not in `P`.
- A key of `P` that is an ancestor of another key of `P` (some key of `P` starts with `K/`) is `unwritable_key(prefix_conflict)`; the keys beneath it stay.
- A key of `P` that equals an archive's key, is an ancestor of one, or lies under one is `unwritable_key(archive_conflict)`; the archive stays.
- A unit with a key refused here is withheld whole: its other keys are `unit_conflict`.
- The check runs once, on `P` as steps 1 to 3 left it, and is not repeated after a unit is withheld. It runs on the sorted keys with each key's ancestors added, so its result does not depend on the order of publishing (`batch.md` 2.9). It is the same in every sink. What remains is the set of **winners**.

### 4.7 Archive members

For each archive and each of its `members` entries, every winner of that kind is a member, at the path the entry's template gives for it (expanded by the attempt that made the candidate, `batch.md` 2.8). Then, per archive:
- members are ordered by bytewise order of path;
- of several members with one path, the one whose key is first in bytewise order is kept, and the others are left out (`duplicate_member`);
- a path that breaks a rule of section 2.3 is left out (`unwritable_path(<rule>)`), and a path that is an ancestor of another member's is left out (`prefix_conflict`).

Membership depends only on the winners, never on how their loose PUTs end, so an archive's bytes are a function of the committed records. With `publish_incomplete: false` and a failed input, nothing is published and every archive is `withheld`.

---

## 5. The NGUI cutter

**Recognition.** A MonoBehaviour of the input's own SerializedFiles is an atlas when:
1. its `m_Script` dereferences (a read that may cross into the view; E21) to a MonoScript whose `m_ClassName` is in `script_classes`; and
2. its typetree has a field `mSprites` that is an array.

A MonoBehaviour that meets 1 but not 2 is skipped, `ngui_layout_unsupported` (NGUI 2's `sprites` layout is not in version 1). One with no typetree is skipped, `no_typetree`; version 1 has no fallback for a stripped atlas. One whose `m_Script` cannot be read is not an atlas, and the input's record counts it (`unresolved_scripts`).

**Skipped atlases.** An atlas with an empty `m_Name` is skipped, `unnamed_atlas`. An atlas whose `m_Name` ends with a string of `skip_name_suffixes` is skipped, `reference_atlas`.

**The sheet** (E21, scope `own_files_by_name`; `follow_pptr` is not in version 1).
1. The material's main-texture name: dereference the atlas's `material`, then, in its `m_SavedProperties.m_TexEnvs` (a list of pairs), the first pair whose first is `_MainTex`; dereference that pair's `m_Texture` and read its `m_Name`. Any error on this path gives no name, and the next step runs.
2. Among the input's own Texture2D objects in object order, the first whose `m_Name` equals that name exactly (bytes, case-sensitive). When there is none, or no name, the first whose `m_Name` equals the atlas's `m_Name`.
3. No match skips the atlas, `sheet_not_found`.

The sheet is decoded as a texture (E15), into the image UnityPy's `Texture2D.image` gives: rows top to bottom. With `outputs.textures` set, the atlases of one input that cut one sheet are one unit with it; without it, each atlas is its own unit (section 4.5).

**Crops.** For each element of `mSprites`, in array order:
- its rectangle is `(x, y, x + width, y + height)` on that image, origin at its top-left, the element's `x`, `y`, `width` and `height` as integers. Borders, paddings and mirror flags are not applied;
- a `width` or `height` of 0 or less skips that sprite, `empty_rect`;
- a rectangle past the image's edge is padded with zero pixels, as Pillow 9.5.0's `Image.crop` pads (E23). The crop keeps the sheet's mode.

**The JSON** (E24): CPython 3.11's `json.dumps(value, indent=2)` of an object with these keys, in this order:

| key | value |
|---|---|
| `name` | the atlas's `m_Name` |
| `texture_width`, `texture_height` | the sheet image's size |
| `pixel_size` | the typetree's `mPixelSize` as read, or `null` when absent |
| `settings` | the typetree's `mSettings` as read, or `null` when absent |
| `sprite_count` | the length of `mSprites` |
| `sprites` | each element of `mSprites`, as read, in array order |

"As read" means UnityPy 1.25.2's `read_typetree()` value, written as `json.dumps` writes it: a typetree dict as an object in its field order, a pair as a 2-element array, an f32 widened exactly to the double, a string as UnityPy decodes it (`surrogateescape`, so an invalid byte is written `\udcXX`). A value `json.dumps` cannot write (bytes) skips the JSON, `json_unwritable`, and keeps the crops (E25). The field names are the typetree's, not renamed. The sheet's key is the outputs row's `unit` (section 7.3).

---

## 6. The spool

### 6.1 Layout

| path | holds |
|---|---|
| `<dir>/.unityrs-spool` | the marker: an empty file the engine writes when it first uses an empty directory |
| `<dir>/lock` | the spool's lock: `flock(LOCK_EX \| LOCK_NB)` for the process's life (E63). Created if absent, never deleted |
| `<dir>/runs/<run_id>/journal` | multipart upload ids, one per line: endpoint, `bucket`, `key`, `upload_id`, appended and `fdatasync`ed after the create, removed after the complete or abort (E64) |
| `<dir>/runs/<run_id>/in/` | staged bundles, read-only: `i<n>` for the n-th of `inputs`, `d<n>` for the n-th of `dependencies` (`batch.md` 2.5) |
| `<dir>/runs/<run_id>/attempts/<input>/` | one attempt's outputs and `records.ndjson` |
| `<dir>/runs/<run_id>/done/<input>/` | a committed attempt |
| `<dir>/runs/<run_id>/winners/` | phase 6's sorted runs, the winners file, the archive member lists and the sorted row files |
| `<dir>/runs/<run_id>/winners/outcomes`, `archive_outcomes` | phase 7's results: one byte per winner at its index in the winners file (0 not written, 1 `written`, 2 `present`, 3 `failed`), and one 128-byte record per archive (status, size, MD5, ETag), sized before the first transfer (`batch.md` 2.9) |

`<input>` and `<n>` are positions from 0, in decimal, so an id never becomes a path.

### 6.2 Start

The engine owns only what it created. Preflight starts the spool in this order, after reading the credentials. A failure in steps 1 to 3 exits 69 with nothing deleted; step 4 never fails the start; a failure in step 6 exits 69 after step 5's deletions, which removed only the engine's own files:
1. **Dedicated.** `dir`'s entries must be none, or only `.unityrs-spool`, `lock` and `runs`, the marker among them, with `lock` a regular file and `runs` a directory, by `lstat` (neither a symlink). Otherwise `spool_not_dedicated`.
2. **Apart from the sink.** With a directory sink, the real paths of `dir` and `root` neither equal nor contain one another. Otherwise `spool_overlaps_sink`.
3. **The lock.** `lock` is opened (created if absent) and locked; busy is `spool_locked`. The lock is held until `_exit` and the file is never deleted, so a second process always meets the same inode. An empty `dir` gets its marker now, under the lock.
4. **Old journals.** Every `runs/*/journal` is read; each upload id whose endpoint and bucket are this sink's is aborted, at most `download_concurrency` at once, each with the sink's `timeout_ms` and `attempts`. `NoSuchUpload` counts as aborted: the upload completed before its id was removed, or the lifecycle rule took it. Any other failure, a 403 among them, leaves the id to the bucket's lifecycle rule (E64) and counts in the warning `abort_failed`, so no journal can stop a later start. An id of another endpoint or bucket is left to that bucket's lifecycle rule.
5. **Cleanup.** Everything under `runs/` is deleted, through directory handles opened with `O_NOFOLLOW`, so no symlink is followed out of the spool; this `run_id`'s earlier attempts and other run ids' alike: the lock proves no live process uses them.
6. **Free space.** `statvfs` of `dir` must show at least `reserve_bytes` free. Otherwise `spool_no_space`.

Nothing in the spool is reused between attempts: every attempt stages and decodes again (E44). After the final manifest, and after a stop, the spool is left as it is; the next start clears it.

---

## 7. The manifest

### 7.1 Location

| sink | the head | the row files |
|---|---|---|
| directory | `<root>/_unityrs/<run_id>/manifest.json` | `<root>/_unityrs/<run_id>/<attempt>/*.ndjson` |
| object store | `<prefix>_unityrs/<run_id>/manifest.json` | `<prefix>_unityrs/<run_id>/<attempt>/*.ndjson` |

No output key may start with `_unityrs/` (2.3). The row files hold the attempt number, so an earlier attempt never overwrites a later one's rows. The head is at most 64 KiB.

### 7.2 Fencing

- **Preflight** reads the head; a 404 proves the sink reachable too. A head over 1 MiB, or one that is not a manifest, exits 69 (`manifest_unreadable`); a sink that cannot be reached exits 69 (`sink_unreachable`). A head of this `run_id` whose `attempt` is at or above this one's exits 7 (`superseded`), and nothing is written.
- **Object store, `"conditional"`.** The store client is configured with `S3ConditionalPut::ETagMatch`. The `running` write is `If-None-Match: *` when no head exists, else `If-Match` on the ETag preflight read. The final write is `If-Match` on the `running` write's ETag.
- **A failed precondition** (412) is read back with a GET. A head with this `attempt` and this write's `write_seq` is this write, whose response was lost: success. A head of a higher `attempt` is `superseded`: the attempt writes nothing more and exits 7. A head of a lower `attempt` can meet only the `running` write: it is an earlier attempt's late head write that landed after preflight's read (once this attempt's head exists, an earlier attempt's conditional write cannot replace it, since every write changes the ETag). The attempt applies the time fence to that head as preflight would, and retries the `running` write with `If-Match` on that head's ETag, at most 4 times; a fifth lower head exits 69 (`previous_attempt_live`). At the final write, any head but this write is `superseded`. Only the finisher writes after `running` (`batch.md` 2.11), so no two writes of one attempt share a `write_seq`.
- **Object store, `"none"`.** Plain PUTs; the caller runs one attempt of a `run_id` at a time.
- **Directory.** `<root>/_unityrs/lock`, one lock for the whole root, whatever the `run_id`: `flock(LOCK_EX | LOCK_NB)` held for the process's life, busy exits 69 (`output_locked`). So one process at a time writes a root, and E62's cleanup of temp files never meets a live run's. Each head write is a temp file, `fsync`, a rename over the head, and an `fsync` of the directory.
- **The time fence** (object store). An earlier attempt of this `run_id` may still have a write in flight, after a stop and after a normal end alike: a try its client gave up on may still land (`batch.md` A14). So every head of this `run_id` with a lower `attempt` that preflight or a read-back finds fences this attempt. The head's own fence is its `ended_at` when it is final with a null `stop_reason` (a normal end, at which no output request is open), else its `deadline`, plus the sink's `timeout_ms` plus 30 s. Its **fence time** is the later of that and its `fence_until`, so a fence passes down every attempt of the `run_id`. A fence time past this attempt's stop cutoff (its `deadline` less the grace), or a head that gives neither time, exits 69 (`previous_attempt_live(<fence time>)`, which tells the caller when to retry). Otherwise phase 7 writes no output before the fence time, and this attempt's heads carry it as `fence_until`. An earlier attempt's process is gone by its deadline (E45), and every store connection is reset rather than drained when dropped or at `_exit` (`SO_LINGER` 0, `batch.md` 2.9), so every write it made has landed before this attempt LISTs or writes, and none lands after.

The head is the record of the run. For every key the final head lists as `written` or `present`, the store holds the bytes of its MD5, given A14 and no writer to the prefix but unityrs attempts. A key the head does not list so is not part of the run's output, whatever the store holds.

### 7.3 Fields

**The head:**

| field | type | meaning |
|---|---|---|
| `manifest_version` | integer | `1` |
| `run_id`, `attempt` | string, integer | from the spec |
| `write_seq` | integer | 0 for the `running` write, 1 for the final |
| `status` | string | `running`, `complete`, `complete_with_skips` or `incomplete` |
| `phase` | string | the phase the run reached (section 10's values) |
| `exit_code` | integer or null | null while `running` |
| `stop_reason` | string or null | a stop reason of section 9.3, or null |
| `binary_version`, `spec_version` | string, integer | |
| `started_at`, `deadline`, `ended_at` | string or null | RFC 3339, UTC |
| `fence_until` | string or null | RFC 3339, UTC: the time fence this attempt applied (section 7.2), or null |
| `counts` | object | integers: `bundles`, `bundles_failed`, `inputs`, `inputs_complete`, `inputs_with_skips`, `inputs_failed`, `inputs_not_run`, `outputs_written`, `outputs_present`, `outputs_not_written`, `outputs_withheld`, `outputs_failed`, `outputs_unwritable`, `skips`, `collisions`, `flips`, `archives_written`, `archives_present` |
| `run` | object | E40's run-level figures: `same_named_cabs`, `undeclared_dependencies`, `nested_containers` (counts; the rows hold the names), `opens`, `evictions`, `reopens`, `input_peak`, `dependency_peak`, `spool_peak`, `vm_hwm`, `memory_bound`, the budgets as read |
| `warnings` | object | warning code (section 9.3) → count |
| `rows` | object | `bundles`, `inputs`, `skips`, `diagnostics`, `outputs`, `collisions`, `archives`, `archive_members`: each row file's key relative to the head's directory, or null |

**The row files** (NDJSON, one object per line, each sorted as stated):

| file | sorted by | fields |
|---|---|---|
| `bundles` | id | `id`, `status` (`staged`, `failed`, `not_staged`), `reason`, `size`, `sha256` (of the staged bytes) |
| `inputs` | id | `id`, `status` (`complete`, `complete_with_skips`, `failed`, `not_run`), `reason`, `input_peak`, `dependency_peak`, `evictions`, `ms`, `skips` (a count), `unresolved_scripts` |
| `skips` | input, then object order | `input`, `path_id`, `class`, `reason`, `detail`, `cut` |
| `diagnostics` | kind, then input | `kind` (`same_named_cab`, `undeclared_dependency`, `nested_container`), `input`, `bundle`, `name`, `cut` |
| `outputs` | key | `key`, `kind` (`texture`, `sprite`, `atlas_sprite`, `atlas_json`), `unit` (its unit key, or null), `input`, `path_id`, `md5`, `visible_hash`, `mode`, `size`, `status` (`written`, `present`, `not_written`, `withheld`, `failed`, `unwritable_key`), `reason` |
| `collisions` | key, then input | one line per candidate of a key with more than one: `key`, `input`, `md5`, `visible_hash`, `winner` (boolean), `rule` (`prefer_input`, `prefer_md5`, `first_input`, `unit`, `unit_conflict`, `withheld`), `flip`, `pixels_differ` (against the winner) |
| `archives` | key | `key`, `status` (`written`, `present`, `not_written`, `empty`, `failed`, `withheld`), `members`, `left_out`, `md5`, `etag`, `size` |
| `archive_members` | archive, then member order | `archive`, `path`, `key`, `input`, `md5`, `included`, `reason` (when left out) |

A reason is a code of section 9.3. A string taken from the bytes is cut to its first 1,024 bytes, at a code point boundary, and its row's `cut` is then true. A row holds at most four strings of at most 1,024 bytes each (an id, a key or path, a cut string), each escaped to at most 6 bytes per byte, so every line is at most 32 KiB (`batch.md` 2.8).

### 7.4 The manifest at each phase

The finisher writes it (`batch.md` 2.11), according to the phase the run reached:

| the finish starts | the head | the row files |
|---|---|---|
| before the `running` write was sent | none; the `exit` line's `status` is null | none |
| with the `running` write in flight | read back first: as below if it landed, else none | as the head |
| in phase 3, 4, 5 or 6 | `incomplete`, with `phase` and `stop_reason` | `bundles` (unstaged ones `not_staged`); `inputs` (committed ones with their records, the rest `not_run`); the committed inputs' `skips` and `diagnostics`. `outputs`, `collisions`, `archives` and `archive_members` are null |
| in phase 7, on a stop | `incomplete` | every file. Each winner's and archive's status is read from the outcomes files (section 6.1): `written` where the write was acknowledged, `present` where resume found the bytes, `failed` (`publish_failed`) where its tries ran out, and `not_written` for every other |
| after phase 7 | `complete`, `complete_with_skips` or `incomplete`, by section 9.2 | every file, statuses read as above |
| superseded at the final write | nothing more; the stored head is the newer attempt's | none from this attempt's finish |

---

## 8. Zip bytes

An archive is a function of its members' bytes, paths and order:
- every member stored (method 0), in member order (section 4.7), CRC-32 from the spool's record;
- local and central headers: version needed 20 (45 for a zip64 member), version made by 20, general-purpose flags `0x0800` (UTF-8 names) and no data descriptor, DOS time `0x0000` and date `0x0021` (1980-01-01 00:00), no extra field but zip64's, internal and external attributes 0, no comment;
- **zip64** only where the classic fields overflow. A member whose size or local-header offset is at least `0xFFFFFFFF` has those fields set to `0xFFFFFFFF` and a zip64 extra field (id `0x0001`) holding, in this order, its uncompressed and compressed sizes (local and central headers) and its offset (central header only, when it overflowed). An archive whose member count is at least `0xFFFF`, or whose central directory's size or offset is at least `0xFFFFFFFF`, ends with a zip64 end of central directory record (version made by and needed 45, no extensible data), its locator (disk 0, total disks 1), and the classic end record with each overflowed field set to its maximum;
- the central directory is spooled while the members stream and written after them (`batch.md` 2.9).

So the archive's MD5, and its multipart ETag at 16 MiB parts, are the same on every run with the same members (E43).

---

## 9. Exit codes and reasons

### 9.1 Codes

| code | status written | meaning | the caller's usual action |
|---|---|---|---|
| 0 | `complete` | every input extracted, every winner published, no skip | done |
| 3 | `complete_with_skips` | object-level skips for data reasons only; every winner published | done; review the skips |
| 4 | `incomplete` | an input failed for a data or transport reason, or an output, archive, row file or the head failed to publish | fix the input, or retry with `attempt + 1` (a transport reason may pass on a retry as is) |
| 5 | `incomplete` | a budget decided an outcome: an input's or a dependency's memory, bytes or work limit, an attempt's budget or eviction cap, the spool | raise the budget the manifest names |
| 6 | `incomplete`, or none | stopped: the deadline, a signal, the parent's death, stdin's EOF | resume with `attempt + 1` |
| 7 | none by this attempt | `superseded`: a later attempt of this `run_id` owns the head | do not retry |
| 69 | none | could not start: section 9.3's preflight reasons | fix the environment, retry |
| 70 | `incomplete` if possible | internal error: a panic outside a decoder, a leaked charge (a refusal of phase 6's working set among them), an unclassifiable error, `ContainerDropped` | report a bug |
| 74 | `incomplete` if possible | host failure during the run: an I/O error on the spool, a staged file or a local source being copied; an allocation the allocator refused | fix the host, retry with `attempt + 1` |
| 78 | none | a bad spec or command line, an unknown `spec_version` included | fix the spec; never retry as is |

- **Precedence**, when several apply: 78, 69, 70, 74, 7, 6, 5, 4, 3, 0. A stop's code is its reason's (section 9.3); a more severe stop that comes before the finisher takes the reason replaces it (`batch.md` 2.11).
- Codes 1, 2 and 64 to 68 are never used. A process killed by a signal shows `128 + N`. A second stop signal during a stop exits at once with the stop's code.
- `unityrs batch --check JOB.json` exits 0 for a valid spec and 78 otherwise. `unityrs --version` exits 0.

### 9.2 From outcomes to codes, at a normal end

| outcome | code |
|---|---|
| an object skipped for a budget: reason `budget(input)`, `budget(dependency)`, or `limit` of kind `input_memory`, `input_bytes` or `decode_work` | 5 |
| an object skipped for any other reason | 3 |
| an input failed for a budget, by the same reasons or `limit(attempt_evictions, ...)`, or by `dependency_failed(<id>, <reason>)` whose inner reason is one of them | 5 |
| an input failed for any other reason (a transport reason, `integrity_mismatch`, `dependency_failed` with any other inner reason, `not_a_bundle`, a container or entry error, a depth or count limit) | 4 |
| an output, archive, row file or the head that failed to publish (`publish_failed`) | 4 |
| a key withheld (E61) | none of its own: the failed input sets the code |

A depth or count limit is a data reason: no budget raises it.

### 9.3 Reasons

A reason is `name` or `name(argument, ...)`. A `limit`'s arguments are the core's `LimitKind` in snake case (or `attempt_evictions`) and its configured value in decimal, never the amount asked for. `unsupported_format`'s argument is UnityPy 1.25.2's `TextureFormat` member name, or the decimal value when it has none. Version 1's names, by where they appear:

| where | names |
|---|---|
| a bundle or an input, transport (exit 4) | `download_failed`, `redirect_refused`, `source_missing` |
| a bundle or an input, data (exit 4) | `integrity_mismatch`, `not_a_bundle`, `container_error`, `entry_error`, `dependency_failed(<id>, <reason>)`, `limit(<kind>, <value>)` |
| a budget (exit 5) | `budget(input)`, `budget(dependency)`, `limit(input_memory \| input_bytes \| decode_work \| attempt_evictions, <value>)` |
| an object's skip | the budget names, a data `limit`, `outside_view(<cab>)`, `bad_pointer`, `decode_error`, `decoder_panic`, `unsupported_format(<name>)`, `unsupported_platform(switch \| xbox360)`, `empty_image`, `duplicate_name`, `duplicate_key`, `unwritable_key(<rule>)`, `empty_rect`, `unnamed_atlas`, `reference_atlas`, `sheet_not_found`, `ngui_layout_unsupported`, `no_typetree`, `json_unwritable` |
| an output's or archive's status | `withheld` (E61), `unit_conflict`, `publish_incomplete`, `publish_failed`, `unwritable_key(<rule>)` |
| a member left out | `duplicate_member`, `unwritable_path(<rule>)`, `prefix_conflict` |
| a stop | `deadline`, `sigterm`, `sigint`, `sighup`, `parent_died`, `stdin_eof` (6); `superseded` (7); `spool_full` (5); `host_io`, `host_memory` (74); `internal` (70) |
| preflight (exit 69) | `spool_not_dedicated`, `spool_overlaps_sink`, `spool_locked`, `spool_no_space`, `output_locked`, `manifest_unreadable`, `sink_unreachable`, `credentials_missing`, `rlimit_too_low`, `overcommit_strict`, `previous_attempt_live(<fence time>)`, `thread_spawn_failed` |
| validation (exit 78) | `bad_spec`, with the field's JSON pointer and the rule in its detail |
| a warning (the head's `warnings`) | `resume_list_failed`, `etag_not_md5`, `stderr_dropped`, `abort_failed` |

---

## 10. The progress stream

NDJSON on stdout (E4), version `p` = the `spec_version` the run reads (for a spec refused at validation, the binary's highest). Each line is one JSON object and a whole snapshot, never a delta: a reader needs only the last line it read. Every line has every field below; a version adds fields only with a new `p`.

| field | type | meaning |
|---|---|---|
| `p` | integer | the progress version |
| `type` | string | `snapshot`, or `exit` for the last line |
| `seq` | integer | 0 on the first line, then +1 per line, gapless |
| `ms` | integer | ms since the process started (`CLOCK_BOOTTIME`) |
| `phase` | string | `validate`, `preflight`, `start`, `stage`, `extract`, `winners`, `publish` or `finish`: `batch.md` 2.1's phases 1 to 8 in order |
| `inputs` | object | integers: `total`, `complete`, `failed`, `running`, `queued`, `not_run` |
| `transfers` | object | integers: `stages_active`, `stages_done`, `stages_failed`, `uploads_active`, `uploads_done`, `uploads_failed` |
| `memory` | object | bytes, integers: `inflight_limit` (`I`), `slots_reserved`, `attempts_used` (the attempts' budgets' summed use), `phase_used`, `rss` |
| `spool` | object | bytes, integers: `used`, `size` |
| `outputs` | object | integers: `candidates`, `winners`, `published`, `present`, `failed`; `winners` is 0 before phase 6 |
| `status` | string or null | on the `exit` line, the status the final manifest holds, or null when none was written; null on a snapshot |
| `code` | integer or null | on the `exit` line, the exit code (section 9); null on a snapshot |
| `stop_reason` | string or null | on the `exit` line, a stop reason of 9.3, or null; null on a snapshot |

- A line is written at least every 2 s from a one-slot mailbox (`extract.md` item 5). No field holds a URL, a header value or a credential (E42).
- A spec refused at validation, and `--check` either way, write one line: the `exit` line.
- `unityrs --version` prints one line, not NDJSON: `unityrs <crate version> spec <highest spec_version>`.
