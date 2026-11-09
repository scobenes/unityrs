# P1b plan: 2D batch extraction (draft)

**Status: draft, design round 2 (2026-11-09), not reviewed.** Written 2026-11-09, when `docs/DESIGN.md` section 5 gave the 2D work a phase of its own. Design round 1 (2026-11-09) settled section 3's items, corrected the requirements its engine design changes, and added E57 to E65. Design round 2 (2026-11-09) folded in round 1's review: sealed attempts in the engine, the exact contract moved to `docs/BATCH_SPEC.md`, and E66 added. Design rounds' findings are cited by what they say. Nothing here is built. This plan gets its own review rounds, run until a round finds no MEDIUM or HIGH problem, before any of it is.

**What it builds on.**
- The core as merged in P1 (`docs/plans/core.md`), through the properties its section 4.1 lists. A need those properties do not cover is a request to the core, made through its integration rule, not an assumption.
- The batch engine, whose design is `docs/plans/batch.md` section 2: sealed attempts and their admission, views, copy-out to the spool, the winners, the stop path and refusal. This plan states what it needs from the engine (sections 1.3, 1.8, 1.13) and does not design it.
- The contract, `docs/BATCH_SPEC.md`: the spec's fields, templates and keys, identity and order, the NGUI cutter, the spool, the manifest, the zip bytes, the exit codes and reasons. A requirement here that names a field, a rule or a code takes its exact form from there.

**What it delivers.** `unityrs batch JOB.json`: one job of N bundles plus their dependencies, decoded in parallel under explicit budgets, with textures, sprites and NGUI atlases written as PNG files and atlas JSON to a directory, zip archives or an S3-compatible store, and a run manifest that says what was written, what was skipped and why. A release job publishes the binary. The first production consumers run on this phase; the plan names none, and their own acceptance runs (L10) are theirs.

**How requirements are written.** Each is numbered (E1, E2, ...). Each names the level it is checked at (`docs/DESIGN.md` section 6) and, once the plan is reviewed, the test that checks it. Where UnityPy 1.25.2 has the behaviour, UnityPy (or the oracle stack, `docs/DESIGN.md` section 3) is the reference. Where it has none (the sinks, the manifest, the cutter's JSON), the reference is the rule written here. Requirements added in design round 1 take the next free numbers (E57 on) and sit in the section they belong to.

---

## 1. Requirements

### 1.1 The job spec and the CLI

- **E1.** A job is a JSON document with a `spec_version`. The binary refuses a version it does not know, before any work, with a named error. It also refuses a spec over 16 MiB, an unknown field at any level, and a spec its schema rejects (section 3, item 4). Version 1's fields, types, defaults, ranges and cross-field rules are `docs/BATCH_SPEC.md` section 1.
- **E2.** `unityrs batch JOB.json` runs one job. `unityrs --version` prints the crate version and the highest `spec_version` the binary reads. `unityrs batch --check JOB.json` validates a spec without running it.
- **E3.** Exit codes tell apart: complete; complete with object-level skips; incomplete for a data reason; a budget refusal; stopped; could not start; an internal error; a host failure; a bad spec. Their values, their precedence and the mapping from each outcome to a code are `docs/BATCH_SPEC.md` section 9, fixed and never reused for another meaning. None is 1 or 2, which shells and argument parsers use.
- **E4.** Progress is NDJSON on stdout; its fields, types and phase values are `docs/BATCH_SPEC.md` section 10 (section 3, item 5). Its lines are at most 2 seconds apart from start to exit, so a caller can treat 5 seconds of silence as a hung run. Each line is a snapshot, and the last is an `exit` line, written as `docs/plans/batch.md` 2.11 says; when it is missing, the exit status is authoritative. Producers never wait on stdout: a parent that stops reading cannot delay the deadline or a signal. Its schema is versioned with the spec.
- **E5.** The budgets of section 1.13 are required fields; a spec without them is refused (E1's error). Version 1 has no autodetection.

Levels: L9 for the CLI's behaviour on synthetic specs.

### 1.2 Inputs and downloads

- **E6.** A bundle (an input or a dependency) is a local path or a URL, with an optional expected `size` and `sha256` (`docs/BATCH_SPEC.md` 1.2). A mismatch fails the bundle (`integrity_mismatch`). A local file must not change during the run: every attempt whose view holds it reads it again (`docs/plans/batch.md` 2.2). Preflight records each local file's size, modification time and inode, and phase 7 (after the last extraction round, before the winners) checks them again; a change fails every input whose view holds that file (`source_changed`), so nothing is published from it.
- **E7.** A URL takes headers, a per-request timeout and a retry policy: which statuses and errors retry, the backoff, `Retry-After`, the number of attempts, and redirects, exactly as `docs/BATCH_SPEC.md` 1.4 defines them. No retry starts when its backoff and timeout would pass the deadline less the grace (E45), and a stop cancels a request in flight. A redirect to another origin fails the download, so a header never reaches another host. A header value that carries a secret is given by reference to an environment variable, never in the spec (E42).
- **E8.** `download_concurrency` bounds parallel transfers: downloads, and in the publish phase uploads.
- **E9.** The spec may give a URL rule that fetches a dependency by name (`docs/BATCH_SPEC.md` 1.3). A name found only in an AssetBundle's own dependency list (`m_Dependencies`) is fetched by that rule in a discovery round between extraction rounds, with no attempt running, and joins the view of the input that lists it (`docs/plans/batch.md` 2.3). The name is the bundle's id, matched to declared ids exactly; any other name must meet the discovered-name rule (no empty, `.` or `..` segment, at most 1,024 bytes), or the input that lists it fails, `invalid_dependency_name` (`docs/BATCH_SPEC.md` 4.1). More distinct names than `max_discovered_bundles` stop the run before any fetch (`discovery_limit`, exit 5). Without a URL rule, such a name is reported in the manifest (`undeclared_dependencies`), never fetched, and stays out of the view.
- **E10.** A download that fails after its retries fails the input (or, for a dependency, every input whose view holds it: E13). A body shorter than its `Content-Length` is a failed download. The run is then `incomplete`.

Levels: L9, against a local test server that serves, delays, stalls mid-body, fails by status, truncates and sends `Retry-After` on command.

### 1.3 Dependencies and isolation

- **E11.** The spec carries a dependency graph: for each bundle, input or dependency, the ids it needs (`deps`). An input's **view** is the input itself, the closure of its list through the graph, and, with a URL rule, the names its `m_Dependencies` lists and their closure (`docs/BATCH_SPEC.md` 4.2). A view over `max_view_bundles` fails its input (`view_too_large`).
- **E12.** Under `EnvOptions::batch()`, an input's `.resS` reads and its container read its own bundle's same-named CAB first (core plan section 9, Q12; `docs/plans/batch.md` 3.9). `EnvOptions::unitypy()` keeps UnityPy's one registry.
  - The rule lives in the engine until the core's `own_bundle_first` option (batch.md section 6, CR3) replaces it.
  - The resource candidates are taken in UnityPy's order, each looked for in the reading file's own container chain (its container, then each enclosing one) before the registry.
  - A pointer reads the reading file's own bundle first (Tier A), then the input, then, among the other bundles of the view, the one whose id is greatest in bytewise order (`docs/BATCH_SPEC.md` 4.3).
- **E13.** "Needs" means "is in the view" (E11). A dependency that fails to download or to open fails exactly the inputs whose view holds it, before their first attempt or at discovery (`docs/plans/batch.md` 3.10). A read that fails for a dependency's own bytes (a corrupt block or entry, found at read time) is an object-level skip of the input that read it, not a failed dependency. A view member whose load in an attempt is refused or fails on its bytes fails the input with `dependency_failed(<id>, <reason>)`, whose exit code follows the inner reason: 5 for a budget or limit, 4 otherwise (`docs/BATCH_SPEC.md` 9.2). An I/O error reading a member's file is a host failure (exit 74).
- **E65.** A pointer or resource read whose CAB no bundle of the input's view holds is an object-level skip, `outside_view(cab)`. The skip and its reason are the same whether another bundle holds that CAB or none does.
- **E14.** Each attempt opens each bundle of its view at most once per residency in its own dependency cache, within `dep_cache_bytes`, and reopens it after eviction. No dependency is shared between attempts, so a dependency that N inputs read is opened up to N times (`docs/plans/batch.md` 1 and 2.2). The manifest counts every open and reopen.
- **E58.** An input's records (its outputs and their bytes, its skips, its failure) are a function of its bytes, the bytes of the bundles in its view, and the spec. They do not depend on `threads`, the order inputs run or finish in, what else is in flight, or which other bundles failed or exist (`docs/plans/batch.md` 1 and 2.10). A host failure stops the run instead (exit 74). Its test is `docs/plans/batch.md` 2.10's, with the nightly's second run (section 5).

Levels: L1 (env) and L9, on testkit scenes.

### 1.4 Decoding

- **E15.** BC7, DXT1, DXT5, RGBA32 and RGB24 decode bit-exact, with UnityPy 1.25.2's output modes for textures with no platform swizzle or byte swap: `RGB` for RGB24, `RGBA` for the other four (`PARITY.md` section 17's rows for these five).
- **E66.** Selection. What one input exports, and which textures are skipped with a named reason (other formats, crunched forms, Switch-swizzled and Xbox 360 textures, a side of 0), is `docs/BATCH_SPEC.md` section 3. A skipped texture is never decoded as another format.
- **E16.** `meshdecode`'s vertex-data unpack (every VertexFormat table, index formats and topologies, `get_triangles`), in f64, as `PARITY.md` section 15.3's rows marked P1b state. CompressedMesh and PackedBitVector are P2a.
- **E17.** Sprites render as UnityPy's `SpriteHelper` renders them (`PARITY.md` section 15.2): atlas render data, tight meshes, rotation, flips and the alpha merge. The affine-triangle path is Tier A if section 3, item 6's port is exact. Otherwise it is a Tolerance row with item 6's metric, whose threshold is measured and stated in `PARITY.md` before E17's test is written.
- **E18.** Every decoder runs under `catch_unwind`. A panic is an object-level skip, recorded in the manifest, never a crashed run.

Levels: L5 on checked-in fixtures for the five formats and every sprite path; L5 on the nightly private corpus.

### 1.5 Images

- **E19.** A PNG writer byte-equal to Pillow 9.5.0's at its defaults, for modes `L`, `LA`, `RGB` and `RGBA` (`docs/DESIGN.md` section 4's `unityrs-imageio` row, L5b).
- **E20.** The visible-pixel hash, exactly: SHA-256 over the ASCII bytes `"{width}x{height}:"`, then the image's RGBA bytes in row order, where each pixel of alpha 0 is written as (0, 0, 0, 0). An image in another mode is converted to RGBA first, as Pillow 9.5.0's `convert("RGBA")` converts it.

Levels: L5b on checked-in fixtures for every mode and on the nightly corpus; E20 by a reference implementation in the parity tools, run on the same images.

### 1.6 The NGUI atlas cutter

- **E21.** Sheet scope `own_files_by_name`: the material's main-texture name, then the atlas name, matched among the input's own Texture2D objects only. A PPtr may cross into a dependency to read a name, never to take its texture. `follow_pptr` is not in version 1. How an atlas is recognised, which fields hold its sprites, the crop geometry and every tie rule are `docs/BATCH_SPEC.md` section 5, which is the specification item 3's reference is written from.
- **E22.** A caller-given list of name suffixes (`skip_name_suffixes`) skips reference atlases.
- **E23.** A crop past the sheet's edge is padded with zeros, as Pillow 9.5.0's `Image.crop` pads.
- **E24.** The atlas JSON is byte-equal to CPython 3.11's `json.dumps(value, indent=2)` (its defaults otherwise: ASCII escapes, the float `repr`, `NaN` written as `NaN`) of the object `docs/BATCH_SPEC.md` section 5 defines: keys `name`, `texture_width`, `texture_height`, `pixel_size`, `settings`, `sprite_count`, `sprites`, in that order, with typetree values written as read (pairs as arrays, f32 widened exactly, strings as UnityPy decodes them).
- **E25.** A value the layout cannot write (bytes) skips that atlas's JSON alone, recorded in the manifest. The crops stay.
- **E26.** Collisions are handled per atlas, and the atlas's winner also wins its sheet texture (`docs/DESIGN.md` section 4's `unityrs-export` row).

Levels: L5 and L5b for the crops, L7 for the JSON, on testkit-built atlases (an NGUI-shaped MonoBehaviour typetree is synthetic) and the nightly corpus.

### 1.7 Output naming

- **E27.** Key templates per output kind come from the spec. A variable is written verbatim, and its `safe_` form through `safe()`. The grammar, the variables, their spelling and where each is allowed are `docs/BATCH_SPEC.md` 2.1. Version 1 has no container path: a scope decision, not an impossibility (the input's own container rows are readable); it can come in a later version as an optional variable.
- **E28.** `safe()` works per code point, an invalid UTF-8 byte counting as one: every code point outside `[A-Za-z0-9._-]` becomes `_`; then every leading and trailing `.` is stripped; then an empty result becomes `unnamed` (`docs/BATCH_SPEC.md` 2.2).
- **E29.** Inside one input, the first object of a given type and name, in object order (`docs/BATCH_SPEC.md` 4.4), takes the key, even when its decode then fails. A later one is skipped, `duplicate_name`. Of two outputs of one input with one key, the first in (kind, object order) is the candidate and the later one is `duplicate_key`.
- **E30.** Inside one atlas, of two sprites with one `safe()` name, the later one wins.

Levels: L9.

### 1.8 Winners across inputs

- **E31.** When several inputs produce one key, the spec's `prefer` map, `{key: {input, md5}}`, decides:
  1. the candidate from the preferred input, if that input has a candidate for the key;
  2. otherwise the candidate with the preferred MD5, and of several such the first by bytewise input id, not reported as a flip;
  3. otherwise the first candidate input in bytewise order of input id, reported in the manifest as a **flip**.
- **E32.** An atlas, its crops, its JSON and its sheet's texture output win as one unit. The unit's `prefer` entry is its unit key's: the sheet's texture key, or the JSON key when textures are not exported (`docs/BATCH_SPEC.md` 4.5). This is the cross-kind rule.
  - Proposed for review; the exact steps are `docs/BATCH_SPEC.md` 4.5 and 4.6. Units are decided first, per unit key.
  - Every key of a winning unit is then reserved: a plain candidate for it loses, reported with the rule `unit`.
  - When two winning units claim one key, the unit whose unit key is first in bytewise order keeps it. The other unit is withheld whole (`unit_conflict`). A unit with a key refused for `prefix_conflict` or `archive_conflict` is withheld whole too. So no unit is ever published in part.
  - Each of these checks runs once, so phase 8 decides them in bounded memory (`docs/plans/batch.md` 2.9): a key a withheld unit claimed stays reserved.
- **E33.** "Pixels differ" means a different visible hash (E20). A collision whose candidates share a visible hash is reported, but not as a pixel difference.
- **E34.** The winners hold candidates until every input that can produce a key has run: a sink that holds output across jobs. `docs/plans/batch.md` 3.5's model allows it: candidates are copied out to the spool, and winners are decided after the last extraction round, as a pure function of the committed records.
- **E60.** All or nothing per input. An input that fails commits no candidate and competes for no key. Only its failure record reaches the manifest.
- **E61.** Withholding. A key whose `prefer` entry names an input that failed in this run is not published (`withheld`). Its rule-2 or rule-3 winner could otherwise overwrite what the next complete run publishes. A key with no such entry is published, even when a failed input might have won it by rule 3: a choice, since withholding every key a failed input could produce would withhold most of a run for one failure. A spec option `publish_incomplete` (default `true`) set to `false` publishes nothing when any input failed.

Levels: L9.

### 1.9 Sinks

- **E35.** Directory.
- **E36.** Zip archives, written to the job's one sink as one more object each: stored members; member paths and their order from the spec's `archives` (`docs/BATCH_SPEC.md` 1.7); streamed from the spool's members, never copied whole; no archive written that would have no member. Their bytes are fixed by `docs/BATCH_SPEC.md` section 8, so an archive's MD5 is the same on every run with the same members. There is one spool for downloads, candidates, records, archives and the manifest, with one size cap and one free-space reserve, checked before every claim (`docs/BATCH_SPEC.md` section 6).
- **E37.** Object store (S3-compatible):
  - credentials from the environment only, a session token included; no instance-metadata fallback. The engine always passes its own static credential provider and refuses to start without credentials;
  - path-style addressing available (bucket names with dots break virtual-hosted TLS);
  - Content-Type set on every PUT (`image/png`, `application/json`, `application/zip`, `application/x-ndjson`), and no other header the engine chooses: no metadata, ACL, cache-control or checksum header. Signing headers, the session token's header, `Content-Length`, and E38's `If-Match` and `If-None-Match` are the protocol's;
  - the client's own timeout and retries are set from the sink's `timeout_ms` and `attempts` (`docs/BATCH_SPEC.md` 1.6), and a stop cancels a request in flight. A `CompleteMultipartUpload` retried after a lost response that meets `NoSuchUpload` is checked with a HEAD: the expected ETag there means it completed;
  - files up to 16 MiB single-part, so the ETag is the MD5; larger ones multipart in 16 MiB parts, like archives, compared by their expected multipart ETag (E43). Under SSE-KMS or SSE-C the single-part ETag is not the MD5; the engine reports `etag_not_md5` instead of failing, and resume then re-PUTs;
  - archives multipart at 16 MiB parts; a failed or interrupted multipart upload is aborted, on SIGTERM too (E64);
  - keys written byte for byte as given, or refused with a named error, never rewritten. Section 3, item 1 names the refused classes. Empty segments, `.` and `..` segments, a leading or trailing `/`, control characters, keys that are not UTF-8 and keys over 1,024 bytes are refused. `%`, `#`, `~`, spaces and non-ASCII are written as given.
- **E62.** Directory-sink safety. Each file is written as a temp file beside its destination and renamed over it. One `syncfs` of the output file system runs before the final manifest, so the manifest never claims a file a power cut could lose. Paths are opened beneath the root (`openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` where available, else per-segment `O_NOFOLLOW`), so neither a key nor a planted symlink can escape it. A segment over 255 bytes (`NAME_MAX`) is a refused key. At start, leftover temp files of any run (`.unityrs-tmp-*`) under the root are removed: the root's one lock (E38) proves no live run owns them, and no key may have a segment starting with `.unityrs-tmp-` (`reserved_name`), so no output is mistaken for one.
- **E64.** Each multipart upload's id is appended and synced to the spool's uploads journal after its create and before its first part, and removed after it completes or aborts. The stop path closes the journal and aborts every id in it, and the next attempt aborts the ids a killed one left (`docs/BATCH_SPEC.md` section 6). The operator's runbook names the bucket's lifecycle rule for uploads left by a lost spool.

Levels: L9 against a local S3-compatible test server, with its request log as the evidence for headers, addressing and keys, and a fault-injecting store (5xx, `SlowDown`, a hang, a conditional-write conflict, a lost complete response, ETags that are not MD5s).

### 1.10 The run manifest

- **E38.** The manifest's first write is status `running`. It carries the `run_id`, the attempt number from the spec's required `attempt` field (an integer the caller raises for each attempt), the binary's version, the `spec_version`, the start time and the deadline. No output is written before it.
  - Its location is fixed by the `run_id`: a small head, `_unityrs/<run_id>/manifest.json` under the sink's root, and per-attempt row files beside it (`docs/BATCH_SPEC.md` 7.1). So runs with different `run_id`s never fence each other.
  - Each head write is fenced on (`run_id`, `attempt`). An attempt that finds a head of its `run_id` with an attempt number at or above its own, in preflight or later, stops with exit 6 (`superseded`) and writes nothing more.
  - In an object store each head write is a conditional PUT (`If-None-Match: *`, then `If-Match`), and a failed precondition is read back before it counts as superseded, so a lost response is not taken for a rival (`docs/BATCH_SPEC.md` 7.2).
  - A store without conditional writes needs `manifest_fencing: "none"`, and the caller then runs one attempt at a time.
  - The directory sink fences with one lock for the whole output root (`_unityrs/lock`), so one process at a time writes a root, and a rename.
  - The head is the record of the run: a key it does not list as `written` or `present` is not the run's output, whatever the store holds.
- **E39.** Its last write replaces it with the final status: `complete`, `complete_with_skips` or `incomplete`. It is written only after every publish was acknowledged, and after E62's `syncfs` for a directory.
- **E40.** The manifest records:
  - **per output:** key, kind, source input, path id, MD5, visible hash, mode, size, and a status of `written`, `present` (resume found the same bytes), `withheld`, `failed` or `unwritable_key`;
  - **per collision:** the key, each candidate (input, MD5, visible hash), the winner and the rule that chose it, flips marked;
  - **per input:** status, failure reason, round, its Input and Dependency peaks, timings, object-level skips;
  - **per archive:** status, and each member's path, key, source input and MD5, or why it was left out;
  - **for the run:** `same_named_cabs`, `undeclared_dependencies`, `nested_containers`, opens, evictions and reopens, the peaks of the inputs' and dependencies' budgets, the spool's peak, peak RSS (`VmHWM`), the bound batch.md section 5 computes for the spec, the budgets as read, and the stop reason.
  - Every field, row file and reason code is `docs/BATCH_SPEC.md` sections 7 and 9.3. A limit refusal's reason names the limit and its configured value, never the amount asked for.
- **E41.** In an object store, Content-Type `application/json`.
- **E42.** No credential appears in the manifest, the progress stream, stderr or any error message. That covers a URL's query string and userinfo, and every header value. A URL is never printed with its query string, and an HTTP client's error is printed without its URL.

Levels: L9, with a test that runs every sink with known credentials, signed URLs and secret headers, and searches every byte the run wrote or printed for them.

### 1.11 Resume and idempotence

- **E43.** A re-run with resume LISTs the output root once and skips a PUT whose key already holds the same MD5 (an archive's, or a file's over 16 MiB: its expected multipart ETag at 16 MiB parts). The LIST is merged with the sorted winners one page at a time, so its memory does not grow with the store (`docs/plans/batch.md` 2.9). A LIST that fails, for example on a foreign key `object_store` cannot parse (section 3, item 1), degrades resume to "PUT everything" with the warning `resume_list_failed`; it does not fail the run. In a directory, resume hashes an existing file only when its size matches.
- **E44.** Every input is still read and decoded, so winners, archives and the manifest cover the whole job. There is no partial ("narrowed") re-run.

Levels: L9: a run killed at each progress line, then resumed, writes the same keys and bytes as an uninterrupted run and leaves no open multipart upload.

### 1.12 Lifetime

- **E45.** A wall-clock `deadline` in the spec is the time by which the process has exited. The engine converts it once to a `CLOCK_BOOTTIME` instant and starts stopping at the deadline less `shutdown_grace_ms` (default 20 s). The stop path cancels every request in flight, aborts open uploads, writes `incomplete` and exits without joining its threads; a watchdog calls `_exit` at the deadline whatever is unfinished (`docs/plans/batch.md` 2.11). Only a filesystem call that never returns can hold the process past it (batch.md A11).
- **E46.** The process exits when its parent dies (Linux `PR_SET_PDEATHSIG`, set by the binary itself, then a `getppid()` check against the spec's `parent_pid`, or against 1 when it is not given) and, when the spec asks, when stdin reaches EOF. Callers are told that Linux sends the signal when the *thread* that forked the child exits: a caller that spawns from a pool thread that may exit should use the stdin rule.
- **E47.** SIGTERM, SIGINT and SIGHUP: abort open multipart uploads, write `incomplete`, exit 6, all within the grace. A second signal during the stop exits at once.
- **E63.** Before the manifest's first write, a preflight takes a non-blocking `flock` on the spool's lock file, so two attempts never share a spool, and, for a directory sink, on the output root's. It also checks the spool's free space against the reserve, checks `RLIMIT_AS` and `RLIMIT_DATA` against the floor `docs/plans/batch.md` 2.7 derives from the memory bound, reads the credentials, checks that the sink is reachable, checks each local bundle's `size` and `sha256`, and reads the stored manifest head for fencing. A preflight failure exits 69 with nothing published; a head that supersedes this attempt exits 6.

Levels: L9.

### 1.13 Budgets

- **E48.** `threads`, `download_concurrency`, `max_inflight_uncompressed_bytes`, `max_decoded_bytes`, `dep_cache_bytes`, the spool directory, the spool size and the spool's free-space reserve are required (E5). Each has one meaning (`docs/plans/batch.md` 2.2):
  - `max_decoded_bytes` (`M`) is every meter's `max_input_memory`, an input's and each dependency's alike, and each attempt's Input budget; every meter's `max_decode_work` is four times it, the core's default ratio;
  - `dep_cache_bytes` (`D`) is each attempt's Dependency budget and the bound of its private dependency cache, registry included;
  - `max_inflight_uncompressed_bytes` (`I`) bounds the slots of the attempts in flight (`M + D` each, or `M` for an input whose view is itself alone), and is the working set of the winners' sort, the resume merge and the transfers' buffers;
  - `download_concurrency` bounds transfers (E8).

  A spec is refused unless `M <= D`, `M + D <= I` and `I >= 64 MiB`. Every field, required or optional, and its default is `docs/BATCH_SPEC.md` section 1.
- **E49.** How the budgets bound memory is `docs/plans/batch.md` section 5: `max_inflight_uncompressed_bytes` plus constants the spec fixes, with the spool bounded on disk by its size and reserve.
- **E59.** Every buffer the engine allocates in proportion to an input or an output (pixels, masks, PNG rows and output, atlas JSON, the attempt's view, its record buffer, zlib's deflate state) is charged to the job's meter, or to the phase budget of `docs/plans/batch.md` 2.9, before it is allocated, at `heap_cost`: the core's rule (core plan 3.15) applied to engine code. A job exports one sheet at a time, so its export buffers are bounded by its largest sheet.
- **E57.** On glibc, the binary sets `M_MMAP_THRESHOLD` to 128 KiB and `M_ARENA_MAX` to `threads + 2` at start, so RSS follows the charged heap (`docs/plans/batch.md` 5.4), and reports peak RSS.

Levels: L9, with the counting-allocator and squeeze rows of the gate (section 5).

### 1.14 Release CI

- **E50.** A tag builds the CLI for x86_64 Linux, glibc-linked against glibc 2.28 at most (the manylinux_2_28 image), with the vendored C libraries linked statically. Not musl: the core's memory meter charges in glibc's allocator chunks (`heap_cost`, core plan section 3.15), so the meter's bound holds only under glibc's allocator. The release profile keeps `panic = "unwind"`, the default, which E18's `catch_unwind` needs.
- **E51.** The tag publishes it as a GitHub release asset, with its SHA-256.
- **E52.** A release that a consumer may reference is never deleted: release tags are protected by a repository rule that forbids deleting or moving them, and the release job never deletes a release.

Today `ci.yml` runs on pushes to main and on pull requests only; the release job is new.

### 1.15 Verification

- **E53.** L5 and L5b run in CI on checked-in fixtures for the five formats, sprites and every PNG mode.
- **E54.** L9 (the extractor tree) for the 2D outputs, in CI on testkit-built inputs.
- **E55.** The nightly private corpus run covers L5, L5b and the atlas JSON, against the oracle stack's output for the same inputs, with section 5's pass rule.
- **E56.** L10, a consumer's shadow run, is the consumer's: it gates that consumer's cutover, not this phase.

---

## 2. Decided here

| decision | why |
|---|---|
| Lossy WebP moves to P3 | No first consumer needs it, and it brings `libwebp-sys` and its licence review into the phase for nothing the phase ships. |
| CompressedMesh and PackedBitVector move to P2a | Sprites need the vertex-data unpack, not the compressed mesh; `PARITY.md`'s rows for the two now say P2a. |
| The release is glibc, not musl | E50's reason: the memory meter's unit is glibc's chunk. |
| No narrowed re-run | E44: a re-run that skips inputs cannot choose winners, fill archives or write a whole manifest. |
| Sealed attempts | `docs/plans/batch.md` 2.2: no outcome may depend on what runs beside an input, and a shared dependency cache could not promise it. The cost is reserved memory and a dependency decoded once per input that reads it. |
| No `{container}` and no NGUI 2 layout in version 1 | E27 and `docs/BATCH_SPEC.md` section 5: scope, to keep version 1's contract small; either can come in a later version as an optional field. |

## 3. Section 3's items, settled in design round 1 (for review)

Each item gives the recommendation and its reason. None is decided until this plan's review rounds end.

### Item 1. Verbatim keys and `object_store`

**Recommendation.** Keep `object_store` (0.12). Write a key only when `Path::parse(key)` gives back exactly `key`, the key is non-empty, valid UTF-8 and at most 1,024 bytes. Refuse any other key with `KeyRefused { key, rule }`, never rewriting it. `Path::from` and `from_iter` are never used.

**What `object_store` 0.12.5 does**, read from its source in this design round:
- **`Path::parse`** (`src/path/mod.rs`, `src/path/parts.rs`):
  - strips one leading and one trailing `/`, silently, which is a rewrite;
  - refuses an empty segment, a segment that is `.` or `..`, and any ASCII control character;
  - keeps `%`, `#`, `~`, spaces, non-ASCII and other punctuation verbatim.
- **`Path::from` and `from_iter`** drop empty segments, percent-encode `%`, `#`, `~` and other reserved characters, and turn `.` and `..` into `%2E` and `%2E%2E`. Those are rewrites.
- **On the wire**, the S3 client percent-encodes the key once, every byte outside `A-Za-z0-9-._~/` (`src/aws/client.rs`), and the store decodes it once. So the stored key is the `Path`'s string, byte for byte.
- **`Path` is a `String`**, so a key that is not UTF-8 cannot be expressed.
- **LIST** parses every key it returns with `Path::parse` (`src/client/s3.rs`). One foreign key under the root that it refuses fails the whole listing (E43's degradation).
- **Credentials.** Without explicit ones, the builder falls back to instance metadata (`src/aws/builder.rs`), so the engine never lets it choose (E37).
- **Content-Type.** Unless set, it is inferred from the extension, so the engine sets it on every PUT.

**Refused classes** (the `rule` of the error), each with an L9 test that reads the test server's request log:
- `empty`;
- `leading_slash`, `trailing_slash`;
- `empty_segment`;
- `dot_segment`, a segment that is `.` or `..`;
- `control_char`;
- `not_utf8`;
- `too_long`, over 1,024 bytes, S3's own limit.

**What each choice cannot write.**
- `object_store` cannot write any of the first six rules, `empty` to `control_char`.
- A hand-written SigV4 client over `reqwest` could add empty segments, slashes at the ends and control characters (the last needing `encoding-type=url` on LIST), but not dot segments. The `url` crate, which every `reqwest`-based client builds on, removes `.`, `..` and their `%2e` forms as dot segments, per the WHATWG URL standard (`url` 2.5.8, `src/parser.rs`).
- No client can write the last two, since S3 refuses them.
- A second HTTP stack, with our own signing, multipart, retries and credentials, is not worth keys no template should produce.

**Where a refusal lands.**
- A template's literal parts are checked at validation: a bad spec (exit 78).
- A key built from a verbatim name is checked when its output is made. The output is an object-level skip (`unwritable_key`), named in the manifest.
- `safe()` never makes a shape refused by its characters: it maps `/` and control characters to `_` and strips dots at the ends. A long name can still make a segment over 255 bytes or a key over 1,024, which is `unwritable_key`.
- The same rules hold for every sink: `segment_too_long` (`NAME_MAX`), `reserved_prefix` and `prefix_conflict` (a key that is also another winner's parent directory, which a directory cannot hold) are refused everywhere, the last decided in phase 8 so it does not depend on publish order (`docs/BATCH_SPEC.md` 2.3 and 4.6). Zip member paths take the rules too, where they also block escapes from the archive's root. So a key is writable in one sink exactly when it is writable in all of them.

**Reason.** It keeps the one S3 client DESIGN names, and every key it writes is byte-exact. The keys it cannot write, no client could write through `url`, or S3 itself refuses.

### Item 2. The gate

**Recommendation.** Section 5's table: one row per requirement group, with its test, level, fixtures and pass rule, the engine's rows from `docs/plans/batch.md`, the memory rows and the operations rows.

**Reason.** It follows P1's gate form. It adds what this phase needs that P1's lacked: fault-injecting servers for the network rules, kill-and-resume for E43 and E44, and memory rows that check the bound and not only the pixels.

### Item 3. The cutter's reference

**Recommendation.** E21 to E26 are the specification. The reference is `tools/parity/ngui_cutter.py`, written from E21 to E26 (not ported from the Rust, and by someone other than the Rust's author), and reviewed with this plan.
- It reads through UnityPy 1.25.2, crops with Pillow 9.5.0's `Image.crop` (whose zero padding is E23's definition), writes PNG with Pillow's `save`, and writes JSON with CPython 3.11's `json.dumps(value, indent=2)` (E24's definition). So every byte it compares comes from the oracle stack.
- The only logic of its own is selection: the sheet scope, the reference-atlas pattern, per-atlas collisions and the unit winner. That is small enough to review by reading. Its own tests use testkit atlases with hand-computed expectations.
- Its outputs on the testkit atlases are recorded as fixtures, so CI compares against recordings and needs no oracle image. The nightly runs it live.

**Reason.** UnityPy has no cutter, so the rule must be the reference. Running the rule on the oracle stack keeps everything but the selection logic as exact as the other rows.

### Item 4. The spec schema and its versioning

**Recommendation.**
- **The schema.** A JSON Schema (2020-12) per version, `docs/batch/spec-v<N>.schema.json`, written from `docs/BATCH_SPEC.md` section 1's tables and embedded in the binary. A test runs every fixture spec, valid and invalid, through both the schema and the Rust parser, and requires that they agree.
- **`spec_version`** is an integer, from 1. A binary reads every version from 1 to its highest, and `--version` prints the highest (E2).
- **Strict.** Unknown fields are refused at every level (`additionalProperties: false`, `deny_unknown_fields`). A misspelt budget that is silently ignored is an outage.
- **Adding a field means a new version.** In it, the field is optional, with a default that reproduces the previous version's behaviour exactly. A removal, a rename, a new required field or a changed meaning also needs a new version, and the old number keeps its old meaning.
- **Golden specs per version** (`tests/specs/v<N>/`) run in CI against every later binary, with their outputs and manifests pinned.
- **Pinned consumers.** A consumer pinned to an older binary keeps writing its version, and newer binaries read it the same way. A newer field sent to an older binary is refused before any work (E1), which is the safe failure.
- **No secret is a spec field** (E7, E42), except what a signed URL carries in its query string; E42 keeps that out of every output. The manifest and the progress stream carry the `spec_version` they were produced under.

**Reason.** A hand-written schema is the document consumers read. The agreement test keeps it equal to the parser, and the golden specs keep old versions working.

### Item 5. Exit code values and the progress schema

**Exit codes:** `docs/BATCH_SPEC.md` section 9: 0, 3, 4, 5, 6, 69, 70, 74 and 78, their precedence, the mapping from every outcome class to a code (a budget-caused skip or failure is 5, a depth or count limit is a data reason), and the codes of `--check` and of a second signal.
- **Reason.** "Stopped", "could not start" and "host failure" are retried differently from a data failure, so each has its own code. The values follow `sysexits.h` where one fits. Codes 1, 2 and 64-68 are never used, so a shell or argument-parser error is never mistaken for a run's verdict.

**Progress, version 1** (NDJSON on stdout): every field, its type, the `phase` values and the `exit` line are `docs/BATCH_SPEC.md` section 10. Each line is a whole snapshot, not an event.
- **Rate.** A progress thread writes a line at least every 2 s, from a one-slot mailbox that producers overwrite. So a parent that stops reading stdout stalls only that thread, and an `EPIPE` stops progress but not the run.
- **The last line** is `exit`. A killed process has none.
- **Reasons** are the fixed codes of `docs/BATCH_SPEC.md` 9.3.
- **No secret.** No URL query string, userinfo or header value appears in any field (E42).
- **Reason.** Per-input outcomes are in the manifest, so a dropped snapshot loses nothing, and a snapshot writer can never block the run.

### Item 6. The sprite affine path's tolerance

**Recommendation in two steps.**
1. **Measure exactness first.** UnityPy's `SpriteHelper.copy_triangle` builds a 6 x 6 system and solves it with `numpy.linalg.solve` (`UnityPy/export/SpriteHelper.py`), then samples with Pillow 9.5.0's nearest-neighbour affine transform.
   - The reference is numpy as the oracle runs it: numpy 2.4.6 linked to OpenBLAS 0.3.31 built with `DYNAMIC_ARCH`, whose `dgesv` is OpenBLAS's own factorisation, with kernels chosen by the host CPU at run time. A probe of 200,000 random integer triangles found 43 % whose solution differs in at least one coefficient bit from a reference-LAPACK-order solve. So the reference's bits depend on the oracle's CPU, and the nightly records that CPU.
   - Port the solve in f64 in OpenBLAS's order for that CPU's kernel, and Pillow's coefficient arithmetic and sampling in each path Pillow takes. Compare on the testkit sprites and the corpus. Only if every pixel matches is the row Tier A.
   - UnityPy's numpy-free solver is not the reference: the oracle has numpy and never runs it (its `PARITY.md` row says so).
2. **If it is not exact, use an edge-band metric.** Where a one-ulp difference in the matrix moves a sample, a pixel takes a neighbouring texel, so its channels can differ by up to 255. A per-channel absolute-difference metric is therefore the wrong shape: every real disagreement would fail it. A share of differing pixels alone would hide a wrong triangle. So both must hold:
   - every pixel whose visible RGBA (E20's clearing) differs lies within 1 px (Chebyshev) of an edge of the sprite mesh's triangles, rasterised on the output grid;
   - the differing pixels are at most a share of the sprite's visible pixels. The share is set from the corpus's measured distribution (its maximum, with the headroom stated) and written into the `PARITY.md` row before E17's test is written.

A sprite outside the metric fails its test and is listed, never passed silently. The nightly compares such sprites by the metric, not by their visible hash.

**Reason.** Exactness would remove the row, but it is not likely: the reference's coefficients vary with its BLAS kernel. The edge band has the shape a sampling miss has, and step 2 is the plan of record unless step 1's measurement shows every pixel equal.

### Open for the review rounds

- **E9's fetch** is kept, with discovery between rounds (`docs/plans/batch.md` 2.3). Dropping its second sentence and requiring a complete graph would remove the discovery round. That is the owner's choice.
- **E32's cross-kind and cross-unit rules** are proposed, not decided.
- **`docs/DESIGN.md`'s batch row** says "Emit only changed pixels against a previous manifest", which no requirement covers. Under copy-out it is simple (skip a winner whose visible hash equals the previous manifest's), but it needs an E or must leave DESIGN.
- **E14 and DESIGN's goal**: the row's "each bundle and dependency parsed once per run" is the owner's to restate (`docs/plans/batch.md` section 8). Under sealed attempts a dependency is parsed once per input that reads it.
- **Item 6's threshold**, if the port is not exact, waits on a measurement of the corpus.

## 4. Before it is built

- `docs/plans/batch.md`'s design reviewed, in its own review rounds, with its section 8 settled.
- This plan's review rounds, run until a round finds no MEDIUM or HIGH problem, with section 3 settled.
- Section 5's gate agreed before the first package starts.

## 5. The gate (proposed in design round 1)

| row | covers | test and evidence | level | fixtures | pass rule | runs |
|---|---|---|---|---|---|---|
| X1 | E15 | decode per format against the oracle's pixels | L5 | Editor-built and testkit textures, every size class and mip | pixels and mode identical | CI |
| X2 | E16, E17 | sprites against `SpriteHelper` | L5 | testkit sprites: atlas, tight, rotation, flips, alpha merge, affine | identical; the affine path within item 6 | CI |
| X3 | E19 | PNG against Pillow 9.5.0 | L5b | the fixtures, and seeded random images in every mode | bytes identical | CI |
| X4 | E20 | the visible hash against the parity tools' reference | L5 | X1's and X3's images | equal | CI |
| X5 | E21-E26 | crops and JSON against item 3's reference | L5, L5b, L7 | testkit NGUI-shaped atlases | equal | CI |
| X6 | E27-E34, E60, E61 | naming, winners, all or nothing, withholding | L9 | synthetic scenes | the rules as written | CI |
| X7 | E35-E37, E41, E62, E64 | sinks against a local S3-compatible server's request log, a fault-injecting store, and a directory with planted symlinks | L9 | synthetic | headers, addressing, keys, aborts, no orphan upload, nothing outside the root | CI |
| X8 | E1-E5, E38-E42, E63 | spec, CLI, exit codes, progress, manifest, fencing, preflight, and the credential search (signed URLs and header values included) | L9 | synthetic | as stated | CI |
| X9 | E6-E10 | downloads against a fault-injecting HTTP server | L9 | synthetic | every rule of E7 and E10 | CI |
| X10 | E43-E47 | a run killed at each progress line, then resumed; the deadline, each signal, the parent's death and stdin's EOF in each phase, a download and a transfer in flight among them | L9 | synthetic | the same keys and bytes as an uninterrupted run; exit 6 within the grace; no open upload | CI |
| X11 | E11-E14, E58, E65, E66, `docs/plans/batch.md` | D (batch.md 2.10, round 1's counterexamples included), views, budgets and charging, the deadlock guard and source scans (batch.md section 4) | L1, L9 | engine scenes | as stated | CI |
| X12 | E59 | the core's counting allocator over the export paths | L9 | testkit inputs | heap never above the charges plus batch.md 5.3's constants, `C_base` measured once and pinned in the test | CI |
| X13 | E48, E59 | the squeeze: each testkit input with `max_decoded_bytes` at its peak less one byte, then at its peak, its peak being the largest over its meter and every dependency meter of its run | L9 | testkit inputs | refused the same way each time, then identical to an unbounded run | CI |
| X14 | E55, E57, E58 | the nightly corpus | L5, L5b, L7, L9 | the private corpus | below | nightly |
| X15 | E50-E52 | the release job | CI | a tag build | asset and SHA-256 published | tag |

**X14's pass rule.**
- Every output of every input the oracle script reads matches it on key, pixels, mode, visible hash, PNG bytes and atlas JSON. The only exceptions are rows `PARITY.md` lists as Tolerance, held to their metric.
- Where both sides fail an input, they fail for the same class of reason.
- The run's peak RSS is at or below `docs/plans/batch.md` 5.1's bound for its spec.
- A second run, at `threads` 1 with ample budgets, writes an output tree and per-input records byte-equal to the main run's at `threads` 8 with budgets at 1.2 times the measured peaks (E58).
- Opens, evictions and reopens are recorded and watched, not gated.

The oracle side is `tools/parity/extract_oracle.py`. It runs UnityPy 1.25.2's export of the five formats and sprites with Pillow 9.5.0, item 3's cutter, and the naming and winner rules (E27 to E34), on the same spec. For each input it builds one UnityPy Environment from that input's view alone, loading the other bundles in bytewise order of id and the input last, and treats a reference no loaded file holds as the `outside_view` skip, so the views and the same-named-CAB rule (E12, E65) are on both sides.
