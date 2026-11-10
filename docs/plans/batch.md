# P1b plan: unityrs-batch, the engine (draft)

**Status: draft, design round 8 (2026-11-10), not reviewed.** P1b is 2D batch extraction (`docs/DESIGN.md` section 5; called P2 until 2026-11-09, when the phase table gave the 2D work its own phase). This file is the engine's plan: staging, admission, the attempt, the hand-off to sinks, the stop path and refusal. The rest of the phase is `docs/plans/extract.md`, whose requirements rely on sections 1 and 2 here. The exact contract (the spec's fields, keys, identity and order, the spool, the manifest, the zip bytes, the exit codes) is `docs/BATCH_SPEC.md`.
- Until P1's third review round, `docs/plans/core.md` section 4.1 described a batch flow. Each review round found new problems in its admission and eviction, so the flow left the core plan for this file. Section 3 lists every problem found in it.
- Design round 1 gave one dependency cache shared by every job; its review showed outcomes then depended on what ran beside an input (3.14). Design round 2 made every attempt **sealed** (2.2). Design round 3 folds round 2's review: one stop path that reaches every exit code (2.11), a spool that owns only what it created, bundles staged into the spool so their bytes are pinned (2.5), and no dependency discovery (2.3). Design round 4 folds round 3's review: a stated contract with the caller, and a verification pass, replace round 3's time fence on output writes (2.9); whoever requests a stop sets it (2.11); every input registers its own bundle (2.2); keys are planned before export (2.6). Design round 5 folds round 4's review: one commit boundary for the stop and the finish (2.9, 2.11), head fencing and per-try classification stated in `object_store`'s terms (2.9), an allocator that counts its own refusals (2.7), and registration's peek under the core's own rule until CR7 (2.6). Design round 6 folds round 5's review: the final write's read-back and `syncfs` before the head (2.11), zlib allocating through the counting allocator (2.6, 2.7), the store client's own settings and the finisher's tries after a stop (2.9, 2.11), and phase 6's spool share stated in `S` (5.6). Design round 7 folds round 6's review: export work's units and the spooled parts stated once, with a block ratio rounded up (2.6 step 10), a 69 at the `running` write handed to the finisher (2.11), writeback started as each output commits and the `syncfs` that stays unbounded (2.9, 2.11), and both HTTP clients' settings (2.5, 2.9). Design round 8 folds round 7's review: X13's need measured by the core's refusal counts (CR8), a directory transfer that stops writing at a stop and the head's `fsync`s that stay unbounded (2.11), one precedence for 7 (2.11), and the open rules of a directory sink (2.1, 2.9). The rounds' dispositions are recorded outside the repository.
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

**Each input decodes its own dependencies.** Each attempt opens the bundles of its view in a dependency cache of its own, so a dependency that N inputs read is opened up to N times and decoded up to N times in what they reach (2.2, its cost). Within one attempt, each bundle of its view is opened at most once per residency in that attempt's cache, and the manifest counts its evictions. DESIGN section 4 says this is version 1's behaviour, and that a shared pre-decoded cache, which keeps D, is planned (section 8).

**Two properties the design keeps.** Every mechanism in section 2 serves one of them or a requirement it names.
- **D, determinism.** An input's records are a function of its staged bytes, the staged bytes of the bundles of its view (2.3), the spec's entries for those bundles, and the spec's other fields. Its records are its outputs and their bytes, its skips, and its failure. They do not depend on:
  - `threads`, or the order inputs run or finish in;
  - what else is in flight;
  - which bundles outside its view failed, or which exist.

  2.10 shows why, against every counterexample earlier reviews built, and names the test. D has three limits, the third until CR2 lands:
  - a failure of the run as a whole stops it instead of deciding a record: a host failure (an I/O error, an allocation the allocator refuses), the spool's cap, or an engine bug (2.7, 2.11);
  - a bundle that could not be staged (2.5: a download that failed after its retries, a local file that is missing) fails the inputs whose view holds it, and that record depends on the network or the host, not on bytes. Its reasons form the `transport` class (`BATCH_SPEC.md` 9.3), which a caller retries;
  - liblzma allocates through C's `malloc`, which the engine's counting allocator (2.7) does not see; the core reports its refusal as a `Budget` refusal of the meter it decodes for (`codec/lzma.rs`, `failure`). Two such refusals go unseen and can decide a record by the host's memory (A13): (a) one on a dependency's meter, which `with_room` takes for a pool refusal, evicts for and retries; (b) one on the input's meter in a core call in which a budget refusal was already absorbed (`grow_counted`, `into_bytes`), so the budget's refusal count moved and it reads as `budget(input)`.
- **B, the memory bound.** The process's live heap is at most `max_inflight_uncompressed_bytes` plus constants fixed by the spec (section 5). Every byte that grows with an input or an output is charged to a budget before it is allocated.

---

## 2. The design

### 2.1 Phases

1. **Validate** the spec (`BATCH_SPEC.md` section 1). No thread starts and nothing is written. A bad spec writes the `exit` line and exits 78.
2. **Preflight** (E63), in this order. Set the allocator (E57); start the lifecycle, progress and finisher threads (2.11); check the deadline (`BATCH_SPEC.md` 1.1); read the credentials and each header variable once (`BATCH_SPEC.md` 1.2), and build the controller's and the finisher's store clients (object store, 2.9); start the spool (`BATCH_SPEC.md` 6.2: dedication, its lock, old journals, cleanup, free space); check resource limits, the overcommit mode, the map count and the host's memory (2.7); check the kernel's release, probe `openat2` on the output root, open `_unityrs`, take the root's lock and open `_unityrs/<run_id>` (directory sink, 2.9, `extract.md` E62), or prove the bucket exists (object store); read the stored head (`BATCH_SPEC.md` 7.2), which fences this attempt. A failure exits 69, a stored head that supersedes this attempt exits 7.
3. **Manifest `running`** (E38), fenced. No output is written before it.
4. **Stage** every bundle into the spool (2.5), at most `download_concurrency` at once.
5. **Extract** every input whose view staged whole (2.4 to 2.8).
6. **Winners** (2.9): a pure function of the committed records.
7. **Publish** (2.9), from the spool, at most `download_concurrency` transfers at once; then, in an object store, verify what was published.
8. **Finish** (2.11): the finisher writes the final manifest and the `exit` line, then exits.

Once preflight has started its threads, every end goes through the finisher (2.11): a normal end, a stop, and a 69 or 7 in preflight or at the `running` write, or a 74 at a directory sink's `running` write, for which it writes no head (`BATCH_SPEC.md` 7.4). A stop (a signal, the deadline less the grace, the parent's death, stdin's EOF, or a run-level failure) can come in any phase from 2 on.

### 2.2 Sealed attempts

An **attempt** is one run of one input's extraction. Each attempt is sealed: everything that holds memory or counts work during it belongs to it alone.
- **Its budget.** The attempt has one `MemoryBudget` object of its own, built by the engine from atomics. It never blocks, never calls the core and takes no lock (the budget contract, core 4.1; `budget/mod.rs`). It counts its own refusals (2.7). It routes each reservation by its `Reservation`, with one `PermitHome` per `Reservation`, since a release carries no reservation and permits merge by home:

  | `Reservation` | limit | what books with it |
  |---|---|---|
  | `Input` | `M` | the attempt's meter `M_X`: its input's bytes, runs, entries, values, and the engine's buffers (2.6) |
  | `Dependency` | `D` | every meter of the attempt's private Environment: its opens, runs, entries, registry slots and keys |
  | `Provider` | refused | version 1 configures no typetree provider (A8), so nothing books here |

  Every attempt registers its own bundle in its private Environment (2.6 step 4), so a pointer between two containers of X's bundle resolves as UnityPy resolves it, and every attempt's Dependency limit is `D`.
- **Its Environment.** The attempt builds a private `Environment` with `EnvOptions::batch(ctx_X)`, `dep_cache_bytes` set to `D`, and registers its view in it (2.6). No other attempt reads through it. It is dropped when the attempt returns.
- **Its context.** `ctx_X` is `FileContext::unitypy()` (`options.rs`: an empty provider chain, stateless codecs, the AssetBundle read at load) with the attempt's budget and the limits below.
- **Limits.** The engine sets, for every meter of the attempt: `max_input_memory` = `M`, `max_input_bytes` = `M`, and `max_decode_work` = `4 M`, the core's default ratio. The other fields keep `Limits::default()` (`limits/mod.rs`).
- **Admission by reservation.** Every attempt's **slot** is `S = M + D`. The controller runs at most `min(threads, floor(I / S))` attempts at once (2.4). The budget's limits are its slot, so the attempts in flight together can never hold more than `I`, and no attempt's reservation is ever refused for what another holds.
- **The rest of the run.** Phases 6 and 7 run when no attempt does, under one **phase budget** of limit `I` (2.9).
- **Leak check.** When the attempt's function returns, every local of it is dropped: the bundle, the private Environment, every file, value and pin. Its worker then reads the budget's `Input` and `Dependency` use. Both must be zero. A non-zero use is reported as a run failure (2.4), and the slot stays reserved.
- **Validation** (`BATCH_SPEC.md` 1.1) refuses a spec unless `M <= D`, `M + D <= I` and `I >= 64 MiB`. The first lets one dependency open at its cap fit the attempt's Dependency budget, its view's registry aside (5.6 sizes for that). The second lets one slot fit. The third gives phases 6 and 7 their working set.

**Cost.** This is what sealing pays for D.
- **Memory.** Each slot is reserved whole for the attempt's life, used or not. Parallelism is the largest number of slots that fit `I`, at most `threads`. For the corpus example (5.6), 8 slots of `M` = `D` = 256 MiB need `I` = 4 GiB.
- **Throughput.** No dependency is shared between attempts. An attempt reads every other bundle of its view from the spool, whole (`load_dependency` reads a `Source::Path` whole, `env/mod.rs`, `read_path`), and decodes again every block it reaches. Measured on the corpus (A5):
  - Most inputs have no dependency: the mean view is about 2 bundles, the largest 22. Dependencies concentrate on a few bundles, a handful of which sit in hundreds to thousands of views.
  - Version 1 crosses a bundle on one read only: the NGUI cutter's step 1, the name of the material's `_MainTex` (`BATCH_SPEC.md` section 5). Sprite sheets and `m_Script` stay inside their bundle on the corpus.
  - At that reach, a full build's extra decoding costs about 2.5 to 3.3 times the CPU of a run that decodes each bundle once: about +30 s of wall time at 8 workers. At worst, with every view member reached, it is 9 to 24 times. These figures predate registration's peek (2.6 step 4), which decodes, in every attempt, the blocks before and holding each top-level entry's first bytes; they are measured again with it before the build (A5, section 8).
  - All of a build's distinct dependencies decode to about 350 MB, which is what the planned shared cache would hold (section 8).
  - The view members' file reads (27 to 47 GB a full build at worst) come from the page cache, and 2.5 keeps staged files cached.
- **What it removes.** The shared dependency cache, the Shared and Alone attempts, the Alone retry, the registration phase before extraction, the key table and flagging.

### 2.3 Views

**Views.** `view(X)` (`BATCH_SPEC.md` 4.2) is X and the closure of `deps` through the spec's graph. Every id in a `deps` list names a bundle the spec declares (validation, `BATCH_SPEC.md` 1.1). `extract.md` E13's "needs" means "is in the view".

**No discovery.** A job declares its complete graph. The engine reads X's AssetBundle object's `m_Dependencies` only to report: a name that is not the id of a bundle of `view(X)` is a diagnostic row (`undeclared_dependency`: the input and the name, `BATCH_SPEC.md` 7.3) and a run count. It is never fetched or opened, and a read that reaches a CAB only it would hold is the object-level skip of rule 2.

The view rules:
1. **A failed member fails X.** A bundle of `view(X)` that could not be staged (2.5), or whose load in the attempt is refused, fails on its bytes, or gives a file that is not a bundle, fails X with `dependency_failed(<id>, <reason>)` (3.10), where the inner reason is the member's own (`not_a_bundle` for the last). The exit code follows the inner reason (`BATCH_SPEC.md` 9.2). A member that failed at staging fails X before it is dispatched. X's own staging failure gives X's own reason; otherwise the record names the failed member with the least id in bytewise order, the one registration meets first. An I/O error reading a member's staged file is not a member's failure: it is a host failure and stops the run (2.7).
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
- `Stopped`: the attempt saw the stop flag (2.6 step 11) and returned with nothing to commit;
- `RunFailure(reason)`: a run-level failure (2.7): a host I/O error, an allocator refusal (seen by the counting allocator or by the refusal count), a refused spool claim, `ContainerDropped`, an error `classify` does not know, a panic outside a decoder's `catch_unwind` (caught by the worker's), or a leak found by the leak check.

The message is fixed-size: the input, its outcome and reason code, its counters and its peaks. Its records are in the spool, never in the message (2.8).

**The loop.** State: `ready` (sorted), `in_flight` (a count), and `slots` = `min(threads, floor(I / S))`.

```
loop:
  if stopped(): hand over to the finisher (2.11) and park
  while in_flight < slots and ready is not empty:
      x = ready.pop(); dispatch(x); in_flight += 1
  if in_flight == 0 and ready is empty: go to phase 6
  wait for one event: an end message | a 1 s tick | a stop's wake
  on end message (input, outcome):
      in_flight -= 1
      match outcome:
          Done:            commit the attempt's directory (2.8)
          Failed(r):       unless stopped(): delete the directory (2.11's checks); commit its failure record (E60)
          Stopped:         delete nothing, commit nothing (the input is not_run; the next start clears the spool)
          RunFailure(r):   request_stop(r) (2.11); hand over and park (a leaked slot is never freed)
```

- A failure record is committed as a records file of one line, where a `Done` attempt's records file is (`done/<input>/records.ndjson`); it counts in the input's spooled part as a records file of one flush, so at least 4 KiB (2.6 step 10).
- `stopped()` reads the stop flag, which `request_stop` sets before it returns (2.11). So the controller's own request is a stop at once: it hands over in the same turn, and no later step of its loop or of phase 6 or 7 runs.
- Every attempt returns (its work is bounded, 2.6 steps 7 and 10), and one slot always fits `I`, so the queue empties.
- The dispatch channel and the end-message channel each have capacity `threads`. At most `threads` attempts run, so no send blocks.
- The controller's own loop runs inside `catch_unwind`; its handler records the stop `internal`, hands over with the phase it reached, and parks.

### 2.5 Staging

Every bundle the spec declares, input or dependency, is staged once, in phase 4, into the spool file `in/i<n>` or `in/d<n>` (`BATCH_SPEC.md` 6.1). Every later read of the bundle, by any attempt, reads that file, never the source.
- **A URL** is downloaded by streaming to the file (E7), with `Accept-Encoding: identity`, so every byte counted is a byte of the bundle. Its `reqwest` client sets exactly: `connect_timeout(http.connect_timeout_ms)`, a `Policy::custom` redirect rule applying `BATCH_SPEC.md` 1.4's `redirects`, no automatic decompression, `pool_max_idle_per_host(0)`, `http1_only()`, `no_proxy()` and the user agent `unityrs/<version>`. It sets no whole-request timeout: the idle timeout and the rate floor are the engine's own (`BATCH_SPEC.md` 1.4).
- **A local path** is opened `O_RDONLY | O_NONBLOCK`, so a FIFO or a device cannot hold the open; `fstat` must show a regular file, or the bundle fails `source_not_regular`; then `O_NONBLOCK` is cleared and the file is copied to the spool file through a 64 KiB buffer, on the runtime's blocking pool, checking the stop flag per buffer. It copies exactly the size `fstat` gave at the open. After the copy, `fstat`'s size, modification time, change time and inode must equal those at the open, or the bundle fails `source_changed` (a transport reason: the source was being written). A local source is on a local filesystem (A11).
- **Size.** A bundle is at most `M` bytes: every meter's `max_input_bytes` is `M`, and the core reads a file whole. The checks run in this order, before any claim, and the first that fails names the reason: a stated size (`size`, the local `fstat` size) above `M` (`limit(input_bytes, M)`: the record its attempt would give); a `Content-Length` other than a given `size` (`integrity_mismatch`); a `Content-Length` above `M` (`limit`). While it streams, a body that passes a given `size` fails it (`integrity_mismatch`), and one that passes `M` too (`limit`).
- **While it streams**, the engine counts its bytes and computes its SHA-256. At the end, a given `size` or `sha256` that does not match fails the bundle (`integrity_mismatch`). The digest is recorded for the manifest's `bundles` rows.
- **Then** the file is made read-only (mode 0400), and nothing writes it again. The spool is the engine's own directory under its lock (`BATCH_SPEC.md` 6.2), so the staged bytes are pinned for the run (A11).
- **Failures.** A download that failed after its retries fails the bundle (`download_failed`, or `redirect_refused`); a local path that does not exist or cannot be opened fails it (`source_missing`), one that is not a regular file fails it (`source_not_regular`), and one written during its copy fails it (`source_changed`). These are the transport reasons of D's second limit (section 1). An I/O error while reading an opened local file, or writing the spool, stops the run (2.7).
- **Space.** Each staged file's bytes are claimed from the spool counter before they are written (2.8's rule): a stated size at the start, a body without one per chunk. A try that fails truncates the file and returns its claim before the next try claims. A refused claim stops the run (`spool_full`); since no bundle claims more than `M`, that is the spool's sizing, not one bundle's doing.
- **Concurrency.** At most `download_concurrency` bundles stage at once. Extraction starts when every bundle has staged or failed.

**Why a copy.** A source file can change during a run without changing its size, modification time or inode, and the core reads a dependency's file again after each eviction. A hash of the first read cannot pin a reread the engine does not see. The copy pins every read, and makes a change of the source after its copy harmless. It costs the spool the build's bundle bytes (1.3 to 1.8 GB on the corpus) and one sequential copy, which a filesystem may serve from its page cache.

### 2.6 An attempt

1. **Budget and meter.** Build the attempt's budget (2.2) and `M_X = Meter::for_input(&limits, budget, Reservation::Input)`.
2. **Its own open.** `bytes = read_path(staged file, &limits, &M_X)` (`env/mod.rs`). Then `check_file_type_with_reader(&bytes, &limits, ReaderKind::Stream)` (`detect.rs`) and `parse_file_with_reader(name, bytes, ty, None, &M_X, &ctx_X, ReaderKind::Stream)` (`files/mod.rs`): the rules the core uses for a `Source::Path`. A file that is not a bundle fails X, `not_a_bundle`. A bundle is one file in version 1: no `.splitN` merge.
3. **The AssetBundle object.** Read X's `m_Dependencies` and container rows, and record each undeclared name (2.3), charged to `M_X`.
4. **The private Environment.** Register `view(X)` less X with `load_dependency(Source::Path(staged file))`, one at a time in bytewise order of id, then X last, always, with `load_dependency(Source::Bytes { name: its spool name, data: the bytes of step 2 })` (`env/load.rs`). X's slot shares its own handle's allocation, so X is never read twice, and the core's own-handle redirect serves X's resources through it (core 4.1). X's slot reads X's header, blocks-info and directory by the memory reader's rules, and X's own handle by the stream reader's (`open_source`, `env/registry/dependency.rs`). Of the two readers' differences (core 3.3), one alone lets both reads of a header succeed with different results: a header string with no NUL in its first 32,767 bytes, which the memory reader returns whole and the stream reader one byte shorter. Every other either raises on the stream side, where step 2 has already failed X, or leaves both readers past the end of the bytes, where they read the same nothing. So the engine compares the slot's header and directory with its own handle's, and a difference fails X (`container_error`). Add each returned `File`'s top-level directory names to the view's **name table**, charged to `M_X`, from which the `same_named_cab` rows are written once the view is registered (`BATCH_SPEC.md` 7.3). Then **reach its nested containers**: for each top-level entry, in directory order, read its first 21 bytes through that `File` (`entry_range`, the bytes the core's detection reads) and, when they are a bundle's, call `get_cab(entry name, &M_X)` on the private Environment. Under `Lazy` a nested container is a key whose slot answers nothing until a lookup reaches it, and reaching registers its whole subtree at the ranks an eager load gives (core's Lazy invariant, `env/registry/mod.rs`; `PARITY.md`, Lazy difference (b)). The reached mark lives on the slot, which outlives an eviction (`env/registry/slot.rs`), so a reopened bundle's nested CABs stay registered. Without this, a pointer by name alone from X's top level into a nested container, X's or a dependency's, would miss where UnityPy's eager load finds it. Then drop the `File`, so the private LRU holds the only strong handle.
   - **The peek's rule** is the core's own (`is_bundle_entry`, `env/registry/reach.rs`): a read error that is not transient means "not a container", and that entry's own reads fail later with it, as any read of it does; a transient one (a budget or meter refusal, an I/O error) is returned and goes as 2.7 classifies it: a refusal fails X with its budget reason, an I/O error is a host failure. So a corrupt block before a `.resS` fails no registration (3.10).
   - **Until CR7** the peek is the engine's read on the member's handle, not an Environment call: its refusal evicts nothing and is not retried, and the LRU counts the runs it caches only from the member's next use. So registration can refuse a view that an eviction would have fitted. The refusal is accepted: it is final, and it depends only on the view, its order and `D`, so D holds; 5.6's `D` rule counts the peeked blocks.
   - A load or a reach that fails, or a load that returns a file that is not a bundle, fails X (2.3, rule 1; for X's own, as an entry or container failure of X), and the attempt makes no read through that Environment after it (CR1 explains why). The stop flag is checked between registrations.
5. **Select and plan** (`BATCH_SPEC.md` sections 3 and 4.4). Walk the entries and objects in object order, filter by `class_id()` from the object table (no value read), and read typetrees only for the selected objects. Then plan, before any export: for every output the selection makes, in (kind, object order), expand its name and key, and decide `duplicate_name`, `duplicate_key` and every skip that needs no pixels. The plan table (one key per planned output) is charged to `M_X`. A planned output keeps its key even when its export later fails, so no export result moves a key.
6. **Export one sheet group at a time.**
   - Sprites are grouped by their sheet: the (bundle id, CAB name, path id) of the sheet texture and of its alpha texture, or none (`BATCH_SPEC.md` 4.4). For each group, decode the sheet (and its alpha texture), render every sprite of the group, then drop the sheet.
   - The atlases that cut one sheet are one group, cut one at a time (`BATCH_SPEC.md` section 5). The sheet's own texture output comes first, then each atlas in object order, its JSON before its crops, so a unit's key is fixed before its crops are recorded (`BATCH_SPEC.md` 4.5). Any other texture is its own group.
   - This replaces UnityPy's per-file texture cache with a cache of size one, so the export buffers are bounded by the largest sheet (5.5). The output is the same: the cache changes when a sheet is decoded, not what it decodes to.
7. **Reads into other bundles.** A pointer is dereferenced with `Environment::deref` on the private Environment, from the file that holds it (`env/deref.rs`). Before its next Environment call, the attempt takes the target's top-level `File` from its `Parent` and holds it while it uses the target (core 4.1, *a dropped container is an error, never a guess*). What is read is copied into the attempt's charged buffers, and every handle into another bundle is dropped when that read ends. After each Environment call the attempt reads the Environment's `stats().dependencies_evicted`; past `max_attempt_evictions` (`BATCH_SPEC.md` 1.1) it fails X, `limit(attempt_evictions, <value>)`. A reopen restarts its meter's work count (core 4.1). Every open of a member after its registration follows an eviction. The core does not mark which `Budget(Dependency)` refusal is a reopen's, so every such refusal after registration fails X (2.7), and a refused reopen is never tried again at the next access, uncounted. So the attempt makes at most `|view| + max_attempt_evictions` opens, each a read of at most `M` bytes and within `4 M` of decode work.
8. **Resources** (own bundle first, core Q12). Take the candidates of `resource_candidates(path)` (`env/resource.rs`) in UnityPy's order. For each, look in the containers that hold the reading file for an entry whose simplified name matches: its container's directory, then each enclosing container's, up to the top-level bundle (`BundleFile::directory`, `parent`), taking the last matching row of a repeated path, as UnityPy's registry keeps it. Read a match with `entry_range` through that handle (`files/bundle/mod.rs`). Only when no candidate is an own entry, read through `Environment::resource(from, path, offset, size)` on the private Environment, which covers a candidate in another bundle of the view (`env/resource_data.rs`). The engine's copy of the candidate rule is checked by `own_first_resources_equal_environment_resource_without_same_named_cabs` (3.9) until CR3 moves the rule into the core.
9. **The container** comes from X's own `SerializedFile::container()` (`files/serialized/mod.rs`), never from `Environment::container()`.
10. **Engine buffers** (`extract.md` E59). Every buffer whose size an input chooses is charged to `M_X` at `heap_cost` before it is allocated: RGBA and alpha sheets, the texture's source data, a sprite's mesh arrays (vertices, indices, triangles) and the affine path's buffers, sprite canvases, crops, masks, PNG rows and output, atlas JSON, the sprite-group table, the record buffer. zlib's deflate state, the sum of its allocations' `heap_cost` at Pillow's settings (about 398 KiB, 5.5), is charged as a constant before `deflateInit2`, and zlib allocates through `zalloc` and `zfree` functions that call Rust's global allocator, as flate2 1.1.10's C backend does (its `src/ffi/c.rs`), so the counting allocator sees its refusals (2.7). The charges go through `Meter::charge` or `DecodeBuf` (`limits/meter.rs`, `budget/mod.rs`): the core's rule of 3.15, applied to engine code. **Export work**, in bytes, sums every pixel byte the attempt writes (a decoded sheet, a canvas, a crop, at its mode's bytes per pixel), unrounded; every file it spools (each PNG, each atlas JSON, and its records file per flush of its 64 KiB buffer, flushed only when full and at the end, a failed input's one-line file included, 2.4), each at `max(4 KiB, its size rounded up to 4 KiB)`, whatever the spool's `st_blksize`; and each member record (2.8) at the largest size its `archive_members` row can take, its `cut` included, over the values of `included` and `reason`, which holds the archive's key. Past `max_export_work` (`BATCH_SPEC.md` 1.1) it fails X, `limit(export_work, <value>)`, before the claim. So the limit is a function of the bytes and the spec, and D holds across hosts. Its **spooled part** is the same sum without the pixel bytes, the input's share of `S` (5.6); the head's `spooled_parts` is the sum of every input's spooled part (`BATCH_SPEC.md` 7.3). The spool counter still claims in blocks of `b`, the spool directory's `st_blksize`, which preflight reads (2.8), so with `k_blk = max(1, ceil(b / 4 KiB))`, or 2 when `b` is below 4 KiB and does not divide it, an input's share of it is at most `max_export_work x k_blk`, whatever its number of objects or crops and however small each is, a bound known before the run (5.6). A spool that fills anyway is `spool_full`, a run-level stop (D's first limit).
11. **Copy-out** (2.8). The attempt checks the stop flag between registrations, planned objects, sheet groups and outputs; a stopped attempt returns at once, and its end message says `Stopped`.
12. **End.** The function returns, which drops everything above. The worker reads the budget (2.2), then sends the end message.

### 2.7 Outcomes

Every error is classified once, by one engine function, `classify` (CR2 would make it the core's). Its classes, and the reason each records (`BATCH_SPEC.md` 9.3):

| what failed | outcome |
|---|---|
| `Budget(Input)` or `Budget(Dependency)` refused by the attempt's budget; `Limit` of kind `InputMemory`, `InputBytes` or `DecodeWork`; the engine's `attempt_evictions` and `export_work` | the budget class (exit 5), recorded where it surfaced: an object-level skip inside an object's read or export; an input failure while opening X or an entry of X; `dependency_failed(<id>, <reason>)` while loading or reaching a view member. A `Budget(Dependency)` after registration fails X wherever it surfaces: the core does not say whether a refusal is a reopen's, and a refused reopen would otherwise be tried again at every access (2.6 step 7). So `budget(dependency)` is never an object's skip: a dependency read that does not fit in `D` once every other dependency is evicted fails the input, and its fix is a larger `D`. Reasons: `budget(input)`, `budget(dependency)`, `limit(<kind>, <configured value>)` |
| any other `Limit` (a depth, a count, `BlockOutput`, `Inflate`, a CAB or resource name) | a data reason (exit 3 or 4), at the same sites: `limit(<kind>, <configured value>)` |
| an error the bytes decide: `Detect`, `OutOfBounds`, `Malformed`, `Unsupported`, `Decompress`, `Version`, `TypeIndex`, `Tree`, `AssetBundle`, `Entry`, `NotFound`, a decoder panic (E18), E25's bytes | DESIGN section 4's failure model: a container or entry failure fails X (`container_error`, `entry_error`), an object's failure skips it (`decode_error`, `decoder_panic`). The detail field names the core's variant |
| `DerefError::NotFound` (no bundle of the view holds the CAB); `Error::NotFound(NotFound::Resource(_))` from the private Environment's resource read, after the own-chain lookup missed (2.6 step 8) | the object's skip, `outside_view(<cab>)`, a resource's argument being the stream path as read (E65) |
| `DerefError::{Null, FileIdPastEnd, FileIdBelowStart, RawTarget, MissingObject}` | the object's skip, `bad_pointer` |
| `DerefError::Load` | classified as its inner `Error` |
| `Error::Io`, or an I/O error of the engine's on the spool (but a full one, below) or a staged file | a run failure, `host_io` (exit 74) |
| an attempt during which the counting allocator's refusal count rose, whatever the attempt recorded; a `Budget` refusal during whose core call the attempt budget's refusal count did not move; zlib's `Z_MEM_ERROR` | the allocator's refusal: a run failure, `host_memory` (exit 74) |
| a spool claim refused, free space below the reserve, or `ENOSPC` or `EDQUOT` from a spool create or write | a run failure, `spool_full` (exit 5), its detail naming `ENOSPC` or `EDQUOT` when the filesystem refused: spool use depends on the order inputs ran in, so it decides no input |
| `DerefError::ContainerDropped`; an error `classify` does not know; any other zlib error; a panic outside a decoder | a run failure, `internal` (exit 70): the attempt holds every container it reads through (2.6, step 7), and an unknown variant must be noticed at once |

**Registration's peek is excepted** (2.6 step 4): it returns only a transient error (a budget refusal, `Limit(InputMemory)`, `Io`), classified as above; any other error there, `DecodeWork` and `Decompress` among them, means "not a container" and records nothing until a read of that entry meets it.

**Why every refusal is final at once.** The attempt runs on one thread in program order. Its budget and every meter that books with it serve that thread alone (2.2). So each charge meets the same state in every run, and each refusal, and each growth a refusal silently shrank (`limits/vec.rs`, `grow_counted`; `budget/mod.rs`, `DecodeBuf::into_bytes`), falls at the same point. No outcome waits for a retry.

**`classify` until CR2.** One module matches the public enums: every `Error` variant, every `DerefError` variant, `ParseError`'s `Read`, `Tree` and `Deref`, `ReadError`'s detail, `TreeError`, `ChargeError` and `LimitKind`. A `DerefError` that `Environment::deref` returns bare is classified as that same chain. Every `#[non_exhaustive]` enum's wildcard arm returns "unknown". Its test builds each wrapper path and checks the class.

**The allocator.** The core turns an allocator's refusal into a `Budget` error (`limits/meter.rs`, `refusal`; `limits/vec.rs`, `reserve_exact`), and three paths absorb one silently: `grow_counted` retries a smaller growth, `into_bytes` keeps a buffer it could not shrink (`limits/vec.rs`, `budget/mod.rs`), and `with_room` takes an allocator refusal on a dependency's meter for the pool's (`pool_refused`, `env/registry/dependency.rs`), evicts every other dependency and retries. So the engine counts refusals itself:
- **The counting allocator.** The binary's `#[global_allocator]` wraps `System` and counts each NULL it returns in a const-initialised thread-local `Cell<u64>`, which allocates nothing. zlib's allocations go through it too (2.6 step 10). A worker reads its count before and after each attempt; any increase makes the attempt's outcome `RunFailure(host_memory)`, whatever it recorded, absorbed refusals included. A second thread-local keeps the thread's live bytes, in `heap_cost` units (each allocation's `heap_cost` added, each free's subtracted), which the controller reads around parsing and validation to measure `C_spec` (5.3).
- **The refusal count.** A `Budget` error from a core call during which the attempt budget's own refusal count did not move is the allocator's too (it catches liblzma's refusal on the input's meter, which no Rust allocation sees, when no budget refusal was absorbed earlier in the call).
- **What neither sees** is liblzma's C `malloc`, the only C allocation outside the counting allocator, refused (a) on a dependency's meter, which `with_room` retries after eviction (`codec/lzma.rs`, `liblzmas_memory_error_on_a_dependency_counts_as_a_pool_refusal`), or (b) on the input's meter after `grow_counted` or `into_bytes` absorbed a budget refusal in the same core call, which moved the count: D's third limit (section 1, A13), until CR2.

Preflight keeps refusals rare within the bound: it refuses to start (exit 69) when `RLIMIT_AS` or `RLIMIT_DATA` is below 1.5 times 5.1's bound (glibc's slack, 5.4) plus 64 MiB per arena of `M_ARENA_MAX` (`rlimit_too_low`), when `vm.overcommit_memory` is 2 (`overcommit_strict`), when `vm.max_map_count` is below `I / 128 KiB + 8,192` (`map_count_too_low`: a mapping per buffer at the mmap threshold, and room for the rest), or when 5.1's bound passes the smaller of `MemTotal` and the cgroup's limit (`memory_too_small`): the process's cgroup is its `/proc/self/cgroup` line for v1's memory controller when one is present (a hybrid host), else its `0::` line; under v2, the smallest finite `memory.max` along the chain of that cgroup and its ancestors, as far up as the mount shows; under v1, `hierarchical_memory_limit` in its `memory.stat`, which counts its ancestors' limits as v2's chain does. A cgroup path names the directory `/proc/self/mountinfo` gives: the mount point of the hierarchy's mount whose root is the longest prefix of the path, plus the path less that root; a path beneath no mount's root has no visible limit in that hierarchy. An absent hierarchy, or v1's absent `memory.stat`, means no cgroup limit, and a v2 level without `memory.max` is skipped, the walk going on; a file present but unreadable exits 69 (`cgroup_unreadable`). It also raises `RLIMIT_NOFILE`'s soft limit to its hard limit and refuses one below `4 x threads + 4 x download_concurrency + 64` (`rlimit_too_low`): each attempt holds at most its records file, an output and a staged file, and each stage or transfer a socket and a file, on the controller's clients and the finisher's.

### 2.8 Copy-out, records and commit

- **Spool layout and lifecycle:** `BATCH_SPEC.md` section 6.
- **Each output:**
  1. It is encoded in charged memory, and its MD5, CRC-32 and visible hash (E20) are computed, and, for one over 16 MiB, its multipart ETag at 16 MiB parts (the ETag resume and verification expect, 2.9).
  2. Its size, rounded up to the spool directory's `st_blksize`, read once at preflight, is claimed from the spool counter (export work counts it in its own fixed unit, 2.6 step 10), an atomic try-add against `spool.size_bytes`, after a `statvfs` check against `spool.reserve_bytes`. Every spool write claims this way before it writes: staged bundles (2.5), outputs, record flushes, sorted runs, member lists, the outcomes files and row files.
  3. It is written to the attempt's directory, and its buffer is dropped.
- **Records.** Candidate records (key, unit key, input, `serialized_file` (`BATCH_SPEC.md` 7.3), kind, path id, MD5, CRC-32, visible hash, mode, size, multipart ETag or none, and `file`, its name within the attempt's directory), one member record per archive member path a candidate expands to (archive index, path, key, input; `BATCH_SPEC.md` 4.7), skip records (with `serialized_file`) and diagnostics are appended to the attempt's `records.ndjson` through a fixed 64 KiB buffer charged to `M_X`. A member path that breaks a rule of `BATCH_SPEC.md` 2.3 (`prefix_conflict` aside, which step 5 decides) is recorded with that rule and its path cut as below. A string a record takes from the bytes (a name, a CAB, a reason's argument) is cut to its first 1,024 bytes, at a code point boundary, with the cut marked (`BATCH_SPEC.md` 7.3). Records are appended in the attempt's record order (`BATCH_SPEC.md` 4.4), which the `skips` and `diagnostics` rows keep, so nothing sorts them. No record holds the input's position, the run id or a spool path, so its size is a function of the input's bytes and the spec; phase 6 builds each spool path when it joins (2.9).
- **The line bound.** A record holds at most four strings, each at most 1,024 bytes before escaping: an id (no control character), a key or member path (no control character, `too_long` past 1,024 bytes), or a cut string. JSON escaping writes a byte as at most 6 (`\u00XX`, or `�` for a byte that is not UTF-8, `BATCH_SPEC.md` 7.3), and the other fields take under 1 KiB. So every line is at most 32 KiB, whatever the number of archives.
- **Commit** (`extract.md` E60, all or nothing per input).
  - An attempt that ends `Done` has its directory renamed to `done/<input>/` by the controller.
  - An input that fails commits only its failure record (2.4): no candidate, so it competes for no key.
  - Deleting an attempt's directory returns its claimed bytes to the spool counter.

### 2.9 Winners and publish

**Winners** (E31 to E34, E61; `BATCH_SPEC.md` 4.5 to 4.7), in phase 6. Each step below is an external sort of spool records and one streaming pass over the result, so phase 6 holds no table that grows with keys, units, candidates or members.
- **The sort.** Runs whose records and index together take at most 32 MiB, sorted in memory and written to `winners/`; then merges of at most 16 runs, with a 1 MiB buffer per run and 1 MiB for the output. A line is at most 32 KiB (2.8). A merge deletes its input runs once it has read them, and a step's intermediate output is deleted once the steps that read it are done; the committed records and the winners file stay, since phase 7 and the finisher read them. Every pass and merge reads the stop flag at least once per 64 MiB it reads (2.11). Every phase-6 file is a projection or a join of committed records, each line at most twice the size export work counted for the record it comes from (2.6 step 10): step 5's join adds a candidate's spool file (its path relative to the spool root), size, CRC-32 and MD5 to a member record, which export work counts at its row's largest size.
- **`/`-first order** (`BATCH_SPEC.md` 4.6) compares two strings bytewise, except that at the first byte where they differ a `/` is less than any other byte; a string that is a prefix of the other is less. It is a comparison, not a mapping of `/` to a low byte, so it holds for any bytes a path holds. In it every key's descendants (the keys that start with it and `/`) follow it contiguously, so one look at the next line tells whether a key is an ancestor of another.
- **Step 1, units.** Sort every unit line by (unit key, input id). One pass decides each unit key's winning unit by E31 and E61, keeping only the best candidate so far: within a key the candidates come in input order, so rules 1 to 3 need no more.
- **Step 2, member keys.** Sort one line (member key, unit key) per key of every winning unit, its unit key included. One pass: of the units claiming one key, the first by unit key keeps it, and every other is withheld whole (`unit_conflict`). Every key a winning unit claims stays reserved, even when its unit is withheld, so the pass is not repeated (`BATCH_SPEC.md` 4.5).
- **Step 3, plain keys.** Merge the sorted plain candidates with step 2's reserved keys. A plain candidate for a reserved key loses (`unit`). Every other key is decided by E31 and E61, streaming as in step 1. The output is the set `P` of keys the decision would publish, in key order.
- **Step 4, prefix and archive conflicts** (`BATCH_SPEC.md` 4.6). Sort `P`'s keys and the archives' keys together in `/`-first order, an archive's key before an equal key of `P`. One pass refuses a key `K` of `P` when the next line starts with `K/` (`prefix_conflict`: an ancestor of a key of `P` or of an archive's), else when `K` equals the last archive key seen or starts with it and `/` (`archive_conflict`). One archive key seen is enough, since validation makes archive keys pairwise not ancestors. The refused keys are sorted back to bytewise order and merged with `P`. A unit with a key so refused is withheld whole: its unit keys are sorted, and a merge with `P` marks its other keys `unit_conflict`. The check is not repeated. What is left is the **winners file**, in bytewise key order.
- **Step 5, archive members** (`BATCH_SPEC.md` 4.7). Sort the member records by (key, input) and merge them with the winners file, keeping those of each winner's candidate. A path recorded with a rule of `BATCH_SPEC.md` 2.3 (`unwritable_path`) goes to the rows only and takes no part in what follows. Sort the rest by (archive, member path in `/`-first order, key). One pass keeps the first key for a path (`duplicate_member`) and refuses a path that the archive's next distinct path starts with, followed by `/` (`prefix_conflict`), and writes each archive's member list, in member order, with each member's spool file, size and CRC-32.
- **Rows.** Steps 1 to 4 leave each key's decision (its status, rule and flip, and its winner's input, MD5 and visible hash) in a decision file in key order. The `outputs` and `collisions` row files are one merge of the key-sorted candidates with it, one line per (key, candidate) for collisions; `archive_members` comes from step 5.
- With `publish_incomplete: false` and a failed input, phase 7 publishes nothing and runs neither resume nor verification: every winner, and every archive with a member, is `withheld` (`publish_incomplete`), on a stop in phase 7 as at a normal end, and the head's `verified` is null (`BATCH_SPEC.md` 7.3, 7.4).
- The decision is a pure function of the committed records, so it is the same whatever order the inputs finished in.
- **Its working set** is at most 40 MiB: a run of 32 MiB, or a merge's 17 MiB, beside at most 8 stream buffers of 1 MiB. `I >= 64 MiB` holds it, so a refusal of the phase budget in phase 6 marks a leak, a run failure `internal`.

**Publish**, in phase 7.
- **Resume** (E43). With `resume` set, the object store's LIST of the output prefix (keys in ascending UTF-8 byte order) is merged with the winners file, one LIST page in memory at a time. A winner whose key is listed with its expected ETag (its MD5; a file's over 16 MiB: the multipart ETag its record carries, 2.8; an archive's: its multipart ETag at 16 MiB parts, `BATCH_SPEC.md` section 8) is `present` and not PUT. An archive's expected ETag needs its bytes: resume computes it in a first pass over the archive's stream, without uploading. A listed location not above the one before it is skipped when its size is 0: `Path::parse` lists a folder marker `k/` as `k` (`src/path/mod.rs`), which no output can be, since none is empty. Any other LIST page out of order, or a LIST that fails, degrades to "PUT everything" with the warning `resume_list_failed`. In a directory, each winner's destination is stat'd, and hashed only when its size matches.
- **A LIST** goes through `PaginatedListStore::list_paginated` with `max_keys` 1000, the prefix as given, and each call first reserves 16 MiB from the phase budget: `object_store` reads a page's body whole and parses it (`src/aws/client.rs`), and a page of 1,000 keys of at most 1,024 bytes, each byte escaped to at most 6, with its other fields, is under 7 MB of body, which `collect_bytes` grows from capacity 0 when it is chunked (`src/util.rs`), beside its parsed keys and their `Path`s (about 1 MiB each): about 14.5 MiB at worst.
- **Transfers.** A transfer reserves its buffer from the phase budget once, at its start: `heap_cost(size)` for a single-part PUT (files up to 16 MiB), or one 16 MiB part for a multipart upload, whose parts go one at a time. It holds that reservation until it ends. A multipart transfer reserves its part-ID vector at its part count when it starts (`C_net`, 5.3), and drops its part buffer before `complete_multipart`, so the reservation covers the part list's copies and the request's XML that `object_store` builds (about 3 MB at 10,000 parts, `src/client/s3.rs`). The scheduler takes winners in key order, then archives in key order, and starts a transfer when a `download_concurrency` permit is free and its reservation is granted; otherwise it waits for a running transfer to end. A reservation refused with no transfer running is a run failure `internal`: every buffer is at most 16 MiB, below `I`. An archive over 10,000 parts (156.25 GiB) is `failed` (`archive_too_large`) without an upload: phase 6's member list gives its size.
- **Outcomes.** Phase 7 starts by claiming and sizing the spool files `winners/outcomes` (one byte per winner, at its index in the winners file) and `winners/archive_outcomes` (one 128-byte record per archive; at most 256, `BATCH_SPEC.md` 1.1), zero-filled (`BATCH_SPEC.md` 6.1, which lists the codes); a missing file reads as all `not_written`. Resume's merge writes `present` at a winner's index. A transfer writes `written` or one of the `failed` codes (with an archive's size, MD5 and ETag) with `pwrite`, in the same poll in which its last response arrives, so every outcome the controller saw is in the files when it hands over; a directory transfer writes its own, under the commit lock (below). A byte left 0 is `not_written`, a transfer a stop left in flight among them. Each index has one writer, and every write happens before the hand-over, so the finisher never reads a record in the middle of its write. Nothing else holds per-key state.
- **Verification** (object store). After the last transfer the controller LISTs the output prefix again and merges it with the winners file and the archives, as resume does. Every key marked `written` or `present` must be listed with the ETag this attempt expects, or its outcome becomes `mismatch` (status `failed`, reason `publish_mismatch`). A key whose ETag is not an MD5 (`etag_not_md5`) is compared by size, and the head counts it in `verified_by_size`, a weaker guarantee (`BATCH_SPEC.md` 7.2). A LIST that fails or comes out of order changes no outcome, and the head says `verified: false` with the warning `verify_list_failed`; one a stop interrupts says `verified: false`, with no warning. Every try of this attempt's PUT of a key carries the same bytes, so only another writer can cause a mismatch: an earlier attempt's late request, or a writer outside the run. What verification and the caller's duties guarantee together is `BATCH_SPEC.md` 7.2.
- **The directory sink** holds the root's lock (`BATCH_SPEC.md` 7.2) for the process's life, writes each file as a temp file named `.unityrs-tmp-<run_id>-<n>` beside its destination, opens paths beneath the root and within its mount (E62), and runs `syncfs` after the row files and before the final head (2.11), on the root lock's descriptor, held since preflight: one opened later would miss a writeback error another process's `syncfs` already saw. Each transfer starts its file's writeback with `sync_file_range(SYNC_FILE_RANGE_WRITE)` after its write, and after every 64 MiB of a streamed archive, so that `syncfs` waits mostly on I/O already under way. Preflight refuses it on a Linux kernel older than 5.8, read from `uname`'s release (`kernel_too_old`), whose `syncfs` reports no writeback error. A transfer's last step is its **commit**, made under the sink's **commit lock**: read the stop flag, and when it is clear rename the temp file over its destination and `pwrite` the outcome. At its hand-over the controller takes the commit lock, then sends the one-shot to the finisher (2.11), and keeps the lock, parked. So every rename has its outcome in the files the finisher reads, or never happens: the directory sink's commit boundary. No key segment may start with `.unityrs-tmp-` (`BATCH_SPEC.md` 2.3), so a leftover temp file is never taken for an output, and the caller may delete it (E62).
- **The zip format** (`BATCH_SPEC.md` section 8) streams each archive from its member list, with its central directory spooled while the members stream.
- **The object store** follows `extract.md` E37. The engine makes every try itself: each client's `RetryConfig` has `max_retries` 0, so each `object_store` call is one HTTP request, and the transfer retries a request up to the sink's `attempts` with `http`'s backoff (`BATCH_SPEC.md` 1.4), reading the stop flag before each try.
  - **The connector.** `object_store`'s errors cannot classify a try: its `RetryError`, which holds the status, is in a private module (`src/client/mod.rs`). So every client is built with the engine's own `HttpConnector` (`AmazonS3Builder::with_http_connector`), which wraps one `reqwest` client per runtime (the controller's store clients share one, the finisher's has its own, 2.11) and records, in a task-local slot the try sets around its call, the request's status, its `Retry-After` and, for a 4xx, the error code of a body it reads up to 64 KiB, passing on only the bytes it read (`object_store` reads a 4xx body whole, `src/client/retry.rs`). Each try is classified by `BATCH_SPEC.md` 1.6's table from that record and the call's result.
  - **Its `reqwest` client** is the engine's own, so `object_store` applies none of the builder's client options to it (only its own connectors call `ClientOptions::client`, `src/client/mod.rs`). The engine sets exactly: `timeout(timeout_ms)`, `connect_timeout(http.connect_timeout_ms)`, `redirect(Policy::none())`, so every 3xx reaches 1.6's table, `https_only` unless the endpoint is `http`, no automatic decompression, the user agent `unityrs/<version>`, at most `download_concurrency` idle connections (2.11), `http1_only()`, `no_proxy()` (section 8), and, in tests only, `extract.md` X7's `resolve` hook.
  - **`unsettled_tries`** (`BATCH_SPEC.md` 7.3), on which the caller's no-wait rule rests, is an atomic each try that `BATCH_SPEC.md` 7.3 counts (the finisher's row-file tries among them) increments at its start and decrements when 1.6's table settles it, so a try a stop froze stays counted; the final head reads it.
  - **The clients.** `MultipartStore::create_multipart` takes no options (`src/aws/mod.rs`), so a multipart upload's Content-Type is its client's default. The controller has one store client per multipart Content-Type, `image/png`, `application/json` and `application/zip`, each built at preflight with `with_default_content_type`; single PUTs set theirs per request. They share the connector's one connection pool, so `C_net` counts their connections once (5.3).
  - Each multipart upload id is appended to the journal after its create and before its first part, and removed after its complete or abort (E64). Only the controller's transfers use multipart: the finisher writes single PUTs (2.11).

### 2.10 Why D holds

**The argument.** An attempt is a program run on one thread. Everything it reads is fixed by D's inputs:
- its staged bytes and its view's staged bytes, read whole from read-only spool files that nothing writes during the run (2.5);
- the spec: its limits, `M` and `D`, its rules, its entries for the view's bundles;
- the registration order, `BATCH_SPEC.md` 4.3, which depends only on the ids in the view.

Everything it writes to is its own: its budget, its meters, its private Environment's registry, LRU, cells and work counts. No other thread books with them (2.2). The core keeps no state across threads but the budget and the provider chain, and its thread-locals carry nothing from one call to the next (A12); version 1's provider chain is empty. So the attempt computes the same records in every run. Absent a stop, whether it runs at all depends only on whether its view staged (D's second limit); a stop, another attempt's run failure among them, leaves it `not_run`.

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
| an allocator refusal that depends on the process's total memory (design round 2) | the counting allocator sees every refusal of a Rust allocation, absorbed or not, and stops the run (2.7); liblzma's two unseen paths are D's third limit until CR2 (A13) |
| a download that times out only beside others (design round 2) | staging has no whole-request timeout, only an idle timeout and a rate floor (`BATCH_SPEC.md` 1.4), so a slow neighbour costs time; a failure that remains is D's second limit |

**Tests.**
- `an_inputs_records_do_not_depend_on_what_runs_beside_it`. The engine scene runs at `threads` 1, 2 and 8; with the inputs in 20 seeded orders; with `I` at the smallest valid value and at 4 times it; with an unrelated dependency that fails to download; with a view whose two members fail at staging (`integrity_mismatch` and `limit(input_bytes, ..)`); with a bundle added that holds a same-named copy of a CAB in another input's dependency. Every input's records must equal those of a run whose spec holds only that input and the declarations of its view, timings and counters aside.
- `round_one_counterexamples_change_no_record`: one scene per row of the table above, each compared with its solo run.
- `an_attempt_books_with_no_budget_but_its_own`: a test budget records the thread of each reservation; every reservation of an attempt comes from its worker, while its attempt runs.
- `a_view_member_that_fails_to_load_fails_the_input_and_nothing_reads_after_it`.
- `a_source_rewritten_during_the_run_changes_no_record`: a local dependency rewritten in place, size and modification time kept, after staging.

### 2.11 Lifetime, stops and the finish

**Threads.** The controller (the main thread), `threads` workers, the lifecycle thread, the progress thread and the finisher, all started in preflight; a failed spawn exits 69 before anything is written. Every one runs its body inside `catch_unwind` (*Panics* below). Staging and transfers are async tasks on a current-thread runtime that only the controller drives, with `block_on`. No async task calls the core. Each runtime's blocking pool is capped at `download_concurrency` threads, which tokio spawns when first needed: a failed spawn panics inside `block_on`, and a panic on a pool thread returns as its task's `JoinError`; the controller takes either as a stop `internal`. The staging client keeps no idle connection (2.5), so its connections are the stages in flight, however many hosts the URLs name. Each runtime's store clients share one connection pool (2.9), which talks to one endpoint and keeps at most `download_concurrency` idle connections.

**One stop path.** A stop has a **reason**, and each reason one exit code (`BATCH_SPEC.md` 9.3): `deadline`, `sigterm`, `sigint`, `sighup`, `parent_died` and `stdin_eof` exit 6; `spool_full` 5; `host_io` and `host_memory` 74; `internal` 70. No stop exits 7: a 7 is a hand-over's or the finisher's own (below).
- **`request_stop(r)`** is the one way to request a stop, and it is async-signal-safe. It first stores `r` in the **stop word**, one atomic, by a compare-and-swap that keeps the more severe reason by `BATCH_SPEC.md` 9.1's precedence. Then it writes one byte to the lifecycle thread's self-pipe, which never blocks (the pipe is non-blocking, and a full pipe already holds a wake). The **stop flag** is "the stop word holds a reason", so a stop holds for every thread from the moment `request_stop` returns. Its callers: the signal handlers (SIGTERM, SIGINT, SIGHUP), the lifecycle thread's timer and stdin watch, and the controller (a `RunFailure`, a spool claim refused in staging, a phase-budget leak in phase 6, an I/O error in phase 7), which then hands over in the same turn (2.4).
- **The lifecycle thread** wakes on the byte. Its only I/O is reading its self-pipe and stdin. At its first wake it arms the **hard exit** at `T_hard = min(deadline, now + grace) - 500 ms`: one atomic, set once by compare-and-swap, here or by the finisher (below), whichever comes first. Then it cancels the controller runtime's token and wakes the controller, so a controller waiting in phase 5 or in `block_on` sees the stop. It then sets its timer for the stored `T_hard` and wakes the finisher's tries in flight (step 3).
- **At `T_hard`**, if the process still runs, the lifecycle thread calls `libc::_exit` with the **end code**: the final code once the finisher has stored it (below), else the stop word's code. It writes nothing first.
- **A second signal**: each handler first increments the **signal count**, a separate atomic, and the one that finds it already above 0 calls `_exit` at once with the final code once stored, else with the stop word's code, or its own reason's when the word's is less severe. Only the handlers touch the count: a stop for another reason (stdin's EOF, the deadline, a run failure) and the finisher's taken bit leave it at 0. So the parent-death SIGTERM that follows a stdin EOF, or an orchestrator's SIGTERM after a run failure, is a first signal and the finisher still writes the manifest.
- **The finisher takes the reason** when it starts, by setting the word's taken bit. On a stop's hand-over it uses the reason it took and arms `T_hard` itself if the lifecycle thread has not yet, so on every stop path `T_hard` is armed before step 3. On a normal end's hand-over it uses none: a reason the word holds by then came after the controller's last check, and, like a later request, it changes nothing but the finisher's try budget, which can leave row files null (step 3: a 4). Once its last head write has ended (at once when the finish writes no head), the finisher stores the code it will exit with in the **final code**, an atomic: the written head's code when that write was acknowledged; 4, or a more severe code, when it failed or its outcome is unknown (step 3); 7 when superseded; with no head written, the stop's code, or a hand-over's. A 7, a superseded end's or a hand-over's, yields to a stop's code more severe by `BATCH_SPEC.md` 9.1's precedence: the reason the finisher took, or one the word holds at a hand-over; a hand-over's 69 or 74 yields to none. The `exit` line names the reason that set the code. This is the exit rule `BATCH_SPEC.md` 9.1 and `extract.md` E47 cite: a `T_hard` or a second signal exits with the final code once it is stored; before that, with the stop's code, and a final head write in flight may still land. With a deadline, the deadline less the grace is such a stop, so a normal-end finish ends by the deadline too, but for *What cannot be bounded* below.
- **The deadline** is converted once at start to a `CLOCK_BOOTTIME` instant. The stop starts at `deadline - grace` (E45).
- **Parent death** (E46). At start the binary sets `PR_SET_PDEATHSIG` (SIGTERM). When the spec gives `parent_pid`, it then compares `getppid()` with it, and a difference (the parent died before the call) stops at once. Without `parent_pid` there is no start check.

**What the stop flag stops.**
- The controller dispatches nothing more, hands over to the finisher and parks until `_exit`: `main` never returns, which would run exit handlers under the finisher. It is the only party that starts the finisher: after phase 7 on a normal end; on a stop at its next check; from its `catch_unwind` handler, with the stop `internal`; and at a 69 or 7 in preflight or at the `running` write (`BATCH_SPEC.md` 7.2), or a 74 at a directory sink's `running` write (`extract.md` E62), with no stop reason but, for the `exit` line, the reason and detail that set that code (`host_io` for the 74), so the finisher writes no head (steps 3 to 5). It joins no worker and no transfer. Its checks: in preflight, between its steps; every 1,000 entries that `BATCH_SPEC.md` 6.2's cleanup or a `Failed` attempt's delete (2.4) removes, a stop leaving the rest to the next start; each turn of its loop; in phase 6, between sort runs and at least once per 64 MiB a pass or a merge reads; in phase 7, at least once per 64 MiB of resume's hashing pass, before each transfer's start, after the transfers drain and again after verification; and at once after its own request. A stop seen at any of them makes the end a stop (in phase 7, `BATCH_SPEC.md` 7.4's row "in phase 7, on a stop"). A check every 64 MiB of a local read leaves a stop waiting well under a second on a local disk (A11), against a grace of at least 5 s.
- Each attempt returns at its next check (2.6 step 11), with the outcome `Stopped`.
- After the hand-over nobody polls the controller's runtime, so every staging and transfer future stays where it was, with its buffers and phase-budget reservations, until `_exit`. A request the server had already received may still complete; `BATCH_SPEC.md` 7.2 says who covers that.
- No transfer starts a try of a committing request (a PUT, a `CompleteMultipartUpload`) or a new multipart upload once the flag is set: it reads the flag just before each try, which the engine makes itself (2.9). A directory transfer reads it before each write buffer, as 2.5's local copy does, and on a stop returns without renaming, leaving its `.unityrs-tmp-*` file; its rename reads it under the commit lock, which the controller takes at its hand-over (2.9). The finisher's own writes are the exception.

**The finisher** owns every manifest write after `running`, the `exit` line and the process's exit, on a normal end and on a stop alike. It is started in preflight, parked on a one-shot channel, and run once, by the controller's hand-over (above). It runs on a current-thread runtime of its own, with a store client of its own, built in preflight after the credentials are read and before the spool starts, and never uses the controller's: that client's pooled connections are driven by tasks on the controller's runtime, which nobody drives after the hand-over. In order:
1. **Close the journal.** When the spool's lock was never taken, this step and the next do nothing: a journal there is another process's. Under the journal's mutex it marks the journal closed, then reads it. A create whose response comes after the close is never journaled, and the bucket's lifecycle rule removes its upload (E64). The finisher starts no multipart upload, so the journal holds every upload the attempt can leave but those.
2. **Abort** every upload id the journal holds, in parallel, at most `download_concurrency` at once, each request with a timeout of `min(5 s, T_hard - now - head_reserve)` (step 3), 5 s when `T_hard` is not armed; when that is under 500 ms, it aborts nothing and leaves them to the next attempt's start (`BATCH_SPEC.md` 6.2).
3. **The final manifest.** It uses the reason it took (none on a normal end), which no later stop changes, and the phase the controller reached. What it writes is `BATCH_SPEC.md` 7.4's row for that phase: nothing before the `running` write was sent, or at a hand-over's 69, 7 or 74; otherwise every row file, then `syncfs` for a directory, then the head, fenced (7.2). A `syncfs` error is `host_io` (74): the head is then written as `BATCH_SPEC.md` 7.4 says, and if that write fails too the head stays `running`. A row file is written in chunks of at most 16 MiB, cut at line ends, each one single PUT or one file (`BATCH_SPEC.md` 7.1), streamed from the spool's files without a sort. When a try of the `running` write was sent and its response not seen, and no try's success was seen (an object store only: the directory sink writes every head on the controller's or the finisher's own thread, so none is in flight at the hand-over), the final head carries that try's own precondition: it lands whether that try landed or not, and a try that lands later fails its precondition. A failed precondition is read back as `BATCH_SPEC.md` 7.2 says: superseded, the finisher writes nothing more, and a final write 7.2 gives up on is a write that fails. A write that fails, its outcome unknown included, makes the code the more severe, by `BATCH_SPEC.md` 9.1's precedence, of 4 (`publish_failed`) and the code it had (the stop's, or 9.2's); the stored head is then the earlier one, unless a try whose response was lost lands.
   - **Once `T_hard` is armed**, every try of the finisher but the head's, in flight or started later, ends by `T_hard - head_reserve`, where `head_reserve` is 2 s plus 1 s, the floor of the head's own try, and no try is retried by 1.6's table. The remaining row-file chunks are skipped when less than `head_reserve` is left, and the head names that row file null in `rows`. So the head always gets its try, unless a directory sink's `syncfs` or the head's `fsync`s outlast `T_hard` (*What cannot be bounded*): each of its requests (a read-back and its 7.2 rewrite included) ends by `T_hard - 2 s`, and none starts with less than 1 s of that.
4. **The `exit` line.** It hands the line to the progress thread, which writes it before anything else and never overwrites it, and waits for the progress thread to report it written, at most 1 s, and never past `T_hard` when it is armed.
5. `libc::_exit` with the final code.

**Its working set** is fixed: one 16 MiB chunk buffer and a 1 MiB read buffer, one row file at a time, a 64 KiB buffer on the outcomes files, and its runtime and client (5.3, `C_fin`). The rows it writes are already in their order in the spool: the `inputs`, `bundles`, `skips` and `diagnostics` rows in input-id order through the spec's id permutation, which the controller keeps from validation (`C_spec`); the `outputs`, `collisions`, `archives` and `archive_members` rows from phase 6's files, each status read in the same order from the outcomes files (2.9).

**When the grace runs out first,** the lifecycle thread's `_exit` ends the process. The stored head then says whatever its last write said: `running` with its deadline when the final write did not land. A reader takes a `running` head past its deadline as a dead attempt. The `exit` line may be missing; the exit status is authoritative.

**Panics and stderr.** The binary installs its own panic hook at start. On every call it overwrites a preallocated slot of the panicking thread, length included, with the payload's first 1,024 bytes, or with the fixed text `non-string panic payload` when the payload is neither `&str` nor `String`, and writes nothing (a blocking-pool thread has no slot, and its payload is dropped). So no slot shows an earlier attempt's panic. The thread's `catch_unwind` then reads the slot:
- a worker or the controller: into the skip record or the run failure (2.4), a stop `internal`;
- the progress thread: a stop `internal`; it writes nothing more, so the finisher's wait for the `exit` line runs out (step 4);
- the lifecycle thread, which owns the hard exit: `_exit` at once with the more severe of 70 and the recorded reason's code;
- the finisher, which owns the end: `_exit` at once with the more severe of 70 and its code so far. The stored head says what its last write said.

So no panic leaves the process running with nobody to end it, deadline or not. Every engine mutex a panicking thread held (section 4) is taken by its next user ignoring the poison (`PoisonError::into_inner`): that panic is already a stop `internal`, and a journal line it tore is skipped (`BATCH_SPEC.md` 6.1). Every stderr write the engine makes goes through one non-blocking writer, which drops a line on `EAGAIN` and counts it. So a stderr that is never read stalls nothing.

**What cannot be bounded.** Every wait above has a timeout or a cancellation but two kinds of filesystem call, which Linux cannot interrupt. One is any call (a read, a write, a rename, `fsync`, `syncfs`, `statvfs`) on a filesystem that stops answering. The other is a directory sink's final `syncfs` and, on ext4 in `data=ordered` or `data=journal` mode, the head's temp-file and directory `fsync`s, whose journal commit waits on other files' data: each lasts as long as the unflushed data on the output's filesystem takes to write, the spool's included when they share one: 2.9 starts each output's writeback as it commits, and 5.6 recommends a spool on another filesystem, but a backlog can still hold the process past `T_hard`, leaving the head `running`. The plan assumes that the spool, the output directory and every local source are on local filesystems whose calls complete (A11); network filesystems are out of scope in version 1. A local source is opened without blocking (2.5), and after staging every bundle read is a spool read. When either kind outlasts `T_hard`, `_exit` still runs, but the kernel ends the process only when the call returns, so the process may outlive its deadline. The caller's own timeout and `SIGKILL` are the last resort, and the stored head says what it last said.

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

Each problem keeps the text that states it, then its answer: solved or shown wrong, the mechanism, what it rests on, and its test. Engine test names are proposals, in `crates/unityrs-batch/tests/`. Sections 3.11 to 3.13 are problems design round 1 found; 3.14 is the one its review found; 3.15 to 3.20 list the reviews of design rounds 2 to 7.

### 3.1 Cached runs never release, and budgets are bound per load, not per job

P1 round 1. Under the old step 1 every input was loaded into the shared Environment, which never evicts caller-loaded files, and each kept its decoded runs and parsed entries for the Environment's life, so the in-flight cap was reached and never released. A per-job budget could not reach a bundle opened with another `FileContext`. A dependency read from job X into input Y decoded Y's blocks on a budget nobody admitted.

**Solved** (2.2). Each attempt owns its input's only job handle and its private Environment, both on its own budget, and drops both when it returns; the worker reads the budget at zero before the slot is freed. A read into Y from X is a read through Y's copy in X's private Environment, on X's Dependency budget. **Rests on** core 4.1 *one meter per input, charged before every input-sized allocation*, *a second handle on the same bytes is independent*, *`Environment::load_dependency(source)`*. **Tests:** `every_attempt_returns_its_charges` (after every attempt, both sides of its budget read zero and `Arc::strong_count(M_X) == 1`); `an_attempts_end_message_holds_no_core_type` (a source scan of its fields against `Bytes`, `File`, `ObjectRef`, `Parsed`, `HeldCab`, `Charge`).

### 3.2 The prepaid estimate is too small, and the alone path can hang

P1 round 2. The estimate had to cover what the core's rules can book, and was still an engine guess against charges made at actual sizes; "Alone" at a cap above the in-flight bound waited forever.

**Shown wrong as a useful estimate; solved without one.** No estimate is provable below `max_input_memory`: an LZMA block's output ignores its declared size (core 3.6), and up to `max_zero_size_elements` values per object read consume no input (`limits/mod.rs`). Admission reserves each attempt's slot, `M + D`, which is the provable bound, and validation requires `M + D <= I`, so one slot always fits. **Tests:** `the_controller_always_progresses` (a model of the loop with fake attempts of random durations, durations and outcomes, run failures and leaks among them: no state has an input queued with nothing running and no stop requested); `a_hostile_lzma_block_fails_only_its_input` (a block declared at 1 byte that decodes to 64 MiB, among 50 small inputs: it fails on its own meter, and every other input's records equal its solo run's).

### 3.3 No eviction mechanism, resource reads through the shared handle, untagged refusals

P1 round 2. Registry handles were never released; a job's own `.resS` was served by the shared handle; both pools refused with one error.

**Solved.** The core's LRU evicts within each private Environment, bounded by `D`, and its pool-refusal retry evicts the others (core 3.13). A job's own resources are read through its own handle (2.6, step 8). Refusals are tagged `Input` or `Dependency` by the core, and 2.7 records either as final. **Tests:** `a_dependency_cache_smaller_than_the_view_costs_reopens_not_a_scheduling_dependent_record` (the same records at `threads` 1 and 8 with `D` = `M`); `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job` (3.8).

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

**Solved.** The engine reads a job's own entries through its handle before any registry lookup (2.6). Every other member registers as `Source::Path`, so its eviction frees its bytes. X registers as `Source::Bytes` over its own handle's bytes (2.6 step 4): its slot keeps the redirect, and its bytes, which the job holds for the attempt's life anyway, cost nothing more. **Test:** `an_input_that_is_also_a_dependency_reads_its_own_resources_through_its_job`: its own resource reads charge nothing to its Dependency budget.

### 3.9 Same-named CABs: the own-bundle-first rule

`docs/plans/core.md` section 9, Q12 (decided 2026-10-30) gives this plan the rule.

**Solved; the rule lives in the engine until CR3 moves it into the core.** Resources: own container chain first (2.6). The container: X's own `SerializedFile::container()`. Pointers: the core's `deref` from the reading file, then the private registry, where X is registered last (`BATCH_SPEC.md` 4.3). `EnvOptions::unitypy()` is untouched. **Tests:** `own_bundle_first_for_resources_and_container` (two inputs each hold `cab-x` and `cab-x.resS` with different pixels, and each lists the other; each input's textures and container rows are its own, under `EnvOptions::batch()` and an Eager variant; and an input with no dependency whose nested container's objects and top-level objects point at each other both ways, and an input whose top level points into a dependency's nested container, read as UnityPy reads them, with the dependency evicted and reopened between the pointers); `a_pointer_from_a_dependency_reads_that_dependencys_sibling_first`; `own_first_resources_equal_environment_resource_without_same_named_cabs` (the engine's rule against `Environment::resource` under `EnvOptions::unitypy()` on every testkit scene without same-named CABs, a bundle that repeats a `.resS` path among them; `PARITY.md` section 19.2's row).

### 3.10 A failed dependency fails the inputs that need it

A dependency that fails to download or parse fails every input that needs it, and no other (DESIGN.md section 4's failure model).

**Solved** (2.3). "Needs" means "is in the view". A bundle fails when it cannot be staged (2.5), its `size` or `sha256` does not match, or its load in an attempt fails. Inputs whose view does not hold it never open it. A read that fails for a dependency's own bytes found at read time (a corrupt block or entry) is an object-level skip of the input that read it; registration's peek meets such a block as "not a container" (2.6 step 4), so it fails nothing. **Tests:** `a_failed_dependency_fails_exactly_the_inputs_whose_view_holds_it` (A declares D and reads it, B declares nothing, C declares D and never reads it, W declares U, which holds a CAB named like one of D's; D, a CAB and its `.resS`, fails by a 404, a truncated body, a corrupt header and a corrupt data block among its CAB's blocks, before the `.resS`, so every registration's peek of the `.resS` decodes it (2.6 step 4). For the first three A and C fail and B's and W's records equal a run where D is whole; for the corrupt block A skips the objects that reach it and C is unaffected); `a_shadowing_bundle_outside_the_view_changes_no_record`; `a_dependency_that_is_not_a_bundle_fails_its_inputs`.

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
| a superseded attempt's late PUT could leave the store holding bytes the head does not describe | design round 3's time fence, which design round 4 replaced (3.16) |
| an attempt's work was unbounded through reopens after eviction | `max_attempt_evictions` (2.6 step 7). **Test:** `an_attempt_that_thrashes_its_cache_fails_on_its_eviction_cap` |
| allocator refusals were both a run stop and an input's record | a seen one stops the run; the counting allocator sees the absorbed ones too, and liblzma's two unseen paths are D's third limit (2.7, A13) |
| a non-bundle dependency was keyed by its spool name | members must be bundles (2.3, rule 5) |
| stderr was an unlisted wait that panics write to | the panic hook and the non-blocking writer (2.11), W13 |

### 3.16 Design round 3's review

| problem found | answer |
|---|---|
| a request the store had already received could commit after the time fence, and `SO_LINGER` could not stop it | no client-side fence can bound a store's commit. The engine fences its own head writes, verifies what it published before the final head, and the caller's stated duties cover the rest; the manifest says which guarantee holds (2.9; `BATCH_SPEC.md` 7.2). The time fence, `SO_LINGER` and the clock-skew allowance are gone. **Test:** `a_late_put_before_verification_is_a_mismatch` |
| a stop the controller requested held only once the lifecycle thread read the pipe, so phase 7 could still publish | `request_stop` sets the stop word before it returns, and the controller hands over in the same turn (2.4, 2.11). **Test:** `every_stop_reason_exits_with_its_code`, with the lifecycle thread held at a hook: no PUT after a `RunFailure` |
| a stopped attempt's end message had no outcome, and two parties could start the finisher | the outcome `Stopped`; only the controller's hand-over starts the finisher (2.4, 2.11) |
| the finisher could not write a row file to a store within `C_fin`, and its multipart uploads escaped the journal | row files in single-PUT chunks of at most 16 MiB, `C_fin` 24 MiB (2.11, 5.3). **Test:** `a_stop_in_each_phase_stays_within_the_bound`, with a row file over 16 MiB |
| the finisher's read-back raced a `running` write still in flight | the final head carries the `running` write's precondition (2.11) |
| a refused reopen was tried again at every access, uncounted | it fails the input (2.6 step 7, 2.7). **Test:** `a_refused_reopen_fails_the_input` |
| an input with no dependency was never registered, so a pointer between its own containers missed | every input registers itself (2.6 step 4) |
| one input's export work and spooled output were unbounded | `max_export_work` (2.6 step 10) |
| one URL could stop the whole run with `spool_full` | a bundle is at most `M`, and stated sizes are checked before the claim (2.5) |
| the attempt exported out of key order, so `duplicate_key` and the row orders were undefined | the plan pass (2.6 step 5) and the record order (`BATCH_SPEC.md` 4.4) |
| the staging client's idle pool grew with the number of hosts | no idle connection for staging (2.11) |
| `with_room` absorbed an allocator refusal silently | named (2.7, A13); CR2 extended; preflight checks the map count and the host's memory |
| a spool on tmpfs is memory outside the bound | refused at preflight (5.4) |
| (critic) object_store's own retries hid a timed-out try, so a head with every request answered let the next attempt start while that try could still commit | the engine makes every try and counts each unsettled one (`unsettled_tries`); the no-wait rule needs `"conditional"` fencing (2.9; `BATCH_SPEC.md` 7.2). **Test:** `a_timed_out_try_replaced_by_an_answered_one_is_unsettled` |
| (critic) under `Lazy`, a pointer from X's top level into an unreached nested container missed | every registered bundle's nested containers are reached at registration (2.6 step 4). **Test:** 3.9's nested scenes |
| (critic) a refused reopen and any other `Budget(Dependency)` could not be told apart | every such refusal after registration fails the input; `budget(dependency)` is no longer an object's skip (2.7) |
| (critic) the first signal after a stop for another reason exited at once, with no manifest | a separate signal count; only a second signal exits at once (2.11). **Test:** `stdin_eof_then_sigterm_still_writes_the_manifest` |
| (critic) one input's committed outputs could still fill a spool sized for the corpus | export work counts every spooled byte, so one input holds at most `max_export_work`, and 5.6's rule has a term for it |

### 3.17 Design round 4's review

| problem found | answer |
|---|---|
| a directory transfer's rename could complete during the hand-over with its outcome never written, so the manifest said `not_written` for a file the root held | the commit lock: a rename and its outcome are one step, and the controller takes the lock at its hand-over (2.9). **Test:** `a_rename_racing_the_hand_over_is_written_or_absent` (a hook holds a rename at each point; each destination's status matches what the root holds) |
| a stop after phase 7's last transfer start, or during verification, ended as a normal end: `complete` over archives never written, with exit 6 | the controller checks after the transfers drain and after verification, and a stop seen there is a stop; a reason the finisher finds after a normal end's hand-over changes nothing but the finisher's try budget; a LIST a stop interrupts gives `verified: false` with no warning (2.11; `BATCH_SPEC.md` 7.4, 9.2). **Test:** `a_stop_after_the_last_transfer_start_ends_as_a_stop`, with the stop after the last start, during verification, and between the last check and the taken bit |
| a `running` write whose tries all went unanswered exited 69 "with nothing written", and the retry 9.1 prescribed then met that write's late head and exited 7 | 69's action is "retry with `attempt + 1`", with no wait under conditional fencing: it leaves at most a fenced `running` head (`BATCH_SPEC.md` 7.2, 9.1). **Test:** `a_late_running_head_after_69_does_not_supersede_the_retry` |
| phase 6's passes and resume's hashing had no stop check, so one long pass outlasted the grace | a check per 64 MiB read (2.9, 2.11). **Test:** X10's resume of a large archive stopped at the minimum grace |
| a `T_hard` after a `complete` head had landed exited 6 | the final code (2.11). **Test:** `every_stop_reason_exits_with_its_code`, with a stop held until the head lands |
| registration's peek was neither classified nor sized: a corrupt block before a `.resS` could fail every input whose view held it, a refused peek evicted nothing, and A5's cost predates it | the core's peek rule; CR7; the refusal accepted and counted in `D`'s sizing; A5 measured again (2.6 step 4, 5.6, section 6, A5). **Test:** 3.10's corrupt block, now before the `.resS`; `a_refused_peek_fails_the_input_the_same_way_every_run` |
| X's slot reads X by the memory reader's rules and X's own handle by the stream reader's, which can give two header strings | the slot's header and directory are compared with the handle's; a difference fails X, `container_error` (2.6 step 4). **Test:** `a_header_string_the_two_readers_cut_differently_fails_the_input` (no NUL in a header string's first 32,767 bytes) |
| the NGUI cutter's "any error gives no name" turned a budget refusal or a host failure into a record | only an error 2.7 makes an object's skip gives no name (`BATCH_SPEC.md` section 5). **Test:** `the_cutters_name_read_routes_errors_as_classify_does` |
| an allocator refusal the core absorbed could move a later record | the counting allocator (2.7); liblzma's two unseen paths (on a dependency's meter; on the input's after an absorbed budget refusal) are D's third limit until CR2. **Test:** `an_absorbed_allocator_refusal_stops_the_run` (the k-th allocation of an attempt refused, for every k) |
| header values read from the environment were outside the bound | each read once into one shared value, at most 16 KiB, and counted in `C_hdr` (5.3; `BATCH_SPEC.md` 1.2). **Test:** `an_environment_header_is_read_once_and_capped` |
| one input could still pass its share of the spool: phase 6's ancestor markers multiplied a deep key, and export work counted bytes where the spool claims blocks | no markers: `/`-first order; export work counts each spooled file in a fixed 4 KiB unit, and the spool share is bounded by `st_blksize` over it; phase 6's files at most `6 x S` (2.6 step 10, 2.9, 5.6). **Test:** `an_input_of_tiny_outputs_fails_on_export_work_before_the_spool`, on spools of 4 KiB and of 128 KiB blocks (`st_blksize` set by a test hook) with the same record, and `a_deep_key_costs_phase_6_its_own_length` |
| a local source that is a FIFO, or on a filesystem that stops answering, held staging forever | a non-blocking open and a regular-file check; network filesystems out of scope (2.5, A11). **Test:** a FIFO source fails `source_not_regular` at once |
| `object_store`'s errors could not tell an unsettled try, `Retry-After` or `ExpiredToken` | the engine's connector and `BATCH_SPEC.md` 1.6's table (2.9). **Test:** X7, one fault per row of the table |
| head fencing named HTTP statuses `object_store` does not surface, and a 409 exited 69 | `Precondition` or `AlreadyExists` goes to the read-back; a read-back with no head retries `If-None-Match: *` (`BATCH_SPEC.md` 7.2). **Test:** X8, a 304, 409, 412 and `If-Match`'s 404 on each head write |
| a multipart upload could carry no Content-Type | a store client per multipart Content-Type (2.9). **Test:** X7's headers check on an archive and on a file over 16 MiB |
| `endpoint` was undefined in virtual-hosted mode, and a 404 on the head did not prove the bucket | the service URL, mapped per mode; a LIST at preflight (`BATCH_SPEC.md` 1.6, 7.2). **Test:** X8, both modes on the test store, and a missing bucket |
| manifest rows had no status or type for `unit_conflict`, the archive counts and the keys `publish_incomplete: false` withholds | `BATCH_SPEC.md` 7.3's value sets; 2.9. **Test:** X6's scenes check every row against 7.3 |
| the planned cache's decoded bytes and decode transients shared one meter | CR6 splits them (section 6, 8) |

### 3.18 Design round 5's review

| problem found | answer |
|---|---|
| preflight's memory check read the process's own cgroup alone, so a finite limit on an ancestor passed unseen | the smallest finite `memory.max` up the cgroup's ancestor chain, or v1's `hierarchical_memory_limit` (2.7). **Test:** X8, a cgroup at `max` under a parent whose limit is below the bound |
| the final write's read-back took a lower attempt's late `running` head for `superseded`, so a stop during a retry after 69 exited 7, "do not retry" | a lower attempt's head is retried with `If-Match` at either write; only a higher one supersedes (`BATCH_SPEC.md` 7.2). **Test:** `a_late_running_head_after_69_does_not_supersede_the_retry`, with a stop held during the retry's `running` write while the late head lands |
| the finisher ran `syncfs` after the head, so a power cut could leave a durable head over files never flushed, and a `syncfs` error had no code | row files, then `syncfs`, then the head; a `syncfs` error is `host_io`; a directory sink needs Linux 5.8, whose `syncfs` reports writeback errors (2.9, 2.11). **Test:** X7, the finisher's calls in order through the filesystem hook, and a failing `syncfs` |
| the engine's C zlib allocated through `malloc`, which the counting allocator never sees, and its `Z_MEM_ERROR` had no class | `zalloc` and `zfree` through Rust's allocator; `Z_MEM_ERROR` is `host_memory`, any other zlib error `internal`; CI keeps flate2's C backends out (2.6 step 10, 2.7, A13; `extract.md` E19). **Test:** `an_absorbed_allocator_refusal_stops_the_run`, with the refused allocation inside `deflateInit2` |
| two failed members of one view could each be the one `dependency_failed` names | X's own staging failure gives its own reason, else the failed member of least id (2.3). **Test:** 2.10's D test, with a view of two failed members |
| phase 6's joined lines and the `archive_members` rows outgrew the records export work counted, so 5.6's spool rule could fail after every input had run | each member record counted at its row's largest size; every phase-6 line at most twice its record's counted size; the rule in `S`, with the `download_concurrency` largest central directories (2.6 step 10, 2.9, 5.6). **Test:** `a_deep_key_costs_phase_6_its_own_length`, with 256 archives of long keys over tiny textures |
| the store's `reqwest` client is the engine's own, so the builder's timeouts and HTTPS rule never applied, and a 3xx was followed | the client's settings listed, no redirect among them (2.9; `extract.md` E37). **Test:** X7, a redirecting store and a stalled response |
| after a stop the finisher's tries had only `timeout_ms`, so one stalled row-file chunk could outlast `T_hard` and leave the head unwritten | one try each, timed to leave the head its own (2.11 step 3, W10). **Test:** `every_stop_reason_exits_with_its_code`, with a row-file chunk stalled at the minimum grace |
| `BATCH_SPEC.md` 9.1 and E47 stated the final-code rule unlike 2.11, and wrongly when the final write failed, was lost or was superseded | 2.11 states it, and both cite it; the `exit` line's `status` is the last head write acknowledged (`BATCH_SPEC.md` section 10). **Test:** `every_stop_reason_exits_with_its_code`, with the final write failed and with its response lost |
| export work left out atlas JSON files, and left the records file's flushes and a pixel's width open | every spooled file, the records per 64 KiB flush, pixels at the mode's width (2.6 step 10; `BATCH_SPEC.md` 1.1). **Test:** 3.17's `an_input_of_tiny_outputs_fails_on_export_work_before_the_spool`, with an input of empty atlases |
| X13 squeezed `M` alone, so `max_export_work`'s default and `D` failed it | one pass per limit, the others at their maxima (`extract.md` X13) |

### 3.19 Design round 6's review

| problem found | answer |
|---|---|
| the head's spooled parts were their sum in `BATCH_SPEC.md` 7.3 and 5.6 but the largest in 2.6 step 10 and `extract.md` E40, and their units mixed 4 KiB-rounded files with byte-sized member records | one statement, in bytes, in 2.6 step 10, which the others cite; pixel bytes count unrounded, in export work only. **Test:** X8, `spooled_parts` against the inputs' spooled files and member rows |
| X13's `D` pass expected `budget(dependency)` or an evicting run, though a refused registration fails `dependency_failed(<id>, budget(dependency))`; and `M`'s need left out decode work and file size | the `D` pass's outcomes include the member's `dependency_failed`, and every need is measured (`extract.md` X13) |
| nested containers' objects: nothing said that selection leaves them out where UnityPy's `env.objects` and `env.container` reach them, and the oracle's walk would export them | `BATCH_SPEC.md` section 3 says so; the oracle exports only the input's top-level SerializedFiles and loads the rest with `is_dependency=True` (`extract.md` section 5). **Test:** X6, a bundle with a nested container |
| the block ratio `st_blksize / 4 KiB` rounded down, so a 6 KiB block claimed past the bound | `k_blk` rounds the ratio up (2.6 step 10). **Test:** 3.17's `an_input_of_tiny_outputs_fails_on_export_work_before_the_spool`, on spools of 6 KiB and 3 KiB blocks too |
| a 69 at the `running` write had no path through the finisher, and `BATCH_SPEC.md` 7.4 wrote an `incomplete` head where 9.1 writes none | a hand-over with no stop reason, carrying the 69's reason for the `exit` line: no head from the finisher, a null `status`, exit 69 (2.11, W10; `BATCH_SPEC.md` 7.4). **Test:** X8, a 69 from unanswered `running` tries |
| a head's read-back that found no head took 1.6's `publish_failed`, against 7.2's retry, and a head it could not read had no rule | a read-back's 404 is no head; a writer never overwrites a head it cannot read (`BATCH_SPEC.md` 1.6, 7.2). **Test:** X8, a read-back that finds no head, and one that finds a head over 1 MiB |
| the finisher's `syncfs` waited on all of phase 7's unflushed output, so a stop could leave the head unwritten past `T_hard` | writeback started as each output commits (2.9); the limit stated (2.11; `extract.md` E45); a spool on another filesystem recommended (5.6). **Test:** X7, each committed output's `sync_file_range` through the filesystem hook |

### 3.20 Design round 7's review

| problem found | answer |
|---|---|
| X13's need could not be measured: the core counts no refusal of a meter's own limit, the need's test was not monotone, and `M`'s wrapped reasons were missing | the need is the least value at which no refusal of the limit occurs, absorbed or surfaced; CR8 counts the core's refusals (section 6; `extract.md` X13) |
| a directory transfer kept writing after a stop, and the head's `fsync`s could wait on that data like `syncfs` | it reads the stop flag before each write buffer; the head's `fsync`s named in *What cannot be bounded* (2.11; `extract.md` E45). **Test:** X10, a directory archive streaming at a stop |
| a 69 or 7 hand-over said there was no head, though a fenced `running` head or a newer attempt's may be stored | the finisher writes none; the null `status` means no write was acknowledged (`BATCH_SPEC.md` 7.4, 9.1, 10) |
| a supersession at the final write was said to leave no row files, though they are written before the head | every row file is written and no head names it (`BATCH_SPEC.md` 7.4). **Test:** X10's 7 cases |

---

## 4. Why it cannot deadlock

Every wait in the process:

| # | who waits | for what | why it ends |
|---|---|---|---|
| W1 | the controller, phase 5 | one event: an end message, a 1 s tick, the stop flag's wake | every dispatched attempt sends exactly one end message (2.4), and attempts only send, on a channel of capacity `threads` |
| W2 | the controller, in `block_on` (phases 2, 3, 4, 7) | network requests | a staging try ends at its idle timeout or below its rate floor, and its bytes are capped by the spool's size (`BATCH_SPEC.md` 1.4); a store request has the sink's `timeout_ms` (1.6); retries are counted; `block_on` selects on the cancellation token |
| W3 | an idle worker | the next attempt on the dispatch channel | it holds nothing, and the controller dispatches before it waits whenever a slot fits |
| W4 | an attempt | a core lock | held only to look up or publish, never across a decode or another lock; the only caller code under one is the budget, which is atomics (core 4.1) |
| W5 | an attempt, the controller, the finisher, a blocking-pool thread | a local filesystem call | A11 |
| W6 | a transfer | a `download_concurrency` permit, or the phase budget | held only by transfers inside requests, which end by W2 |
| W7 | a transfer or the finisher | the journal's mutex | held for one append or one removal, never across a request; the file call inside is W5. The outcomes files take no lock (2.9) |
| W8 | the progress thread | stdout | nothing waits on it but the finisher, for at most 1 s (2.11) |
| W9 | the lifecycle thread | the self-pipe, its timer, stdin; `CancellationToken::cancel`'s internal mutex | nothing waits on it; it does no I/O but those reads and `_exit`; `tokio_util` holds that mutex for one bounded walk of the token's waiters |
| W10 | the finisher | its hand-over; abort requests and the manifest's writes | the hand-over comes from the controller: after phase 7, at a stop, at a 69, 7 or 74 before the `running` head, or from its panic handler; each request is on its own runtime and client, and, once `T_hard` is armed, one try, ending by `T_hard - head_reserve`, the head's by `T_hard - 2 s` (2.11 step 3); file calls are W5 |
| W11 | the spool's and the output root's locks | `flock` with `LOCK_NB` | never waits: busy exits 69 |
| W12 | the controller, after its hand-over | nothing: it parks | the finisher's or the lifecycle thread's `_exit` |
| W13 | any thread | stderr | never waits: the panic hook writes nothing, and the engine's writer is non-blocking (2.11) |
| W14 | a requester of a stop | the self-pipe | never waits: the pipe is non-blocking, and a full pipe already holds a stop |
| W15 | a blocking-pool thread's commit, or the controller at its hand-over | the directory sink's commit lock | held for one flag read, one rename and one `pwrite` (W5); the controller then keeps it, parked (W12), and a commit that waits on it after the hand-over waits, holding nothing, until `_exit` |
| W16 | the controller or the finisher (an attempt takes no engine lock, 2.12), or the progress thread | the progress mailbox's mutex | held for one swap of the slot, never across a write to stdout |

**There is no cycle.** Rank the parties: 0, the kernel, the network and stdout; 1, core locks, the budgets and the engine's mutexes; 2, a running attempt; 3, transfers and the finisher; 4, the controller; and the lifecycle thread apart. Every wait-for edge goes to a lower rank, or to rank 0 with a timeout, except W3's, which comes from an idle worker that holds nothing, W10's hand-over, which the controller sends before it parks, and W15's, whose holder waits on nothing but a file call. The lifecycle thread waits on nothing in the process but W9's bounded mutex, and its `_exit` at `T_hard` ends every other wait but a stuck file call (A11) and a directory sink's final `syncfs` and head `fsync`s (2.11, *What cannot be bounded*). The core blocks nowhere (core 4.1, *the core never waits and never starts a thread*). No engine lock is held across a core call or a network request.

**Tests.** Every engine test runs under `unityrs_testkit::guard`, which fails a hang instead of waiting forever. The scenes include a stalled sink, a stalled download server, a stdout and a stderr that are never read, and a stop in each phase; `a_wedged_attempt_cannot_pass_the_deadline` (a test decoder spins, and the process exits 6 by the deadline); `a_stop_during_staging_exits_within_the_grace`; `a_leaked_charge_stops_the_run_with_70_and_no_deadline`. `the_engine_starts_no_unbounded_wait` is a source scan in the core's style (`unityrs_testkit::waits`): it finds every `recv`, `lock`, `join`, `wait`, `park`, `flock`, `write` to stderr and `block_on` in `unityrs-batch` and checks each against this table, by module, and that no lock guard is alive across a call into `unityrs_core` or an `.await`.

---

## 5. The memory argument

### 5.1 The bound

In the charge unit (`heap_cost`, glibc's chunk, core 3.15), at every instant:

```
heap <= I + C_thr + C_env + C_net + C_spec + C_hdr + C_fin + C_base
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
| the AssetBundle object's records, the view's name table, the plan table, the sprite-group table, the record buffer | `Input` | the attempt | its end |
| the keys of a lookup | the meter of the lookup's `from` | during the call | at its return |
| each private open: a `Path` source's bytes, its runs and entries, the runs registration's peeks cache (2.6 step 4) | the attempt's `Dependency` | load, or reopen after eviction | eviction, or the attempt's end |
| the private registry's slots and keys | `Dependency` | registration | the attempt's end |
| a read's handles into another bundle | `Dependency` | the read | the read's end |
| the sort's runs and merge buffers | phase | phase 6 | before phase 7 |
| a LIST page (resume, verification: 16 MiB reserved per call); a transfer's buffer; an archive's hashing pass | phase | phase 7 | its end, or `_exit` for a transfer frozen by a stop |
| staged bundles, outputs, records, sorted runs, member lists, archives' central directories, row files | disk (spool) | their write | the spool's next start (`BATCH_SPEC.md` 6.2) |

### 5.3 The constant terms

| term | value | source |
|---|---|---|
| `C_thr` | `threads x (2 x stack_needed + 0.11 MiB) + 4 x 2 MiB + 2 x download_concurrency x 2 MiB` | each worker's stack of twice `Limits::stack_needed` (release: 1.75 MiB, `limits/mod.rs`; 2.4), counted whole, plus its detection scratch, inflater state and window and tally (0.11 MiB, core 3.15) and its 1 KiB panic slot (2.11); the controller, lifecycle, progress and finisher threads at 2 MiB each; the two runtimes' blocking pools, each capped at `download_concurrency` threads of 2 MiB (2.11) |
| `C_env` | `threads x (c_env + max_view_bundles x 1 KiB)` | each private Environment's fixed parts, uncharged (core 3.15, one per Environment), and per registered bundle the uncharged `Meter::OWN_HEAP`, `Dependency` record and two copies of its staged path (`limits/meter.rs`, `env/load.rs`, `env/mod.rs`), 1 KiB at most with `spool.dir` at most 256 bytes (A1) |
| `C_net` | `3 x download_concurrency x (c_net + 2 MiB + 64 KiB)` | connection, TLS and a 64 KiB streaming buffer per transfer or stage, `c_net <= 2 MiB` (A1), for the controller's clients and for the finisher's (2.11), and for as many idle connections as the start's aborts can leave in the controller's store pool while staging runs. The staging client keeps no idle connection, and each runtime's store clients (the controller's three, one per multipart Content-Type, or the finisher's one) share one pool of at most `download_concurrency` connections to their one endpoint (2.9). Plus, per transfer, a multipart upload's part list, reserved at its part count, at most 10,000 parts, 2 MiB (A15), and a 4xx body the connector reads, at most 64 KiB. Staging streams through that buffer; upload payloads are in the phase budget |
| `C_spec` | the parsed spec's heap, measured + 256 B x the declared bundles | the parsed spec, measured by the counting allocator's live-bytes count on the controller thread across parsing and validation (2.7); the controller's per-bundle state and the id permutation. Preflight computes the bound after validation, so it uses the measured figure, and no parse factor is assumed (A1) |
| `C_hdr` | the distinct header values' total + `download_concurrency x` the largest bundle's header block | each `{"env": NAME}` header variable is read once into one shared `Bytes`-backed value, at most 16 KiB (`BATCH_SPEC.md` 1.2), however many bundles name it; and each stage in flight serializes its bundle's headers, names and resolved values, into its request. Both are known at preflight, before the bound is checked |
| `C_fin` | 24 MiB | the finisher's 16 MiB chunk buffer, its 1 MiB read buffer and its outcomes buffer, its runtime and its panic and progress slots (2.11) |
| `C_base` | measured | the binary, the runtime, the progress thread's slots, the controller's queues |

There is no provider term: version 1 configures no typetree provider (A8), so the provider cache (`ProviderChain::DEFAULT_CACHE_BYTES`, 64 MiB, one per `ProviderChain` shared by every cloned context; CR5) holds nothing.

### 5.4 Outside the charge unit

- **glibc's slack.** A live allocation whose chunk is below 128 KiB may hold up to 16 bytes past its charge (`CHUNK_SLACK`), at most half again of those allocations' charges (core 4.1, *one meter per input*; `limits/mod.rs`). So in glibc's own bytes the worst case is 1.5 times 5.1's bound, the figure 2.7's resource-limit check uses. The budgets are dominated by large mapped buffers (runs, pixels, PNG output), whose pages cover the slack, so the excess is a few percent in practice. The nightly reports it.
- **Allocator retention.** glibc raises its mmap threshold each time a mapped chunk is freed, after which large buffers come from arenas that do not return memory (`limits/mod.rs`; `tests/heap_rounding.rs`). `extract.md` E57: the binary calls `mallopt(M_MMAP_THRESHOLD, 128 KiB)` and sets `M_ARENA_MAX` to `threads + 2` at start. Fragmentation of small chunks inside arenas is not bounded by the core's rules (A7). The nightly measures peak RSS against the bound.
- **A spool in memory.** On tmpfs or ramfs the spool's bytes are memory that a memory cgroup counts and only swap can reclaim; preflight refuses such a spool (`spool_in_memory`, `BATCH_SPEC.md` 6.2).
- **Page cache.** Spool writes and file reads fill the kernel's page cache: reclaimable and outside the heap, but counted by a memory cgroup (A7). The spool writer drops the pages of outputs, sorted runs and row files after closing them (`posix_fadvise(DONTNEED)`), and keeps staged bundles cached, since attempts read them again.

### 5.5 Arithmetic on the corpus

**A block decode's transient** (`limits/vec.rs`, `budget/mod.rs`, `codec/lzma.rs`).
- `DecodeBuf` doubles its capacity and, while it grows, charges the new room beside the old. `into_bytes` charges the exact size beside the old capacity before it moves the bytes.
- For an output of `B` bytes with final capacity `c_n < 2B`, the charge is `c_(n-1) + c_n` during the last doubling and `B + c_n` at `into_bytes`. A last doubling means `B > c_(n-1)`, so the peak is `B + c_n < 3B`, at `into_bytes`.
- LZMA adds its dictionary, `min(clamp(declared, 4 KiB, 8 MiB), named)` plus 576 B, and 33,040 B of state.

For the corpus's largest block, `B` = 69.4 MB = 66.2 MiB and `c_n` = 128 MiB, so the transient peaks at 66.2 + 128 + 8 MiB + 32 KiB = 202.2 MiB = 212.1 MB. That input's measured peak is 212 MB. The model leaves nothing for its file bytes and values at that instant, so which meter and which instant the measurement took is not known (A5). The fix in progress, which sizes the room to the declared size, brings the transient to about `B + 8 MiB`.

**One attempt's Input peak** is `P_X = s_X + runs_X + transient_X + meta_X + values_X + export_X`:
- `s_X`, X's file size; `runs_X`, at most the declared decoded size plus one more copy of each block a span straddles (core 3.6); `transient_X < 3 B_X + max(8 MiB, 2 B_X) + 33 KiB` before the fix, the dictionary 8 MiB at most unless a retry after a data error widened it to `min(named, 2 x produced)` (`codec/lzma.rs`, `decode`); `values_X`, the objects being exported, one at a time;
- `export_X` for one sheet group of `L` x `H` pixels: the texture's source data (up to `4 LH x 4/3` with mips, plus its alpha texture's), RGBA and alpha sheets of `4 LH` each, the image being encoded (`4 LH`), PNG output of at most `4 LH + H` plus deflate's overhead, filter rows of `5 (4 L + 1)`, and deflate's state, charged as the sum of its five allocations' `heap_cost`, about 398 KiB (zlib 1.2.13 at E19's windowBits 15 and memLevel 9: a 64 KiB window, `prev` 64 KiB, `head` and the pending buffer 128 KiB each, which `zalloc`'s 8-byte header puts past the 128 KiB mmap threshold at 132 KiB each, and the state). So `export_X` is about `27 LH` for sprites and atlases and `13 LH` for a plain texture: 2048 x 2048 takes 107 MiB for sprites, 4096 x 4096 takes 427 MiB. A sprite's mesh arrays add a few bytes per vertex and index.

**One attempt's Dependency peak** is the private registry (about 300 B per CAB of the view, A4) plus the opens its reads keep within `D`, the blocks registration's peeks cache (X's own included: its slot decodes X's blocks on `D`, 2.6 step 4), and one decode's transient. On the corpus no view's dependencies decode to more than 89.4 MB, so at `D` = 256 MiB no attempt evicts for its reads; whether a registration's peeks fit beside them is measured again with the peek (A5).

### 5.6 Sizing rules, and an example

Each rule is a recommendation the manifest reports against, not a validation.

| goal | rule |
|---|---|
| every input that fits at all completes | `M >= max(input_peak, dependency_peak, ceil(decode_work_peak / 4), the largest staged size)` from an earlier run's manifest (`BATCH_SPEC.md` 7.3; `max_decode_work` is `4 M`, 2.2), a guide, since `decode_work_peak` misses some work (7.3); X13's need (`extract.md`) is the exact figure; and `D >= max` dependency peak, registration's peeked blocks included (5.5) |
| full parallelism | `I >= threads x (M + D)` |
| the spool fits | `spool.size_bytes >= the staged bundles + S + threads x W + 6 x S + the download_concurrency largest central directories`, where `W = max_export_work x k_blk` bounds one input's claims, `S = spooled_parts x k_blk` (2.6 step 10) bounds the inputs' claims and their records' counted size, one input's at most `W`, and `6 x S` holds phase 6's files, its row files among them: one sort's runs and its merged output, beside the step outputs later steps and the finisher read, each line at most twice its record's counted size, runs and intermediate outputs deleted once read (2.9). `S` from an earlier run's `spooled_parts`, plus `W` for each input whose output is not known, keeps one hostile input from stopping the run with `spool_full`: the claim refused there is a run failure (2.7), since what the spool holds depends on the order inputs ran in |
| the spool's filesystem holds what no claim counts | free space at start `>= size_bytes + reserve_bytes +` the directories (about `inputs x 2 x st_blksize`, and an entry per spooled file), and free inodes `>=` the spooled files `+ 2 x inputs`, the spooled files being at most `spooled_parts / 4 KiB` plus the staged bundles, phase 6's files and the row files; `reserve_bytes` is the filesystem's own margin (5.7) |
| a directory sink's final `syncfs` stays short | the spool on another filesystem than the output root, so its writes are not that `syncfs`'s (2.11) |

**Example for the corpus:** `threads` 8, `download_concurrency` 8, `M` = `D` = 256 MiB, `I` = 4 GiB.
- The constants come to about 241 MiB: 69 MiB of thread stacks (37 MiB for workers and the four named threads, 32 MiB for the two blocking pools), 9 MiB of private Environments at `max_view_bundles` 1,024, 98 MiB for three sets of 8 connections (the two runtimes' and the start's idle ones) with their part lists and error bodies, about 21 MiB of spec and controller state for a full build's 12,000 bundles (a spec of about 4.8 MB), 24 MiB for the finisher, and about 20 MiB of base.
- So the bound is about 4.23 GiB in the charge unit, and about 6.35 GiB (1.5 times it) under glibc's worst-case slack.
- The corpus's largest input peaks at 212 MB before the decode fix, inside `M`; that is a decode-only figure, and the export peak (5.5's `export_X`) is not measured (A5). Every slot reserves 512 MiB, so the 8 slots reserve 4 GiB.

### 5.7 Disk

`spool.size_bytes` bounds the sum of the staged bundles, every committed candidate and its records until phase 7, the partial outputs of the attempts in flight, phase 6's sorted runs and member lists, archives' central directories while they stream, and the manifest's row files. A free-space check against `spool.reserve_bytes` runs before each claim. A refused claim stops the run (2.7). The directories' own blocks (`attempts/` and `done/`, one or more per input) and the files' inodes are claimed by nothing: 5.6 provisions them. One input's claims total at most `max_export_work x k_blk` (2.6 step 10).

### 5.8 How it is checked

- `every_attempt_returns_its_charges` (3.1), and `the_slots_in_flight_never_exceed_the_inflight_bound`.
- X12 in `extract.md`'s gate: the core's counting allocator (`unityrs_testkit`) over the engine's export paths. The heap never holds more than the charges plus 5.3's constants, as `tests/memory_model.rs` checks for the core.
- X13 in `extract.md`'s gate: each limit squeezed in its own pass, below and at each testkit input's need.
- `a_stop_in_each_phase_stays_within_the_bound`: the counting allocator over the finisher's streaming.
- `rss_tracks_live_heap_after_large_frees`, with and without E57's settings.
- The nightly writes `VmHWM` into the manifest and checks it against 5.1's bound for its spec.

---

## 6. Requests to the core

Each request goes through the core's integration rule. None changes a core 4.1 property, and `EnvOptions::unitypy()` is untouched by all of them.

| request | change | property it adds, and its test | what the engine does until it lands |
|---|---|---|---|
| **CR1, wanted** | `load_dependency` publishes nothing when it fails: a registration refused after some of its keys were inserted (`env/load.rs`, `try_load_dep`; `env/registry/register.rs`) removes them before it returns the error | a failed load leaves the registry as it was. Test: a bundle whose registration is refused at its k-th entry, for every k, after another bundle registered a same-named CAB; every lookup after it answers as before the load | the attempt fails X on any failed load of its view (2.3, rule 1) and makes no read through that Environment after it, so the partial slots are never read. Granted, an attempt could skip a failed member that its reads never reach; D does not need it |
| **CR2, wanted** | `Error::refusal(&self) -> Option<Refusal>`, with the same on `DerefError` and `ParseError`: `Refusal::{Budget(Reservation), Limit(LimitKind, MeterKind), Allocator(Reservation), Io}`, where `MeterKind` says whether the refused meter was a dependency's. The allocator's refusal (`limits/meter.rs`, `refusal`), liblzma's `LZMA_MEM_ERROR` (`codec/lzma.rs`, `failure`) among them, gets its own variant, `grow_counted`'s smaller retry and `into_bytes`'s keep apply only to meter and budget refusals, and `pool_refused` does not match it, so `with_room` never evicts for it: an allocator refusal always surfaces | one public classification of every error the engine must route, stable across new variants, with no silent allocator path. Test: every wrapper path the core can build (`Error` → `DerefError` → `ParseError` → `ReadError` → `TreeError` → `ChargeError`) classifies as its innermost cause; an allocator that refuses the k-th allocation, for every k of a decode and of a dependency read under `with_room`, surfaces as `Allocator` | 2.7's `classify` over the public enums, with unknown variants stopping the run (exit 70); the counting allocator and the refusal count see every allocator refusal but liblzma's two unseen paths (A13) |
| **CR3, wanted** | `EnvOptions::own_bundle_first`, true in `batch()` and false in `unitypy()`: a resource read's candidates and the AssetBundle container are looked up first in `from`'s own container chain | Q12's rule in the core, where its candidate order and slice rule live. Needs a `PARITY.md` section 19.3 row and Q12's tests in both load modes | the engine's copy (2.6, step 8) and its differential test (3.9) |
| **CR4, wanted** | a `Source::Path` registration that reads only the bundle's header, blocks-info and directory, and reads the rest of the file when a data block is first reached | registration costs the directory, not the file. Test: registering an N-byte bundle charges at most its header, blocks-info and directory, and a later read gives the same bytes as a whole-file load | every registration reads its file whole (2.2's cost) |
| **CR5, text only** | core 3.15 and its constants' table: the provider cache's bound is per `ProviderChain`, shared by every context that clones its `Arc` | none (a correction) | 5.3 says why it is not a term in version 1 |
| **CR6, not needed for version 1** | `BundleFile::predecode(data, ctx, pool, meter, reader) -> Result<Arc<Decoded>, Error>`: decodes every block once, in order, and keeps per block its length or the input-determined error its decode gave (a refusal fails the call and is not kept); returns an immutable `Send + Sync` value (header, blocks-info, directory, per-block outcomes, the joined data up to the first failed block as one `Bytes`). The retained data is charged to `pool`, the run's pool budget `P`, and the decode's transients (`DecodeBuf`'s room, LZMA's dictionary) to `meter`, a per-bundle meter released when the call returns: a `Charge` stays bound to its meter (`limits/meter.rs`), so one meter cannot hold both. And `Source::Decoded { name, decoded, reader }` for `load_dependency`: its open, first or after eviction, charges only its own tables and parses, serves each block as a zero-copy slice, returns a kept block error where a lazy open's read would, and counts no decode work for the shared data | the planned shared cache (section 8). Tests: *same bytes, same errors* (every testkit and fuzz-corpus bundle reads the same bytes and the same input-determined error through a `Decoded` open as through a lazy `Path` open with unbounded meters); *charges nothing for the data*; *retained bytes on `pool`, transients on `meter`*; *immutable* | nothing: version 1 seals every attempt |
| **CR7, wanted** | `load_dependency` reaches the loaded bundle's top-level nested containers itself, by `reach.rs`'s rule (`is_bundle_entry`), inside the load's own `with_room`, before it returns | a load's peeks are Environment work: a refusal evicts the others and retries, and the engine keeps no copy of the peek rule. Test: a view whose peeks fit only after an eviction registers, under `Lazy`, with every nested container reached | the engine's peek (2.6 step 4), with the core's rule and a final refusal |
| **CR8, test-only; a prerequisite of `extract.md` X13** | under a test-only cargo feature the testkit enables, the core counts every refusal it meets, in process-wide counters it only writes: each `Limit` refusal by `LimitKind` (`Meter::reserve`'s account limit, `limits/meter.rs`, among them), each budget refusal by `Reservation`, and each allocator refusal | X13 sees every refusal of the limit it squeezes, one the core absorbs (`grow_counted`'s smaller retry, `into_bytes`'s kept buffer) as well as one it returns; X13 reads the counts before and after each run, in a child of its own. Test: each kind counted once per refusal, absorbed or returned | X13 is not built |

**Withdrawn with design round 1's shared cache:** `Environment::evict_dependencies`, a public `with_room`, `deref_held`, and the `DecodeWork` request. A sealed attempt's Environment is dropped whole, and no outcome waits for a retry.

---

## 7. Assumed (not checked)

- **A1.** Sizes not measured: a transfer's connection and TLS state is at most 2 MiB (`c_net`); a registered bundle's uncharged heap at most 1 KiB. Test: the counting allocator on a 100 MiB download over TLS from a server that offers h2, which must negotiate HTTP/1.1, and on a view of `max_view_bundles` bundles. The parsed spec is measured at run time (`C_spec`); 5.6's example estimates it at 4 times its JSON, which a spec of short ids and long `deps` lists passes (about 11 times).
- **A4.** A registry entry costs about 300 B per CAB, from core 3.13's list, not measured. It sizes `D`; the budget enforces whatever it is.
- **A5.** Measured on the corpus, 2026-11-09 (2.2's cost), in aggregates:
  - views: 324 of each build's corpus inputs have no dependency; the mean view is 2.1 to 2.5 bundles (1.8 to 2.0 in a full build of about 11,500 bundles), the 99th percentile 6 to 10, the largest 22;
  - holders: a full build has about 335 distinct dependencies, about 190 of them held by one view, and 4 held by 741 to 3,516 views each; per corpus build the top 4 hold 80 % of the (input, dependency) pairs;
  - at version 1's reach (the NGUI cutter's `_MainTex` name, 112 atlases in 110 inputs, all on one 67 to 69 MB block): +3.6 to 3.9 GB decoded per corpus build; scaled to a full build, about +48 GB and 220 to 226 s of CPU, 2.5 to 3.3 times a once-per-run decode, about +30 s at 8 workers. All 112 reads end in `sheet_not_found`;
  - at worst, every view member reached: 8.2 times on corpus build A (7.58 GB against 0.92 GB); +95 to 258 GB in a full build, 800 to 1,570 s of CPU, 9 to 24 times;
  - the distinct dependencies of a full build decode to about 340 to 350 MB (55 % measured, the rest estimated); the corpus's to 119 to 189 MB.
  Not measured: the total candidate bytes of a build, in claimed bytes, which sizes the spool; how much of 5.5's 212 MB peak is file bytes and values; the largest export peak (5.5's `export_X`), which sizes `M`.
  Every figure above predates registration's peek (2.6 step 4), which decodes in every attempt the blocks before and holding each top-level entry's first bytes, a single-block member's whole block: on the corpus machine, before the build, the cost and the dependency peak are measured again with it (section 8).
- **A7.** With E57's settings, glibc's arena retention keeps peak RSS within a few percent of the live heap. The core's rules do not bound fragmentation, and a memory cgroup also counts the spool's page cache. The nightly measures both.
- **A8.** Version 1 configures no typetree provider: the spec has no field for one, and the base context's chain is empty. The corpus's typetrees are embedded; whether every NGUI atlas MonoBehaviour's is, is checked by the nightly (its `no_typetree` count).
- **A9.** Staging is a small part of a run on the corpus, so finishing it before extraction costs little (2.1).
- **A10.** The MonoBehaviour script hook (`Environment::object_tree`) dereferences `m_Script` inside the core. It matters only for stripped MonoBehaviours with a script provider, which version 1 does not configure.
- **A11.** The spool, the output directory and every local source are on local filesystems whose calls complete, and nothing but the engine writes the spool while it holds the spool's lock. Network filesystems are out of scope in version 1. A filesystem that stops answering can hold a thread in a call Linux cannot interrupt, past `T_hard` (2.11).
- **A12.** The core keeps no state that one thread's call leaves for another's, except the budget, the provider chain and, under CR8's test feature, its refusal counters, which it only writes. Its non-test thread-locals: in `limits/meter.rs`, the pool-refusal count (read as a difference) and the grant frames (each lazy computation opens and closes its own); in `env/registry/dependency.rs`, `RETRYING`, `RETRIED` and `RUNNING` (restored or cleared by their guards when the outermost `with_room` returns, unwinding included); in `env/registry/mod.rs`, `LOAD_KEYS_HERE` (read as a difference, `env/container_index.rs`). So what an earlier attempt on the same worker left in them changes nothing. Tests: `an_inputs_records_do_not_depend_on_what_runs_beside_it` at `threads` 1, where every attempt follows another on one thread; `the_core_has_no_unlisted_thread_local` (a source scan for `thread_local!` outside `#[cfg(test)]`, and for a `static` outside CR8's feature, against this list).
- **A13.** Under preflight's resource-limit, overcommit and memory checks, the allocator rarely refuses an allocation within 5.1's bound. When it does (a cgroup that refuses mappings, `vm.max_map_count` reached), the counting allocator sees every refusal of a Rust allocation, absorbed or not, zlib's included (its `zalloc` is Rust's allocator, 2.6 step 10), and the run stops (2.7). The two it cannot see are liblzma's C `malloc` refused (a) on a dependency's meter, which `with_room` takes for a pool refusal and retries after eviction, and (b) on the input's meter after a budget refusal absorbed earlier in the same core call, which reads as `budget(input)` (`codec/lzma.rs`, `failure`; `limits/vec.rs`, `grow_counted`): each can decide a record by the host's memory without a trace, D's third limit (section 1). CR2, or an `lzma_allocator` the core routes through Rust's allocator, removes both. An allocator refusal in an infallible allocation (an uncharged collection, engine code, tokio, the TLS stack) aborts the process (`SIGABRT`, exit status 134), with the head left `running` and no `exit` line.
- **A14.** The caller keeps the duties of `BATCH_SPEC.md` 7.2 (one attempt of a `run_id` at a time, no writer of its keys but its attempts, the stated wait before a retry), and the store's LIST shows a write once it is acknowledged, as S3 does. The head's guarantee rests on both; verification checks the second for this attempt's own writes.
- **A15.** `object_store` 0.12.5's behaviour is read from its source (`extract.md` section 3, item 1; 2.9). What remains assumed is the store's: its error bodies, ETags and upload ids are S3-sized (an error document under 64 KiB, which the connector reads at most of; an ETag under 100 bytes, so a part list reserved at 10,000 parts fits `C_net`'s 2 MiB; an upload id under 1 KiB); a LIST page holds at most the `max_keys` asked for; and `hyper` never sends a request's bytes twice within one try.

(A2, A3 and A6 were design round 1's assumptions about the shared cache, and went with it.)

---

## 8. Open, for the review rounds

1. **Sealing.** Its cost was measured before registration's peek existed (2.2, A5). A review should look for any state an attempt reads that another attempt can write.
2. **Staging** copies every local bundle into the spool (2.5). Design round 4's review found that nothing reads a source after its copy.
3. **Overlapping staging with decoding** is given up (2.1). A later version may start an attempt once its view has staged.

**Measurements on the corpus machine, before the build:**
- the sealing cost and the dependency peak again, with registration's peek (A5, 5.5);
- the largest export peak and the largest export work before `M`'s example and `max_export_work`'s default (16 x `M`) freeze;
- how many of the corpus's Texture2D objects are in formats outside E15's five (ETC2, ASTC, ARGB32, RGBA4444, Alpha8 and the rest), each skipped as `unsupported_format` (exit 3) where UnityPy exports it, so a consumer's cutover would lose them.

**Planned optimization: a shared pre-decoded dependency cache.** Not in version 1. It keeps D and B in this form:
1. **Selection per bundle.** Whether a bundle Y is pre-decoded is a function of Y's own spec entry and bytes alone: a per-bundle flag (`"predecode": true`), and all of Y's blocks decoding within the pre-decode meter's limits. Never a rule over other entries ("in some view", "what fits after the others"): either would make Y's path, and so the records of every input that reads Y, Y itself included, depend on bundles outside their views.
2. **All or stop.** When the selected set's decoded total passes the pool, the run stops before extraction (exit 5). It never falls back per bundle.
3. **The pool inside `I`.** Pre-decode runs sealed, one bundle per budget. Each `Decoded`'s retained bytes stay charged to a run-level pool budget of limit `P` until extraction ends; its decode's transients (one file, `B + c_n` and the dictionary) go on a per-bundle meter of limit `M`, released when it returns (CR6). At most `floor((I - P) / M)` bundles pre-decode at once, so the transients fit under `I - P`. Validation becomes `P + M + D' <= I`, where `D'` is an attempt's Dependency budget for parse state and for the blocks of any bundle nested in a pre-decoded one, which keeps its own compressed blocks and is decoded in each attempt. Attempts read the pool's `Bytes` slices without charge, so their leak check still reads zero. The joined data grows on the per-bundle meter and moves to `pool` once, at its final size, with the `Decoded` tables; liblzma's unseen refusal on that meter is D's third limit in this mode too (A13).
4. **Equality across modes is not promised.** A `"shared"` and a `"sealed"` run may differ under tight budgets; D holds within each mode, since the mode is in the spec.

It needs CR6 (section 6). A full build's distinct dependencies decode to about 350 MB (2.2), so a pool of 1 GiB holds them. It saves the extra decoding of 2.2's cost, not the per-attempt parse.

**Follow-ups** (earlier rounds' LOW findings not fixed in this text):
- The core's evict-and-retry for direct reads on a dependency's handle (3.12), which would let a read fit that a full cache refuses today.
- `{container}` in key templates (`extract.md` E27).
- A spec field for the manifest's location (`BATCH_SPEC.md` 7.1).
- `PARITY.md`'s IDAT chunk size, `max(65536, 4 x width)` (`extract.md` E19).
- An optional prune of keys under the output root that the manifest does not list.
- Check the NGUI atlas's material field name against NGUI's serialized layout (`material`, or `mMat` in some versions) before the cutter is written (`BATCH_SPEC.md` section 5); name a component atlas (`UIAtlas`) by its GameObject, so it can join `script_classes`' default.
- A `PARITY.md` row, before X2 meets it, for UnityPy's sprite-sheet cache, keyed by path id alone, which aliases two sheets of one path id in different CABs that one sprite file reaches; the engine's groups keep them apart (`BATCH_SPEC.md` 4.4).
- Honouring `HTTPS_PROXY` and `NO_PROXY`, and which TLS roots the clients trust (a private CA for a self-hosted store).
- Exit 3 for skips a caller chose or expects (`reference_atlas`, `duplicate_name`, `unsupported_format`, `empty_rect`): split them from data skips before the code contract freezes.
- Deleting a run's spool directory after a normal end with an empty journal, as an option.
- LISTing only the winners' key range for resume and verification, instead of the whole prefix.
- Refreshing temporary credentials during a run, instead of the caller's duty to give ones that outlive it (E37).
- X's registry slot parses an entry of X a second time when a lookup through the registry reaches it (charged to `D`); sharing X's own handle's parses would remove that.
- A rule that a `_MainTex` outside the input's own files gives no name would remove version 1's only cross-bundle decode (2.2). It would change records only where a crossed texture shares a name with one of the input's own textures. The owner's call.

## 9. Before it is built

- This plan's review rounds, run until a round finds no MEDIUM or HIGH problem, with section 8 settled.
- CR1 to CR5 and CR7 answered. None is needed for correctness; each has its interim. CR6 waits for the planned cache. CR8 is granted before X13 is built.
- `extract.md`'s gate (its section 5) agreed. It holds the engine's tests named here: D (2.10), budgets and charging, the deadlock guard and scans (section 4), the stop path (3.15), and the memory rows (5.8).
