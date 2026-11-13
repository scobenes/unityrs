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
