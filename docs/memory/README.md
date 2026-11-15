# Aggregate memory calibration

The bound in `docs/plans/batch.md` section 5.3 remains provisional. The
regeneration tool validates public aggregate evidence; it does not collect
measurements or certify that submitted observations are authentic. No measured
constant set is installed by this change.

`tools/memory/regenerate.py` uses Python's standard library. Its input and output
formats are [aggregate-input-v1.schema.json](aggregate-input-v1.schema.json) and
[aggregate-output-v1.schema.json](aggregate-output-v1.schema.json). Every object
rejects unknown members. Duplicate JSON members, nonfinite numbers, booleans in
numeric fields, negative values and integers exceeding `u64` are refused.
Additional cross-record and arithmetic checks described below run after schema
validation. The output is validated again before serialization.

## Input records

The root has exactly `schema_version` (1), `status` (`provisional-b12b` or
`final`) and `records` (one through eight records). Each record has:

| Member | Meaning |
|---|---|
| `target` | `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` |
| `profile` | `debug` or `release` |
| `sink` | `directory` or `store` |
| `provenance` | Structured build and native host configuration |
| `coverage` | Required boolean for each synthetic scene ID in the schema |
| `coefficients` | Raw maximum bytes for all six empirical coefficients |
| `structural_floors` | Measured representation floors for `m_dispatch` and `m_end` |
| `fixed_envelopes` | Maximum observed bytes or holder counts for each fixed allowance |
| `attribution` | Required attestations about the measurements' accounting |

The axes must be unique. Coefficients are `c_base`, `c_env`, `m_thread`, `m_pool`,
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

All records must agree on revision, lockfile, Rust version, native-library
configuration and feature set. Host kernel and glibc versions may differ between
native hosts. Features must be `test-hooks`; the allocator must be
`system-glibc`; `native` must be true; `page_size` must be 4,096;
`mmap_threshold` must be 131,072; `arena_extra` must be 2, recording production
`M_ARENA_MAX = threads + 2`. Emulated measurements and other page sizes are not
eligible, including in provisional aggregate evidence.

Attribution requires `base_workers = base_download_concurrency = 1`. All other
attribution members must be true: deductions use held charges and live sizes,
use raw incremental coefficients, exclude native-only charges from Rust heap
deductions, exclude instrumentation and peers, include parent-side thread startup,
and use coherent observations. For `c_base`, take the maximum residual over
startup, normal execution and terminal paths in the prescribed scene. Never
subtract unused reservations, padded selected coefficients, network/header/
credential/finisher caps or unseen native heap. Both runtimes' initialized pool
threads and outstanding task cells require independent probes.

The schema lists the complete closed scene vocabulary. Set a flag only when its
measurement and scale/ownership argument have been reviewed. Store flags are
required for store records in final mode; other scene flags, including stopping
in every phase, are required for every final record. A flag describes coverage,
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
and 160-byte archive resume entries. Report observed holder counts for
`header_copies` and `credential_holders`, bytes for the remaining keys. A breach
in any mode is refused and requires a bound revision. Moving the excess into
`c_base` does not resolve it.

## Provisional and final generation

`provisional-b12b` permits an incomplete native matrix and incomplete coverage.
It always emits `shipping_replacement: false` with `store-calibration`,
`stop-calibration` and `final-acceptance` pending. It also identifies missing
native matrix or other scene coverage. Its selected values are review evidence,
and cannot produce a Rust replacement. A zero raw coefficient in a partial
record can represent no observation only with its coverage flag false; it must
not be presented as a complete calibration.

Store process coverage remains pending for B11d/B12c. Complete store/stop
calibration, the final native matrix and acceptance remain pending for B12c.

`final` requires all eight native target/profile/sink combinations and complete
applicable coverage. It is reserved for the complete store and stop calibration
and review. The generator can then emit a candidate `MEASURED: Constants`, with
the unchanged 4 MiB connection envelope. Generating that candidate does not
integrate it into the build or finish acceptance.

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
  python scripts/unittest-suites tools/memory/tests=33
```
