# P1b plan: unityrs-batch, the engine (draft)

**Status: draft, design round 2 (2026-11-09), not reviewed.** P1b is 2D batch extraction (`docs/DESIGN.md` section 5; called P2 until 2026-11-09, when the phase table gave the 2D work its own phase). This file is the engine's plan: admission, the attempt, the hand-off to sinks, the stop path and refusal. The rest of the phase is `docs/plans/extract.md`, whose requirements rely on sections 1 and 2 here. The exact contract (the spec's fields, keys, identity and order, the manifest, the zip bytes, the exit codes) is `docs/BATCH_SPEC.md`.
- Until P1's third review round, `docs/plans/core.md` section 4.1 described a batch flow. Each review round found new problems in its admission and eviction, so the flow left the core plan for this file. Section 3 lists every problem found in it.
- Design round 1 (2026-11-09) gave a design with one dependency cache shared by every job, a Shared and an Alone attempt per input, and a retry rule. Its review found that the shared cache made outcomes depend on what ran beside an input in ways no retry rule could close (section 3.14). Design round 2 replaces it: every attempt is **sealed** (2.2). The round's dispositions are recorded outside the repository, like the earlier rounds'.
- Nothing here is built. Before `unityrs-batch` is, this plan gets its own review rounds, run until a round finds no MEDIUM or HIGH problem.

**What it builds on.** The core API and what it guarantees any batch engine: `docs/plans/core.md` section 4.1, a list of properties each checked by a P1 test. This plan relies on those properties, on the public API named where it is used, and on nothing else the core does. A need the list does not cover is a request to the core (section 6), not an assumption. Each request says what the engine does until the core grants it.

**Citations.**
- P1's review findings are cited by the P1 review round that found them, as `core.md` cites its own. Each problem in section 3 restates what was found, so no review document is needed to read it.
- Design rounds' findings are cited by what they say.
- "core 4.1, *property*" names a row of `docs/plans/core.md` section 4.1 by its first column. "core 3.13" is a section of that plan. A source path without a crate is under `crates/unityrs-core/src/`.
- Corpus figures are aggregates from the private corpus, measured on the P1 core before its perf fixes.

---

## 1. Goal

DESIGN.md section 4's `unityrs-batch` row: a JSON job spec of inputs, outputs and resource budgets (`threads`, `max_inflight_uncompressed_bytes` among them), run in parallel with bounded memory, streaming outputs to sinks, with the run marked `incomplete` when an input fails.

`docs/plans/extract.md` adds what the engine must give the rest of P1b:
- inputs that each know their dependencies by id (the spec's graph and its views, 2.3);
- the own-bundle-first rule for same-named CABs under `EnvOptions::batch()` (3.9);
- a failed dependency failing exactly the inputs that need it (3.10);
- sinks that may hold output across inputs, for the `prefer` winners (3.5).

**DESIGN's "each bundle and dependency parsed once per run, within `dep_cache_bytes`" does not hold.** Each attempt opens the bundles of its view in a dependency cache of its own, so a dependency that N inputs read is opened and decoded up to N times (2.2, its cost). The goal this design meets: within one attempt, each bundle of its view is opened at most once per residency in that attempt's cache, and the manifest counts every open and reopen. `extract.md` E14 says the same; DESIGN's row is the owner's to change (section 8).

**Two properties the design keeps.** Every mechanism in section 2 serves one of them or a requirement it names.
- **D, determinism.** An input's records are a function of three things only: its bytes, the bytes of the bundles in its view (2.3), and the spec. Its records are its outputs and their bytes, its skips, and its failure. They do not depend on:
  - `threads`, or the order inputs run or finish in;
  - what else is in flight;
  - which other inputs or bundles failed, or which others exist.

  2.10 shows why, against every counterexample design round 1's review built, and names the test. D has one limit: a failure of the run as a whole stops it instead of deciding a record. That is a host failure (an I/O error on the spool or a bundle's file, an allocation the OS refuses; 2.7), the spool's cap (2.7), or the discovered-bundle cap (2.3).
- **B, the memory bound.** The process's live heap is at most `max_inflight_uncompressed_bytes` plus constants fixed by the spec (section 5). Every byte that grows with an input or an output is charged to a budget before it is allocated.

---

## 2. The design

### 2.1 Phases

1. **Validate** the spec (`BATCH_SPEC.md` section 1). Nothing is written. A bad spec exits 78.
2. **Preflight** (E63). Take the spool's lock, check its free space and the process's resource limits, read the credentials, check that the sink is reachable, take the output root's lock (directory sink), read the stored manifest head for fencing, stat every local bundle and check its `size` and `sha256`, set the allocator (E57). A failure exits 69, or 6 when the stored head supersedes this attempt.
3. **Manifest `running`** (E38), fenced. No output is written before it.
4. **Download** every bundle given as a URL, streamed to the spool, at most `download_concurrency` at once (E7, E8).
5. **Extraction round 1** (2.4 to 2.8).
6. **Discovery round**, only when the spec has a URL rule and an attempt was deferred (2.3): fetch the names, then extraction round 2 for the deferred inputs.
7. **Source check** (E6): every local bundle is stat'd again. One whose size, modification time or inode changed fails every input whose view holds it (`source_changed`), before anything is decided from its records.
8. **Winners** (2.9): a pure function of the committed records.
9. **Publish** (2.9), from the spool, at most `download_concurrency` transfers at once.
10. **Manifest final** (E39), then the progress stream's `exit` line, then exit with the code of `BATCH_SPEC.md` section 9.

A stop (the deadline less the grace, SIGTERM, SIGINT, SIGHUP, the parent's death, stdin's EOF) can come in any phase. It takes the stop path of 2.11.

**Cost.** No decode overlaps a download. On the corpus (1,101 inputs, 1.3-1.8 GB a build) the downloads are a small part of a run, against decoding at 72-75 % of the CPU. That has not been measured end to end (section 7, A9).

### 2.2 Sealed attempts

An **attempt** is one run of one input's extraction. Each attempt is sealed: everything that holds memory or counts work during it belongs to it alone.
- **Its budget.** The attempt has one `MemoryBudget` object of its own, built by the engine from atomics. It never blocks, never calls the core and takes no lock (the budget contract, core 4.1; `budget/mod.rs`). It routes each reservation by its `Reservation`:

  | `Reservation` | limit | what books with it |
  |---|---|---|
  | `Input` | `M` | the attempt's meter `M_X`: its input's bytes, runs, entries, values, and the engine's buffers (2.6) |
  | `Dependency` | `D_X` | every meter of the attempt's private Environment: its opens, runs, entries, registry slots and keys |
  | `Provider` | refused | version 1 configures no typetree provider (A8), so nothing books here |

  `D_X` is `D` (`dep_cache_bytes`) when the spec has a URL rule, or when X's declared view (`BATCH_SPEC.md` 4.2 without `m_Dependencies`) holds another bundle; otherwise it is 0. With a URL rule, step 3 of 2.6 can add any declared or discovered bundle to the view, so `D_X` is fixed by the spec alone and is the same in both rounds.
- **Its Environment.** The attempt builds a private `Environment` with `EnvOptions::batch(ctx_X)`, `dep_cache_bytes` set to `D_X`, and registers its view in it (2.6). No other attempt reads through it. It is dropped when the attempt returns.
- **Its context.** `ctx_X` is a clone of the base `FileContext` with the attempt's budget. The base context's provider chain is empty and its codecs are stateless (`options.rs`).
- **Admission by reservation.** The attempt's **slot** is `S_X = M + D_X`. The controller dispatches an attempt only while the slots in flight sum to at most `I` with it (2.4). The budget's limits are its slot, so the attempts in flight together can never hold more than `I`, and no attempt's reservation is ever refused for what another holds.
- **The rest of the run.** Phases 8 and 9 run when no attempt does, under one **phase budget** of limit `I` (2.9).
- **Leak check.** When the attempt's function returns, every local of it is dropped: the bundle, the private Environment, every file, value and pin. Its worker then reads the budget's `Input` and `Dependency` use. Both must be zero; otherwise the run stops as an internal error (exit 70). Only then does the worker send the end message, and the controller count the slot free. Debug builds assert it.
- **Limits.** Every meter (an attempt's and its dependencies') has `max_input_memory` = `M` and `max_decode_work` = `4 M`, the core's default ratio (`Limits::default()`: 8 GiB and 32 GiB, `limits/mod.rs`).
- **Validation** (`BATCH_SPEC.md` 1.1) refuses a spec unless `M <= D`, `M + D <= I` and `I >= 64 MiB`. The first lets one dependency open at its cap fit the attempt's Dependency budget, its view's registry aside (5.6 sizes for that). The second lets every slot fit with nothing else in flight, so the head of the queue always runs eventually. The third gives phases 8 and 9 their working set.

**Cost.** This is what sealing pays for D.
- **Memory.** Each slot is reserved whole for the attempt's life, used or not. Parallelism is the largest number of slots that fit `I`, at most `threads`. For the corpus example (5.6), 8 slots of `M` = `D` = 256 MiB need `I` = 4 GiB, where design round 1 bounded 8 threads at 1.6 GiB. An input whose view is itself alone reserves only `M`.
- **Throughput.** No dependency is shared between attempts. An attempt reads every bundle of its view from the spool or its local path, whole (`load_dependency` reads a `Source::Path` whole, `env/mod.rs`, `read_path`), and decodes again every block it reaches. A dependency read by N inputs costs N opens and up to N decodes of what they reach. The page cache serves the repeated file reads. How much this costs on the corpus is not measured (A5). CR4 (section 6) would cut each registration to the bundle's header and directory.
- **What it removes.** The shared dependency cache, the Shared and Alone attempts, the Alone retry, the registration phase before extraction, the key table, flagging, and every request round 1 made for them (section 6).

### 2.3 Views and discovery

**Views.** `view(X)` (`BATCH_SPEC.md` 4.2) is X, the `deps` of every bundle in it, and, with a URL rule, the names in X's own AssetBundle object's `m_Dependencies`, closed the same way. `extract.md` E13's "needs" means "is in the view".

The view rules:
1. **A failed member fails X.** A bundle of `view(X)` that failed to download, failed its `size` or `sha256`, or whose load in the attempt is refused or fails on its bytes, fails X with `dependency_failed(<id>, <reason>)` (3.10), where the inner reason is the member's own. The exit code follows the inner reason (`BATCH_SPEC.md` 9.2): a budget or limit is 5, any other 4. One that failed before the attempt fails X before it starts. An I/O error reading a member's file is not a member's failure: it is a host failure and stops the run (2.7).
2. **Reads stay in the view.** The private Environment holds only `view(X)`, so every lookup resolves within it. A read whose CAB no bundle of the view holds is an object-level skip, `outside_view(cab)`.
3. **Same-named CABs** resolve by `BATCH_SPEC.md` 4.3: the reading file's own bundle first, then X, then the greatest id. This is a Tier C rule of `EnvOptions::batch()` only.
4. **Size.** A view of more than `max_view_bundles` bundles fails X, `view_too_large`. It bounds the uncharged heap a private Environment holds per registration (5.3).

**Discovery** (`extract.md` E9).
- An attempt reads X's AssetBundle object before anything else. When the spec has a URL rule, a name of `m_Dependencies` that is not a declared id must meet the discovered-name rule (`BATCH_SPEC.md` 4.1: at most 1,024 bytes, and no empty, `.` or `..` segment, so the URL rule's path cannot be walked); one that breaks it fails X, `invalid_dependency_name`. When such a name was not fetched yet, the attempt writes X's unfetched names, one per line, to its spool file `deferred/<input>` (`BATCH_SPEC.md` section 6), ends `deferred` and commits nothing else. The end message stays fixed-size.
- After round 1, with no attempt running, the controller reads the `deferred/` files in input order and builds the set of distinct names. When the set would pass `max_discovered_bundles`, the run stops before any fetch, exit 5 (`discovery_limit`): round 1's committed inputs keep their records, the deferred ones are `not_run`, and nothing is published. Otherwise it fetches each name by the URL rule to `in/x-<sha256 of the name, hex>` (2.11 bounds the wait) and runs round 2 for the deferred inputs. A bundle's spool path is a function of its name, so no table passes from the controller to an attempt.
- A name whose fetch fails fails every deferred input whose view holds it.
- The cap is a property of the whole run, not of an input, so it stops the run rather than fail a chosen input (1, D). It bounds the controller's state for discovered bundles (5.3).
- One round is enough: no attempt reads a dependency's own `m_Dependencies`, so after it every view is whole.
- **Tests:** `a_discovered_name_with_a_dot_segment_fails_its_input`; `the_discovered_bundle_cap_stops_the_run_before_any_fetch`; `a_view_grown_by_discovery_registers_on_a_full_dependency_budget` (an input whose declared view is itself alone, whose `m_Dependencies` names a declared bundle).
- Whether an input is deferred depends only on the spec and its own bytes, and a deferred attempt commits nothing, so deferral changes no record.

Without a URL rule, an `m_Dependencies` name outside the view is reported (`undeclared_dependencies`), never fetched, and stays out of the view.

### 2.4 The controller

**Workers.** `threads` OS threads, each with an explicit stack of twice `Limits::stack_needed` (core 3.15). Each runs one attempt from start to end, on that one thread.

**Order.** The ready queue is sorted by file size, largest first, then by bytewise id. Largest first shortens the tail. Order changes no record (D).

**The loop.** State: `ready` (sorted), `deferred`, `in_flight` (a count), `reserved` (the sum of the slots in flight).

```
loop:
  if the stop flag is set: return                                  // the stopper takes over (2.11)
  while in_flight < threads and ready is not empty and reserved + S(head) <= I:
      x = ready.pop(); dispatch(x); in_flight += 1; reserved += S(x)
  if in_flight == 0 and ready is empty: end the round
  wait for one event: an attempt ended | a 1 s tick | the stop flag's wake
  on attempt ended (input, outcome):              // sent after its budget read zero (2.2)
      in_flight -= 1; reserved -= S(input)
      match outcome:
          Done:        commit the attempt's directory (2.8)
          Deferred:    delete the attempt's directory (its names are in deferred/<input>); deferred.push(input)
          Failed:      commit its failure record only (E60)
```

- The queue is served strictly in order: the head waits until its slot fits. Every slot fits `I` alone, and every attempt returns (2.12), so the head always runs.
- The dispatch channel and the end-message channel each have capacity `threads`. At most `threads` attempts run, so no send blocks.
- An end message has a fixed size: the input, its outcome class, its counters and its peaks. Its records are in the spool, never in the message (2.8).
- Each input runs at most once per round.

### 2.5 (Removed)

Design round 1's Shared and Alone attempts. Sealing makes every attempt the canonical one, so there is nothing to retry.

### 2.6 An attempt

1. **Budget and meter.** Build the attempt's budget (2.2) and `M_X = Meter::for_input(&limits, budget, Reservation::Input)`.
2. **Its own open.** `bytes = read_path(file, &limits, &M_X)` (`env/mod.rs`). Then `check_file_type_with_reader(&bytes, &limits, ReaderKind::Stream)` (`detect.rs`) and `parse_file_with_reader(name, bytes, ty, None, &M_X, &ctx_X, ReaderKind::Stream)` (`files/mod.rs`): the rules the core uses for a `Source::Path`, and UnityPy's for a path. A file that is not a bundle fails X, `not_a_bundle`.
3. **View.** Read X's AssetBundle object: `m_Dependencies` and its container rows. Compute `view(X)` (2.3), charged to `M_X`. Defer when 2.3 says so.
4. **The private Environment.** Register `view(X)` less X with `load_dependency(Source::Path(file))`, one at a time in bytewise order of id, then X last when the view holds another bundle (`env/load.rs`). Drop each returned `File`, so the private LRU holds the only strong handle. A load that fails fails X (2.3, rule 1), and the attempt makes no read through that Environment after it (CR1 explains why).
5. **Select** (`BATCH_SPEC.md` section 3). Walk the entries and objects in object order, filter by `class_id()` from the object table (no value read), and read typetrees only for the selected objects.
6. **Export one sheet group at a time.**
   - Sprites are grouped by their sheet texture (`BATCH_SPEC.md` 4.4). For each group, decode the sheet (and its alpha texture), render every sprite of the group, then drop the sheet.
   - NGUI atlases are cut one at a time (`BATCH_SPEC.md` section 5). A plain texture is its own group.
   - This replaces UnityPy's per-file texture cache with a cache of size one, so the export buffers are bounded by the largest sheet (5.5). The output is the same: the cache changes when a sheet is decoded, not what it decodes to.
7. **Reads into other bundles.** A pointer is dereferenced with `Environment::deref` on the private Environment, from the file that holds it (`env/deref.rs`). Before its next Environment call, the attempt takes the target's top-level `File` from its `Parent` and holds it while it uses the target (core 4.1, *a dropped container is an error, never a guess*). What is read is copied into the attempt's charged buffers, and every handle into another bundle is dropped when that read ends.
8. **Resources** (own bundle first, core Q12). Take the candidates of `resource_candidates(path)` (`env/resource.rs`) in UnityPy's order. For each, look in the reading file's own container chain for an entry whose simplified name matches: its container's directory, then each enclosing container's, up to the top-level bundle (`BundleFile::directory`, `parent`). Read a match with `entry_range` through that handle (`files/bundle/mod.rs`). Only when no candidate is an own entry, read through `Environment::resource(from, path, offset, size)` on the private Environment, which covers a candidate in another bundle and one that is a top-level raw file (`env/resource_data.rs`). The engine's copy of the candidate rule is checked by `own_first_resources_equal_environment_resource_without_same_named_cabs` (3.9) until CR3 moves the rule into the core.
9. **The container** comes from X's own `SerializedFile::container()` (`files/serialized/mod.rs`), never from `Environment::container()`.
10. **Engine buffers** (`extract.md` E59). Every buffer whose size an input chooses is charged to `M_X` at `heap_cost` before it is allocated: RGBA and alpha sheets, the texture's source data, sprite canvases, crops, masks, PNG rows and output, atlas JSON, the sprite-group table, the record buffer. zlib's deflate state is charged as a constant before `deflateInit`, as the core charges LZMA's decoder state (`codec/lzma.rs`). The charges go through `Meter::charge` or `DecodeBuf` (`limits/meter.rs`, `budget/mod.rs`): the core's rule of 3.15, applied to engine code.
11. **Copy-out** (2.8). Between objects the attempt checks the stop flag; a stopped attempt returns at once and commits nothing.
12. **End.** The function returns, which drops everything above. The worker reads the budget (2.2), then sends the end message.

### 2.7 Outcomes

Every error is classified once, by one engine function, `classify` (CR2 would make it the core's).

| what failed | outcome |
|---|---|
| a refusal of the attempt's budget or of one of its meters: `Budget(Input)`, `Budget(Dependency)`, any `Limit` | recorded where it surfaced: an object-level skip inside an object's read or export; an input failure while opening X or an entry of X; `dependency_failed(<id>, <reason>)` while loading a view member (2.3, rule 1). Its reason names the budget, or the limit and its configured value |
| an error the bytes decide: a corrupt block or entry, a typetree, an unsupported format, a decoder panic (E18), a key that breaks a rule, E25's bytes | DESIGN section 4's failure model: a container or entry failure fails X, an object's failure skips it |
| a host failure: an I/O error reading X's file or a view member's file, or writing the spool | the run stops, exit 74 |
| a spool claim refused, or free space below the reserve | the run stops, exit 5: spool use depends on the order inputs ran in, so it decides no input |
| `DerefError::ContainerDropped` | the run stops, exit 70: the attempt holds every container it reads through (2.6, step 7), so it marks an engine bug |
| an error `classify` does not know | the run stops, exit 70, so a new core variant is noticed at once |

**Why every refusal is final at once.** The attempt runs on one thread in program order. Its budget and every meter that books with it serve that thread alone (2.2). So each charge meets the same state in every run, and each refusal, and each growth a refusal silently shrank (`limits/vec.rs`, `grow_counted`; `budget/mod.rs`, `DecodeBuf::into_bytes`), falls at the same point. No outcome waits for a retry.

**`classify` until CR2.** One module matches the public enums: `Error`'s `Entry`, `Tree`, `AssetBundle`, `Deref`, `Limit` and `Budget` variants, `DerefError`'s `ContainerDropped` and `Load`, `ParseError`'s `Read`, `Tree` and `Deref`, `ReadError`'s detail, `TreeError` and `ChargeError`. A `DerefError` that `Environment::deref` returns bare is classified as that same chain. Every `#[non_exhaustive]` enum's wildcard arm returns "unknown". Its test builds each wrapper path and checks the class.

**An allocation the allocator refuses** comes back from the core as a `Budget` refusal (`limits/meter.rs`, `refusal`). Until CR2 tells it apart, preflight refuses to start (exit 69) when `RLIMIT_AS` or `RLIMIT_DATA` is below 1.5 times 5.1's bound (glibc's slack, 5.4) plus 64 MiB per arena of `M_ARENA_MAX` (each arena reserves that much address space), so the allocator does not refuse below it. One that still refuses is recorded as the budget's refusal, which is A13.

### 2.8 Copy-out, records and commit

- **Spool layout and lifecycle:** `BATCH_SPEC.md` section 6.
- **Each output:**
  1. It is encoded in charged memory, and its MD5, CRC-32 and visible hash (E20) are computed.
  2. Its size is claimed from the spool counter, an atomic try-add against `spool.size_bytes`, after a `statvfs` check against `spool.reserve_bytes`.
  3. It is written to the attempt's directory, and its buffer is dropped.
- **Records.** Candidate records (key or unit, input, kind, path id, MD5, CRC-32, visible hash, mode, size, file), skip records and diagnostics are appended to the attempt's `records.ndjson` through a fixed 64 KiB buffer charged to `M_X`. A record longer than the buffer is written through it in pieces. An atlas unit is written as one unit line and one line per member.
- **Commit** (`extract.md` E60, all or nothing per input).
  - An attempt that ends `Done` has its directory renamed to `done/<input>/` by the controller.
  - An input that fails commits only its failure record: no candidate, so it competes for no key.
  - A deferred attempt's directory is deleted.

### 2.9 Winners and publish

**Winners** (E31 to E34, E61; `BATCH_SPEC.md` 4.5 and 4.6), in phase 8, after the last round. Each step below is an external sort of spool records and one streaming pass over the result, so phase 8 holds no table that grows with keys, units or candidates.
- **The sort.** Runs whose records and index together take at most 32 MiB, sorted in memory and written to `winners/`; then merges of at most 16 runs, with a 1 MiB buffer per run and 1 MiB for the output. A record line is at most 8 KiB (a key and an id are each at most 1,024 bytes).
- **Step 1, units.** Sort every unit line by (unit key, input id). One pass decides each unit key's winning unit by E31 and E61, keeping only the best candidate so far: within a key the candidates come in input order, so rules 1 to 3 need no more.
- **Step 2, member keys.** Sort one line (member key, unit key) per key of every winning unit, its unit key included. One pass: of the units claiming one key, the first by unit key keeps it, and every other is withheld whole (`unit_conflict`). Every key a winning unit claims stays reserved, even when its unit is withheld, so the pass is not repeated (`BATCH_SPEC.md` 4.5).
- **Step 3, plain keys.** Merge the sorted plain candidates with step 2's reserved keys. A plain candidate for a reserved key loses (`unit`). Every other key is decided by E31 and E61, streaming as in step 1. The output is the set `P` of keys the decision would publish, in key order.
- **Step 4, prefix and archive conflicts** (`BATCH_SPEC.md` 4.6). Sort `P`'s keys and the archives' keys, each with its ancestors (`a`, `a/x` for `a/x/y.png`) as markers. One pass refuses a key of `P` that is an ancestor of another key of `P` (`prefix_conflict`), or that equals, contains or lies under an archive's key (`archive_conflict`). A unit with a key so refused is withheld whole: its unit keys are sorted, and a merge with `P` marks its other keys `unit_conflict`. The check is not repeated.
- With `publish_incomplete: false` nothing is published when any input failed.
- The winners and the collisions are merged into `winners/` in key order. The decision is a pure function of the committed records, so it is the same whatever order the inputs finished in.
- **Its working set** is at most 40 MiB: a run of 32 MiB, or a merge's 17 MiB, beside at most 8 stream buffers of 1 MiB. `I >= 64 MiB` holds it, so a refusal of the phase budget in phase 8 marks a leak, exit 70.

**Publish**, in phase 9.
- **Resume** (E43). With `resume` set, the object store's LIST of the output prefix (keys in ascending UTF-8 byte order) is merged with the sorted winners file, one LIST page in memory at a time. A winner whose key is listed with its MD5 as ETag (an archive's, or a file's over 16 MiB: its multipart ETag at 16 MiB parts, `BATCH_SPEC.md` section 8) is `present` and not PUT. A LIST page out of order, or a LIST that fails, degrades to "PUT everything" with the warning `resume_list_failed`. In a directory, each winner's destination is stat'd, and hashed only when its size matches.
- **Transfers.** A transfer reserves its buffer from the phase budget once, at its start: `heap_cost(size)` for a single-part PUT (files up to 16 MiB), or one 16 MiB part for a multipart upload, whose parts go one at a time. It holds that reservation until it ends. The scheduler starts a transfer when a `download_concurrency` permit is free and its reservation is granted; otherwise it waits for a running transfer to end. A reservation refused with no transfer running is an internal error (exit 70): every buffer is at most 16 MiB, below `I`.
- **The directory sink** holds the root's lock (`BATCH_SPEC.md` 7.2) for the process's life, writes each file as a temp file named `.unityrs-tmp-<run_id>-<n>` beside its destination and renames it, opens paths beneath the root (E62), and runs `syncfs` before the final manifest. No key segment may start with `.unityrs-tmp-` (`BATCH_SPEC.md` 2.3), so the start's cleanup (E62) removes only temp files.
- **The zip format** (`BATCH_SPEC.md` section 8) streams each archive from the spooled members, with its central directory spooled while the members stream.
- **The object store** follows `extract.md` E37. Each multipart upload id is appended to the journal after its create and before its first part, and removed after its complete or abort (E64).
- **The manifest** (`BATCH_SPEC.md` section 7): its row files are streamed from the spool to the sink; then the head, at most 64 KiB, is written fenced.

### 2.10 Why D holds

**The argument.** An attempt is a program run on one thread. Everything it reads is fixed by D's three inputs:
- its bytes and its view's bytes, read whole from files that do not change during the run (E6; the source check of phase 7 fails what changed);
- the spec: its limits, `M` and `D_X`, its rules;
- the registration order, `BATCH_SPEC.md` 4.3, which depends only on the ids in the view.

Everything it writes to is its own: its budget, its meters, its private Environment's registry, LRU, cells and work counts. No other thread books with them (2.2). The core keeps no state across threads but the budget and the provider chain (A12), and version 1's provider chain is empty. So the attempt computes the same records in every run. A deferred attempt commits nothing, and round 2's attempt starts from nothing again.

**Against round 1's counterexamples.**

| counterexample | why a sealed attempt is immune |
|---|---|
| a cold decode's transient peaks above a warm read's cached run, so a read succeeds only after another input cached the run | no attempt reads a run another attempt cached. Every read of the attempt meets only runs its own earlier reads cached, in program order |
| a growth or a shrink silently cut back by a pool another attempt filled, which moves a later refusal of the input's own meter | the attempt's budget is filled only by its own charges |
| a dependency's growth ladder settled lower under contention, so a read site fits warm and not cold | as above, for the Dependency side |
| overlapping entry spans: which runs are cached, and their cost, depend on which reader came first | the attempt's private opens see only its own reads |
| a flagged input's private registry takes room a shared one does not | every attempt registers exactly its view, in one order, so its registry's charges are a function of its view |
| a registration refused by a pool whose fill depends on every other bundle registered | registration happens inside the attempt, on its own budget |
| a failed load that published some of its CABs before it failed | a failed load of a view member fails X, and no read follows it (CR1) |
| a dependency's work allowance used up by other inputs' reads | each attempt opens its own copy, with its own allowance |
| a same-named CAB in a bundle outside the view, or its failure | the private registry holds only the view |
| a lookup that reaches a dropped slot of a bundle outside the view | as above |
| a nested container registered when first reached, by whichever attempt got there | only this attempt reaches its private Environment's containers, in program order |

**Tests.**
- `an_inputs_records_do_not_depend_on_what_runs_beside_it`. The engine scene runs at `threads` 1, 2 and 8; with the inputs in 20 seeded orders; with `I` at the smallest valid value and at 4 times it; with an unrelated dependency that fails to download; with a bundle added that holds a same-named copy of a CAB in another input's dependency. Every input's records must equal those of a run whose spec holds only that input and its view, timings and counters aside.
- `round_one_counterexamples_change_no_record`: one scene per row of the table above, each compared with its solo run.
- `an_attempt_books_with_no_budget_but_its_own`: a test budget records the thread of each reservation; every reservation of an attempt comes from its worker, while its attempt runs.
- `a_view_member_that_fails_to_load_fails_the_input_and_nothing_reads_after_it`.

### 2.11 Lifetime and the stop path

**Threads.** The controller (the main thread), `threads` workers, the lifecycle thread, the progress thread, and, once a stop starts, the stopper. Downloads and transfers are async tasks on a current-thread runtime that only the controller drives, with `block_on`. No async task calls the core.

**The lifecycle thread** owns the stop decision and the hard exit. Its only I/O is reading its self-pipe and stdin.
- Signal handlers (SIGTERM, SIGINT, SIGHUP) only write to a self-pipe. The thread waits on that pipe, on a timer, and, when the spec asks, on stdin's EOF (E46).
- **The deadline** is converted once at start to a `CLOCK_BOOTTIME` instant. The stop starts at `deadline - grace` (E45).
- **Parent death.** At start the binary sets `PR_SET_PDEATHSIG` (SIGTERM), then compares `getppid()` with `parent_pid` when the spec gives it, else with 1. A mismatch (the parent died before the call) stops at once (E46).
- **On a stop** it sets the stop flag, cancels the runtime's cancellation token, wakes the controller, spawns the stopper, and arms the **hard exit** at `T_hard = min(deadline, now + grace) - 500 ms`.
- **At `T_hard`**, if the process still runs, it calls `libc::_exit(6)`. It writes nothing first.
- **A second stop signal** calls `_exit(6)` at once.
- A stop after the final manifest's write changes nothing.

**What the stop flag stops.**
- The controller dispatches nothing more, leaves its loop and parks until `_exit`: `main` never returns during a stop, which would run exit handlers under the stopper. It joins no worker and no transfer, and drives its runtime no more.
- Each attempt returns at its next object, and commits nothing.
- Each download and transfer future selects on the cancellation token, so a request in flight is dropped at once. A request the server had already received may still complete there.
- No transfer starts a committing request after the flag: no PUT, no `CompleteMultipartUpload`, no rename. No new multipart upload starts.
- Phase 8 checks the flag between sort runs.

**The stopper** runs on a current-thread runtime of its own, with a store client of its own, built at the stop from the credentials and sink settings preflight read. It never uses the controller's client: that client's pooled connections are driven by tasks on the controller's runtime, which nobody drives after a stop, so a request through it could wait out its timeout. In order:
1. **Close the journal.** Under the journal's mutex it marks the journal closed, then reads it. A transfer that creates an upload after the close cannot journal it, and aborts it itself; if it is cancelled first, the bucket's lifecycle rule removes it (E64).
2. **Abort** every upload id the journal holds, in parallel, at most `download_concurrency` at once, each request with a timeout of `min(5 s, T_hard - now - 2 s)`.
3. **The final manifest**, status `incomplete` with the stop reason, fenced (`BATCH_SPEC.md` 7.2). Its rows come from the spool: the inputs whose directories are in `done/` with their records, every other input `not_run`. Nothing is published after a stop, so no output row says `written` but those whose PUT was acknowledged before it.
4. **The `exit` line.** The stopper hands it to the progress thread, which writes it before anything else and never overwrites it, and waits for the progress thread to report it written, at most `min(1 s, T_hard - now)`.
5. `libc::_exit(6)`.

**When the grace runs out first,** the lifecycle thread's `_exit` ends the process. The stored head then says whatever its last write said: `running` with its deadline when the final write did not land. A reader takes a `running` head past its deadline as a dead attempt. The `exit` line may be missing; the exit status is authoritative.

**What cannot be bounded.** Every wait above has a timeout or a cancellation but one: a filesystem call (a write, a rename, `fsync`, `syncfs`, `statvfs`) on a filesystem that stops answering. Linux cannot interrupt a thread in such a call. The plan assumes that the spool and the output directory are local filesystems whose calls complete (A11). When one does not, `_exit` still runs at `T_hard`, but the kernel ends the process only when the stuck call returns, so the process may outlive its deadline. The caller's own timeout and `SIGKILL` are the last resort, and the stored head says what it last said.

### 2.12 What an attempt never waits on

- on a budget: its budget only `try_reserve`s (core 4.1, *budgets only `try_reserve`*);
- on another attempt: nothing it uses is shared (2.2);
- on a core lock held by a waiter: every core lock is held only to look up or to publish (core 4.1, *no caller code runs under a core lock but the budget*);
- on the controller, once it has started: its end message's send never blocks (2.4);
- on an engine lock: the attempt takes none;
- on the network: every download it needs ended before its round;
- on the spool: a claim is an atomic try-add that refuses, and a write is a local file call (A11).

---

## 3. Open problems, and their answers

Each problem keeps the text that states it, then its answer: solved or shown wrong, the mechanism, what it rests on, and its test. Engine test names are proposals, in `crates/unityrs-batch/tests/`. Sections 3.11 to 3.13 are problems design round 1 found; 3.14 is the one its review found.

### 3.1 Cached runs never release, and budgets are bound per load, not per job

P1 round 1. Under the old step 1 every input was loaded into the shared Environment, which never evicts caller-loaded files, and each kept its decoded runs and parsed entries for the Environment's life, so the in-flight cap was reached and never released. A per-job budget could not reach a bundle opened with another `FileContext`. A dependency read from job X into input Y decoded Y's blocks on a budget nobody admitted.

**Solved** (2.2). Each attempt owns its input's only job handle and its private Environment, both on its own budget, and drops both when it returns; the worker reads the budget at zero before the slot is freed. A read into Y from X is a read through Y's copy in X's private Environment, on X's Dependency budget. **Rests on** core 4.1 *one meter per input, charged before every input-sized allocation*, *a second handle on the same bytes is independent*, *`Environment::load_dependency(source)`*. **Tests:** `every_attempt_returns_its_charges` (after every attempt, both sides of its budget read zero and `Arc::strong_count(M_X) == 1`); `an_attempts_end_message_holds_no_core_type` (a source scan of its fields against `Bytes`, `File`, `ObjectRef`, `Parsed`, `HeldCab`, `Charge`).

### 3.2 The prepaid estimate is too small, and the alone path can hang

P1 round 2. The estimate had to cover what the core's rules can book, and was still an engine guess against charges made at actual sizes; "Alone" at a cap above the in-flight bound waited forever.

**Shown wrong as a useful estimate; solved without one.** No estimate is provable below `max_input_memory`: an LZMA block's output ignores its declared size (core 3.6), and up to `max_zero_size_elements` values per object read consume no input (`limits/mod.rs`). Admission reserves each attempt's slot, `M + D_X`, which is the provable bound, and validation requires `M + D <= I`, so the head of the queue always fits once nothing else runs. **Tests:** `the_controller_always_progresses` (a model of the loop with fake attempts of random slots, durations and outcomes: no state has an input queued with nothing running); `a_hostile_lzma_block_fails_only_its_input` (a block declared at 1 byte that decodes to 64 MiB, among 50 small inputs: it fails on its own meter, and every other input's records equal its solo run's).

### 3.3 No eviction mechanism, resource reads through the shared handle, untagged refusals

P1 round 2. Registry handles were never released; a job's own `.resS` was served by the shared handle; both pools refused with one error.

**Solved.** The core's LRU evicts within each private Environment, bounded by `D_X`, and its pool-refusal retry evicts the others (core 3.13). A job's own resources are read through its own handle (2.6, step 8). Refusals are tagged `Input` or `Dependency` by the core, and 2.7 records either as final. **Tests:** `a_dependency_cache_smaller_than_the_view_costs_reopens_not_a_scheduling_dependent_record` (the same records at `threads` 1 and 8 with `D` = `M`); `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job` (3.8).

### 3.4 The first admission can wait forever, and the estimate is inflated

P1 round 3, in the plan's and the code's reviews. A bundle whose estimate was above the in-flight bound never fit, and the FIFO controller hung every bundle behind it; the estimate counted one block once per entry.

**Solved: there is no estimate.** Validation's `M + D <= I` replaces the cap, and an input whose needs pass its budget fails on it at once (2.7). **Tests:** `a_spec_whose_slot_exceeds_the_inflight_bound_is_refused`; `an_input_above_its_meter_fails_once_and_the_run_goes_on`.

### 3.5 What a consumer holds is charged to a job that may never wait

P1 round 3, in the plan's and the code's reviews. A slow sink's backlog was charged to the job; a sink that held output across jobs waited on a job the controller would not admit.

**Solved: copy-out, with winners decided after the last round** (2.8, 2.9). An attempt hands no core type to anyone: it writes outputs and records to the spool. Winners are a pure function of the committed records, decided once every input has run, and sinks run only in phase 9, so a slow sink lengthens the run and changes no record. **Cost:** the spool holds every candidate until phase 9 (5.7). **Tests:** `winners_are_the_same_whatever_the_completion_order`; `a_stalled_sink_never_stalls_an_attempt` (every request held open: the rounds end, and the run exits 6 by its deadline); `an_atlas_wins_as_one_unit`; `a_failed_input_commits_no_candidate`; `a_key_whose_preferred_input_failed_is_withheld`; `a_unit_with_a_refused_member_key_is_withheld_whole` (a member key that loses to `prefix_conflict`, `archive_conflict` or another unit); `phase_8_stays_within_its_working_set` (a million candidates of one key, and a million keys, under the counting allocator).

### 3.6 The old step 1 named a load the API did not have

P1 round 3. The only way to open a source into the Environment gave an `Input` meter that was never evicted.

**Solved.** Every registration is `load_dependency(Source::Path)` (core 3.13), in a private Environment with no resolver, so no lookup can start a download or a registration. **Test:** `a_deferred_input_matches_a_run_that_declared_its_dependency`.

### 3.7 Problems settled in the core that the design must not undo

| finding | the core property now (core 4.1), and its use here |
|---|---|
| P1 round 1: a codec allocated before the budget saw it | one meter per input, charged before every input-sized allocation. The engine's buffers follow the same rule (E59) |
| P1 round 2: a retained `Parsed` carried no charge | every `Parsed` carries its charge. Values never leave an attempt (copy-out) |
| P1 round 3: a cell published a pool refusal for good | transient refusals are never cached. Inside one attempt, a refusal that `with_room`'s retry clears is not kept for the attempt's later reads |
| P1 round 3: resolving a resource parsed the shared slot | resource reads through `from`'s own handle. Not needed: the engine reads own entries through the job's handle first (2.6) |
| P1 round 3: eviction kept a `Path` dependency's bytes | eviction frees what it can re-read. Every registration is a `Path` |
| P1 round 3: a dereference from a dropped bundle was unspecified | `DerefError::ContainerDropped`. The attempt holds the target's top-level `File` while it uses the target (2.6) |
| P1 round 3: decode work under `Lazy` was unbounded | `max_decode_work` per open input; a reopen restarts the count (P1 round 4). It bounds every attempt's work (2.12) |
| implementation review round 6, the Environment's MEDIUM-3: a load published its keys one at a time | each key's registration is published whole, and a load returns after all its keys (core 3.16 item 6). The attempt registers its whole view before its first read (2.6) |

### 3.8 The resource redirect and eviction cannot both hold for a job's second handle

P1 round 4. The core redirects a resource read through the job's handle only for a slot it can match without reading; an evicted `Path` slot loses the redirect, and a `Bytes` slot keeps its bytes for the run's life.

**Solved by needing neither.** The engine reads a job's own entries through its handle before any registry lookup (2.6), so every registration is `Source::Path` and eviction frees its bytes. **Cost:** an input that its own view's other bundles point back into is also registered in its private Environment and read whole a second time. **Test:** `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job`: its own resource reads charge nothing to its Dependency budget.

### 3.9 Same-named CABs: the own-bundle-first rule

`docs/plans/core.md` section 9, Q12 (decided 2026-10-30) gives this plan the rule.

**Solved; the rule lives in the engine until CR3 moves it into the core.** Resources: own container chain first (2.6). The container: X's own `SerializedFile::container()`. Pointers: the core's `deref` from the reading file, then the private registry, where X is registered last (`BATCH_SPEC.md` 4.3). `EnvOptions::unitypy()` is untouched. **Tests:** `own_bundle_first_for_resources_and_container` (two inputs each hold `cab-x` and `cab-x.resS` with different pixels, and each lists the other; each input's textures and container rows are its own, under `EnvOptions::batch()` and an Eager variant); `a_pointer_from_a_dependency_reads_that_dependencys_sibling_first`; `own_first_resources_equal_environment_resource_without_same_named_cabs` (the engine's rule against `Environment::resource` under `EnvOptions::unitypy()` on every testkit scene without same-named CABs; `PARITY.md` section 19.2's row).

### 3.10 A failed dependency fails the inputs that need it

A dependency that fails to download or parse fails every input that needs it, and no other (DESIGN.md section 4's failure model).

**Solved** (2.3). "Needs" means "is in the view". A bundle fails when its download fails after its retries, its `size` or `sha256` does not match, or its load in an attempt fails. Inputs whose view does not hold it never open it. A read that fails for a dependency's own bytes found at read time (a corrupt block or entry) is an object-level skip of the input that read it. **Tests:** `a_failed_dependency_fails_exactly_the_inputs_whose_view_holds_it` (A declares D and reads it, B declares nothing, C declares D and never reads it, W declares U, which holds a CAB named like one of D's; D fails by a 404, a truncated body, a corrupt header and a corrupt data block. For the first three A and C fail and B's and W's records equal a run where D is whole; for the corrupt block A skips the objects that reach it and C is unaffected); `a_shadowing_bundle_outside_the_view_changes_no_record`.

### 3.11 A cached dependency's work allowance is shared by every input that reads it

Found in design round 1. A cached dependency read by many inputs built one shared work count, and a used-up allowance was kept by its cells for every later reader.

**Solved** (2.2). Each attempt opens its own copy, with its own allowance. **Test:** `round_one_counterexamples_change_no_record`.

### 3.12 A direct read of a dependency object gets no evict-and-retry

Found in design round 1. A value read on a dependency's `ObjectRef` is no Environment call, so its refusal gets no eviction and retry (core 3.13), and could be refused by a pool full of evictable entries.

**Shown not to matter for D.** Inside a sealed attempt, whether such a read is refused depends only on the attempt's own earlier reads, so it is the same in every run. It can refuse a read that would fit after an eviction: a cost in outcomes, the same in every run. CR2's classifier does not change it; a core request for the retry on direct reads is a follow-up (section 8).

### 3.13 A same-named CAB in a bundle an input does not need can change its answers

Found in design round 1. Ranks give a CAB key to the registration of greatest rank, so a bundle outside X's view, or its failure, could change X's reads.

**Solved** (2.3). The private Environment holds only `view(X)`. **Test:** `a_shadowing_bundle_outside_the_view_changes_no_record`.

### 3.14 A shared dependency cache makes outcomes depend on what runs beside an input

Found in design round 1's review. Design round 1's D argument rested on "a read that succeeds anywhere holds at least its cold charges". It does not: a decode's transient peaks above the run it leaves, growths and shrinks react silently to refusals of a shared pool, overlapping spans make cached runs order-dependent, and a private Environment's registry took room a shared one did not. Each gave a record that depended on another input.

**Solved by sealing** (2.2, 2.10), at the cost 2.2 states.

---

## 4. Why it cannot deadlock

Every wait in the process:

| # | who waits | for what | why it ends |
|---|---|---|---|
| W1 | the controller, phases 5 and 6 | one event: an end message, a 1 s tick, the stop flag's wake | the tick; attempts only send, on a channel of capacity `threads` |
| W2 | the controller, in `block_on` (phases 2, 3, 4, 6, 9, 10) | network requests | each request has a timeout (`BATCH_SPEC.md` 1.4 and 1.6), and `block_on` selects on the cancellation token |
| W3 | an idle worker | the next attempt on the dispatch channel | it holds nothing, and the controller dispatches before it waits whenever a slot fits |
| W4 | an attempt | a core lock | held only to look up or publish, never across a decode or another lock; the only caller code under one is the budget, which is atomics (core 4.1) |
| W5 | an attempt, the controller, the stopper | a local filesystem call | A11 |
| W6 | a transfer | a `download_concurrency` permit, or the phase budget | held only by transfers inside requests, which end by W2 |
| W7 | a transfer or the stopper | the journal's mutex | held for one append, or for the stopper's close; the file call inside is W5 |
| W8 | the progress thread | stdout | nothing waits on it but the stopper, for at most 1 s (2.11) |
| W9 | the lifecycle thread | the self-pipe, its timer, stdin | nothing waits on it; it does no I/O but those reads and `_exit` |
| W10 | the stopper | abort requests and the manifest's writes | on its own runtime and client (2.11); each request's timeout is inside `T_hard`; file calls are W5 |
| W11 | the spool's and the output root's locks | `flock` with `LOCK_NB` | never waits: busy exits 69 |
| W12 | the controller, after a stop | nothing: it parks | the lifecycle thread's or the stopper's `_exit` |

**There is no cycle.** Rank the parties: 0, the kernel, the network and stdout; 1, core locks, the budgets and the journal's mutex; 2, a running attempt; 3, transfers and the stopper; 4, the controller; and the lifecycle thread apart. Every wait-for edge goes to a lower rank, or to rank 0 with a timeout, except W3's, which comes from an idle worker that holds nothing. The lifecycle thread waits on nothing in the process, and its `_exit` at `T_hard` ends every other wait but a stuck file call (A11). The core blocks nowhere (core 4.1, *the core never waits and never starts a thread*). No engine lock is held across a core call or a network request.

**Tests.** Every engine test runs under `unityrs_testkit::guard`, which fails a hang instead of waiting forever. The scenes include a stalled sink, a stalled download server, a stdout that is never read, and a stop in each phase; `a_wedged_attempt_cannot_pass_the_deadline` (a test decoder spins, and the process exits 6 by the deadline); `a_stop_during_a_download_exits_within_the_grace`. `the_engine_starts_no_unbounded_wait` is a source scan in the core's style (`unityrs_testkit::waits`): it finds every `recv`, `lock`, `join`, `wait`, `park`, `flock` and `block_on` in `unityrs-batch` and checks each against this table, by module, and that no lock guard is alive across a call into `unityrs_core` or an `.await`.

---

## 5. The memory argument

### 5.1 The bound

In the charge unit (`heap_cost`, glibc's chunk, core 3.15), at every instant:

```
heap <= I + C_thr + C_env + C_net + C_spec + C_base
```

**Proof.**
1. Every allocation that grows with an input or an output is charged before it is made. The core charges its own (core 4.1, *one meter per input, charged before every input-sized allocation*). The engine charges its buffers, the attempt's view, the record buffer (2.6), the sort's runs and buffers, the resume merge's page and the transfers' buffers (2.9) under the same rule.
2. During the rounds, each charge books with one attempt's budget, whose limits are its slot. The controller dispatches while the slots in flight sum to at most `I`, and frees a slot only after its budget read zero (2.2).
3. In phases 8 and 9 no attempt runs, and every charge books with the phase budget, of limit `I`.
4. Each budget refuses any reservation that would pass its limit, in one compare-and-swap.
5. Everything else the process allocates is a constant term of 5.3.

Records, candidates, the winners, the zip central directory and the manifest's rows live on disk (2.8, 2.9), so no term grows with the number of outputs, skips or keys in the store.

### 5.2 Every holder of charged bytes

| holder | budget | taken | released |
|---|---|---|---|
| the attempt's copy of its input's bytes | the attempt's `Input` | its start | its end |
| the job handle's runs, parsed entries, compiled trees, kept errors (core 3.6, 3.15) | `Input` | as read | its end |
| the values of X's objects | `Input` | per object | after that object's outputs |
| a block decode's transients: `DecodeBuf`'s old and new room while it grows, LZMA's dictionary and state | whichever meter decodes | during the decode | at its end |
| export buffers, one sheet group at a time | `Input` | per sheet | when its last output is spooled |
| the view, the sprite-group table, the record buffer | `Input` | the attempt | its end |
| the keys of a lookup | the meter of the lookup's `from` | during the call | at its return |
| each private open: a `Path` source's bytes, its runs and entries | the attempt's `Dependency` | load, or reopen after eviction | eviction, or the attempt's end |
| the private registry's slots and keys | `Dependency` | registration | the attempt's end |
| a read's handles into another bundle | `Dependency` | the read | the read's end |
| the sort's runs and merge buffers | phase | phase 8 | before phase 9 |
| the resume merge's LIST page; a transfer's buffer; the manifest head | phase | phase 9 | its end |
| outputs, records, downloads, sorted runs, archives, row files | disk (spool) | their write | the run's end (`BATCH_SPEC.md` section 6) |

### 5.3 The constant terms

| term | value | source |
|---|---|---|
| `C_thr` | `threads x (2 x stack_needed + 0.11 MiB) + 4 x 2 MiB` | each worker's stack of twice `Limits::stack_needed` (release: 1.75 MiB, `limits/mod.rs`; 2.4), counted whole, plus its detection scratch, inflater state and window and tally (0.11 MiB, core 3.15); the controller, lifecycle, progress and stopper threads at 2 MiB each |
| `C_env` | `threads x (c_env + max_view_bundles x 512 B)` | each private Environment's fixed parts, uncharged (core 3.15, one per Environment), and per registered bundle the uncharged `Meter::OWN_HEAP`, `Dependency` record and path (`limits/meter.rs`, `env/load.rs`), 512 B at most (A1) |
| `C_net` | `2 x download_concurrency x c_net` | connection, TLS and a 64 KiB streaming buffer per transfer, `c_net <= 2 MiB` (A1), for the controller's client and for the stopper's (2.11). Downloads stream to the spool through that buffer; upload payloads are in the phase budget |
| `C_spec` | 4 x the spec's size + 256 B x the declared bundles + 1.25 KiB x `max_discovered_bundles` | the parsed spec and the controller's per-bundle state (A1); a discovered bundle's state holds its name, at most 1,024 bytes (2.3). A spec over 16 MiB is refused, and the discovered bundles are capped, so every factor is fixed by the spec |
| `C_base` | measured | the binary, the runtime, the progress thread's slots, the controller's queues |

There is no provider term: version 1 configures no typetree provider (A8), so the provider cache (`ProviderChain::DEFAULT_CACHE_BYTES`, 64 MiB, one per `ProviderChain` shared by every cloned context; CR5) holds nothing.

### 5.4 Outside the charge unit

- **glibc's slack.** A live allocation whose chunk is below 128 KiB may hold up to 16 bytes past its charge (`CHUNK_SLACK`), at most half again of those allocations' charges (core 4.1, *one meter per input*; `limits/mod.rs`). So in glibc's own bytes the worst case is `1.5 x I` plus the constants. The budgets are dominated by large mapped buffers (runs, pixels, PNG output), whose pages cover the slack, so the excess is a few percent in practice. The nightly reports it.
- **Allocator retention.** glibc raises its mmap threshold each time a mapped chunk is freed, after which large buffers come from arenas that do not return memory (`limits/mod.rs`; `tests/heap_rounding.rs`). `extract.md` E57: the binary calls `mallopt(M_MMAP_THRESHOLD, 128 KiB)` and sets `M_ARENA_MAX` to `threads + 2` at start. Fragmentation of small chunks inside arenas is not bounded by the core's rules (A7). The nightly measures peak RSS against the bound.
- **Page cache.** Spool writes and file reads fill the kernel's page cache: reclaimable and outside the heap, but counted by a memory cgroup (A7). The spool writer drops each file's pages after closing it (`posix_fadvise(DONTNEED)`).

### 5.5 Arithmetic on the corpus

**A block decode's transient** (`limits/vec.rs`, `budget/mod.rs`, `codec/lzma.rs`).
- `DecodeBuf` doubles its capacity and, while it grows, charges the new room beside the old. `into_bytes` charges the exact size beside the old capacity before it moves the bytes.
- For an output of `B` bytes with final capacity `c_n < 2B`, the charge peaks at `c_(n-1) + c_n < 3B` during the last doubling, and at `B + c_n < 3B` at `into_bytes`.
- LZMA adds its dictionary, `min(clamp(declared, 4 KiB, 8 MiB), named)` plus 576 B, and 33,040 B of state.

For the corpus's largest block, `B` = 69.4 MB = 66.2 MiB: `c_(n-1)` = 64 MiB and `c_n` = 128 MiB, so the transient peaks at 192 MiB + 8 MiB + 32 KiB = 209.8 MB. That input's measured peak is 212 MB, which also holds its file bytes and its values, so the transient is most of it; the 2 MB left is not checked against those terms (A5). The fix in progress, which sizes the room to the declared size, brings the transient to about `B + 8 MiB`, consistent with the measured 145 MB once values are added.

**One attempt's Input peak** is `P_X = s_X + runs_X + transient_X + meta_X + values_X + export_X`:
- `s_X`, X's file size; `runs_X`, at most the declared decoded size plus one more copy of each block a span straddles (core 3.6); `transient_X < 3 B_X + 8 MiB + 33 KiB` before the fix; `values_X`, the objects being exported, one at a time;
- `export_X` for one sheet group of `L` x `H` pixels: the texture's source data (up to `4 LH x 4/3` with mips, plus its alpha texture's), RGBA and alpha sheets of `4 LH` each, the image being encoded (`4 LH`), PNG output of at most `4 LH + H` plus deflate's overhead, filter rows of `5 (4 L + 1)`, and about 262 KiB of deflate state. So `export_X` is about `24 LH` for sprites and atlases and `13 LH` for a plain texture: 2048 x 2048 takes 96 MiB for sprites, 4096 x 4096 takes 384 MiB.

**One attempt's Dependency peak** is the private registry (about 300 B per CAB of the view, A4) plus the opens its reads keep within `D_X`, plus one decode's transient.

### 5.6 Sizing rules, and an example

Each rule is a recommendation the manifest reports against, not a validation.

| goal | rule |
|---|---|
| every input that fits at all completes | `M >= max P_X` (E40's input peak) and `D >= max` dependency peak |
| full parallelism | `I >= threads x (M + D)` |
| the spool fits | `spool.size_bytes >= downloads + all candidates + records + threads x the largest attempt's outputs + the sort's runs + the largest archive's central directory` |

**Example for the corpus:** `threads` 8, `download_concurrency` 8, `M` = `D` = 256 MiB, `I` = 4 GiB.
- The constants come to about 100 MiB: 37 MiB of thread stacks, 5 MiB of private Environments at `max_view_bundles` 1,024, 32 MiB for 8 transfers on each of two clients, 5.6 MiB of spec and controller state at `max_discovered_bundles` 1,024, and about 20 MiB of base.
- So the bound is about 4.1 GiB in the charge unit, and about 6.1 GiB under glibc's worst-case slack.
- The corpus's largest input peaks at 212 MB before the decode fix, inside `M`. An input whose view is itself alone reserves 256 MiB, so the 8 slots reserve between 2 and 4 GiB.

### 5.7 Disk

`spool.size_bytes` bounds the sum of the downloads, every committed candidate and its records until phase 9, the partial outputs of the attempts in flight, the deferred inputs' name files, phase 8's sorted runs, archives' central directories while they stream, and the manifest's row files. A free-space check against `spool.reserve_bytes` runs before each claim. A refused claim stops the run (2.7).

### 5.8 How it is checked

- `every_attempt_returns_its_charges` (3.1), and `the_slots_in_flight_never_exceed_the_inflight_bound`.
- X12 in `extract.md`'s gate: the core's counting allocator (`unityrs_testkit`) over the engine's export paths. The heap never holds more than the charges plus 5.3's constants, as `tests/memory_model.rs` checks for the core.
- X13: each testkit input run with `M` at its peak less one byte, then at its peak, where its peak is the largest over its meter and every dependency meter of its run. It is refused the same way every time, then gives the records of an unbounded run.
- `rss_tracks_live_heap_after_large_frees`, with and without E57's settings.
- The nightly writes `VmHWM` into the manifest and checks it against 5.1's bound for its spec.

---

## 6. Requests to the core

Each request goes through the core's integration rule. None changes a core 4.1 property, and `EnvOptions::unitypy()` is untouched by all of them.

| request | change | property it adds, and its test | what the engine does until it lands |
|---|---|---|---|
| **CR1, wanted** | `load_dependency` publishes nothing when it fails: a registration refused after some of its keys were inserted (`env/load.rs`, `try_load_dep`; `env/registry/register.rs`) removes them before it returns the error | a failed load leaves the registry as it was. Test: a bundle whose registration is refused at its k-th entry, for every k, after another bundle registered a same-named CAB; every lookup after it answers as before the load | the attempt fails X on any failed load of its view (2.3, rule 1) and makes no read through that Environment after it, so the partial slots are never read. Granted, an attempt could skip a failed member that its reads never reach; D does not need it |
| **CR2, wanted** | `Error::refusal(&self) -> Option<Refusal>`, with the same on `DerefError` and `ParseError`: `Refusal::{Budget(Reservation), Limit(LimitKind, MeterKind), Allocator(Reservation), Io}`, where `MeterKind` says whether the refused meter was a dependency's. The allocator's refusal (`limits/meter.rs`, `refusal`) gets its own variant | one public classification of every error the engine must route, stable across new variants. Test: every wrapper path the core can build (`Error` → `DerefError` → `ParseError` → `ReadError` → `TreeError` → `ChargeError`) classifies as its innermost cause | 2.7's `classify` over the public enums, with unknown variants stopping the run (exit 70); preflight's resource-limit check stands in for the allocator variant (A13) |
| **CR3, wanted** | `EnvOptions::own_bundle_first`, true in `batch()` and false in `unitypy()`: a resource read's candidates and the AssetBundle container are looked up first in `from`'s own container chain | Q12's rule in the core, where its candidate order and slice rule live. Needs a `PARITY.md` section 19.3 row and Q12's tests in both load modes | the engine's copy (2.6, step 8) and its differential test (3.9) |
| **CR4, wanted** | a `Source::Path` registration that reads only the bundle's header, blocks-info and directory, and reads the rest of the file when a data block is first reached | registration costs the directory, not the file. Test: registering an N-byte bundle charges at most its header, blocks-info and directory, and a later read gives the same bytes as a whole-file load | every registration reads its file whole (2.2's cost) |
| **CR5, text only** | core 3.15 and its constants' table: the provider cache's bound is per `ProviderChain`, shared by every context that clones its `Arc` | none (a correction) | 5.3 says why it is not a term in version 1 |

**Withdrawn with design round 1's shared cache:** `Environment::evict_dependencies`, a public `with_room`, `deref_held`, and the `DecodeWork` request. A sealed attempt's Environment is dropped whole, and no outcome waits for a retry.

---

## 7. Assumed (not checked)

- **A1.** Sizes not measured: a transfer's connection and TLS state is at most 2 MiB (`c_net`); a parsed spec at most 4 times its JSON; a registered bundle's uncharged heap at most 512 B. Test: the counting allocator on a 100 MiB download, a spec at the cap, and a view of `max_view_bundles` bundles.
- **A4.** A registry entry costs about 300 B per CAB, from core 3.13's list, not measured. It sizes `D`; the budget enforces whatever it is.
- **A5.** Not measured on the corpus: how many bundles a view holds; how many inputs read each dependency, and so how many times sealing reopens and re-decodes it (2.2's cost); the total candidate bytes of a build, which sizes the spool; how much of 5.5's 212 MB peak is file bytes and values.
- **A7.** With E57's settings, glibc's arena retention keeps peak RSS within a few percent of the live heap. The core's rules do not bound fragmentation, and a memory cgroup also counts the spool's page cache. The nightly measures both.
- **A8.** Version 1 configures no typetree provider: the spec has no field for one, and the base context's chain is empty. The corpus's typetrees are embedded.
- **A9.** Download time is a small part of a run on the corpus, so finishing downloads before extraction costs little (2.1). The figures give sizes and the CPU's split, not wall times.
- **A10.** The MonoBehaviour script hook (`Environment::object_tree`) dereferences `m_Script` inside the core. It matters only for stripped MonoBehaviours with a script provider, which version 1 does not configure.
- **A11.** The spool and the output directory are local filesystems whose calls complete. A filesystem that stops answering can hold a thread in a call Linux cannot interrupt, past `T_hard` (2.11).
- **A12.** The core keeps no state that one thread's call leaves for another's, except the budget and the provider chain. Its non-test thread-locals are in `limits/meter.rs`: the pool-refusal count, which a call reads as a difference, and the grant frames, which each lazy computation opens and closes. So what an earlier attempt on the same worker left in them changes nothing. Test: `an_inputs_records_do_not_depend_on_what_runs_beside_it` at `threads` 1, where every attempt follows another on one thread.
- **A13.** Under preflight's resource-limit check, the allocator does not refuse an allocation within 5.1's bound. When it does (strict overcommit, a cgroup that refuses mappings), the core reports a budget refusal (`limits/meter.rs`, `refusal`), which the engine records as one, and the record then depends on the host. CR2 removes the assumption.

(A2, A3 and A6 were design round 1's assumptions about the shared cache, and went with it.)

---

## 8. Open, for the review rounds

1. **Sealing is new.** Its cost (2.2) is stated but not measured (A5). A review should look for any state an attempt reads that another attempt can write.
2. **E9 is kept, with discovery between rounds** (2.3). Dropping its fetch clause and requiring a complete graph would remove the discovery round. That is the owner's choice (`extract.md` section 3).
3. **DESIGN.md's `unityrs-batch` row** still says "each bundle and dependency parsed once per run, within `dep_cache_bytes`". Section 1 says why it does not hold; the row, and `dep_cache_bytes`'s meaning there, are the owner's to change.
4. **Overlapping downloads with decoding** is given up (2.1). A later version may start an attempt once its view's downloads have ended.

**Follow-ups** (round 1's LOW findings not fixed in this text, and what sealing leaves for later):
- A shared dependency cache that keeps D. It needs the core to charge a read at its cold cost whatever is cached, growth ladders that ignore budget refusals, and run caching independent of read order. Only worth asking for once A5 shows sealing's re-decodes cost enough.
- The core's evict-and-retry for direct reads on a dependency's handle (3.12), which would let a read fit that a full cache refuses today.
- `{container}` in key templates (`extract.md` E27).
- Pattern-based archive member maps, and a spec field for the manifest's location (`BATCH_SPEC.md` 1.7 and 7.1).
- Pinning the PNG writer's zlib to the oracle's version, and `PARITY.md`'s IDAT chunk size, `max(65536, 4 x width)` (`extract.md` E19).
- An optional prune of keys under the output root that the manifest does not list.
- Check the NGUI atlas's material field name against NGUI's serialized layout (`material`, or `mMat` in some versions) before the cutter is written (`BATCH_SPEC.md` section 5).

## 9. Before it is built

- This plan's review rounds, run until a round finds no MEDIUM or HIGH problem, with section 8 settled.
- CR1 to CR5 answered. None is needed for correctness; each has its interim.
- `extract.md`'s gate (its section 5) agreed. It holds the engine's tests named here: D (2.10), budgets and charging, the deadlock guard and scans (section 4), and the memory rows (5.8).
