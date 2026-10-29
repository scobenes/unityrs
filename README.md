# unityrs

Fast Unity asset extraction in Rust: asset bundles, serialized files,
typetrees, textures, sprites, meshes, animation and glTF export, as a library,
a command-line tool and Python bindings.

**Status: pre-alpha.** Nothing here is usable yet. The plan is in
[`docs/DESIGN.md`](docs/DESIGN.md).

## Goals

- **Parity with [UnityPy](https://github.com/K0lb3/UnityPy).** Every behaviour is
  tested against UnityPy, pinned to an exact version and native stack, and sorted
  into tiers: bit-exact where UnityPy is right, correct where it raises or is
  wrong, and documented either way.
- **Speed through batching.** Each bundle and dependency is parsed once, the work
  is decoded and encoded in parallel, and memory stays bounded.
- **Any game.** Version detection, a typetree-provider chain for stripped files,
  and hooks for encryption, altered headers and custom compression.
- **3D export.** Prefabs, skins, Mecanim animation and scenes to glTF/GLB.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT licence](LICENSE-MIT) at your option. Code ported from other projects keeps
its original notices. [`THIRD_PARTY_LICENSES`](THIRD_PARTY_LICENSES) collects
them, with the licences of the crates unityrs builds on.

unityrs is not affiliated with or endorsed by Unity Technologies. "Unity" is a
trademark of Unity Technologies.
