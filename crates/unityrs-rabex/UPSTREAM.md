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
