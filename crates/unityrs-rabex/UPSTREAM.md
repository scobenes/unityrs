# unityrs-rabex: rabex 0.2.1, vendored

This crate is [rabex](https://github.com/jakobhellermann/RustyAssetBundleEXtractor)
0.2.1 as published on crates.io, plus the numbered patches below
(`docs/plans/core.md` sections 2.3 and 2.4).

- **Source.** The crates.io tarball `rabex-0.2.1.crate`, sha256
  `d5f8c39f8069c0dc6d01d5435bd21153fda86b2e01af5fea9b5ddfe0716c3d58`. Its
  `src/`, `LICENSE-MIT.md` and `LICENSE-APACHE.md` are here; nothing else of
  it is (no `resources/`, so no TPK, no examples).
- **Ours.** `Cargo.toml` (the package is `unityrs-rabex`, the library keeps
  the name `rabex`) and this file.
- **Licence.** rabex is offered as MIT OR Apache-2.0; unityrs takes it under
  MIT (`license = "MIT"`), so Apache-2.0 section 4(b)'s change notices do not
  apply. Both licence files travel with the crate. Authors, from rabex's
  `Cargo.toml`: Rudolf Kolbe and Jakob Hellermann. `LICENSE-MIT.md` has no
  copyright line, and none is added.
- **Fenced.** unityrs uses the TPK reader (`src/tpk.rs`) through
  `unityrs-tpk` only. The rest is compiled, unchanged, and unreachable from
  every unityrs crate: `crates/unityrs-tpk/clippy.toml` disallows the
  superseded types and functions, and a test fails on any `rabex::` use
  outside `unityrs-tpk`, the P0 spike and this crate.

## The check

`scripts/fork-check.sh` unpacks the pinned tarball from the cargo registry
cache (checking its sha256), checks that the commit of `F0` holds its `src/`
and licence files unchanged, applies every later commit's change to `src/` as
a patch series (`git format-patch --relative`, `git am`), and fails unless the
result equals `src/` here byte for byte. A commit that changes `src/` must be
a ledger patch: its subject starts `rabex <id>: `, and each ledger row below
has exactly one such commit.

## Ledger

Classes: **U** an upstreamable bug fix or additive API; **P** policy
(licence, provenance); **C** UnityPy compatibility, not upstreamable. A row's
commit is named by its subject, which survives a rebase where a hash would
not. "Upstream" is whether the patch has been offered (Q5 of the plan: not
before the owner decides).

| id | commit subject | rabex lines | class | upstream | change, and why |
|---|---|---|---|---|---|
| F0 | `rabex F0: vendor rabex 0.2.1 from crates.io, unmodified` | all | n/a | n/a | The pristine import: the tarball's `src/` and licence files, unchanged. `Cargo.toml` leaves out `embed-tpk` and `resources/`, elects MIT and sets `doctest = false` (the pristine examples import `anyhow` and call `embedded()`). It allows `unexpected_cfgs` until F1 names the feature values. |
| F1 | `rabex F1: name the feature values the source tests that the manifest does not declare` | `Cargo.toml` only | P | not upstreamable | `[lints.rust] unexpected_cfgs` lists the five feature values the pristine source tests and this manifest leaves out (`embed-tpk`, `compression-lz4hc`, `unitycn`, `tpk-compression-brotli`, `serde_path_to_error`; with `compression-lzma` and `compression-lz4` declared, the seven of `grep -rhoE 'feature = "[^"]+"' src`), so those `#[cfg]`s compile without a warning under `-D warnings` and any other unknown cfg still warns. Policy: it follows from this manifest's choices. |
| F2 | `rabex F2: an unknown class has no TPK tree instead of panicking` | `src/tpk.rs:475` | U | not offered | `TpkTypeTreeBlob::get_typetree_node` indexed `class_information` with `[]`, so a class the TPK does not hold panicked; it now returns `None`, as for a class with no entry at the version. Test: `tpk::patch_tests::f2_an_unknown_class_has_no_tree`. |
| F3 | `rabex F3: every panic a TPK file can reach becomes an error` | `src/tpk.rs:127-130`, `:150`, `:158`, `:177`, `:256`, `:306`, `:332-366`, `:431-455`, `:470-527`, `:554-590`; `Cargo.toml` (lz4_flex features) | U | not offered | A TPK file could panic, abort or hang the reader. Now: the header's sizes are refused when negative and read through `take` (no `vec![0; n]` from the file); an LZ4 size past 255 times the block (the most LZ4 expands) is refused before the output is allocated; every count is read as UnityPy's `Tpk.py` reads it, never sizing an allocation (the version list and the common-string indices, struct formats there, refuse a negative count; the rest, `range()` there, read none), so a huge one fails when the bytes run out; the flag byte's `unwrap` is gone (every byte is a flag set); `get_count` drops an `assert_eq!` that panicked on a version list out of order (its forward scan is UnityPy's); the node walk checks every node and string id, holds its level in a `usize` (the `u8` overflowed past 255 levels and then underflowed in `reconstruct_tree`), and stops with an error at 1,024 levels or 65,536 rows, so a node that lists itself or an ancestor ends; `reconstruct_tree` returns an error where it unwrapped; a 7-bit-prefixed string (length up to 2^35 - 1) is read through `take` into a buffer that grows by exactly the bytes read, never pre-allocated at its length; `try_get_typetree_node` and `try_get_typetree_node_for_class` give the walk's error, where the `Option` forms give `None`; lz4_flex's `safe-decode` and `checked-decode` are on, so a corrupt LZ4 block is not decoded unchecked. Tests: `tpk::patch_tests::f3_*`; the string's heap is measured in `unityrs-tpk`'s `tests/string_heap.rs`. |
| F5 | `rabex F5: decompress an LZMA or brotli TPK into an empty buffer` | `src/tpk.rs:158`, `:177` | U | not offered | `TpkFile::decompress` gave the decoder a `vec![0; uncompressed_size]` as its `Write` target, which appends, so the payload followed `uncompressed_size` zeros and parsed as a blob with no versions and no classes, with no error: UnityPy's own `lzma.tpk` read as empty. The buffer now starts empty, with a capacity from the size (after F3 refuses a negative one) capped at 16 MiB. Tests: `tpk::patch_tests::f5_an_lzma_tpk_reads_as_its_payload_not_behind_zeros` (a TPK compressed in code), and `unityrs-tpk`'s G7 test, which reads UnityPy's `lzma.tpk` from the oracle image at test time and compares every class with UnityPy's `Tpk.py`. |
| F6 | `rabex F6: read a class's release root under its own flag` | `src/tpk.rs:311-313` | U | not offered | `UnityClass::read` tested `HAS_EDITOR_ROOT_NODE` for both roots, so a class with only a release root (flags 128) read none and one with only an editor root (flags 64) read two, shifting every read after it: the TPK failed to load or loaded garbage. UnityPy reads each root under its own flag (`Tpk.py:302-305`). UnityPy's own TPK has no one-root class, so this shows only on synthetic TPKs. Test: `tpk::patch_tests::f6_each_root_is_read_under_its_own_flag` (flags 0, 64, 128 and 192, each followed by a class with both roots), and G7's synthetic set in `unityrs-tpk`. |
