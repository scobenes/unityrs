# The batch job contract, version 1

**Status: draft, design round 17 (2026-11-11), not reviewed.** This file is the exact contract of `unityrs batch`: the job spec, keys and templates, identity and order, the NGUI cutter, the spool, the manifest, the zip bytes, the exit codes and the progress stream. `docs/plans/extract.md` states the requirements (E1 onward) and cites this file for their exact form; `docs/plans/batch.md` designs the engine behind them. Where this file and a requirement disagree, the review rounds fix one of them. Design round 5 states the store's per-try classification (1.6), head fencing in `object_store`'s terms (7.2) and every manifest field's values and type (7.3). Design round 6 retries a lower attempt's head at the final write too (7.2), gives the finisher's requests one try each after a stop (1.6), and counts every file an attempt spools in its export work (1.1). Design round 7 states what selection leaves out (section 3), a head's read-back that finds no head or one it cannot read (1.6, 7.2), and the manifest after a 69 at the `running` write (7.4). Design round 8 orders selection by `BundleFile::files()` (section 3, 4.4), states what a 69 or 7 leaves stored and what a supersession at the final write wrote (7.4), and names an object's SerializedFile in its rows (7.3). Design round 9 cites the core's Lazy differences and the gap below depth 1 (4.3), names `objects()`'s order (4.4), states 7.4's first two rows by the `running` tries' answers, and routes a full filesystem to provisioning (9.1). Design round 10 defines a dropped container row by its registry key (4.3), gives `stop_detail` its causes and `run` the spool's budgets (7.3), and compares `same_named_cab` by key (7.3). Design round 11 states the own-chain resource lookup as one procedure (4.3), fixes `same_named_cab`'s `name` (7.3), and reports each input's export work (7.3). Design round 12 names a cross-bundle drop that changes no record (4.3), picks `same_named_cab`'s bundle among top-level holders (7.3), and opens the journal by path, removing a line only after a successful complete or abort (6.1). Design round 13 caps every string that reaches a request (1.2, 1.6), raises the spec's cap to 64 MiB and records its digest (7.3), and bounds the `exit` line and names its writer (section 10). Design round 14 states the caller's rule for stdin (1.1), takes `JOB.json` only as a regular file (section 1), caps URLs as serialized (1.2, 1.4), adds a `detail` to the `bundles` and `inputs` rows and a compatibility rule to the manifest (7.3), and gives a store's 69 a fixed-vocabulary detail (section 10). Design round 15 classifies a download's tries (1.4), states the trust rule (1.2), the readers' rules for a `running` head and for new values (7.3, 7.4), and the detail of every row and `exit` line (7.3, 9.3, section 10). Design round 16 keeps the most severe run failure with its input (7.3), classes a malformed response and a 408 (1.4), names `limit`'s kinds and the remaining preflight reasons (9.3), and fixes `bad_spec`'s form and order (section 10). Design round 17 gives the progress stream its own version (section 10), leaves the engine's own writes out of the timers (1.4), names a store body's failure (1.6), and widens the spool's and the root's reasons (6.2, 7.2). The manifest is deterministic within unityrs; byte identity with another implementation is not a goal.

**Conventions.**
- *bytes*: an integer count of bytes. *ms*: an integer count of milliseconds.
- *Bytewise order*: the order of the strings' UTF-8 bytes, compared as unsigned bytes, a shorter prefix first.
- *Refused at validation*: the binary exits 78 before any work (section 9).
- `M`, `D` and `I` abbreviate `max_decoded_bytes`, `dep_cache_bytes` and `max_inflight_uncompressed_bytes`.
- Every integer field is at most 2^53 - 1, so every JSON parser reads it exactly. The engine computes in `u64` with checked arithmetic, and the ranges below keep every sum it forms below 2^63.

---

## 1. The job spec

A spec is one JSON object, at most 64 MiB (an average of about 670 bytes per input at 100,000 inputs), UTF-8, in a regular file: a `JOB.json` of another type, or a longer one, is refused at validation (`bad_spec`). An unknown field at any level, a duplicate member in any object, a value of the wrong type and a value out of its range are refused at validation. `docs/batch/spec-v1.schema.json` (JSON Schema 2020-12) is written by hand from these tables; it covers each field's type and range, a byte cap only as a code-point bound, and the byte caps and the cross-field rules are the parser's alone (`extract.md` section 3, item 4). `unityrs batch --schema` prints the schema of the binary's highest `spec_version` alone, as one JSON document with no progress line (9.1); an earlier version's is its file above.

### 1.1 Top level

| field | type | required | default | allowed values and meaning |
|---|---|---|---|---|
| `spec_version` | integer | yes | | `1` |
| `run_id` | string | yes | | 1-64 bytes of `[A-Za-z0-9._-]`, not `.` or `..`. Names one logical run; its attempts share it (section 7) |
| `attempt` | integer | yes | | 1 to 2^31-1. The caller raises it for each attempt of one `run_id` |
| `deadline` | string or null | no; yes with an object store sink | `null` | an RFC 3339 date-time with `Z` or an offset; `null` sets none (E45). A deadline whose stop cutoff (the deadline less the grace) has passed at preflight exits 69 (`deadline_too_close`) |
| `shutdown_grace_ms` | integer | no | 20000 | 5,000 to 600,000 (E45) |
| `stop_on_stdin_eof` | boolean | no | `false` | E46. The engine must be stdin's only reader: a caller that shares stdin with another reader does not set it. Stdin must be a pipe, a FIFO, a socket or a terminal, open for reading, or the run exits 69 (`startup_failed`, `batch.md` 2.11) |
| `parent_pid` | integer or null | no | `null` | 2 to 2^22: the pid of the engine's parent process (a wrapper that forks the engine is that parent), checked at start (E46). A caller whose pid is 1, or outside the engine's pid namespace (where the engine's `getppid()` is 0), omits it; the parent-death signal applies either way |
| `threads` | integer | yes | | 1 to 1,024: the most attempts that run at once |
| `download_concurrency` | integer | yes | | 1 to 256: the most stages and transfers at once (E8) |
| `max_inflight_uncompressed_bytes` | integer | yes | | `I`, 64 MiB to 2^50: what the attempts in flight may reserve together, and phases 6 and 7's working set (`batch.md` 2.2) |
| `max_decoded_bytes` | integer | yes | | `M`, 1 MiB to 2^40: every meter's `max_input_memory` and `max_input_bytes`, and each attempt's Input budget |
| `dep_cache_bytes` | integer | yes | | `D`, 1 MiB to 2^40: each attempt's Dependency budget and its private dependency cache (`batch.md` 2.2) |
| `max_view_bundles` | integer | no | 1024 | 1 to 65,536: the most bundles one input's view may hold |
| `max_attempt_evictions` | integer | no | 1024 | 0 to 2^20: the most dependency evictions one attempt may cause (`batch.md` 2.6 step 7); more fails the input, `limit(attempt_evictions, <value>)` |
| `max_export_work` | integer | no | 16 x `M` | 1 MiB to 2^53-1: the most export work one attempt may do, in bytes (`batch.md` 2.6 step 10: the pixels it writes, and the files and member records it spools); more fails the input, `limit(export_work, <value>)`. So one input's claims on the spool total at most this much times `k_blk` (`batch.md` 2.6 step 10) |
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
- No two names of one bundle's `headers` are equal compared ASCII case-insensitively (1.2).
- `http.backoff_max_ms >= http.backoff_initial_ms` (1.4).
- Every id in a `deps` list names a bundle of the spec, and no list repeats an id or holds more than `max_view_bundles` ids.
- No view (section 4.2) holds more than `max_view_bundles` bundles.
- With a directory sink, `spool.dir` and `sink.root`, compared as paths after removing a trailing `/`, neither equal nor contain one another (preflight compares their real paths again, section 6.2).
- An object store's `endpoint` whose host is an IP address has `path_style` true: `<bucket>.<address>` is not a host (1.6).
- A `bucket` with an uppercase letter or `_` has `path_style` true: as a host name it would be lowercased, reaching another bucket, or refused (1.6).
- `deadline` is set when `sink.kind` is `"object_store"`: it bounds when an attempt's process has ended, from which the caller's wait counts (section 7.2).
- Every `prefer` entry whose `input` is not null names an input.
- Every template parses and passes its literal checks (section 2.1), and every literal key (`prefer`'s keys, `archives`' keys) passes section 2.3's rules but `prefix_conflict` and `archive_conflict`, which phase 6 decides. A non-empty sink prefix passes them without its final `/`.
- `archives`' keys are unique, and none is an ancestor of another (`a` of `a/b.zip`). In one archive, each member `kind` appears once.

### 1.2 Bundle (an item of `inputs` or `dependencies`)

| field | type | required | default | allowed values and meaning |
|---|---|---|---|---|
| `id` | string | yes | | 1-1,024 bytes of UTF-8, no control character. The bundle's identity (section 4.1) |
| `path` | string | one of `path`, `url` | | an absolute path of a regular file on a local filesystem (network filesystems are out of scope in version 1) |
| `url` | string | one of `path`, `url` | | an `http` or `https` URL with no userinfo, at most 16 KiB as the `url` crate serializes it, its percent-encoding included. An `https` server's certificate must chain to a root `webpki-roots` compiles in: version 1 reads no host CA store, so a private CA fails each try (`connect`, its detail `tls`, 1.4) |
| `headers` | object | no | `{}` | only with `url`; at most 32 headers, whose names and literal values total at most 64 KiB. Header name → a string of at most 16 KiB, or `{"env": NAME}` read from the environment at preflight, once per variable however many bundles name it, into one shared value: an unset or empty variable exits 69 (`header_env_missing`), one over 16 KiB exits 69 (`header_env_too_long`). `Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, `Range` and `Accept-Encoding` are refused. Names are HTTP tokens compared ASCII case-insensitively, for that list and for duplicates (1.1); any other name, `Authorization` among them, is sent as given. A value's UTF-8 bytes are only tab, space, visible ASCII and 0x80-0xFF: a literal value outside that set is refused at validation, an env value exits 69 (`header_env_invalid`) |
| `size` | integer | no | | 0 to 2^53-1: the expected byte count |
| `sha256` | string | no | | 64 lowercase hex digits: the expected digest |
| `deps` | array of string | no | `[]` | the ids this bundle needs (E11). A dependency may list its own |

Every bundle is staged into the spool before extraction (`batch.md` 2.5), and every read of it reads the staged copy. A `size` or `sha256` that does not match the staged bytes fails the bundle (`integrity_mismatch`). A bundle over `M` bytes, by a stated size or by its body, fails before its bytes are claimed (`limit(input_bytes, <M>)`), since no meter could read it. A local path that does not exist or cannot be opened fails it (`source_missing`), one that `fstat` shows is not a regular file fails it (`source_not_regular`; the open is non-blocking, so a FIFO cannot hold it), and one written while it is copied fails it (`source_changed`). Every bundle in a view must be an AssetBundle file; a member that is not fails its inputs (`dependency_failed(<id>, not_a_bundle)`).

### 1.3 Spool

| field | type | required | allowed values and meaning |
|---|---|---|---|
| `dir` | string | yes | an absolute path of at most 256 bytes, of an existing directory on a local filesystem that is not tmpfs or ramfs, dedicated to unityrs spools (section 6.2) |
| `size_bytes` | integer | yes | 1 MiB to 2^53-1: the cap on what the run writes there |
| `reserve_bytes` | integer | yes | 0 to 2^53-1: the free space, on the spool's filesystem, that every claim leaves: the filesystem's own margin (`batch.md` 5.6) |

### 1.4 Http

| field | type | default | allowed values and meaning |
|---|---|---|---|
| `idle_timeout_ms` | integer | 30000 | 1,000 to 600,000: a try fails (`"timeout"`) when no byte arrives for this long. This timer and the rate floor leave out the time the engine spends writing what arrived to the spool (`batch.md` 2.5) |
| `min_rate_bytes` | integer | 1024 | 1 to 2^30: a try also fails (`"timeout"`) when fewer than this many bytes arrive in a period of `idle_timeout_ms`, the periods counted back to back from the request's send. There is no whole-request timeout, so a download slowed by its neighbours takes longer and fails only below this floor. A staged bundle is at most `M` bytes, so with the floor every try ends |
| `connect_timeout_ms` | integer | 10000 | 100 to 60,000: every client's, the store's included (E37): one timer per try over the name's resolution, the TCP connects to its addresses, tried in order and not splitting the timer, and the TLS handshake |
| `attempts` | integer | 3 | 1 to 10: tries per download, the first included |
| `backoff_initial_ms` | integer | 500 | 0 to 60,000 |
| `backoff_max_ms` | integer | 30000 | at most 600,000, and at least `backoff_initial_ms` (1.1) |
| `retry_on` | array of string | all six | a subset of `"connect"`, `"no_response"`, `"timeout"`, `"truncated"`, `"status_5xx"`, `"status_429"`: the try classes below that retry |
| `redirects` | string | `"same_origin"` | `"same_origin"` or `"none"` |

- The k-th retry waits `min(backoff_max_ms, backoff_initial_ms x 2^(k-1))`, with no jitter. A `Retry-After` header (seconds, or an HTTP date) raises that wait to its value, still capped at `backoff_max_ms`.
- No retry starts when its wait would pass the stop cutoff (the deadline less the grace, E7). A request in flight at a stop is abandoned: nothing polls it after the hand-over, and it ends at the process's exit (`batch.md` 2.11).
- `"same_origin"` follows at most 5 redirects whose scheme, host and port equal the request's, with its headers. A redirect anywhere else, past the fifth, with a `Location` over 16 KiB, or whose next URL, as the `url` crate serializes it, is over 16 KiB fails the download (`redirect_refused`), so a header never reaches another origin. `"none"` fails at the first redirect.
- **A try's class.** A try succeeds on a 200 whose body arrives whole. A try that fails takes the first row that matches. While tries remain it is retried when its `retry_on` member is listed, and the last try's `detail` is the `bundles` row's (7.3):

  | class | the try | `detail` | `retry_on` member |
  |---|---|---|---|
  | connect | fails before the connection is up: a name that does not resolve, a connection refused or unreachable, `connect_timeout_ms`, or a TLS handshake that fails, a certificate the roots do not chain among them (1.2) | `connect`, or `tls` for a failure rustls reports (a certificate, an alert, a protocol error); a close or reset during the handshake is `connect` | `connect` |
  | no response | sent its request, or part of it, and the connection closed or was reset before a complete response head, or the head was malformed (its status line, a header, a `Content-Length`) | `no_response` | `no_response` |
  | timeout | `idle_timeout_ms` or the rate floor, whose periods run from the request's send, so they cover the connect, when they fire before `connect_timeout_ms`, and the wait for the status line; or a 408 | `timeout`, or `status 408` | `timeout` |
  | truncated | a body that ends before its `Content-Length`, a chunked body that ends without its last chunk or holds a malformed one, or a reset mid-body | `truncated` | `truncated` |
  | a 5xx, or a 429 | answered with that status | `status <N>` | `status_5xx`, or `status_429` |
  | any other status | any other status the client returns as final: a 4xx; a 2xx but 200; a 101, the one 1xx `hyper` returns (it skips the others, `src/proto/h1/role.rs`); a 3xx the engine does not follow: any but a 301, 302, 303, 307 or 308, and one with no `Location` or one that does not join to a URL; and a redirect the rule above refuses (`redirect_refused`) | `status <N>` | none: never retried |
- Every request sends `Accept-Encoding: identity`, and no body is decompressed. Before any body is read, in this order, the first that fails naming the reason: a `size` above `M` (`limit(input_bytes, <M>)`); a `Content-Length` other than `size` (`integrity_mismatch`); a `Content-Length` above `M` (`limit(input_bytes, <M>)`). A try that fails truncates its file and returns its spool claim before the next try.

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
| `"object_store"` | `bucket` (string, required: 3 to 255 bytes of ASCII letters, digits, `.`, `-` and `_`); `region` (string, required: 1 to 64 bytes of ASCII letters, digits, `-` and `_`); `prefix` (string, default `""`, else ending in `/`; at most 512 bytes, so the run's fixed keys, at most 128 bytes past it (7.1), pass 2.3's `too_long`); `endpoint` (an `http` or `https` URL with no userinfo, query or fragment, its path empty or `/`, at most 1 KiB serialized, or null, default null: AWS; an `http` endpoint allows plain HTTP to that endpoint only, and an `https` one, AWS's included, is trusted as 1.2's URLs are); `path_style` (boolean, default `false`); `timeout_ms` (integer, default 30000, 1,000 to 300,000, per try); `attempts` (integer, default 3, 1 to 10: tries per request, the first included) |

**`endpoint`** is the service's URL without the bucket (`https://s3.example.net:9000`). The engine maps it for each mode: with `path_style` true, requests go to `<endpoint>/<bucket>/<key>`; with `path_style` false, to `<scheme>://<bucket>.<host>[:<port>]/<key>`, which the engine builds from it. With `endpoint` null, AWS's endpoint for `region` in the same mode.

Every key is written at the sink's root (directory) or after its prefix (object store). The engine makes every try of a store request itself, waiting between tries as `http`'s backoff rule says (1.4), and the store client retries nothing, so each try is seen and counted (7.3). The engine's own HTTP service records each try's status, `Retry-After` and the S3 error code its body names, at any status (`batch.md` 2.9), and the try is classified by the first row that matches:

| the try | settled | retried, while tries remain |
|---|---|---|
| a failure before the connection was up, so nothing was sent (a resolve, connect or TLS failure, or `timeout_ms` during them) | yes | yes |
| no response otherwise: a `timeout_ms`, a dropped connection, a try a stop abandoned | no | yes |
| a 5xx; a 2xx the call still returns as an error (a body holding an `<Error>`, one past the connector's cap, or an `ETag` the engine keeps over 100 bytes, `batch.md` 2.9) | no | yes, `Retry-After` raising the wait as in 1.4 |
| a 2xx | yes | no: success |
| a 408 or a 429 | yes | yes, `Retry-After` raising the wait |
| a 304, 409 or 412, or an `If-Match` write's 404, on a head write | yes | no: read back (7.2) |
| a 404 on a head's read-back | yes | no: no head (7.2) |
| a `NoSuchUpload` on a retried `CompleteMultipartUpload` | yes | no: a HEAD of the key, whose expected ETag means `written` (E37), else `publish_failed` |
| a 4xx whose error code is `ExpiredToken` | yes | no: `credentials_expired` |
| any other 3xx or 4xx | yes | no: `publish_failed` |
| anything else | no | no |

A body that fails, or passes `timeout_ms`, after its status arrived leaves the try its status's row, a 2xx's being a 2xx the call returns as an error, with section 10's `truncated`. The table classifies every try of a transfer, a row-file write, a head write, a head's read-back, a phase-7 LIST and the HEAD after `NoSuchUpload`. For a head write or its read-back, a row that gives `credentials_expired` or `publish_failed` gives 7.2's outcome instead: a `running` write exits 69, a final write records 4. A phase-7 LIST that fails degrades as `batch.md` 2.9 says (`resume_list_failed`, `verify_list_failed`), and a HEAD that fails gives `publish_failed`. Preflight's requests and every abort use only the first two columns, with their own outcomes: the bucket proof takes any 2xx as proof, whatever its body (7.2), and an abort's `NoSuchUpload` counts as a successful abort, in the run, in the finisher and at the spool's start (6.2). After a stop, no try is retried by this table; 7.2's rewrites still run (`batch.md` 2.11). A try of an output, archive or row-file request that is not settled is counted in `unsettled_tries` (7.3). Credentials come from the environment only (E37): `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` when set and not empty, as AWS's own tools read it. Either of the first two unset or empty exits 69 (`credentials_missing`). An access key id or a secret over 128 bytes, a session token over 16 KiB, any of the three that is not UTF-8, a key id or token holding a byte outside 1.2's header-value set, or an `AWS_CREDENTIAL_EXPIRATION` set but not RFC 3339 exits 69 (`credentials_invalid`): the key id and token go into request headers, and the secret only keys the signature. The caller gives credentials valid until the deadline: when `AWS_CREDENTIAL_EXPIRATION` (RFC 3339, as the AWS CLI exports it) is set and comes before the deadline, preflight exits 69 (`credentials_expire`); a request the store refuses with `ExpiredToken` fails with `credentials_expired`, and is not retried.

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

**Literal checks at validation.** What the literals fix is checked on the template: each `/`-separated segment made only of literals passes section 2.3's segment rules (`empty_segment`, `dot_segment`, `segment_too_long`, `reserved_name`); a template that starts with a literal passes `leading_slash` and `reserved_prefix` on it, the key `_unityrs` itself refused when that literal is the whole template; one that ends with a literal passes `trailing_slash`. Everything else is checked on each expansion.

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
| `reserved_prefix` | it is `_unityrs` or starts with `_unityrs/` (section 7) |
| `reserved_name` | a segment starts with `.unityrs-tmp-` (the directory sink's temp files, E62) |
| `prefix_conflict` | it is an ancestor of another key of `P` or of an archive's key (section 4.6), or, for a member path, of another remaining member's path (section 4.7) |
| `archive_conflict` | it equals an archive's key or lies under one (section 4.6) |

A literal key that breaks a rule is refused at validation. An expanded key that does is the output's object-level skip, `unwritable_key(<rule>)`. The rules are the same for every sink, and an archive member path takes every rule but `reserved_prefix`, `reserved_name` and `archive_conflict` (section 4.7), so a key is writable in one sink exactly when it is writable in all of them.

---

## 3. Selection

What one input exports (E66):
- **Textures**, when `outputs.textures` is set: every Texture2D object of the input's own SerializedFiles. A sheet section 5 cuts is exported too, as the unit's texture (section 4.5).
- **Sprites**, when `outputs.sprites` is set: every Sprite object of the input's own SerializedFiles. The sheet a sprite renders from may be in a dependency (`batch.md` 2.6).
- **Atlases**, when `outputs.atlases` is set: section 5.

The input's **own SerializedFiles** are the SerializedFiles among its bundle's top-level files, as `BundleFile::files()` gives them: UnityPy's `files` dict, where a later directory row of a repeated path replaces an earlier one in its place. A nested container's files are not selected, where UnityPy's `env.objects` and `env.container` reach them; each such container is a `nested_container` diagnostic (7.3).

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

- The attempt's private Environment registers `view(X)` less `X` in bytewise order of id, then `X` last, always, from the bytes of its own open. Each top-level entry of a registered bundle that is itself a bundle is reached at its registration, which registers its subtree as UnityPy's eager load does (`batch.md` 2.6 step 4). So a lookup by name finds every CAB of the view, nested or not, and a pointer between two containers of `X`'s bundle, or into a dependency's nested container, resolves as UnityPy resolves it, whatever `X` declares. The attempt inherits the core's `Lazy` differences from UnityPy's eager load (`PARITY.md` section 19.3, (a) to (d)):
  - (a) and (c): an entry that fails to parse, or a block that fails to decode, fails only the reads that meet it (`batch.md` 2.7), where UnityPy's eager load fails the input, except a `Limit` or a refusal that registration's peek meets (`batch.md` 2.6 step 4). One level down, a nested container's reach fails the input on a `Limit` or a parse failure (`batch.md` 2.6 step 4). An earlier directory row of a repeated path, shadowed in `files()` (section 3), is never selected. Until CR7 (`batch.md` section 6), two gaps remain, each able to decide a record without naming its reason. Two or more levels down, a non-transient `Limit` or a parse failure leaves the container unreached or a plain entry. And at a **dropped container row**, a top-level row of a registered bundle whose registry key (`simplify_name`, the lower-cased basename) a later row of the same bundle takes, an exact repeated path among them, the core's settle swallows a parse failure or a non-transient `Limit` and retries a refusal at the next lookup, its evictions counted; once the row's bundle is evicted, nothing reaches the row, since only a container pass does and the engine runs none. A non-container top-level row of one member can also be dropped by a later member's row of the same key: the core peeks it at that load, or at the reach that drops it, and retries a refusal at later lookups. That changes no record but through the evictions its retried peek causes, counted against `max_attempt_evictions`, and, until CR7, the decode work its retries spend: the later row holds the key, as in UnityPy, and the refusal is absorbed and counted.
  - (b): a nested container's own name is a key, so a CAB of that name registered before it is not found by name, where UnityPy's eager load, which gives the container no key, finds it.
  - (d): a read that decodes its span again can meet `InputMemory` where an eager load's one run of every block fits.
- **A pointer** is dereferenced by the core's `Environment::deref` from the file that holds it. The reading file's own bundle answers first (its siblings, through `Parent`; Tier A). Then the registry answers with the registration of greatest rank: `X` when it holds the CAB, else the greatest id among the view's bundles that hold it.
- **A resource** (`.resS`) read follows the **own-chain procedure**. For each of UnityPy's candidates in order, it walks the containers that hold the reading file, from the innermost outward. In each container it takes the candidate's matching top-level rows (by registry key) from last to first, opening each: a row that fails to open fails the read; a raw row is read; a SerializedFile fails the read (`not_a_reader`); a bundle or WebFile row moves on to the previous matching row. When no row is left, it moves to the next enclosing container. So the innermost container's row wins, and within one container the last matching row that is not a container (`PARITY.md` section 19.2's Tier C). Only when no candidate was read, or refused, from an own entry are they looked for through `Environment::resource` on the private Environment (`batch.md` 2.6 step 8).
- A CAB no bundle of the view holds is the reading object's skip, `outside_view(<cab>)`.

### 4.4 Objects and candidates

- **Object order** in a bundle: its own SerializedFiles (section 3) in `files()` order, and in each `objects()` order, UnityPy's `objects` dict order.
- **The plan.** Before it exports anything, the attempt plans every output its selection makes (section 3; section 5's recognition), in **plan order**: (kind, object order), kinds ordered `textures`, `sprites`, `atlases`, and within one atlas its JSON before its sprites, the sprites in `mSprites` order. In this order:
  1. **Names** (E29). Of two objects of one class and one `m_Name`, the first in object order takes the name, even when its export then fails. The later one is skipped, `duplicate_name`.
  2. **Skips that need no pixels**: `unsupported_format`, `unsupported_platform`, `empty_image`, `unnamed_atlas`, `reference_atlas`, `ngui_layout_unsupported`, `no_typetree`, `sheet_not_found`, `empty_rect`. Such an output is not planned and takes no key.
  3. **Keys.** Every planned output's key is expanded; one that breaks a rule of section 2.3 but `prefix_conflict` and `archive_conflict`, which phase 6 decides (4.6), is skipped, `unwritable_key(<rule>)`. Of two planned outputs with one key, the first in plan order takes it and the later one is skipped, `duplicate_key`, even when the first's export then fails. The exception: inside one atlas, of two sprites with one key, the later one wins (E30).
- **Sprite groups.** Sprites are grouped by their sheet: the (bundle id, CAB name, path id) of the sheet texture, and the same of its alpha texture or none. The groups run in the order of each group's first sprite.
- **Export order.** Sprite groups, then each sheet that atlases cut with its atlases (section 4.5), then every other texture, each group's outputs in plan order.
- **Record order.** An attempt records its `same_named_cab` diagnostics (once the view is registered, in bytewise order of name), then its `undeclared_dependency` diagnostics (in `m_Dependencies` order) and `nested_container` diagnostics (in object order), then the plan's skips in plan order, then the export's skips in export order. Within an input, the `skips` and `diagnostics` rows are in this order.

### 4.5 Winners across inputs

E31 to E33 and E61 as written, with these orders and units:
- "first by bytewise input id" everywhere;
- E31 rule 1 applies when the preferred input has a candidate for the key;
- **Units** (E32). A unit is made of outputs the attempt made. When the input made a sheet's texture output (`outputs.textures` set, and the texture neither skipped nor failed), the unit is that texture with every atlas of the input that cuts the sheet (section 5), their crops and their JSONs, and its **unit key** is the texture's key. Otherwise each atlas whose JSON was made is a unit of its own, its crops and its JSON, keyed by the JSON's key; an atlas with neither has no unit, and its crops are plain candidates. The attempt exports a sheet's texture before its atlases, and an atlas's JSON before its crops (4.4), so a unit's key is fixed before its crops are recorded. A unit key is always the key of an output of the unit. A unit's **keys** are its unit key and its members' keys. Its `prefer` entry is its unit key's (an entry for another key of a unit is not consulted), and E31 rule 2 compares that output's MD5.
- The steps, in order (`batch.md` 2.9):
  1. Each unit key's winning unit is decided by E31 and E61 among the units with that unit key. A unit key that E61 withholds withholds every unit with that key, and their keys are reserved in step 2 as a winning unit's are.
  2. Every key of every winning unit is **reserved**. Of the winning units that share a key, the first by unit key keeps it, and every other is withheld whole (`unit_conflict`). A key stays reserved even when every unit claiming it is withheld; this step is not repeated.
  3. A plain candidate for a reserved key loses (rule `unit`). Every other key is decided by E31 and E61.
  4. Section 4.6 runs once on the keys steps 1 to 3 would publish.
- So phase 6 never publishes a unit in part: a unit is published whole or withheld whole. An attempt-time skip of one of its outputs (`unwritable_key`, `duplicate_key`) removes that output only.

### 4.6 Prefix conflicts

Let `P` be the keys section 4.5's steps 1 to 3 would publish: withheld keys, `unit_conflict` keys and losers are not in `P`.
- A key `K` of `P` that is an ancestor of another key of `P` or of an archive's key (one starts with `K/`) is `unwritable_key(prefix_conflict)`; the keys and archives beneath it stay.
- Any other key of `P` that equals an archive's key or lies under one is `unwritable_key(archive_conflict)`; the archive stays.
- A unit with a key refused here is withheld whole: its other keys are `unit_conflict`.
- The check runs once, on `P` as steps 1 to 3 left it, and is not repeated after a unit is withheld. It runs on `P`'s keys and the archives' keys in **`/`-first order** (bytewise, except that at the first byte where two strings differ a `/` is less than any other byte), in which every key's descendants follow it, so its result does not depend on the order of publishing (`batch.md` 2.9). It is the same in every sink. What remains is the set of **winners**.

### 4.7 Archive members

For each archive and each of its `members` entries, every winner of that kind is a member, at the path the entry's template gives for it (expanded by the attempt that made the candidate, `batch.md` 2.8). Then, per archive:
- a path that breaks a rule of section 2.3 but `prefix_conflict` (below) is left out first (`unwritable_path(<rule>)`) and takes no part in the rules below;
- the rest are ordered by path in `/`-first order (4.6), so the members under one directory are contiguous;
- of several members with one path, the one whose key is first in bytewise order is kept, and the others are left out (`duplicate_member`);
- a path that is an ancestor of another remaining member's path is left out (`prefix_conflict`).

Membership depends only on the winners, never on how their loose PUTs end, so an archive's bytes are a function of the committed records. With `publish_incomplete: false` and a failed input, nothing is published, and every archive with a member is `withheld` (`publish_incomplete`).

---

## 5. The NGUI cutter

**Recognition.** A MonoBehaviour of the input's own SerializedFiles is an atlas when:
1. its `m_Script` dereferences (a read that may cross into the view; E21) to a MonoScript whose `m_ClassName` is in `script_classes`; and
2. its typetree has a field `mSprites` that is an array.

A MonoBehaviour that meets 1 but not 2 is skipped, `ngui_layout_unsupported` (NGUI 2's `sprites` layout is not in version 1). One with no typetree is skipped, `no_typetree`; version 1 has no fallback for a stripped atlas. One whose `m_Script` read fails with an error `batch.md` 2.7 makes an object's skip is not an atlas, and the input's record counts it (`unresolved_scripts`); when that error is of the budget class (exit 5), it is also recorded as a skip of the object it failed on, with its reason, so the records and the exit code show that a budget decided. Any other error (a `Budget(Dependency)`, a run-level class) goes as 2.7 says: it fails the input or stops the run.

**Skipped atlases.** An atlas with an empty `m_Name` is skipped, `unnamed_atlas`. An atlas whose `m_Name` ends with a string of `skip_name_suffixes` is skipped, `reference_atlas`.

**The sheet** (E21, scope `own_files_by_name`; `follow_pptr` is not in version 1).
1. The material's main-texture name: dereference the atlas's `material`, then, in its `m_SavedProperties.m_TexEnvs` (a list of pairs), the first pair whose first is `_MainTex`, or a struct whose `name` is (UnityPy 1.25.2 types it either way, `classes/generated.py:11477`); dereference that pair's `m_Texture` and read its `m_Name`. An error on this path that `batch.md` 2.7 makes an object's skip gives no name, and the next step runs, recorded as above when it is of the budget class. Any other error goes as 2.7 says, as above.
2. Among the input's own Texture2D objects in object order, the first whose `m_Name` equals that name exactly (bytes, case-sensitive). When there is none, or no name, the first whose `m_Name` equals the atlas's `m_Name`.
3. No match skips the atlas, `sheet_not_found`.

A sheet that is skipped or fails (`unsupported_format`, `unsupported_platform`, `empty_image`, `decode_error`, a budget) skips each atlas that cuts it, crops and JSON, with the sheet's reason; so does a sprite whose sheet does.

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
| `<dir>/runs/<run_id>/journal.tmp` | a removal's rewrite of the journal, renamed over it (next row) |
| `<dir>/runs/<run_id>/journal` | multipart upload ids, one per line as a JSON array `[endpoint, bucket, key, upload_id]`. Every append and every removal opens the journal by path, under the journal's mutex. Before an append, a journal whose last byte is not `\n` gets one. A line is appended and `fdatasync`ed after the create (the append that creates the journal is followed by an `fsync` of `runs/<run_id>/`), and removed only after a successful complete or abort (E64): the journal is streamed line by line, one line held, into `journal.tmp` without it, which is `fdatasync`ed and renamed over it, and `runs/<run_id>/` is `fsync`ed before the mutex is released. So it holds the uploads in flight and those whose abort failed; a removal a crash loses costs only an extra abort (6.2 step 4); a reader skips a line that is empty, over 16 KiB (which no line at 1.6's and 2.3's caps reaches, escaped, with an upload id under 1 KiB, `batch.md` 2.9), or that does not parse, a torn last line among them |
| `<dir>/runs/<run_id>/in/` | staged bundles, read-only: `i<n>` for the n-th of `inputs`, `d<n>` for the n-th of `dependencies` (`batch.md` 2.5) |
| `<dir>/runs/<run_id>/attempts/<input>/` | one attempt's outputs and `records.ndjson` |
| `<dir>/runs/<run_id>/done/<input>/` | a committed attempt |
| `<dir>/runs/<run_id>/winners/` | phase 6's sorted runs, the winners file, the archive member lists and the sorted row files |
| `<dir>/runs/<run_id>/winners/outcomes`, `archive_outcomes` | phase 7's results: one byte per winner at its index in the winners file, and one 128-byte record per archive (that byte, size, MD5, ETag), made at phase 7's start (`batch.md` 2.9); a missing or short file reads as 0 past its end. The byte: 0 `not_written`, 1 `written`, 2 `present`, 3 `failed` (`publish_failed`), 4 `failed` (`publish_mismatch`), 5 `failed` (`credentials_expired`), 6 `failed` (`archive_too_large`, an archive's only) |

`<input>` and `<n>` are positions from 0, in decimal, so an id never becomes a path.

### 6.2 Start

The engine owns only what it created. Preflight starts the spool in this order, after reading the credentials. A failure in steps 1 to 3 exits 69 with nothing deleted, and any call on `dir` in steps 1 to 3 and 6 that fails with an error no step names is `spool_unusable` (9.3); step 4 never fails the start; an I/O error in step 5 is `host_io` (74), through the finisher, with no head (7.4); a failure in step 6 exits 69 after step 5's deletions, which removed only the engine's own files:
1. **Dedicated.** `dir`'s entries must be none, or only `.unityrs-spool`, `lock` and `runs`, the marker among them, with `lock` a regular file and `runs` a directory, by `lstat` (neither a symlink). Otherwise `spool_not_dedicated`. A `dir` whose `statfs` type is tmpfs or ramfs is `spool_in_memory`: its bytes would be memory outside the bound (`batch.md` 5.4).
2. **Apart from the sink.** With a directory sink, the real paths of `dir` and `root` neither equal nor contain one another. Otherwise `spool_overlaps_sink`; a `root` whose real path cannot be taken, a missing one among them, is `sink_root_unusable` (7.2).
3. **The lock.** `lock` is opened (created if absent) and locked; busy is `spool_locked`. The lock is held until `_exit` and the file is never deleted, so a second process always meets the same inode. An empty `dir` gets its marker now, under the lock.
4. **Old journals.** Every `runs/*/journal` is read, streamed through a fixed buffer, holding only the ids of the aborts in flight; each upload id whose endpoint and bucket are this sink's is aborted, at most `download_concurrency` at once, each with the sink's `timeout_ms` and `attempts`. `NoSuchUpload` counts as aborted (1.6): the upload completed before its id was removed, or the lifecycle rule took it. Any other failure, a 403 among them, leaves the id to the bucket's lifecycle rule (E64) and counts in the warning `abort_failed`, so no journal can stop a later start. An id of another endpoint or bucket is left to that bucket's lifecycle rule.
5. **Cleanup.** Everything under `runs/` is deleted, through directory handles opened with `O_NOFOLLOW`, so no symlink is followed out of the spool; this `run_id`'s earlier attempts and other run ids' alike: the lock proves no live process uses them.
6. **Free space.** `statvfs` of `dir` must show at least `reserve_bytes` free. Otherwise `spool_no_space`.

Nothing in the spool is reused between attempts: every attempt stages and decodes again (E44). After the final manifest, and after a stop, the spool is left as it is; the next start clears it.

---

## 7. The manifest

### 7.1 Location

| sink | the head | the row files |
|---|---|---|
| directory | `<root>/_unityrs/<run_id>/manifest.json` | `<root>/_unityrs/<run_id>/<attempt>/<file>.<k>.ndjson` |
| object store | `<prefix>_unityrs/<run_id>/manifest.json` | `<prefix>_unityrs/<run_id>/<attempt>/<file>.<k>.ndjson` |

No output key may start with `_unityrs/` (2.3). The row files hold the attempt number, so an earlier attempt never overwrites a later one's rows. Each row file is written in chunks `k` = 0, 1, ..., each at most 16 MiB and cut at a line's end, so each is one single-part PUT; a reader concatenates them in order. The head names each file's chunk count, not its keys, so it stays under 64 KiB whatever the rows' size.

### 7.2 Fencing, verification and the caller's duties

- **Preflight**, for an object store, first proves the bucket: a LIST of the prefix at `max_keys` 1 whose response is a 2xx, whatever its body holds, which is read under the LIST cap and dropped, never scanned or parsed (`batch.md` 2.9); a 404 exits 69 (`bucket_missing`), any other failure `sink_unreachable`. Then, for either sink (a directory under its lock, below), it reads the head; a 404, or no file, is no head. A head over 1 MiB, or one that is not a manifest, exits 69 (`manifest_unreadable`); a sink that cannot be reached exits 69 (`sink_unreachable`). A head of this `run_id` whose `attempt` is at or above this one's exits 7 (`superseded`; `batch.md` 2.11's precedence), and nothing is written but, in a directory, the `_unityrs` lock and `<run_id>` directory preflight made. Every head read, preflight's and every read-back, reads at most 1 MiB + 1 byte, so a longer head is never read whole (`batch.md` 2.9).
- **Object store, `"conditional"`.** The store client is configured with `S3ConditionalPut::ETagMatch`. The `running` write is `If-None-Match: *` (`PutMode::Create`) when no head exists, else `If-Match` (`PutMode::Update`) on the ETag preflight read. The final write is `If-Match` on the `running` write's ETag, or, in 7.4's second row, the precondition of the `running` write's last try (`batch.md` 2.11). A `running` write that fails other than by a failed precondition exits 69 (`sink_unreachable`), with nothing written but possibly the `running` head, which its own fence covers (9.1).
- **A failed precondition** is `object_store`'s `Precondition` or `AlreadyExists`: a 412, a `Create`'s 304 or 409, an `Update`'s 409, or an `Update`'s 404, which `object_store` reports as `Precondition` (`src/aws/mod.rs`). Each is read back with a GET. A head with this `attempt` and this write's `write_seq` is this write, whose response was lost: success. Only a head of a higher `attempt` is `superseded`: the attempt writes nothing more and exits 7 (`batch.md` 2.11's precedence). A writer never overwrites a head it cannot read: one over 1 MiB, or not a manifest, makes the `running` write exit 69 (`manifest_unreadable`) and the final write record 4, the head left as it was. Any other head is retried with `If-Match` on its ETag: at the final write, this attempt's own `running` head; at either write, a lower attempt's, an earlier attempt's late write that landed after preflight's read (after its 69, the caller retried at once, 9.1). No head at all (a concurrent write's 409, or a head removed) is retried with `If-None-Match: *`. Those retries are at most 4 per write; a fifth failed precondition exits 69 at the `running` write (`previous_attempt_live`), and at the final write leaves the stored head as it was, recording 4 (`publish_failed`). Only the finisher writes after `running` (`batch.md` 2.11), so no two writes of one attempt share a `write_seq`.
- **Object store, `"none"`.** Plain PUTs.
- **Directory.** `<root>/_unityrs/lock`, one lock for the whole root, whatever the `run_id`: `flock(LOCK_EX | LOCK_NB)` held for the process's life, busy exits 69 (`output_locked`); any call on the root at preflight that fails with an error no rule names (its open as a directory, `realpath`, the `_unityrs` and `<run_id>` creates, the head's read, `openat2` with an errno but `ENOSYS` and `EPERM`) exits 69 (`sink_root_unusable`). So one process at a time writes a root. Each head write is a temp file, `fsync`, a rename over the head, and an `fsync` of the directory, made on the writing thread itself.
- **Verification** (object store). After its last transfer, phase 7 LISTs the prefix and checks every key and archive it marks `written` or `present` against the ETag it expects (`batch.md` 2.9). One that is missing or differs is `failed` (`publish_mismatch`). A key whose ETag is not an MD5 (`etag_not_md5`, SSE-KMS or SSE-C) is checked by its size alone and counted in `verified_by_size`: for it, the store held an object of this attempt's size, not necessarily its bytes. The head's `verified` is true when the pass ran to its end; false when a LIST failed or came out of order (warning `verify_list_failed`) or a stop interrupted it (no warning); and null for a directory sink, where the root's lock leaves no other writer, and when nothing was published (`publish_incomplete: false`).
- **What no client can fence.** A store may commit a request it has received long after the client gave up on it, and after the client's process has ended: S3 documents that a `CompleteMultipartUpload` can take several minutes. So the engine bounds only its own writes, and the caller owns the rest.
- **The caller's duties** (object store):
  1. At most one process of a `run_id` runs at a time, two of one attempt number included (an orchestrator's single-flight claim per run does this), nothing but that run's attempts writes its keys, and two `run_id`s never share keys. `"conditional"` is set only for a store that enforces `If-Match` and `If-None-Match` on a PUT.
  2. Before it starts attempt `n + 1`, the caller waits `W` past the end of attempt `n`'s process, or past its `deadline` when it cannot see that end. `W` is the store's longest time to commit a request it has received, plus the time the host's kernel may still send a closed connection's buffered bytes after the process ends (Linux's orphan retries, `tcp_orphan_retries`); for AWS S3 this contract states 15 minutes. There is no wait when attempt `n` ran with `"conditional"` fencing and its final head has a null `stop_reason` and `unsettled_tries` 0: every try of every output, archive and row-file request got a response that settled it, and every try of a head write is fenced (above), so none can land after the head. Nor is there a wait after an exit 69, which leaves at most a fenced `running` head. With `"none"` the caller always waits, after a 69 too.

The head is the record of the run. For every key the final head lists as `written` or `present`, the store held this attempt's bytes when verification listed it (with `verified` true; for a key counted in `verified_by_size`, an object of their size), and holds them still if the caller kept its duties. With `verified` false or null, the first half is not checked. A key the head does not list so is not part of the run's output, whatever the store holds.

### 7.3 Fields

**The head:**

| field | type | meaning |
|---|---|---|
| `manifest_version` | integer | `1`. Within a version these sets are closed: the reasons, statuses and warning codes; the kinds, rules and values inside a reason's arguments (`limit(<kind>, ..)`'s, 9.3's table; `unwritable_key(<rule>)`'s and `unwritable_path(<rule>)`'s, 2.3's; `unsupported_platform`'s `switch` and `xbox360`); the `detail` vocabularies (this section's codes, 1.4's try classes, 9.3's and section 10's forms); the keys of `counts` and `warnings`; the values of `collisions.rule`, `diagnostics.kind`, `outputs.kind`, `mode`, `phase` and `stop_detail`; and UnityPy 1.25.2's `TextureFormat` names and its `ClassIDType` names (`skips.class`). A value's free parts are open: a panic payload, an S3 error code, an errno or call name, a JSON pointer, a status number, a variable's name, a path, an id, a key and a number, a `TextureFormat` value or a class id written in decimal among them. A new closed value or a new field takes a new version, and a version only adds; a value only the progress stream carries (a validation or preflight reason, or its detail) raises `p` instead (section 10). A reader of an older version ignores a field it does not know, and reads a value of a closed set it does not know as "other". Fencing (7.2) reads a head of any version by its `run_id`, `attempt` and `write_seq` alone: a head without them, of these types, is not a manifest |
| `run_id`, `attempt` | string, integer | from the spec |
| `write_seq` | integer | 0 for the `running` write, 1 for the final |
| `status` | string | `running`, `complete`, `complete_with_skips` or `incomplete` |
| `phase` | string | the phase the controller had reached when the finish started (section 10's values): `start` in the `running` write, `publish` at a normal end |
| `exit_code` | integer or null | null while `running` |
| `stop_reason` | string or null | a stop reason of section 9.3, or null |
| `stop_detail` | string or null | the detail kept with `stop_reason` (`batch.md` 2.11): for `spool_full`, `ENOSPC`, `EDQUOT`, `reserve` or `size_bytes` (`batch.md` 2.7); else null |
| `binary_version`, `spec_version` | string, integer | |
| `spec_sha256` | string | the SHA-256 of `JOB.json`'s bytes as read, lowercase hex: each attempt's file holds its own `attempt`, so the digest differs between attempts |
| `started_at`, `deadline`, `ended_at` | string or null | RFC 3339, UTC |
| `verified` | boolean or null | whether verification ran to its end (section 7.2); null for a directory sink, or when nothing was published |
| `verified_by_size` | integer | the keys and archives verification checked by size alone (`etag_not_md5`, section 7.2); 0 for a directory sink |
| `unsettled_tries` | integer | the tries of output, archive and row-file requests this attempt made (`CreateMultipartUpload` and every part included, aborts aside) that the store may still commit: each 1.6's table does not settle (no response, a timeout, a try a stop abandoned, a 5xx, anything unclassified). The engine makes each try itself (1.6), so a try replaced by a later one is counted too (section 7.2) |
| `counts` | object | integers, each a count of row-file lines but `collisions` and `flips`, which count keys: `bundles` (every `bundles` row), `bundles_failed` (status `failed`); `inputs` (every `inputs` row), `inputs_complete`, `inputs_with_skips`, `inputs_failed`, `inputs_not_run` (by status); `outputs_written`, `outputs_present`, `outputs_not_written`, `outputs_withheld`, `outputs_failed`, `outputs_unwritable` (`outputs` rows by status, the last `unwritable_key`); `skips` (every `skips` row); `collisions` (the keys with more than one candidate); `flips` (those decided by a flip); `archives_written`, `archives_present` (`archives` rows by status). Each is 0 for a row file not written |
| `run` | object | E40's run-level figures. Its scalars are integers; a sum or peak over inputs with no term is 0; in the `running` write every figure the run measures is 0; `memory_bound`, the budgets as read and `spool` are the run's own in every write: `same_named_cabs`, `undeclared_dependencies`, `nested_containers` (counts of the `diagnostics` rows of each kind), `evictions` (the sum over inputs), `input_peak` and `dependency_peak` (the most of any input), `export_work_peak` (the most `export_work` of any input), `decode_work_peak` (the most decode work of any meter an attempt holds, sampled from `File::meter().decoded()`; it misses work on a meter after the attempt last held it, a refused first `with_room` try's, and a reopen's inside the core), `spool_peak`, `spooled_parts` (bytes, `batch.md` 2.6 step 10), `vm_hwm`, `memory_bound`, and the budgets as read, under their spec names: `max_inflight_uncompressed_bytes`, `max_decoded_bytes`, `dep_cache_bytes`, `max_attempt_evictions`, `max_export_work`, and `spool`, an object of `size_bytes` and `reserve_bytes` as in 1.3 |
| `warnings` | object | warning code (section 9.3) → count |
| `rows` | object | `bundles`, `inputs`, `skips`, `diagnostics`, `outputs`, `collisions`, `archives`, `archive_members`: each row file's chunk count `n` (its chunks are `<attempt>/<file>.<k>.ndjson` for `k` below `n`, relative to the head's directory, section 7.1), 0 for one with no rows, or null for one not written in full or after a `syncfs` error (7.4) |

**The row files** (NDJSON, one object per line, each sorted as stated):

| file | sorted by | fields |
|---|---|---|
| `bundles` | id | `id`, `status` (`staged`, `failed`, `not_staged`), `reason`, `detail`, `size`, `sha256` (of the staged bytes) |
| `inputs` | id | `id`, `status` (`complete`, `complete_with_skips`, `failed`, `not_run`), `reason`, `detail`, `input_peak`, `dependency_peak`, `evictions`, `export_work` (`batch.md` 2.6 step 10; for a failed input, its attempt's work up to its failure, a refused increment and its earlier flushes included, never the controller's failure record), `ms`, `skips` (a count), `unresolved_scripts`, `cut` |
| `skips` | input, then record order (4.4) | `input`, `serialized_file`, `path_id`, `class`, `reason`, `detail`, `cut` |
| `diagnostics` | input, then record order (4.4) | `kind` (`same_named_cab`, `undeclared_dependency`, `nested_container`), `input`, `bundle`, `name`, `cut` |
| `outputs` | key | `key`, `kind` (`texture`, `sprite`, `atlas_sprite`, `atlas_json`), `unit` (its unit key, or null), `input`, `serialized_file`, `path_id`, `md5`, `visible_hash`, `mode`, `size`, `status` (`written`, `present`, `not_written`, `withheld`, `failed`, `unwritable_key`), `reason`, `cut` |
| `collisions` | key, then input | one line per candidate of a key with more than one: `key`, `input`, `serialized_file`, `md5`, `visible_hash`, `winner` (boolean), `rule` (`prefer_input`, `prefer_md5`, `first_input`, `unit`, `unit_conflict`, `withheld`), `flip`, `pixels_differ` (against the winner), `cut` |
| `archives` | key | `key`, `status` (`written`, `present`, `not_written`, `empty`, `failed`, `withheld`), `reason`, `members`, `left_out`, `md5`, `etag`, `size` |
| `archive_members` | archive, then member order | `archive`, `path`, `key`, `input`, `md5`, `included`, `reason` (when left out), `cut` |

- **`outputs`** holds one row per key that has a candidate: the candidate the rules chose, also when its key is `withheld` or `unwritable_key`. The other candidates are in `collisions` only.
- **Statuses and reasons.** Each status allows only these reasons; `reason` is null where none is listed:

  | file | status | `reason` |
  |---|---|---|
  | `bundles` | `staged`; `not_staged` (a stop came first) | |
  | `bundles` | `failed` | a bundle reason of 9.3 |
  | `inputs` | `complete`, `complete_with_skips`; `not_run`, never dispatched | |
  | `inputs` | `not_run`, dispatched but not committed when the stop came (`batch.md` 2.4) | `stopped`, which, like the set of `not_run` inputs, depends on scheduling |
  | `inputs` | `failed` | an input reason of 9.3 |
  | `outputs` | `written`, `present`, `not_written` | |
  | `outputs` | `withheld` | `withheld` (E61), `unit_conflict` (4.5, 4.6) or `publish_incomplete` |
  | `outputs` | `failed` | `publish_failed`, `publish_mismatch` or `credentials_expired` |
  | `outputs` | `unwritable_key` | `unwritable_key(prefix_conflict)` or `unwritable_key(archive_conflict)` |
  | `archives` | `written`, `present`, `not_written`, `empty` | |
  | `archives` | `withheld` | `publish_incomplete` |
  | `archives` | `failed` | `publish_failed`, `publish_mismatch`, `credentials_expired` or `archive_too_large` |
  | `archive_members` | (`included` true) | |
  | `archive_members` | (`included` false) | `duplicate_member`, `unwritable_path(<rule>)` or `prefix_conflict` |

- **Types.** `id`, `input`, `key`, `path`, `archive`, `bundle`, `name`, `unit` and `serialized_file` (the stored path of the object's SerializedFile, as `nested_container`'s `name` is) are strings (`bundle` and `unit` may be null). `size`, `path_id` (signed), `input_peak`, `dependency_peak`, `evictions`, `export_work`, `ms`, `skips`, `unresolved_scripts` and the archives' `members` and `left_out` (the counts of its `archive_members` rows included and left out) are integers. A `bundles` row's `size` and `sha256` are null unless it is `staged`, or failed `integrity_mismatch` after its whole body arrived, when they are what arrived; an `inputs` row's peaks, `evictions`, `export_work` and `ms` are null when it is `not_run` or failed before dispatch (but a `stopped` row's `ms` is the time from its dispatch to the hand-over, `batch.md` 2.4, so a wedged attempt shows), and its `skips` and `unresolved_scripts` 0 when it is `not_run` or `failed`. `md5` and `sha256` are lowercase hex; `visible_hash` is lowercase hex, and `mode` one of `L`, `LA`, `RGB`, `RGBA`, both null on an `atlas_json` row. An archive's `md5`, `etag` and `size` are null unless it is `written` or `present`, and `etag` (quotes removed) is null for a directory sink. `cut`, `winner`, `flip`, `pixels_differ` and `included` are booleans. `skips.class` is UnityPy 1.25.2's `ClassIDType` name of the skipped object's class, or its class id in decimal when it has none; `detail` is a string or null.
- **`detail`** on a `bundles` row: for `download_failed` and `redirect_refused`, the last try's (1.4's try table); for `integrity_mismatch`, `size` or `sha256`, the first check that failed, the size's coming before the SHA-256's; else null. An error's **code** is that of the innermost variant `classify` reaches through the wrappers `Error::Entry`, `Error::AssetBundle`, `Error::Deref`, `ParseError::Deref` and `DerefError::Load` (`batch.md` 2.7): `detect`, `out_of_bounds`, `malformed`, `unsupported`, `decompress`, `version`, `type_index`, `tree` or `not_found`, a `ParseError`'s `Read` being `read`, its `Tree` `tree`, and any other `asset_bundle`. So a code the core adds or renames reaches a manifest only with a new `manifest_version`, this list and `classify` changing with it. On an `inputs` row: for `container_error`, the error's code; for `entry_error`, the code, a space and the entry's path in its container's directory, the innermost container's for a nested entry, cut (below) so the whole is at most 1,024 bytes; for a transport reason or `integrity_mismatch`, the bundle's `detail`; under `dependency_failed`, the member's, by its inner reason; for `stopped`, null but on the stop's culprit, the input whose attempt's run failure set the stop's reason (`batch.md` 2.11), where it is that reason, `internal`, `host_io`, `host_memory` or `spool_full`; else null. On a `skips` row: for `decode_error`, the error's code; for `decoder_panic`, the panic's payload, cut to its first 1,024 bytes at a code point boundary (`cut` then true), or `non-string panic payload` (`batch.md` 2.11); else null. A status is the server's answer, as the transport reason it qualifies is (`batch.md` section 1, D's second limit); a code is a function of the bytes.
- **A flip** is a rule-3 decision (E31) among candidates whose MD5s are not all equal.
- **Diagnostics.** `same_named_cab`: a registry key (`simplify_name`, 4.3) that two or more bundles of the input's view hold in their top-level directories, one row per key, `bundle` the top-level holder 4.3 ranks highest (`X` when it holds the key, else the greatest id) and `name` the full directory path of that bundle's last top-level row with that key. `undeclared_dependency`: `bundle` null, `name` the `m_Dependencies` name (4.1). `nested_container`: an entry of the input's own bundle that is itself a bundle, met while selecting, `bundle` the input, `name` the entry's path.

A reason is a code of section 9.3. A string taken from the bytes is cut to its first 1,024 bytes, at a code point boundary (an invalid byte counting as one code point, 2.2), and its row's `cut` is then true. A string that is not valid UTF-8 (a name, a CAB, an entry path, a reason's argument) is written with each invalid byte replaced by U+FFFD, so every line is UTF-8 JSON that any parser reads; its original bytes are not recoverable from the manifest, and two stored paths that agree after cutting and that replacement are not told apart. A row holds at most four strings of at most 1,024 bytes each (an id, a key or path, a cut string), each escaped to at most 6 bytes per byte, so every line is at most 32 KiB (`batch.md` 2.8). Apart from the figures the normalised comparison leaves out (timings, peaks, the host's, `extract.md` section 5), a manifest is a function of the run's records and outcomes; byte identity with another implementation is not a goal.

### 7.4 The manifest at each phase

The finisher writes it (`batch.md` 2.11), according to the phase the run reached:

| the finish starts | the head | the row files |
|---|---|---|
| before the `running` write was sent; at a 69 or 7 from it (7.2), or a directory sink's 74 (`extract.md` E62); or at a stop, with no try of it succeeded and every try settled by 1.6, a connect failure among them | none from the finisher, though a fenced `running` head whose response was not seen, or a newer attempt's head, may be stored; the `exit` line's `status` is null | none |
| at a stop, with no try of the `running` write succeeded and some try unsettled by 1.6, or answered with a 408 or 429 (object store); this row wins over the one above | the row below for the phase reached, written with the last try's precondition (7.2) | as that row |
| in phase 3, 4, 5 or 6 | `incomplete`, with `phase` and `stop_reason` | `bundles` (unstaged ones `not_staged`); `inputs` (committed ones with their records, the rest `not_run`); the committed inputs' `skips` and `diagnostics`. `outputs`, `collisions`, `archives` and `archive_members` are null |
| in phase 7, on a stop: a stop the controller saw at any of its phase-7 checks (`batch.md` 2.11: during resume's LIST or hashing, before a transfer's start, while it waits on the transfers or a LIST, after the transfers drained, after verification) | `incomplete`, with `phase` and `stop_reason` | every file. Each winner's and archive's status is read from the outcomes files (section 6.1): `written` where the write was acknowledged, `present` where resume found the bytes, `failed` with the outcome's reason, and `not_written` for every other, a transfer the stop left in flight included. With `publish_incomplete: false` and a failed input, every winner and every archive with a member is `withheld` (`publish_incomplete`), as at a normal end |
| after phase 7: the controller's last check saw no stop | `complete`, `complete_with_skips` or `incomplete`, by section 9.2, with a null `stop_reason`; a stop requested after that check changes only the finisher's try budget (`batch.md` 2.11) | every file, statuses read as above; with `publish_incomplete: false` and a failed input, every winner and every archive with a member `withheld` (`publish_incomplete`) |
| superseded at the final write | nothing more; the stored head is the newer attempt's | every row file, written before the head write found the newer head; no head names them |

**A `running` head** whose `deadline` has passed is a dead attempt's. In a directory, a reader whose `flock(LOCK_EX | LOCK_NB)` on `<root>/_unityrs/lock` succeeds knows that no writer is live, and releases the lock at once. The probe holds the writer's lock for that instant, so a writer starting then exits 69 (`output_locked`), and its caller retries.

In the first two rows, a read-back (7.2) is not a try of the `running` write. In a directory, a `syncfs` error after the row files writes the head `incomplete`, with `stop_reason` the more severe by 9.1's precedence of `host_io` and the stop's own reason, if any, and every row file null in `rows`, so it claims no file (`batch.md` 2.11).

---

## 8. Zip bytes

An archive is a function of its members' bytes, paths and order:
- every member stored (method 0), in member order (section 4.7), CRC-32 from the spool's record;
- local and central headers: version needed 20, or 45 in a header that carries a zip64 extra field (so an entry whose offset alone overflowed says 45 in its central header and 20 in its local one), version made by 20, general-purpose flags `0x0800` (UTF-8 names) and no data descriptor, DOS time `0x0000` and date `0x0021` (1980-01-01 00:00), no extra field but zip64's, internal and external attributes 0, no comment;
- **zip64** only where the classic fields overflow (APPNOTE 4.5.3). In a central header, each of the uncompressed size, the compressed size and the local-header offset that is at least `0xFFFFFFFF` is set to `0xFFFFFFFF`, and a zip64 extra field (id `0x0001`) holds exactly those fields, 8 bytes each, in that order. A local header carries a zip64 extra, holding both sizes with both classic size fields set to `0xFFFFFFFF`, only when a size overflowed; an offset alone gives the local header none. An archive whose member count is at least `0xFFFF`, or whose central directory's size or offset is at least `0xFFFFFFFF`, ends with a zip64 end of central directory record (version made by and needed 45, no extensible data), its locator (disk 0, total disks 1), and the classic end record with each overflowed field set to its maximum;
- the central directory is spooled while the members stream and written after them (`batch.md` 2.9).

So the archive's MD5, and its multipart ETag at 16 MiB parts, are the same on every run with the same members (E43). An archive over 10,000 such parts (156.25 GiB) is not written (`archive_too_large`). The `zip_offset` test hook (`batch.md` 2.11), which starts the offsets near `0xFFFFFFFF`, checks the zip64 forms two ways: CPython 3.11's `zipfile` reads every member's bytes back (a read check, since CPython writes other header fields), and the archive's bytes equal a golden file built from these rules.

---

## 9. Exit codes and reasons

### 9.1 Codes

| code | status written (or `running` when the head write itself failed) | meaning | the caller's usual action |
|---|---|---|---|
| 0 | `complete` | every input extracted, every winner published, no skip | done |
| 3 | `complete_with_skips` | object-level skips for data reasons only; every winner published | done; review the skips |
| 4 | `incomplete` | an input failed for a data or transport reason, or an output, archive, row file or the head failed to publish, verification among them | fix the input, or retry with `attempt + 1` after 7.2's wait (a transport reason may pass on a retry as is) |
| 5 | `incomplete` | a budget decided an outcome: an input's or a dependency's memory, bytes or work limit, an attempt's budget or eviction cap, the spool | raise the budget the manifest names, or, for a `spool_full` whose `stop_detail` is `ENOSPC`, `EDQUOT` or `reserve`, provision the filesystem (`batch.md` 5.6); retry with `attempt + 1` after 7.2's wait |
| 6 | `incomplete`, or none | stopped: the deadline, a signal, the parent's death, stdin's EOF | resume with `attempt + 1` after 7.2's wait |
| 7 | none by the finisher (7.4) | `superseded`: a later attempt of this `run_id` owns the head; in a directory, the `_unityrs` lock and `<run_id>` directory preflight made stay (7.2) | do not retry |
| 69 | none by the finisher (7.4) | could not start: section 9.3's preflight reasons; nothing written but what 7.4's first row may leave stored and, in a directory, the `_unityrs` lock and `<run_id>` directory preflight made | fix the environment, retry with `attempt + 1`, with no wait under `"conditional"` fencing (7.2) |
| 70 | `incomplete` if possible | internal error: a panic outside a decoder, a leaked charge (a refusal of phase 6's working set among them), an unclassifiable error, `ContainerDropped` | report a bug; a retry with `attempt + 1` after 7.2's wait may pass, as after a 6, but one that stops again at the same input (the `stopped` row with a `detail`, 7.3) is that input's failure: report it, or drop the input |
| 74 | `incomplete` if possible | host failure during the run: an I/O error on the spool (a full one is 5, `batch.md` 2.7), a staged file or a local source being copied, or the output directory's `syncfs`; any error of a directory sink's `running` write, for which the finisher writes no head (7.4); a mount crossed beneath the output root, or a filesystem's own error on it, in phase 7 (`extract.md` E62), but not, under resume, a mount at an output's destination itself (`extract.md` E43); a fallible allocation the allocator refused (an infallible one aborts, below) | fix the host, retry with `attempt + 1` after 7.2's wait |
| 78 | none | a bad spec or command line, an unknown `spec_version` included | fix the spec; never retry as is |

- **Precedence**, when several apply: 78, 69, 70, 74, 7, 6, 5, 4, 3, 0. A failure in `batch.md` 2.11's step 1 comes before validation, so its 69 stands whatever the spec holds. A stop's code is its reason's (section 9.3); a more severe stop that comes before the finisher takes the reason replaces it (`batch.md` 2.11).
- Codes 1, 2 and 64 to 68 are never used. A process killed by a signal shows `128 + N`: `SIGKILL`; a `SIGXCPU` past `RLIMIT_CPU`; `SIGABRT` (134) when the allocator refuses an infallible allocation (`batch.md` A13); a signal before `batch.md` 2.11's step 4 installs the handlers; and after it, any signal whose default action ends the process but the four it handles, SIGXFSZ, which it ignores (`batch.md` 2.11), and SIGPIPE, which Rust's runtime ignores. A second signal (SIGTERM, SIGINT or SIGHUP) exits at once, and the hard exit at `T_hard` exits, with the final code once it is stored, else with the stop's code (`batch.md` 2.11), and a final head write in flight may still land. Only SIGTERM, SIGINT and SIGHUP count toward a second signal (the parent's death does not), so a stop for any other reason, followed by one of them, still ends through the finisher.
- `unityrs batch --check JOB.json` exits 0 for a valid spec and 78 otherwise. `unityrs --version` and `unityrs batch --schema` exit 0.

### 9.2 From outcomes to codes, at a normal end

| outcome | code |
|---|---|
| an object skipped for a budget: reason `budget(input)`, or `limit` of kind `input_memory`, `input_bytes` or `decode_work` | 5 |
| an object skipped for any other reason | 3 |
| an input failed for a budget, by the same reasons, `budget(dependency)`, `limit(attempt_evictions, ...)` or `limit(export_work, ...)`, or by `dependency_failed(<id>, <reason>)` whose inner reason is one of them other than these two, which always fail the input by their own reason (`batch.md` 2.7) | 5 |
| an input failed for any other reason (a transport reason, `integrity_mismatch`, `dependency_failed` with any other inner reason, `not_a_bundle`, a container or entry error, a depth or count limit) | 4 |
| an output, archive, row file or the head that failed to publish (`publish_failed`, `publish_mismatch`, `credentials_expired`, `archive_too_large`) | 4 |
| an output or archive `not_written` (none at a normal end: a stop seen after the last transfer's start makes the end a stop, 7.4) | 4 |
| a bundle that failed, whether or not an input's view holds it | 4, or 5 for a budget reason |
| a key, member or archive that phase 6 refused (`unit_conflict`, `unwritable_key(prefix_conflict)`, `unwritable_key(archive_conflict)`, `duplicate_member`, `unwritable_path`) | 3 |
| a key withheld (E61, `publish_incomplete`) | none of its own: the failed input sets the code |
| an archive `empty` | none |

A depth or count limit is a data reason: no budget raises it. `budget(dependency)` is never an object's skip: a Dependency budget refusal the core returns after the view is registered (4.3's gaps aside, until CR7) fails the input (`batch.md` 2.7).

### 9.3 Reasons

A reason is `name` or `name(argument, ...)`. A `limit`'s arguments are its kind, from this table, and its configured value in decimal, never the amount asked for. The engine maps the core's `LimitKind` through the table; a kind the table lacks (a newer core's) is `classify`'s unknown, a run failure `internal` (`batch.md` 2.7).

| kind | from | class |
|---|---|---|
| `input_memory`, `input_bytes`, `decode_work` | the core's `InputMemory`, `InputBytes`, `DecodeWork` | a budget (exit 5) |
| `attempt_evictions`, `export_work` | the engine (`batch.md` 2.6 steps 7 and 10) | a budget (exit 5), always the input's failure |
| `block_output`, `inflate`, `container_depth`, `tree_depth`, `value_depth`, `values_per_object`, `zero_size_elements`, `count` | the core's `BlockOutput`, `Inflate`, `ContainerDepth`, `TreeDepth`, `ValueDepth`, `ValuesPerObject`, `ZeroSizeElements`, `Count` | data (exit 3 or 4) |

`unsupported_format`'s argument is UnityPy 1.25.2's `TextureFormat` member name, or the decimal value when it has none. Version 1's names, by where they appear:

| where | names |
|---|---|
| a bundle or an input, transport (exit 4) | `download_failed`, `redirect_refused`, `source_missing`, `source_not_regular`, `source_changed` |
| a bundle or an input, data (exit 4) | `integrity_mismatch`, `not_a_bundle`, `container_error`, `entry_error`, `dependency_failed(<id>, <reason>)`, `limit(<kind>, <value>)` |
| a budget (exit 5) | `budget(input)`, `budget(dependency)`, `limit(input_memory \| input_bytes \| decode_work \| attempt_evictions \| export_work, <value>)` |
| an object's skip | `budget(input)`, `limit(input_memory \| input_bytes \| decode_work, <value>)`, a data `limit`, `outside_view(<cab>)`, `bad_pointer`, `decode_error`, `decoder_panic`, `unsupported_format(<name>)`, `unsupported_platform(switch \| xbox360)`, `empty_image`, `duplicate_name`, `duplicate_key`, `unwritable_key(<rule>)`, `empty_rect`, `unnamed_atlas`, `reference_atlas`, `sheet_not_found`, `ngui_layout_unsupported`, `no_typetree`, `json_unwritable` |
| an output's or archive's status | `withheld` (E61), `unit_conflict`, `publish_incomplete`, `publish_failed`, `publish_mismatch`, `credentials_expired`, `archive_too_large`, `unwritable_key(<rule>)` |
| a member left out | `duplicate_member`, `unwritable_path(<rule>)`, `prefix_conflict` |
| a stop | `deadline`, `sigterm`, `sigint`, `sighup`, `parent_died`, `stdin_eof` (6); `spool_full` (5); `host_io`, `host_memory` (74); `internal` (70) |
| an input `not_run` (7.3) | `stopped` |
| a head read (exit 7, 7.2) | `superseded` |
| preflight (exit 69; the `detail` in parentheses, null where none is named) | `spool_not_dedicated`, `spool_in_memory`, `spool_overlaps_sink`, `spool_locked`, `spool_no_space`, `spool_unusable` and `sink_root_unusable` (`<call> <ERRNO name>`, 6.2, 7.2), `output_locked`, `manifest_unreadable` (`too_large` or `not_a_manifest`, 7.2: a head the sink gave whole; a head read or read-back the store refuses is `sink_unreachable`), `bucket_missing`, `sink_unreachable` and `previous_attempt_live` (from a store request, section 10's form), `credentials_missing`, `credentials_invalid`, `header_env_missing`, `header_env_too_long` and `header_env_invalid` (each the variable's name; `credentials_missing`'s the first of `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` unset or empty), `credentials_expire`, `deadline_too_close`, `rlimit_too_low` (`<limit> <needed> <found>`: `RLIMIT_AS`, `RLIMIT_DATA`, `RLIMIT_NOFILE`, `RLIMIT_FSIZE`, `RLIMIT_NPROC` or `pids.max`, and two figures in decimal: the needed one rounded up to an integer, `RLIMIT_FSIZE`'s `infinity`; the found one the soft limit, but the hard one for `RLIMIT_NOFILE` and `RLIMIT_FSIZE`, whose soft limits preflight raises to it, `infinity` for none; for `pids.max`, the found figure is the room, `pids.max - pids.current`, and the level's cgroup path follows it, relative to the hierarchy's root, as `/proc/self/cgroup` gives it), `overcommit_strict`, `map_count_too_low`, `memory_too_small` (`MemTotal <bound> <found>`, or `cgroup <bound> <found>` when the cgroup's limit is the smaller, in bytes), `cgroup_unreadable` and `procfs_unreadable` (the file's path, or `capget`), `startup_failed` (for a call of `batch.md` 2.11's step 1 or steps 3 to 5, `<call> <ERRNO name>`, such as `prctl EINVAL`, or the call alone where it sets no errno; for a store client whose build fails in preflight, `s3_build`; or `stdin` for an fd 0 that `stop_on_stdin_eof` cannot watch, 1.1), `thread_spawn_failed`, `kernel_too_old` (a directory sink on Linux before 5.8), `mount_crossing`, `mount_check_unavailable` (`extract.md` E62) |
| validation (exit 78) | `bad_spec` (its rule and pointer, section 10) |
| a warning (the head's `warnings`) | `resume_list_failed`, `verify_list_failed`, `etag_not_md5`, `stderr_dropped`, `abort_failed`, `stdin_watch_ended` |

---

## 10. The progress stream

NDJSON on stdout (E4), in the stream's own format version `p`: 1 in this release, raised when a binary adds a field or a value of a closed set (`type`, `phase`, `status`, `reason`, a `detail`'s form), whatever the `spec_version`, as `manifest_version` is (7.3). Each line is one JSON object and a whole snapshot, never a delta: a reader needs only the last line it read. Every line has every field below. A reader ignores a field it does not know, reads a value of a closed set it does not know as "other", and reads a line of a higher `p` by the fields it knows.

| field | type | meaning |
|---|---|---|
| `p` | integer | the stream's format version |
| `type` | string | `snapshot`, or `exit` for the last line |
| `seq` | integer | 0 on the first line, then +1 per line, gapless |
| `ms` | integer | ms since the process started (`CLOCK_BOOTTIME`) |
| `phase` | string | `validate`, `preflight`, `start`, `stage`, `extract`, `winners`, `publish` or `finish`: `batch.md` 2.1's phases 1 to 8 in order |
| `inputs` | object | integers: `total`, `complete`, `failed`, `running`, `queued`, `not_run` |
| `transfers` | object | integers: `stages_active`, `stages_done`, `stages_failed`, `uploads_active`, `uploads_done`, `uploads_failed` |
| `memory` | object | bytes, integers: `inflight_limit` (`I`), `slots_reserved`, `attempts_used` (the attempts' budgets' summed use), `phase_used`, `rss` |
| `spool` | object | bytes, integers: `used`, `size` |
| `outputs` | object | integers: `candidates`, `winners`, `published`, `present`, `failed`; `winners` is 0 before phase 6 |
| `status` | string or null | on the `exit` line, the status of this attempt's last head write the store acknowledged, or null when none was or the attempt was superseded, which does not mean the store holds no head (7.4); null on a snapshot |
| `code` | integer or null | on the `exit` line, the exit code (section 9); null on a snapshot |
| `reason` | string or null | on the `exit` line, the reason that set the code: a stop, preflight, head-read or validation reason of 9.3; `publish_failed` when a failed final head write set it (`batch.md` 2.11 step 3); or null for any other code section 9.2 decided; null on a snapshot |
| `detail` | string or null | on the `exit` line, at most 1,024 bytes as written, escaping included (it is cut at a code point boundary before escaping, so that its escaped form fits): for `bad_spec`, its rule and pointer (below); for a 69 from a store request, its last try by 1.6's table, in a fixed form that holds no text from a body and no secret: `connect` (`tls` for a failure rustls reports, 1.4), `no_response`, or `status <HTTP status>` and, when the body named one of 1 to 64 ASCII letters and digits, its S3 error code (`status 403 AccessDenied`), then `truncated` when the body failed or passed `timeout_ms` (1.6; `status 200 truncated`); for a stop, its `stop_detail` (7.3); for `--check` and a valid spec, `memory_bound <bytes>`, `batch.md` 5.1's bound for the spec with every `{"env": NAME}` value at its 16 KiB cap and the main thread's environment block at 6 MiB, Linux's cap on `execve`'s arguments and environment (three quarters of `_STK_LIM`, `fs/exec.c`), since `--check` reads no environment and counts the block at its cap (a memory cgroup's limit is provisioned at `(1.5 x it + 64 MiB x (threads + 2) + 10 MiB x (2 x download_concurrency + 1)) / (1 - vm.dirty_ratio)`: the figure `batch.md` 2.7 asks of `RLIMIT_AS`; the buffers of each live socket, which a v2 cgroup charges, at most `tcp_rmem`'s and `tcp_wmem`'s maxima, 6 MiB and 4 MiB by Linux's defaults, and no orphan's unsent bytes, since a socket a timer drops closes with `SO_LINGER` 0 (`batch.md` 2.5); and the spool's dirty pages, which the kernel keeps under `vm.dirty_ratio` of the cgroup's memory, reclaiming its clean ones under the limit, `batch.md` A7); else the reason's detail (9.3) or null; null on a snapshot |

- The first line is written when the threads start (`batch.md` 2.11 step 7), and then a line at least every 2 s while stdout is a pipe, a FIFO, a socket or a terminal that the caller keeps reading (a line a full pipe holds back is the caller's own backlog); to a regular file or a block device the host's writeback can delay a write (`batch.md` section 8). The lines come from a one-slot mailbox (`extract.md` item 5). No field holds a URL, a header value or a credential (E42). A reader ignores a last line with no `\n`: an `_exit` can cut the line being written.
- A run that ends before the progress thread exists (a spec refused at validation, a 69 before that thread's spawn), and `--check` either way, write one line: the `exit` line, with `seq` 0, written by the main thread (`batch.md` 2.11); for a failure at its step 1, before validation, `phase` is `validate`. A main-thread panic writes none (70).
- **`bad_spec`'s detail** is `<rule> <JSON pointer>`: the rule broken, then the RFC 6901 pointer of the value refused, empty for the document itself, when the detail is the rule alone, with no space, and, for `missing`, the pointer the absent member would have; only the pointer is cut to fit, and a cut pointer ends in `~`, which no whole one does (RFC 6901 writes `~` as `~0`). A bad command line (an unknown command or option, an argument missing or extra) is `usage`, the rule alone. The parser reports the first error in this order. First the document's own rules: `unreadable` (a `JOB.json` missing, or one that cannot be opened or read), `not_regular`, `too_large`, `not_utf8`, `syntax`. Then `spec_version` (`duplicate_member`, `missing`, `type` or `version`, an unknown version). Then each other value in document order (an absent member at its object's end), by the first it breaks of `duplicate_member`, `unknown_field`, `missing`, `type`, `range` (a length, a count, or a string outside its field's values among them), `format` (a pattern, a URL's form, a date, a digest) and `refused_header` (1.2). Then the cross-field rules, in this order, each with its pointer: 1.1's, in its list's order, `budgets`, `/max_decoded_bytes`; `duplicate_id`, the second occurrence's `id`; `duplicate_header`, the second of the two names, under its bundle's `headers`; `backoff`, `/http/backoff_max_ms`; `deps`, the `deps` list; `view_size`, the input whose view breaks it; `spool_in_sink`, `/spool/dir`; `path_style`, `/sink/endpoint` for an address, `/sink/bucket` for a bucket's name; `deadline_required`, `/deadline`; `unknown_input`, the `prefer` entry's `input`; `template`, the template; `key(<rule>)`, the literal key, its rule 2.3's; `archive_key`, the later of the two keys; `archive_kind`, the later member's `kind`; then `source`, a bundle with both or neither of `path` and `url`, the bundle (1.2); `headers_without_url`, the bundle's `headers` (1.2); `outputs_empty`, `/outputs`, and `prefer_empty`, a Prefer whose fields are both null, the entry (1.5). The detail holds nothing else: never the parser's message, which may quote a value (E42).
- With `detail` so bounded, every `exit` line is under `PIPE_BUF` (4,096 bytes).
- `unityrs --version` prints one line, not NDJSON: `unityrs <crate version> spec <highest spec_version>`.
