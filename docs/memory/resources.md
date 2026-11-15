# Directory and HTTP resource inventory

This inventory covers the integrated directory sink and local/HTTP(S) sources.
It reconciles `BATCH_SPEC.md` §§6/10 and `plans/batch.md` §§2.11, 5.1–5.4.
The bound and `bound::PROVISIONAL` remain **provisional estimates**. A component
measurement or a passing whole-process verification is not a calibrated constant.
Store ownership and process coverage remain pending for B11d/B12c; complete
store/stop calibration and final acceptance remain pending for B12c.

## Memory ownership

| Holder and owning modules | Lifetime and accounting |
|---|---|
| Spec, graph, view adjacency and traversal scratch, controller bundle/input state, ready queues, active slots, peek words/bits, extraction/template plans and winner indices (`phase1`, `view`, `attempt/options`) | Constructed inside measured phase 1, retained in `C_spec`. Splitting `Facts` from `Owners` moves storage. No additional bundle coefficient. |
| Parsed URLs, prevalidated header names, both masker reference vectors (`phase1`, `stage/slot`, `stage/env`) | Measured phase-1 capacities, including repeated header references. URL stages borrow them. |
| Main read/parser transients (`spec/read`, `spec/parse`, `phase1`) | Actual phase-1 peak `p`; `C_parse = max(0, p - C_spec - I - C_fin)`. The raw document is dropped before recording retained heap. |
| Private Environment, view walk, staged-path and member descriptor vectors (`worker`, `view`, core `env`) | `C_env`: fixed private state and at most 1 KiB per view member. The walk has three capped `u32` vectors, with its extra traversal entry bounded before insertion. Linux generated paths begin at the retained spool descriptor and stay below 128 bytes, including maximum run ID and bundle index. |
| Core meters, dependency records and retained sources (core `limits/meter`, `env/load`, `env/registry`) | Fixed per-member ownership within the view coefficient; registry slots/keys, parsed data, decode backing and resource lookup keys are separately charged to the attempt. No provider is configured. |
| Dispatch and end channels (`execution`, `worker`, `job`) | Capacity is worker count. One end per dispatch. `m_dispatch`/`m_end` include slot bookkeeping and largest inline failure detail. The end channel's fixed cost is included even at capacity one. |
| Local staging (`stage/local`, `controller/stages`) | One admitted stage envelope per active copy; copy buffer, paths, future, task and file owners stay in the slot until receipt. |
| URL stage (`stage/url`, `stage/slot`, `controller/network`) | Future representation checked against 16 KiB inside the 128 KiB stage envelope. The slot retains its future, task, file, held end and residual owner through stop. |
| Both request tables (`http/permit`, `run/process/network`) | At most `dc` slots per runtime, allocated once before the head. Variable slot/vector ownership belongs to `2(dc+1) m_pool`; it is not hidden in fixed `c_base`. Acquisition adds no slot allocation. The isolated table probe includes `dc=256`. |
| Runtimes, pools and task cells (`lifetime/runtimes`, `execution`) | Two current-thread runtimes, each with exactly `dc+1` prewarmed blocking threads. Current-thread bookkeeping and shared TLS configuration are fixed `C_base`; thread/task increments and tables follow `m_pool`. Both pools have distinct tally names and explicit 2 MiB stacks. Independent gates initialize both real pools and task cells; final coefficient selection still requires the native calibration matrix. |
| Connection and response (`http/connect`, `http/shared`, `http/response`) | Response owns the unspawned hyper driver, TLS state, full backing allocations and permit. The 3.5 MiB transport envelope and charged DATA pieces fit the existing 4 MiB connection allowance; a slice length never replaces backing capacity. Focused transport gates remain independently required. |
| Resolver and moved writes (`http/resolve`, `stage/url`) | Closure owns the operation token until return, including after timeout. Lookup's 512 KiB Rust reservation stays with it. A permit cannot begin a second blocking operation while quarantined. Host copies and addresses use its network share; native resolver heap is separate provisioning. |
| Header values/table (`stage/env`, `stage/headers`) | One owned value/name per distinct variable; values remain live for masking. Exact 25% table growth bounds old/new allocation overlap within existing 48 B table + 32 B spare masker allowances. Validation consumes the original value allocation. Requests use bounded header copies in `C_hdr`. |
| Attempt data, export buffers, records and candidate metadata (`attempt`, core/export) | Charged before allocation to sealed Input/Dependency accounts; slot is released only after zero-charge audit. Disk holds records and candidates. A leaked account keeps admission booked. |
| Sort and winner/member streams (`sort`, `winners`) | Charged phase account of limit `I`; phase 6 separately retains its 40 MiB proof. Run files and merge generations live on disk, with bounded cursor buffers. |
| Publication, resume hashing and ZIP (`publish`, `archive`, `outcomes`) | Dispatcher/task vector, reusable 16 MiB drive buffer and archive scratch charged to phase `I`. Resume hit drops hash scratch before a miss transfer. ZIP central directory, member streams and outcomes live on disk. |
| Frozen work (`run::Parked`, `phase1::Owners`, `finish::Spool`) | Controller, workers and active tasks retain owners and spool leases through process exit. Frozen phase/attempt charges stay booked against `I`; the finisher allocates independently against `C_fin`. |
| Finishing (`finish`, `manifest`, `run/process/threads`) | Separate 24 MiB account, rows streamed directly to sink, no full manifest row collection. The directory frozen-publication scene observes a live 16 MiB buffer through terminal finishing. Complete store finisher certification remains pending. |
| Reset table, lifecycle/stop state, diagnostic ring and slots, progress mailbox/line, preflight head scratch and host-line scratch (`sys`, `lifetime`, `diagnostic`, `progress`, `start`) | Fixed capacities in `C_base`/`C_fin`, as applicable. No history accumulates per request or output. Successful startup warnings use main after controller spawn, preserving independent progress and avoiding an extra detached writer stack. Early failures write warning and error within one terminal writer budget. |
| Startup journal IDs/line, archive resume metadata | Existing named slopes: `dc × 2080 B`, 16 KiB line and `archives × 160 B`. Directory runs do not exercise actual store abort, LIST, credential, signing or multipart holders. |

## Descriptors, threads and address space

On Linux the fixed service subtotal is **13 descriptors**: signal self-pipe 2,
BOOTTIME timer 1, controller/finisher/end wake pairs 6, and two reactors at 2
each. Readiness registrations and the 514-entry reset table add no descriptor.
The fixed 72-descriptor allowance also covers standard streams, progress's
stdout duplicate, spool/root locks, metadata and transient opens, startup spare
descriptors and diagnostic fallback descriptors. Each concurrent connection or
lookup uses the existing `16 dc` share; worker opens use `4 threads`.
`start::host::Requirements::files` remains `4 threads + 16 dc + 72`.

Live normal topology is main, controller, lifecycle, progress, finisher,
`threads` workers, `2(dc+1)` pool threads and optional stdin watch:
`threads + 7 + 2dc + stop_on_stdin_eof`. Terminal early writers precede the
controller or substitute for a thread that failed to start; successful warnings
create no early writer. The host's thread floor and the bound's configured
stacks agree. Test runners, local peers and FIFO drains are excluded.

`Requirements::address` remains `ceil(1.5 bound) + 64 MiB (threads+2) +
3 MiB (dc+1)`. Arena address space, native resolver heap, kernel socket buffers,
dirty pages and allocator retention are provisioning assumptions, not live Rust
heap deductions. Actual inherited environment/argument accounting still uses the
conservative 6 MiB allowance for normal runs and for `--check`.

## Evidence boundaries

`tests/memory_gate.rs` uses fresh supervised children, `alloc::Counting`, the
official tally registrations and the allocation-free testkit ledger. Every
allocation/charge transition contributes to a coherent maximum residual. It
checks live held charges plus applicable uncharged terms separately from the
whole bound. It never adds unused admission capacity to the residual allowance.
An uncharged 64 MiB holder fails that gate while still fitting the whole bound.
Observation code is absent without `test-hooks` and never runs in a signal
handler. Test-only report/hold descriptors are outside the measured heap.

The view proof combines full-capacity worker vectors at 65,536 members with
the core's private-owner structural probe (meter, dependency, two bounded
paths and basename). Real initialized pools and request tables are checked
independently at several pool sizes; maximum table capacity is checked without
requiring hundreds of threads in every PR. Post-staging output/ZIP-member peak
and terminal residual comparisons, and sequential hostname/retry retention
comparisons, check growth independently of the large base estimate. Transport
peaks remain covered by their separate fixed network allowance. Nonmoving
shrinks replace costs atomically; only moving reallocations count two blocks.
Component probes serialize driver lifetimes and assign registrations to one
observation generation; late pool cleanup cannot enter the next probe's ledger.

These Rust-heap checks do not certify native-only allocations or RSS. Native
component tests, allocator retention, representative RSS and cgroup provisioning
remain separate assertions. In particular, native-only charges must not be
subtracted when fitting a Rust-only base coefficient. The aggregate generator
requires explicit safe attribution and incomplete records retain pending labels;
see [the regeneration procedure](README.md).
