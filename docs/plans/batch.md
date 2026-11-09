# P1b plan: unityrs-batch, the engine (draft)

**Status: draft, design round 3 (2026-11-09), not reviewed.** P1b is 2D batch extraction (`docs/DESIGN.md` section 5; called P2 until 2026-11-09, when the phase table gave the 2D work its own phase). This file is the engine's plan: staging, admission, the attempt, the hand-off to sinks, the stop path and refusal. The rest of the phase is `docs/plans/extract.md`, whose requirements rely on sections 1 and 2 here. The exact contract (the spec's fields, keys, identity and order, the spool, the manifest, the zip bytes, the exit codes) is `docs/BATCH_SPEC.md`.
- Until P1's third review round, `docs/plans/core.md` section 4.1 described a batch flow. Each review round found new problems in its admission and eviction, so the flow left the core plan for this file. Section 3 lists every problem found in it.
- Design round 1 gave one dependency cache shared by every job; its review showed outcomes then depended on what ran beside an input (3.14). Design round 2 made every attempt **sealed** (2.2). Design round 3 folds round 2's review: one stop path that reaches every exit code (2.11), a spool that owns only what it created, bundles staged into the spool so their bytes are pinned (2.5), a time fence on output writes (2.9), and no dependency discovery (2.3). The rounds' dispositions are recorded outside the repository.
- Nothing here is built. Before `unityrs-batch` is, this plan gets its own review rounds, run until a round finds no MEDIUM or HIGH problem.

**What it builds on.** The core API and what it guarantees any batch engine: `docs/plans/core.md` section 4.1, a list of properties each checked by a P1 test. This plan relies on those properties, on the public API named where it is used, and on nothing else the core does. A need the list does not cover is a request to the core (section 6), not an assumption. Each request says what the engine does until the core grants it.

**Citations.**
- P1's review findings are cited by the P1 review round that found them, as `core.md` cites its own. Each problem in section 3 restates what was found, so no review document is needed to read it.
- Design rounds' findings are cited by what they say.
- "core 4.1, *property*" names a row of `docs/plans/core.md` section 4.1 by its first column. "core 3.13" is a section of that plan. A source path without a crate is under `crates/unityrs-core/src/`.
- Corpus figures are aggregates from the private corpus, measured on the P1 core.

---

## 1. Goal

DESIGN.md section 4's `unityrs-batch` row: a JSON job spec of inputs, outputs and resource budgets, run in parallel with bounded memory, streaming outputs to sinks, with the run marked `incomplete` when an input fails.

`docs/plans/extract.md` adds what the engine must give the rest of P1b:
- inputs that each declare their dependencies by id, the spec's whole graph (2.3);
- the own-bundle-first rule for same-named CABs under `EnvOptions::batch()` (3.9);
- a failed dependency failing exactly the inputs that need it (3.10);
- sinks that may hold output across inputs, for the `prefer` winners (3.5).

**Each input decodes its own dependencies.** Each attempt opens the bundles of its view in a dependency cache of its own, so a dependency that N inputs read is opened up to N times and decoded up to N times in what they reach (2.2, its cost). Within one attempt, each bundle of its view is opened at most once per residency in that attempt's cache, and the manifest counts every open and reopen. DESIGN section 4 says this is version 1's behaviour, and that a shared pre-decoded cache, which keeps D, is planned (section 8).

**Two properties the design keeps.** Every mechanism in section 2 serves one of them or a requirement it names.
- **D, determinism.** An input's records are a function of its staged bytes, the staged bytes of the bundles of its view (2.3), the spec's entries for those bundles, and the spec's other fields. Its records are its outputs and their bytes, its skips, and its failure. They do not depend on:
  - `threads`, or the order inputs run or finish in;
  - what else is in flight;
  - which bundles outside its view failed, or which exist.

  2.10 shows why, against every counterexample earlier reviews built, and names the test. D has two limits:
  - a failure of the run as a whole stops it instead of deciding a record: a host failure (an I/O error, an allocation the allocator refuses), the spool's cap, or an engine bug (2.7, 2.11);
  - a bundle that could not be staged (2.5: a download that failed after its retries, a local file that is missing) fails the inputs whose view holds it, and that record depends on the network or the host, not on bytes. Its reasons form the `transport` class (`BATCH_SPEC.md` 9.3), which a caller retries.
- **B, the memory bound.** The process's live heap is at most `max_inflight_uncompressed_bytes` plus constants fixed by the spec (section 5). Every byte that grows with an input or an output is charged to a budget before it is allocated.

---

## 2. The design

### 2.1 Phases

1. **Validate** the spec (`BATCH_SPEC.md` section 1). No thread starts and nothing is written. A bad spec writes the `exit` line and exits 78.
2. **Preflight** (E63). Start the lifecycle, progress and finisher threads (2.11); start the spool (`BATCH_SPEC.md` 6.2: dedication, its lock, old journals, cleanup, free space); check resource limits and the overcommit mode (2.7); set the allocator (E57); read the credentials; take the output root's lock (directory sink); read the stored head (`BATCH_SPEC.md` 7.2), which fences this attempt, fixes the time fence and proves the sink reachable. A failure exits 69, a stored head that supersedes this attempt exits 7.
3. **Manifest `running`** (E38), fenced. No output is written before it.
4. **Stage** every bundle into the spool (2.5), at most `download_concurrency` at once.
5. **Extract** every input whose view staged whole (2.4 to 2.8).
6. **Winners** (2.9): a pure function of the committed records.
7. **Publish** (2.9), from the spool, after the time fence, at most `download_concurrency` transfers at once.
8. **Finish** (2.11): the finisher writes the final manifest and the `exit` line, then exits.

Once preflight has started its threads, every end goes through the finisher (2.11): a normal end, a stop, and a preflight failure, which writes no head. A stop (a signal, the deadline less the grace, the parent's death, stdin's EOF, or a run-level failure) can come in any phase from 2 on.

### 2.2 Sealed attempts

An **attempt** is one run of one input's extraction. Each attempt is sealed: everything that holds memory or counts work during it belongs to it alone.
- **Its budget.** The attempt has one `MemoryBudget` object of its own, built by the engine from atomics. It never blocks, never calls the core and takes no lock (the budget contract, core 4.1; `budget/mod.rs`). It counts its own refusals (2.7). It routes each reservation by its `Reservation`:

  | `Reservation` | limit | what books with it |
  |---|---|---|
  | `Input` | `M` | the attempt's meter `M_X`: its input's bytes, runs, entries, values, and the engine's buffers (2.6) |
  | `Dependency` | `D_X` | every meter of the attempt's private Environment: its opens, runs, entries, registry slots and keys |
  | `Provider` | refused | version 1 configures no typetree provider (A8), so nothing books here |

  `D_X` is `D` (`dep_cache_bytes`) when `view(X)` holds another bundle, and 0 otherwise. The view is fixed by the spec (2.3), so `D_X` is too.
- **Its Environment.** The attempt builds a private `Environment` with `EnvOptions::batch(ctx_X)`, `dep_cache_bytes` set to `D_X`, and registers its view in it (2.6). No other attempt reads through it. It is dropped when the attempt returns.
- **Its context.** `ctx_X` is `FileContext::unitypy()` (`options.rs`: an empty provider chain, stateless codecs, the AssetBundle read at load) with the attempt's budget and the limits below.
- **Limits.** The engine sets, for every meter of the attempt: `max_input_memory` = `M`, `max_input_bytes` = `M`, and `max_decode_work` = `4 M`, the core's default ratio. The other fields keep `Limits::default()` (`limits/mod.rs`).
- **Admission by reservation.** The attempt's **slot** is `S_X = M + D_X`. The controller dispatches an attempt only while the slots in flight sum to at most `I` with it (2.4). The budget's limits are its slot, so the attempts in flight together can never hold more than `I`, and no attempt's reservation is ever refused for what another holds.
- **The rest of the run.** Phases 6 and 7 run when no attempt does, under one **phase budget** of limit `I` (2.9).
- **Leak check.** When the attempt's function returns, every local of it is dropped: the bundle, the private Environment, every file, value and pin. Its worker then reads the budget's `Input` and `Dependency` use. Both must be zero. A non-zero use is reported as a run failure (2.4), and the slot stays reserved.
- **Validation** (`BATCH_SPEC.md` 1.1) refuses a spec unless `M <= D`, `M + D <= I` and `I >= 64 MiB`. The first lets one dependency open at its cap fit the attempt's Dependency budget, its view's registry aside (5.6 sizes for that). The second lets every slot fit with nothing else in flight, so the head of the queue always runs eventually. The third gives phases 6 and 7 their working set.

**Cost.** This is what sealing pays for D.
- **Memory.** Each slot is reserved whole for the attempt's life, used or not. Parallelism is the largest number of slots that fit `I`, at most `threads`. For the corpus example (5.6), 8 slots of `M` = `D` = 256 MiB need `I` = 4 GiB. An input whose view is itself alone reserves only `M`.
- **Throughput.** No dependency is shared between attempts. An attempt reads every bundle of its view from the spool, whole (`load_dependency` reads a `Source::Path` whole, `env/mod.rs`, `read_path`), and decodes again every block it reaches. Measured on the corpus (A5):
  - Most inputs have no dependency: the mean view is about 2 bundles, the largest 22. Dependencies concentrate on a few bundles, a handful of which sit in hundreds to thousands of views.
  - Version 1 crosses a bundle on one read only: the NGUI cutter's step 1, the name of the material's `_MainTex` (`BATCH_SPEC.md` section 5). Sprite sheets and `m_Script` stay inside their bundle on the corpus.
  - At that reach, a full build's extra decoding costs about 2.5 to 3.3 times the CPU of a run that decodes each bundle once: about +30 s of wall time at 8 workers. At worst, with every view member reached, it is 9 to 24 times.
  - All of a build's distinct dependencies decode to about 350 MB, which is what the planned shared cache would hold (section 8).
  - The view members' file reads (27 to 47 GB a full build at worst) come from the page cache, and 2.5 keeps staged files cached.
- **What it removes.** The shared dependency cache, the Shared and Alone attempts, the Alone retry, the registration phase before extraction, the key table and flagging.

### 2.3 Views

**Views.** `view(X)` (`BATCH_SPEC.md` 4.2) is X and the closure of `deps` through the spec's graph. Every id in a `deps` list names a bundle the spec declares (validation, `BATCH_SPEC.md` 1.1). `extract.md` E13's "needs" means "is in the view".

**No discovery.** A job declares its complete graph. The engine reads X's AssetBundle object's `m_Dependencies` only to report: a name that is not the id of a bundle of `view(X)` is a diagnostic row (`undeclared_dependency`: the input and the name, `BATCH_SPEC.md` 7.3) and a run count. It is never fetched or opened, and a read that reaches a CAB only it would hold is the object-level skip of rule 2.

The view rules:
1. **A failed member fails X.** A bundle of `view(X)` that could not be staged (2.5), or whose load in the attempt is refused, fails on its bytes, or gives a file that is not a bundle, fails X with `dependency_failed(<id>, <reason>)` (3.10), where the inner reason is the member's own (`not_a_bundle` for the last). The exit code follows the inner reason (`BATCH_SPEC.md` 9.2). A member that failed at staging fails X before it is dispatched. An I/O error reading a member's staged file is not a member's failure: it is a host failure and stops the run (2.7).
2. **Reads stay in the view.** The private Environment holds only `view(X)`, so every lookup resolves within it. A read whose CAB no bundle of the view holds is an object-level skip, `outside_view(<cab>)`.
3. **Same-named CABs** resolve by `BATCH_SPEC.md` 4.3: the reading file's own bundle first, then X, then the greatest id. This is a Tier C rule of `EnvOptions::batch()` only.
4. **Size.** Validation refuses a spec in which a view holds more than `max_view_bundles` bundles. It bounds the uncharged heap a private Environment holds per registration (5.3).
5. **Members are bundles.** Every bundle is staged under a spool name (2.5), and the core keys a top-level raw or serialized dependency by its file name (`env/load.rs`, `register_file`), so such a member could never be found by its own name. Version 1 refuses it by rule 1.

### 2.4 The controller

**Workers.** `threads` OS threads, each with an explicit stack of twice `Limits::stack_needed` (core 3.15). Each runs one attempt from start to end, on that one thread, inside `catch_unwind`.

**Order.** The ready queue holds every input whose view staged whole, sorted by staged size, largest first, then by bytewise id. Largest first shortens the tail. Order changes no record (D).

**The end message is total.** Every dispatched attempt sends exactly one end message, whatever happens in it. Its outcome is one of:
- `Done`: its directory is ready to commit;
- `Failed(reason)`: an input failure;
- `RunFailure(reason)`: a run-level failure (2.7): a host I/O error, an allocator refusal, a refused spool claim, `ContainerDropped`, an error `classify` does not know, a panic outside a decoder's `catch_unwind` (caught by the worker's), or a leak found by the leak check.

The message is fixed-size: the input, its outcome and reason code, its counters and its peaks. Its records are in the spool, never in the message (2.8).

**The loop.** State: `ready` (sorted), `in_flight` (a count), `reserved` (the sum of the slots in flight).

```
loop:
  if the stop flag is set: hand over to the finisher (2.11) and park
  while in_flight < threads and ready is not empty and reserved + S(head) <= I:
      x = ready.pop(); dispatch(x); in_flight += 1; reserved += S(x)
  if in_flight == 0 and ready is empty: go to phase 6
  wait for one event: an end message | a 1 s tick | the stop flag's wake
  on end message (input, outcome):
      in_flight -= 1
      match outcome:
          Done:            reserved -= S(input); commit the attempt's directory (2.8)
          Failed(r):       reserved -= S(input); delete the directory; commit its failure record (E60)
          RunFailure(r):   request a stop with reason r (2.11); a leak keeps S(input) reserved
```

- The queue is served strictly in order: the head waits until its slot fits. Every slot fits `I` alone, and every attempt returns (its work is bounded, 2.6 step 7), so the head always runs.
- The dispatch channel and the end-message channel each have capacity `threads`. At most `threads` attempts run, so no send blocks.
- The controller's own loop runs inside `catch_unwind`; a panic there requests a stop with reason `internal`.

### 2.5 Staging

Every bundle the spec declares, input or dependency, is staged once, in phase 4, into the spool file `in/i<n>` or `in/d<n>` (`BATCH_SPEC.md` 6.1). Every later read of the bundle, by any attempt, reads that file, never the source.
- **A URL** is downloaded by streaming to the file (E7).
- **A local path** is opened (`O_RDONLY`, a regular file) and copied to the file through a 64 KiB buffer, on the runtime's blocking pool.
- **While it streams**, the engine counts its bytes and computes its SHA-256. At the end, a given `size` or `sha256` that does not match fails the bundle (`integrity_mismatch`). The digest is recorded for the manifest's `bundles` rows.
- **Then** the file is made read-only (mode 0400), and nothing writes it again. The spool is the engine's own directory under its lock (`BATCH_SPEC.md` 6.2), so the staged bytes are pinned for the run (A11).
- **Failures.** A download that failed after its retries fails the bundle (`download_failed`, or `redirect_refused`); a local path that does not exist, is not a regular file or cannot be opened fails it (`source_missing`). These are the transport reasons of D's second limit (section 1). An I/O error while reading an opened local file, or writing the spool, is a host failure (2.7).
- **Space.** Each staged file's bytes are claimed from the spool counter before they are written: a download with a `Content-Length` claims it at its start, one without claims each chunk, a local copy claims its size. A refused claim stops the run (`spool_full`).
- **Concurrency.** At most `download_concurrency` bundles stage at once. Extraction starts when every bundle has staged or failed.

**Why a copy.** A source file can change during a run without changing its size, modification time or inode, and the core reads a dependency's file again after each eviction. A hash of the first read cannot pin a reread the engine does not see. The copy pins every read, and makes a change of the source after its copy harmless. It costs the spool the build's bundle bytes (1.3 to 1.8 GB on the corpus) and one sequential copy, which a filesystem may serve from its page cache.

### 2.6 An attempt

1. **Budget and meter.** Build the attempt's budget (2.2) and `M_X = Meter::for_input(&limits, budget, Reservation::Input)`.
2. **Its own open.** `bytes = read_path(staged file, &limits, &M_X)` (`env/mod.rs`). Then `check_file_type_with_reader(&bytes, &limits, ReaderKind::Stream)` (`detect.rs`) and `parse_file_with_reader(name, bytes, ty, None, &M_X, &ctx_X, ReaderKind::Stream)` (`files/mod.rs`): the rules the core uses for a `Source::Path`. A file that is not a bundle fails X, `not_a_bundle`. A bundle is one file in version 1: no `.splitN` merge.
3. **The AssetBundle object.** Read X's `m_Dependencies` and container rows, and record each undeclared name (2.3), charged to `M_X`.
4. **The private Environment.** Register `view(X)` less X with `load_dependency(Source::Path(staged file))`, one at a time in bytewise order of id, then X last when the view holds another bundle (`env/load.rs`). Drop each returned `File`, so the private LRU holds the only strong handle. A load that fails, or that returns a file that is not a bundle, fails X (2.3, rule 1), and the attempt makes no read through that Environment after it (CR1 explains why). The stop flag is checked between registrations.
5. **Select** (`BATCH_SPEC.md` section 3). Walk the entries and objects in object order, filter by `class_id()` from the object table (no value read), and read typetrees only for the selected objects.
6. **Export one sheet group at a time.**
   - Sprites are grouped by their sheet texture (`BATCH_SPEC.md` 4.4). For each group, decode the sheet (and its alpha texture), render every sprite of the group, then drop the sheet.
   - NGUI atlases are cut one at a time (`BATCH_SPEC.md` section 5). A plain texture is its own group.
   - This replaces UnityPy's per-file texture cache with a cache of size one, so the export buffers are bounded by the largest sheet (5.5). The output is the same: the cache changes when a sheet is decoded, not what it decodes to.
7. **Reads into other bundles.** A pointer is dereferenced with `Environment::deref` on the private Environment, from the file that holds it (`env/deref.rs`). Before its next Environment call, the attempt takes the target's top-level `File` from its `Parent` and holds it while it uses the target (core 4.1, *a dropped container is an error, never a guess*). What is read is copied into the attempt's charged buffers, and every handle into another bundle is dropped when that read ends. After each Environment call the attempt reads the Environment's `stats().dependencies_evicted`; past `max_attempt_evictions` (`BATCH_SPEC.md` 1.1) it fails X, `limit(attempt_evictions, <value>)`. A reopen restarts its meter's work count (core 4.1), so this bound is what makes the attempt's total work finite: at most (1 + the cap) opens of each member, each within `4 M` of decode work.
8. **Resources** (own bundle first, core Q12). Take the candidates of `resource_candidates(path)` (`env/resource.rs`) in UnityPy's order. For each, look in the reading file's own container chain for an entry whose simplified name matches: its container's directory, then each enclosing container's, up to the top-level bundle (`BundleFile::directory`, `parent`). Read a match with `entry_range` through that handle (`files/bundle/mod.rs`). Only when no candidate is an own entry, read through `Environment::resource(from, path, offset, size)` on the private Environment, which covers a candidate in another bundle of the view (`env/resource_data.rs`). The engine's copy of the candidate rule is checked by `own_first_resources_equal_environment_resource_without_same_named_cabs` (3.9) until CR3 moves the rule into the core.
9. **The container** comes from X's own `SerializedFile::container()` (`files/serialized/mod.rs`), never from `Environment::container()`.
10. **Engine buffers** (`extract.md` E59). Every buffer whose size an input chooses is charged to `M_X` at `heap_cost` before it is allocated: RGBA and alpha sheets, the texture's source data, a sprite's mesh arrays (vertices, indices, triangles) and the affine path's buffers, sprite canvases, crops, masks, PNG rows and output, atlas JSON, the sprite-group table, the record buffer. zlib's deflate state is charged as a constant before `deflateInit`, as the core charges LZMA's decoder state (`codec/lzma.rs`). The charges go through `Meter::charge` or `DecodeBuf` (`limits/meter.rs`, `budget/mod.rs`): the core's rule of 3.15, applied to engine code.
11. **Copy-out** (2.8). The attempt checks the stop flag between registrations, sheet groups and objects; a stopped attempt returns at once and commits nothing.
12. **End.** The function returns, which drops everything above. The worker reads the budget (2.2), then sends the end message.

### 2.7 Outcomes

Every error is classified once, by one engine function, `classify` (CR2 would make it the core's). Its classes, and the reason each records (`BATCH_SPEC.md` 9.3):

| what failed | outcome |
|---|---|
| `Budget(Input)` or `Budget(Dependency)` refused by the attempt's budget; `Limit` of kind `InputMemory`, `InputBytes` or `DecodeWork`; the engine's `attempt_evictions` | the budget class (exit 5), recorded where it surfaced: an object-level skip inside an object's read or export; an input failure while opening X or an entry of X; `dependency_failed(<id>, <reason>)` while loading a view member. Reasons: `budget(input)`, `budget(dependency)`, `limit(<kind>, <configured value>)` |
| any other `Limit` (a depth, a count, `BlockOutput`, `Inflate`, a CAB or resource name) | a data reason (exit 3 or 4), at the same sites: `limit(<kind>, <configured value>)` |
| an error the bytes decide: `Detect`, `OutOfBounds`, `Malformed`, `Unsupported`, `Decompress`, `Version`, `TypeIndex`, `Tree`, `AssetBundle`, `Entry`, `NotFound`, a decoder panic (E18), E25's bytes | DESIGN section 4's failure model: a container or entry failure fails X (`container_error`, `entry_error`), an object's failure skips it (`decode_error`, `decoder_panic`). The detail field names the core's variant |
| `DerefError::NotFound` (no bundle of the view holds the CAB) | the object's skip, `outside_view(<cab>)` |
| `DerefError::{Null, FileIdPastEnd, FileIdBelowStart, RawTarget, MissingObject}` | the object's skip, `bad_pointer` |
| `DerefError::Load` | classified as its inner `Error` |
| `Error::Io`, or an I/O error of the engine's on the spool or a staged file | a run failure, `host_io` (exit 74) |
| a `Budget` refusal during whose core call the attempt budget's refusal count did not move | the allocator's refusal: a run failure, `host_memory` (exit 74) |
| a spool claim refused, or free space below the reserve | a run failure, `spool_full` (exit 5): spool use depends on the order inputs ran in, so it decides no input |
| `DerefError::ContainerDropped`; an error `classify` does not know; a panic outside a decoder | a run failure, `internal` (exit 70): the attempt holds every container it reads through (2.6, step 7), and an unknown variant must be noticed at once |

**Why every refusal is final at once.** The attempt runs on one thread in program order. Its budget and every meter that books with it serve that thread alone (2.2). So each charge meets the same state in every run, and each refusal, and each growth a refusal silently shrank (`limits/vec.rs`, `grow_counted`; `budget/mod.rs`, `DecodeBuf::into_bytes`), falls at the same point. No outcome waits for a retry.

**`classify` until CR2.** One module matches the public enums: every `Error` variant, every `DerefError` variant, `ParseError`'s `Read`, `Tree` and `Deref`, `ReadError`'s detail, `TreeError`, `ChargeError` and `LimitKind`. A `DerefError` that `Environment::deref` returns bare is classified as that same chain. Every `#[non_exhaustive]` enum's wildcard arm returns "unknown". Its test builds each wrapper path and checks the class.

**The allocator.** The core turns an allocator's refusal into a `Budget` error (`limits/meter.rs`, `refusal`; `limits/vec.rs`, `reserve_exact`). The attempt's budget counts its own refusals, so a `Budget` error from a core call during which that count did not move is the allocator's, and stops the run (`host_memory`). Two paths absorb an allocator refusal silently: `grow_counted` retries a smaller growth, and `into_bytes` keeps a buffer it could not shrink (`limits/vec.rs`, `budget/mod.rs`). Neither can be seen from outside the core until CR2. So preflight makes them unreachable within the bound: it refuses to start (exit 69) when `RLIMIT_AS` or `RLIMIT_DATA` is below 1.5 times 5.1's bound (glibc's slack, 5.4) plus 64 MiB per arena of `M_ARENA_MAX`, or when `vm.overcommit_memory` is 2. What remains is A13.

### 2.8 Copy-out, records and commit

- **Spool layout and lifecycle:** `BATCH_SPEC.md` section 6.
- **Each output:**
  1. It is encoded in charged memory, and its MD5, CRC-32 and visible hash (E20) are computed.
  2. Its size is claimed from the spool counter, an atomic try-add against `spool.size_bytes`, after a `statvfs` check against `spool.reserve_bytes`.
  3. It is written to the attempt's directory, and its buffer is dropped.
- **Records.** Candidate records (key, unit key, input, kind, path id, MD5, CRC-32, visible hash, mode, size, file, and each archive member path it expands to, `BATCH_SPEC.md` 4.7), skip records and diagnostics are appended to the attempt's `records.ndjson` through a fixed 64 KiB buffer charged to `M_X`. A record longer than the buffer is written through it in pieces. A string a record takes from the bytes (a name, a CAB, a reason's argument) is cut to its first 1,024 bytes, at a code point boundary, with the cut marked (`BATCH_SPEC.md` 9.3). So every line is at most 8 KiB.
- **Commit** (`extract.md` E60, all or nothing per input).
  - An attempt that ends `Done` has its directory renamed to `done/<input>/` by the controller.
  - An input that fails commits only its failure record: no candidate, so it competes for no key.
  - Deleting an attempt's directory returns its claimed bytes to the spool counter.

### 2.9 Winners and publish

**Winners** (E31 to E34, E61; `BATCH_SPEC.md` 4.5 to 4.7), in phase 6. Each step below is an external sort of spool records and one streaming pass over the result, so phase 6 holds no table that grows with keys, units, candidates or members.
- **The sort.** Runs whose records and index together take at most 32 MiB, sorted in memory and written to `winners/`; then merges of at most 16 runs, with a 1 MiB buffer per run and 1 MiB for the output. A line is at most 8 KiB (2.8).
- **Step 1, units.** Sort every unit line by (unit key, input id). One pass decides each unit key's winning unit by E31 and E61, keeping only the best candidate so far: within a key the candidates come in input order, so rules 1 to 3 need no more.
- **Step 2, member keys.** Sort one line (member key, unit key) per key of every winning unit, its unit key included. One pass: of the units claiming one key, the first by unit key keeps it, and every other is withheld whole (`unit_conflict`). Every key a winning unit claims stays reserved, even when its unit is withheld, so the pass is not repeated (`BATCH_SPEC.md` 4.5).
- **Step 3, plain keys.** Merge the sorted plain candidates with step 2's reserved keys. A plain candidate for a reserved key loses (`unit`). Every other key is decided by E31 and E61, streaming as in step 1. The output is the set `P` of keys the decision would publish, in key order.
- **Step 4, prefix and archive conflicts** (`BATCH_SPEC.md` 4.6). Sort `P`'s keys and the archives' keys, each with its ancestors (`a`, `a/x` for `a/x/y.png`) as markers. One pass refuses a key of `P` that is an ancestor of another key of `P` (`prefix_conflict`), or that equals, contains or lies under an archive's key (`archive_conflict`). A unit with a key so refused is withheld whole: its unit keys are sorted, and a merge with `P` marks its other keys `unit_conflict`. The check is not repeated. What is left is the **winners file**, in key order.
- **Step 5, archive members** (`BATCH_SPEC.md` 4.7). For each winner, one line (archive, member path, key) per archive member path its record holds. Sort by (archive, member path), with each path's ancestors as markers. One pass keeps the first key for a path (`duplicate_member`) and refuses a path that is an ancestor of another (`prefix_conflict`), and writes each archive's member list, in member order, with each member's spool file, size and CRC-32.
- **Rows.** The `outputs` and `collisions` row files are written in key order from steps 3 and 4, one line per (key, candidate) for collisions; `archive_members` from step 5.
- With `publish_incomplete: false` nothing is published when any input failed.
- The decision is a pure function of the committed records, so it is the same whatever order the inputs finished in.
- **Its working set** is at most 40 MiB: a run of 32 MiB, or a merge's 17 MiB, beside at most 8 stream buffers of 1 MiB. `I >= 64 MiB` holds it, so a refusal of the phase budget in phase 6 marks a leak, a run failure `internal`.

**Publish**, in phase 7.
- **The time fence** (`BATCH_SPEC.md` 7.2). When preflight read an earlier attempt's head of this `run_id` that may still have a write in flight, phase 7 starts no output write before that head's fence time. The wait is a timer the stop cancels. So every write of an earlier attempt has landed before this one LISTs or writes, and nothing it wrote can land after.
- **Resume** (E43). With `resume` set, the object store's LIST of the output prefix (keys in ascending UTF-8 byte order) is merged with the winners file, one LIST page in memory at a time. A winner whose key is listed with its expected ETag (its MD5; an archive's, or a file's over 16 MiB: its multipart ETag at 16 MiB parts, `BATCH_SPEC.md` section 8) is `present` and not PUT. An archive's expected ETag needs its bytes: resume computes it in a first pass over the archive's stream, without uploading. A LIST page out of order, or a LIST that fails, degrades to "PUT everything" with the warning `resume_list_failed`. In a directory, each winner's destination is stat'd, and hashed only when its size matches.
- **Transfers.** A transfer reserves its buffer from the phase budget once, at its start: `heap_cost(size)` for a single-part PUT (files up to 16 MiB), or one 16 MiB part for a multipart upload, whose parts go one at a time. It holds that reservation until it ends. The scheduler takes winners in key order, then archives in key order, and starts a transfer when a `download_concurrency` permit is free and its reservation is granted; otherwise it waits for a running transfer to end. A reservation refused with no transfer running is a run failure `internal`: every buffer is at most 16 MiB, below `I`.
- **Acknowledgements.** The scheduler keeps, behind a mutex never held across a request, the index below which every winner is acknowledged and the set of acknowledged indices above it (fewer than `download_concurrency`), and one state per archive. The finisher reads them (2.11). They are constant in size.
- **The directory sink** holds the root's lock (`BATCH_SPEC.md` 7.2) for the process's life, writes each file as a temp file named `.unityrs-tmp-<run_id>-<n>` beside its destination and renames it, opens paths beneath the root (E62), and runs `syncfs` before the final manifest. No key segment may start with `.unityrs-tmp-` (`BATCH_SPEC.md` 2.3), so the start's cleanup (E62) removes only temp files.
- **The zip format** (`BATCH_SPEC.md` section 8) streams each archive from its member list, with its central directory spooled while the members stream.
- **The object store** follows `extract.md` E37. Each multipart upload id is appended to the journal after its create and before its first part, and removed after its complete or abort (E64).

### 2.10 Why D holds

**The argument.** An attempt is a program run on one thread. Everything it reads is fixed by D's inputs:
- its staged bytes and its view's staged bytes, read whole from read-only spool files that nothing writes during the run (2.5);
- the spec: its limits, `M` and `D_X`, its rules, its entries for the view's bundles;
- the registration order, `BATCH_SPEC.md` 4.3, which depends only on the ids in the view.

Everything it writes to is its own: its budget, its meters, its private Environment's registry, LRU, cells and work counts. No other thread books with them (2.2). The core keeps no state across threads but the budget and the provider chain, and its thread-locals carry nothing from one call to the next (A12); version 1's provider chain is empty. So the attempt computes the same records in every run. Whether it runs at all depends only on whether its view staged (D's second limit).

**Against earlier reviews' counterexamples.**

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
| a source file rewritten in place between two attempts' reads, its size and modification time kept (design round 2) | every attempt reads the staged copy (2.5) |
| another input's declaration of a discovered name adding bundles to X's view (design round 2) | there is no discovery; a bundle has one declaration, and only declarations reached from X enter its view (2.3) |
| an allocator refusal that depends on the process's total memory (design round 2) | a seen one stops the run (2.7); a silent one is unreachable under preflight's checks (A13) |
| a download that times out only beside others (design round 2) | staging's timeout is per idle period, not per request (`BATCH_SPEC.md` 1.4), so a slow neighbour costs time; a failure that remains is D's second limit |

**Tests.**
- `an_inputs_records_do_not_depend_on_what_runs_beside_it`. The engine scene runs at `threads` 1, 2 and 8; with the inputs in 20 seeded orders; with `I` at the smallest valid value and at 4 times it; with an unrelated dependency that fails to download; with a bundle added that holds a same-named copy of a CAB in another input's dependency. Every input's records must equal those of a run whose spec holds only that input and the declarations of its view, timings and counters aside.
- `round_one_counterexamples_change_no_record`: one scene per row of the table above, each compared with its solo run.
- `an_attempt_books_with_no_budget_but_its_own`: a test budget records the thread of each reservation; every reservation of an attempt comes from its worker, while its attempt runs.
- `a_view_member_that_fails_to_load_fails_the_input_and_nothing_reads_after_it`.
- `a_source_rewritten_during_the_run_changes_no_record`: a local dependency rewritten in place, size and modification time kept, after staging.

### 2.11 Lifetime, stops and the finish

**Threads.** The controller (the main thread), `threads` workers, the lifecycle thread, the progress thread and the finisher, all started in preflight; a failed spawn exits 69 before anything is written. Staging and transfers are async tasks on a current-thread runtime that only the controller drives, with `block_on`. No async task calls the core. Each runtime's blocking pool is capped at `download_concurrency` threads, and its HTTP client keeps at most `download_concurrency` idle connections per host.

**One stop path.** A stop has a **reason**, and each reason one exit code (`BATCH_SPEC.md` 9.3): `deadline`, `sigterm`, `sigint`, `sighup`, `parent_died` and `stdin_eof` exit 6; `superseded` 7; `spool_full` 5; `host_io` and `host_memory` 74; `internal` 70.
- Anyone requests a stop by writing the reason's byte to the lifecycle thread's self-pipe: the signal handlers (SIGTERM, SIGINT, SIGHUP), the lifecycle thread's own timer and stdin watch, and the controller (a `RunFailure`, a spool claim refused in staging, a phase-budget leak in phase 6, an I/O error in phase 7). The write never blocks: the pipe is non-blocking, and a full pipe already holds a stop.
- **The lifecycle thread** owns the stop. Its only I/O is reading its self-pipe and stdin. On the first stop it records the reason, sets the stop flag, cancels the controller runtime's token, wakes the controller and the finisher, and arms the **hard exit** at `T_hard = min(deadline, now + grace) - 500 ms`. A later reason replaces the recorded one when its code comes first in `BATCH_SPEC.md` 9.1's precedence, until the finisher takes the reason for the head (step 3 below).
- **At `T_hard`**, if the process still runs, the lifecycle thread calls `libc::_exit` with the recorded reason's code. It writes nothing first.
- **A second stop signal** calls `_exit` at once, with the recorded code.
- **The deadline** is converted once at start to a `CLOCK_BOOTTIME` instant. The stop starts at `deadline - grace` (E45).
- **Parent death** (E46). At start the binary sets `PR_SET_PDEATHSIG` (SIGTERM). When the spec gives `parent_pid`, it then compares `getppid()` with it, and a difference (the parent died before the call) stops at once. Without `parent_pid` there is no start check.

**What the stop flag stops.**
- The controller dispatches nothing more, leaves its loop, hands over to the finisher and parks until `_exit`: `main` never returns, which would run exit handlers under the finisher. It joins no worker and no transfer, and drives its runtime no more.
- Each attempt returns at its next check (2.6 step 11), and commits nothing.
- Each staging and transfer future selects on the cancellation token. Its future is frozen, never polled again, and its buffers and phase-budget reservations stay until `_exit`. A request the server had already received may still complete there.
- No transfer starts a committing request after the flag: no PUT, no `CompleteMultipartUpload`, no rename. No new multipart upload starts.
- Phase 6 checks the flag between sort runs.

**The finisher** owns every manifest write after `running`, the `exit` line and the process's exit, on a normal end and on a stop alike. It is started in preflight, parked on a one-shot channel, and run once: by the controller's hand-over after phase 7 (a normal end, no reason) or after a stop. It runs on a current-thread runtime of its own, with a store client of its own built in preflight, and never uses the controller's: that client's pooled connections are driven by tasks on the controller's runtime, which nobody drives after a stop. In order:
1. **Close the journal.** Under the journal's mutex it marks the journal closed, then reads it. A create whose response comes after the close is never journaled, and the bucket's lifecycle rule removes its upload (E64).
2. **Abort** every upload id the journal holds, in parallel, at most `download_concurrency` at once, each request with a timeout of `min(5 s, T_hard - now - 2 s)`; when that is under 500 ms, it aborts nothing and leaves them to the next attempt's start (`BATCH_SPEC.md` 6.2).
3. **The final manifest.** It takes the stop reason (none on a normal end), which no later stop then changes, and the phase the controller reached. What it writes is `BATCH_SPEC.md` 7.4's row for that phase: nothing before the `running` write was sent; otherwise every row file, streamed from the spool and from the scheduler's acknowledgements without a sort, then the head, fenced (7.2), then `syncfs` for a directory. When the `running` write's fate is unknown (cancelled in flight), it reads the head back first and writes only when that write landed. A failed precondition is read back once (7.2); a head that is not this write means `superseded`, and the finisher writes nothing more. A write that fails otherwise leaves the stored head as it was, and the code becomes the stop's or 4 (`publish_failed`).
4. **The `exit` line.** It hands the line to the progress thread, which writes it before anything else and never overwrites it, and waits for the progress thread to report it written, at most `min(1 s, T_hard - now)` on a stop and 1 s on a normal end.
5. `libc::_exit(code)`: on a normal end the code of `BATCH_SPEC.md` 9.2 from the records, on a stop the recorded reason's, on a preflight failure 69 (or 7 for a superseding head).

**Its working set** is fixed: a 1 MiB read buffer and a 1 MiB write buffer per row file it streams, one at a time, and its runtime and client (5.3, `C_fin`). The rows it writes are already in their order in the spool: the `inputs`, `bundles`, `skips` and `diagnostics` rows in input-id order through the spec's id permutation, which the controller keeps from validation (`C_spec`); the `outputs`, `collisions` and `archive_members` rows from phase 6's files, each line's status taken from the acknowledgements.

**When the grace runs out first,** the lifecycle thread's `_exit` ends the process. The stored head then says whatever its last write said: `running` with its deadline when the final write did not land. A reader takes a `running` head past its deadline as a dead attempt. The `exit` line may be missing; the exit status is authoritative.

**Panics and stderr.** The binary installs its own panic hook at start. It copies the payload's first 1,024 bytes into a preallocated slot of the panicking thread and writes nothing; the worker's or the controller's `catch_unwind` then reads the slot into the skip record or the run failure. Every stderr write the engine makes goes through one non-blocking writer, which drops a line on `EAGAIN` and counts it. So a stderr that is never read stalls nothing.

**What cannot be bounded.** Every wait above has a timeout or a cancellation but one: a filesystem call (a read, a write, a rename, `fsync`, `syncfs`, `statvfs`) on a filesystem that stops answering. Linux cannot interrupt a thread in such a call. The plan assumes that the spool and the output directory are local filesystems whose calls complete (A11); after staging, every bundle read is a spool read. When one does not, `_exit` still runs at `T_hard`, but the kernel ends the process only when the stuck call returns, so the process may outlive its deadline. The caller's own timeout and `SIGKILL` are the last resort, and the stored head says what it last said.

### 2.12 What an attempt never waits on

- on a budget: its budget only `try_reserve`s (core 4.1, *budgets only `try_reserve`*);
- on another attempt: nothing it uses is shared (2.2);
- on a core lock held by a waiter: every core lock is held only to look up or to publish (core 4.1, *no caller code runs under a core lock but the budget*);
- on the controller, once it has started: its end message's send never blocks (2.4);
- on an engine lock: the attempt takes none;
- on the network: every bundle it reads was staged before extraction;
- on stderr: the panic hook writes nothing (2.11);
- on the spool: a claim is an atomic try-add that refuses, and a read or write is a local file call (A11).

---

## 3. Open problems, and their answers

Each problem keeps the text that states it, then its answer: solved or shown wrong, the mechanism, what it rests on, and its test. Engine test names are proposals, in `crates/unityrs-batch/tests/`. Sections 3.11 to 3.13 are problems design round 1 found; 3.14 is the one its review found; 3.15 lists design round 2's review.

### 3.1 Cached runs never release, and budgets are bound per load, not per job

P1 round 1. Under the old step 1 every input was loaded into the shared Environment, which never evicts caller-loaded files, and each kept its decoded runs and parsed entries for the Environment's life, so the in-flight cap was reached and never released. A per-job budget could not reach a bundle opened with another `FileContext`. A dependency read from job X into input Y decoded Y's blocks on a budget nobody admitted.

**Solved** (2.2). Each attempt owns its input's only job handle and its private Environment, both on its own budget, and drops both when it returns; the worker reads the budget at zero before the slot is freed. A read into Y from X is a read through Y's copy in X's private Environment, on X's Dependency budget. **Rests on** core 4.1 *one meter per input, charged before every input-sized allocation*, *a second handle on the same bytes is independent*, *`Environment::load_dependency(source)`*. **Tests:** `every_attempt_returns_its_charges` (after every attempt, both sides of its budget read zero and `Arc::strong_count(M_X) == 1`); `an_attempts_end_message_holds_no_core_type` (a source scan of its fields against `Bytes`, `File`, `ObjectRef`, `Parsed`, `HeldCab`, `Charge`).

### 3.2 The prepaid estimate is too small, and the alone path can hang

P1 round 2. The estimate had to cover what the core's rules can book, and was still an engine guess against charges made at actual sizes; "Alone" at a cap above the in-flight bound waited forever.

**Shown wrong as a useful estimate; solved without one.** No estimate is provable below `max_input_memory`: an LZMA block's output ignores its declared size (core 3.6), and up to `max_zero_size_elements` values per object read consume no input (`limits/mod.rs`). Admission reserves each attempt's slot, `M + D_X`, which is the provable bound, and validation requires `M + D <= I`, so the head of the queue always fits once nothing else runs. **Tests:** `the_controller_always_progresses` (a model of the loop with fake attempts of random slots, durations and outcomes, run failures and leaks among them: no state has an input queued with nothing running and no stop requested); `a_hostile_lzma_block_fails_only_its_input` (a block declared at 1 byte that decodes to 64 MiB, among 50 small inputs: it fails on its own meter, and every other input's records equal its solo run's).

### 3.3 No eviction mechanism, resource reads through the shared handle, untagged refusals

P1 round 2. Registry handles were never released; a job's own `.resS` was served by the shared handle; both pools refused with one error.

**Solved.** The core's LRU evicts within each private Environment, bounded by `D_X`, and its pool-refusal retry evicts the others (core 3.13). A job's own resources are read through its own handle (2.6, step 8). Refusals are tagged `Input` or `Dependency` by the core, and 2.7 records either as final. **Tests:** `a_dependency_cache_smaller_than_the_view_costs_reopens_not_a_scheduling_dependent_record` (the same records at `threads` 1 and 8 with `D` = `M`); `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job` (3.8).

### 3.4 The first admission can wait forever, and the estimate is inflated

P1 round 3, in the plan's and the code's reviews. A bundle whose estimate was above the in-flight bound never fit, and the FIFO controller hung every bundle behind it; the estimate counted one block once per entry.

**Solved: there is no estimate.** Validation's `M + D <= I` replaces the cap, and an input whose needs pass its budget fails on it at once (2.7). **Tests:** `a_spec_whose_slot_exceeds_the_inflight_bound_is_refused`; `an_input_above_its_meter_fails_once_and_the_run_goes_on`.

### 3.5 What a consumer holds is charged to a job that may never wait

P1 round 3, in the plan's and the code's reviews. A slow sink's backlog was charged to the job; a sink that held output across jobs waited on a job the controller would not admit.

**Solved: copy-out, with winners decided after extraction** (2.8, 2.9). An attempt hands no core type to anyone: it writes outputs and records to the spool. Winners are a pure function of the committed records, decided once every input has run, and sinks run only in phase 7, so a slow sink lengthens the run and changes no record. **Cost:** the spool holds every candidate until phase 7 (5.7). **Tests:** `winners_are_the_same_whatever_the_completion_order`; `a_stalled_sink_never_stalls_an_attempt` (every request held open: extraction ends, and the run exits 6 by its deadline); `an_atlas_wins_as_one_unit`; `a_failed_input_commits_no_candidate`; `a_key_whose_preferred_input_failed_is_withheld`; `a_unit_with_a_refused_member_key_is_withheld_whole` (a member key that loses to `prefix_conflict`, `archive_conflict` or another unit); `phase_6_stays_within_its_working_set` (a million candidates of one key, and a million keys, under the counting allocator).

### 3.6 The old step 1 named a load the API did not have

P1 round 3. The only way to open a source into the Environment gave an `Input` meter that was never evicted.

**Solved.** Every registration is `load_dependency(Source::Path)` (core 3.13), in a private Environment with no resolver, so no lookup can start a download or a registration. **Test:** `a_lookup_never_loads_outside_the_view` (an input whose `m_Dependencies` names an undeclared bundle that holds the CAB it reads: the read is `outside_view`, and the name is a diagnostic row).

### 3.7 Problems settled in the core that the design must not undo

| finding | the core property now (core 4.1), and its use here |
|---|---|
| P1 round 1: a codec allocated before the budget saw it | one meter per input, charged before every input-sized allocation. The engine's buffers follow the same rule (E59) |
| P1 round 2: a retained `Parsed` carried no charge | every `Parsed` carries its charge. Values never leave an attempt (copy-out) |
| P1 round 3: a cell published a pool refusal for good | transient refusals are never cached. Inside one attempt, a refusal that `with_room`'s retry clears is not kept for the attempt's later reads |
| P1 round 3: resolving a resource parsed the shared slot | resource reads through `from`'s own handle. Not needed: the engine reads own entries through the job's handle first (2.6) |
| P1 round 3: eviction kept a `Path` dependency's bytes | eviction frees what it can re-read. Every registration is a `Path` |
| P1 round 3: a dereference from a dropped bundle was unspecified | `DerefError::ContainerDropped`. The attempt holds the target's top-level `File` while it uses the target (2.6) |
| P1 round 3: decode work under `Lazy` was unbounded | `max_decode_work` per open input; a reopen restarts the count (P1 round 4). It bounds one open's work; the engine's `max_attempt_evictions` bounds the opens (2.6 step 7) |
| implementation review round 6, the Environment's MEDIUM-3: a load published its keys one at a time | each key's registration is published whole, and a load returns after all its keys (core 3.16 item 6). The attempt registers its whole view before its first read (2.6) |

### 3.8 The resource redirect and eviction cannot both hold for a job's second handle

P1 round 4. The core redirects a resource read through the job's handle only for a slot it can match without reading; an evicted `Path` slot loses the redirect, and a `Bytes` slot keeps its bytes for the run's life.

**Solved by needing neither.** The engine reads a job's own entries through its handle before any registry lookup (2.6), so every registration is `Source::Path` and eviction frees its bytes. **Cost:** an input that its own view's other bundles point back into is also registered in its private Environment and read whole a second time. **Test:** `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job`: its own resource reads charge nothing to its Dependency budget.

### 3.9 Same-named CABs: the own-bundle-first rule

`docs/plans/core.md` section 9, Q12 (decided 2026-10-30) gives this plan the rule.

**Solved; the rule lives in the engine until CR3 moves it into the core.** Resources: own container chain first (2.6). The container: X's own `SerializedFile::container()`. Pointers: the core's `deref` from the reading file, then the private registry, where X is registered last (`BATCH_SPEC.md` 4.3). `EnvOptions::unitypy()` is untouched. **Tests:** `own_bundle_first_for_resources_and_container` (two inputs each hold `cab-x` and `cab-x.resS` with different pixels, and each lists the other; each input's textures and container rows are its own, under `EnvOptions::batch()` and an Eager variant); `a_pointer_from_a_dependency_reads_that_dependencys_sibling_first`; `own_first_resources_equal_environment_resource_without_same_named_cabs` (the engine's rule against `Environment::resource` under `EnvOptions::unitypy()` on every testkit scene without same-named CABs; `PARITY.md` section 19.2's row).

### 3.10 A failed dependency fails the inputs that need it

A dependency that fails to download or parse fails every input that needs it, and no other (DESIGN.md section 4's failure model).

**Solved** (2.3). "Needs" means "is in the view". A bundle fails when it cannot be staged (2.5), its `size` or `sha256` does not match, or its load in an attempt fails. Inputs whose view does not hold it never open it. A read that fails for a dependency's own bytes found at read time (a corrupt block or entry) is an object-level skip of the input that read it. **Tests:** `a_failed_dependency_fails_exactly_the_inputs_whose_view_holds_it` (A declares D and reads it, B declares nothing, C declares D and never reads it, W declares U, which holds a CAB named like one of D's; D fails by a 404, a truncated body, a corrupt header and a corrupt data block. For the first three A and C fail and B's and W's records equal a run where D is whole; for the corrupt block A skips the objects that reach it and C is unaffected); `a_shadowing_bundle_outside_the_view_changes_no_record`; `a_dependency_that_is_not_a_bundle_fails_its_inputs`.

### 3.11 A cached dependency's work allowance is shared by every input that reads it

Found in design round 1. A cached dependency read by many inputs built one shared work count, and a used-up allowance was kept by its cells for every later reader.

**Solved** (2.2). Each attempt opens its own copy, with its own allowance. **Test:** `round_one_counterexamples_change_no_record`.

### 3.12 A direct read of a dependency object gets no evict-and-retry

Found in design round 1. A value read on a dependency's `ObjectRef` is no Environment call, so its refusal gets no eviction and retry (core 3.13), and could be refused by a pool full of evictable entries.

**Shown not to matter for D.** Inside a sealed attempt, whether such a read is refused depends only on the attempt's own earlier reads, so it is the same in every run. It can refuse a read that would fit after an eviction: a cost in outcomes, the same in every run. A core request for the retry on direct reads is a follow-up (section 8).

### 3.13 A same-named CAB in a bundle an input does not need can change its answers

Found in design round 1. Ranks give a CAB key to the registration of greatest rank, so a bundle outside X's view, or its failure, could change X's reads.

**Solved** (2.3). The private Environment holds only `view(X)`. **Test:** `a_shadowing_bundle_outside_the_view_changes_no_record`.

### 3.14 A shared dependency cache makes outcomes depend on what runs beside an input

Found in design round 1's review. Design round 1's D argument rested on "a read that succeeds anywhere holds at least its cold charges". It does not: a decode's transient peaks above the run it leaves, growths and shrinks react silently to refusals of a shared pool, overlapping spans make cached runs order-dependent, and a private Environment's registry took room a shared one did not. Each gave a record that depended on another input.

**Solved by sealing** (2.2, 2.10), at the cost 2.2 states.

### 3.15 Design round 2's review

| problem found | answer |
|---|---|
| a run-level failure inside an attempt (a leak, a panic, a host I/O error, a full spool) had no path to stopping the run, so the controller waited forever or every stop exited 6 | the end message is total (2.4), and every stop, internal or external, goes through one path with its own reason and code (2.11). **Test:** `every_stop_reason_exits_with_its_code` (each reason, injected in each phase, with and without a deadline) |
| the stopper and the controller could both write the final head, and the stopper's read-back could take the controller's write for its own | the finisher makes every write after `running`, once (2.11). **Test:** `a_stop_during_the_final_write_writes_one_head` |
| the stopped manifest was undefined before phase 3 and after phase 6, and its rows needed a sort outside the bound | `BATCH_SPEC.md` 7.4 defines it per phase; the finisher streams rows already in order, within `C_fin` (2.11, 5.3) |
| the spool's start deleted every entry under a caller's directory, the lock file among them | the spool is a dedicated directory the engine marks, the lock is never deleted, and the start's steps have a fixed order (`BATCH_SPEC.md` 6.2). **Test:** `a_spool_that_is_not_dedicated_is_refused_untouched` |
| a source rewritten in place could pass the source check | staging (2.5) |
| a superseded attempt's late PUT could leave the store holding bytes the head does not describe | the time fence (2.9; `BATCH_SPEC.md` 7.2). **Test:** `a_late_put_of_a_superseded_attempt_lands_before_the_next_attempt_writes` |
| an attempt's work was unbounded through reopens after eviction | `max_attempt_evictions` (2.6 step 7). **Test:** `an_attempt_that_thrashes_its_cache_fails_on_its_eviction_cap` |
| allocator refusals were both a run stop and an input's record | a seen one stops the run; preflight makes the silent ones unreachable (2.7, A13) |
| a non-bundle dependency was keyed by its spool name | members must be bundles (2.3, rule 5) |
| stderr was an unlisted wait that panics write to | the panic hook and the non-blocking writer (2.11), W13 |

---

## 4. Why it cannot deadlock

Every wait in the process:

| # | who waits | for what | why it ends |
|---|---|---|---|
| W1 | the controller, phase 5 | one event: an end message, a 1 s tick, the stop flag's wake | every dispatched attempt sends exactly one end message (2.4), and attempts only send, on a channel of capacity `threads` |
| W2 | the controller, in `block_on` (phases 2, 3, 4, 7) | network requests, the time fence's timer | each request has a timeout (`BATCH_SPEC.md` 1.4 and 1.6), and `block_on` selects on the cancellation token |
| W3 | an idle worker | the next attempt on the dispatch channel | it holds nothing, and the controller dispatches before it waits whenever a slot fits |
| W4 | an attempt | a core lock | held only to look up or publish, never across a decode or another lock; the only caller code under one is the budget, which is atomics (core 4.1) |
| W5 | an attempt, the controller, the finisher, a blocking-pool thread | a local filesystem call | A11 |
| W6 | a transfer | a `download_concurrency` permit, or the phase budget | held only by transfers inside requests, which end by W2 |
| W7 | a transfer or the finisher | the journal's mutex; the acknowledgements' mutex | held for one append or one update, never across a request; the file call inside is W5 |
| W8 | the progress thread | stdout | nothing waits on it but the finisher, for at most 1 s (2.11) |
| W9 | the lifecycle thread | the self-pipe, its timer, stdin | nothing waits on it; it does no I/O but those reads and `_exit` |
| W10 | the finisher | its hand-over; abort requests and the manifest's writes | the hand-over comes from the controller's end or a stop; each request is on its own runtime and client, with a timeout inside `T_hard` (2.11); file calls are W5 |
| W11 | the spool's and the output root's locks | `flock` with `LOCK_NB` | never waits: busy exits 69 |
| W12 | the controller, after its hand-over | nothing: it parks | the finisher's or the lifecycle thread's `_exit` |
| W13 | any thread | stderr | never waits: the panic hook writes nothing, and the engine's writer is non-blocking (2.11) |
| W14 | a requester of a stop | the self-pipe | never waits: the pipe is non-blocking, and a full pipe already holds a stop |

**There is no cycle.** Rank the parties: 0, the kernel, the network and stdout; 1, core locks, the budgets and the engine's mutexes; 2, a running attempt; 3, transfers and the finisher; 4, the controller; and the lifecycle thread apart. Every wait-for edge goes to a lower rank, or to rank 0 with a timeout, except W3's, which comes from an idle worker that holds nothing, and W10's hand-over, which the controller sends before it parks. The lifecycle thread waits on nothing in the process, and its `_exit` at `T_hard` ends every other wait but a stuck file call (A11). The core blocks nowhere (core 4.1, *the core never waits and never starts a thread*). No engine lock is held across a core call or a network request.

**Tests.** Every engine test runs under `unityrs_testkit::guard`, which fails a hang instead of waiting forever. The scenes include a stalled sink, a stalled download server, a stdout and a stderr that are never read, and a stop in each phase; `a_wedged_attempt_cannot_pass_the_deadline` (a test decoder spins, and the process exits 6 by the deadline); `a_stop_during_staging_exits_within_the_grace`; `a_leaked_charge_stops_the_run_with_70_and_no_deadline`. `the_engine_starts_no_unbounded_wait` is a source scan in the core's style (`unityrs_testkit::waits`): it finds every `recv`, `lock`, `join`, `wait`, `park`, `flock`, `write` to stderr and `block_on` in `unityrs-batch` and checks each against this table, by module, and that no lock guard is alive across a call into `unityrs_core` or an `.await`.

---

## 5. The memory argument

### 5.1 The bound

In the charge unit (`heap_cost`, glibc's chunk, core 3.15), at every instant:

```
heap <= I + C_thr + C_env + C_net + C_spec + C_fin + C_base
```

**Proof.**
1. Every allocation that grows with an input or an output is charged before it is made. The core charges its own (core 4.1, *one meter per input, charged before every input-sized allocation*). The engine charges its buffers, the attempt's records of the AssetBundle object, the record buffer (2.6), the sort's runs and buffers, the resume merge's page and the transfers' buffers (2.9) under the same rule.
2. During extraction, each charge books with one attempt's budget, whose limits are its slot. The controller dispatches while the slots in flight sum to at most `I`, and frees a slot only after its budget read zero (2.2); a leaked slot stays reserved.
3. In phases 6 and 7 no attempt runs, and every charge books with the phase budget, of limit `I`.
4. During a stop, the attempts still in flight hold at most their slots, and frozen transfers at most their phase-budget reservations; neither budget grants anything new beyond its limit, and the finisher allocates only `C_fin`.
5. Each budget refuses any reservation that would pass its limit, in one compare-and-swap.
6. Everything else the process allocates is a constant term of 5.3.

Records, candidates, the winners, the archive member lists, the zip central directory and the manifest's rows live on disk (2.8, 2.9), so no term grows with the number of outputs, skips, members or keys in the store.

### 5.2 Every holder of charged bytes

| holder | budget | taken | released |
|---|---|---|---|
| the attempt's copy of its input's bytes | the attempt's `Input` | its start | its end |
| the job handle's runs, parsed entries, compiled trees, kept errors (core 3.6, 3.15) | `Input` | as read | its end |
| the values of X's objects | `Input` | per object | after that object's outputs |
| a block decode's transients: `DecodeBuf`'s old and new room while it grows, LZMA's dictionary and state | whichever meter decodes | during the decode | at its end |
| export buffers, one sheet group at a time | `Input` | per sheet | when its last output is spooled |
| the AssetBundle object's records, the sprite-group table, the record buffer | `Input` | the attempt | its end |
| the keys of a lookup | the meter of the lookup's `from` | during the call | at its return |
| each private open: a `Path` source's bytes, its runs and entries | the attempt's `Dependency` | load, or reopen after eviction | eviction, or the attempt's end |
| the private registry's slots and keys | `Dependency` | registration | the attempt's end |
| a read's handles into another bundle | `Dependency` | the read | the read's end |
| the sort's runs and merge buffers | phase | phase 6 | before phase 7 |
| the resume merge's LIST page; a transfer's buffer; an archive's hashing pass | phase | phase 7 | its end, or `_exit` for a transfer frozen by a stop |
| staged bundles, outputs, records, sorted runs, member lists, archives' central directories, row files | disk (spool) | their write | the spool's next start (`BATCH_SPEC.md` 6.2) |

### 5.3 The constant terms

| term | value | source |
|---|---|---|
| `C_thr` | `threads x (2 x stack_needed + 0.11 MiB) + 4 x 2 MiB + 2 x download_concurrency x 2 MiB` | each worker's stack of twice `Limits::stack_needed` (release: 1.75 MiB, `limits/mod.rs`; 2.4), counted whole, plus its detection scratch, inflater state and window and tally (0.11 MiB, core 3.15); the controller, lifecycle, progress and finisher threads at 2 MiB each; the two runtimes' blocking pools, each capped at `download_concurrency` threads of 2 MiB (2.11) |
| `C_env` | `threads x (c_env + max_view_bundles x 512 B)` | each private Environment's fixed parts, uncharged (core 3.15, one per Environment), and per registered bundle the uncharged `Meter::OWN_HEAP`, `Dependency` record and path (`limits/meter.rs`, `env/load.rs`), 512 B at most (A1) |
| `C_net` | `2 x download_concurrency x c_net` | connection, TLS and a 64 KiB streaming buffer per transfer or stage, `c_net <= 2 MiB` (A1), for the controller's client and for the finisher's (2.11), whose idle pools are capped at `download_concurrency` per host. Staging streams through that buffer; upload payloads are in the phase budget |
| `C_spec` | 4 x the spec's size + 256 B x the declared bundles | the parsed spec, the controller's per-bundle state and the id permutation (A1). A spec over 16 MiB is refused, so every factor is fixed by the spec |
| `C_fin` | 8 MiB | the finisher's two 1 MiB row buffers, its runtime and its panic and progress slots (2.11) |
| `C_base` | measured | the binary, the runtime, the progress thread's slots, the controller's queues, the panic slots |

There is no provider term: version 1 configures no typetree provider (A8), so the provider cache (`ProviderChain::DEFAULT_CACHE_BYTES`, 64 MiB, one per `ProviderChain` shared by every cloned context; CR5) holds nothing.

### 5.4 Outside the charge unit

- **glibc's slack.** A live allocation whose chunk is below 128 KiB may hold up to 16 bytes past its charge (`CHUNK_SLACK`), at most half again of those allocations' charges (core 4.1, *one meter per input*; `limits/mod.rs`). So in glibc's own bytes the worst case is `1.5 x I` plus the constants. The budgets are dominated by large mapped buffers (runs, pixels, PNG output), whose pages cover the slack, so the excess is a few percent in practice. The nightly reports it.
- **Allocator retention.** glibc raises its mmap threshold each time a mapped chunk is freed, after which large buffers come from arenas that do not return memory (`limits/mod.rs`; `tests/heap_rounding.rs`). `extract.md` E57: the binary calls `mallopt(M_MMAP_THRESHOLD, 128 KiB)` and sets `M_ARENA_MAX` to `threads + 2` at start. Fragmentation of small chunks inside arenas is not bounded by the core's rules (A7). The nightly measures peak RSS against the bound.
- **Page cache.** Spool writes and file reads fill the kernel's page cache: reclaimable and outside the heap, but counted by a memory cgroup (A7). The spool writer drops the pages of outputs, sorted runs and row files after closing them (`posix_fadvise(DONTNEED)`), and keeps staged bundles cached, since attempts read them again.

### 5.5 Arithmetic on the corpus

**A block decode's transient** (`limits/vec.rs`, `budget/mod.rs`, `codec/lzma.rs`).
- `DecodeBuf` doubles its capacity and, while it grows, charges the new room beside the old. `into_bytes` charges the exact size beside the old capacity before it moves the bytes.
- For an output of `B` bytes with final capacity `c_n < 2B`, the charge is `c_(n-1) + c_n` during the last doubling and `B + c_n` at `into_bytes`. A last doubling means `B > c_(n-1)`, so the peak is `B + c_n < 3B`, at `into_bytes`.
- LZMA adds its dictionary, `min(clamp(declared, 4 KiB, 8 MiB), named)` plus 576 B, and 33,040 B of state.

For the corpus's largest block, `B` = 69.4 MB = 66.2 MiB and `c_n` = 128 MiB, so the transient peaks at 66.2 + 128 + 8 MiB + 32 KiB = 202.2 MiB = 212.1 MB. That input's measured peak is 212 MB. The model leaves nothing for its file bytes and values at that instant, so which meter and which instant the measurement took is not known (A5). The fix in progress, which sizes the room to the declared size, brings the transient to about `B + 8 MiB`.

**One attempt's Input peak** is `P_X = s_X + runs_X + transient_X + meta_X + values_X + export_X`:
- `s_X`, X's file size; `runs_X`, at most the declared decoded size plus one more copy of each block a span straddles (core 3.6); `transient_X < 3 B_X + 8 MiB + 33 KiB` before the fix; `values_X`, the objects being exported, one at a time;
- `export_X` for one sheet group of `L` x `H` pixels: the texture's source data (up to `4 LH x 4/3` with mips, plus its alpha texture's), RGBA and alpha sheets of `4 LH` each, the image being encoded (`4 LH`), PNG output of at most `4 LH + H` plus deflate's overhead, filter rows of `5 (4 L + 1)`, and about 262 KiB of deflate state. So `export_X` is about `24 LH` for sprites and atlases and `13 LH` for a plain texture: 2048 x 2048 takes 96 MiB for sprites, 4096 x 4096 takes 384 MiB. A sprite's mesh arrays add a few bytes per vertex and index.

**One attempt's Dependency peak** is the private registry (about 300 B per CAB of the view, A4) plus the opens its reads keep within `D_X`, plus one decode's transient. On the corpus no view's dependencies decode to more than 89.4 MB, so at `D` = 256 MiB no attempt evicts.

### 5.6 Sizing rules, and an example

Each rule is a recommendation the manifest reports against, not a validation.

| goal | rule |
|---|---|
| every input that fits at all completes | `M >= max P_X` (E40's input peak) and `D >= max` dependency peak |
| full parallelism | `I >= threads x (M + D)` |
| the spool fits | `spool.size_bytes >= the staged bundles + all candidates + records + threads x the largest attempt's outputs + the sort's runs + the member lists + the largest archive's central directory + the row files` |

**Example for the corpus:** `threads` 8, `download_concurrency` 8, `M` = `D` = 256 MiB, `I` = 4 GiB.
- The constants come to about 140 MiB: 69 MiB of thread stacks (37 MiB for workers and the four named threads, 32 MiB for the two blocking pools), 5 MiB of private Environments at `max_view_bundles` 1,024, 32 MiB for 8 transfers on each of two clients, about 4 MiB of spec and controller state for a full build's 12,000 bundles, 8 MiB for the finisher, and about 20 MiB of base.
- So the bound is about 4.14 GiB in the charge unit, and about 6.14 GiB under glibc's worst-case slack.
- The corpus's largest input peaks at 212 MB before the decode fix, inside `M`. An input whose view is itself alone reserves 256 MiB, so the 8 slots reserve between 2 and 4 GiB.

### 5.7 Disk

`spool.size_bytes` bounds the sum of the staged bundles, every committed candidate and its records until phase 7, the partial outputs of the attempts in flight, phase 6's sorted runs and member lists, archives' central directories while they stream, and the manifest's row files. A free-space check against `spool.reserve_bytes` runs before each claim. A refused claim stops the run (2.7).

### 5.8 How it is checked

- `every_attempt_returns_its_charges` (3.1), and `the_slots_in_flight_never_exceed_the_inflight_bound`.
- X12 in `extract.md`'s gate: the core's counting allocator (`unityrs_testkit`) over the engine's export paths. The heap never holds more than the charges plus 5.3's constants, as `tests/memory_model.rs` checks for the core.
- X13: each testkit input run with `M` at its peak less one byte, then at its peak, where its peak is the largest over its meter and every dependency meter of its run, through the engine's library API below validation. It is refused the same way every time, then gives the records of an unbounded run.
- `a_stop_in_each_phase_stays_within_the_bound`: the counting allocator over the finisher's streaming.
- `rss_tracks_live_heap_after_large_frees`, with and without E57's settings.
- The nightly writes `VmHWM` into the manifest and checks it against 5.1's bound for its spec.

---

## 6. Requests to the core

Each request goes through the core's integration rule. None changes a core 4.1 property, and `EnvOptions::unitypy()` is untouched by all of them.

| request | change | property it adds, and its test | what the engine does until it lands |
|---|---|---|---|
| **CR1, wanted** | `load_dependency` publishes nothing when it fails: a registration refused after some of its keys were inserted (`env/load.rs`, `try_load_dep`; `env/registry/register.rs`) removes them before it returns the error | a failed load leaves the registry as it was. Test: a bundle whose registration is refused at its k-th entry, for every k, after another bundle registered a same-named CAB; every lookup after it answers as before the load | the attempt fails X on any failed load of its view (2.3, rule 1) and makes no read through that Environment after it, so the partial slots are never read. Granted, an attempt could skip a failed member that its reads never reach; D does not need it |
| **CR2, wanted** | `Error::refusal(&self) -> Option<Refusal>`, with the same on `DerefError` and `ParseError`: `Refusal::{Budget(Reservation), Limit(LimitKind, MeterKind), Allocator(Reservation), Io}`, where `MeterKind` says whether the refused meter was a dependency's. The allocator's refusal (`limits/meter.rs`, `refusal`) gets its own variant, and `grow_counted`'s smaller retry and `into_bytes`'s keep apply only to meter and budget refusals, so an allocator refusal always surfaces | one public classification of every error the engine must route, stable across new variants, with no silent allocator path. Test: every wrapper path the core can build (`Error` → `DerefError` → `ParseError` → `ReadError` → `TreeError` → `ChargeError`) classifies as its innermost cause; an allocator that refuses the k-th allocation, for every k of a decode, surfaces as `Allocator` | 2.7's `classify` over the public enums, with unknown variants stopping the run (exit 70); the refusal count tells the allocator's visible refusals apart; preflight's checks keep the silent ones out of reach (A13) |
| **CR3, wanted** | `EnvOptions::own_bundle_first`, true in `batch()` and false in `unitypy()`: a resource read's candidates and the AssetBundle container are looked up first in `from`'s own container chain | Q12's rule in the core, where its candidate order and slice rule live. Needs a `PARITY.md` section 19.3 row and Q12's tests in both load modes | the engine's copy (2.6, step 8) and its differential test (3.9) |
| **CR4, wanted** | a `Source::Path` registration that reads only the bundle's header, blocks-info and directory, and reads the rest of the file when a data block is first reached | registration costs the directory, not the file. Test: registering an N-byte bundle charges at most its header, blocks-info and directory, and a later read gives the same bytes as a whole-file load | every registration reads its file whole (2.2's cost) |
| **CR5, text only** | core 3.15 and its constants' table: the provider cache's bound is per `ProviderChain`, shared by every context that clones its `Arc` | none (a correction) | 5.3 says why it is not a term in version 1 |
| **CR6, not needed for version 1** | `BundleFile::predecode(data, ctx, meter, reader) -> Result<Arc<Decoded>, Error>`: decodes every block once, in order, and keeps per block its length or the input-determined error its decode gave (a refusal fails the call and is not kept); returns an immutable `Send + Sync` value (header, blocks-info, directory, per-block outcomes, the joined data up to the first failed block as one `Bytes`, charged to `meter`). And `Source::Decoded { name, decoded, reader }` for `load_dependency`: its open, first or after eviction, charges only its own tables and parses, serves each block as a zero-copy slice, returns a kept block error where a lazy open's read would, and counts no decode work for the shared data | the planned shared cache (section 8). Tests: *same bytes, same errors* (every testkit and fuzz-corpus bundle reads the same bytes and the same input-determined error through a `Decoded` open as through a lazy `Path` open with unbounded meters); *charges nothing for the data*; *immutable* | nothing: version 1 seals every attempt |

**Withdrawn with design round 1's shared cache:** `Environment::evict_dependencies`, a public `with_room`, `deref_held`, and the `DecodeWork` request. A sealed attempt's Environment is dropped whole, and no outcome waits for a retry.

---

## 7. Assumed (not checked)

- **A1.** Sizes not measured: a transfer's connection and TLS state is at most 2 MiB (`c_net`); a parsed spec at most 4 times its JSON; a registered bundle's uncharged heap at most 512 B. Test: the counting allocator on a 100 MiB download, a spec at the cap, and a view of `max_view_bundles` bundles.
- **A4.** A registry entry costs about 300 B per CAB, from core 3.13's list, not measured. It sizes `D`; the budget enforces whatever it is.
- **A5.** Measured on the corpus (2.2's cost): view sizes, how many views hold each dependency, the decoded bytes once per run and under sealing, and the distinct dependencies' decoded total. Not measured: the total candidate bytes of a build, which sizes the spool; how much of 5.5's 212 MB peak is file bytes and values.
- **A7.** With E57's settings, glibc's arena retention keeps peak RSS within a few percent of the live heap. The core's rules do not bound fragmentation, and a memory cgroup also counts the spool's page cache. The nightly measures both.
- **A8.** Version 1 configures no typetree provider: the spec has no field for one, and the base context's chain is empty. The corpus's typetrees are embedded; whether every NGUI atlas MonoBehaviour's is, is checked by the nightly (its `no_typetree` count).
- **A9.** Staging is a small part of a run on the corpus, so finishing it before extraction costs little (2.1).
- **A10.** The MonoBehaviour script hook (`Environment::object_tree`) dereferences `m_Script` inside the core. It matters only for stripped MonoBehaviours with a script provider, which version 1 does not configure.
- **A11.** The spool and the output directory are local filesystems whose calls complete, and nothing but the engine writes the spool while it holds the spool's lock. A filesystem that stops answering can hold a thread in a call Linux cannot interrupt, past `T_hard` (2.11).
- **A12.** The core keeps no state that one thread's call leaves for another's, except the budget and the provider chain. Its non-test thread-locals: in `limits/meter.rs`, the pool-refusal count (read as a difference) and the grant frames (each lazy computation opens and closes its own); in `env/registry/dependency.rs`, `RETRYING`, `RETRIED` and `RUNNING` (restored or cleared by their guards when the outermost `with_room` returns, unwinding included); in `env/registry/mod.rs`, `LOAD_KEYS_HERE` (read as a difference, `env/container_index.rs`). So what an earlier attempt on the same worker left in them changes nothing. Tests: `an_inputs_records_do_not_depend_on_what_runs_beside_it` at `threads` 1, where every attempt follows another on one thread; `the_core_has_no_unlisted_thread_local` (a source scan for `thread_local!` outside `#[cfg(test)]` against this list).
- **A13.** Under preflight's resource-limit and overcommit checks, the allocator does not refuse an allocation within 5.1's bound. When it does (a cgroup that refuses mappings, `vm.max_map_count` reached), a visible refusal stops the run (2.7), but one that `grow_counted` or `into_bytes` absorbs changes a later record without a trace. CR2 removes the assumption.
- **A14.** An object store finishes a request whose body it has received within the sink's `timeout_ms`, and the clocks of the hosts that run attempts of one `run_id` agree within 30 s. The time fence (`BATCH_SPEC.md` 7.2) rests on both.

(A2, A3 and A6 were design round 1's assumptions about the shared cache, and went with it.)

---

## 8. Open, for the review rounds

1. **Sealing.** Its cost is measured (2.2, A5). A review should look for any state an attempt reads that another attempt can write.
2. **Staging** copies every local bundle into the spool (2.5). A review should check that nothing reads a source after its copy.
3. **Overlapping staging with decoding** is given up (2.1). A later version may start an attempt once its view has staged.

**Planned optimization: a shared pre-decoded dependency cache.** Not in version 1. It keeps D and B in this form:
1. **Selection per bundle.** Whether a dependency Y is pre-decoded is a function of Y's bytes and the spec alone (for example, a spec field `dependency_mode: "sealed" | "shared"`, and in `"shared"` every bundle in some view that is not only an input, whose blocks all decode within the pre-decode meter's limits). Never "what fits after the others": that makes Y's path, and so its readers' records, depend on which other bundles the run holds.
2. **All or stop.** When the selected set's decoded total passes the pool, the run stops before extraction (exit 5). It never falls back per bundle.
3. **The pool inside `I`.** Pre-decode runs sealed, one bundle per meter and budget, and the pool is charged to a run-level budget resident until extraction ends. Validation becomes `P + M + D' <= I`, where `P` is the pool and `D'` an attempt's Dependency budget for parse state only. Attempts read the pool's `Bytes` slices without charge, so their leak check still reads zero.
4. **Equality across modes is not promised.** A `"shared"` and a `"sealed"` run may differ under tight budgets; D holds within each mode, since the mode is in the spec.

It needs CR6 (section 6). A full build's distinct dependencies decode to about 350 MB (2.2), so a pool of 1 GiB holds them. It saves the extra decoding of 2.2's cost, not the per-attempt parse.

**Follow-ups** (earlier rounds' LOW findings not fixed in this text):
- The core's evict-and-retry for direct reads on a dependency's handle (3.12), which would let a read fit that a full cache refuses today.
- `{container}` in key templates (`extract.md` E27).
- A spec field for the manifest's location (`BATCH_SPEC.md` 7.1).
- Pinning the PNG writer's zlib to the oracle's version, and `PARITY.md`'s IDAT chunk size, `max(65536, 4 x width)` (`extract.md` E19).
- An optional prune of keys under the output root that the manifest does not list.
- Check the NGUI atlas's material field name against NGUI's serialized layout (`material`, or `mMat` in some versions) before the cutter is written (`BATCH_SPEC.md` section 5); name a component atlas (`UIAtlas`) by its GameObject, so it can join `script_classes`' default.
- Preflight could check `vm.max_map_count` against `2 x I / 128 KiB` (A13).
- A rule that a `_MainTex` outside the input's own files gives no name would remove version 1's only cross-bundle decode (2.2). It would change records only where a crossed texture shares a name with one of the input's own textures. The owner's call.

## 9. Before it is built

- This plan's review rounds, run until a round finds no MEDIUM or HIGH problem, with section 8 settled.
- CR1 to CR5 answered. None is needed for correctness; each has its interim. CR6 waits for the planned cache.
- `extract.md`'s gate (its section 5) agreed. It holds the engine's tests named here: D (2.10), budgets and charging, the deadlock guard and scans (section 4), the stop path (3.15), and the memory rows (5.8).
