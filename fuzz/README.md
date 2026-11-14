# unityrs fuzz targets

`cargo-fuzz` targets for `unityrs-core`: the fuzz plan of
`docs/plans/core.md` section 6, and section 8's R9 (the LZ4 decoder against
liblz4); for `unityrs-texture`'s decoders and selection
(`docs/plans/extract.md` E15, E18, E59, E66); and for `unityrs-meshdecode`'s
vertex-data unpack and triangles (E16, E59). Any panic, timeout over 10 s,
or allocation over 2 GiB is a bug.
The `zlib_match` target also compares unityrs-imageio's derived deflate
with pristine zlib 1.2.13, byte for byte at Pillow's settings.

This crate is a workspace of its own. It builds only with a nightly
toolchain and cargo-fuzz, and nothing in it ships.

## Targets

| target | what it runs | input |
|---|---|---|
| `detect` | `detect::check_file_type` (the gzip check bounded by `max_inflate`) | any bytes |
| `zlib_match` | derived deflate against pristine zlib 1.2.13, with 257-byte input and 509-byte output windows (`test-reference`) | any bytes, capped at 4 MiB |
| `bundle` | `BundleFile::open` plus `load_all` (every block decoded, every entry parsed, nested bundles loaded), by the memory and the stream reader's rules | any bytes |
| `unitycn` | UnityCN header table construction and in-place token walk, including the virtual trailing zero and Tier U refusal; a valid synthetic signature keeps arbitrary nibble tables reachable | any bytes: the first 16 also mutate the encrypted table vector, and the first 8 select the block index |
| `serialized` | `SerializedFile::parse`, then every object's `parse_as_dict(true)` (UnityPyBoost's reader or the pure-Python one, by the object's size), by both readers' rules | any bytes |
| `typetree` | `parse_blob` (format 10 and from 12) or `parse_old`, as the format says, then `NodeList::from_rows` and `Tree::compile` | `unityrs_testkit::fuzz::blob_case`: format, endian, stored tree |
| `boost_read` | `walk::boost::read` | `unityrs_testkit::fuzz::TreeCase`: rows, ref types, value bytes |
| `python_read` | `walk::python::read_with_reader`, from the case's start, by the memory or the stream reader's rules | `TreeCase` |
| `write_after_read` | a read, `walk::write::write` under `WriteRules::MirrorReader`, the read again and the write again: the value and the bytes must come back the same (`docs/DESIGN.md`'s write rule); a writer's refusal must be its named error | `TreeCase` |
| `lz4` | `codec::decode_block` with `codec::Lz4` against liblz4 1.9.4's `LZ4_decompress_safe` (lz4-sys 1.10.0): the same verdict, the same bytes, the same error code; the differences allowed are `PARITY.md`'s LZ4 row and its limits (an output past the 8 MiB block bound is refused with `BlockOutput`) | `unityrs_testkit::fuzz::lz4_case`: declared size, block |
| `texture` | `unityrs_texture::select` and `decode` (a format outside the thirty-six it decodes, the five version 1 exports, the rest of Pillow's raw and `bcn` formats, the ETC and EAC formats and the ASTC LDR formats, decoded as one of them), under the memory bound or a meter of the decode's peak charge (`Plan::peak_charge`) less 1, the charge or plus 1: an export must be of the texture's own format, the decode's answer its `plan`'s (but a refusal of a charge past the meter's limit), the image the plan's size, mode and charge, and the meter back at 0 after it. Its data is the input, a few KiB at most, so the decodes it reaches are small: what needs more data (a crop's decompression-bomb check, a large decode) is the unit and seeded tests' (`crates/unityrs-texture/src/decode/tests.rs`, `crates/unityrs-texture/tests/limits.rs`) | `unityrs_testkit::fuzz::TextureCase`: format, size, platform, meter, platform blob, data |
| `astc` | `unityrs_texture::decode` of an ASTC LDR texture (the case's format taken as one of the twelve), under the memory bound or a meter of the decode's peak charge, its output and its tables, less 1, the charge or plus 1: the decode's answer its `plan`'s (but a refusal of a charge past the meter's limit), the image the plan's size, mode and charge, the meter back at 0 after it, so the tables are returned, and each block's texels in the image those of the block decoded alone as a one-block texture (every block of an image of up to 32, and of a larger one the first 32 and those the right and bottom edges cut): the assembly, the cut and the flip | `unityrs_testkit::fuzz::TextureCase`: format, size, platform, meter, platform blob, data |
| `meshdecode` | `walk::boost::read` of a Mesh's or a Sprite's value, then `unityrs_meshdecode::MeshHandler::process` (with or without a resource reader, either byte order, any version), `get_triangles` twice and `process` again, run last once the first's outputs drop, under the case's limits and, when the case's meter is not 0, again under a meter of `meter² × 64` bytes with the same work bound: the second `get_triangles` and the second `process` must answer as the first (but a refusal of the work it adds), the smaller meter's answer must be the first's (but a refusal of a charge past its limit), and each meter back at 0 once the outputs drop | `unityrs_testkit::fuzz::MeshCase`: flags, meter, version, resource, then a `TreeCase` |

Every run is bounded by the limits in `src/lib.rs`: 128 MiB of memory, of
input and of one block's output (`MEMORY`), and 8 MiB of decode work and of
inflate (`WORK`). A value read never reaches 128 MiB within the work bound,
so a `TreeCase` with `SMALL_MEMORY` set bounds its memory at 8 MiB instead
(`SMALL_MEMORY`), which a read of many small values reaches first: the code
that grows a charged buffer at the bound runs there. An input that reaches
a bound ends with the core's named limit error, never an abort, and an
allocation the meter did not see is what reaches libFuzzer's allocation
limit. The work bound keeps the slowest inputs (a list of small values read
to the bound, 0.6 to 1.4 s under the sanitizer) well inside the 10 s
timeout; the comments in `src/lib.rs` give the numbers.

## Seeds

The seeds are written by the testkit's builders, never taken from a corpus,
and never committed:

```sh
cargo run -p unityrs-testkit --example fuzz_seeds -- /tmp/fuzz/seeds
```

writes one folder per target: every builder output (the format matrix,
every format, codec and flag set, every Environment scene's files), the
files each bundle holds, each object's tree and bytes (with `SMALL_MEMORY`
clear and set), every stored tree in ten formats both ways round, the deep
values of `unityrs_testkit::depth`, LZ4 blocks of every output at the
exact, a larger, a smaller and a zero declared size, and textures of the
five formats and two others, at sizes from a side of 0 to past the
decoders' bound and Pillow's limits, with data a byte short, exact and a
byte long, cut to at most 4 KiB (so past a few KiB of mip 0 only the checks
before the data's are reached), on Windows, Switch and Xbox 360; the
blocks `unityrs-texture`'s ASTC oracle fixture records for each square
footprint (`crates/unityrs-texture/tests/fixtures/astc.json`: astcenc's
encoder's, random and crafted blocks of every block-mode class, partition
count, dual-plane channel, endpoint mode, void extent and error block), as
textures of each of the twelve ASTC LDR formats; and every
mesh and sprite case `unityrs-meshdecode`'s handler fixture records
(`crates/unityrs-meshdecode/tests/fixtures/process.json`, synthetic values
from UnityPy's own class trees), each under meters 0, 1, 16 and 64.
The `zlib_match` seeds are the builders' outputs (capped at 4 MiB), plus
zero and periodic inputs around match and window boundaries.

## Running

With a nightly toolchain and cargo-fuzz 0.13.2 (CI pins the nightly in
`.github/workflows/fuzz.yml`), from this folder:

```sh
cargo +nightly test --lib     # the harness's own tests (src/tests.rs)
cargo +nightly fuzz build -a
cargo +nightly fuzz run -a TARGET /tmp/fuzz/corpus/TARGET /tmp/fuzz/seeds/TARGET -- \
    -max_total_time=900 -timeout=10 -rss_limit_mb=1536 -malloc_limit_mb=2048
```

`-a` keeps debug assertions, so an integer overflow panics. The targets
build with AddressSanitizer, which reserves terabytes of address space: a
`ulimit -v` must stay above about 20 TiB (25000000000 KiB works), so bound
memory with `-rss_limit_mb` instead. `-malloc_limit_mb=2048` is the plan's
2 GiB, as in CI (which allows 3072 MB of RSS).

## A finding

libFuzzer writes the input to `artifacts/TARGET/`. Then:

1. `cargo +nightly fuzz tmin -a TARGET artifacts/TARGET/crash-...` minimizes
   it;
2. a regression test goes into the stable suite (`cargo test`), its input
   written by a testkit builder where it can be, else the minimized bytes
   as a hex literal (they are synthetic);
3. the fix gives UnityPy's answer, or the named error `PARITY.md` allows;
4. the target runs again.

A CI failure uploads its input as the run's artifact; download it and start
at step 1.

## CI

- `.github/workflows/fuzz.yml`: every night, each target for an hour, from
  its corpus of the night before (kept with `actions/cache`) and the seeds.
  A finding fails the target's job and uploads the input.
- `.github/workflows/ci.yml`'s `fuzz-build`: on every push, every target
  built and run for 30 seconds from the seeds, the harness's own tests
  (`cargo test --lib`), and `cargo fmt --check` over this crate.

## Licences

The targets are run, never distributed, so this crate's dependencies are no
part of `THIRD_PARTY_LICENSES`: cargo-about reads only the repository's
workspace, which this crate is not a member of. They are held to the same
licences all the same: CI's `licenses` job runs cargo-deny over this crate
with `deny.toml` here, the repository's allow list plus one exception,
NCSA for `libfuzzer-sys` (LLVM's libFuzzer).
