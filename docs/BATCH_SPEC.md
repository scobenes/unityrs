# The batch job contract, version 1

**Status: draft, design round 2 (2026-11-09), not reviewed.** This file is the exact contract of `unityrs batch`: the job spec, keys and templates, identity and order, the NGUI cutter, the spool, the manifest, the zip bytes and the exit codes. `docs/plans/extract.md` states the requirements (E1 onward) and cites this file for their exact form; `docs/plans/batch.md` designs the engine behind them. Where this file and a requirement disagree, the review rounds fix one of them.

**Conventions.**
- *bytes*: an integer count of bytes. *ms*: an integer count of milliseconds.
- *Bytewise order*: the order of the strings' UTF-8 bytes, compared as unsigned bytes, a shorter prefix first.
- *Refused at validation*: the binary exits 78 before any work (section 9).
- `M`, `D` and `I` abbreviate `max_decoded_bytes`, `dep_cache_bytes` and `max_inflight_uncompressed_bytes`.

---

## 1. The job spec

A spec is one JSON object, at most 16 MiB, UTF-8. An unknown field at any level, a duplicate field, a value of the wrong type and a value out of its range are refused at validation. `docs/batch/spec-v1.schema.json` (JSON Schema 2020-12) is generated from these tables; its agreement test is `extract.md` section 3, item 4's.

### 1.1 Top level

| field | type | required | default | allowed values and meaning |
|---|---|---|---|---|
| `spec_version` | integer | yes | | `1` |
| `run_id` | string | yes | | 1-64 bytes of `[A-Za-z0-9._-]`, not `.` or `..`. Names one logical run; its attempts share it (section 7) |
| `attempt` | integer | yes | | 1 to 2^31-1. The caller raises it for each attempt of one `run_id` |
| `deadline` | string or null | no | `null` | an RFC 3339 date-time with `Z` or an offset; `null` sets none (E45) |
| `shutdown_grace_ms` | integer | no | 20000 | 2,000 to 600,000 (E45) |
| `stop_on_stdin_eof` | boolean | no | `false` | E46 |
| `parent_pid` | integer or null | no | `null` | the caller's pid, checked at start (E46) |
| `threads` | integer | yes | | 1 to 1,024: the most attempts that run at once |
| `download_concurrency` | integer | yes | | 1 to 256: the most transfers at once, downloads and uploads (E8) |
| `max_inflight_uncompressed_bytes` | integer | yes | | `I`: what the attempts in flight may reserve together, and phases 8 and 9's working set (`batch.md` 2.2) |
| `max_decoded_bytes` | integer | yes | | `M`, 1 MiB to 2^40: every meter's `max_input_memory`, and each attempt's Input budget |
| `dep_cache_bytes` | integer | yes | | `D`: each attempt's Dependency budget and its private dependency cache (`batch.md` 2.2) |
| `max_view_bundles` | integer | no | 1024 | 1 to 65,536: the most bundles one input's view may hold |
| `spool` | Spool | yes | | section 6 |
| `http` | Http | no | every default | section 1.4 |
| `inputs` | array of Bundle | yes | | 1 to 100,000 items |
| `dependencies` | array of Bundle | no | `[]` | bundles that are read but not extracted |
| `url_rule` | UrlRule or null | no | `null` | E9 |
| `outputs` | Outputs | yes | | section 1.5 |
| `prefer` | object | no | `{}` | output key → Prefer (E31) |
| `publish_incomplete` | boolean | no | `true` | E61 |
| `resume` | boolean | no | `false` | E43 |
| `sink` | Sink | yes | | section 1.6 |
| `archives` | array of Archive | no | `[]` | section 1.7 |
| `manifest_fencing` | string | no | `"conditional"` | `"conditional"` or `"none"` (section 7.2) |

**Cross-field rules**, each refused at validation:
- `M <= D` and `M + D <= I`, and `I >= 64 MiB` (`batch.md` 2.2).
- Every `id` is unique across `inputs` and `dependencies`.
- Every id in a `deps` list names a bundle of the spec, unless `url_rule` is set (it is then fetched by the rule, section 4.1).
- No declared view (section 4.2, without `m_Dependencies`) holds more than `max_view_bundles` bundles.
- `spool.reserve_bytes < spool.size_bytes`.
- Every template parses (section 2.1), and every literal key (`prefer`'s keys, `archives`' keys and member paths, the sink's prefix) passes section 2.3's rules.

### 1.2 Bundle (an item of `inputs` or `dependencies`)

| field | type | required | default | allowed values and meaning |
|---|---|---|---|---|
| `id` | string | yes | | 1-1,024 bytes of UTF-8, no control character. The bundle's identity (section 4.1) |
| `path` | string | one of `path`, `url` | | an absolute path of a local regular file |
| `url` | string | one of `path`, `url` | | an `http` or `https` URL with no userinfo |
| `headers` | object | no | `{}` | only with `url`. Header name → a string, or `{"env": NAME}` read from the environment at preflight |
| `size` | integer | no | | the expected byte count |
| `sha256` | string | no | | 64 lowercase hex digits: the expected digest |
| `deps` | array of string | no | `[]` | the ids this bundle needs (E11). A dependency may list its own |

A `size` or `sha256` that does not match fails the bundle (`integrity_mismatch`): a download after its last byte, a local file at preflight.

### 1.3 UrlRule

| field | type | required | default | meaning |
|---|---|---|---|---|
| `template` | string | yes | | an `http` or `https` URL holding `{name}` exactly once |
| `headers` | object | no | `{}` | as a Bundle's |

`{name}` is replaced by the name with every byte outside `A-Za-z0-9-._~/` percent-encoded in upper case. The fetched bundle's id is the name, unchanged.

### 1.4 Http

| field | type | default | allowed values and meaning |
|---|---|---|---|
| `timeout_ms` | integer | 60000 | 1,000 to 600,000: one request, from its start to its last byte |
| `connect_timeout_ms` | integer | 10000 | 100 to 60,000 |
| `attempts` | integer | 3 | 1 to 10: tries per download, the first included |
| `backoff_initial_ms` | integer | 500 | 0 to 60,000 |
| `backoff_max_ms` | integer | 30000 | at least `backoff_initial_ms`, at most 600,000 |
| `retry_on` | array of string | all five | a subset of `"connect"`, `"timeout"`, `"status_5xx"`, `"status_429"`, `"truncated"` |
| `redirects` | string | `"same_origin"` | `"same_origin"` or `"none"` |

- The k-th retry waits `min(backoff_max_ms, backoff_initial_ms x 2^(k-1))`, with no jitter. A `Retry-After` header (seconds, or an HTTP date) raises that wait to its value, still capped at `backoff_max_ms`.
- No retry starts when its wait plus `timeout_ms` would pass the stop cutoff (the deadline less the grace, E7). A request in flight at a stop is cancelled (`batch.md` 2.11).
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
| `script_classes` | array of string | no | `["NGUIAtlas", "UIAtlas"]` | the `m_ClassName`s that make a MonoBehaviour an atlas (section 5) |
| `skip_name_suffixes` | array of string | no | `[]` | an atlas whose `m_Name` ends with one of them is skipped as a reference atlas (E22) |

**Prefer:** `{"input": id or null, "md5": 32 lowercase hex digits or null}`, at least one not null (E31).

### 1.6 Sink

One sink per job. Outputs are written to it as loose objects, and each archive (section 1.7) is written to it as one more object.

| `kind` | fields |
|---|---|
| `"directory"` | `root`: an absolute path of an existing directory |
| `"object_store"` | `bucket` (string); `prefix` (string, default `""`, else ending in `/`); `region` (string); `endpoint` (URL or null, default null: AWS); `path_style` (boolean, default `false`); `timeout_ms` (integer, default 30000, 1,000 to 300,000, per request); `attempts` (integer, default 3, 1 to 10) |

Every key is written at the sink's root (directory) or after its prefix (object store). Credentials come from the environment only (E37).

### 1.7 Archive

| field | type | required | meaning |
|---|---|---|---|
| `key` | string | yes | the archive's key in the sink |
| `members` | array of `{"path": string, "key": string}` | yes | 1 to 1,000,000 members, in archive order. `path` is the member's path, unique in the archive; `key` names an output |

An output can be both a loose object and a member. A member whose key has no published winner (withheld, failed or never produced) is left out, and the manifest lists it (section 7.3). An archive left with no member is not written (`empty`).

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
| `type` | the object's class name: `Texture2D` or `Sprite` | `textures`, `sprites` |
| `name`, `safe_name` | the object's `m_Name` | `textures`, `sprites` |
| `path_id` | the object's path id, signed decimal, no leading zero | every template; an atlas's is the MonoBehaviour's |
| `atlas`, `safe_atlas` | the atlas MonoBehaviour's `m_Name` | `sprite_key`, `json_key` |
| `sprite`, `safe_sprite` | the `mSprites` element's `name` | `sprite_key` |

A plain variable is written verbatim: a name's stored bytes. Its `safe_` form is `safe()` of the same value (2.2). There is no `{container}` variable in version 1 (E27).

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
| `prefix_conflict` | it is also the parent directory of another winner's key (section 4.6) |

A literal key that breaks a rule is refused at validation. An expanded key that does is the output's object-level skip, `unwritable_key(rule)`. The rules are the same for every sink and for zip member paths, so a key is writable in one sink exactly when it is writable in all of them.

---

## 3. Selection

What one input exports (E66):
- **Textures**, when `outputs.textures` is set: every Texture2D object of the input's own SerializedFiles. A sheet section 5 cuts is exported too, as a member of its atlas's unit (E26).
- **Sprites**, when `outputs.sprites` is set: every Sprite object of the input's own SerializedFiles. The sheet a sprite renders from may be in a dependency (2.6 of `batch.md`).
- **Atlases**, when `outputs.atlases` is set: section 5.

A texture is exported only in the five formats of E15 with no platform swizzle or byte swap. Any other is an object-level skip naming the format (`unsupported_format(<name>)`), a Switch-swizzled texture (`unsupported_platform(switch)`) or an Xbox 360 texture (`unsupported_platform(xbox360)`) among them, until their phase. A texture with a side of 0 is skipped (`empty_image`), as UnityPy's extractor skips it.

---

## 4. Identity and order

### 4.1 Bundles

- A bundle's identity is its spec `id`. A bundle found by discovery has the name `m_Dependencies` gives it as its id, unchanged: no case folding, no path or variant rule.
- A discovered name equal to a declared id is that bundle.
- Wherever bundles are ordered, it is by bytewise order of their ids.

### 4.2 Views

`view(X)` is the smallest set that holds `X`, the `deps` of every bundle it holds, and, when `url_rule` is set, the names in X's own AssetBundle object's `m_Dependencies`. A dependency's own `m_Dependencies` is never read.

### 4.3 Resolution inside an attempt

- The attempt's private Environment registers `view(X)` less `X` in bytewise order of id, then `X` last (`batch.md` 2.6).
- **A pointer** is dereferenced by the core's `Environment::deref` from the file that holds it. The reading file's own bundle answers first (its siblings, through `Parent`; Tier A). Then the registry answers with the registration of greatest rank: `X` when it holds the CAB, else the greatest id among the view's bundles that hold it.
- **A resource** (`.resS`) read takes UnityPy's candidates in order. Each is looked for first in the reading file's own container chain, and only when none is found there, through `Environment::resource` on the private Environment (`batch.md` 2.6).
- A CAB no bundle of the view holds is the reading object's skip, `outside_view(<cab>)`.

### 4.4 Objects and candidates

- **Object order** in a bundle: its directory's entries in directory order, the SerializedFiles among them, and in each SerializedFile its object table's order.
- **One input's names** (E29). Of two objects of one class and one `m_Name`, the first in object order takes the name, even when its export then fails. The later one is skipped, `duplicate_name`.
- **One input's keys.** Of two outputs of one input with one key, the first in (kind, object order) is the candidate, kinds ordered `textures`, `sprites`, `atlases`; the later one is skipped, `duplicate_key`. Inside one atlas, of two sprites with one key, the later one wins (E30).
- **Sprite groups.** Sprites are grouped by their sheet texture (bundle id, path id), and the groups run in the order of each group's first sprite.

### 4.5 Winners across inputs

E31 to E33 and E61 as written, with these orders:
- "first by bytewise input id" everywhere;
- E31 rule 1 applies when the preferred input has a candidate for the key;
- E32: a unit's **unit key** is its sheet's texture key when `outputs.textures` is set, else its JSON key, and its `prefer` entry is that key's. Units are decided first, in bytewise order of unit key, then input id, then object order; a unit's member keys are reserved before plain candidates are decided.

### 4.6 Prefix conflicts

After the winners are decided, a winner's key `K` that is also a parent directory of another winner's key (some key starts with `K/`) loses to them: `K` is `unwritable_key(prefix_conflict)`, in every sink. The check runs on the sorted keys with each key's ancestors added, so its result does not depend on the order of publishing (`batch.md` 2.9).

---

## 5. The NGUI cutter

**Recognition.** A MonoBehaviour of the input's own SerializedFiles is an atlas when:
1. its `m_Script` dereferences (a read that may cross into the view; E21) to a MonoScript whose `m_ClassName` is in `script_classes`; and
2. its typetree has a field `mSprites` that is an array.

A MonoBehaviour that meets 1 but not 2 is skipped, `ngui_layout_unsupported` (NGUI 2's `sprites` layout is not in version 1). One with no typetree is skipped, `no_typetree`. One whose `m_Script` cannot be read is not an atlas, and the input's record counts it (`unresolved_scripts`).

**Reference atlases.** An atlas whose `m_Name` ends with a string of `skip_name_suffixes` is skipped, `reference_atlas`. So is an atlas with an empty `m_Name`.

**The sheet** (E21, scope `own_files_by_name`; `follow_pptr` is not in version 1).
1. The material's main-texture name: dereference the atlas's `material`, then, in its `m_SavedProperties.m_TexEnvs` (a list of pairs), the first pair whose first is `_MainTex`; dereference that pair's `m_Texture` and read its `m_Name`. Any error on this path gives no name, and the next step runs.
2. Among the input's own Texture2D objects in object order, the first whose `m_Name` equals that name exactly (bytes, case-sensitive). When there is none, or no name, the first whose `m_Name` equals the atlas's `m_Name`.
3. No match skips the atlas, `sheet_not_found`.

The sheet is decoded as a texture (E15), into the image UnityPy's `Texture2D.image` gives: rows top to bottom.

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

"As read" means UnityPy 1.25.2's `read_typetree()` value, written as `json.dumps` writes it: a typetree dict as an object in its field order, a pair as a 2-element array, an f32 widened exactly to the double, a string as UnityPy decodes it (`surrogateescape`, so an invalid byte is written `\udcXX`). A value `json.dumps` cannot write (bytes) skips the JSON, `json_unwritable`, and keeps the crops (E25).

**Collisions** (E26): an atlas, its crops, its JSON and, when `outputs.textures` is set, its sheet's texture output are one unit (E32), decided by section 4.5.

---

## 6. The spool

`spool.dir` (absolute, an existing directory on a local filesystem), `spool.size_bytes` (the cap on what the run writes there) and `spool.reserve_bytes` (the free space every claim leaves).

| path | holds |
|---|---|
| `<dir>/lock` | the spool's lock: `flock(LOCK_EX \| LOCK_NB)` for the process's life (E63) |
| `<dir>/<run_id>/journal` | multipart upload ids, one per line: `bucket`, `key`, `upload_id`, appended and `fdatasync`ed after the create, removed after the complete or abort (E64) |
| `<dir>/<run_id>/in/` | downloads |
| `<dir>/<run_id>/attempts/<input>.<round>/` | one attempt's outputs and `records.ndjson` |
| `<dir>/<run_id>/done/<input>/` | a committed attempt |
| `<dir>/<run_id>/winners/` | phase 8's sorted runs and its winners file |
| `<dir>/<run_id>/manifest/` | the manifest's row files before they are written to the sink |

`<input>` is the input's position in `inputs`, in decimal, so an id never becomes a path.

**Lifecycle.**
- **At start**, after the lock: the ids in `<dir>/<run_id>/journal` are aborted (E64). Then every other entry under `<dir>`, other run ids' directories included, is deleted. The lock proves no live run uses them; an upload a deleted journal names is left to the bucket's lifecycle rule (E64).
- **After the final manifest**, `<dir>/<run_id>/` is deleted.
- **A stop** deletes nothing: the next attempt's start clears it.

Nothing in the spool is reused between attempts: every attempt downloads and decodes again (E44).

---

## 7. The manifest

### 7.1 Location

| sink | the head | the row files |
|---|---|---|
| directory | `<root>/_unityrs/<run_id>/manifest.json` | `<root>/_unityrs/<run_id>/<attempt>/*.ndjson` |
| object store | `<prefix>_unityrs/<run_id>/manifest.json` | `<prefix>_unityrs/<run_id>/<attempt>/*.ndjson` |

No output key may start with `_unityrs/` (2.3). The row files hold the attempt number, so an earlier attempt never overwrites a later one's rows. The head is at most 64 KiB.

### 7.2 Fencing

- **Preflight** reads the head. A head over 1 MiB, or one that is not a manifest, exits 69 (`manifest_unreadable`). A head of this `run_id` whose `attempt` is at or above this one's exits 6 (`superseded`), and nothing is written.
- **Object store, `"conditional"`.** The store client is configured with `S3ConditionalPut::ETagMatch`. The first write is `If-None-Match: *` when no head exists, else `If-Match` on the ETag preflight read. Each later write is `If-Match` on the ETag of this attempt's previous write.
- **A failed precondition** (412) is read back with a GET. A head with this `attempt` and this write's `write_seq` is this write, whose response was lost: success. Anything else stops the attempt at once, exit 6 (`superseded`).
- **Object store, `"none"`.** Plain PUTs; the caller runs one attempt of a `run_id` at a time.
- **Directory.** `<root>/_unityrs/<run_id>/lock`, `flock(LOCK_EX | LOCK_NB)` held for the process's life: busy exits 69 (`output_locked`). Each head write is a temp file, `fsync`, a rename over the head, and an `fsync` of the directory.

The head is the record of the run. A key the stored head does not list as `written` or `present` is not part of the run's output, whatever the store holds (a PUT that landed after a stop, a key an earlier attempt wrote).

Output PUTs are not fenced. Two attempts of one `run_id` write the same bytes to a key when their bundles are the same, which `size` and `sha256` pin (1.2). Without the pins, a superseded attempt's late PUT can overwrite a key with other bytes; the head still says which MD5 the run claims, and a resumed attempt re-checks every key's MD5 (E43).

### 7.3 Fields

**The head:**

| field | type | meaning |
|---|---|---|
| `manifest_version` | integer | `1` |
| `run_id`, `attempt` | string, integer | from the spec |
| `write_seq` | integer | 0 for the `running` write, then +1 per write |
| `status` | string | `running`, `complete`, `complete_with_skips` or `incomplete` |
| `exit_code` | integer or null | null while `running` |
| `stop_reason` | string or null | a reason of section 9.3, or null |
| `binary_version`, `spec_version` | string, integer | |
| `started_at`, `deadline`, `ended_at` | string or null | RFC 3339, UTC |
| `counts` | object | integers: `inputs`, `inputs_complete`, `inputs_with_skips`, `inputs_failed`, `inputs_not_run`, `outputs_written`, `outputs_present`, `outputs_withheld`, `outputs_failed`, `outputs_unwritable`, `skips`, `collisions`, `flips`, `archives_written` |
| `run` | object | E40's run-level figures: `same_named_cabs`, `undeclared_dependencies`, `nested_containers` (counts; the rows hold the names), `opens`, `evictions`, `reopens`, `input_peak`, `dependency_peak`, `spool_peak`, `vm_hwm`, `memory_bound`, the budgets as read |
| `rows` | object | `outputs`, `inputs`, `skips`, `collisions`, `archives`, `archive_members`, `diagnostics`: each row file's key relative to the head's directory, or null |

**The row files** (NDJSON, one object per line, each sorted as stated):

| file | sorted by | fields |
|---|---|---|
| `outputs` | key | `key`, `kind` (`texture`, `sprite`, `atlas_sprite`, `atlas_json`), `input`, `path_id`, `md5`, `visible_hash`, `mode`, `size`, `status` (`written`, `present`, `withheld`, `failed`, `unwritable_key`), `reason` |
| `inputs` | id | `id`, `status` (`complete`, `complete_with_skips`, `failed`, `not_run`), `reason`, `round`, `input_peak`, `dependency_peak`, `ms`, `skips` (a count), `unresolved_scripts` |
| `skips` | input, then object order | `input`, `path_id`, `class`, `reason`, `detail` |
| `collisions` | key | `key`, `candidates` (`[{input, md5, visible_hash}]`), `winner` (input or null), `rule` (`prefer_input`, `prefer_md5`, `first_input`, `unit`, `unit_conflict`, `withheld`), `flip`, `pixels_differ` |
| `archives` | key | `key`, `status` (`written`, `empty`, `failed`, `withheld`), `members`, `left_out` |
| `archive_members` | archive, then member order | `archive`, `path`, `key`, `input`, `md5`, `included` |
| `diagnostics` | kind, then input | `kind` (`same_named_cab`, `undeclared_dependency`, `nested_container`), `input`, `bundle`, `name` |

A reason is a code of section 9.3. A limit refusal's reason names the limit and its value as configured, never the amount it was asked for.

---

## 8. Zip bytes

An archive is a function of its members' bytes, paths and order:
- every member stored (method 0), in `members` order, CRC-32 from the spool's record;
- local and central headers: version needed 20 (45 for a zip64 member), version made by 20, general-purpose flags `0x0800` (UTF-8 names) and no data descriptor, DOS time `0x0000` and date `0x0021` (1980-01-01 00:00), no extra field but zip64's, internal and external attributes 0, no comment;
- zip64 records only for a member, an offset or a count past the classic format's limits; otherwise none;
- the central directory is spooled while the members stream and written after them (`batch.md` 5.2).

So the archive's MD5, and its multipart ETag at 16 MiB parts, are the same on every run with the same members (E43).

---

## 9. Exit codes and reasons

### 9.1 Codes

| code | status written | meaning | the caller's usual action |
|---|---|---|---|
| 0 | `complete` | every input extracted, every winner published, no skip | done |
| 3 | `complete_with_skips` | object-level skips for data reasons only; every winner published | done; review the skips |
| 4 | `incomplete` | an input failed for a data reason, or an output or archive failed to publish | fix the input, or retry with `attempt + 1` |
| 5 | `incomplete` | a budget decided an outcome: an input's or a dependency's memory or work limit, an attempt's budget, the spool, phase 8's working set | raise the budget the manifest names |
| 6 | `incomplete`, or none | stopped: the deadline, a signal, the parent's death, stdin's EOF; or `superseded` | resume with `attempt + 1`; when `superseded`, do not retry |
| 69 | none | could not start: the spool or output root locked or unusable, a sink unreachable, credentials missing, the manifest unreadable, a resource limit below the bound (E63) | fix the environment, retry |
| 70 | `incomplete` if possible | internal error: a panic outside a decoder, a leaked charge, an unclassifiable error | report a bug |
| 74 | `incomplete` if possible | host failure during the run: an I/O error on the spool or on a bundle's file | fix the host, retry with `attempt + 1` |
| 78 | none | a bad spec or command line, an unknown `spec_version` included | fix the spec; never retry as is |

- **Precedence**, when several apply: 78, 69, 70, 74, 6, 5, 4, 3, 0.
- Codes 1, 2 and 64 to 68 are never used. A process killed by a signal shows `128 + N`. A second stop signal during a stop exits at once with 6.
- `unityrs batch --check` exits 0 for a valid spec and 78 otherwise. `unityrs --version` exits 0.
- A spec refused at validation writes one progress line, the `exit` line, with `p` the binary's highest version.

### 9.2 From outcomes to codes

| outcome | code |
|---|---|
| an object skipped for a budget: reason `budget(input)`, `budget(dependency)`, `limit(input_memory)` or `limit(decode_work)` | 5 |
| an object skipped for any other reason | 3 |
| an input failed for a budget, by the same four reasons | 5 |
| an input failed for any other reason (a download, `integrity_mismatch`, `source_changed`, `dependency_failed`, `not_a_bundle`, a container or entry error, a depth or count limit) | 4 |
| an output, archive or row file that failed to publish | 4 |
| a key withheld (E61) | none of its own: the failed input sets the code |
| a spool claim refused or below the reserve; phase 8's working set refused | 5 |

A depth or count limit is a data reason: no budget raises it.

### 9.3 Reasons

A reason is `name` or `name(argument)`. Version 1's names: `download_failed`, `redirect_refused`, `integrity_mismatch`, `source_changed`, `dependency_failed`, `not_a_bundle`, `container_error`, `entry_error`, `view_too_large`, `budget`, `limit`, `outside_view`, `decode_error`, `decoder_panic`, `unsupported_format`, `unsupported_platform`, `empty_image`, `duplicate_name`, `duplicate_key`, `unwritable_key`, `empty_rect`, `reference_atlas`, `sheet_not_found`, `ngui_layout_unsupported`, `no_typetree`, `json_unwritable`, `withheld`, `publish_failed`, `spool_full`, `deadline`, `sigterm`, `sigint`, `sighup`, `parent_died`, `stdin_eof`, `superseded`, `host_io`, `internal`. A limit's argument is the core's `LimitKind` in snake case.
