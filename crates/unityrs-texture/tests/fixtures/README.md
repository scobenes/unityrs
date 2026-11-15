# Texture decoder fixtures

All inputs are synthetic. Fixture writers live in `tools/fixtures/` and accept
`--check` to regenerate and compare without updating files.

| File | Generator | Reference |
| --- | --- | --- |
| `decode.json`, `raw.json`, `etc.json`, `select.json` | `texture.py`, with `texture_raw.py` and `texture_etc.py` | UnityPy 1.25.2, Pillow 9.5.0, texture2ddecoder 1.0.5 |
| `astc.json` | `texture_astc.py` | UnityPy 1.25.2, Pillow 9.5.0, astc-encoder-py 0.1.12 |
| `sprite.json`, `sprite_solve.json` | `sprite.py`, `sprite_solve.py` | UnityPy 1.25.2, Pillow 9.5.0 and the pinned numpy/OpenBLAS stack |
| `crunch_native.json` | `texture_crunch.py --native crates/unityrs-texture/tests/fixtures/crunch_native.json` | Original in-code CRN writer and independently constructed BC blocks; no encoder or oracle dependency |
| `crunch.json` (optional until generated with the pinned stack) | `texture_crunch.py` | Synthetic serialized Texture2D files read through UnityPy 1.25.2's `.image`, Pillow 9.5.0 and texture2ddecoder 1.0.5 |

Legacy Crunch covers DXT1 and DXT5, palette sizes 1, 3 and 17, 1×1 and odd
sizes, all eight chunk layouts, reservoir refill, serpentine traversal, skipped
edge blocks, endpoint and selector delta wrapping, singleton and long canonical Huffman codes,
and all four Huffman code-length run forms. Fourteen textures contain 32 mip
levels. Additional regression groups cover truncated regions, under-declared data
sizes, and four 1×1 streams whose skipped colour or alpha selectors temporarily
leave a singleton palette. The coordinator records the new cases with the pinned
oracle on regeneration. Legacy Crunch uses static Huffman models; its layout reservoir and delta
indices carry state. Unity's newer Crunch codec is outside this fixture's scope.

Regenerate the dependency-free fixture:

```sh
python3 tools/fixtures/texture_crunch.py --native crates/unityrs-texture/tests/fixtures/crunch_native.json
python3 tools/fixtures/texture_crunch.py --native crates/unityrs-texture/tests/fixtures/crunch_native.json --check
```

Record or check the pinned oracle fixture:

```sh
scripts/dev python tools/fixtures/texture_crunch.py
scripts/dev python tools/fixtures/texture_crunch.py --check
```

`crunch_oracle.rs` always replays the known BC blocks. The optional UnityPy replay
prints a clear skip message if `crunch.json` is absent; this is not a claim of
measured UnityPy pixel parity. The wrapper uses Unity 2017.2 to select the legacy
codec and stores deterministic JSON with `ensure_ascii=False`.
