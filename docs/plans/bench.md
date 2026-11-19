# Benchmark plan: unityrs against UnityPy

**Status:** proposed, 2026-11-19, revised after review round 1, with the owner's decisions of section 10. Nothing is built yet. Section 9 reports the feasibility spike behind decisions 6 to 9. The owner asked for the following:

> once we have a shippable version, i want you to create benchmark suite that can be run to compare unityrs and unitypy and put summary on the main readme, and then bigger breakdown on a benchmark file. also we should record the numbers so that they can be reran and part of the tests so we can catch any regressions

The owner then set when it happens: figures go public with the first benchmark PR after the current merge train (BN-1, section 8).

**What it delivers.**
- One command reruns a like-for-like comparison of unityrs and UnityPy 1.25.2 with UnityPyBoost on public inputs.
- A results file records the machine, the versions and every raw run.
- A README summary and a `BENCHMARKS.md` breakdown are generated from that file, and a test checks them.
- A deterministic CI gate fails a PR that moves unityrs's instruction counts beyond ±1% in either direction, with an explicit "refresh the baseline" message.

**What it builds on.**
- The corpus perf report (`tools/perf/`, `docs/plans/core.md` sections 6, 8 and 10): `perf.py`, `bench_unitypy.py` with its `stack_problem` check of the pinned oracle, `mem_peak.py`'s summed-PSS sampler with the load beside it, and `report.py`.
- `unityrs-dump`'s `bench` example, its `--many-blocks` runs and its per-thread CPU clock.
- The testkit's builders (`unityrs-testkit`: bundles in every codec, serialized files with embedded trees, `ngui::texture`, `scene3d` meshes and clips, `fsb5::bank`, the crunch builder).
- The oracle pins (`tools/parity/requirements-oracle.txt`, `oracle_env.py --exact`).
- The UnityPy-compatible wheel (`crates/unityrs-py`, module `unityrs`).

All of that stays as it is. The corpus report remains a private local check, and nothing it measures is published.

**Citations.** UnityPy behaviour is cited by its `PARITY.md` row. Repository paths are relative to the repository root.

---

## 0. Decisions

| # | decision | why |
|---|---|---|
| 1 | **Two measurements, kept apart.** The *comparison* measures wall time, CPU time, peak memory and throughput for both tools, records them and publishes them. It never fails a PR. The *gate* counts instructions (callgrind `Ir`) for unityrs only, against committed baselines, and fails CI. | Wall clock on shared runners is too noisy to gate on (section 5.1). Instruction counts are deterministic, but they mean nothing across languages. |
| 2 | **Public inputs only.** The published figures come from the `bench-v1` fixture set, which `unityrs-bench gen` builds from the testkit's builders with fixed seeds (section 2). The owner's Editor-built fixtures join later (decision 12). The private corpus is never an input to a committed file or a published figure, and it is never named. UnityPy's own test samples (`tools/parity/unitypy_samples.json`) are game bundles, so they are excluded too. | The repository is public, and every published figure must be one anyone can reproduce. |
| 3 | **Fixtures are generated, not committed.** `benchmarks/fixtures.json` commits each file's name, size and sha256, plus the generator's seed and scale. A test regenerates the gate-scale set and checks it against the manifest. The full set (at most about 490 MB, section 2.2) is cached outside the checkout. | The set is too large to commit, and generation is deterministic. The manifest keeps a rerun honest. |
| 4 | **Only comparable work is published as a speedup.** A row is *paired* only when three things hold. Its `PARITY.md` row is Tier A for the operation timed (for a texture, its section 17 row). Both tools complete it with zero failures. Their outputs have identical digests in that run. Anything else is published only in `BENCHMARKS.md`'s unityrs-only table, with the reason. That covers formats or classes UnityPy refuses, such as BC6H (Tier B: UnityPy raises), and Tier B or C outputs. Every paired total and every count in the README (formats, pixels, objects, files) is computed by `render` from the matched rows of that run, never written by hand. | A faster wrong answer, or a run against a refusal, is not a speedup. Equal digests prove that both sides did the same work. |
| 5 | **Three unityrs columns, one UnityPy column.** The comparison runs (a) the `unityrs` Python wheel under the *same* Python workload script as UnityPy (`import unityrs as UnityPy`), (b) the CLI for whole extraction (`unityrs extract` against `UnityPy.tools.extractor.extract_assets`, section 4.2) and (c) the Rust library through `unityrs-bench run`. The README uses (a) and (b). (c) appears in `BENCHMARKS.md` only. There is no parallel headline until both sides have an entry point with the same dependency visibility, output ownership and job count. Today `unityrs extract` has no `-j`, and a one-bundle-per-task UnityPy pool would resolve cross-bundle references differently. | (a) is what a UnityPy user would switch to, line for line. (b) is the batch user's path. A parallel row now would compare different work. |
| 6 | **The gate metric is callgrind `Ir`.** The gate uses valgrind 3.19.0 (Debian bookworm's `valgrind_3.19.0-1_amd64.deb`, pinned by sha256 and fetched from snapshot.debian.org). It runs inside `debian:bookworm` pinned by digest, with every apt package from a dated snapshot.debian.org mirror, so the C toolchain is pinned too. The runner is `ubuntu-24.04` x86-64. Only the counted scope (section 4.3) is counted, on one thread. Estimated cycles are recorded but never gated. | `Ir` does not depend on code alignment, cache state or the host's load. Valgrind gives the guest a fixed, built-in CPUID for each host capability class (section 9). |
| 7 | **Baselines come from CI, never from a laptop, and land through a reviewed PR** (section 5.3). | The owner's dev container is x86-64 under Rosetta. Its guest CPU class (SSE4.2, no AVX) differs from the runners' (AVX2), so its counts are not CI's (section 9). |
| 8 | **The gate is two-sided at ±1.0% per workload.** Outside the band in either direction, the job fails with "out of band: refresh the baseline (`bench.py baseline --from-run <this run>`)". The gate enforces only after calibration (section 5.2). There is no aggregate rule. A geometric-mean band is reported only, and it becomes a rule only if calibration supports one (section 5.2). | The owner's decision. A one-sided gate would let the slack left by an improvement hide a later regression. An aggregate band can fail while every workload is inside its own band, so it needs evidence of its own first. |
| 9 | **UnityPy is wall clock only.** It runs under callgrind (section 9), but the gate never counts it. | UnityPy is a pinned dependency and does not change under our PRs. Counting it costs about 50x its wall time and catches nothing. |
| 10 | **The reference host is GitHub's `ubuntu-24.04` x86-64 runner**, for both the gate's baselines and the recorded wall clock. The harness records `emulated: true` when it detects Rosetta (`VirtualApple` in `/proc/cpuinfo`) or QEMU, and refuses to publish emulated results. | The owner's decision. The runner is native and public, and anyone can rerun the workflows on a fork. |
| 11 | **The documents are generated, never edited by hand.** `bench.py render` writes the README section between `<!-- bench:begin -->` and `<!-- bench:end -->`, and the whole of `BENCHMARKS.md`, from `benchmarks/wall.json` and `benchmarks/gate.json`. `tools/bench/tests` fails when either document differs from what `render` writes. | The JSON is the record, and the prose cannot drift from it. |
| 12 | **The Editor fixtures join later as a "real Unity output" row.** That waits until they are checked in (`docs/plans/static3d.md` section 8, `docs/plans/rest.md` section 6). An Editor-built bench scene is deferred, and the suite starts synthetic. | The owner's decision. |
| 13 | **No new benchmark dependency in the workspace.** The gate drives valgrind from Python's standard library and does not use iai-callgrind or criterion. `unityrs-bench` is `publish = false` and depends only on workspace crates. | A runner crate pins its own binary version and adds `Cargo.lock`, `deny.toml` and `about.toml` churn. Valgrind (GPL-2.0) is a CI tool, never linked or shipped. |
| 14 | **Every provenance claim is checked before merge, in CI.** A committed `wall.json` or `gate.json` names the workflow run that produced it, and that run's head, base and tested merge SHAs. CI checks three things. The run exists and came from the expected workflow file and event. The diff from its head to the PR's head touches only `benchmarks/`, `README.md` and `BENCHMARKS.md`. The gate's counts equal a fresh measurement of the PR's merge state (section 5.3). | A check after merge is too late to enforce anything. |

---

## 1. Goals and non-goals

**Goals.**
- G1. A single command (`scripts/dev python tools/bench/bench.py run --out DIR`) reproduces every published figure on any x86-64 Linux machine that has the oracle stack.
- G2. Each operation category has a paired row, where UnityPy does the work at Tier A: load and bundle parse, typetree read, texture decode by format, mesh export, audio, animation (typetree read of clips), and a whole extraction of a supported inventory (section 4.2).
- G3. For each row and each tool, the comparison reports the median wall time with its spread, CPU time, peak summed PSS and throughput, each labelled by exactly what its timer contains.
- G4. CI fails on an unityrs instruction-count change beyond ±1% in either direction, once calibrated. It never fails because of noise.
- G5. The README summary and `BENCHMARKS.md` always match the committed JSON.

**Non-goals.**
- Gating on wall clock, or gating UnityPy.
- Publishing corpus figures, or naming the corpus.
- A parallel comparison (decision 5).
- Paired rows for work UnityPy cannot do. Animation glTF export, GLB scenes, the batch engine, BC6H and other refused formats are unityrs-only rows. The gate still covers them.
- Vorbis audio. Its setup-header table is FMOD-derived and never committed (`docs/plans/rest.md` decision 9). MPEG and Opus are excluded too, because unityrs does not decode them.

---

## 2. Inputs

### 2.1 The generator

`cargo run --release -p unityrs-bench -- gen --scale full|gate DIR` writes the set and its `manifest.json`. The manifest lists each file's name, bytes and sha256, plus the seed, the scale and the builder version.

- Every byte comes from the testkit's builders and `unityrs_testkit::Rng` (SplitMix64), with seeds fixed in the source.
- Bundles are written by `bundle::build` in the named codec, and serialized files by `serialized::build` with embedded typetrees. The Unity version is 2022.3.62f1, plus a 2019.4 variant of the object set.
- A unit test regenerates the gate scale twice and compares it with `benchmarks/fixtures.json`. `bench.py run` checks a full-scale set against the manifest before timing anything.
- **Legal blocks only.** Each block-compressed format is written so that every block is a legal, non-error block:
  - **DXT, BC4 and BC5, ETC and EAC, PVRTC and ATC:** every bit pattern is a legal block, so seeded block bytes are used.
  - **BC7:** seeded bytes with each block's mode byte set to a fixed mix (modes 1, 5 and 6 at 40%, 30% and 30%). The reserved all-zero mode byte never occurs.
  - **ASTC:** written by a small legal-block writer in `gen`, not from random bytes, because the vendored astcenc decoder has invalid-block paths that random payloads would time instead of decoding. The writer emits a fixed mix: single-plane blocks with 1, 2 and 3 partitions and dual-plane blocks, over the weight grids and endpoint modes it states, plus 1% void-extent blocks. Weights and endpoints are encoded by integer sequence encoding (ISE) within each mode's bit budget. An encoder is not used, because its output can depend on the build and the instruction set (`docs/plans/rest.md` decision 16), which would break the manifest.
  - **BC6H** (unityrs-only): legal mode bits only.
  - **Crunch:** the testkit's crunch builder (`docs/plans/rest.md` decision 5).
- **The mix is verified at both scales.** A test classifies every generated BC7 and ASTC block with `unityrs-texture`'s own block-mode parsing (a test-only `classify` helper). It checks the counts against the stated mix within ±1 percentage point, and that no block takes an error or void-colour path not in the mix. A `parity-py` test decodes the gate-scale ASTC fixture through the oracle (astc-encoder-py) and checks that no pixel takes astcenc's error colour.
- **Other data.** Raw formats are seeded noise. Meshes are procedural grids with deterministic jitter. Audio is synthetic tones in FSB5 banks.

### 2.2 The `bench-v1` set

These are full-scale targets. The arithmetic column shows how each size follows, and BN-1's manifest replaces every figure with exact bytes. The gate scale is 1/64 of each count (section 5.2).

| fixture | contents | arithmetic | ~MB | rows |
|---|---|---|---|---|
| `objects-lz4.bundle` | one CAB of 20,000 objects of 10 classes, all with embedded trees: Transform, Material with property maps, TextAsset, MonoBehaviour with nested arrays and maps, and others. LZ4HC, 128 KiB blocks | about 1.2 KB each decoded is 24 MB, at about 3:1 | 9 | load, typetree |
| `objects-lzma.bundle` | the same CAB, one LZMA block | 24 MB at about 4:1 | 6 | load |
| `objects-2019.assets` | the same objects, uncompressed, 2019.4 layout | 20,000 x 1.2 KB | 24 | load, typetree |
| `many-blocks.bundle` | the `--many-blocks` entry: 64 MiB in 16 KiB LZ4 blocks | 64 MiB decoded, compressed size from the manifest | ≤ 67 | load |
| `textures-<format>.bundle` | 1024x1024 mip-0 Texture2Ds: 4 each of RGBA32, RGB24 and ARGB4444; 8 each of DXT1, DXT5, BC4, BC5, BC6H, BC7, ETC_RGB4, ETC2_RGBA8, EAC_R, ASTC 4x4, ASTC 8x8, PVRTC_RGBA4, ATC_RGBA8, DXT1Crunched and ETC2_RGBA8Crunched. LZ4 (noise and block data barely compress) | raw: 4 x (4 + 3 + 2) MiB = 36. Blocks per 1 Mpx: 0.5 MiB (DXT1, BC4, ETC, EAC, PVRTC), 1 MiB (DXT5, BC5, BC6H, BC7, ETC2, ATC, ASTC 4x4), 0.125 MiB (ASTC 8x8). So 8 x (5 x 0.5 + 7 x 1 + 0.125) = 77, and crunch at most 8 x (0.5 + 1) = 12 | 125 | texture decode, PNG |
| `textures-mips.bundle` | 4 BC7 2048x2048 textures with full mip chains, data in a `.resS` | 4 x 4 MiB x 4/3 | 22 | texture decode (streamed) |
| `meshes.bundle` | 32 meshes of 16,384 vertices (position, normal, tangent and uv0 at 48 B, 16-bit indices); 8 skinned meshes of 16,384 vertices with 4 weights (32 B a vertex) and 16 blend shapes touching 25% of vertices (36 B a delta); 8 `m_CompressedMesh` meshes | 32 x (768 KiB + 190 KiB) = 30. 8 x (768 + 512 KiB + 16 x 4,096 x 36 B) = 29. Compressed meshes about 2 | 61 | mesh export |
| `audio-pcm.bundle` | 8 AudioClips, 30 s stereo 48 kHz PCM16, in FSB5 | 8 x 48,000 x 2 x 2 x 30 B | 46 | audio |
| `audio-adpcm.bundle` | the same tones as FADPCM | about 3.6:1 against PCM16 | 13 | audio |
| `anim.bundle` | 100 AnimationClips of 2 s at 60 fps over 60 bones: a third dense (40 B a bone a frame), a third streamed (about 2x dense), a third constant | 33 x 120 x 60 x 40 B = 9.5, plus 19 streamed | 29 | animation |
| `extract-set/` | 20 bundles of the supported extraction inventory only (section 4.2): each 6 textures of 512x512 (DXT1, DXT5, BC7, RGBA32, ETC2_RGBA8, ASTC 6x6), 4 meshes of 4,096 vertices, 8 TextAssets of 16 KiB and 1 PCM16 clip of 5 s mono, all as container entries | 20 x (3.0 + 0.8 + 0.13 + 0.48) MB | 88 | whole extraction |

The sum is about 490 MB, taking `many-blocks` at its decoded bound. The gate scale is about 8 MB. The set is cached under `$UNITYRS_BENCH_CACHE`, by default `~/.cache/unityrs-bench/<manifest sha256>/`. `bench.py` refuses to write inside the checkout, as `perf.py` does.

---

## 3. Metrics

| metric | how | where |
|---|---|---|
| wall time | `time.perf_counter_ns` around the counted scope only (section 4.3). The whole-process wall time is recorded beside it. Each row's label says what its timer contains. | both tools |
| conditions | warm page cache (each input read once before the round), a fresh process per run, nothing else running in the job | both |
| repeats | 1 warm-up run, then N = 7 measured runs. The runs of all implementations are interleaved round by round, as `perf.py` does. The reported value is the median, with min and max as the spread, plus the IQR. A row whose IQR exceeds 10% of its median carries a dagger. | both |
| CPU time | `resource.getrusage`, self and children, user plus sys | both |
| peak memory | `mem_peak.py`'s peak summed PSS of the process tree, with `VmHWM` beside it | both |
| throughput | source MB/s, plus the row's own unit: objects/s, Mpixel/s, vertices/s, samples/s, keyframes/s or files/s | both |
| load beside | `/proc/loadavg`, other processes' CPU and the machine's busy cores (`mem_peak.py`) | recorded only |
| `Ir` | callgrind, counted scope only (section 4.3) | unityrs, gate |

**Speedup** is `median(UnityPy) / median(unityrs)`. Its published range is `min(UnityPy) / max(unityrs)` to `max(UnityPy) / min(unityrs)`.

---

## 4. The harness

### 4.1 Workloads

A workload is one input and one operation, defined once in `tools/bench/workloads.json`. `bench.py` reads it with Python's `json` and `unityrs-bench` with the workspace's `serde_json`. Each operation exists twice:
- in `tools/bench/py_workloads.py`, run with `--impl unitypy` or `--impl unityrs`, with the same code and a different import;
- in `crates/unityrs-bench/src/work/*.rs`.

Both forms have the same three phases (section 4.3).

| category | workload(s) | setup (not timed) | timed and counted scope | digest (after the scope) |
|---|---|---|---|---|
| load and parse | `load-lz4`, `load-lzma`, `load-assets`, `load-many-blocks` | the file's bytes in memory | `Environment().load_file(bytes)`, every file's object table listed | the object table |
| typetree read | `tree-lz4`, `tree-2019` | the environment loaded | `parse_as_dict()` on every object | canonical JSON (the dump's) |
| texture decode | `tex-<format>` | the environment loaded, Texture2D readers listed | `obj.read().image.tobytes()` for each: object read plus decode, in the image mode UnityPy returns | see below |
| PNG export | `png-rgba32`, `png-bc7` | images decoded | `image.save(BytesIO, "PNG")` | PNG bytes (Pillow-exact) |
| mesh export | `mesh-obj`, `mesh-skinned`, `mesh-compressed` | the environment loaded | `obj.read().export()`: object read plus OBJ text | OBJ text |
| audio | `audio-pcm`, `audio-adpcm` | the environment loaded | `obj.read().samples` | WAV bytes for each sample |
| animation | `anim-tree` (paired), `anim-gltf` (unityrs only) | the environment loaded | `parse_as_dict()` on every AnimationClip | canonical JSON |
| whole extraction | `extract-set` | the input folder on disk | the whole process, for both (section 4.2) | the output tree |

**The pixel representation.** A texture row's digest is the sha256 of the mode name, the width and height, and the bytes of `tobytes()` in the mode UnityPy's `Texture2D.image` returns. That mode is the mode column of `PARITY.md` section 17's TextureFormat matrix: RGB for RGB24, RGBA for most formats. The bytes are after UnityPy's vertical flip. The unityrs side returns the same mode, and no row converts to RGBA inside the timer. Pixel counts in the README come from the matched rows' widths and heights.

### 4.2 The supported extraction inventory

`UnityPy.tools.extractor.extract_assets` aborts or writes nothing in documented cases (`PARITY.md` section 19.2, "extractor crashes"). A GameObject in a container stops the whole export. A MonoBehaviour raises after writing. A sprite without an extension raises in no-container mode. So the extraction row uses a defined inventory that UnityPy completes:
- Texture2D, in formats whose decode is Tier A, written as PNG;
- Mesh, as OBJ;
- TextAsset;
- AudioClip, as PCM16 written to WAV;
- all as container entries, with `use_container=True`, the CLI's default.

`extract-set/` holds nothing else (section 2.2). The row is paired only when three things hold. UnityPy's call returns without an exception. `unityrs extract` exits 0 with an empty `failures` list. The two output trees are identical: the same sorted relative paths, sizes and sha256s. If any condition fails, `bench.py run` marks the row unpaired and `import` refuses the run.

The timer for this row is the whole process on both sides:
- `python -c 'import UnityPy; ...extract_assets(...)'`, including interpreter start and the import;
- `unityrs extract INPUT... -o DIR`, including its start.

In-process times are recorded beside. Output goes to a fresh folder on the same filesystem each run.

### 4.3 Counted and timed boundaries

Each workload runs in three phases:
1. `setup(input)`: neither timed nor counted.
2. `run_<name>(&state) -> Output`: the only thing timed and the only thing counted. It returns its outputs, held in memory.
3. `digest(&output)`: after the timer stops and outside the counted scope.

Hashing and verification never dilute a count or a time. In Rust, `run_<name>` is `#[inline(never)]` and is the callgrind toggle (`--toggle-collect='*unityrs_bench::work::*::run_<name>'`).

`callgrind.py` reads the out file and fails the measurement in four cases:
- the scope function was entered other than exactly once (its `calls=` count);
- the scope's inclusive `Ir` is below 10,000 (a scope that is missing or empty);
- any other `run_*` function was entered (a scope activated unexpectedly);
- collection was on outside the scope (`Ir` attributed outside the scope's inclusive cost).

The Python timer wraps the same phase 2, and the same rule applies: `digest()` runs after `perf_counter_ns()` stops.

### 4.4 Files

```text
benchmarks/
  fixtures.json          # the bench-v1 manifest (decision 3)
  wall.json              # the published comparison, with provenance (decision 14)
  gate.json              # Ir baselines from CI, with provenance
  calibration.jsonl      # gate calibration and weekly-ratio history (sections 5.2, 5.5)
  schema/*.schema.json
BENCHMARKS.md            # generated (decision 11)
crates/unityrs-bench/    # publish = false; gen, run, the Rust workloads, classify tests
tools/bench/
  README.md              # file and test tables, checked by tests/test_readme.py
  bench.py               # gen | run | gate | baseline | import | render | check
  workloads.json
  harness.py             # shared: child runs via tools/parity/child.py, scratch, fingerprint, provenance
  results.py             # schemas, aggregation, pairing (decision 4)
  py_workloads.py        # the Python side, for both implementations
  callgrind.py           # valgrind invocation, scope checks, out-file parsing
  render.py              # README section and BENCHMARKS.md
  install_valgrind.sh    # the pinned deb, sha256-checked, unpacked to a prefix (no root)
  tests/                 # unittest, standard library only, its own CI minimum
.github/workflows/bench-gate.yml, bench-wall.yml, bench-calibrate.yml
```

`tools/bench/tests/test_readme.py` checks that `tools/bench/README.md`'s two tables list every file in `tools/bench` and in `tools/bench/tests`, and no other. It does this the way `tools/parity/tests/test_readme.py` does for its own folder, which is hard-wired to `tools/parity` and so cannot cover `tools/bench`.

### 4.5 The commands

```sh
scripts/dev python tools/bench/bench.py run --out DIR           # the full comparison
scripts/dev python tools/bench/bench.py run --out DIR --quick   # N = 3, gate scale: a smoke run
scripts/dev python tools/bench/bench.py gate --out DIR          # Ir against benchmarks/gate.json (debugging only off CI)
python tools/bench/bench.py baseline --from-run RUN_ID          # import a CI candidate (section 5.3)
python tools/bench/bench.py import --from-run RUN_ID            # adopt a bench-wall run (section 5.4)
python tools/bench/bench.py render [--check]
python tools/bench/bench.py check                               # schemas, provenance, docs: what CI runs
```

`run` builds what it times, unless `--no-build` is given: `unityrs-bench` and the CLI with `cargo build --release --locked`, and the wheel with `maturin build --release --locked` into a venv beside the oracle's.

### 4.6 The results files

`wall.json`, schema version 1:
- `schema_version`, `started_utc`, `took_s`.
- `provenance`: `run_id`, `workflow` (the file path), `event`, `head_sha`, `base_sha`, `merge_sha` (the tested commit) and `dirty: false`.
- `tools`: unityrs's version, commit and `rustc -vV`; UnityPy 1.25.2 and the UnityPyBoost sha256 (`stack_problem`'s values); Pillow, fmod_toolkit and Python; the wheel's version.
- `machine`: CPU model, flags hash, logical and physical cores, memory, kernel, OS, glibc, `ImageOS` and `ImageVersion` of the runner, and `emulated`.
- `fixtures`: the manifest's sha256.
- `workloads[]`: each has `name`, `impl`, `timer` (the scope's label), `paired` with a reason if false, and `parity_row`. Its `runs[]` hold the raw `wall_s`, `process_wall_s`, `cpu_s`, `peak_pss_bytes`, `vm_hwm_bytes`, `digest` and `load`, by round. Its `agg` holds the median, min, max, IQR and throughputs.

`gate.json`:
- `schema_version`, `provenance` (as above), `mode` (`calibrating` or `enforcing`), `tolerance_pct` (1.0);
- `fingerprint`: valgrind version and deb sha256, the container digest, the apt snapshot date and the `dpkg -l` digest, `rustc -vV`, `cc --version`, the target, the guest CPUID class, and the `Cargo.lock` sha256;
- `native`: recorded, not compared, namely the runner's CPU model, `ImageVersion` and kernel;
- `workloads`: a map from each name to `{ir, digest, calls}`, with `estimated_cycles` informational.

No field holds a host path or a user name. `results.py` rejects unknown members, as `tools/memory/regenerate.py` does.

---

## 5. CI

### 5.1 Why wall clock is not gated

GitHub's hosted runners are shared VMs on mixed CPU models. Run-to-run and SKU-to-SKU differences in wall time are far larger than the regressions worth catching. Instruction counts remove the host's load and, through valgrind's built-in CPUID, its CPU model (section 9).

### 5.2 The gate: `bench-gate.yml`, and its calibration

- **Trigger:** `pull_request` and `push` to `main`, on `crates/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `tools/bench/**`, `benchmarks/**` and the workflow itself. It is a required check.
- **Environment:**
  - `runs-on: ubuntu-24.04`, inside `debian:bookworm@sha256:268fb24c…`, the digest `memory-calibration.yml` pins;
  - apt sources pointed at one dated snapshot.debian.org mirror, so gcc, libc6-dev and the rest are pinned. `memory-calibration.yml`'s live installs are not copied;
  - checksum-verified rustup and the pinned toolchain;
  - valgrind from `install_valgrind.sh`;
  - actions pinned by SHA, and a timeout set at twice the measured run (below).
- **Steps:**
  1. `cargo build --release --locked -p unityrs-bench`.
  2. `bench.py gen --scale gate`, checked against `benchmarks/fixtures.json`.
  3. `bench.py gate` runs each gated workload once under callgrind with the scope checks of 4.3. The environment is `env -i` with a fixed `PATH`, `HOME=/nonexistent` and `LANG=C`. The fixtures sit at the fixed path `/bench/fixtures`, and the binary has one thread.
  4. The job always uploads `gate-candidate.json` (the counts, digests, fingerprint and provenance of this run). It writes a job summary comparing three sets of counts for every workload: `main`'s committed `gate.json`, this PR's `gate.json`, and the measured counts. Changes to digests, the fingerprint and the tolerance are marked.
- **What fails the job** (once `mode` is `enforcing`):
  1. any workload with `|Ir / baseline - 1| > 1.0%`, in either direction, with the "refresh the baseline" message and the run id to refresh from;
  2. a digest that differs from `gate.json`'s: the workload did different work;
  3. a workload set that differs from `gate.json`'s, either way;
  4. a scope check of 4.3 that fails;
  5. a fingerprint mismatch, with `Cargo.lock` reported but failing only through item 1;
  6. a `gate.json` that is schema-invalid or whose provenance fails section 5.3's checks.
  In `calibrating` mode, items 1, 2 and 5 are reported without failing. Items 3, 4 and 6 fail from the start.
- **Calibration, before enforcing.** BN-2 lands the gate in `calibrating` mode, together with `bench-calibrate.yml` (`workflow_dispatch`). That workflow builds `unityrs-bench` once and uploads the binary. Then a matrix of 12 fresh `ubuntu-24.04` jobs runs that unchanged binary on the gate set. Each job records its native fingerprint (`native` above) and its counts into `benchmarks/calibration.jsonl` through a PR. The gate switches to `enforcing`, in a PR that changes nothing else, only when two calibration rounds on different days give identical counts on every workload across every job. If they do not, the varying workloads are fixed or dropped first. The spike's evidence is two workloads, run three times each under Rosetta (section 9), and it is not enough alone.
- **Platform-driven refreshes.** A fingerprint change (a new container digest, apt snapshot, valgrind or toolchain) is a PR that changes nothing under `crates/`. Its gate run builds and measures both the PR's merge state and `main`'s head in the new environment, and the job requires their counts to be equal. That shows the change in counts comes from the platform, not the code. The refreshed baseline is the PR's run.
- **The aggregate rule is dropped for now.** The job reports the geometric mean of the ratios. A separate calibration over the first 20 code PRs records how it moves when every workload is within ±1%. A later PR may add an aggregate rule with that evidence.
- **Why ±1.0%.** Measured noise is zero within the scope (section 9), and calibration must confirm that across runners. The band is not for noise. It absorbs refactors that move a few thousand instructions. It still catches what matters: a lost fast path, an extra copy of a decoded buffer, or a per-object allocation, each 3 to 30%.
- **Runtime budget: measured before it is committed.** None of these figures is a commitment. BN-2's PR includes one reference-runner run of each of these, with its times written into the PR description: a cold build, generation, the gate, and a refresh run (the gate measuring base and head, about twice the gate). Each workflow's `timeout-minutes` is set to twice the measured time. A gate whose measured run exceeds 20 minutes drops workloads to the gate scale's smallest representative of each category before it merges. As an expectation to check: about 40 workloads of 20 to 400 million `Ir`, a few minutes under callgrind, after a 6 to 8 minute cold build.

### 5.3 Refreshing a baseline: a reviewed PR

1. A PR that moves counts fails the gate, and the job summary shows the delta against `main`.
2. The author runs `bench.py baseline --from-run <run id>`. It checks through `gh api` that the run belongs to `.github/workflows/bench-gate.yml` on the `pull_request` event for this PR. It also checks that the run's head SHA is the PR's current head, that its merge SHA's parents are that head and the then-current `main`, and that the artifact's sha256 matches the one the job printed. It then writes `benchmarks/gate.json` and prints the delta table (count, digest, fingerprint and tolerance changes), which goes into the PR description.
3. The refresh is a commit of its own, `bench: refresh gate baseline (<reason>)`, touching only `benchmarks/gate.json`. A change of tolerance or fingerprint goes in a PR of its own, with no code change (5.2).
4. **Enforced before merge.** On the next run the gate measures the PR's merge state again and requires its counts to equal the committed `gate.json` exactly, not just within the band. The counts are deterministic, so a refresh from an older merge state, or one taken before `main` moved, fails here. `bench.py check` (5.6) verifies the provenance: the named run's head is an ancestor of the PR head, and the diff between them touches only `benchmarks/`.
5. **The merge must be of the measured state.** The gate is a required check with "branches must be up to date before merging" (or a merge queue). A PR whose base has moved since its last gate run cannot merge until it reruns. The coordinator's merge tooling must enforce the same rule, since it merges directly. The `push` run on `main` remains as a backstop, not as the enforcement.

### 5.4 The first public result, and adopting a run

`bench-wall.yml` runs the full comparison on `ubuntu-24.04`. The oracle stack is installed exactly as `parity-py` installs it: hash-checked wheels in a fresh venv, then `oracle_env.py --exact`. It runs on `workflow_dispatch`, on a weekly schedule (Sunday 06:00 UTC), and on `pull_request` for `tools/bench/**` and `benchmarks/**`. Every run uploads its `wall.json` with its provenance.

**Bootstrap (BN-1).** There is no committed `wall.json` to compare against at first, and the weekly check is record-only anyway (5.5), so nothing circular remains:
1. BN-1's own `pull_request` run of `bench-wall.yml` measures the PR's merge state and uploads `wall.json`.
2. `bench.py import --from-run <id>` checks the run as `baseline` does, with the workflow `bench-wall.yml`. It refuses emulated or unpaired-headline results, then writes `benchmarks/wall.json` and renders the README and `BENCHMARKS.md`.
3. Those files are committed to BN-1 as a commit of their own. `bench.py check` (5.6) verifies the provenance: the named run's head is an ancestor of the PR's head, and the diff between them touches only `benchmarks/`, `README.md` and `BENCHMARKS.md`. So the figures describe the code that merges.
4. Later publications follow the same path, from a `workflow_dispatch` run on `main` or from a PR's run.

### 5.5 The weekly ratio: calibration only

The weekly run compares each paired row's speedup ratio with the history in `benchmarks/calibration.jsonl`, by CPU class (model name and core count). It **never fails** until like-CPU history exists: at least 8 runs on the same CPU class. After that a band can be proposed: the larger of 15% and 4 MADs of that class's ratios. That proposal is a PR with the evidence. Until then the run only records. A maintainer appends each week's per-row ratios (aggregates only) to `calibration.jsonl` in a small PR, because workflow artifacts expire. A ratio far outside its class's history is a prompt to bisect with the gate, which already isolates changes in unityrs's own code.

### 5.6 The Python suite and the docs check

`tools/bench/tests` is registered explicitly. CI's `scripts-py` job gets a step `scripts/unittest-suites tools/bench/tests=N` with its own exact minimum, raised by every PR that adds tests. That suite holds the following:
- `test_readme.py`, the inventory check of 4.4;
- `test_docs.py`: `render --check` against `wall.json` and `gate.json`, schema validation, and a check that no published row is emulated, unpaired or Tier B or C;
- the pairing and band math, the scope checks against recorded callgrind out files, and the provenance checks against recorded `gh api` responses. No network is used in tests.

`bench.py check`, the provenance check against the live repository, is a step in `bench-gate.yml`, which has the git history and a token.

---

## 6. The README summary

A "Performance" section after "Goals", generated between the markers. Every count in it comes from the matched rows (decision 4):

```markdown
## Performance

unityrs against UnityPy 1.25.2 with UnityPyBoost on public synthetic fixtures (`bench-v1`),
one thread, on GitHub's ubuntu-24.04 runner (<CPU model>), commit <short sha>, <date>.
Median of 7 runs, each a fresh process with a warm file cache. API rows time the
operation only, after interpreter start and imports; the extraction row times the
whole process on both sides, startup and imports included.

| operation (timer contains) | input | UnityPy | unityrs (Python) | unityrs (CLI) | speedup |
|---|---|---|---|---|---|
| load and parse a bundle | <n> objects, LZ4 | … s | … s | — | ×… |
| read every typetree | <n> objects | … s | … s | — | ×… |
| read and decode textures | <k> formats, <p> Mpixel | … s | … s | — | ×… |
| read and export meshes to OBJ | <m> meshes, <v> vertices | … s | … s | — | ×… |
| read audio samples to WAV | <c> clips, <t> min | … s | … s | — | ×… |
| extract the supported inventory | <b> bundles, <f> files | … s | — | … s | ×… |

Per-format, per-workload, memory and unityrs-only figures, the method, and how to
rerun: [BENCHMARKS.md](BENCHMARKS.md).
```

Each aggregate row sums its category's paired workloads. The speedup has two significant figures, and the range is in `BENCHMARKS.md`.

## 7. `BENCHMARKS.md`

Every section is generated by `render`:

1. **How to rerun.** The one command, the native x86-64 requirement, and how a run is adopted (5.4).
2. **This run.** The machine, runner image, kernel, CPU, cores and memory, the run's provenance, unityrs's commit and version, the UnityPy, UnityPyBoost, Pillow and Python versions, and the fixture manifest's sha256.
3. **Inputs.** The section 2.2 table with exact bytes, sha256 prefixes and the verified block-mode mixes.
4. **Summary.** The README table with ranges and its conditions.
5. **By category.** One table each, every row labelled with what its timer contains. Texture rows are one per paired format, with Mpixel/s for each tool. Each row has the median, min to max, IQR, CPU s, peak PSS and throughput for each implementation, and the speedup with its range.
6. **Memory.** Peak PSS for each tool and row, and unityrs's metered peak beside it.
7. **unityrs only.** Rows with no paired UnityPy result, each with its reason: BC6H (UnityPy raises), `anim-gltf`, and any row unpaired in this run.
8. **The regression gate.** The mode, every workload's baseline `Ir`, digest prefix and call count, the band, the fingerprint, and how to refresh (5.3).
9. **Method and caveats.** Sections 3, 4.2 and 4.3. What is not compared and why (Vorbis, MPEG, Opus, parallel runs). No game data is used.

---

## 8. PRs

Each PR is at most about 1,500 non-test lines, the repository's cap, and every `.rs` and `.py` file stays under 800 lines. Each raises the minimum for `tools/bench/tests` (and `parity-py`'s where it adds oracle tests), pins actions by SHA and sets each timeout at twice a measured run.

| # | PR | contents | ~lines | depends on |
|---|---|---|---|---|
| BN-0 | plan | this document | docs | none |
| BN-1 | shared harness and first public result | `tools/bench/harness.py`, `results.py` (schemas, pairing, fingerprint, provenance), `render.py`, `bench.py` (`gen`, `run`, `import`, `render`, `check`), and `py_workloads.py` for the load, typetree and texture rows. `crates/unityrs-bench`'s `gen` for those fixtures, with the BC7 and ASTC legal-block writers, `classify` tests and the manifest. `bench-wall.yml`, the `tools/bench/tests` CI step with its minimum, `test_readme.py` and `test_docs.py`. The first `benchmarks/wall.json` through the 5.4 bootstrap, with the README section and `BENCHMARKS.md`. The PR description carries measured reference-runner times for the cold build, generation and the full comparison. The shared harness is in this PR so that both later streams build on one `harness.py` and `results.py`. | 1,500 | BN-0; the current merge train |
| BN-2 | gate, calibrating | `crates/unityrs-bench run` with the scoped `run_*` functions for BN-1's rows, `callgrind.py` with the scope checks, `install_valgrind.sh`, the snapshot-pinned container, `bench.py gate` and `baseline`, `bench-gate.yml` in `calibrating` mode, `bench-calibrate.yml`, and the first `gate.json` (5.3). Measured times are in the description | 1,000 | BN-1 |
| BN-3 | gate, enforcing | `mode: enforcing` after two clean calibration rounds (5.2), with their `calibration.jsonl` lines. Nothing else | 20 | BN-2 and its calibration |
| BN-4 | mesh, audio, animation rows | the fixtures, both workload forms and gate entries for mesh, audio, `anim-tree` and `anim-gltf`; a refreshed `wall.json` and `gate.json` | 900 | BN-2 |
| BN-5 | extraction row | `extract-set/`, the 4.2 inventory checks (zero failures, identical trees), the whole-process timer, gate entries, refreshed results | 700 | BN-4 |
| BN-6 | remaining textures and PNG | the textures BN-1 left out (crunch, PVRTC, ATC, BC6H unityrs-only, mips via `.resS`), the PNG rows, refreshed results | 800 | BN-2 |
| BN-7 | Editor rows | the `extract-editor` rows on the checked-in Editor fixtures | 200 | the Editor fixtures (decision 12) |

BN-4 and BN-6 run in parallel after BN-2. Each later PR republishes through 5.4, so the public figures grow row by row from BN-1 on.

---

## 9. Feasibility spike (2026-11-19)

Run in `unityrs-dev`, which is x86-64 Debian 12 under Rosetta on the owner's Mac, with 10 cores. The guest CPU reads as `VirtualApple`, with SSE4.2 and no AVX. Nothing from the spike is committed.

| question | result |
|---|---|
| valgrind in `unityrs-dev`? | Not installed, and there is no root. Bookworm's `valgrind_3.19.0-1_amd64.deb` installs without root: its sha256 is checked against the bookworm `Packages` index, it is unpacked with `dpkg -x` into a prefix, and it runs with `VALGRIND_LIB` set. Callgrind works. Memcheck needs `libc6-dbg`, but the gate does not use memcheck. |
| valgrind on GitHub's runners? | Ubuntu 24.04 ships valgrind 3.22.0 in `main`, installable with `apt-get`. The plan uses the 3.19.0 deb in the pinned container instead (decision 6). Not yet run on a runner: BN-2's calibration is the check. |
| `perf stat` counters? | No `perf` in the container, `perf_event_paranoid` is 2, and Rosetta exposes no PMU. GitHub's hosted runners are VMs without reliable PMU access. Not relied on. |
| guest CPUID | Natively the container reads `VirtualApple` (SSE4.2, FMA). Under valgrind it reads a built-in `Intel Core i5 670` (SSE4.2, no AVX, no FMA). So valgrind replaces the host's CPUID with a fixed one for each capability class. The Mac's class is not an AVX2 runner's, hence decision 7. |
| `Ir` variance, small workload | `unityrs dump` of a 3,466-byte testkit scene bundle (7 objects, release, 3 runs). Whole process: 3,232,842, 3,232,822 and 3,233,519, a spread of 0.022% from startup, rayon spawn and hash seeds. Scoped to `unityrs_dump::input::dump_on` (`--collect-atstart=no --toggle-collect`): 2,399,110 all three times, with identical output digests. |
| `Ir` variance, larger workload | `bench --many-blocks --reads 1` (128 MiB of LZ4 decode in 128 KiB and 16 KiB blocks), scoped to `*BundleFile*entry_bytes*`: 86,040,117 all three times. That took 8 to 12 s under valgrind and Rosetta, against 0.4 s natively. |
| UnityPy under callgrind? | It runs. `import UnityPy` alone is 5.089 to 5.094 billion `Ir` over 6 runs, a spread of about 0.09% with or without `PYTHONHASHSEED=0`. Loading the same 7-object bundle with `parse_as_dict()` adds about 25 million, less than the import's own noise. The run is about 52x slower than native (85 s against 1.6 s). Scoping would need a C shim that issues callgrind client requests. That is feasible, but unused (decision 9). |

**What follows.** Scoping removes all the variance measured here, which is why only `run_*` is counted (4.3). Two workloads under emulation are not the suite on real runners, which is why the gate calibrates before it enforces (5.2). The Rosetta wall times are not representative, and no comparison was timed.

---

## 10. Owner decisions (2026-11-19)

1. **Publication.** Figures go public with the first benchmark PR after the current merge train: BN-1 (section 8, bootstrap 5.4).
2. **Reference host.** GitHub's `ubuntu-24.04` x86-64 runner, for the gate's baselines and the weekly wall-clock job (decision 10).
3. **The Editor-built bench scene.** Later. The suite starts synthetic (decision 12).
4. **The gate.** Two-sided. Beyond ±1% either way it fails with an explicit "refresh the baseline" message (decision 8).
