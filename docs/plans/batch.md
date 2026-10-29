# P2 plan: unityrs-batch (draft)

**Status: draft, not reviewed, P2.** Nothing here is a decision. This file holds the batch flow that `docs/plans/core.md` section 4.1 described until P1's third review round, and every problem reviewers found in it. Each round found new ones in admission and eviction, and the batch engine is not built in P1, so the flow left the core plan. Before `unityrs-batch` is built, this plan gets its own design and review rounds, and every open problem in section 3 is solved or shown wrong.

**What it builds on.** The core API and what it guarantees any batch engine: `docs/plans/core.md` section 4.1, a list of properties each checked by a P1 test. This plan may rely on those properties and on nothing else the core does. A need the list does not cover is a request to the core, not an assumption.

**Citations.** Review findings are cited by the name of the review file that holds them and the finding's id. Those files belong to the review process and are not in the repository; each problem below restates what it found.

---

## 1. Goal

DESIGN.md section 4's `unityrs-batch` row: a JSON job spec of inputs, outputs and resource budgets (`threads`, `max_inflight_uncompressed_bytes` among them), run in parallel with bounded memory, streaming outputs to sinks, with the run marked `incomplete` when an input fails.

## 2. The flow as P1's plan last described it

This is core plan section 4.1 as it stood at P1's third review round, unchanged except that its section numbers now point at the core plan. Section 3 lists what is wrong with it.

`unityrs-batch` builds on the core like this. Memory has one currency: each input's meter, which books every charge with its context's budget (core 3.15), and a refusal names the pool that refused (`Reservation::Input` or `Reservation::Dependency`).
1. **Registry.** Open every input bundle `Lazy` into the shared `Environment` as a dependency: each gets a `Dependency` meter booked with the dependency pool (a `Counting` of `dep_cache_bytes`), and only headers, blocks-info and directories are read, so CAB names, `blocks()` and `directory()` are known for all of them. Whatever the Environment later decodes through these handles (a cross-bundle deref from any job) is charged to the pool. When the pool refuses, the Environment evicts its least recently used handles and retries once (core 3.13); a read still refused fails that object with `Error::Budget` naming `Dependency`, never the job.
2. **Admission, outside the pool.** For each bundle the engine computes an estimate from the public `blocks()` and `directory()`: the declared sizes of the blocks each entry's span overlaps, summed over entries (a block that straddles two entries counts twice, as a job decodes it twice, core 3.6), plus blocks-info, plus a parse allowance per entry (an engine setting, tuned from the peaks earlier jobs' meters reached), capped at `Limits::max_input_memory`. A FIFO admission controller waits until the estimate fits under `max_inflight_uncompressed_bytes` less what admitted jobs hold, then builds a job `FileContext` whose budget is a `Counting` of exactly the estimate. Waiting happens only there, on the controller's thread, before the job enters the rayon pool; inside the pool the core only calls `try_reserve`, which cannot wait.
3. **Work.** The job opens its **own** bundle handle on the same `Bytes` with the job context and an `Input` meter (a second open reads the header and directory again, nothing else), then `entry()`, `objects()`, filter by `class_id()` from the table (no value read), `read_typetree` only for selected objects, payloads handed on as `Bytes` slices. Its runs, parsed entries and values are charged to its meter and live in its handle, so no other job waits on them. Pointers resolve through `Environment::deref`, which accepts a file not loaded in the Environment: its bundle siblings come through the job handle's `Parent`, every other CAB through the shared, pool-paid registry. A resource read that resolves to an entry of the job's own input reads through the job's handle (core 3.13), so a texture's `.resS` is charged to the job, not the pool.
4. **Refusal.** A job whose charges outgrow its estimate (a hostile directory, or values larger than the allowance) gets `Error::Budget` naming `Input`, never a wait. The engine drops the job's handles, which returns everything they hold, and queues the bundle to run **alone**: the controller admits nothing else until in-flight bytes reach zero, then runs it with a `Counting` of `min(max_input_memory, max_inflight_uncompressed_bytes)`. If that refuses too, the bundle fails (the run is `incomplete`, DESIGN section 4's failure model) and is not queued again. A refusal naming `Dependency` re-admits nothing (step 1).
5. **Release.** At the end the job drops its handles: every charge returns as the last slice of each buffer and the last `Parsed` drop, so admission's books equal what is alive. Slices and values a job hands on keep their charges until the consumer drops them, and the controller counts them as in flight.

Admission's books therefore cover everything a job holds, its parsed metadata, rows and values included, not only decoded bytes.

**Why it cannot deadlock.** The waits are: (W1) the controller waiting for an estimate to fit; (W2) the controller waiting for in-flight bytes to reach zero before an alone job; (W3) the engine joining a job's tasks; and the core's locks. In-flight bytes fall only when a job or a consumer drops what it holds, so W1 and W2 are safe if every admitted job finishes without waiting on them:
- a job never waits on a budget: inside the pool the core calls `try_reserve`, which refuses instead of waiting (core 3.16);
- a job never waits on another job: its runs and entries are its own handle's, the shared registry's handles are read through cells that are computed outside the cell and published (core 3.16), and a dependency read the pool refuses fails instead of waiting for a handle to be released;
- a job never waits on a core lock held by a waiter: every core lock is held only for a lookup or a publish, never across user code, a decode, or another lock (core 3.16), so no core lock can close a cycle;
- a job's work is bounded (core 3.15's limits), so it finishes or fails, and its bytes return.

W2 also holds new admissions back (the controller is FIFO), so in-flight bytes reach zero once the jobs already running finish. W3 waits only for jobs, which never wait. Consumers (sinks) never call the controller, so a stalled sink slows the run without closing a cycle. No step blocks a pool thread, so the pool cannot starve itself.

---

## 3. Open problems

Each is a problem this plan must solve, or show wrong, before the batch crate is built. Where the core has since changed, the problem says what the core now guarantees (core plan section 4.1) and what is left for the batch design.

### 3.1 Cached runs never release, and budgets are bound per load, not per job

`claude1-core-plan.md` M7 (P1 round 1).
- Under step 1 every input is loaded into the shared Environment, which never evicts caller-loaded files, and each keeps its decoded runs and parsed entries for the Environment's life. Step 5's "dropping returns every charge" never happens while the batch runs, so W1 waits forever once the cumulative decoded size reaches the in-flight cap; counting completions instead of charges lets memory grow without bound.
- A per-job budget cannot reach a bundle whose budget is the `FileContext` it was opened with.
- A dependency read from job X into input Y decodes Y's blocks on a budget nobody admitted.

**The core now provides:** a second handle on the same bytes with its own context and meter (a file need not be loaded in the Environment to dereference); dependencies under their own meter and pool, refusals named by `Reservation`; `Environment::load_dependency`; eviction that drops a dependency's handle.
**Open:** state which handle owns which runs for the life of a job, and show that every charge the controller counts is dropped at a point the controller can wait for.

### 3.2 The prepaid estimate is too small, and the alone path can hang

`claude2-core-plan.md` M1 (P1 round 2).
- The estimate must cover what the core's rules can book: the declared blocks, blocks-info, a block decoded once per span that overlaps it, entries, rows and values. Round 2 rewrote step 2's estimate, but it is still an engine guess against charges the core makes at actual sizes (an LZMA block ignores its declared size).
- "Alone" at a cap larger than the in-flight bound waits forever (the alone path's cap was round 2's fix; 3.4 shows the first admission has the same hang).

**Open:** an estimate the core's rules provably cannot exceed for a well-formed bundle, or an admission that does not need one (admit by count and let the budget refuse); a definition of "alone" that cannot wait forever.

### 3.3 No eviction mechanism, resource reads through the shared handle, untagged refusals

`claude2-core-plan.md` M2 (P1 round 2).
- Registry handles were never released, so the dependency pool filled and every later cross-bundle read failed.
- A job's own `.resS` was served by the shared registry handle, moving texture bytes onto the dependency pool.
- Both pools refused with the same error, so the engine re-ran a job alone for a pool problem.

**The core now provides:** registry slots that hold no strong dependency handle; eviction that frees runs, parsed entries and re-readable source bytes; resource reads located without parsing and read through `from`'s own handle when the CAB is an entry of `from`'s input; refusals tagged `Input` or `Dependency`.
**Open:** the engine's use of each: when it evicts, how it sizes `dep_cache_bytes`, what it does with a `Dependency` refusal.

### 3.4 The first admission can wait forever, and the estimate is inflated

`claude3-core-plan.md` M1 and `claude3-core-code.md` M5 (P1 round 3; the same defect, reported twice).
- Step 2 caps the estimate at `max_input_memory` only. With `max_inflight_uncompressed_bytes` below that (4 GiB on a 16 GB host), a bundle whose estimate is above the in-flight bound never fits, even with nothing in flight, and the FIFO controller hangs every bundle behind it. Step 4 caps at `min(max_input_memory, max_inflight_uncompressed_bytes)`; step 2 does not.
- The estimate sums the blocks each entry's span overlaps over entries, so N entries in one block count it N times, though the core serves every later entry from the one cached run (core 3.6). Ordinary single-block bundles hit the cap.

**Open:** cap the first estimate at the in-flight bound (or send an estimate above it straight to the alone path, which fails cleanly); count each distinct block span once; add both waits to the deadlock argument.

### 3.5 What a consumer holds is charged to a job that may never wait

`claude3-core-plan.md` M6 (P1 round 3), and the W2 part of `claude3-core-code.md` M5.
- A slow sink's backlog of `Parsed` values and slices is charged to the job's fixed budget, so the job is refused, re-run alone with the same sink, and fails: the outcome depends on sink speed, and "a stalled sink slows the run" is false.
- A sink that holds output across jobs (DESIGN's `prefer` winners for same-named outputs, per-atlas collision resolution) holds charged bytes that W1 or W2 waits on, while it waits for a job the controller will not admit: a cycle.

**Open:** pick one model and state it. **Backpressure:** a bounded hand-off where the job waits, that wait added to the deadlock argument, and consumers forbidden to hold output across jobs. **Copy out:** consumers copy or spool on receipt, so a job's charges are its own and in-flight bytes exclude what consumers hold.

### 3.6 Step 1 names a load the API did not have

`claude3-core-plan.md` L7 (P1 round 3).
- The only way to open a source into the Environment was `load`, which gives an `Input` meter and is never evicted, so a pool refusal was tagged `Input` and no registry handle could be evicted.

**The core now provides:** `Environment::load_dependency(source)` (core 3.13; D implements and tests it): a `Dependency` meter, CABs registered lazily, evictable, not in `files`.
**Open:** step 1 rewritten around it, or around a `CabResolver` that maps CAB names to the inputs; say which, and how the engine learns CAB names.

### 3.7 Problems settled in the core that the batch design must restate

These were found against the batch flow but were defects of the core API, fixed in P1. The batch plan relies on the property and must not undo it.

| finding | what was wrong | the core property now (core plan 4.1) |
|---|---|---|
| `codex1-core.md` H3 (round 1) | a codec allocated its output before the budget saw it, so jobs could exceed the batch budget before a refusal | one meter per input, charged before every input-sized allocation; a refused charge allocates nothing. **Open for this plan:** admission's allowance for output still growing inside admitted jobs |
| `codex2-core.md` H2 (round 2) | a retained `Parsed` carried no charge, so values outlived the books | every `Parsed` carries the charge for its value; values a job hands on stay counted until dropped (3.5 decides who counts them) |
| `claude3-core-plan.md` M2 (round 3) | a cell published a pool refusal for good, so one job's refusal poisoned a shared dependency, and "evict and retry once" retried a cached error | transient refusals are never cached |
| `claude3-core-plan.md` M3 (round 3) | resolving a resource's CAB parsed the shared slot, decoding the whole `.resS` on the pool before the redirect to the job's handle | a resource read goes through `from`'s own handle, resolution included |
| `claude3-core-plan.md` M5 (round 3) | eviction kept a `Path` dependency's bytes and their charge, so the pool filled with bytes eviction could not free | eviction frees what it can re-read |
| `claude3-core-plan.md` L6 (round 3) | a dereference from a file whose bundle a job had dropped was unspecified | `DerefError::ContainerDropped`; a caller that hands out files keeps a strong bundle handle beside each |
| `claude3-core-code.md` M4 (round 3) | under `Lazy`, decode work was unbounded: a failed block and an `entry_range` decode were redone for every read | block outcomes kept, `entry_range` runs cached, and `max_decode_work` per input |

---

## 4. Before it is built

- A design written against core plan section 4.1's properties only.
- A deadlock argument that names every wait, the sinks' included, and shows each cannot close a cycle with the properties it relies on.
- Its own review rounds, run until a round finds no MEDIUM or HIGH problem.
