# LZMA decoder: liblzma or lzma-rust2

**Status:** measured on 2026-10-29 (UTC) for DESIGN.md section 4, which plans `liblzma` (static) for UnityFS LZMA blocks and asks for a P0 benchmark against `lzma-rust2`. The code is `crates/unityrs-bench-lzma` (`publish = false`). Section 5 names the script that printed each number.

**Recommendation: keep `liblzma` with the `static` feature, as DESIGN.md says.**
- It is 1.25-1.31x as fast as lzma-rust2 on both sets, on one core and 8-way, and 1.14-1.16x on the single largest stream.
- Its costs are small for this project. The full build needs a C compiler anyway (static zlib for Tier A PNG bytes, libwebp, astcenc). The C adds 0.8 s to a clean build at the default `-j`, and its licence is 0BSD.
- lzma-rust2 without its `optimization` feature is the fallback, if unityrs-core ever has to build without a C compiler (wasm, for example). It gave identical bytes on every corpus block, and liblzma is 1.26-1.31x as fast. As a fallback it needs two adjustments, where liblzma and the crate differ: it must reject properties with lc + lp > 4, which the crate accepts, and clamp a dictionary size above 0xFFFFFFF0, which the crate refuses (section 2).

## 1. What is decoded

- **The block.** Every corpus bundle that holds LZMA holds exactly one LZMA data block: `verify` finds 1,097 blocks, and 1,097 of the 1,100 bundles have one.
  - The block is 5 property bytes, then raw LZMA1 with no size.
  - UnityPy 1.25.2 decodes it in `CompressionHelper.decompress_lzma`: Python's `lzma.LZMADecompressor(FORMAT_RAW, FILTER_LZMA1)`, with no output size, so liblzma decodes until the input runs out.
- **In this corpus**, all 1,097 LZMA blocks use properties `0x5D` (lc 3, lp 0, pb 2) and a 524,288-byte dictionary, and UnityPy's output always has the size the blocks-info gives. The Rust decoders here fill a buffer of exactly that size and stop. Section 2 shows where that differs from UnityPy.
- **Implementations:**

| name | what |
|---|---|
| `liblzma` | crate `liblzma` 0.4.9, `static`: xz's C liblzma **5.8.4**, compiled by `cc`. The raw LZMA1 decoder (`lzma_raw_decoder`) is the same path Python takes. |
| `lzma-rust2-reader` | crate `lzma-rust2` 0.24.0, `default-features = false, features = ["std", "optimization"]`, through `LzmaReader` (`io::Read`) |
| `lzma-rust2-stream` | the same crate through `LzmaStream` (push, shaped like `lzma_code`) |
| `…-safe` | the two above without `optimization`, which leaves the crate `forbid(unsafe_code)` |
| `unitypy-liblzma-5.4.1` | UnityPy's `decompress_lzma` as shipped in the oracle image: Python 3.11 `lzma` on Debian 12's liblzma 5.4.1. This **is** "Python's lzma module"; UnityPy adds only a 5-byte header parse. |
| `unitypy-liblzma-5.8.4` | the same Python call, run on xz 5.8.4 (liblzma-sys's sources built as `liblzma.so.5` with `-O3`, through `LD_LIBRARY_PATH`) to separate the library version from the language |

Both lzma-rust2 paths make two adjustments before they decode (`codec.rs`). They reject properties with lc + lp > 4 (`check_lc_lp`), as liblzma does; the crate alone would accept them. And they clamp a dictionary size above 0xFFFFFFF0 to the output size, or the crate's 4,096-byte minimum if that is larger (`dict_size`); liblzma takes such a size as it is, and the crate alone would refuse it.

## 2. Correctness

- **Equal to UnityPy on every corpus block:**
  - Input: all 1,100 UnityFS files in the corpus: 1,096 corpus bundles and UnityPy's 4 sample bundles, 3 of which hold no LZMA block.
  - Result: 1,097 LZMA blocks, 1,345,877,135 bytes out. Every decoder above (`liblzma`, both lzma-rust2 APIs, with and without `optimization`) gave the blocks-info size and the sha256 of UnityPy's own output for every block: **0 blocks failed**.
  - The oracle side captures each block inside UnityPy's `BundleFile.decompress_data`, so it is UnityPy's own code path, not a re-implementation.
- **`verify` checks both ways.** Every block the finder reports needs exactly one oracle row, and every oracle row needs exactly one reported block, so a finder that drops a block fails. An extra, repeated or malformed row fails too. The verdict counts failed blocks, not failure lines. `cargo test` covers each of these, and a wrong sha256, on synthetic bundles.
- **The finder reads headers by UnityPy's rules.**
  - Format 6 is aligned from engine 2019.4.15 on. The archive flags are read as `ArchiveFlagsOld` before 2020.3.34, 2021.3.2 and 2022.1.1, and under that set 0x200 means encryption, not padding.
  - The finder returns an error for an encrypted bundle, a format other than 6-8, an engine version UnityPy would replace with its fallback, and blocks-info compression other than none, LZ4 and LZ4HC.
  - It also returns an error for an engine version UnityPy might read differently from its digits. UnityPy packs the fields into one integer and reads each back masked, so a field too wide for its slot spills into the next; and Python's `\d` matches every Unicode digit. So a field above 0xFFFF (a type number above 0xFF) or a non-ASCII character where UnityPy might take it for a digit is an error: `2019.4.14f65536` and `2019.4.1５f1` both read as 2019.4.15 in UnityPy. So is a header string of 32,767 bytes or more, which UnityPy cuts short.
  - The corpus does not exercise most of these rules, so `unitypy_lzma.py synth` builds 7 header shapes from no game data: format 6 from 2019.4.20 with raw, LZ4HC and at-end blocks-info; format 6 from 2019.4.14; format 7 under the old flag set; and format 8 with padding and LZ4 or LZ4HC blocks-info at either end. On all 14 of their blocks, UnityPy and both builds' `verify` agree: 0 blocks failed.
- **Where the decoders differ from UnityPy.** UnityPy passes no output size. It returns whatever liblzma yields from all of the block, ignoring the blocks-info size, and liblzma validates the properties (lc + lp ≤ 4) but takes any 32-bit dictionary size. The decoders here stop when a buffer of the blocks-info size is full. The two agree whenever a stream yields exactly the blocks-info size, as every corpus block does. `synth` builds the cases where they do not, from 200,000 bytes of synthetic data in the corpus layout, and `verify` records each decoder's result against UnityPy's:

  | synthetic block | UnityPy 1.25.2 | `liblzma` | `lzma-rust2`, both APIs, both builds |
  |---|---|---|---|
  | truncated: the stream cut in half, the blocks-info size kept | returns the 100,984 bytes it could decode, with no error | fails: `stalled at 100984 of 200000 bytes` | fails: `unexpected end of LZMA input` (reader), `truncated LZMA stream` (stream) |
  | over-long: the blocks-info says 100,000, which ends inside a match | returns all 200,000 | returns the first 100,000 | fails: `end reached but not decoder finished` |
  | over-long: the blocks-info says 101,304, which ends between two literals | returns all 200,000 | returns the first 101,304 | returns the first 101,304 |
  | properties `0x5F` (lc 5 + lp 0 > 4), on a stream that decodes the same under lc 3 and lc 5 | raises `LZMAError` | fails: liblzma rejects the properties | fails, through `check_lc_lp`. Without that check the crate decodes the block (a `codec.rs` test). |
  | dictionary size `0xFFFFFFF1`, on the stream of the first row, uncut | returns all 200,000 | returns all 200,000 | returns all 200,000, through the `dict_size` clamp. Without it the crate fails: `dict size too large` (a `codec.rs` test). |

  So against UnityPy, `verify` fails the first three cases for every decoder and passes the last two. unityrs-core has to choose: follow UnityPy and decode to the end of the input whatever the blocks-info says, or reject a size mismatch. Either way, a reader built on lzma-rust2 needs both adjustments: the lc + lp check and the dictionary clamp. `cargo test` pins the decoders' side, and `test_unitypy_lzma.py` pins UnityPy's.
- **The library version does not change the output.** UnityPy on liblzma 5.4.1 and both Rust crates agree byte for byte on every corpus block. The oracle image's old liblzma affects UnityPy's speed only, not parity.

## 3. Speed

MB/s is decompressed output, in 10^6 bytes. Each cell is the median of 5 rounds; the full table, with ranges, is in section 6.

| implementation | largest20, 1 core | sample200, 1 core | largest20, 8-way | sample200, 8-way |
|---|---:|---:|---:|---:|
| `liblzma` (xz 5.8.4) | **138.2** | **131.6** | **1014.8** | **1007.6** |
| `lzma-rust2-reader` | 108.1 | 100.8 | 811.1 | 780.6 |
| `lzma-rust2-stream` | 109.2 | 102.2 | 797.6 | 792.0 |
| `lzma-rust2-reader-safe` | 107.8 | 101.3 | 794.6 | 769.6 |
| `lzma-rust2-stream-safe` | 108.1 | 101.5 | 807.4 | 783.2 |
| `unitypy-liblzma-5.4.1` (UnityPy as shipped) | 81.3 | 75.4 | 579.3 | 559.9 |
| `unitypy-liblzma-5.8.4` | 137.2 | 129.0 | 933.9 | 929.1 |

How many times as fast liblzma is, from the same medians:

| implementation | largest20, 1 core | sample200, 1 core | largest20, 8-way | sample200, 8-way |
|---|---:|---:|---:|---:|
| `lzma-rust2-reader` | 1.28 | 1.31 | 1.25 | 1.29 |
| `lzma-rust2-stream` | 1.27 | 1.29 | 1.27 | 1.27 |
| `lzma-rust2-reader-safe` | 1.28 | 1.30 | 1.28 | 1.31 |
| `lzma-rust2-stream-safe` | 1.28 | 1.30 | 1.26 | 1.29 |
| `unitypy-liblzma-5.4.1` | 1.70 | 1.75 | 1.75 | 1.80 |
| `unitypy-liblzma-5.8.4` | 1.01 | 1.02 | 1.09 | 1.08 |

By block size, one core. Each value is the sum of bytes over the sum of per-block median times:

| set | blocks | n | MB out | liblzma | lzma-rust2 (reader) | UnityPy 5.4.1 | Python on 5.8.4 |
|---|---|---:|---:|---:|---:|---:|---:|
| sample200 | < 64 kB | 51 | 0.8 | 100.9 | 77.5 | 60.3 | 97.3 |
| sample200 | 64 kB-1 MB | 100 | 36.3 | 169.7 | 129.8 | 100.0 | 166.5 |
| sample200 | ≥ 1 MB | 49 | 174.7 | 126.4 | 96.8 | 71.9 | 123.9 |
| largest20 | largest block | 1 | 69.4 | 260.8 | 224.1 | 180.2 | 256.0 |

What the numbers say:
1. **liblzma is 1.25-1.31x as fast as lzma-rust2 on both sets,** on one core and 8-way, against both of lzma-rust2's APIs with and without `optimization`. On the single largest stream (69.4 MB) the margin is smaller: 1.14-1.16x. In sample200's three size bins the two keep the same order and a similar gap, so the gap is in steady decoding, not in starting a stream. lzma-rust2's documentation says that, against the `liblzma` crate, it "has improved decoding speed"; on these streams it does not.
2. **lzma-rust2's `optimization` feature buys nothing measurable here.** liblzma is 1.25-1.31x as fast as the builds with it and 1.26-1.31x as fast as those without, and neither build is ahead in every cell. Its two APIs are the same speed.
3. **UnityPy's 75-81 MB/s comes from Debian's liblzma 5.4.1, not from Python.** The same Python call on xz 5.8.4 runs at 129-137 MB/s on one core, where liblzma is only 1.01-1.02x as fast, and 1.08-1.09x at 8-way. xz's NEWS for 5.6.0 lists "LZMA decoder: Speed optimizations to the C code and added GCC & Clang compatible inline assembly for x86-64". So the Rust crate's 1.70-1.80x over UnityPy as shipped comes from the bundled library, not from leaving Python.
4. **All of them scale about 7x on 8 workers.** 8-way over 1 core is 7.3-7.8 for the Rust decoders, 7.1-7.4 for UnityPy as shipped and 6.8-7.2 for Python on 5.8.4.
5. **Over a whole run the gap is seconds.** At sample200 rates, each GB of decompressed output costs:

   | | core-seconds | wall seconds on 8 workers |
   |---|---:|---:|
   | liblzma | 7.60 | 0.99 |
   | lzma-rust2-reader | 9.92 | 1.28 |
   | UnityPy as shipped | 13.27 | 1.79 |

   The longest single stream (69.4 MB) takes 0.266 s with liblzma and 0.305-0.310 s with lzma-rust2.

## 4. Build, licence, size, upkeep

**Build cost.** Measured by `sizes.sh`, three runs: release builds from clean, stripped. Each figure is the extra over the same binary with no decoder, from the lowest to the highest of the three runs. On the shared box the `-j1` extras moved a little from run to run; the default-`-j` extras and the binary sizes did not.

| | `liblzma` 0.4.9, `static` | `lzma-rust2` 0.24.0, `std` only |
|---|---|---|
| what is compiled | Rust bindings plus 77 C files of xz liblzma 5.8.4, through `cc`. The bindings are pre-generated, so no libclang. | pure Rust; no dependencies with these features |
| toolchain | a C99 compiler for every target. Build dependencies: `cc`, `pkg-config`, `jobserver`, `shlex`, `find-msvc-tools`. | rustc only |
| clean build, `-j1` | +5.5 to +5.7 s | +1.5 to +1.7 s, with or without `optimization` |
| clean build, default `-j` | +0.8 s in all three runs | +0.1 s in all three runs, with or without `optimization` |
| stripped binary | +101,008 bytes | +120,192 bytes, both APIs linked (+123,584 with `optimization`) |

**Licence and upkeep.**

| | `liblzma` | `lzma-rust2` |
|---|---|---|
| licence | bindings MIT OR Apache-2.0. All 77 compiled C files are `SPDX-License-Identifier: 0BSD`, which asks for no notice. The 7.6 MiB crate source also carries xz's GPL/LGPL scripts and translations; none of them is compiled. | Apache-2.0: ship its licence text (DESIGN.md's `THIRD_PARTY_LICENSES` already covers Apache-2.0 dependencies) |
| `deny.toml` | allowed: the crate declares MIT OR Apache-2.0. `cargo-about` needs a 0BSD clarification for the vendored C. | allowed: Apache-2.0 is on the list |
| memory safety | C, outside Rust's checks. The xz source carries OSS-Fuzz targets (`xz/tests/ossfuzz`). | `forbid(unsafe_code)` without `optimization` |
| releases | follows xz upstream: 0.4.5 to 0.4.9 came out between 2025-09-28 and 2026-10-19 | breaking 0.x bumps, weekly in October 2026: 0.22.0, 0.23.0 and 0.24.0 on 2026-10-11, 10-18 and 10-25 |
| most-downloaded reverse dependencies include | apache-avro, rpm, datafusion, async-compression | backhand, noodles-cram, sevenz-rust2, zip |

Notes:
- **The C compiler is already a project requirement.** `unityrs-imageio` needs static zlib (Tier A PNG bytes) and libwebp, and `unityrs-texture` needs astcenc. liblzma only makes `unityrs-core` itself need one.
- **Use `static`.** Without it, liblzma-sys looks for a system liblzma of at least 5.8.0 (`MIN_LIBLZMA` in its `build.rs`) through pkg-config, and builds its own only if none is found. `static` always builds the pinned 5.8.4.
- **Writing is not measured.** DESIGN.md requires the `lzma` packer to be equal only after decompression, and both crates can encode. Byte-equal output with UnityPy's `compress_lzma` would depend on Python's liblzma version anyway.

## 5. Method

- **Sets.** A corpus bundle that repeats another byte for byte (367 do) is timed once.
  - **largest20:** the 20 largest of the remaining bundles. 131.7 MB compressed, 405.4 MB out; blocks of 11.7-69.4 MB.
  - **sample200:** 200 of the rest, drawn with seed 20261028. 75.0 MB compressed, 211.7 MB out; blocks of 0.005-11.6 MB, median 0.29 MB.
  - The corpus is a selection of bundles, not a whole build, so its totals say nothing about a build; the per-GB costs above carry over.
- **Timing.**
  - Compressed blocks are in memory before the clock starts, and each decode writes a freshly allocated output buffer, as a reader does per bundle.
  - **1 core** is the sum of per-block times in one thread.
  - **8-way** is the wall time of 8 workers taking blocks largest first: Rust threads, and forked Python processes. Both pools are started and warmed before the clock starts, and the Rust threads are reused from rep to rep.
  - Each of the 5 rounds runs every implementation once per set and thread count, in turn, so that a slow spell on the shared box lands on all of them.
- **Box.**
  - An Intel Core Ultra 9 285K under WSL2 with 24 vCPUs, in the `unityrs-dev` container (Debian 12) with a 12 GiB memory cap.
  - Other sessions shared the box: the 1-minute load average was 1.5-1.9 at the start of each round.
  - The CPU has performance and efficiency cores, and WSL2 does not tie a vCPU to either kind, so the one-core figures are what the host scheduler gives one busy thread.
- **Toolchains.** Rust 1.99.1, release profile (opt-level 3, no LTO; `cc` compiles xz with `-O3`). Python 3.11.17, UnityPy 1.25.2.

**Reproduce.** This needs the private corpus at `/corpus`, and network for one crates.io query. Run from the checkout root inside the container (`scripts/dev sh -c 'cd /work/... && ...'` for a worktree):

    sh crates/unityrs-bench-lzma/run.sh 5    # oracle, verify, 5 rounds, report: 716 s in this run
    sh crates/unityrs-bench-lzma/sizes.sh    # build times and binary sizes; run three times here
    cargo test --locked -p unityrs-bench-lzma
    python3 -m unittest discover -s crates/unityrs-bench-lzma

`run.sh` writes under `out/lzma/`, and cargo writes its two builds under `target/` (`target/release`, `target/lzma-safe`). `sizes.sh` writes under `target/lzma-size`. All three are gitignored. No output holds bundle bytes; `oracle.tsv` holds paths, lengths and sha256s.

Where each number comes from, all under `out/lzma/`:
- `env.txt`: the CPU, vCPUs, OS, memory cap and toolchain versions, the load average at each round, and the run time.
- `sets.txt`: the file counts, the 367 repeated bundles, and each set's compressed MB.
- `verify.txt`: bundles, blocks, bytes out, properties and dictionary size, and 0 blocks failed, for both builds.
- `synth/shapes-verify.txt` and `synth/contract-verify.txt`: the 14 synthetic header-shape blocks, and the decode-contract table. Its sizes (200,000, 100,000 and 101,304) and the dictionary size 0xFFFFFFF1 are constants in `unitypy_lzma.py synth`; 0xFFFFFFF0 and 4,096 are lzma-rust2's `DICT_SIZE_MAX` and `DICT_SIZE_MIN`.
- `report.md` (`unitypy_lzma.py report`): every MB/s, every "times as fast" ratio, the scaling, the per-GB costs, the block-size ranges and the largest block's seconds.
- `crates.txt`: the 77 compiled C files and their licence, the crate's source size, `MIN_LIBLZMA`, the xz NEWS line, lzma-rust2's `forbid(unsafe_code)`, and the crates.io release dates and reverse dependencies.
- The crate versions are the pins in `Cargo.toml`. The xz versions 5.4.1 and 5.8.4 are what `lzma_version_string` returned in each Python bench, which names its rows by it.
- The build-cost table is three runs of `sizes.sh`, which prints each extra in brackets.
- The finder's limits are in `unityfs.rs`, taken from UnityPy 1.25.2's source: 0xFFFF for a version field and 0xFF for the type number (the slots `UnityVersion` packs them into), and 32,767 bytes for a header string (`read_string_to_null`'s `max_length`). `test_unitypy_lzma.py` pins how UnityPy reads `2019.4.14f65536`, `67555.4.15f1`, `2019.4.1５f1` and `65536.0.0`.

## 6. Full results

As `unitypy_lzma.py report` printed them. The range is the slowest and fastest of the 5 rounds.

| set | threads | implementation | blocks | MB out | reps | median s | MB/s (median) | MB/s (range) |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| largest20 | 1 | liblzma | 20 | 405.4 | 5 | 2.932 | 138.2 | 136.6-139.2 |
| largest20 | 1 | lzma-rust2-reader | 20 | 405.4 | 5 | 3.749 | 108.1 | 107.0-108.5 |
| largest20 | 1 | lzma-rust2-reader-safe | 20 | 405.4 | 5 | 3.759 | 107.8 | 107.3-108.4 |
| largest20 | 1 | lzma-rust2-stream | 20 | 405.4 | 5 | 3.711 | 109.2 | 108.0-109.5 |
| largest20 | 1 | lzma-rust2-stream-safe | 20 | 405.4 | 5 | 3.751 | 108.1 | 107.5-108.3 |
| largest20 | 1 | unitypy-liblzma-5.4.1 | 20 | 405.4 | 5 | 4.985 | 81.3 | 81.0-81.6 |
| largest20 | 1 | unitypy-liblzma-5.8.4 | 20 | 405.4 | 5 | 2.955 | 137.2 | 134.9-137.8 |
| largest20 | 8 | liblzma | 20 | 405.4 | 5 | 0.399 | 1014.8 | 1002.5-1046.8 |
| largest20 | 8 | lzma-rust2-reader | 20 | 405.4 | 5 | 0.500 | 811.1 | 790.7-817.1 |
| largest20 | 8 | lzma-rust2-reader-safe | 20 | 405.4 | 5 | 0.510 | 794.6 | 782.9-801.8 |
| largest20 | 8 | lzma-rust2-stream | 20 | 405.4 | 5 | 0.508 | 797.6 | 792.9-808.6 |
| largest20 | 8 | lzma-rust2-stream-safe | 20 | 405.4 | 5 | 0.502 | 807.4 | 777.7-814.2 |
| largest20 | 8 | unitypy-liblzma-5.4.1 | 20 | 405.4 | 5 | 0.700 | 579.3 | 568.0-586.8 |
| largest20 | 8 | unitypy-liblzma-5.8.4 | 20 | 405.4 | 5 | 0.434 | 933.9 | 888.4-945.5 |
| sample200 | 1 | liblzma | 200 | 211.7 | 5 | 1.609 | 131.6 | 131.2-132.6 |
| sample200 | 1 | lzma-rust2-reader | 200 | 211.7 | 5 | 2.101 | 100.8 | 100.8-101.7 |
| sample200 | 1 | lzma-rust2-reader-safe | 200 | 211.7 | 5 | 2.090 | 101.3 | 100.6-101.4 |
| sample200 | 1 | lzma-rust2-stream | 200 | 211.7 | 5 | 2.072 | 102.2 | 101.3-102.6 |
| sample200 | 1 | lzma-rust2-stream-safe | 200 | 211.7 | 5 | 2.086 | 101.5 | 101.1-101.9 |
| sample200 | 1 | unitypy-liblzma-5.4.1 | 200 | 211.7 | 5 | 2.809 | 75.4 | 74.3-76.0 |
| sample200 | 1 | unitypy-liblzma-5.8.4 | 200 | 211.7 | 5 | 1.641 | 129.0 | 126.3-130.1 |
| sample200 | 8 | liblzma | 200 | 211.7 | 5 | 0.210 | 1007.6 | 981.7-1014.9 |
| sample200 | 8 | lzma-rust2-reader | 200 | 211.7 | 5 | 0.271 | 780.6 | 770.6-791.7 |
| sample200 | 8 | lzma-rust2-reader-safe | 200 | 211.7 | 5 | 0.275 | 769.6 | 756.0-783.5 |
| sample200 | 8 | lzma-rust2-stream | 200 | 211.7 | 5 | 0.267 | 792.0 | 788.9-802.9 |
| sample200 | 8 | lzma-rust2-stream-safe | 200 | 211.7 | 5 | 0.270 | 783.2 | 762.3-791.4 |
| sample200 | 8 | unitypy-liblzma-5.4.1 | 200 | 211.7 | 5 | 0.378 | 559.9 | 554.3-571.2 |
| sample200 | 8 | unitypy-liblzma-5.8.4 | 200 | 211.7 | 5 | 0.228 | 929.1 | 891.2-942.0 |
