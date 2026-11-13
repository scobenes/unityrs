# Batch sort and committed-reader contract

`unityrs-batch` supplies the bounded infrastructure for phase 6. Projection
comparators and later decision passes supply the selection semantics. The full
phase remains subject to the retained-artifact and projection-size proofs in
`plans/batch.md` section 2.9; this infrastructure alone establishes neither.

## Allocation and failure handling

The controller prepares the `Account` and `Workspace` before entering the phase
execution boundary. Within a sort, fixed data buffers charge their exact
`heap_cost` before `try_reserve_exact`. A separate 64 KiB overhead charge covers
transient paths and file adapters. Workspace basenames use a stack buffer for
`f` plus up to twenty decimal digits; the complete `PathBuf` reserves the
directory, separator and basename fallibly before pushing either component.
No per-file formatting or path join allocates on the phase path.

Unix file operations also prepare their NUL-terminated paths fallibly, including
long paths that would cause std's path adapters to allocate. The spool retains
exclusive creation, no-follow opens, regular-file validation and explicit
unlink, large-file support on 32-bit Linux, and retries of interrupted opens.
A sparse-file regression in `tests/spool.rs` checks length, open and unlink above
2 GiB without writing a large payload. Boxed file adapters allocate fallibly through `alloc.rs`; system calls
remain in `sys.rs`. Refusal drops temporary storage and any opened descriptor,
returns `host_memory` and retains existing disk claims. No deletion is attempted
as compensation for a failed operation.

The phase audit catches allocator refusals even if a caller absorbs an error.
A leaked charge, panic or phase-account refusal still follows the existing
severity precedence. `sort/`, `records/read.rs` and `phase.rs` use no other
infallible allocation within the execution boundary; the account's shared
owners are initialized before it.

`tests/sort_alloc.rs` runs counted children. It refuses the first allocation
after `BeforeSort`, each allocation ordinal in a successful multigeneration
sort with short and long workspace paths, and each committed-reader buffer
allocation. Every refused allocation must finish as `host_memory`, request the
sort's stop where applicable, consume the refusal and leave no scratch charge.

## Framing and canonical order

Private artifacts have a 16-byte magic/version/tag header and frames with an
inclusive little-endian `u32` length, at most 64 KiB including that prefix.
Zero remaining bytes mean clean EOF. One to three remaining bytes already
prove truncation and return `internal` before attempting a prefix read; an
injected I/O failure on that unnecessary read must not change classification.

Semantic equality is completed by canonical payload bytes. Only identical
payloads may use a run-index tie. `tests/sort.rs` pins complete artifact bytes
for equal semantic keys with different payloads and repeated identical records,
in both input directions, at run cuts 1, 2, 3 and 8 and fan-ins 2, 3 and 16.
Removing the canonical-byte tie-break must fail this test. Independent `/`-first
vectors remain separately pinned.

## Committed image modes

The typed reader accepts every image mode in `BATCH_SPEC.md` section 7.3:
`L`, `LA`, `RGB` and `RGBA`, preserving the mode and visible digest. Its accepted
vocabulary follows the spec regardless of which modes a current producer emits.
`tests/records_read.rs` covers all four modes through the terminal summary and
EOF. The existing null/digest relationship and invalid-mode checks still apply.

## Unit, key and prefix handoff

`winners::Prepared` keeps preference/input indices and `/`-first archive
indices in phase-1 measured memory, including `--check`. `winners::Inputs`
borrows a frozen set of committed artifacts: every numeric position appears
exactly once and has its final success/failure fact. The test adapter opens
synthetic B4 commits with the same fallible filesystem operations. The concrete
controller adapter is B6c's integration work.

`winners::steps123` is the structural X6 boundary. `winners::run` adds exactly
one prefix pass, then unit propagation, and returns fixed capabilities:

| capability | order | contents |
|---|---|---|
| `units` (`units.decisions`) | bytewise unit key | chosen root/input, E31 rule, E61 or whole-unit conflict state |
| `keys` (`keys.decisions`) | bytewise key | representative metadata and numeric locator, state, E31 rule, flip, total candidate count |
| `winners` (`winners.bin`) | bytewise key | structurally publishable representatives, with complete source/digest metadata |

Names describe logical streams; workspace-generated basenames remain private.
Empty streams use no file. Projection tags 101–106 identify key/input,
instance, unit/member, key/unit, unit/key and `/`-first orders. They share v1
sort framing. Frame payloads begin with checked rule/state/boolean/kind/mode
tags, little-endian numeric fields and digests with explicit optional tags,
then four `u16`-length UTF-8 fields: key, unit, input, serialized file. The first
three allow 1,024 bytes, the last 3,072 for repaired source text. No spool path
is encoded. A blank unit denotes a plain candidate. Root decisions retain
their E31 rule independently of withholding; B6c derives each candidate's
collision rule by joining unit state or the plain-key decision.

`Artifacts::counts` counts collision keys and flip keys, including later
refusals. `publication_state` applies `publish_incomplete` only to otherwise
publishable states; it never changes structural winners or membership.

The committed-source reader allocates all parser buffers before the sort's
borrowed-source audit. Reopening the next committed input reuses that storage;
allocation failure or cancellation cannot leave a newly charged reader inside
the audit baseline. Unordered marker outputs use a crate-private framed reader
that checks schema/framing but defers order validation to their external sort.
Every final stream uses the ordered reader.

Archive keys merge directly from prepared indices into the original sorted
publish set, archive first on equality. Refusals do not change lookahead.
Affected-unit markers are sorted once, then joined back; no loser is promoted
and no second prefix pass runs. The artifact lifetime proof and measured peaks
are in `plans/batch.md` 2.9; row/member materialization remains B6c's.
