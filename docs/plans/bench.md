# Benchmark plan: unityrs against UnityPy

**Status:** proposed, 2026-11-19. Nothing is built yet. Section 9 reports the feasibility spike behind decisions 6 to 9. The harness and the CI gate can land before the first release. The README and `BENCHMARKS.md` figures wait for the first shippable version (the owner's condition, question 1). The owner asked for the following:

> once we have a shippable version, i want you to create benchmark suite that can be run to compare unityrs and unitypy and put summary on the main readme, and then bigger breakdown on a benchmark file. also we should record the numbers so that they can be reran and part of the tests so we can catch any regressions

**What it delivers.**
- One command reruns a like-for-like comparison of unityrs and UnityPy 1.25.2 with UnityPyBoost on public inputs.
- A results file records the machine, the versions and every raw run.
- A README summary and a `BENCHMARKS.md` breakdown are generated from that file, and a test checks them.
- A deterministic CI gate fails a PR that changes unityrs's instruction counts beyond a stated tolerance.

**What it builds on.**
- The corpus perf report (`tools/perf/`, `docs/plans/core.md` sections 6, 8 and 10): `perf.py`, `bench_unitypy.py` with its `stack_problem` check of the pinned oracle, `mem_peak.py`'s summed-PSS sampler with the load beside it, and `report.py`.
- `unityrs-dump`'s `bench` example, its `--many-blocks` runs and its per-thread CPU clock.
- The testkit's builders (`unityrs-testkit`: bundles in every codec, serialized files with embedded trees, `ngui::texture`, `scene3d` meshes and clips, `fsb5::bank`).
- The oracle pins (`tools/parity/requirements-oracle.txt`, `oracle_env.py --exact`).
- The UnityPy-compatible wheel (`crates/unityrs-py`, module `unityrs`).

All of that stays as it is. The corpus report remains a private local check, and nothing it measures is published.

**Citations.** UnityPy behaviour is cited by its `PARITY.md` row. Repository paths are relative to the repository root.

---

## 0. Decisions

| # | decision | why |
|---|---|---|
| 1 | **Two measurements, kept apart.** The *comparison* measures wall time, CPU time, peak memory and throughput for both tools, records them and publishes them. It never fails a PR. The *gate* counts instructions (callgrind `Ir`) for unityrs only, against committed baselines, and fails CI. | Wall clock on shared runners is too noisy to gate on (section 5.1). Instruction counts are deterministic, but they mean nothing across languages, so they are kept out of the comparison. |
| 2 | **Public inputs only.** The published figures come from the `bench-v1` fixture set, which `unityrs-bench gen` builds from the testkit's builders with fixed seeds (section 2), and from the owner's Editor-built fixtures once they are checked in (decision 12). The private corpus is never an input to a committed file or a published figure, and it is never named. `tools/perf/perf.py` stays its local report. UnityPy's own test samples (`tools/parity/unitypy_samples.json`) are game bundles, so they are excluded too. | The repository is public, and every published figure must be one anyone can reproduce. |
| 3 | **Fixtures are generated, not committed.** `benchmarks/fixtures.json` commits each file's name, size and sha256, plus the generator's seed and scale. The bytes are never committed. A test regenerates the gate-scale set and checks it against the manifest. The full set is about 420 MB, takes seconds to build, and is cached outside the checkout. | The full set is far too large to commit, and generation is deterministic. The manifest keeps a rerun honest. |
| 4 | **Like for like means the same work, the same machine and the same stack.** Both tools run in one container, under the oracle pins (`oracle_env.py --exact` and `bench_unitypy.stack_problem()` must both pass, or the harness refuses), on the same fixture bytes with a warm page cache. Each run is a fresh process. Each workload checks that both tools produced the same output digest, so no row compares unequal work. A row whose output is Tier B or C in `PARITY.md` names that row, and the digests are compared against the stated reference instead. | A faster wrong answer is not a speedup. Equal digests prove that both sides did the same work. |
| 5 | **Three unityrs columns, one UnityPy column.** The comparison runs (a) the `unityrs` Python wheel under the *same* Python workload script as UnityPy (`import unityrs as UnityPy`), (b) the CLI where UnityPy has a counterpart (`unityrs extract` against `UnityPy.tools.extractor.extract_assets`), and (c) the Rust library through `unityrs-bench run`. The README headline uses (a) and (b). (c) appears in `BENCHMARKS.md` only. | (a) is what a UnityPy user would switch to, line for line. (b) is the batch user's path. (c) shows the cost of the binding's overhead. |
| 6 | **The gate metric is callgrind `Ir`.** The gate uses valgrind 3.19.0 (Debian bookworm's `valgrind_3.19.0-1_amd64.deb`, pinned by sha256 and fetched from snapshot.debian.org). It runs inside `debian:bookworm` pinned by digest, as `memory-calibration.yml` does, on `ubuntu-24.04`. It counts instructions in the workload function only (`--toggle-collect`), on one thread. Estimated cycles and cache misses are recorded but never gated. | `Ir` does not depend on code alignment, cache state or the host's load. Valgrind gives the guest a fixed, built-in CPUID for each host capability class, so runtime CPU dispatch takes the same path on every AVX2 runner (section 9). |
| 7 | **Baselines come from CI, never from a laptop.** The gate job always uploads the counts it measured as `gate-candidate.json`. `tools/bench/bench.py baseline --from-run RUN_ID` downloads that file with `gh`, checks it and writes `benchmarks/gate.json`. | The owner's dev container is x86-64 under Rosetta. Its guest CPU class (SSE4.2, no AVX) differs from the runners' (AVX2), so its counts are not CI's (section 9). |
| 8 | **Gate tolerance is ±1.0% per workload and ±0.5% on the geometric mean, in both directions.** A count outside its band fails the gate. A drop as well as a rise fails it, so an improvement must refresh the baseline in the same PR. | Section 5.2 gives the noise argument. A one-sided gate would let the slack left by an improvement hide a later regression. |
| 9 | **UnityPy is wall clock only.** It runs under callgrind (section 9), but the gate never counts it. | UnityPy is a pinned dependency and does not change under our PRs. Counting it would cost about 60x its wall time and catch nothing. |
| 10 | **The published reference host is a native x86-64 Linux machine.** The default is GitHub's `ubuntu-24.04` runner, through the manual and weekly `bench-wall.yml` workflow (section 5.4). The harness records `emulated: true` when it detects Rosetta (`VirtualApple` in `/proc/cpuinfo`) or QEMU, and `render` refuses emulated results. | Anyone can rerun the workflow on a fork. Emulation distorts the ratio between native code and the interpreter (question 2). |
| 11 | **The documents are generated, never edited by hand.** `bench.py render` writes the README section between `<!-- bench:begin -->` and `<!-- bench:end -->`, and the whole of `BENCHMARKS.md`, from `benchmarks/wall.json` and `benchmarks/gate.json`. A test in `scripts/tests` fails when either document differs from what `render` would write. | This follows the existing table tests (`tools/parity/tests/test_readme.py`): the JSON is the record, and the prose cannot drift from it. |
| 12 | **The Editor fixtures join as a "real Unity output" row** once they are checked in (`docs/plans/static3d.md` section 8, `docs/plans/rest.md` section 6). They are small parity fixtures, so the scaled synthetic set remains the throughput input. A larger Editor-built bench scene is question 3. | Synthetic data proves scale. Editor data proves that the files are shaped like Unity's. |
| 13 | **No new benchmark dependency in the workspace.** The gate drives valgrind from Python's standard library and does not use iai-callgrind or criterion. `unityrs-bench` is `publish = false` and depends only on workspace crates. | A runner crate pins its own binary version and adds `Cargo.lock`, `deny.toml` and `about.toml` churn. The harness needs about 150 lines. Valgrind (GPL-2.0) is a CI tool, never linked or shipped. |

---

## 1. Goals and non-goals

**Goals.**
- G1. A single command (`scripts/dev python tools/bench/bench.py run --out DIR`) reproduces every published figure on any x86-64 Linux machine that has the oracle stack.
- G2. Each operation category has a row for both tools: environment load and bundle parse, typetree read, texture decode by format, mesh export, audio, animation, and a whole extraction.
- G3. For each row and each tool, the comparison reports the median wall time with its spread, CPU time, peak summed PSS and throughput.
- G4. CI fails on an unityrs instruction-count regression or an unrefreshed improvement. It never fails because of noise.
- G5. The README summary and `BENCHMARKS.md` always match the committed JSON.

**Non-goals.**
- Gating on wall clock, or gating UnityPy.
- Publishing corpus figures, or naming the corpus.
- Benchmarking paths without a UnityPy counterpart as a comparison. These are animation glTF export, GLB scenes and the batch engine's spec runs. `BENCHMARKS.md` lists them in a separate unityrs-only table, which the gate still covers.
- Vorbis audio. Its setup-header table is FMOD-derived and is never committed (`docs/plans/rest.md` decision 9). Neither are MPEG or Opus, which unityrs does not decode.

---

## 2. Inputs

### 2.1 The generator

`cargo run --release -p unityrs-bench -- gen --scale full|gate DIR` writes the set and its manifest. The `unityrs-bench` crate is new, with `publish = false`.

- Every byte comes from the testkit's builders and `unityrs_testkit::Rng` (SplitMix64), with seeds fixed in the source.
- Bundles are written by the testkit's `bundle::build` in the named codec, and serialized files by `serialized::build` with embedded typetrees.
- Unity versions are 2022.3.62f1 for the main set, plus a 2019.4 variant of the typetree fixture, so the older layout is timed too.
- `gen` then writes `manifest.json` (name, bytes, sha256, seed, scale and builder version). A unit test compares the gate-scale manifest with `benchmarks/fixtures.json`, so a builder change that alters a byte fails until the manifest is refreshed.
- At `full` scale, a check in `bench.py run` compares the set on disk with the committed manifest before any timing.

Block-compressed texture data is random block bytes from the seeded generator. Every bit pattern is a valid block for the decoders timed here, so random data stands in for encoder output. The cost of BC7 and ASTC depends on the block mode, so their headers are set to a fixed mix:
- BC7: modes 1, 5 and 6, at 40%, 30% and 30%, stated in `gen` and in `BENCHMARKS.md`;
- ASTC: single-plane and dual-plane blocks with one to three partitions, in fixed proportions stated in `gen`.

Crunch streams come from the in-code crunch builder (`docs/plans/rest.md` decision 5). Raw formats are seeded noise. Meshes are procedural grids with deterministic jitter. Audio is synthetic tones in FSB5 banks.

### 2.2 The `bench-v1` set

Sizes are the full scale. The gate scale (section 5) is about 1/64 of each, and the CI gate builds it in under 2 s.

| fixture | contents | ~bytes on disk | categories |
|---|---|---|---|
| `objects-lz4.bundle` | one UnityFS bundle in LZ4HC blocks of 128 KiB. One CAB holds 20,000 objects of 12 classes (GameObject, Transform, MonoBehaviour with nested arrays and maps, Material with property maps, MeshFilter, MeshRenderer, Animator, TextAsset and others), all with embedded trees | 9 MB (24 MB decoded) | load, typetree read |
| `objects-lzma.bundle` | the same CAB in one LZMA block | 6 MB | load (LZMA) |
| `objects-2019.assets` | the same objects as an uncompressed 2019.4 SerializedFile | 24 MB | load, typetree read |
| `many-blocks.bundle` | the `--many-blocks` entry: 64 MiB in LZ4 blocks of 16 KiB | 40 MB | load (block overhead) |
| `textures-<format>.bundle` | 16 Texture2D objects of 1024x1024 each, mip 0 only, LZ4 bundle. One fixture for each of RGBA32, RGB24, ARGB4444, DXT1, DXT5, BC4, BC5, BC6H, BC7, ETC_RGB4, ETC2_RGBA8, EAC_R, ASTC 4x4, ASTC 8x8, PVRTC_RGBA4, ATC_RGBA8, DXT1Crunched and ETC2_RGBA8Crunched | 1 to 64 MB each, about 230 MB in all | texture decode, image export |
| `textures-mips.bundle` | 4 BC7 textures of 2048x2048 with full mip chains, streamed from a `.resS` | 22 MB | texture decode (streamed data) |
| `meshes.bundle` | 64 meshes of 65,536 vertices (position, normal, tangent, uv0, 16-bit indices), 8 skinned meshes with 4 bone weights and 16 blend shapes, and 8 `m_CompressedMesh` meshes | 75 MB | mesh export |
| `audio-pcm.bundle`, `audio-adpcm.bundle` | 8 AudioClips each: 30 s stereo 48 kHz, as PCM16 and as FADPCM FSB5 banks | 92 MB and 24 MB | audio |
| `anim.bundle` | 200 AnimationClips (2 s at 60 fps, 60 bones, with streamed, dense and constant curves) and an Animator with its controller | 18 MB | animation |
| `mixed/` | 40 bundles that mix the classes above at the proportions of a typical mobile asset build (a stated table in `gen`), with cross-bundle PPtrs | 110 MB | whole extraction |
| `editor-2022.3/`, `editor-6000.x/` | the checked-in Editor fixtures, read in place (decision 12) | as delivered | whole extraction (real Unity output) |

The set is about 420 MB without the Editor fixtures, and is cached under `$UNITYRS_BENCH_CACHE` (by default `~/.cache/unityrs-bench/<manifest sha256>/`). `bench.py` refuses to write inside the checkout, as `perf.py` does.

---

## 3. Metrics

| metric | how | where |
|---|---|---|
| wall time | `time.perf_counter_ns` around the workload, inside the process, after imports and before teardown. The whole-process wall time is recorded beside it. | both tools |
| repeats | 1 warm-up run, then **N = 7** measured runs. Each run is a separate process, and the runs of all tools are interleaved round by round, as `perf.py` does, so drift in the load spreads over every column. The reported value is the median, with the min and max as the spread, plus the IQR. Runs whose IQR is more than 10% of the median are flagged in the results, and `render` prints them with a dagger. | both |
| CPU time | `resource.getrusage(RUSAGE_SELF and RUSAGE_CHILDREN)`, user plus sys | both |
| peak memory | `mem_peak.py`'s peak summed PSS of the process tree, with `VmHWM` beside it | both |
| throughput | source MB/s for every row, plus the row's own unit: objects/s (typetree), Mpixel/s (textures), vertices/s (mesh), samples/s (audio), keyframes/s (animation), files/s (extraction) | both |
| load beside | `/proc/loadavg` before and after, other processes' CPU, and the machine's busy cores (`mem_peak.py`) | recorded, never rendered |
| parallel | the whole-extraction row also runs on `-j N` (N = the machine's cores): unityrs extract's worker pool against UnityPy in a fork pool of N, one bundle per task (`bench_unitypy.py`'s model) | both |
| `Ir` | callgrind, workload function only (section 5) | unityrs, gate |

**Speedup** in the summary is `median(UnityPy) / median(unityrs)`. Its range is `min(UnityPy) / max(unityrs)` to `max(UnityPy) / min(unityrs)`, so the published range is conservative.

---

## 4. The harness

### 4.1 Workloads

A workload is a named pair: one input and one operation. It is defined once in `tools/bench/workloads.json`, which `bench.py` and `unityrs-bench` both read (Python's `json` and the workspace's `serde_json`). Each operation exists in two forms:
- `tools/bench/py_workloads.py`, run with `--impl unitypy` or `--impl unityrs`. The code is the same and only the import differs. For example, `texture_decode` loads the bundle, then calls `obj.read().image.tobytes()` for every Texture2D;
- `crates/unityrs-bench/src/work/*.rs`, each an `#[inline(never)] fn run_<name>(input) -> Digest`, the call path the CLI and library take.

| category | workload(s) | UnityPy operation | unityrs output compared |
|---|---|---|---|
| load / bundle parse | `load-lz4`, `load-lzma`, `load-assets`, `load-many-blocks` | `Environment().load_file(path)`, each file's `objects` listed | the object table's digest |
| typetree read | `tree-lz4`, `tree-2019` | `obj.parse_as_dict()` on every object | canonical JSON digest (the dump's) |
| texture decode | `tex-<format>` (18 rows) | `obj.read().image` then `tobytes()` | RGBA bytes digest |
| image export | `png-rgba32`, `png-bc7` | `image.save(PNG)` | PNG bytes (Pillow-exact), digest |
| mesh export | `mesh-obj`, `mesh-skinned`, `mesh-compressed` | `MeshExporter.export_mesh` (`mesh.export()`) | OBJ text digest |
| audio | `audio-pcm`, `audio-adpcm` | `clip.samples` (fmod_toolkit) | WAV bytes digest |
| animation | `anim-tree` (both), `anim-gltf` (unityrs only) | `parse_as_dict()` on every AnimationClip | canonical JSON digest |
| whole extraction | `extract-mixed`, `extract-mixed-j`, `extract-editor` | `UnityPy.tools.extractor.extract_assets(src, dst, use_container=True)` | the output tree's digest (sorted paths, sizes, sha256) |

### 4.2 Files

```text
benchmarks/
  fixtures.json        # the bench-v1 manifest (decision 3)
  wall.json            # the published comparison: machine, versions, raw runs, aggregates
  gate.json            # Ir baselines from CI (decision 7)
  schema/wall-v1.schema.json
  schema/gate-v1.schema.json
BENCHMARKS.md          # generated (decision 11)
crates/unityrs-bench/  # publish = false; gen, run, the Rust workloads
tools/bench/
  bench.py             # gen | run | gate | baseline | render | check
  workloads.json       # the workload table
  py_workloads.py      # the Python side, for both impls
  callgrind.py         # valgrind invocation and out-file parsing
  results.py           # schema validation, aggregation, fingerprint
  render.py            # README section and BENCHMARKS.md
  install_valgrind.sh  # the pinned deb, sha256-checked, unpacked to a prefix (no root)
  tests/               # unittest; standard library only
.github/workflows/bench-gate.yml, bench-wall.yml
```

`tools/bench/README.md` gets the file and test tables that the existing table tests require of a tool folder.

### 4.3 The commands

```sh
scripts/dev python tools/bench/bench.py run --out DIR           # the full comparison (about 35 min at N = 7)
scripts/dev python tools/bench/bench.py run --out DIR --quick   # N = 3, gate scale: a smoke run
scripts/dev python tools/bench/bench.py gate --out DIR          # Ir against benchmarks/gate.json
python tools/bench/bench.py baseline --from-run RUN_ID          # import CI's candidate
python tools/bench/bench.py import DIR/wall.json                # adopt a run as benchmarks/wall.json, then render
python tools/bench/bench.py render [--check]                    # regenerate or verify the docs
```

`run` builds what it times, unless `--no-build` is given: `unityrs-bench` and the CLI with `cargo build --release --locked`, and the wheel with `maturin build --release --locked` installed into a venv beside the oracle's. It then generates or verifies the fixtures, runs the rounds and writes `DIR/wall.json`. `import` is the only path into `benchmarks/wall.json`. It refuses a run that is emulated, that comes from a dirty tree, that is not on `main` or a release tag, whose fixture manifest differs from the committed one, or whose tool versions are not the pins.

### 4.4 The results file

`wall.json`, schema version 1:
- `schema_version`, `started_utc`, `took_s`.
- `commit`: 40 hex digits, plus `dirty: false` and `ref`.
- `tools`:
  - unityrs: the workspace version, commit and `rustc -V`;
  - UnityPy 1.25.2 and the UnityPyBoost sha256 (`bench_unitypy.stack_problem`'s values), plus Pillow, fmod_toolkit and the Python version;
  - the wheel's version and its build.
- `machine`:
  - CPU model and flags hash, logical and physical cores, memory, kernel, OS release, glibc;
  - the container image digest, or the workflow run id, and `emulated`;
  - the CPU governor where readable, and `runner` (`github:ubuntu-24.04` or `local`).
- `fixtures`: the manifest's sha256.
- `workloads[]`. Each has `name`, `impl` and `jobs`, then:
  - `runs[]`: raw `wall_s`, `process_wall_s`, `cpu_s`, `peak_pss_bytes`, `vm_hwm_bytes`, `digest` and `load`, in run order with its round;
  - `agg`: median, min, max, IQR, and throughput for each unit.

No field holds a host path or a user name. `results.py` rejects unknown members, as `tools/memory/regenerate.py` does.

`gate.json`:
- `schema_version`;
- `fingerprint`: the valgrind version, the deb sha256, the container digest, `rustc -V`, the target, the guest CPUID class (section 9) and the `Cargo.lock` sha256;
- `commit` and `run_id`;
- `workloads`: a map from each name to `{ir, digest}`, with `estimated_cycles` beside each, informational only.

---

## 5. CI

### 5.1 Why wall clock is not gated

GitHub's hosted runners are shared VMs on mixed CPU models. Consecutive runs of the same binary differ by several percent, and runs on different SKUs differ by tens of percent. A gate tight enough to catch a 2% regression would fail often on unchanged code, and a gate loose enough to stay quiet would miss most regressions. Instruction counts remove both sources of noise: the host's load and the host's CPU model (section 9).

### 5.2 The gate: `bench-gate.yml`

- **Trigger:** `pull_request` and `push` to `main`, on `crates/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `tools/bench/**`, `benchmarks/**` and the workflow itself.
- **Environment:** `runs-on: ubuntu-24.04`, inside `debian:bookworm@sha256:268fb24c…` (the digest `memory-calibration.yml` already pins), with `timeout-minutes: 30`. Rustup is checksum-verified with the pinned toolchain, as in that workflow. Valgrind 3.19.0 is installed by `install_valgrind.sh` from snapshot.debian.org, checked by sha256. Actions are pinned to full SHAs (`scripts/tests/test_ci_workflow.py`).
- **Steps:**
  1. `cargo build --release --locked -p unityrs-bench`.
  2. `bench.py gen --scale gate`, checked against `benchmarks/fixtures.json`.
  3. `bench.py gate` runs each gated workload once under `valgrind --tool=callgrind --collect-atstart=no --toggle-collect='unityrs_bench::work::*::run_*'`, with `env -i` and a fixed environment (`PATH`, `HOME=/nonexistent`, `LANG=C`). The fixtures sit at a fixed path (`/bench/fixtures`) and the binary has one thread (`-j 1`, `RAYON_NUM_THREADS=1`).
  4. Any workload outside its band runs twice more, and the median of the three is used. In section 9's measurements this never changes a verdict. The reruns are there in case a hash seed changes a probe count.
  5. The step uploads `gate-candidate.json` and prints a table of baseline, measured, delta and band for every workload.
- **What fails the job, exactly:**
  1. Any workload with `|Ir / baseline - 1| > 1.0%`.
  2. A geometric mean of the ratios over all gated workloads outside `1 ± 0.5%`.
  3. A digest that differs from `gate.json`'s. The workload did different work, so its count is not comparable. The fix is a parity change, or a refreshed baseline when the output change is intended and recorded elsewhere.
  4. A workload set that differs from `gate.json`'s, either way. An exact set, like the parity suites' exact minimums.
  5. A fingerprint mismatch (valgrind, container, toolchain, target or guest CPUID class). `Cargo.lock` is the exception: it is reported, but it fails only through items 1 and 2. A dependency bump that changes no count passes.
  6. A schema-invalid `gate.json`.
- **Why it is stable.**
  - Everything that can move `Ir` without a code change is pinned: the toolchain, the container and its glibc, valgrind, the fixtures and the environment.
  - Callgrind counts guest instructions only. The kernel's work, page faults, scheduling and cache state are invisible to it.
  - Valgrind gives every AVX2 host the same CPUID, so `is_x86_feature_detected!`, glibc's ifuncs, and liblzma's and zlib's dispatch all choose the same code.
  - Valgrind serialises threads, and the gated workloads run on one thread anyway, so no interleaving reaches the count.
  - The remaining sources, Rust's `RandomState` hash seeds and thread start-up, move a whole-process count by 0.022% in section 9. Counting only the workload function removed them completely (identical counts in 3 of 3 runs, twice).
- **Why ±1.0% and ±0.5%.**
  - The band must be wide enough that measurement noise (0 when scoped, 0.022% for a whole process) never fails a run, and so that refactors that legitimately move a few thousand instructions do not fail either. The latter is the real source of churn.
  - It must also be narrow enough to catch regressions that matter. A 1% per-workload band catches a lost fast path, an extra copy of the decoded buffer or a per-object allocation, all of which cost 3 to 30%.
  - The geometric-mean band catches a broad 0.5% slowdown spread thinly across every workload, which no per-workload band would see.
  - Both directions fail (decision 8). An improvement is welcome, and a CI-produced baseline costs one command.
  - The bands are in `gate.json` (`tolerance_pct`, `geomean_tolerance_pct`), so a later PR can tune them with evidence.
- **Gated set:** every row of section 4.1 except the parallel `extract-mixed-j`, at gate scale. That is about 40 workloads, each 20 to 400 million `Ir`, about 0.1 to 2.5 s under callgrind at roughly 60x.
- **Runtime budget:** a cold release build of `unityrs-bench` and its workspace dependencies takes about 6 to 8 minutes on the 4-vCPU runner (faster with the `actions/cache` keyed on `Cargo.lock`, as other jobs use). Generation takes under 5 s, the callgrind runs about 2 minutes, and the whole job about 12 minutes, inside its 30-minute timeout.
- **Refreshing a baseline:** run `bench.py baseline --from-run <id>` on the PR's latest gate run, commit `benchmarks/gate.json`, and give the reason in the commit message.
  - `gate.json` lands only with a run on the same merged state. The gate job on `push` to `main` re-measures, and a mismatch there fails `main` loudly.
  - The existing rule applies (build and test against current `main` before merging): two PRs that each refresh `gate.json` cannot both merge without one rebasing and refreshing again.

### 5.3 The docs check

`scripts/tests/test_benchmarks_docs.py` (in `scripts-py`, whose minimum count rises) runs `render --check`. It fails if the README section between the markers, or `BENCHMARKS.md`, differs from what `wall.json` and `gate.json` render. It also checks that `wall.json` validates and that no field is emulated. The test is fast, uses the standard library only and needs no build.

### 5.4 The recorded wall clock: `bench-wall.yml`

- **Trigger:** `workflow_dispatch`, plus a weekly `schedule` (Sunday 06:00 UTC). It is never a PR check.
- **Environment:** `ubuntu-24.04`, with the oracle stack installed exactly as `parity-py` installs it (hash-checked wheels in a fresh venv, then `oracle_env.py --exact`). The pinned toolchain builds the wheel and the CLI. `timeout-minutes: 90`.
- **Steps:** `bench.py run --out "$RUNNER_TEMP/bench"` at full scale and N = 7, then the check below. The step uploads `wall.json` as an artifact.
- **The check** compares speedup *ratios*, not absolute times, against the committed `wall.json`. It fails the scheduled run if any row's median speedup falls by more than 15%, or if the geometric mean of the speedups falls by more than 7%.
  - Ratios cancel most of the runner's SKU difference, because both tools ran on the same VM minutes apart.
  - The 15% band is set to about 3x the run-to-run spread of the ratios that the first four weekly runs observe. The first PR records them, and the band is revisited after a month.
  - A failure opens nothing automatically. It is a signal to bisect with the gate's `Ir`, which already pins down any regression in unityrs's own code.
- **Publishing:** a maintainer runs `bench.py import` on a downloaded artifact at a release, then `render`, and commits the result. The README figures therefore always come from one identified run on `main`.

---

## 6. The README summary

A short section after "Goals", generated between markers:

```markdown
## Performance

unityrs against UnityPy 1.25.2 with UnityPyBoost, on public synthetic fixtures
(`bench-v1`). Median of 7 runs, one thread unless noted. <machine line>, commit <short sha>, <date>.

| operation | input | UnityPy | unityrs (Python) | unityrs (CLI) | speedup |
|---|---|---|---|---|---|
| load and parse bundles | 20,000 objects, LZ4 | … s | … s | — | ×… |
| read every typetree | 20,000 objects | … s | … s | — | ×… |
| decode textures | 18 formats, 288 Mpixel | … s | … s | — | ×… |
| export meshes to OBJ | 80 meshes, 5.2 M vertices | … s | … s | — | ×… |
| decode audio to WAV | 16 clips, 8 min | … s | … s | — | ×… |
| extract everything | 40 bundles, 110 MB | … s | — | … s | ×… |
| extract everything, 8 jobs | the same | … s | — | … s | ×… |

Peak memory, per-format and per-workload figures, the method and how to rerun
them: [BENCHMARKS.md](BENCHMARKS.md).
```

The speedup column shows the Python wheel's ratio for the API rows and the CLI's for the extraction rows. It is printed to two significant figures, with the conservative range in `BENCHMARKS.md`. The aggregate rows sum the workloads of their category, and the input column says what each one covers.

## 7. `BENCHMARKS.md`

Every section is generated by `render`:

1. **How to rerun.** The one command, its run time, the requirement for a native x86-64 host, and how to adopt a run.
2. **This run.** The machine, OS, kernel, CPU, cores and memory, the container or workflow run, the date, unityrs's commit and version, the UnityPy, UnityPyBoost, Pillow and Python versions, and the fixture manifest's sha256.
3. **Inputs.** The section 2.2 table, with each file's bytes and its sha256 prefix.
4. **Summary.** The README table with ranges.
5. **By category.** One table each for load, typetree, textures (one row per format, Mpixel/s for each tool), mesh, audio, animation and extraction. Each row has the median, min and max, IQR, CPU s, peak PSS and throughput for each implementation, and the speedup with its range. Rows flagged for spread carry the dagger.
6. **Memory.** Peak PSS for each tool and row, and unityrs's metered peak (`Meter::used()` from `run`) beside it.
7. **unityrs only.** `anim-gltf`, GLB scene export and the batch spec run, with no UnityPy column.
8. **The regression gate.** Every gated workload's baseline `Ir` and digest prefix, the bands, the fingerprint, and how to refresh.
9. **Method and caveats.** Section 3's rules. The rows whose outputs are compared against a Tier B or C reference. What is not compared (Vorbis, MPEG, Opus). Emulated hosts are refused. These are synthetic fixtures and Editor fixtures, and no game data is used.

---

## 8. PRs

Each PR is about 400 to 900 non-test lines. Every `.rs` and `.py` file stays under 800 lines. Each PR raises the `scripts-py` and `parity-py` minimum counts for the tests it adds, pins actions by SHA and gives each job a timeout.

| # | PR | contents | ~lines | depends on |
|---|---|---|---|---|
| BN-0 | plan | this document | docs | none |
| BN-1 | fixtures | `crates/unityrs-bench` with `gen` for both scales (section 2), `benchmarks/fixtures.json`, and a determinism test (gate scale, twice and against the manifest). It also adds a parity smoke test that UnityPy loads every fixture kind and yields the same digests as the Rust side, run in `parity-py` | 900 | BN-0 |
| BN-2 | Rust workloads | `unityrs-bench run WORKLOAD` (JSON: wall, CPU, digest, metered peak), every section 4.1 row, `workloads.json` | 800 | BN-1 |
| BN-3 | the comparison | `tools/bench/bench.py run`, `py_workloads.py` for both impls, `results.py` with its schema, wheel and CLI builds, round interleaving through `mem_peak.py`, the emulation check, and tests on recorded fake runs | 900 (Python) | BN-2 |
| BN-4 | the gate | `callgrind.py`, `install_valgrind.sh`, `bench.py gate` and `baseline`, `bench-gate.yml`, the first `benchmarks/gate.json` (from that PR's own CI run), and tests of the band math and every failure item in 5.2 | 600 | BN-2 |
| BN-5 | publishing | `render.py`, the README markers, `BENCHMARKS.md`, `test_benchmarks_docs.py`, `bench-wall.yml` with the ratio check, and the first `benchmarks/wall.json` from a `main` run of that workflow | 600 | BN-3, BN-4; the first release (question 1) |
| BN-6 | Editor rows | the `extract-editor` rows on the checked-in Editor fixtures, and their gate entries | 200 | the Editor fixtures (decision 12) |

BN-3 and BN-4 run in parallel after BN-2. BN-4 can merge before the first release, so the gate protects performance from then on.

---

## 9. Feasibility spike (2026-11-19)

Run in `unityrs-dev`, which is x86-64 Debian 12 under Rosetta on the owner's Mac, with 10 cores. The guest CPU reads as `VirtualApple`, with SSE4.2 and no AVX. Nothing from the spike is committed.

| question | result |
|---|---|
| valgrind in `unityrs-dev`? | Not installed, and there is no root. Bookworm's `valgrind_3.19.0-1_amd64.deb` installs without root: its sha256 is checked against the bookworm `Packages` index, it is unpacked with `dpkg -x` into a prefix, and it runs with `VALGRIND_LIB` set. Callgrind works. Memcheck does not, because it needs `libc6-dbg`, which the gate does not use. |
| valgrind on GitHub's runners? | Ubuntu 24.04 ships valgrind 3.22.0 in `main`, so `apt-get install valgrind` works on `ubuntu-24.04`. The plan uses the 3.19.0 deb in the pinned bookworm container instead (decision 6), so the runner image cannot change the version. Not yet run on a runner: BN-4's first CI run is the check. |
| `perf stat` counters? | No `perf` in the container, `perf_event_paranoid` is 2, and Rosetta exposes no PMU. GitHub's hosted runners are VMs without reliable PMU access. Not relied on. |
| guest CPUID | Natively the container reads `VirtualApple`, with SSE4.2 and FMA. Under valgrind it reads a built-in `Intel Core i5 670`, SSE4.2 with no AVX and no FMA. So valgrind replaces the host's CPUID with a fixed one for each capability class, which supports decision 6. The Mac's class differs from an AVX2 runner's, which is why decision 7 takes baselines from CI. |
| `Ir` variance, small workload | `unityrs dump` of a 3,466-byte testkit scene bundle (7 objects, release build, 3 runs). Whole process: 3,232,842, 3,232,822 and 3,233,519 (spread 697, **0.022%**: startup, rayon spawn and hash seeds). Counting only `unityrs_dump::input::dump_on` (`--collect-atstart=no --toggle-collect`): **2,399,110 all three times**, with identical output digests. |
| `Ir` variance, larger workload | `bench --many-blocks --reads 1` (128 MiB of LZ4 decode in blocks of 128 KiB and 16 KiB), counting `*BundleFile*entry_bytes*` only: **86,040,117 all three times**. That took 8 to 12 s under valgrind and Rosetta, against 0.4 s natively. |
| UnityPy under callgrind? | It runs. `import UnityPy` alone is 5.089 to 5.094 billion `Ir` across 6 runs (spread about 0.09%, with or without `PYTHONHASHSEED=0`) and takes 67 to 176 s. Loading the same 7-object bundle and calling `parse_as_dict()` adds about 25 million `Ir`, which is less than the import's own run-to-run noise. The whole run is about 52x slower than native (85 s against 1.6 s). Scoping the count to the workload would need a small C shim that issues callgrind client requests. That is feasible, but decision 9 does not need it: UnityPy is wall clock only. |

**What follows.** Scoping the count to the workload function removes all measured variance. That is why the gate counts only `run_*` (5.2, step 3) and why a 1.0% band is about 100 times the worst whole-process noise seen. The emulated dev container can build and debug the gate, but it cannot produce baselines (decision 7). The Rosetta wall times above are not representative, and no comparison was timed here (decision 10).

---

## 10. Questions for the owner

1. **When do the figures go public?** The proposal: BN-1 to BN-4 now, so the gate protects performance from today, and BN-5's first published `wall.json` at the first release you call shippable.
2. **The reference host.** The proposal is GitHub's `ubuntu-24.04` runner (decision 10): public, rerunnable by anyone, and native x86-64, but a shared VM on a varying CPU. The alternative is a fixed machine of yours (the Windows PC under Docker or WSL2, which would need the oracle image there). The Mac's x86-64 container is emulated and cannot be the reference. Do you also want an arm64 table from a native arm64 container?
3. **An Editor-built bench scene.** Would you add a menu item to the fixture project? It would build a larger scene: about 200 textures imported with platform compression (real BC7, ASTC and ETC2 encoder output), a few skinned characters and some AudioClips from our tones. That would give a realistic whole-extraction row beside the synthetic one, at about 50 to 100 MB checked in, or fetched from a release asset if that is too large to commit.
4. **The gate's direction.** Should an *improvement* beyond 1% fail until the baseline is refreshed (proposed: yes, decision 8), or only warn?
5. **Where the summary sits in the README.** The proposal is a "Performance" section after "Goals", at most 10 rows.
