# P1b plan: 2D batch extraction (draft)

**Status: draft, design round 5 (2026-11-09), not reviewed.** Written 2026-11-09, when `docs/DESIGN.md` section 5 gave the 2D work a phase of its own. Design round 1 (2026-11-09) settled section 3's items, corrected the requirements its engine design changes, and added E57 to E65. Design round 2 (2026-11-09) folded in round 1's review: sealed attempts in the engine, the exact contract moved to `docs/BATCH_SPEC.md`, and E66 added. Design round 3 (2026-11-09) folded in round 2's review: staged bundles (E6), no discovery (E9), one stop path and the finisher (E45 to E47), a dedicated spool (E63), a time fence (E38), archives by kind (E36). Design round 4 (2026-11-09) folded in round 3's review: the caller's duties and a verification pass replace the time fence (E38), every input registers itself (E12), keys are planned before export (E29), and one input's export work is bounded (E48). Design round 5 (2026-11-09) folded in round 4's review: the store's per-try classification and Content-Types (E37), a 69 retried with the next attempt number (E38), a stop seen at phase 7's end (E47), the allocator's refusals counted (E58), and section 3, item 1 checked against `object_store` 0.12.5's source. Design rounds' findings are cited by what they say. Nothing here is built. This plan gets its own review rounds, run until a round finds no MEDIUM or HIGH problem, before any of it is.

**What it builds on.**
- The core as merged in P1 (`docs/plans/core.md`), through the properties its section 4.1 lists. A need those properties do not cover is a request to the core, made through its integration rule, not an assumption.
- The batch engine, whose design is `docs/plans/batch.md` section 2: staging, sealed attempts and their admission, views, copy-out to the spool, the winners, the stop path and refusal. This plan states what it needs from the engine (sections 1.3, 1.8, 1.13) and does not design it.
- The contract, `docs/BATCH_SPEC.md`: the spec's fields, templates and keys, identity and order, the NGUI cutter, the spool, the manifest, the zip bytes, the exit codes and reasons. A requirement here that names a field, a rule or a code takes its exact form from there.

**What it delivers.** `unityrs batch JOB.json`: one job of N bundles plus their dependencies, decoded in parallel under explicit budgets, with textures, sprites and NGUI atlases written as PNG files and atlas JSON to a directory, zip archives or an S3-compatible store, and a run manifest that says what was written, what was skipped and why. A release job publishes the binary. The first production consumers run on this phase; the plan names none, and their own acceptance runs (L10) are theirs.

**How requirements are written.** Each is numbered (E1, E2, ...). Each names the level it is checked at (`docs/DESIGN.md` section 6) and, once the plan is reviewed, the test that checks it. Where UnityPy 1.25.2 has the behaviour, UnityPy (or the oracle stack, `docs/DESIGN.md` section 3) is the reference. Where it has none (the sinks, the manifest, the cutter's JSON), the reference is the rule written here. Requirements added in design round 1 take the next free numbers (E57 on) and sit in the section they belong to.

---

## 1. Requirements

### 1.1 The job spec and the CLI

- **E1.** A job is a JSON document with a `spec_version`. The binary refuses a version it does not know, before any work, with a named error. It also refuses a spec over 16 MiB, an unknown field at any level, and a spec its schema rejects (section 3, item 4). Version 1's fields, types, defaults, ranges and cross-field rules are `docs/BATCH_SPEC.md` section 1.
- **E2.** `unityrs batch JOB.json` runs one job. `unityrs --version` prints the crate version and the highest `spec_version` the binary reads, in one line (`docs/BATCH_SPEC.md` section 10). `unityrs batch --check JOB.json` validates a spec without running it.
- **E3.** Exit codes tell apart: complete; complete with object-level skips; incomplete for a data or transport reason; a budget refusal; stopped; superseded; could not start; an internal error; a host failure; a bad spec. Every stop exits with its own reason's code, whoever requested it. Their values, their precedence and the mapping from each outcome to a code are `docs/BATCH_SPEC.md` section 9, fixed and never reused for another meaning. None is 1 or 2, which shells and argument parsers use.
- **E4.** Progress is NDJSON on stdout; its fields, types and phase values are `docs/BATCH_SPEC.md` section 10 (section 3, item 5). Its first line is written when validation ends, and its lines are then at most 2 seconds apart until the exit, so a caller can treat 5 seconds of silence as a hung run. The `exit` line names the reason that set the code (a stop, preflight or validation reason) with its detail. Each line is a snapshot, and the last is an `exit` line, written by the finisher (`docs/plans/batch.md` 2.11); when it is missing, the exit status is authoritative. Producers never wait on stdout or stderr: a parent that stops reading either cannot delay the deadline or a signal. Its schema is versioned with the spec.
- **E5.** The budgets of section 1.13 are required fields; a spec without them is refused (E1's error). Version 1 has no autodetection.

Levels: L9 for the CLI's behaviour on synthetic specs.

### 1.2 Inputs and downloads

- **E6.** A bundle (an input or a dependency) is a local path or a URL, with an optional expected `size` and `sha256` (`docs/BATCH_SPEC.md` 1.2). Every bundle is staged into the spool before extraction, a local file by a copy and a URL by its download, and every read of it during the run reads the staged copy (`docs/plans/batch.md` 2.5). Its SHA-256 is computed while it stages and recorded in the manifest's `bundles` rows. A `size` or `sha256` that does not match the staged bytes fails the bundle (`integrity_mismatch`); a local path that is missing fails it (`source_missing`), one that is not a regular file fails it at once, without blocking (`source_not_regular`), and one written during its copy fails it (`source_changed`). Local sources are on local filesystems; network filesystems are out of scope in version 1. A bundle over `max_decoded_bytes` fails before its bytes are claimed (`limit(input_bytes, <M>)`), so no one bundle's staging can fill the spool (an input's outputs are bounded by `max_export_work`, E48). A change to a source after its copy changes nothing.
- **E7.** A URL takes headers, an idle timeout and a rate floor (no byte for that long, or fewer than the floor's bytes in such a period, fails a try; there is no whole-request timeout, so a download slowed by its neighbours takes longer and fails only below the floor) and a retry policy: which statuses and errors retry, the backoff, `Retry-After`, the number of attempts, and redirects, exactly as `docs/BATCH_SPEC.md` 1.4 defines them. No retry starts when its backoff would pass the deadline less the grace (E45), and a stop abandons a request in flight. A redirect to another origin fails the download, so a header never reaches another host. A body is staged as sent (`Accept-Encoding: identity`), so `size`, `sha256` and `Content-Length` all describe the same bytes. A header value that carries a secret is given by reference to an environment variable, never in the spec (E42), read once at preflight and at most 16 KiB.
- **E8.** `download_concurrency` bounds parallel transfers: stages (downloads and local copies), and in the publish phase uploads.
- **E9.** A job declares its complete dependency graph: every bundle any input needs is in `inputs` or `dependencies`, with its own path or URL, and every `deps` id names one of them (`docs/BATCH_SPEC.md` 1.1). A name in an input's AssetBundle `m_Dependencies` that is not the id of a bundle of its view is never fetched or opened: it is reported (a `diagnostics` row with the input and the name, and the run's `undeclared_dependencies` count), and a read that reaches a CAB only it would hold is the object-level skip `outside_view` (E65). There is no discovery. A first consumer builds its graph from its own build manifest, spelling ids as `m_Dependencies` spells them (`docs/BATCH_SPEC.md` 4.1).
- **E10.** A download that fails after its retries fails the bundle: the input itself, or, for a dependency, every input whose view holds it (E13). A body shorter than its `Content-Length` is a failed download. The run is then `incomplete` (exit 4), and the reason is in the `transport` class, which a retry as is may clear (`docs/BATCH_SPEC.md` 9.3).

Levels: L9, against a local test server that serves, delays, stalls mid-body, fails by status, truncates and sends `Retry-After` on command, and against local files rewritten during the run.

### 1.3 Dependencies and isolation

- **E11.** The spec carries a dependency graph: for each bundle, input or dependency, the ids it needs (`deps`). An input's **view** is the input itself and the closure of its list through the graph (`docs/BATCH_SPEC.md` 4.2). A spec with a view over `max_view_bundles` is refused at validation. Every member of a view must be an AssetBundle file; one that is not fails the inputs whose view holds it (`dependency_failed(<id>, not_a_bundle)`), since a staged file has no name the core could key a raw dependency by (`docs/plans/batch.md` 2.3).
- **E12.** Under `EnvOptions::batch()`, an input's `.resS` reads and its container read its own bundle's same-named CAB first (core plan section 9, Q12; `docs/plans/batch.md` 3.9). `EnvOptions::unitypy()` keeps UnityPy's one registry.
  - The rule lives in the engine until the core's `own_bundle_first` option (batch.md section 6, CR3) replaces it.
  - The resource candidates are taken in UnityPy's order. Every candidate is looked for in the reading file's own container chain (its container, then each enclosing one) first; only when none is found there are they looked for in the registry.
  - A pointer reads the reading file's own bundle first (Tier A), then the input, then, among the other bundles of the view, the one whose id is greatest in bytewise order (`docs/BATCH_SPEC.md` 4.3). Every input is registered in its own attempt, with or without dependencies, and every nested container of a registered bundle is reached at its registration (`docs/plans/batch.md` 2.6 step 4), so a pointer between two of its own containers, or into a dependency's nested container, resolves as UnityPy's eager load resolves it.
- **E13.** "Needs" means "is in the view" (E11). A dependency that fails to stage or to open fails exactly the inputs whose view holds it, a staging failure before their attempt (`docs/plans/batch.md` 3.10). A read that fails for a dependency's own bytes (a corrupt block or entry, found at read time) is an object-level skip of the input that read it, not a failed dependency; registration's peek at an entry's first bytes takes such a block as "not a container" and fails nothing (`docs/plans/batch.md` 2.6 step 4). A view member whose load in an attempt is refused or fails on its bytes fails the input with `dependency_failed(<id>, <reason>)`, whose exit code follows the inner reason: 5 for a budget or limit, 4 otherwise (`docs/BATCH_SPEC.md` 9.2). An I/O error reading a member's staged file is a host failure (exit 74).
- **E65.** A pointer or resource read whose CAB no bundle of the input's view holds is an object-level skip, `outside_view(cab)`. The skip and its reason are the same whether another bundle holds that CAB or none does.
- **E14.** Each attempt opens each bundle of its view at most once per residency in its own dependency cache, within `dep_cache_bytes`, and reopens it after eviction. Past `max_attempt_evictions` evictions the input fails (`limit(attempt_evictions, <value>)`). Any refusal of its dependency budget after the view is registered, a reopen's or a read's, fails it too (`budget(dependency)`, exit 5): the core does not say which refusals are reopens. So an attempt makes at most its view's size plus the cap in opens, and `budget(dependency)` is never an object's skip. No dependency is shared between attempts, so a dependency that N inputs read is opened up to N times (`docs/plans/batch.md` 1 and 2.2). The manifest counts every eviction. `docs/DESIGN.md` section 4 says the same, and plans a shared pre-decoded cache for later.
- **E58.** An input's records (its outputs and their bytes, its skips, its failure) are a function of its staged bytes, the staged bytes of the bundles in its view, the spec's entries for those bundles, and the spec's other fields. They do not depend on `threads`, the order inputs run or finish in, what else is in flight, or which bundles outside its view failed or exist (`docs/plans/batch.md` 1 and 2.10). Three limits: a run-level failure (a host failure, every allocator refusal the counting allocator sees among them, the spool's cap, an engine bug) stops the run instead; a bundle that could not be staged fails its inputs with a transport reason, which depends on the network or the host; and, until the core's CR2, liblzma's allocation refused unseen, on a dependency's meter (which the core retries after eviction) or on the input's after a budget refusal absorbed in the same core call, can decide a record by the host's memory (batch.md A13). Its test is `docs/plans/batch.md` 2.10's, with the nightly's second run (section 5).

Levels: L1 (env) and L9, on testkit scenes.

### 1.4 Decoding

- **E15.** BC7, DXT1, DXT5, RGBA32 and RGB24 decode bit-exact, with UnityPy 1.25.2's output modes, for textures UnityPy neither unswizzles nor byte-swaps (`docs/BATCH_SPEC.md` section 3): `RGB` for RGB24, `RGBA` for the other four (`PARITY.md` section 17's rows for these five).
- **E66.** Selection. What one input exports, and which textures are skipped with a named reason (other formats, crunched forms, Switch-swizzled textures, Xbox 360 textures of a byte-swapped format, a side of 0), is `docs/BATCH_SPEC.md` section 3. A skipped texture is never decoded as another format.
- **E16.** `meshdecode`'s vertex-data unpack (every VertexFormat table, index formats and topologies, `get_triangles`), in f64, as `PARITY.md` section 15.3's rows marked P1b state. CompressedMesh and PackedBitVector are P2a.
- **E17.** Sprites render as UnityPy's `SpriteHelper` renders them (`PARITY.md` section 15.2): atlas render data, tight meshes, rotation, flips and the alpha merge. The affine-triangle path is Tier A if section 3, item 6's port is exact. Otherwise it is a Tolerance row with item 6's metric, whose threshold is measured and stated in `PARITY.md` before E17's test is written.
- **E18.** Every decoder runs under `catch_unwind`. A panic is an object-level skip, recorded in the manifest, never a crashed run.

Levels: L5 on checked-in fixtures for the five formats and every sprite path; L5 on the nightly private corpus.

### 1.5 Images

- **E19.** A PNG writer byte-equal to Pillow 9.5.0's at its defaults, for modes `L`, `LA`, `RGB` and `RGBA` (`docs/DESIGN.md` section 4's `unityrs-imageio` row, L5b). Its deflate is zlib 1.2.13, the version the oracle image's Pillow links and the reference for PNG bytes (`docs/DESIGN.md` section 3's zlib row): the build compiles a vendored zlib 1.2.13 statically, since `libz-sys`'s static build bundles its own newer version, and CI asserts that `zlibVersion()` is `1.2.13` and that no zlib-ng is linked. Nothing is vendored yet.
- **E20.** The visible-pixel hash, exactly: SHA-256 over the ASCII bytes `"{width}x{height}:"`, then the image's RGBA bytes in row order, where each pixel of alpha 0 is written as (0, 0, 0, 0). An image in another mode is converted to RGBA first, as Pillow 9.5.0's `convert("RGBA")` converts it.

Levels: L5b on checked-in fixtures for every mode and on the nightly corpus; E20 by a reference implementation in the parity tools, run on the same images.

### 1.6 The NGUI atlas cutter

- **E21.** Sheet scope `own_files_by_name`: the material's main-texture name, then the atlas name, matched among the input's own Texture2D objects only. A PPtr may cross into a dependency to read a name, never to take its texture. `follow_pptr` is not in version 1, nor a fallback for an atlas stored without a typetree (`no_typetree`). How an atlas is recognised, which fields hold its sprites, the crop geometry and every tie rule are `docs/BATCH_SPEC.md` section 5, which is the specification item 3's reference is written from.
- **E22.** A caller-given list of name suffixes (`skip_name_suffixes`) skips reference atlases.
- **E23.** A crop past the sheet's edge is padded with zeros, as Pillow 9.5.0's `Image.crop` pads.
- **E24.** The atlas JSON is byte-equal to CPython 3.11's `json.dumps(value, indent=2)` (its defaults otherwise: ASCII escapes, the float `repr`, `NaN` written as `NaN`) of the object `docs/BATCH_SPEC.md` section 5 defines: keys `name`, `texture_width`, `texture_height`, `pixel_size`, `settings`, `sprite_count`, `sprites`, in that order, with typetree values written as read (pairs as arrays, f32 widened exactly, strings as UnityPy decodes them).
- **E25.** A value the layout cannot write (bytes) skips that atlas's JSON alone, recorded in the manifest. The crops stay.
- **E26.** Collisions are handled per unit: a sheet texture and every atlas of the input that cuts it, or one atlas when textures are not exported. The unit's winner also wins its sheet texture (`docs/DESIGN.md` section 4's `unityrs-export` row; `docs/BATCH_SPEC.md` 4.5). The outputs rows name each output's unit key, which links an atlas to its sheet.

Levels: L5 and L5b for the crops, L7 for the JSON, on testkit-built atlases (an NGUI-shaped MonoBehaviour typetree is synthetic) and the nightly corpus.

### 1.7 Output naming

- **E27.** Key templates per output kind come from the spec. A variable is written verbatim, and its `safe_` form through `safe()`. The grammar, the variables, their spelling and where each is allowed are `docs/BATCH_SPEC.md` 2.1. Version 1 has no container path: a scope decision, not an impossibility (the input's own container rows are readable); it can come in a later version as an optional variable.
- **E28.** `safe()` works per code point, an invalid UTF-8 byte counting as one: every code point outside `[A-Za-z0-9._-]` becomes `_`; then every leading and trailing `.` is stripped; then an empty result becomes `unnamed` (`docs/BATCH_SPEC.md` 2.2).
- **E29.** Inside one input, the first object of a given type and name, in object order (`docs/BATCH_SPEC.md` 4.4), takes the name, even when its decode then fails. A later one is skipped, `duplicate_name`. Keys are planned before any export: of two planned outputs of one input with one key, the first in (kind, object order) takes it, even when its export then fails, and the later one is `duplicate_key`; within an atlas its JSON comes before its sprites. An output skipped for a reason that needs no pixels (an unsupported format, an empty rectangle, a reference atlas) is not planned and takes no key (`docs/BATCH_SPEC.md` 4.4).
- **E30.** Inside one atlas, of two sprites with one key, the later one wins.

Levels: L9.

### 1.8 Winners across inputs

- **E31.** When several inputs produce one key, the spec's `prefer` map, `{key: {input, md5}}`, decides:
  1. the candidate from the preferred input, if that input has a candidate for the key;
  2. otherwise the candidate with the preferred MD5 (for a unit, its unit key's output's MD5), and of several such the first by bytewise input id, not reported as a flip;
  3. otherwise the first candidate input in bytewise order of input id, reported in the manifest as a **flip** when the candidates' MD5s are not all equal.
- **E32.** A sheet's texture output and every atlas of the same input that cuts it, with their crops and JSONs, win as one unit; when textures are not exported, one atlas with its crops and JSON is a unit. The unit's `prefer` entry is its unit key's: the sheet's texture key, or the JSON key (`docs/BATCH_SPEC.md` 4.5). This is the cross-kind rule.
  - The exact steps are `docs/BATCH_SPEC.md` 4.5 and 4.6. A unit is made of outputs the input made, so its unit key is always an output's key: the sheet's texture, else each atlas's JSON. Units are decided first, per unit key.
  - Every key of a winning unit is then reserved: a plain candidate for it loses, reported with the rule `unit`.
  - When two winning units claim one key, the unit whose unit key is first in bytewise order keeps it. The other unit is withheld whole (`unit_conflict`). A unit with a key refused for `prefix_conflict` or `archive_conflict` is withheld whole too. So phase 6 never publishes a unit in part; an attempt-time skip of one output (`unwritable_key`, `duplicate_key`) drops that output only.
  - Each of these checks runs once, so phase 6 decides them in bounded memory (`docs/plans/batch.md` 2.9): a key a withheld unit claimed stays reserved.
- **E33.** "Pixels differ" means a different visible hash (E20). A collision whose candidates share a visible hash is reported, but not as a pixel difference.
- **E34.** The winners hold candidates until every input that can produce a key has run: a sink that holds output across jobs. `docs/plans/batch.md` 3.5's model allows it: candidates are copied out to the spool, and winners are decided after extraction, as a pure function of the committed records.
- **E60.** All or nothing per input. An input that fails commits no candidate and competes for no key. Only its failure record reaches the manifest.
- **E61.** Withholding. A key whose `prefer` entry names an input that failed in this run is not published (`withheld`). Its rule-2 or rule-3 winner could otherwise overwrite what the next complete run publishes. A key with no such entry is published, even when a failed input might have won it by rule 3: a choice, since withholding every key a failed input could produce would withhold most of a run for one failure. A spec option `publish_incomplete` (default `true`) set to `false` publishes nothing when any input failed: every winner and every archive with a member is `withheld` (`publish_incomplete`), and neither resume nor verification runs (`docs/BATCH_SPEC.md` 7.3).

Levels: L9.

### 1.9 Sinks

- **E35.** Directory.
- **E36.** Zip archives, written to the job's one sink as one more object each: stored members. An archive names output kinds and, per kind, a member path template over the output's variables; every published output of those kinds is a member, in `/`-first order of path (bytewise, except that a `/` is less than any other byte; `docs/BATCH_SPEC.md` 1.7 and 4.7). So a caller needs no list of keys before the run. Membership is decided with the winners, never by how loose PUTs end. Members stream from the spool, never copied whole; no archive is written that would have no member. Their bytes are fixed by `docs/BATCH_SPEC.md` section 8, so an archive's MD5 is the same on every run with the same members. There is one spool for staged bundles, candidates, records, archives and the manifest, with one size cap and one free-space reserve, checked before every claim (`docs/BATCH_SPEC.md` section 6).
- **E37.** Object store (S3-compatible):
  - credentials from the environment only (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `AWS_SESSION_TOKEN` when set); no fallback chain. The engine builds each client with `AmazonS3Builder::new()`, never `from_env()`, which reads every `AWS_*` setting (checksums, encryption, request-payer) and would add headers this requirement forbids, passes its own static credentials with `with_credentials`, which skips the fallback chain, and refuses to start without credentials;
  - path-style addressing available (bucket names with dots break virtual-hosted TLS), and plain HTTP allowed only for an `http` endpoint the spec names (the L9 test store). `endpoint` is the service's URL without the bucket, which the engine maps for each addressing mode, and preflight proves the bucket exists with a LIST (`docs/BATCH_SPEC.md` 1.6, 7.2);
  - Content-Type set on every PUT (`image/png`, `application/json`, `application/zip`, `application/x-ndjson`), and no other header the engine chooses: no metadata, ACL, cache-control or checksum header. Signing headers, the session token's header, `Content-Length`, and E38's `If-Match` and `If-None-Match` are the protocol's. A single PUT sets its Content-Type per request; `MultipartStore::create_multipart` takes no options, so a multipart upload takes its client's default, and the controller has one client per multipart Content-Type (`docs/plans/batch.md` 2.9);
  - the caller passes credentials valid until the deadline; a set `AWS_CREDENTIAL_EXPIRATION` before the deadline is refused at start, and a request refused with `ExpiredToken` fails with its own reason, `credentials_expired` (`docs/BATCH_SPEC.md` 1.6);
  - the client's per-try timeout is the sink's `timeout_ms`, and its `RetryConfig` retries nothing (`max_retries` 0): the engine makes each of the sink's `attempts` tries itself, so it sees and counts every try the store may still commit (`unsettled_tries`, `docs/BATCH_SPEC.md` 1.6, 7.3). `object_store`'s errors do not expose a try's status, so the engine's own HTTP connector records each try's status, `Retry-After` and a 4xx's error code, and `docs/BATCH_SPEC.md` 1.6's table classifies it. A stop abandons a try in flight. Multipart uploads go through `MultipartStore` (`create_multipart`, `put_part`, `complete_multipart`, `abort_multipart`), whose upload ids the journal records. A `CompleteMultipartUpload` retried after a lost response that meets `NoSuchUpload` is checked with a HEAD: the expected ETag there means it completed;
  - files up to 16 MiB single-part, so the ETag is the MD5; larger ones multipart in 16 MiB parts, like archives, compared by their expected multipart ETag (E43). Under SSE-KMS or SSE-C the single-part ETag is not the MD5; the engine reports `etag_not_md5` instead of failing, and resume then re-PUTs;
  - archives multipart at 16 MiB parts; a failed or interrupted multipart upload is aborted, on SIGTERM too (E64);
  - keys written byte for byte as given, or refused with a named error, never rewritten. Section 3, item 1 names the refused classes. Empty segments, `.` and `..` segments, a leading or trailing `/`, control characters, keys that are not UTF-8 and keys over 1,024 bytes are refused. `%`, `#`, `~`, spaces and non-ASCII are written as given.
- **E62.** Directory-sink safety. Each file is written as a temp file beside its destination and renamed over it. One `syncfs` of the output file system runs before the final manifest, so the manifest never claims a file a power cut could lose. Paths are opened beneath the root (`openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` where available, else per-segment `O_NOFOLLOW`), so neither a key nor a planted symlink can escape it. A segment over 255 bytes (`NAME_MAX`) is a refused key. A killed run can leave temp files (`.unityrs-tmp-*`). No key may have a segment starting with `.unityrs-tmp-` (`reserved_name`), so none is ever taken for an output, and the engine never walks the root for them: the caller may delete them while no run holds the root's lock (E38).
- **E64.** Each multipart upload's id is appended and synced to the spool's uploads journal after its create and before its first part, and removed after it completes or aborts. The finisher closes the journal and aborts every id in it (it starts no multipart upload itself: its row files are single PUTs), and the next run in the same spool, of any `run_id`, aborts the ids a killed one left for its own endpoint and bucket (`docs/BATCH_SPEC.md` 6.2); an id it cannot abort is left to the lifecycle rule with a warning, and never stops its start. The operator's runbook names the bucket's lifecycle rule for uploads left by a lost spool.

Levels: L9 against a local S3-compatible test server, with its request log as the evidence for headers, addressing and keys, and a fault-injecting store (one fault per row of `docs/BATCH_SPEC.md` 1.6's table, `SlowDown` and `Retry-After` among them, a hang, a conditional-write conflict, a lost complete response, ETags that are not MD5s, a folder marker under the prefix, a PUT held until after the attempt's last transfer, which verification must report).

### 1.10 The run manifest

- **E38.** The manifest's first write is status `running`. It carries the `run_id`, the attempt number from the spec's required `attempt` field (an integer the caller raises for each attempt), the binary's version, the `spec_version`, the start time and the deadline. No output is written before it.
  - Its location is fixed by the `run_id`: a small head, `_unityrs/<run_id>/manifest.json` under the sink's root, and per-attempt row files beside it (`docs/BATCH_SPEC.md` 7.1). So runs with different `run_id`s never fence each other.
  - Each head write is fenced on (`run_id`, `attempt`). An attempt that finds a head of its `run_id` with an attempt number above its own (or, in preflight, equal to it), in preflight or on a write's read-back, writes nothing more and exits 7 (`superseded`). A head of a lower attempt that the `running` write's read-back finds is an earlier attempt's late write: the attempt retries the write on it, at most 4 times (`docs/BATCH_SPEC.md` 7.2).
  - In an object store each head write is a conditional PUT (`If-None-Match: *`, then `If-Match`), and a failed precondition (`object_store`'s `Precondition` or `AlreadyExists`) is read back before it counts as superseded, so a lost response is not taken for a rival (`docs/BATCH_SPEC.md` 7.2). A `running` write that fails otherwise exits 69 having written at most a fenced `running` head, so under conditional fencing the caller retries with the next attempt number, at once.
  - A store without conditional writes needs `manifest_fencing: "none"`; the caller's duties below are then the only fence on the head too.
  - The directory sink fences with one lock for the whole output root (`_unityrs/lock`), so one process at a time writes a root, and a rename.
  - **Verification and the caller's duties.** No client can bound when a store commits a request it has received, so in an object store the engine bounds its own writes and, before the final head, LISTs what it published and checks each key's ETag: another writer's bytes make the key `failed` (`publish_mismatch`), and the head says whether the pass ran (`verified`). The caller keeps two duties: one attempt of a `run_id` at a time, with nothing else writing its keys; and before a retry, a wait of the store's longest commit time (15 minutes for S3) past the earlier attempt's end, or its deadline, unless that attempt ran with conditional fencing and either its final head shows a normal end with no unsettled try or it exited 69; with `manifest_fencing: "none"` the caller always waits (`docs/BATCH_SPEC.md` 7.2). The spec gives a `deadline` with an object store, so the caller always has an end to count from.
  - The head is the record of the run: for every key it lists as `written` or `present`, the store held this attempt's bytes when verification listed them (for a key whose ETag is not an MD5, an object of their size, counted in `verified_by_size`), and holds them still if the caller kept its duties. A key it does not list so is not the run's output, whatever the store holds.
- **E39.** Its last write, by the finisher, replaces it with the final status: `complete`, `complete_with_skips` or `incomplete`. On a normal end it is written only after every publish was acknowledged and, in an object store, verified, and after E62's `syncfs` for a directory. On a stop it says what was reached, as `docs/BATCH_SPEC.md` 7.4 defines for each phase; a stop before the `running` write writes no head.
- **E40.** The manifest records:
  - **per bundle:** whether it staged, its reason, its size and the SHA-256 of its staged bytes;
  - **per output:** key, kind, unit key, source input, path id, MD5, visible hash, mode, size, and a status of `written`, `present` (resume found the same bytes), `not_written` (a stop came first), `withheld`, `failed` or `unwritable_key`;
  - **per collision:** one row per candidate (key, input, MD5, visible hash), the winner and the rule that chose it, flips marked;
  - **per input:** status, failure reason, its Input and Dependency peaks, its evictions, timings, object-level skips;
  - **per archive:** status, MD5, ETag and size, and each member's path, key, source input and MD5, or why it was left out;
  - **for the run:** `same_named_cabs`, `undeclared_dependencies`, `nested_containers`, evictions, the peaks of the inputs' and dependencies' budgets, the spool's peak, peak RSS (`VmHWM`), the bound batch.md section 5 computes for the spec, the budgets as read, the stop reason, whether verification ran to its end and how many keys it checked by size alone, and each warning's count (`resume_list_failed`, `verify_list_failed`, `etag_not_md5`, `stderr_dropped`, `abort_failed`).
  - Every field, row file and reason code is `docs/BATCH_SPEC.md` sections 7 and 9.3. A limit refusal's reason names the limit and its configured value, never the amount asked for.
- **E41.** In an object store, Content-Type `application/json`.
- **E42.** No credential appears in the manifest, the progress stream, stderr or any error message. That covers a URL's query string and userinfo, and every header value. A URL is never printed with its query string, and an HTTP client's error is printed without its URL.

Levels: L9, with a test that runs every sink with known credentials, signed URLs and secret headers, and searches every byte the run wrote or printed for them.

### 1.11 Resume and idempotence

- **E43.** A re-run with resume LISTs the output root once, at phase 7's start, and skips a PUT whose key already holds the same MD5 (an archive's, or a file's over 16 MiB: its expected multipart ETag at 16 MiB parts, which for an archive takes a first pass over its bytes), marking it `present`. The LIST is merged with the sorted winners one page at a time, so its memory does not grow with the store (`docs/plans/batch.md` 2.9). A LIST that fails, for example on a foreign key `object_store` cannot parse (section 3, item 1), degrades resume to "PUT everything" with the warning `resume_list_failed`; it does not fail the run. In a directory, resume hashes an existing file only when its size matches.
- **E44.** Every input is still read and decoded, so winners, archives and the manifest cover the whole job. There is no partial ("narrowed") re-run.

Levels: L9: a run killed at each progress line, then resumed, writes the same keys and bytes under the root, `_unityrs/` aside, as an uninterrupted run, and leaves no open multipart upload.

### 1.12 Lifetime

- **E45.** A wall-clock `deadline` in the spec is the time by which the process has exited. The engine converts it once to a `CLOCK_BOOTTIME` instant and starts stopping at the deadline less `shutdown_grace_ms` (default 20 s, at least 5 s). Every stop, from a signal, the deadline, the parent, stdin or a run-level failure inside the run (a leak, a host I/O error, an allocator refusal, a full spool, an engine bug), takes one path: every request in flight is abandoned, and the finisher aborts open uploads, writes the manifest the phase allows, and exits with the stop's own code, without joining its threads; a watchdog calls `_exit` with that code before the deadline whatever is unfinished (`docs/plans/batch.md` 2.11). Only a filesystem call that never returns can hold the process past it (batch.md A11).
- **E46.** The process exits when its parent dies (Linux `PR_SET_PDEATHSIG`, set by the binary itself) and, when the spec asks, when stdin reaches EOF. When the spec gives `parent_pid`, the binary compares `getppid()` with it right after setting the signal and stops at once when they differ, which closes the race of a parent that died first; without `parent_pid` there is no start check. Callers are told that Linux sends the signal when the *thread* that forked the child exits: a caller that spawns from a pool thread that may exit should use the stdin rule.
- **E47.** SIGTERM, SIGINT and SIGHUP stop the run (E45): abort open multipart uploads, write the manifest `docs/BATCH_SPEC.md` 7.4 gives for the phase reached (none before the `running` write was sent), exit 6, all within the grace. A stop the controller sees at phase 7's end, after the transfers drain or after verification, makes the end a stop; a signal after the controller's last check leaves the normal end's manifest and code, and only arms the hard deadline. Once the finisher's last head write has ended, the hard deadline and a second signal exit with the code that head records; before then they exit with the stop's code, and the head says `running` (`docs/plans/batch.md` 2.11). A second signal exits at once; a stop for another reason (stdin's EOF, the deadline, a run failure) followed by one signal still writes the manifest.
- **E63.** Before the manifest's first write, a preflight runs in a fixed order (`docs/plans/batch.md` 2.1). It starts its threads and reads the credentials. It starts the spool (`docs/BATCH_SPEC.md` 6.2): it checks that the spool directory is dedicated to unityrs (empty, or holding only its marker, lock and runs) and apart from a directory sink's root, takes a non-blocking `flock` on its lock file, which is never deleted, so two attempts never share a spool, aborts the uploads its old journals name, deletes only its own `runs/`, then checks the free space against the reserve. It refuses a spool on tmpfs or ramfs, whose bytes would be memory. It checks `RLIMIT_AS` and `RLIMIT_DATA` against the floor `docs/plans/batch.md` 2.7 derives from the memory bound, raises `RLIMIT_NOFILE` to its hard limit and checks it, and refuses `vm.overcommit_memory` 2, a `vm.max_map_count` below the bound's need and a bound above the smaller of `MemTotal` and the memory cgroup's `memory.max`. It takes the output root's lock (directory sink), or proves the bucket exists with a LIST (object store), and reads the stored manifest head for fencing. A preflight failure exits 69 with nothing published or deleted that the engine did not create; a head that supersedes this attempt exits 7. A missing or mismatched bundle is not a preflight failure: it fails at staging, and only the inputs that need it (E6, E13).

Levels: L9.

### 1.13 Budgets

- **E48.** `threads`, `download_concurrency`, `max_inflight_uncompressed_bytes`, `max_decoded_bytes`, `dep_cache_bytes`, the spool directory, the spool size and the spool's free-space reserve are required (E5). Each has one meaning (`docs/plans/batch.md` 2.2):
  - `max_decoded_bytes` (`M`) is every meter's `max_input_memory` and `max_input_bytes`, an input's and each dependency's alike, and each attempt's Input budget; the engine sets every meter's `max_decode_work` to four times it, the core's default ratio;
  - `dep_cache_bytes` (`D`) is each attempt's Dependency budget and the bound of its private dependency cache, registry included;
  - `max_inflight_uncompressed_bytes` (`I`) bounds the slots of the attempts in flight (`M + D` each), and is the working set of the winners' sort, the resume merge and the transfers' buffers;
  - `download_concurrency` bounds transfers (E8);
  - `max_attempt_evictions` (optional) bounds one attempt's dependency evictions, and so its work (E14);
  - `max_export_work` (optional) bounds the pixel bytes one attempt writes plus the files it spools (PNGs and records), each counted in a fixed 4 KiB unit whatever the host's block size, so the limit is a function of the bytes and the spec; and so it bounds its export time and its share of the spool: past it the input fails, `limit(export_work, <value>)`. A spool sized by `docs/plans/batch.md` 5.6 leaves room for it.

  A spec is refused unless `M <= D`, `M + D <= I` and `I >= 64 MiB`. Every field, required or optional, and its default is `docs/BATCH_SPEC.md` section 1.
- **E49.** How the budgets bound memory is `docs/plans/batch.md` section 5: `max_inflight_uncompressed_bytes` plus constants the spec fixes, with the spool bounded on disk by its size and reserve.
- **E59.** Every buffer the engine allocates in proportion to an input or an output (pixels, masks, a sprite's mesh arrays and the affine path's buffers, PNG rows and output, atlas JSON, the attempt's records of its AssetBundle object, its record buffer, zlib's deflate state) is charged to the job's meter, or to the phase budget of `docs/plans/batch.md` 2.9, before it is allocated, at `heap_cost`: the core's rule (core plan 3.15) applied to engine code. A job exports one sheet at a time, so its export buffers are bounded by its largest sheet.
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
| Sealed attempts | `docs/plans/batch.md` 2.2: no outcome may depend on what runs beside an input, and a shared dependency cache could not promise it. The cost is reserved memory and a dependency decoded once per input that reads it, measured in batch.md 2.2; a shared pre-decoded cache that keeps D is planned (batch.md section 8). |
| No discovery: the job declares its whole graph | E9. Fetching names found in `m_Dependencies` made a view depend on other inputs' declarations and needed a second extraction round, its caps and its reasons. A caller has its build's graph already. |
| The caller owns attempt serialisation and the wait before a retry (object store) | E38: no client can bound when a store commits a request it already received, so the engine verifies what it published and states the caller's duties, instead of a fence it cannot keep (`docs/BATCH_SPEC.md` 7.2). |
| Every input registers itself, and every slot is `M + D` | E12: a pointer between an input's own containers must resolve the same with or without dependencies. The input's slot shares its own bytes, so the cost is the reserved slot, not a second read. |
| Every bundle is staged into the spool | E6: the only way to pin the bytes every attempt reads, since the core rereads a dependency's file after each eviction. |
| View members are bundles | E11: a staged raw file has no name the core could key it by. A later version can add a named source with a core request. |
| No `{container}`, no NGUI 2 layout and no stripped-atlas fallback in version 1 | E21, E27 and `docs/BATCH_SPEC.md` section 5: scope, to keep version 1's contract small; each can come in a later version as an optional field. |
| Changed-pixel emission is not in version 1 | `docs/DESIGN.md`'s batch row plans it; the manifest's visible hashes let a caller compare two runs meanwhile. |

## 3. Section 3's items, settled in design round 1 (for review)

Each item gives the recommendation and its reason. None is decided until this plan's review rounds end.

### Item 1. Verbatim keys and `object_store`

**Recommendation.** Keep `object_store` (0.12). Write a key only when `Path::parse(key)` gives back exactly `key`, the key is non-empty, valid UTF-8 and at most 1,024 bytes. Refuse any other key with `KeyRefused { key, rule }`, never rewriting it. `Path::from` and `from_iter` are never used.

**What `object_store` 0.12.5 does**, as read in design round 1 and checked against the crate's source in design round 5 (what remains assumed is the store's side, `docs/plans/batch.md` A15):
- **`Path::parse`** (`src/path/mod.rs`, `src/path/parts.rs`):
  - strips one leading and one trailing `/`, silently, which is a rewrite;
  - refuses an empty segment, a segment that is `.` or `..`, and any ASCII control character (`PathPart::parse`);
  - keeps `%`, `#`, `~`, spaces, non-ASCII and other punctuation verbatim.
- **`Path::from` and `from_iter`** drop empty segments, percent-encode `%`, `#`, `~` and other reserved characters, and turn `.` and `..` into `%2E` and `%2E%2E`. Those are rewrites.
- **On the wire**, the S3 client percent-encodes the key once, every byte outside `A-Za-z0-9-._~/` (`STRICT_PATH_ENCODE_SET`, `src/aws/mod.rs`, used in `src/aws/client.rs`), and the store decodes it once. So the stored key is the `Path`'s string, byte for byte.
- **`Path` is a `String`**, so a key that is not UTF-8 cannot be expressed.
- **LIST** reads each page's body whole and parses every key it returns with `Path::parse` (`src/client/s3.rs`). One foreign key under the root that it refuses fails the whole listing (E43's degradation). `PaginatedListStore::list_paginated` takes a page size (`max_keys`), which the engine sets to 1,000 (`docs/plans/batch.md` 2.9).
- **Credentials.** Without explicit ones, the builder falls back to web identity, the container and pod providers and instance metadata (`src/aws/builder.rs`), and `from_env` reads every `AWS_*` variable, so the engine uses neither (E37).
- **Content-Type.** A single PUT's attributes set it; otherwise the client's extension map, empty unless configured, then its default (`ClientOptions::get_content_type`). `create_multipart` takes no attributes, so a multipart upload gets the client's default (E37).
- **Errors.** `object_store`'s retry error type is private to the crate, so a try's status is not visible from outside; the engine reads it through its own `HttpConnector` (`AmazonS3Builder::with_http_connector`, E37).

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
- A template's literal parts are checked at validation (`docs/BATCH_SPEC.md` 2.1's literal checks): a bad spec (exit 78).
- A key built from a verbatim name is checked when its output is made. The output is an object-level skip (`unwritable_key`), named in the manifest.
- `safe()` never makes a shape refused by its characters: it maps `/` and control characters to `_` and strips dots at the ends. A long name can still make a segment over 255 bytes or a key over 1,024, which is `unwritable_key`.
- The same rules hold for every sink: `segment_too_long` (`NAME_MAX`), `reserved_prefix`, `prefix_conflict` (a key that is also a parent directory of another winner's key or of an archive's key, which a directory cannot hold) and `archive_conflict` (a key equal to an archive's key or under it) are refused everywhere, the last two decided in phase 6 so it does not depend on publish order (`docs/BATCH_SPEC.md` 2.3 and 4.6). Zip member paths take the rules too (but the reserved prefix and name), where they also block escapes from the archive's root. So a key is writable in one sink exactly when it is writable in all of them.

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
- **The schema.** A JSON Schema (2020-12) per version, `docs/batch/spec-v<N>.schema.json`, written by hand from `docs/BATCH_SPEC.md` section 1's tables and embedded in the binary. It states each field's type, range and presence, which a schema can express; the cross-field rules (unique ids across two arrays, `M <= D`, view sizes, templates, literal keys, archive ancestry, the spool against the sink) it cannot.
- **Its tests.** Every per-field fixture spec, valid and invalid, runs through both the schema and the Rust parser, and they must agree. Every cross-field fixture runs through the parser alone and names the rule it breaks. A duplicate member in any object is refused by the parser's own deserializer, which sees what a schema cannot.
- **`spec_version`** is an integer, from 1. A binary reads every version from 1 to its highest, and `--version` prints the highest (E2).
- **Strict.** Unknown fields are refused at every level (`additionalProperties: false`, `deny_unknown_fields`). A misspelt budget that is silently ignored is an outage.
- **Adding a field means a new version.** In it, the field is optional, with a default that reproduces the previous version's behaviour exactly. A removal, a rename, a new required field or a changed meaning also needs a new version, and the old number keeps its old meaning.
- **Golden specs per version** (`tests/specs/v<N>/`) run in CI against every later binary, with their outputs and manifests pinned under the normalised comparison (section 5).
- **Pinned consumers.** A consumer pinned to an older binary keeps writing its version, and newer binaries read it the same way. A newer field sent to an older binary is refused before any work (E1), which is the safe failure.
- **No secret is a spec field** (E7, E42), except what a signed URL carries in its query string; E42 keeps that out of every output. The manifest and the progress stream carry the `spec_version` they were produced under.

**Reason.** A hand-written schema is the document consumers read. The agreement test keeps it equal to the parser, and the golden specs keep old versions working.

### Item 5. Exit code values and the progress schema

**Exit codes:** `docs/BATCH_SPEC.md` section 9: 0, 3, 4, 5, 6, 7, 69, 70, 74 and 78, their precedence, the mapping from every outcome class to a code (a budget-caused skip or failure is 5, a depth or count limit is a data reason), and the codes of `--check` and of a second signal.
- **Reason.** "Stopped", "superseded", "could not start" and "host failure" are acted on differently from a data failure, so each has its own code. The values follow `sysexits.h` where one fits. Codes 1, 2 and 64-68 are never used, so a shell or argument-parser error is never mistaken for a run's verdict.

**Progress, version 1** (NDJSON on stdout): every field, its type, the `phase` values and the `exit` line are `docs/BATCH_SPEC.md` section 10. Each line is a whole snapshot, not an event.
- **Rate.** A progress thread writes a line at least every 2 s, from a one-slot mailbox that producers overwrite. So a parent that stops reading stdout stalls only that thread, and an `EPIPE` stops progress but not the run. stderr is written only through a non-blocking writer that drops a line it cannot write (`stderr_dropped`), and the panic hook writes nothing.
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
| X7 | E35-E37, E41, E62, E64 | sinks against a local S3-compatible server's request log, a fault-injecting store (one fault per row of `docs/BATCH_SPEC.md` 1.6's table, a 200 whose body is an error, a 4xx body over 64 KiB, a journal whose ids answer `NoSuchUpload` or 403), and a directory with planted symlinks | L9 | synthetic | headers, each multipart upload's Content-Type, addressing in both modes, keys, each try's class and the `unsettled_tries` count, aborts, no orphan upload, nothing outside the root | CI |
| X8 | E1-E5, E38-E42, E63 | spec, CLI, exit codes, progress, manifest (every status, reason and count of `docs/BATCH_SPEC.md` 7.3, `publish_incomplete: false`, a name that is not UTF-8), fencing (each failed-precondition form, a read-back that finds no head, a 69 from the `running` write retried at once), verification and the caller's duties, preflight and the spool's start (a spool that is not dedicated, one inside the output root, a held lock, a missing bucket, a header variable over 16 KiB, a FIFO source), and the credential search (signed URLs and header values included) | L9 | synthetic | as stated; a refused spool is left byte for byte as it was | CI |
| X9 | E6-E10 | staging against a fault-injecting HTTP server and local files rewritten in place, missing or unreadable | L9 | synthetic | every rule of E6, E7 and E10 | CI |
| X10 | E43-E47 | a run killed at each progress line, then resumed; every stop reason (the deadline, each signal, the parent's death, stdin's EOF, a leak, a host I/O error, an allocator refusal, a full spool, `superseded`) in each phase, a stage and a transfer in flight among them, with and without a deadline; a panic injected on each engine thread; a stop during a normal-end finish, and one at each of phase 7's end checks; a stop during resume's hashing of a large archive, and during phase 6's longest pass, at the minimum grace; a directory rename racing the hand-over | L9 | synthetic | the normalised comparison below against an uninterrupted run; each reason's own exit code within the grace; the manifest of `docs/BATCH_SPEC.md` 7.4 for the phase; no open upload | CI |
| X11 | E11-E14, E58, E65, E66, `docs/plans/batch.md` | D (batch.md 2.10, every earlier review's counterexamples included), views, budgets and charging, the deadlock guard and source scans (batch.md section 4) | L1, L9 | engine scenes | as stated | CI |
| X12 | E59 | the core's counting allocator over the export paths | L9 | testkit inputs | heap never above the charges plus batch.md 5.3's constants, `C_base` measured once and pinned in the test | CI |
| X13 | E48, E59 | the squeeze, through the engine's library API below validation (whose 1 MiB floor a small testkit input's peak sits under): each testkit input with `M` at its peak less one byte, then at its peak, its peak being the largest over its meter and every dependency meter of its run | L9 | testkit inputs | refused the same way each time, then identical to an unbounded run | CI |
| X14 | E55, E57, E58 | the nightly corpus | L5, L5b, L7, L9 | the private corpus | below | nightly |
| X15 | E50-E52 | the release job | CI | a tag build | asset and SHA-256 published | tag |

**X14's pass rule.**
- Every output of every input the oracle script reads matches it on key, pixels, mode, visible hash, PNG bytes and atlas JSON. The only exceptions are rows `PARITY.md` lists as Tolerance, held to their metric.
- Where both sides fail an input, they fail for the same class of reason.
- The run's peak RSS is at or below `docs/plans/batch.md` 5.1's bound for its spec.
- A second run of the same spec at `threads` 1 gives the normalised comparison below equal to the main run's at `threads` 8 (E58).
- Evictions are recorded and watched, not gated.

**The normalised comparison** (X10, X14 and item 4's golden specs): the bytes of every key under the output root, `_unityrs/` left out, and every manifest row with these fields removed: `ms`, the peaks, `evictions` (X10 only), `vm_hwm`, `binary_version`, `started_at`, `ended_at`, `attempt`, `write_seq`, and an output's or archive's `written` or `present` status read as one. From the head, every field but `status`, `exit_code`, `stop_reason`, `counts` (with `outputs_written` and `outputs_present` compared as their sum, and `archives_written` and `archives_present` likewise, since a resumed run finds as `present` what an uninterrupted one writes), `warnings` less `stderr_dropped`, and `run`'s `same_named_cabs`, `undeclared_dependencies` and `nested_containers` is removed too: its other `run` figures (`memory_bound`, the budgets as read, `spool_peak`) change with `threads` and scheduling.

The oracle side is `tools/parity/extract_oracle.py`. It runs UnityPy 1.25.2's export of the five formats and sprites with Pillow 9.5.0, item 3's cutter, and the naming and winner rules (E27 to E34), on the same spec. For each input it builds one UnityPy Environment from that input's view alone, with an empty `path` so `find_file` never walks a directory, loading the other bundles in bytewise order of id and the input last, and treats a reference no loaded file holds as the `outside_view` skip, so the views and the same-named-CAB rule (E12, E65) are on both sides. A table in the script maps each UnityPy exception it meets to a reason class of `docs/BATCH_SPEC.md` 9.3 (a zero-height texture's raise to `empty_image`), which "the same class of reason" compares.
