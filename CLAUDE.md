# CLAUDE.md

Guidance for agents working in this repo.

## Build and test

Development happens in the `unityrs-dev` container: a persistent satellite built
from a pinned image (`unityrs-oracle:upy1.25.2-2026-10-28`) that holds the parity
oracle (UnityPy 1.25.2 with UnityPyBoost, Pillow 9.5.0, texture2ddecoder 1.0.5,
astc-encoder-py, etcpak) beside the Rust toolchain. Run everything through
`scripts/dev`, which execs as the host user and overrides host-only linker
settings:

    scripts/dev cargo test --workspace
    scripts/dev cargo clippy --workspace --all-targets -- -D warnings
    scripts/dev python tools/parity/...    # the oracle side

Never `docker run` / `docker rm` / `docker stop` anything. The container has a
12 GB memory cap; never remove the cap.

## Rules

- **Never commit game data.** Bundles from commercial games are not ours to
  publish. A corpus is a manifest of URLs plus sha256, kept outside this repo.
  Only synthetic fixtures, or fixtures built with the Unity Editor, are checked
  in.
- **Licences.** UnityPy, UnityPyBoost and AssetStudio are MIT and may be ported;
  keep their notices. Pillow ports carry MIT-CMU. AssetRipper and other
  GPL/AGPL projects are read for format knowledge only; never copy their code.
- **Parity is measured, not assumed.** A behaviour claimed as UnityPy-equal needs
  a test against the pinned oracle. See `docs/DESIGN.md` section 3 for the tiers.
- **Third-party notices.** `THIRD_PARTY_LICENSES` is generated: after a change to
  `Cargo.lock`, `about.toml` or `about.hbs`, run `scripts/third-party-licenses`
  (it says how to install the pinned cargo-about), or CI fails. A port's notice
  goes into `about.hbs`. A new crate under `crates/` links `LICENSE-MIT` and
  `LICENSE-APACHE` to the repository's (`ln -s ../../LICENSE-MIT`), or the
  script refuses its template notice. A licence clarified or accepted for one
  crate in `about.toml` is mirrored in `deny.toml`, which the script checks.
