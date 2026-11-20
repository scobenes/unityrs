# Aggregate memory calibration

The bound in [batch.md §5.3](../plans/batch.md#53-the-constant-terms) uses
calibrated constants from the checked-in [aggregate evidence](constants-v1.json).
The collector measures the raw terms. The regeneration tool validates the
aggregate records and derives the compiled selection.

`tools/memory/regenerate.py` uses Python's standard library. Its input and output
formats are [aggregate-input-v1.schema.json](aggregate-input-v1.schema.json) and
[aggregate-output-v1.schema.json](aggregate-output-v1.schema.json). Every object
rejects unknown members. Duplicate JSON members, nonfinite numbers, booleans in
numeric fields, negative values and integers exceeding `u64` are refused.
Additional cross-record and arithmetic checks described below run after schema
validation. The output is validated again before serialization.

## Input records

The root has exactly `schema_version` (1), `status` (`provisional-b12b` or
`final`) and `records` (one through sixteen records). Each record has:

| Member | Meaning |
|---|---|
| `kind` | `synthetic` or `whole-run` evidence |
| `target` | `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` |
| `profile` | `debug` or `release` |
| `sink` | `directory` or `store` |
| `provenance` | Structured build and native host configuration |
| `coverage` | Required booleans for the synthetic catalogue; all false on whole-run records |
| `whole_run_coverage` | Closed facts required on whole-run records; true as applicable in final mode |
| `coefficients` | Raw maximum bytes for all six empirical coefficients |
| `structural_floors` | Measured representation floors for `m_dispatch` and `m_end` |
| `fixed_envelopes` | Certified upper bounds, overlaid with protocol observations where emitted; bytes or holder counts |
| `attribution` | Required attestations about the measurements' accounting |

The kind/target/profile/sink axes must be unique. Coefficients are `c_base`, `c_env`, `m_thread`, `m_pool`,
`m_dispatch` and `m_end`. Structural floors include the actual message
representation, its owned heap and channel bookkeeping. The end floor must be
at least 3,328 bytes: its bounded 3,072-byte detail plus the existing 256-byte
slot allowance. This minimum does not replace measuring the complete union or
full-channel storage, including fixed channel overhead at small capacities.

Provenance accepts no free-form labels. `revision` is a lowercase 40-digit Git
object ID; `lockfile_sha256` and `native_libraries_sha256` are lowercase 64-digit
hashes. `rustc` and `kernel` are three-integer version arrays; `glibc` is a
two-integer version array. `native_libraries_sha256` identifies a canonical
configuration manifest of library versions and build options, shared across the
targets; it is not a hash of target-specific executable bytes. Keep the full
configuration manifest with the measurement evidence for review.

All records must agree on revision, lockfile, Rust and C/C++ compiler versions,
native-library configuration, native build flags and feature set. Host kernel
and glibc versions may differ between native hosts. Features must be `test-hooks`; the allocator must be
`system-glibc`; `native` must be true; `page_size` must be 4,096;
`mmap_threshold` must be 131,072; `arena_extra` must be 2, recording production
`M_ARENA_MAX = threads + 2`. Emulated measurements and other page sizes are not
eligible, including in provisional aggregate evidence.

Attribution requires `base_workers = base_download_concurrency = 1`. All other
attribution members must be true: deductions use held charges and live sizes,
use raw incremental coefficients, exclude native-only charges from Rust heap
deductions, use allocation-free observers and exclude peers, include parent-side thread startup,
and use coherent observations. For `c_base`, take the maximum residual over
startup, normal execution and terminal paths in the prescribed scene. Never
subtract unused reservations, padded selected coefficients, network/header/
credential/finisher caps or unseen native heap. Both runtimes' initialized pool
threads and outstanding task cells require independent probes.

Whole-run records combine a directly observed zero-deduction one-worker,
one-download base with the five independent component increments, representation
floors and fixed-envelope certificates from the matching synthetic axis. They
must not understate that axis's component/floor/envelope evidence. When the
same build's synthetic axis was also measured on another host, each component
coefficient, floor and envelope is the maximum over every matching
target/profile/sink record of that build. `c_base` stays the directly observed
whole-run base. This is explicitly mixed evidence, not six slopes fitted from a
corpus run.
`whole_run_coverage` names both-builds, one-worker, eight-workers, resume, loose,
archive, phase-seven and component-probes, plus store-roundtrip for a store
record. All are required true in final mode, with store-roundtrip false and
inapplicable for directory records. A real run's full peak or RSS cannot replace
component isolation, full-capacity synthetic scenes or their scale arguments.

The schema lists the complete closed synthetic scene vocabulary. Set a synthetic
flag only when its measurement and scale/ownership argument have been reviewed.
Store flags are required for synthetic store records in final mode; other
synthetic scene flags, including stopping in every phase, are required for every
final synthetic record. Whole-run records require their separate closed coverage
object in every mode and must keep every synthetic flag false. A flag describes coverage,
not a path, test log, report attachment or arbitrary input identifier. Publish
only the structured aggregates. Input identities and detailed reports stay
outside this repository and CI artifacts. Validation diagnostics do not echo
rejected labels or file paths.

## Selection and fixed envelopes

For the maximum raw observation `r` across eligible records:

```text
margin   = max(ceil(r / 4), minimum_margin)
selected = round_up(max(r + margin, structural_floor), quantum)
```

| Coefficients | Minimum margin | Quantum |
|---|---:|---:|
| `c_base`, `c_env`, `m_thread`, `m_pool` | 65,536 bytes | 4,096 bytes |
| `m_dispatch`, `m_end` | 64 bytes | 64 bytes |

The output records raw maximum, margin, structural floor and selected value
separately. All addition and rounding must fit `u64`; overflow is an error.
For example, `r = 262145` with the 64 KiB margin minimum selects 331776 bytes
with a 65537-byte margin. A message observation of 257 bytes selects 384 bytes
with a 65-byte margin, unless its structural floor requires more.

Fixed envelopes are retained without padding or fitting. The schema's
`fixed_envelopes` keys correspond to: 4 MiB connection state; 24 MiB finishing;
1 KiB per view member; 48-byte header table and 32-byte masker entries; four
header copies per download slot; six credential holders with 128/128/16384-byte
credential caps; 2 MiB multipart lists; 64 KiB response collection; 115344-byte
worker state; 2048-byte panic slots; 2080-byte abort pairs; 16 KiB journal lines;
and 160-byte archive resume entries. The collector retains certified ceilings
unless a numeric envelope observation is emitted. Units are holder counts for `header_copies` and
`credential_holders`, bytes for the remaining keys; the current protocol emits
only the credential-holder count. A breach in any mode is refused and requires a bound revision. Moving the excess into
`c_base` does not resolve it.

## Provisional and final generation

`provisional-b12b` permits an incomplete native matrix and incomplete coverage.
It always emits `shipping_replacement: false` with `store-calibration`,
`stop-calibration` and `final-acceptance` pending. It also identifies missing
native matrix or other scene coverage. Its selected values are review evidence,
and cannot produce a Rust replacement. A zero raw coefficient in a partial
record can represent no observation only with its coverage flag false; it must
not be presented as a complete calibration.

`final` requires all eight native synthetic target/profile/sink combinations,
matching native whole-run aggregates for both profiles and sinks on at least
one architecture, and complete applicable coverage of the store and stop
scenes. The generator emits `MEASURED: Constants` with the fixed 4 MiB connection
envelope. Generate at the compiled selection path to integrate a candidate,
then verify it with the independent gates before accepting it.

1. Check out the exact integrated revision and locked dependencies. Apply the
   production allocator configuration before measuring.
2. Run synthetic component and whole-engine gates on both native architectures,
   debug and release, with both sinks. Include startup, full channels, maximum
   paths/views, both initialized pools, store paths and every stopped phase.
3. Reconcile coherent heap/charge observations, independent coefficient probes,
   representation floors and count-independent residuals. Retain native heap,
   RSS and resource provisioning evidence separately from charge-unit constants.
4. Review the structured aggregate records and coverage, then generate explicitly.
5. Re-run independent X12/X13 and native/RSS checks against a final candidate.
   Review the generated evidence, formula and documentation together before
   integrating any constant replacement.

Run Python only in the persistent development container. With an input aggregate
and writable output path already prepared, use:

```sh
docker exec -w /work unityrs-dev \
  python tools/memory/regenerate.py /tmp/aggregate-input.json \
  --output /tmp/aggregate-output.json

docker exec -w /work unityrs-dev \
  python tools/memory/regenerate.py /tmp/aggregate-input.json \
  --output /tmp/aggregate-output.json --check
```

For complete final evidence only, add `--rust-output /tmp/measured.rs` to either
command. Output directories must already exist. Input and output paths must be
distinct, including symbolic or hard links. Every payload is validated before
writing starts. Writes replace the explicitly named files; a filesystem failure
is reported and may require rerunning generation. `--check` reads existing
files without creating directories or rewriting stale outputs. Exit codes are
0 for success, 1 for stale/missing output, and 2 for invalid input or file errors.
Serialization uses sorted JSON keys and sorted target/profile/sink records, with
one trailing newline; record order in the input does not affect output bytes.

No generation runs during `cargo build`, and CI checks never regenerate files.
Changes to toolchain, dependencies, runtime topology, channel representations,
header/masker/store ownership, native libraries or the charge unit require
revalidation. A fixed-envelope breach requires review before enabling the
affected behavior.

The dependency-free test suite runs through the existing test-count gate:

```sh
docker exec -w /work unityrs-dev \
  python scripts/unittest-suites tools/memory/tests=59
```

## Native collection and compiled selection

The calibration workflow runs the synthetic matrix on native
`ubuntu-24.04` (x86_64) and `ubuntu-24.04-arm` (aarch64), in debug and release.
Each job runs in Debian 12 (bookworm), pinned to the multi-architecture image
index digest in the workflow; the native runner pulls its own architecture.
This matches the host toolchain used for private whole-run aggregate evidence:
GCC/G++ 12.2.0 and glibc 2.36. Running a container on a native host is still
native execution. Both `cc -dumpfullversion` and `c++ -dumpfullversion` must
report exactly `12.2.0` before measurement. All synthetic and whole-run records,
across both architectures and profiles, must agree on `native_compilers`.
The jobs install Debian build dependencies without recommended packages, verify
the architecture-specific published SHA-256 of the pinned rustup installer,
and install the toolchain from `rust-toolchain.toml`. No ambient build or
allocator overrides or external Cargo configuration are supplied.

The measured targets inspect their own and their children's `/proc` entries,
which default Docker containers expose. They do not require ptrace or create
user/PID namespaces; the separate `lifetime_process` namespace test is outside
the collector's target list. No additional container capabilities or security
options are needed.

The workflow checks architecture and 4 KiB pages, builds before measuring, runs scenes
serially, and uploads exactly one aggregate JSON per job. Child output is
parsed in memory; no measurement logs or traces are uploaded. The workflow
checks the PR head revision, so all eight sink records describe the same source.
`workflow_dispatch` also supports explicit revalidation.

`collect.py` runs the component, integrated process, native component and
retention gates. Its closed numeric protocol supplies all six raw empirical
coefficients and both representation floors. Each coverage flag names actual
tests whose discovery is checked. Both sinks receive the conservative maximum
over the joint catalogue. The base probe deliberately deducts **nothing**:
charged buffers, network state, credentials, startup and finisher overlap
remain in its raw value. This conservative duplication is visible and cannot
manufacture headroom by subtracting a cap, an unused reservation or unseen
native heap. Worker probes include channels, startup and initialized jobs;
pool probes include both initialized pools and both request tables. Fixed
envelopes retain their source/structural bounds and separate component gates.
The credential provider moves the strings into one shared owner; all four
store clients and the masker borrow it. The six-holder allowance remains.

Before local collection, wait until both commands find no process (status 1):

```sh
docker exec unityrs-dev pgrep controller_allo
docker exec unityrs-dev-arm pgrep controller_allo
```

Run one collection at a time, with no allocation sweep or heavy build beside
it. The collector checks its own PID namespace too. Invoke it inside the
existing Linux container, from the checkout, with the usual user, Cargo and
worktree environment configured:

```sh
python tools/memory/collect.py --native --profile debug --output /tmp/arm-debug.json
python tools/memory/collect.py --native --profile release --output /tmp/arm-release.json
```

On an emulated host, replace `--native` with `--emulated`. `--allow-dirty`
permits development diagnostics. Such records retain `native: false` or
`clean: false` respectively, are schema-shaped, and are always rejected by
the generator. They cannot replace normative native workflow records. Native
execution is an explicit runner attestation; architecture alone cannot detect
emulation. The native-library fingerprint hashes the locked graph, toolchain
and sorted native source/configuration files, rather than executable bytes.

Records now have the closed `kind` field: `synthetic` or `whole-run`. Final
selection requires all eight native synthetic target/profile/sink axes and
native whole-run aggregates for both profiles and sinks on at least one architecture, with complete applicable coverage and
matching build provenance. A whole-run record cannot substitute for a missing
synthetic axis. Up to sixteen distinct kind/target/profile/sink axes are
accepted. Only the aggregate crosses this interface; detailed run reports stay
outside the checkout and artifacts.

Use `status: "provisional-b12b"` for each partial input document, including
a standalone whole-run matrix. `--final` validates their combined evidence;
a document already marked `final` must itself contain the complete matrix.

Download the four workflow artifacts into separate directories under a records
directory. Supply the independently produced native whole-run input through
`WHOLE_RUN_AGGREGATE`, then generate and verify explicitly:

```sh
python tools/memory/regenerate.py /tmp/records/*/aggregate.json "$WHOLE_RUN_AGGREGATE" \
  --final --output docs/memory/constants-v1.json \
  --rust-output crates/unityrs-batch/src/bound/measured.rs
python tools/memory/regenerate.py /tmp/records/*/aggregate.json "$WHOLE_RUN_AGGREGATE" \
  --final --output docs/memory/constants-v1.json \
  --rust-output crates/unityrs-batch/src/bound/measured.rs --check
cargo test --locked -p unityrs-batch --features test-hooks \
  --test memory_gate --test store_memory_gate --test memory_constants --test limit_squeeze
cargo test --locked --release -p unityrs-batch --features test-hooks \
  --test memory_gate --test store_memory_gate --test memory_constants --test limit_squeeze
```

The checked-in `bound/measured.rs` selects `MEASURED`, with `CALIBRATED = true`.
The `--bootstrap` mode can generate a selection of `PROVISIONAL` with
`CALIBRATED = false` when developing without final evidence. Generation is
explicit. Execution and CLI `--check` share the saturating calculator and
`SELECTED` with the verification gates. Hosts with an unknown page size or a
page size other than 4 KiB report the bound as uncertified on stderr.

Build provenance also records numeric C/C++ compiler versions per host; both
must match across all synthetic and whole-run records. The shared native
configuration digest covers source and locked options, including C++ headers
and assembly. A second shared digest identifies the default locked profiles.
The collector rejects ambient native/Rust/Cargo build overrides, test-driver controls, external
Cargo configuration and allocator interposition/tuning. Compiler target triples
are verified against the host architecture rather than hashed into a digest
that must match across architectures.

The attribution field `observer_allocation_free` pins the fixed, allocation-free
measurement ledger. Fixture/report/hold storage is prepared outside the window.
Engine code compiled under `test-hooks` still owns bounded points/resolver and
per-slot bookkeeping allocations. Those remain in the raw maxima conservatively;
they are never deducted as if they were measured production ownership. The
record's feature provenance describes that graph. This separates observer
overhead from the retained test-hook footprint without an inaccurate exclusion
attestation or a subtraction of unrelated peaks.

CI checks the committed evidence against the compiled file on every
calibration job. The check is read-only.

```sh
python tools/memory/regenerate.py --compiled-evidence docs/memory/constants-v1.json \
  --rust-output crates/unityrs-batch/src/bound/measured.rs --check
```

For a development diagnostic, add `--smoke --allow-dirty`. It runs active
correctness/component scenes and leaves scale-only coverage false. Ignored
tests never supply coverage. These records remain ineligible for final
selection; their native/emulated and source-cleanliness labels are preserved.

The CLI observer exists only with `unityrs-cli/test-hooks`. Explicitly setting
`UNITYRS_TEST_WHOLE_MEMORY=1` installs charge/phase observation and enables a
single report at the normal terminal Exit point. Other values produce no
report. The feature-enabled allocator tracks all Rust allocations from process
initialization, including CLI setup, argument ownership and parent-side thread
startup. It uses two fixed allocation-free ledgers, with no registration filter,
heap walk, history or sampling. Growing reallocations retain conservative
old/new overlap; cross-thread frees balance in the process total. Setup ownership
is retained in the base, with zero deductions. Before preparation, the opt-in
observer establishes the normal allocator settings for one worker; normal run
preparation subsequently applies the prepared worker count.

Reporting formats decimal numbers on the stack and writes to stderr, which the
caller must drain or redirect to an already-open regular file. Instrumentation
adds fixed atomic state, short ledger critical sections and bounded setup
ownership. It changes scheduling and can affect RSS. Native heap, other
processes and forced-kill/crash paths are not certified by this observer.

For workers=download-concurrency=1 it emits
`B12_COEFFICIENT c_base <peak>`; other concurrency settings emit no coefficient.
Every report also contains exactly one newline-terminated line:

```text
WHOLE_MEMORY_V1 peak events phase-mask workers download-concurrency network heap-negative heap charged-negative charged residual-negative residual
```

All twelve fields are unsigned u64 decimal integers. The three negative fields
are sign bits (0 or 1), each followed by its magnitude. A complete run has phase
mask 255 (Validate through Finish) and a positive event count. `peak` is the
zero-deduction Rust heap maximum; `network` is the outstanding network reservation.
Heap, held charges and maximum residual are diagnostic observations from the
second ledger. That ledger includes native-only charges: its residual must never
be used to fit a Rust coefficient. The two ledger snapshots are separate and
must not be combined algebraically. `tools/memory/collect.py` exposes
`whole_memory_protocol` to reject missing, duplicate, truncated, malformed or
inconsistent reports. It verifies prepared concurrency and complete phase
coverage, positive peak and representable nonnegative ledger fields; successful exit, clean matching revision, component evidence, corpus
completion and output correctness remain separate eligibility requirements.
