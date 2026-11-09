# P1b plan: unityrs-batch, the engine (draft)

**Status: draft, design round 1 (2026-11-09), not reviewed.** P1b is 2D batch extraction (`docs/DESIGN.md` section 5; called P2 until 2026-11-09, when the phase table gave the 2D work its own phase). This file is the engine's plan: registration, admission, eviction, the hand-off to sinks and refusal. The rest of the phase (the job spec, decoding, images, sinks, the manifest, releases) is `docs/plans/extract.md`, which relies on the engine meeting sections 1 and 2.
- Until P1's third review round, `docs/plans/core.md` section 4.1 described a batch flow. Each review round found new problems in its admission and eviction, and the batch engine is not built in P1, so the flow left the core plan for this file. Section 3 lists every problem found in it.
- Design round 1 (2026-11-09) put three designs before a panel of judges and merged the winner with what the judges took from the other two. Section 2 is that design. It replaces the old flow, which is gone from this file; section 3 marks each problem solved or shown wrong, and adds three the panel found.
- Nothing here is built. Before `unityrs-batch` is, this plan gets its own review rounds, run until a round finds no MEDIUM or HIGH problem.

**What it builds on.** The core API and what it guarantees any batch engine: `docs/plans/core.md` section 4.1, a list of properties each checked by a P1 test. This plan relies on those properties, on the public API named where it is used, and on nothing else the core does. A need the list does not cover is a request to the core, made through its integration rule (section 6), not an assumption. Where the design can work without a request, the request says what it saves.

**Citations.**
- P1's review findings are cited by the name of the review file that holds them and the finding's id. Those files belong to the review process and are not in the repository; each problem in section 3 restates what it found.
- Design round 1's findings are cited by what they say. Its files are not in the repository either.
- "core 4.1, *property*" names a row of `docs/plans/core.md` section 4.1 by its first column. "core 3.13" is a section of that plan. A source path without a crate is under `crates/unityrs-core/src/`.
- Corpus figures are aggregates from the private corpus, measured on the P1 core before its perf fixes.

---

## 1. Goal

DESIGN.md section 4's `unityrs-batch` row: a JSON job spec of inputs, outputs and resource budgets (`threads`, `max_inflight_uncompressed_bytes` among them), run in parallel with bounded memory, streaming outputs to sinks, with the run marked `incomplete` when an input fails.

`docs/plans/extract.md` adds what the engine must give the rest of P1b:
- inputs that each know their dependencies by name (the spec's graph, 3.6);
- the own-bundle-first rule for same-named CABs under `EnvOptions::batch()` (3.9);
- a failed dependency failing exactly the inputs that need it (3.10);
- sinks that may hold output across jobs, for the `prefer` winners (3.5).

**DESIGN's "each bundle and dependency parsed once per run, within `dep_cache_bytes`" cannot hold as written.** Under eviction a dependency is reopened after it is evicted, and an input that other inputs read is opened by its own job and again as a dependency (3.8). The goal this design meets instead: each dependency is opened at most once per residency in the dependency cache, and the manifest counts every reopen (`EnvStats`, `env/mod.rs`). `extract.md` E14 says the same; DESIGN's row is the owner's to change (section 8).

**Two properties the design keeps.** Every mechanism in section 2 serves one of them or a requirement it names.
- **D, determinism.** An input's records are a function of three things only: its bytes, the bytes of the bundles in its view (2.3), and the spec. Its records are its outputs and their bytes, its skips, and its failure. They do not depend on:
  - `threads`, or the order inputs run or finish in;
  - what else is in flight or cached;
  - which other inputs or dependencies failed, or which others are registered;
  - any refusal that was retried.

  An input's **canonical run** is an Alone attempt (2.5) made from an empty dependency cache. Section 2.10 shows that every run gives the canonical records, and names its test. D has two limits. A nested container inside a bundle is one (2.10; core Q10). The other is a spool or index refusal, which stops the whole run instead of deciding one input's records (2.7, 2.9).
- **B, the memory bound.** The process's live heap is at most the two pools' limits plus a constant fixed by the spec (section 5). Every byte that grows with an input or an output is charged to a pool before it is allocated.

---

## 2. The design

### 2.1 Phases

1. **Validate** the spec (`extract.md` E1, E5, E48). Nothing is written. A bad spec exits 78 (`extract.md` section 3, item 5).
2. **Preflight** (`extract.md` E63). Take the spool's lock, check its free space, read the credentials, check that each sink is reachable, read the stored manifest for fencing, and set the allocator (E57). A failure exits 69, with nothing published.
3. **Manifest `running`** (E38), fenced by the spec's `attempt`. No output is written before it.
4. **Download** every input and declared dependency given as a URL, streamed to the spool, at most `download_concurrency` at once (E7, E8).
5. **Register** (2.3), on the controller's thread, with no job running.
6. **Extraction round 1** (2.4 to 2.8).
7. **Discovery round**, only when the spec has a URL rule and an attempt was deferred (2.3). Fetch, register, then extraction round 2 for the deferred inputs.
8. **Winners** (2.9): a pure function of the committed records.
9. **Publish** (2.9), from the spool, at most `download_concurrency` transfers at once.
10. **Manifest final** (E39), then the progress stream's `exit` line, then exit with `extract.md`'s code.

A stop (the deadline less `shutdown_grace`, SIGTERM, SIGINT, SIGHUP, the parent's death, stdin's EOF) can come in any phase. It takes the stop path of 2.11.

**Cost.** No decode overlaps a download. Registration needs every bundle's directory, and every rank fixed, before the first job (2.3). On the corpus (1,101 inputs, 1.3-1.8 GB a build) the downloads are a small part of a run, against decoding at 72-75 % of the CPU. That has not been measured end to end (section 7, A9).

### 2.2 Budgets and pools

The engine builds its own `MemoryBudget` and `PermitHome` objects out of atomics. They never block, never call the core and take no lock, which is the budget contract (core 4.1; `budget/mod.rs`). Each reservation is routed by its `Reservation` (`budget/mod.rs`):

| pool | what books with it | limit |
|---|---|---|
| **Input pool** | every attempt's meter, through the attempt's home; in phase 8, the winners' index; in phase 9, the transfers' buffers | `I` = `max_inflight_uncompressed_bytes` |
| **Dependency pool** | every dependency meter, of the shared Environment and of an attempt's private Environment (2.3): opens, runs, entries, registry slots, the engine's key table, and what attempts pin | `dep_cache_bytes + F` |
| `Provider` | unbounded: a TPK's own caps bound it (core 3.12), and the provider cache is one per `ProviderChain`, shared by every context cloned from the base (section 5.3) | none |

- **Per-attempt home.** Each attempt's budget is a thin wrapper over the Input pool, with its own `used` and `peak` atomics. That is one home per attempt; the budget contract forbids a home per reservation, not this (`budget/mod.rs`, the contract's docs).
  - When the attempt returns, its worker reads `used`. It must be zero. Otherwise the run stops as an internal error (exit 70), so a leak never leaves the books short. Debug builds assert it.
  - The controller counts the attempt finished only after that read.
  - `peak` is the input's peak, reported in the manifest (E40).
- **Limits.** Every meter of the run (attempts', dependencies' and private Environments') has `max_input_memory` = `max_decoded_bytes` (written `M` below). Its `max_decode_work` is `4 M`, the core's own default ratio (`Limits::default()`: 8 GiB and 32 GiB, `limits/mod.rs`).
- **F, the registry floor.** F is the Dependency pool's use right after registration and an eviction of every dependency (CR1). It holds the registry's slots and keys and the engine's key table (2.3), which are charged to dependency meters and never freed during a run (core 3.13 and 3.16; the key table by the engine).
  - The pool's limit is set to `dep_cache_bytes + F` by an atomic store while no job runs, and again after a discovery round's registrations.
  - The LRU's bound stays `dep_cache_bytes` (`EnvOptions::dep_cache_bytes`).
  - So the registry's permanent charges never come out of the room the cache and the reads have.
- **Validation** (E48) refuses a spec unless `M <= I` and `M <= dep_cache_bytes`.
  - The first lets an Alone attempt be refused only by its own meter, never by the Input pool (2.5).
  - The second lets one dependency open at its cap fit the pool after a full eviction. With it the LRU never evicts the entry a call has just used, since that entry's meter is within the bound alone (2.6; A6).
  - A read site may touch two opens (2.6), so the sizing rules recommend `2 M` (section 5.6). Below that, a read site too large for the pool is refused the same way in every run, which D allows.

### 2.3 Registration and views

**Registration.** It happens on the controller's thread, while no job runs.
- The engine calls `Environment::load_dependency(Source::Path(file))` (core 4.1, *`Environment::load_dependency(source)`*) for **every input and every dependency** the spec names. It calls them one at a time, in bytewise name order, on one Environment built with `EnvOptions::batch(ctx)`: `resolver: None`, and `dep_cache_bytes` from the spec (`env/mod.rs`).
- Opening reads only each bundle's header, blocks-info and directory (core 4.1, *opening a bundle reads only its header, blocks-info and directory*).
- From each returned `File` the engine reads the directory's CAB names, keyed as the registry keys them (`pystr::simplify_name`). It adds each to its **key table** (CAB key → the registered bundles that hold it), charged to the Dependency pool through a meter of the engine's own. It then drops the `File`, so the LRU holds the only strong handle (`load_dependency`'s docs, `env/load.rs`).
- A failed load marks that bundle failed.

Why every input, and why all of it before the first job:
- **Every input.** A dependency may point back into an input's CAB. With every input registered, that read resolves the same way whether or not another input declares the input. Registration reads headers only, so it costs about 300 B of registry per CAB (A4), under 1 MiB for the corpus's 3,139 files.
- **Before the first job.** A load's keys appear one at a time (core 3.16 item 6; 3.7's last row), and a load's number ranks same-named keys (core 3.13, **Ranks**). No job may see either while it runs.

**Views.** An input's view is what it needs, so `extract.md` E13's "needs" means "is in the view". `view(X)` is the union of:
- `X` itself;
- the closure of X's declared list through the spec's graph, where a name that is itself an input adds its own list;
- when the spec has a URL rule, the names in X's AssetBundle's `m_Dependencies`, and their closure.

The view rules:
1. **A failed member fails X.** A bundle of `view(X)` that failed to download or to load fails X before its first attempt (or at discovery), with `dependency_failed(name, reason)` (3.10).
2. **Reads stay in the view.** Every read X's attempt makes in another bundle resolves within `view(X)`. A read whose CAB no bundle of `view(X)` holds is an object-level skip, `outside_view(cab)`. The skip and its reason are the same whether some other bundle holds that CAB or none does. The manifest's run-level diagnostics name the other bundle; X's records do not.
3. **Same-named CABs within a view.** X's own bundle comes first (core section 9, Q12). After it comes the bundle whose name is greatest in bytewise order. This is a Tier C rule of `EnvOptions::batch()` only. `EnvOptions::unitypy()` is unchanged.
4. **Flagged inputs.** X is **flagged** when some CAB key held by a bundle of `view(X)` is held by two or more registered bundles.
   - An unflagged input reads through the shared Environment. Every key of its view has one registered holder, so the registry's answer (its registration of greatest rank) is rule 3's answer.
   - A flagged input's attempt builds a **private Environment**, with `EnvOptions::batch` on the dependency context. It registers the bundles of `view(X)` in bytewise order with X last, each through `load_dependency(Source::Path)`, so its ranks give rule 3's answer. Its meters book with the Dependency pool, and it is dropped with the attempt.
   - Flagging picks the mechanism, never the answer, so which inputs are flagged changes no record. The manifest lists them, with every same-named CAB (`same_named_cabs`).

**Discovery** (`extract.md` E9). An attempt reads X's AssetBundle object before anything else.
- When the spec has a URL rule and `m_Dependencies` names a bundle that is neither registered nor failed, the attempt ends `deferred` at once and commits nothing.
- After round 1, with no job running, the controller:
  1. fetches each such name by the URL rule;
  2. registers the new bundles in bytewise order;
  3. measures F again and sets the pool's limit;
  4. rebuilds the key table and the flags;
  5. runs round 2 for the deferred inputs.
- A name whose fetch or load fails fails every deferred input whose view holds it.
- One round is enough. A discovered bundle is a dependency, and no attempt reads a dependency's own `m_Dependencies`, so after one round every view is whole.
- Bundles registered after round 1 change no record of an input that ran in round 1. That input's view did not hold them, and rule 2 kept its reads inside its view.

Without a URL rule, an `m_Dependencies` name outside the view is reported (`undeclared_dependencies`), never fetched, and stays outside the view. So the view, and with it every record, is the same whether or not another input declares that name.

### 2.4 The controller

**Workers.** `threads` OS threads, each with an explicit stack of twice `Limits::stack_needed` (core 3.15). Each runs one attempt from start to end, single-threaded inside it. A meter used by one thread in program order refuses at the same point in every run, which 2.7 needs.

**Order.** The ready queue is sorted by declared `uncompressed_size()` (known after registration, `files/bundle/mod.rs`), largest first, then by spec order. Largest first shortens the tail; the corpus's largest single block decodes to 69.4 MB. Order changes no record (D).

**The loop.** State: `ready` (sorted), `alone` (FIFO), `deferred`, `in_flight` (a count), `alone_running` (a flag).

```
loop:
  if a stop was requested: take the stop path (2.11)                      // no join
  if not alone_running:
      if alone is not empty:
          if in_flight == 0:
              evict every dependency (CR1)
              dispatch(alone.pop(), Alone); alone_running = true
          // else dispatch nothing: wait for the attempts in flight to end
      else:
          while in_flight < threads and ready is not empty: dispatch(ready.pop(), Shared)
  if in_flight == 0 and ready and alone are empty: end the round
  wait for one event: an attempt ended | a 1 s tick | a stop
  on attempt ended (input, kind, outcome):       // sent after the attempt's home read zero
      in_flight -= 1
      if kind == Alone: alone_running = false
      match outcome:
          Transient (Shared only): delete the attempt's spool directory; alone.push(input)
          Deferred:                delete the attempt's spool directory; deferred.push(input)
          Done:                    commit the attempt's directory (2.8)
          Failed(reason):          record the failure; commit no candidate (E60)
```

- `dispatch` increments `in_flight` and sends the attempt on a channel the workers read. That channel never holds more than `threads` attempts.
- An attempt's end message has a fixed size: the input, the attempt number, its kind, its outcome class, its counters and its peak. Its records are in the spool, never in the message (2.8).
- The message channel's capacity is `threads`. At most `threads` attempts run, so a send never blocks.
- `alone_running` keeps every Shared dispatch out while an Alone attempt runs. Once `alone.pop()` empties the queue, nothing else would stop the next pass of the loop from refilling the workers.

### 2.5 Shared and Alone

- A **Shared** attempt runs beside others.
- An **Alone** attempt is dispatched only when `in_flight` is 0, after CR1 evicts every dependency, and nothing is dispatched beside it until it ends.
- Each input runs at most one Shared and one Alone attempt per round. A deferred input runs again only in round 2.

**Why Alone ends.**
- It waits only for `in_flight` to reach 0. An attempt never waits (2.12), and its work is bounded: core 4.1, *bounded work per open input*; core 3.15's per-object limits; and the engine's own loops, bounded by objects and by sheet area. So every attempt in flight returns.
- Its own run is bounded the same way.
- Phases 8 and 9, the only other users of the Input pool, run after the last round.

**What an Alone attempt can meet.**
- The Input pool holds only its charges, since every other attempt's home read zero. With `M <= I`, its own meter refuses before the pool. A `Budget(Input)` refusal in an Alone attempt is an internal error.
- The Dependency pool holds F plus what this attempt causes.
- Every dependency open it makes starts cold. CR1 has dropped the LRU's handles, nothing else holds one, and a reopen has a fresh work allowance on the same memory account (`Meter::reopen`, `limits/meter.rs`; core 4.1, *bounded work per open input*).

### 2.6 An attempt

1. **Meter and bytes.** `M_X = Meter::for_input(&limits, job_budget, Reservation::Input)`, then `bytes = read_path(file, &limits, &M_X)` (`env/mod.rs`). The input's bytes are charged to `M_X`.
2. **Its own open.** `bundle = BundleFile::open(name, bytes, None, &M_X, &ctx_X)` (`files/bundle/open.rs`). The job's context `ctx_X` is a clone of the base context with the job's budget. This is the job's handle. It is independent of the input's registered copy (core 4.1, *a second handle on the same bytes is independent*).
3. **View.** Read X's AssetBundle object: `m_Dependencies` and its container rows. Compute `view(X)` and its flag (2.3), charged to `M_X`. Defer when 2.3 says so. Build the private Environment when X is flagged.
4. **Select.** Walk the entries and objects, filter by `class_id()` from the object table (no value read), and read typetrees only for the selected objects.
5. **Export one sheet at a time.**
   - Sprites are grouped by their sheet texture (file, path id). For each group, decode the sheet (and its alpha texture), render every sprite of the group, then drop the sheet.
   - NGUI atlases are cut one at a time. A plain texture is its own group.
   - This replaces UnityPy's per-file texture cache with a cache of size one, so the export buffers are bounded by the largest sheet (section 5.5). The output is the same: the cache changes when a sheet is decoded, not what it decodes to.
6. **Reads into other bundles** go through read sites (below).
7. **Engine buffers** (`extract.md` E59). Every buffer whose size an input chooses is charged to `M_X` at `heap_cost` before it is allocated: RGBA and alpha sheets, sprite canvases, crops, masks, PNG rows and output, atlas JSON, the record buffer. zlib's deflate state is charged as a constant before `deflateInit`, as the core charges LZMA's decoder state (`codec/lzma.rs`). The charges go through `Meter::charge` or `DecodeBuf` (`limits/meter.rs`, `budget/mod.rs`): the core's rule of 3.15, applied to engine code.
8. **Copy-out** (2.8).
9. Between objects the attempt checks the stop flag.
10. **End.** The attempt's last act is its end message. Returning drops the bundle, the private Environment, every file, value, slice and pin, and so every charge on `M_X`.

**Read sites.** Every read that reaches a bundle other than X's own goes through one engine function, `dep_read`, as one **read site**. A read site is one of:
- a pointer's target object and the value read from it;
- a texture's value and its image data (its `.resS`, in the texture's own bundle or another bundle of the view);
- a name read across a pointer (E21).

Inside a read site:
- **The CAB is resolved before any read.** The engine takes the pointer's external's name (`SerializedFile::externals`, `files/serialized/mod.rs`), simplifies it, and resolves it within `view(X)` by 2.3's rules 2 and 3. On the shared Environment the engine checks that the registry answered with that holder: `HeldCab::hold` (`env/registry/cab.rs`), or the target's top-level bundle through its `Parent`.
- **The target's top-level `File` is taken before the next Environment call.** It comes from `get_cab_held`'s `hold` (`env/deref.rs`), or with the target under CR5. It is held beside each dependency object while that object is used (core 4.1, *a dropped container is an error, never a guess*).
- **Pins last only for the read site.** What is read is copied into the attempt's charged buffers, and every pin is dropped when the read site ends. A read site pins at most two dependency opens (a texture's bundle and its `.resS` bundle), and never holds a pin across read sites.

**The resource rule** (own bundle first, core Q12).
- Take the candidates of `resource_candidates(path)` (`env/resource.rs`) in UnityPy's order. For each candidate, look in the reading file's own container chain for an entry whose simplified name matches: its container's directory, then each enclosing container's, up to the top-level bundle (`BundleFile::directory`, `parent`). Read a match with `entry_range` through that handle (`files/bundle/mod.rs`).
- Only when no candidate is found there, look in the registry within `view(X)`.
- `Environment::resource` and the resolver are not used.
- The rule copies the core's candidate order and slice rule. A differential test (`own_first_resources_equal_environment_resource_without_same_named_cabs`) compares it with `Environment::resource` under `EnvOptions::unitypy()` on every testkit scene without same-named CABs, until CR3 moves the rule into the core. That case is the rule's own Tier C row (`PARITY.md` section 19.2).

**The container** comes from X's own `SerializedFile::container()` (`files/serialized/mod.rs`), never from `Environment::container()`. Its pointers resolve through `deref`, which reads the job's own siblings first through its `Parent` (Tier A; core 4.1, *a second handle on the same bytes is independent*). `Environment::container_path` is not used: it refuses a dependency's object under evicting options (core 3.13).

### 2.7 Outcomes, and the one retry rule

Every error is classified before it becomes a skip or a failure. Inside `dep_read` the error is **dependency-side**; outside it, **job-side**.

| what failed | in a Shared attempt | in an Alone attempt |
|---|---|---|
| dependency-side, inside `dep_read`: `Budget(Dependency)`; a `Limit` of kind `InputMemory` or `DecodeWork`; `ContainerDropped`; an I/O error reopening a dependency. A refusal of the job's own meter met inside `dep_read` (a lookup's key) is counted here too, which costs an Alone run and never changes a record | the attempt ends `Transient` and is queued Alone | drop the read site's pins, evict every dependency (CR1, on each Environment the attempt reads through), redo the read site once; a second failure is the outcome: an object-level skip of what the read site fed, naming the pool or the limit |
| inside `dep_read`, an error the dependency's bytes decide: a corrupt block, a bad entry, a `Limit` of another kind (a depth, a count). The core keeps these in its cells (core 4.1, *transient refusals are never cached*), so every reader meets the same one | an object-level skip naming the dependency; not a failed dependency (3.10) | the same |
| job-side, transient: `Budget(Input)`, or I/O on the job's own file | `Transient`, queued Alone | internal error (`Budget(Input)`; 2.5), or an input failure (I/O) |
| job-side `Limit` on the job's own meter (`InputMemory`, `DecodeWork`, depths, counts) | deterministic: recorded as it surfaced, an input failure for the container, an entry or a block, a skip for an object | the same |
| outside `dep_read`, other input-determined errors: a corrupt block, an entry's parse, a typetree, an unsupported format, a decoder panic (E18), an unwritable key, E25's bytes | as DESIGN section 4's failure model: a container or entry failure fails X, an object's failure skips it | the same |
| a spool claim refused, or free space below the reserve | the run stops as a budget refusal (exit 5): spool use depends on the order inputs ran in, so it decides no input | the same |

**Why the job side is deterministic.** The core counts `InputMemory` transient (`ChargeError::is_transient`, `error.rs`) because a dependency's memory account is shared. A job's meter is shared with nothing:
- it counts X's own handle, X's buffers, and the keys of X's lookups while they run (core 3.13, **Names**);
- a registration charges the slot's dependency, whichever handle read the entry (core 3.16, R2);
- the attempt runs on one thread in program order.

So `M_X` refuses at the same point in every run, and its refusal is final at once, with no Alone run.

**Why the dependency side is not deterministic in a Shared attempt** (3.11, 3.12).
- A dependency's memory account is shared by every open of it that is alive and by every pin into it. `Meter::reopen` keeps the one account (`limits/meter.rs`).
- Its work allowance is shared by every reader of the open: "every read and write of an object's value counts its work toward the open's" (core 4.1, *bounded work per open input*; `walk/work.rs`).
- `DecodeWork` is not transient (`error.rs`). Once the allowance is used up, the cell keeps the refusal (core 4.1, *transient refusals are never cached*).
- A read the caller makes on a dependency's handle gets no evict-and-retry (core 3.13: "its refusal is the caller's, and nothing is evicted").

Each of these depends on what other attempts read, held and cached before.

**Why the Alone retry is deterministic and never refuses what would fit.**
- After CR1, with the read site's pins dropped, every open the read site touches is reopened. Each starts with its account at its registrations and a fresh work allowance (`Meter::reopen`).
- The read site's charges are then its **cold charges**: the source's bytes, the runs it needs, its entry and its value. They are a function of the read site alone.
- So a second failure is the same in every run. A first refusal caused by the attempt's own earlier read sites (runs cached on an open, or work used up on it) is never the outcome.
- This is the engine's evict-and-retry for the direct reads core 3.13 leaves untried. CR2 would give Shared attempts the core's retry too and save Alone runs; correctness does not need it.
- Cold work: one read site's work on one fresh open is its runs, its entry's parse and its value. Each is bounded through `M`, together under the `4 M` allowance (A2). So the canonical run never ends in `DecodeWork` on a dependency.

### 2.8 Copy-out, records and commit

- **Spool layout:**
  - `spool/in/`, the downloads;
  - `spool/attempts/<input>.<n>/`, one attempt's outputs and its `records.ndjson`;
  - `spool/done/<input>/`, a committed attempt;
  - `spool/uploads.journal` (2.9);
  - `spool/manifest.tmp`.
- **Each output:**
  1. It is encoded in charged memory, and its MD5 and visible hash (E20) are computed.
  2. Its size is claimed from the spool counter, an atomic try-add against `spool_size`, after a `statvfs` check against `spool_reserve`.
  3. It is written to the attempt's directory, and its buffer is dropped.
- **Records.** Candidate records (key or atlas unit, input, MD5, visible hash, mode, size, file) and skip records are appended to the attempt's `records.ndjson`, through a fixed buffer charged to `M_X`. Nothing about an output stays in memory after its record is written, so no record moves from charged to uncharged memory when the attempt ends.
- **Commit** (`extract.md` E60, all or nothing per input).
  - An attempt that ends `Done` has its directory renamed to `spool/done/<input>/` by the controller.
  - An input that fails commits only its failure record: no candidate, so it competes for no key.
  - `Transient` and `Deferred` attempts' directories are deleted.

### 2.9 Winners and publish

**Winners** (E31 to E34), in phase 8, after the last round.
- The controller streams every committed `records.ndjson` into an index, key → candidates. The index is charged to the Input pool through a meter of its own. No attempt runs, so the pool is empty (2.2's home reads).
- The decision is a pure function of the committed records, so it is the same whatever order the inputs finished in.
- An atlas unit is one record with its member keys (E32, and `extract.md`'s cross-kind rule).
- **Withholding** (E61): a key whose `prefer` input failed in this run is not published (`withheld`). Its rule-2 or rule-3 winner could otherwise overwrite what the next complete run publishes. With `publish_incomplete: false`, nothing is published when any input failed.
- The winners and the collisions are written to spool files, and the index is dropped.
- An index the pool refuses stops the run as a budget refusal (exit 5). That is the same in every run for one record set.

**Publish**, in phase 9.
- Each transfer reserves its buffer from the Input pool before it reads: `heap_cost(size)` for a single-part PUT's body, and the part size (16 MiB) for each part of a multipart upload. At most `download_concurrency` transfers run at once.
- The scheduler waits only for a transfer to end, never on a counter. A reservation refused with no transfer running is an internal error. It cannot happen without a leak: every output file was encoded under an attempt's meter, so it is at most `M <= I`.
- The **directory** sink writes each file as a temp file and renames it, runs `syncfs` before the final manifest, and opens paths beneath the root (E62).
- The **zip** sink streams the archive from the spooled members (E36).
- The **object store** follows `extract.md` E37 and section 3, item 1.
- Each multipart upload id is appended to `spool/uploads.journal` before the first part and removed after its complete or abort (E64).

**The manifest** is streamed from the spool's files, never built whole in memory. Each write is fenced (E38).

### 2.10 Why D holds

1. **An Alone attempt is deterministic.**
   - It runs on one thread.
   - It starts after CR1, on an Environment whose registry is the same in every run of its round (2.3: registration before extraction, in a fixed order).
   - Every dependency open it makes starts cold.
   - Its meter, and every dependency meter it touches, sees only its charges.
2. **A read that succeeds gets the canonical answer.**
   - Its bytes are the same whatever is cached. Eviction frees only what the core can re-read, and the next read reopens it (core 4.1, *eviction frees what it can re-read*). A resource read gives the same answer through either handle (core 4.1, *a resource read goes through `from`'s own handle when the core can tell without reading*).
   - Every CAB resolves by `view(X)` (2.3), whatever else is registered or failed.
3. **A refusal whose outcome could depend on other attempts is retried.** In a Shared attempt it ends the attempt (2.7). The Alone retry runs the read site cold.
4. **A read site that succeeds in a Shared attempt also succeeds in the canonical run.**
   - When it succeeded, each open it touched held at least its cold charges, within the open's `M`. The Dependency pool held at least F plus those, within its limit (A3).
   - So its cold charges fit, and the canonical run's retry, which runs it cold, succeeds too.
   - A job-side refusal falls at the same point in both runs (2.7).

So a Shared attempt that meets no refusal commits the canonical records, and any other attempt hands its input to the canonical run.

**Not covered: nested containers** (core Q10, Q13). A nested container's CABs are registered when it is first reached (core 3.13), so whether a lookup by name finds one can depend on which attempt reached it first. Nested bundles are not expected in real builds (Q10). The engine reports every nested container it meets (`nested_containers`, E40). D is stated for runs without them.

**Tests.**
- `an_inputs_records_do_not_depend_on_what_runs_beside_it`. The engine scene runs:
  - at `threads` 1, 2 and 8;
  - with the inputs in 20 seeded orders;
  - with a refusal injected at the k-th `Input`, and then at the k-th `Dependency`, reservation, for every k up to the scene's total;
  - with pools at 1.0, 1.5 and 4 times the largest peak;
  - with an unrelated dependency that fails to download;
  - with a bundle added that holds a same-named copy of a CAB in another input's dependency.

  Every input's records must equal its canonical run's, timings and counters aside. The canonical run is checked as a run of its own, whose spec holds only that input and the bundles of its view.
- `a_cached_dependencys_work_allowance_used_up_by_many_readers_changes_no_record` (3.11). N inputs each read one large value of one cached dependency, where N is past that open's allowance. Every input's records equal its canonical run's.
- `an_alone_read_refused_by_its_own_cached_reads_is_retried_cold` (3.12). An Alone attempt reads two dependencies whose cached runs together pass the pool. Its records equal a run with a pool twice the size.

### 2.11 Lifetime and the stop path

- **The lifecycle thread** owns the stop decision. Signal handlers (SIGTERM, SIGINT, SIGHUP) only write to a self-pipe. The thread waits on that pipe, on a timer and, when the spec asks, on stdin's EOF (E46).
- **The deadline** is converted once at start to a `CLOCK_BOOTTIME` instant. The engine starts stopping at `deadline - shutdown_grace` (default 20 s), so it has exited by the deadline (E45).
- **Parent death.** `PR_SET_PDEATHSIG` (SIGTERM), then a `getppid()` check against the parent seen at start, since the parent may have died before the call (E46).
- **The stop sequence:**
  1. Set the stop flag. The controller dispatches nothing. Attempts stop at their next object, ending `Transient`, and commit nothing.
  2. Abort every open multipart upload in the journal, in parallel, each with a timeout inside the grace.
  3. Write the final manifest `incomplete`, fenced, with the stop reason. Inputs that never finished are `not_run`. Nothing is published after a stop.
  4. Write the `exit` progress line.
  5. Exit 6 **without joining** workers or transfers.

  When the grace runs out first, the process exits as it stands. The stored manifest then says `running`, with its deadline, which is the truth.
- **A second signal** during the stop exits at once.

### 2.12 What an attempt never does

An attempt never waits:
- on a budget: its pools only `try_reserve` (core 4.1, *budgets only `try_reserve`*);
- on another attempt: its runs are its own, shared cells are computed outside the cell (core 3.16), and a dependency read the pool refuses fails instead of waiting;
- on a core lock held by a waiter: every core lock is held only to look up or to publish (core 4.1, *no caller code runs under a core lock but the budget*);
- on the controller, once it has started;
- on the spool: a claim is an atomic try-add that refuses, and a write is local file I/O no other party holds;
- on a sink, which runs only in phase 9;
- on a download: every download it needs ended before registration.

---

## 3. Open problems, and their answers

Each problem keeps the text that states it. Its answer, from design round 1, says whether it is solved or shown wrong, the mechanism, the core property it rests on (or the request of section 6), and the test that shows it. Engine test names are proposals, in `crates/unityrs-batch/tests/`. Sections 3.11 to 3.13 are problems design round 1 found.

### 3.1 Cached runs never release, and budgets are bound per load, not per job

`claude1-core-plan.md` M7 (P1 round 1).
- Under the old step 1 every input was loaded into the shared Environment, which never evicts caller-loaded files, and each kept its decoded runs and parsed entries for the Environment's life. The old step 5's "dropping returns every charge" never happened while the batch ran, so W1 waited forever once the cumulative decoded size reached the in-flight cap; counting completions instead of charges let memory grow without bound.
- A per-job budget cannot reach a bundle whose budget is the `FileContext` it was opened with.
- A dependency read from job X into input Y decodes Y's blocks on a budget nobody admitted.

**Solved.**
- **Who owns what.** Each attempt owns the only job handle on its input, opened on its own meter, which books with the Input pool through the attempt's home (2.2, 2.6). Dependencies (every input's registered copy included) are owned by the Environment's LRU, on Dependency meters. A read site's pins last only for the read site. Section 5.2 lists every holder with its pool and its release point.
- **Where every counted charge drops.** The controller counts attempts, and the Input pool is the sum of the attempts' homes. Every charge of an attempt is held by a local of its function, so all of them drop when it returns. The worker reads the home's `used` as zero before the controller counts the attempt finished (2.2).
- **The second bullet is gone.** The per-input bound is `M_X`'s own limit, and `M_X` is the attempt's.
- **The third is gone too.** A read into Y from X is a read through Y's registered copy, on the Dependency pool, whose budget it is.
- **Rests on** core 4.1:
  - *one meter per input, charged before every input-sized allocation*;
  - *a second handle on the same bytes is independent*;
  - *`Environment::load_dependency(source)`*;
  - *eviction frees what it can re-read*;
  - core 3.16 R2: a registration charges the slot's dependency.

**Tests.**
- `every_attempt_returns_its_charges`: after every attempt of every engine scene, `home.used() == 0` and `Arc::strong_count(M_X) == 1`.
- `the_input_pool_is_empty_whenever_nothing_is_in_flight`.
- `an_attempts_end_message_holds_no_core_type`: a source scan of the message's field types against a deny list (`Bytes`, `File`, `ObjectRef`, `Parsed`, `HeldCab`, `Charge`).
- `a_dependency_read_is_charged_to_the_dependency_pool`.

### 3.2 The prepaid estimate is too small, and the alone path can hang

`claude2-core-plan.md` M1 (P1 round 2).
- The estimate must cover what the core's rules can book: the declared blocks, blocks-info, a block decoded once per span that overlaps it, entries, rows and values. Round 2 rewrote the old step 2's estimate, but it is still an engine guess against charges the core makes at actual sizes (an LZMA block ignores its declared size).
- "Alone" at a cap larger than the in-flight bound waits forever (the alone path's cap was round 2's fix; 3.4 shows the first admission had the same hang).

**Shown wrong, as a useful estimate; solved without one.**
- **No estimate the core's rules provably cannot exceed admits more than one job at a time.**
  - An LZMA block's output ignores its declared size (core 3.6, **Codecs**), so a decoded size is not known before the decode.
  - Values are not bounded per input byte at any useful ratio. An element of a class with no fields consumes no input, and up to `max_zero_size_elements` (2^20) of them are allowed per object read (`limits/mod.rs`), each a value on the heap.
  - The only provable bound is `max_input_memory` itself.
- **Admission is by count** (2.4), so there is nothing to under-estimate. Every charge is made at its actual size, and the meter or a pool refuses past its limit.
- **Alone cannot wait forever** (2.5). It waits only for the attempts in flight, which never wait and whose work is bounded. With `M <= I` the Input pool cannot refuse it.
- **Rests on** core 4.1:
  - *budgets only `try_reserve`*;
  - *a refused charge allocates nothing, and leaves the input usable or cleanly failed*;
  - *transient refusals are never cached*;
  - *bounded work per open input*.

**Tests.**
- `a_refusal_at_any_reservation_reruns_the_attempt_alone_with_the_same_records`: 2.10's injection.
- `an_alone_attempt_runs_with_nothing_beside_it`: no Shared dispatch between an Alone dispatch and its end.
- `the_controller_always_progresses`: a deterministic model of the controller with fake attempts of random peaks, durations and outcomes. It checks that no state has an attempt queued with nothing running, and that each input runs at most one Shared and one Alone attempt per round.
- `a_hostile_lzma_block_in_a_shared_run`: one block declared at 1 byte that decodes to 64 MiB, among 50 small inputs. The Input pool never passes `I`. The hostile input fails on its own meter, with no Alone run (a job-side `Limit`). Every other input's records equal its canonical run's.

### 3.3 No eviction mechanism, resource reads through the shared handle, untagged refusals

`claude2-core-plan.md` M2 (P1 round 2).
- Registry handles were never released, so the dependency pool filled and every later cross-bundle read failed.
- A job's own `.resS` was served by the shared registry handle, moving texture bytes onto the dependency pool.
- Both pools refused with the same error, so the engine re-ran a job alone for a pool problem.

**The core now provides:** registry slots that hold no strong dependency handle; eviction that frees runs, parsed entries and re-readable source bytes; resource reads located without parsing and read through `from`'s own handle when the core can tell without reading that the CAB is an entry of `from`'s input; refusals tagged `Input` or `Dependency`.

**Solved.**
- **Eviction.**
  - The core's LRU, bounded by `dep_cache_bytes`, orders what goes first (`env/registry/lru/mod.rs`).
  - A pool refusal inside an Environment call evicts the other dependencies and retries once (core 3.13).
  - The engine evicts everything with CR1 before each Alone attempt and in the Alone retry (2.7). It evicts nothing during Shared work.
- **Sizing.**
  - The Dependency pool is `dep_cache_bytes + F`, with F measured (2.2).
  - Validation requires `M <= dep_cache_bytes`, and section 5.6 recommends `2 M` and more.
  - A value too small costs reopens and Alone runs, never a different record (D). The manifest reports evictions, reopens and Alone runs, so undersizing shows.
- **A `Dependency` refusal** is dependency-side (2.7). In a Shared attempt it sends the input Alone. In an Alone attempt it is retried cold once, and a second refusal is an object-level skip naming the pool.
- **The job's own `.resS`** never goes through the registry: the engine's resource rule reads it through the job's handle (2.6).
- **Rests on** core 4.1 *eviction frees what it can re-read* and *transient refusals are never cached* (a refusal under contention does not poison the shared dependency for the Alone run), and on CR1.

**Tests.**
- 2.10's injection, its `Dependency` half.
- `a_dependency_cache_smaller_than_the_working_set_costs_reopens_not_records`: the same records at `dep_cache_bytes` = M and = 8 M.
- `sprites_across_ten_dependencies_with_a_small_pool`: 50 inputs whose sprites use 10 dependency sheets round robin, with the pool at twice the largest dependency. Every record equals an unbounded run's.
- `a_read_site_larger_than_the_pool_skips_exactly_its_objects`.
- `the_dependency_pool_holds_f_after_a_full_eviction`.

### 3.4 The first admission can wait forever, and the estimate is inflated

`claude3-core-plan.md` M1 and `claude3-core-code.md` M5 (P1 round 3; the same defect, reported twice).
- The old step 2 capped the estimate at `max_input_memory` only. With `max_inflight_uncompressed_bytes` below that (4 GiB on a 16 GB host), a bundle whose estimate is above the in-flight bound never fits, even with nothing in flight, and the FIFO controller hangs every bundle behind it.
- The estimate summed the blocks each entry's span overlaps over entries, so N entries in one block count it N times, though the core serves every later entry from the one cached run (core 3.6). Ordinary single-block bundles hit the cap.

**Solved: both come from the estimate, and there is none.**
- The constraint that replaces 3.4's cap is validation's `M <= I` (2.2).
- A bundle whose needs exceed what its meter allows fails on its own meter, at once (a job-side `Limit`, 2.7), and is never queued again.
- Both waits, the Alone drain and the end-of-round drain, are W2 and W3 in section 4.

**Tests.**
- `a_spec_whose_input_cap_exceeds_the_input_pool_is_refused`.
- `an_input_above_its_meter_fails_once_and_the_run_goes_on`.

### 3.5 What a consumer holds is charged to a job that may never wait

`claude3-core-plan.md` M6 (P1 round 3), and the W2 part of `claude3-core-code.md` M5.
- A slow sink's backlog of `Parsed` values and slices was charged to the job's fixed budget, so the job was refused, re-run alone with the same sink, and failed: the outcome depended on sink speed, and "a stalled sink slows the run" was false.
- A sink that holds output across jobs (DESIGN's `prefer` winners, per-atlas collision resolution) held charged bytes that the old W1 or W2 waited on, while it waited for a job the controller would not admit: a cycle.

P1b needs a consumer that holds output across jobs. The `prefer` winners keep every candidate for a key until each input that can produce it has run, and an atlas, its crops and its JSON win as one unit.

**Solved: copy-out, with winners decided after the last attempt** (2.8, 2.9).
- **Nothing in memory crosses a job.** A job hands no core type to anyone. It writes each output to the spool and its records to its spool directory.
- **Pools exclude what consumers hold.** The Input pool's in-flight bytes are exactly the attempts' homes. Consumers hold files on disk, outside every pool.
- **Winners are decided once, after the last round.** A key's winner can depend on any input that produces it, which is known only once every input has run. So E31 to E34 decide by a pure function of the committed records, and units are allowed as they are. The candidates wait on disk, which the controller never waits on.
- **Sinks run in phase 9.** A slow or stalled sink lengthens the run and changes no record.
- **Cost.**
  - The spool holds every candidate until phase 9, beside the downloads (section 5.7).
  - Uploads start only after decoding, at about one more pass over the output at network speed.
  - Publishing a key once nothing can beat it (its `prefer` input committed) changes no rule. It is left for later.
- **Rests on** nothing in the core: the core never sees a sink.

**Tests.**
- `winners_are_the_same_whatever_the_completion_order`: every permutation for small scenes, 50 seeded orders otherwise.
- `a_stalled_sink_never_stalls_an_attempt`: an object-store test server holds every request open. The rounds still end, and the run ends `incomplete` at its deadline.
- `an_atlas_wins_as_one_unit`.
- `a_failed_input_commits_no_candidate`.
- `a_key_whose_preferred_input_failed_is_withheld`.

### 3.6 The old step 1 named a load the API did not have

`claude3-core-plan.md` L7 (P1 round 3).
- The only way to open a source into the Environment was `load`, which gives an `Input` meter and is never evicted, so a pool refusal was tagged `Input` and no registry handle could be evicted.

**The core now provides:** `Environment::load_dependency(source)` (core 3.13): a `Dependency` meter, CABs registered lazily, evictable, not in `files`.

**Solved** (2.3).
- **Registration** is `load_dependency(Source::Path)` for every input and declared dependency, one at a time, in bytewise name order, before the first job, with no `CabResolver`. The core never opens a path it was not given, so no lookup inside a job can start a download or a registration.
- **CAB names** come from each registered bundle's own directory, read by the load.
- **Bundle names** come from the spec's graph. A name found only in an AssetBundle's `m_Dependencies` is fetched by the URL rule in a discovery round between extraction rounds, and only for the inputs whose view holds it.
- **Rests on:**
  - core 4.1, *`Environment::load_dependency(source)`* and *opening a bundle reads only its header, blocks-info and directory*;
  - core 3.16 item 6 (3.7's last row): every registration has returned before any job that could read it starts.

**Tests.**
- `registration_decodes_no_data_block`: through the engine, a bundle whose every data block is corrupt registers.
- `registry_answers_do_not_depend_on_download_order`: two inputs holding a CAB of one name, with downloads finishing in each order, 50 runs; the same answers and the same `same_named_cabs` record.
- `no_attempt_starts_before_registration_ends`.
- `a_deferred_input_matches_a_run_that_declared_its_dependency`.

### 3.7 Problems settled in the core that the batch design must restate

These were found against the batch flow but were defects of the core API, fixed in P1. The batch design relies on the property and must not undo it. The third column says how section 2 uses it.

| finding | what was wrong | the core property now (core plan 4.1), and its use here |
|---|---|---|
| `codex1-core.md` H3 (round 1) | a codec allocated its output before the budget saw it, so jobs could exceed the batch budget before a refusal | one meter per input, charged before every input-sized allocation; a refused charge allocates nothing. **Here:** the engine's own buffers follow the same rule (E59), and output still growing inside an admitted job needs no allowance, because the Input pool refuses it |
| `codex2-core.md` H2 (round 2) | a retained `Parsed` carried no charge, so values outlived the books | every `Parsed` carries the charge for its value. **Here:** values never leave an attempt (copy-out). A dependency value a read site reads is charged to that dependency and dropped at the read site's end |
| `claude3-core-plan.md` M2 (round 3) | a cell published a pool refusal for good, so one job's refusal poisoned a shared dependency, and "evict and retry once" retried a cached error | transient refusals are never cached. **Here:** the Alone run and its retry rely on it. `DecodeWork` is not transient, and an open that used its allowance up keeps refusing; 3.11 and 2.7 handle that |
| `claude3-core-plan.md` M3 (round 3) | resolving a resource's CAB parsed the shared slot, decoding the whole `.resS` on the pool before the redirect to the job's handle | a resource read goes through `from`'s own handle, resolution included, when the core can tell without reading that the slot is `from`'s input. **Here: not used.** The engine's resource rule reads a job's own entries through its handle before any registry lookup (2.6; 3.8) |
| `claude3-core-plan.md` M5 (round 3) | eviction kept a `Path` dependency's bytes and their charge, so the pool filled with bytes eviction could not free | eviction frees what it can re-read. **Here:** every registration is a `Path`, so eviction frees its bytes |
| `claude3-core-plan.md` L6 (round 3) | a dereference from a file whose bundle a job had dropped was unspecified | `DerefError::ContainerDropped`; a caller that hands out files keeps a strong bundle handle beside each. **Here:** a read site takes the target's top-level `File` before its next Environment call and holds it while it reads (2.6) |
| `claude3-core-code.md` M4 (round 3) | under `Lazy`, decode work was unbounded: a failed block and an `entry_range` decode were redone for every read | block outcomes kept, `entry_range` runs cached, and `max_decode_work` per open input; a refused growth counts no work, and the Environment restarts a dependency's count when it reopens it after eviction (`claude4-core-code.md` M2, round 4). **Here:** the Alone path's termination and the cold retry's fresh allowance rely on it |
| implementation review round 6, the Environment's MEDIUM-3 | the core said concurrent calls act as if made one after another, but a load and a reach publish their keys one at a time, so another thread could find one key of a load and miss the next | each key's registration, each `files` entry and each `path_dict` entry is published whole; a call that reaches a container returns only after its whole subtree is registered, and a load only after all its keys and its `files` entry are (core 3.16, item 6). **Here:** every registration happens while no job runs (2.3). A nested container's reach is the one registration during a round, and D excludes it (2.10) |

The engine also relies on the core's error classes (`CellError::is_transient`, public in `once.rs`; `ChargeError::is_transient`, `error.rs`) only through 2.7's table, which splits them by where the error arose.

### 3.8 The resource redirect and eviction cannot both hold for a job's second handle

`claude4-core-plan.md` M3 (P1 round 4).
- The old step 3's job opened its own handle on its input's bytes. The core redirects a resource read through that handle only when it can tell without reading that the CAB slot is the same input: the slot is the job's handle itself, or a `Source::Bytes` slot whose `Bytes` the job's handle shares (core 3.13). An evicted `Source::Path` or `Source::Fetch` slot holds only a path or a fetcher, so the core resolves it by UnityPy's rule, reopening it on the dependency pool, which is the charge the redirect was for.
- Eviction frees a dependency's bytes only for `Path` and `Fetch` (3.3; `claude3-core-plan.md` M5). So registering the inputs as `Bytes` keeps the redirect and the bytes for the run's life, and registering them as `Path` frees the bytes on eviction and loses the redirect for every evicted input.

**Solved, by needing neither.**
- **The redirect is not needed.** It exists so that a job's own `.resS`, found through the shared registry, is read through the job's handle. Here a job never asks the registry for its own resources. The engine's resource rule (2.6) reads the input's own entries through the job's handle first, and that rule is the own-bundle-first rule itself (3.9).
- **So every registration is `Source::Path`**, and eviction frees its bytes (core 4.1, *eviction frees what it can re-read*).
- **Rejected.**
  - Reading through the handle `load_dependency` returns would put every job's runs on the Dependency pool, and the Input pool would no longer bound jobs.
  - `Source::Bytes` over the engine's own buffers keeps every input's bytes for the run's life. Over `mmap`s, it leaves those bytes under no budget and faults when a mapped file is truncated, which the core declined for `Source::Path` (`env/mod.rs`, `Source::Path`'s docs).
  - An input identity in the core is not needed.
- **Cost.**
  - An input that another input reads is opened twice: by its job on the Input pool, and as a dependency on the Dependency pool.
  - The entries other inputs reach in it are decoded on the dependency side too, once per residency in the cache. For the corpus's single-block bundles that is one more decode of the whole block per residency in which another input reaches it.
  - Neither cost has been measured (section 7, A5).

**Test.** `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job`: its own resource reads charge nothing to the Dependency pool, before and after a full eviction.

### 3.9 Same-named CABs: the own-bundle-first rule

`docs/plans/core.md` section 9, Q12 (decided 2026-10-30) gives this plan the rule.
- Under `EnvOptions::batch()`, an input's `.resS` reads and its container read its own bundle's CAB first when two bundles hold a CAB of one name (Tier C, `PARITY.md` section 19.2's same-named CABs row). `EnvOptions::unitypy()` keeps UnityPy's one registry, Tier A, in both load modes.
- P1 implements UnityPy's registry under every option set; a `PPtr` from inside a bundle already reads its own bundle's copy first (Tier A).

**Solved; the rule lives in the engine until CR3 moves it into the core.**
- **Resources:** the engine's resource rule (2.6) looks through the reading file's own container chain before the registry.
  - The order is UnityPy's candidate order, each candidate checked in the own chain first. The registry is asked only when no candidate is an own entry. This differs from interleaving per candidate only when the own bundle holds a later candidate and another bundle an earlier one, which is this rule's own Tier C case.
  - A `.resS` in a nested bundle of the input is found in that bundle's directory, through the chain.
- **The container:** X's own `SerializedFile::container()`. Its pointers read their siblings first (Tier A).
- **Pointers across bundles:** the view's rule 3 (2.3). Own bundle first, then the greatest name in bytewise order, among the view's bundles only.
- **`EnvOptions::unitypy()`** is untouched, because the core does not change.
- **Rests on** Q12's assignment of the rule to this plan, and on CR3 once granted. The engine's copy of the core's candidate and slice rules is guarded by the differential test of 2.6 until then.

**Tests.**
- `own_bundle_first_for_resources_and_container`. Two inputs each hold `cab-x` and `cab-x.resS` with different pixels, and each lists the other as a dependency. Each input's textures and container rows are its own. The test runs under `EnvOptions::batch()` and under an Eager variant of it. P1's `EnvOptions::unitypy()` suite runs unchanged.
- `own_first_resources_equal_environment_resource_without_same_named_cabs` (2.6).

### 3.10 A failed dependency fails the inputs that need it

- A dependency that fails to download or parse fails every input that needs it, and no other; the run is then `incomplete` (DESIGN.md section 4's failure model).

**Solved** (2.3, the views).
- **"Needs"** means "is in the view". The view is X, the closure of its declared list through the spec's graph, and, with a URL rule, the names in its `m_Dependencies`.
- **When.** A bundle fails if its download fails after its retries (E10), its source is refused, or `load_dependency` fails. Every input whose view holds it fails before its first attempt, or at discovery for a name found there, with `dependency_failed(name, reason)`.
- **Inputs whose view does not hold it cannot observe it.**
  - Their reads resolve within their views (rule 2).
  - Their answers do not depend on which bundles registered (rules 3 and 4).
  - So their records are the same as in a run where nothing failed.
- **A read that fails for a dependency's own bytes** (a corrupt block or entry, found at read time) is not a failed dependency. It is an object-level outcome of the input whose read reached it (2.7). The core keeps block outcomes, so every input that reads that block meets the same error.
- **Rests on** the key table built from registered directories, `HeldCab::hold` and the target's `Parent` (2.6), and *transient refusals are never cached*.

**Tests.**
- `a_failed_dependency_fails_exactly_the_inputs_whose_view_holds_it`. A declares D and reads it, B declares nothing, C declares D but never reads into it, and W declares U, which holds a CAB named like one of D's. D fails, in turn, by a 404, a truncated body, a corrupt header and a corrupt data block.
  - For the first three, A and C fail with the reason, B's and W's records equal a run in which D succeeds, and the run exits 4.
  - For the corrupt data block, D registers and fails no input. A skips the objects whose reads reach the block, C is unaffected, and B's and W's records again equal a run in which D is whole.
- `a_shadowing_bundle_outside_the_view_changes_no_record`. The same scene with U failing instead: A's records equal a run in which U succeeds.

### 3.11 A cached dependency's work allowance is shared by every input that reads it

Found in design round 1.
- A value read counts its work on the meter of the open it reads (`walk/work.rs`; core 4.1, *bounded work per open input*). A cached dependency read by many inputs therefore builds up one shared count, and only a reopen resets it (`Meter::reopen`).
- `DecodeWork` is not transient (`error.rs`), and once an open's allowance is used up its cells keep the refusal (core 4.1). So the refusal lands on whichever input crosses the line, and every later reader of that open meets it until something evicts the open.
- At `max_decode_work = 4 M`, a popular dependency kept in the cache reaches its allowance in an ordinary run.

**Solved** (2.7).
- A `Limit` on a dependency's meter is dependency-side. In a Shared attempt it ends the attempt, and the input runs Alone after CR1, which also drops the poisoned open.
- In the Alone attempt, a read site refused by its own cached reads is retried cold, with a fresh allowance.
- CR6 would stop a used-up allowance from poisoning later readers at all, and save those Alone runs.

**Test:** `a_cached_dependencys_work_allowance_used_up_by_many_readers_changes_no_record` (2.10).

### 3.12 A direct read of a dependency object gets no evict-and-retry

Found in design round 1.
- A value read on a dependency's `ObjectRef` (a sprite's texture, say) is no Environment call. Its refusal is the caller's, and nothing is evicted (core 3.13).
- The LRU's sum of its meters can lag a use behind, so the pool's refusal, not the LRU, is the real bound (`env/registry/lru/mod.rs`, the LRU's docs).
- In an Alone attempt that has loaded several dependencies, such a read can be refused by a pool full of the attempt's own evictable entries, and recorded as the outcome though it would fit.

**Solved** (2.7). The Alone retry evicts everything with CR1 and redoes the read site cold, so an Alone outcome never records a refusal that a cold read site would fit. CR2 would give Shared attempts the core's own retry; correctness does not need it.

**Test:** `an_alone_read_refused_by_its_own_cached_reads_is_retried_cold` (2.10).

### 3.13 A same-named CAB in a bundle an input does not need can change its answers

Found in design round 1.
- Ranks give a CAB key to the registration of greatest rank (core 3.13, **Ranks**). Suppose X declares A, and B, declared only by another input, holds a CAB with the same name as one of A's. Then X's read of that CAB resolves to B.
- If B fails to download, it is never registered, and the same read resolves to A. X's records then depend on a bundle X does not need, against 3.10's test.
- Refusing the later registration (a name-conflict rule) only moves the dependence to B's dependents, whose records then depend on A.

**Solved** (2.3, the views).
- X's reads resolve only within `view(X)`, by rule 3: own bundle first, then the greatest name in bytewise order.
- A flagged input reads through a private Environment that holds its view alone. So no bundle outside a view, and no failure of one, changes an answer inside it.

**Tests.**
- `a_shadowing_bundle_outside_the_view_changes_no_record` (3.10).
- `a_flagged_input_gives_the_records_of_a_run_with_only_its_view_registered`.

---

## 4. Why it cannot deadlock

Every wait in the process:

| # | who waits | for what | why it ends, and why the awaited party never waits on the waiter |
|---|---|---|---|
| W1 | the controller | one event: an attempt's end message, a 1 s tick, a stop | attempts only send, on a channel of capacity `threads`, and never read the controller's state; the tick and the lifecycle thread hold nothing |
| W2 | the controller, before an Alone attempt | `in_flight == 0`, through W1 | every attempt returns: it never waits (2.12), and its work is bounded (core 4.1, *bounded work per open input*) |
| W3 | the controller, at a round's end | `in_flight == 0` with both queues empty, through W1 | as W2 |
| W4 | an idle worker | the next attempt on the dispatch channel | it holds nothing, and the controller never waits on an idle worker: it dispatches before it waits whenever a worker is free and an attempt is queued |
| W5 | the controller, in phases 4 and 7 | the downloads to end | each ends by success or by failure within its retries and timeouts (E7), and none starts a retry past `deadline - shutdown_grace` |
| W6 | a transfer | a `download_concurrency` permit | permits are held only by transfers inside one request, which end by W7 |
| W7 | a transfer | the network | timeouts and attempt counts (E7, E37) |
| W8 | a core caller (an attempt, or the controller registering) | a core lock | held only to look up or publish, never across a decode, user code or another lock; the only caller code under one is the budget, which is atomics (core 4.1, *no caller code runs under a core lock but the budget*; *the budget contract*) |
| W9 | an attempt | its spool file's I/O | local; no other party holds the file; `ENOSPC` is a refusal (2.7) |
| W10 | the publish scheduler | a transfer to end | as W7. Its Input-pool reservations are `try_reserve`, never a wait (2.9) |
| W11 | the final manifest's write | every transfer to have ended or been aborted | transfers never wait on the manifest |
| W12 | the progress thread | stdout | producers overwrite a one-slot mailbox; nothing waits on the progress thread |
| W13 | the lifecycle thread | the self-pipe, its timer, stdin | nothing waits on it: it sets the stop flag and wakes the controller |
| W14 | the stop path | the multipart aborts and the manifest write, each with a timeout inside the grace | it never joins a worker or a transfer (2.11) |

**There is no cycle.** Rank the parties:
- 0: the kernel, the network and stdout, external and bounded by timeouts;
- 1: core locks, and the engine's budgets, which are atomics and take no lock;
- 2: a running attempt, its private Environment included;
- 3: transfers;
- 4: the controller.

Every wait-for edge goes to a lower rank or to an external party with a timeout:
- an attempt (2) waits only on 1 and 0;
- a transfer (3) waits on 0, and on permits held by transfers that wait only on 0;
- the controller (4) waits on 2, 3 and 0.

The one edge that points up, W4, comes from an idle worker, which holds nothing and is not one of the running attempts the controller waits for. The progress and lifecycle threads only wait on others; no one waits on them. The core blocks nowhere (core 4.1, *the core never waits and never starts a thread*), so no core call adds an edge. `block_on` is called only by the controller, never by an attempt or a tokio worker, and async tasks never call the core.

**Tests.** Every engine test runs under `unityrs_testkit::guard`, which fails a hang instead of waiting forever. The scenes include:
- a stalled sink, a stalled download server, and a stdout that is never read;
- attempts paused at test hooks inside core calls while other attempts and an Alone drain proceed;
- `a_wedged_attempt_cannot_pass_the_deadline`: a test decoder spins, and the process exits 6 by the deadline.

`the_engine_starts_no_unbounded_wait` is a source scan in the core's style (`unityrs_testkit::waits`). It finds every `recv`, `lock`, `join`, `wait`, `park` and `block_on` in `unityrs-batch` and checks each against this table's list, by module.

---

## 5. The memory argument

### 5.1 The bound

In the charge unit (`heap_cost`, glibc's chunk, core 3.15), at every instant:

```
heap <= I + dep_cache_bytes + F          the Input and Dependency pools' limits (2.2)
      + C_prov + C_thr + C_open + C_net + C_spec + C_base
```

**Proof.**
1. Every allocation that grows with an input or an output is charged before it is made. The core charges its own (core 4.1, *one meter per input, charged before every input-sized allocation*). The engine charges its buffers, the attempt's view, the record buffer (2.6, E59), the key table (2.3), the winners' index (2.9) and the transfers' buffers (2.9) under the same rule.
2. Each charge books with exactly one pool:
   - an attempt's meter, through its home, with the Input pool;
   - every dependency meter with the Dependency pool, since the Environment's context budget is the Dependency pool's (`env/load.rs`, `try_load_dep`);
   - phases 8 and 9 reserve from the Input pool explicitly.
3. Each pool refuses any reservation that would take it past its limit, in one compare-and-swap.
4. Everything else the process allocates is one of the constant terms of section 5.3.

Records live on disk (2.8), not in memory, so no term grows with the number of outputs or skips.

### 5.2 Every holder of charged bytes

| holder | pool | taken | released |
|---|---|---|---|
| the attempt's copy of its input's bytes (`read_path` on `M_X`) | Input | the attempt's start | the attempt's end |
| the job handle's runs, parsed entries, compiled trees, kept errors (core 3.6, 3.15) | Input | as read | the attempt's end |
| the values of X's objects | Input | per object | after that object's outputs |
| a block decode's transients: `DecodeBuf`'s old and new room while it grows, LZMA's dictionary and state | Input | during the decode | at the decode's end |
| export buffers, one sheet group at a time | Input | per sheet | when the sheet's last output is spooled |
| the view, the record buffer, the private Environment's lookup keys | Input | the attempt | the attempt's end |
| the keys of a lookup (core 3.13, **Names**) | Input | during the call | at its return |
| a dependency's open: a `Path` source's bytes, its runs and entries | Dependency | load, or reopen after eviction | eviction, bytes included |
| registry slots and keys, and the engine's key table | Dependency | registration | never during the run: F |
| a read site's pins: a `HeldCab`, an `ObjectRef` with its top-level `File`, a dependency value, a resource's `Bytes` | Dependency | in the read site | at the read site's end |
| a private Environment's opens and slots | Dependency | the attempt | the attempt's end |
| the winners' index | Input | phase 8 | before phase 9 |
| a transfer's buffer | Input | the transfer's start | its end |
| outputs, records, downloads, archives | disk (spool) | their write | the run's end |

### 5.3 The constant terms

| term | value | source |
|---|---|---|
| `C_prov` | at most 64 MiB, once, plus a TPK's trees when a TPK is given (A8) | `ProviderChain::DEFAULT_CACHE_BYTES` (`provider.rs`). Core 3.15 states it "per `FileContext`", but the cache lives in the `Arc<ProviderChain>`, and every context the engine makes is a clone of one base context sharing that `Arc` (CR7) |
| `C_thr` | `(threads + 1)` x about 1.86 MiB | per thread that reads input (the workers and the controller, which registers): `Limits::stack_needed` (release: 512 levels x 3 KiB + 256 KiB = 1.75 MiB, `limits/mod.rs`), detection scratch of at most 64 KiB, the inflater's state and 32 KiB window, the 1 KiB tally (core 3.15) |
| `C_open` | `(N_reg + threads + P_open) x 192 B`, plus 64 B per old open a stale handle keeps | per live open, `Meter::OWN_HEAP` and the `Bytes` headers the core may clone; `REOPEN_HEAP` per old open (`limits/meter.rs`); `N_reg` registered bundles, `P_open` private opens |
| `C_net` | `download_concurrency` x `c_net` | connection, TLS and streaming state per transfer, `c_net` <= 2 MiB (A1); payloads are in the Input pool |
| `C_spec` | at most 4 x the spec's size, plus 256 B per input | the parsed spec and the controller's per-input state (A1). A spec over 16 MiB is refused (E1) |
| `C_base` | measured | the binary, the I/O runtime, the registry's fixed parts (one per Environment, core 3.15), the progress mailbox |

### 5.4 Outside the charge unit

- **glibc's slack.** A live allocation whose chunk is below 128 KiB may hold up to 16 bytes past its charge (`CHUNK_SLACK`), at most half again of those allocations' charges (core 4.1, *one meter per input*; `limits/mod.rs`). So in glibc's own bytes the worst case is `1.5 x (I + dep_cache_bytes + F)` plus the constants. The pools are dominated by large mapped buffers (runs, pixels, PNG output), whose pages cover the slack, so the excess is a few percent in practice. The nightly reports it.
- **Allocator retention** (RSS above the live heap). `heap_cost` assumes a chunk of 128 KiB or more is mapped, but glibc raises its mmap threshold each time a mapped chunk is freed. After that, large buffers come from arenas that do not return memory to the system (`limits/mod.rs`; `tests/heap_rounding.rs` pins the threshold for that reason).
  - `extract.md` E57: the binary calls `mallopt(M_MMAP_THRESHOLD, 128 KiB)` at start. That fixes the threshold, so every buffer of 128 KiB or more is mapped and unmapped on free, which is the `heap_cost` model.
  - It also sets `M_ARENA_MAX` to `threads + 2`.
  - Fragmentation of small chunks inside arenas is not bounded by the core's rules (A7). The nightly measures peak RSS against the bound.
- **Page cache.** Spool writes and the downloads fill the kernel's page cache. It is reclaimable and outside the heap, but a memory cgroup counts it (A7). The spool writer drops each file's pages after closing it (`posix_fadvise(DONTNEED)`).

### 5.5 Arithmetic on the corpus

**A block decode's transient** (`limits/vec.rs`, `budget/mod.rs`, `codec/lzma.rs`).
- `DecodeBuf` doubles its capacity, and while it grows it charges the new room beside the old. `into_bytes` charges the exact size beside the old capacity before it moves the bytes.
- For an output of `B` bytes with final capacity `c_n < 2B`:
  - during the last doubling the charge peaks at `c_(n-1) + c_n < 3B`;
  - at `into_bytes` it is `B + c_n < 3B`.
- LZMA adds its dictionary, `min(clamp(declared, 4 KiB, 8 MiB), named)` plus 576 B, and 33,040 B of state.

For the corpus's largest block, `B` = 69.4 MB = 66.2 MiB:
- `c_(n-1)` = 64 MiB and `c_n` = 128 MiB, so the peak is 192 MiB + 8 MiB + 32 KiB = 200.03 MiB = 209.8 MB.
- That is within 1.1 % of that input's measured peak of 212 MB, so the peak is almost all decode transient (A3).
- The fix in progress, which sizes the room to the declared size, brings it to about `B + 8 MiB`. That is consistent with the measured 145 MB once values are added.

**One attempt's peak** is

```
P_X = s_X + runs_X + transient_X + meta_X + values_X + export_X
```

- `s_X` is X's file size.
- `runs_X` is at most the declared decoded size, plus one more copy of each block a span straddles (core 3.6).
- `transient_X < 3 B_X + 8 MiB + 33 KiB` before the fix.
- `values_X` covers only the objects being exported, one at a time.
- `export_X` covers one sheet group of `L` x `H` pixels: RGBA and alpha sheets of `4 LH` each, the image being encoded (at most `4 LH`), PNG output of at most `4 LH + H` plus a deflate block's overhead, filter rows of `5 (4 L + 1)`, and about 262 KiB of deflate state. So `export_X <= 16 LH + 0.3 MiB` for sprites and atlases, and `8 LH + 0.3 MiB` for a plain texture. A 2048 x 2048 sheet takes 64 MiB; a 4096 x 4096 one, 256 MiB.

### 5.6 Sizing rules, and an example

Each rule is a recommendation the manifest reports against, not a validation.

| goal | rule |
|---|---|
| every input that fits at all completes | `M >= max P_X` (an attempt's peak, E40) |
| few Alone runs for the Input pool | `I >= threads x p95(P_X) + max(P_X)` |
| every read site fits after a full eviction | `dep_cache_bytes >= 2 M` (validation requires `>= M`, 2.2); a flagged input's private registrations come out of this room too |
| few Alone runs for the Dependency pool | `dep_cache_bytes >= threads x p95(read site's pins) + the working set worth keeping` |
| the spool fits | `spool_size >= downloads + all candidates + records + threads x the largest attempt's outputs + the largest archive` |

**Example for the corpus:** `threads` 8, `download_concurrency` 8, `I` = 1 GiB, `M` = 512 MiB, `dep_cache_bytes` = 512 MiB, F about 1 MiB.
- The constants come to about 121 MiB: 64 MiB of provider cache, 16.7 MiB for 9 threads, 0.2 MiB of opens, 16 MiB for 8 transfers, 4.3 MiB of spec and controller state, and about 20 MiB of base.
- So the bound is about 1,658 MiB in the charge unit, and about 2.4 GiB under glibc's worst-case slack.
- The corpus's largest input peaks at 212 MB before the decode fix, well inside `M`. No corpus input should fail for memory at these budgets. How many Alone runs they cause is not yet known (A5).

### 5.7 Disk

`spool_size` bounds the sum of:
- downloaded inputs and dependencies, kept until the run ends;
- every committed candidate and its records, kept until phase 9;
- the partial outputs of the attempts in flight;
- archives being built;
- the manifest.

A free-space check against `spool_reserve` runs before each claim. A refused claim stops the run (2.7).

### 5.8 How it is checked

- `every_attempt_returns_its_charges` and `the_input_pool_is_empty_whenever_nothing_is_in_flight` (3.1).
- X12 in `extract.md`'s gate (its section 5): the core's counting allocator (`unityrs_testkit`) over the engine's export paths. The heap never holds more than the charges plus section 5.3's constants, as `tests/memory_model.rs` checks for the core.
- X13: each testkit input run with `M` at its peak less one byte, and at its peak. It is refused the same way every time, then gives the records of an unbounded run.
- `rss_tracks_live_heap_after_large_frees`, with and without E57's settings.
- The nightly writes `VmHWM` (`/proc/self/status`) into the manifest and checks it against 5.1's bound for its spec.

---

## 6. Requests to the core

Each request goes through the core's integration rule. Each names the change, the property it adds to core 4.1, and what the engine does without it. No core 4.1 property changes, and `EnvOptions::unitypy()` is untouched by all of them.

| request | change | property it adds | without it |
|---|---|---|---|
| **CR1, needed** | `Environment::evict_dependencies(&self)`: drop the LRU's handle on every dependency. It is the existing `DepCache::evict_others(None)` (`env/registry/lru/mod.rs`, `pub(crate)` today), made public | after it, the Dependency pool holds only registry charges and what callers pin, and the next read of each dependency reopens it cold, with a fresh work allowance. Test: the pool reads F after it when nothing is pinned, and a read after it reopens (`Meter::reopen`) | the engine drops and rebuilds the Environment, re-registering every bundle, before each Alone attempt and each Alone retry, at the cost of reading every bundle's header again |
| **CR2, wanted** | `Environment::with_room` public: the private `room` (`env/mod.rs`), with `PoolRefusal` (`env/registry/dependency.rs`) made public | a caller's read on a dependency's handle, made inside it, gets the core's evict-and-retry-once, as an Environment call does (core 3.13) | Shared attempts refused this way run Alone. The Alone retry (2.7) keeps every outcome right |
| **CR3, wanted** | `EnvOptions::own_bundle_first`, true in `batch()` and false in `unitypy()`: a resource read's candidates and the AssetBundle container are looked up first in `from`'s own container chain, by the registry's key rule | Q12's rule in the core, where its candidate order and slice rule already live. Needs a `PARITY.md` section 19.3 row and Q12's tests in both load modes | the engine's copy (2.6) and its differential test |
| **CR4, optional** | `Error::refused_by(&self)`, with `DerefError` and `ParseError` equivalents: the `Reservation` or the `LimitKind`, and whether the meter was a dependency's. It is the public form of the internal `pool_refused` classifier (`env/registry/dependency.rs`), generalised | a refusal names its pool, its limit and its meter's kind | 2.7 classifies by where the error arose (inside `dep_read` or not), and the manifest says "dependency-side" without the pool |
| **CR5, optional** | `Environment::deref_held(from, pptr)`, returning the target with its top-level `File`, as `get_cab_held` does (`env/deref.rs`) | a dereference returns its target's container held, so no eviction can come between them | the engine takes the `File` from the target's `Parent` before its next Environment call. A Shared attempt that loses the race to another thread's eviction meets `ContainerDropped` and runs Alone, where nothing else evicts |
| **CR6, optional** | a `DecodeWork` refusal on a `Dependency` meter is not kept in a cell, and the Environment evicts that dependency so that its next read reopens it with a fresh allowance | a dependency open's used-up work allowance never poisons its later readers | 3.11's Alone runs, one per allowance's worth of reads of a popular dependency |
| **CR7, text only** | core 3.15 and its constants' table: the provider cache's bound is per `ProviderChain`, shared by every context that clones its `Arc` | none (a correction) | 5.3 counts `C_prov` once and says why |

`DerefError::is_transient` is `pub(crate)` (`env/pptr.rs`). The engine reaches it wrapped as `Error::Deref` through `CellError::is_transient` (`once.rs`, public), and needs no request for it.

---

## 7. Assumed (not checked)

- **A1.** Sizes not measured: a transfer's connection and TLS state is at most 2 MiB (`c_net`), and a parsed spec at most 4 times its JSON. Test: the counting allocator on a 100 MiB download and on a spec at the cap.
- **A2.** One read site's work on one fresh open stays under `4 M`. That work is its runs, its entry's parse and its value, each bounded through `M`. The nightly reports the largest read site's work, and the squeeze row (X13) runs a variant with `max_decode_work` at the read site's measured work.
- **A3.** A read that succeeds through any open holds at least its cold charges on that open at that moment: the source's bytes, its runs, its entry and its value. 2.10's step 4 needs this. Its test is 2.10's injection and pool-size matrix.
- **A4.** A registry entry costs about 300 B per CAB, worked out from core 3.13's list (key, slot `Arc`, rank path, B-tree share, buckets), not measured. F is measured at run time either way (2.2).
- **A5.** Not measured on the corpus:
  - the share of inputs that other inputs read;
  - how many inputs read each shared dependency;
  - the total candidate bytes of a build, which sizes the spool;
  - how often a Shared attempt is refused at realistic budgets;
  - how often inputs are flagged.

  Section 5.5's 1.1 % agreement is an arithmetic check against one measured peak, not a measurement of the model.
- **A6.** The LRU evicts least recently used first, and never the entry a call has just used when that entry's meter is within its bound alone (`env/registry/lru/mod.rs`). With `M <= dep_cache_bytes`, a single-threaded Alone attempt therefore finds a dereferenced target's container alive until its next Environment call.
- **A7.** With E57's settings, glibc's arena retention keeps peak RSS within a few percent of the live heap. The core's rules do not bound fragmentation, and a memory cgroup also counts the spool's page cache. The nightly measures both.
- **A8.** A TPK provider's trees, charged to an unbounded meter (`unityrs-tpk`), are a bounded constant. The corpus's typetrees are embedded, so P1b's runs configure no TPK.
- **A9.** Download time is a small part of a run on the corpus, so finishing registration before extraction costs little (2.1). The brief's figures give sizes and the CPU's split, not wall times.
- **A10.** The MonoBehaviour script hook (`Environment::object_tree`) dereferences `m_Script` inside the core, where 2.6's view check cannot apply. It matters only for stripped MonoBehaviours with a script provider, which P1b does not configure.

---

## 8. Open, for the review rounds

1. **Three of the panel's findings have answers here but no review yet:** 3.11, 3.12 and 3.13. Above all, 2.10's step 4 rests on A2 and A3. A review should look for a read that succeeds in a Shared attempt where its cold read would be refused.
2. **The views are new.** The panel asked for some rule on same-named CABs among dependencies, and the views are this round's answer. Their cost (the key table, the per-attempt check, and private Environments for flagged inputs) has not been weighed against a core request for lookups restricted to a set of loads. That request was considered and not made: the engine can do this with the public API.
3. **E9 is kept, with discovery between rounds** (2.3). Dropping its fetch clause and requiring a complete graph would remove the discovery round. That is the owner's choice (`extract.md` section 3).
4. **DESIGN.md's `unityrs-batch` row** still says "each bundle and dependency parsed once per run, within `dep_cache_bytes`". Section 1 says why it cannot hold. The row is the owner's to change.
5. **Nested containers** are outside D (2.10). Whether the engine should refuse an input with one, instead of reporting it, is open.
6. **Overlapping downloads with decoding** is given up (2.1). A later version may start jobs early when the key table shows that no later registration can share a key with the views they need.
7. **The cross-kind and cross-unit winner rules** (`extract.md` E32) are proposed there, not decided.

## 9. Before it is built

- This plan's review rounds, run until a round finds no MEDIUM or HIGH problem, with section 8 settled.
- CR1 granted, or its fallback accepted. CR2 to CR7 answered.
- `extract.md`'s gate (its section 5) agreed. It holds the engine's tests named here: D (2.10), pool emptiness, charging, the deadlock guard and scans (section 4), and the memory rows (5.8).
