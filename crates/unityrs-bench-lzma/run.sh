#!/bin/sh
# Reproduces docs/spikes/lzma.md from the checkout root, in the dev container:
#
#   scripts/dev sh crates/unityrs-bench-lzma/run.sh [ROUNDS]
#
# It needs the private corpus at /corpus, and network for one crates.io query.
# It writes under out/lzma, and cargo writes its two builds under target/
# (target/release, target/lzma-safe); both directories are gitignored. Each
# round runs every implementation once per set and thread count, so that a
# slow spell on a shared box lands on all of them.
set -eu
here=crates/unityrs-bench-lzma
out=out/lzma
rounds=${1:-5}
py="python3 $here/unitypy_lzma.py"
started=$(date +%s)
mkdir -p "$out"

# The box and toolchains, for the method section.
{
  echo "date: $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')"
  echo "vcpus: $(nproc)"
  echo "os: $(. /etc/os-release && echo "$PRETTY_NAME")"
  echo "container memory cap: $(cat /sys/fs/cgroup/memory.max) bytes"
  rustc --version
  echo "$(python3 --version), UnityPy $(python3 -c 'import UnityPy; print(UnityPy.__version__)')"
} >"$out/env.txt"

cargo build --locked --release -p unityrs-bench-lzma
cargo build --locked --release -p unityrs-bench-lzma --target-dir target/lzma-safe \
  --no-default-features --features lzma-rust2
fast=target/release/unityrs-bench-lzma
safe=target/lzma-safe/release/unityrs-bench-lzma

# Where cargo unpacked a dependency's source.
source_of() {
  cargo metadata --locked --format-version 1 | python3 -c '
import json, sys
pkgs = json.load(sys.stdin)["packages"]
print(next(p["manifest_path"] for p in pkgs if p["name"] == sys.argv[1]).rsplit("/", 1)[0])' "$1"
}
xz=$(source_of liblzma-sys)
src=$xz/xz/src
dirs="common lzma lz check delta rangecoder simple"
# The xz release liblzma-sys compiles in, built as a shared library with the
# flags the cc crate uses for a release build, so that Python's lzma module
# can run on the same liblzma as the Rust crate (LD_LIBRARY_PATH below). The
# file list is liblzma-sys's build.rs: no threads, no table generators.
files="$src/common/tuklib_cpucores.c $src/common/tuklib_physmem.c $(
  for d in $dirs; do find "$src/liblzma/$d" -maxdepth 1 -name '*.c'; done | sort |
    grep -Ev '(mt|tablegen|crc32_small|crc64_small|crc_clmul_consts_gen)\.c$'
)"
mkdir -p "$out/xz"
# shellcheck disable=SC2046,SC2086
gcc -O3 -std=c99 -fPIC -shared -DHAVE_CONFIG_H=1 -DNDEBUG -I"$xz" -I"$src/common" \
  -I"$src/liblzma/api" $(for d in $dirs; do printf ' -I%s' "$src/liblzma/$d"; done) \
  -Wl,-soname,liblzma.so.5 -Wl,--version-script="$src/liblzma/liblzma_linux.map" \
  -o "$out/xz/liblzma.so.5" $files

# What the crates are, for the build and upkeep section.
# shellcheck disable=SC2086
{
  echo "liblzma-sys source: $(du -sh "$xz" | cut -f1)"
  echo "liblzma-sys C files compiled: $(echo $files | wc -w), licences:" \
    $(grep -h -m1 'SPDX-License-Identifier' $files | sort | uniq -c)
  echo "liblzma-sys build.rs: $(grep -h 'const MIN_LIBLZMA' "$xz/build.rs")"
  awk '/^[0-9]+[.][0-9]+[.][0-9]+[a-z]* [(]/ { release = $1 }
    /LZMA decoder: Speed optimizations/ { line = $0; getline; print "xz NEWS, " release ":" line $0 }' \
    "$xz/xz/NEWS" | tr -s ' '
  echo "lzma-rust2 lib.rs: $(grep -h 'forbid(unsafe_code)' "$(source_of lzma-rust2)/src/lib.rs")"
  $py crates || echo "crates.io not reached"
} >"$out/crates.txt"

# Synthetic bundles, built from no game data: header shapes the finder must
# read as UnityPy does, then the blocks on which the decoders and UnityPy part
# ways (codec.rs). The second set is expected to fail; its output is the record.
$py synth --out-dir "$out/synth"
for case in shapes contract; do
  $py oracle --list "$out/synth/$case.txt" --out "$out/synth/$case.tsv"
done
: >"$out/synth/shapes-verify.txt"
for bin in $fast $safe; do
  $bin verify --list "$out/synth/shapes.txt" --expect "$out/synth/shapes.tsv" \
    >>"$out/synth/shapes-verify.txt" || { cat "$out/synth/shapes-verify.txt"; exit 1; }
done
cat "$out/synth/shapes-verify.txt"
for bin in $fast $safe; do
  $bin verify --list "$out/synth/contract.txt" --expect "$out/synth/contract.tsv" || true
done >"$out/synth/contract-verify.txt"
grep -q "blocks failed" "$out/synth/contract-verify.txt"

$py sets --out-dir "$out" >"$out/sets.txt"
cat "$out/sets.txt"
$py oracle --list "$out/all.txt" --out "$out/oracle.tsv"
: >"$out/verify.txt"
for bin in $fast $safe; do
  $bin verify --list "$out/all.txt" --expect "$out/oracle.tsv" >>"$out/verify.txt" ||
    { cat "$out/verify.txt"; exit 1; }
done
cat "$out/verify.txt"

: >"$out/runs.tsv"
: >"$out/blocks.tsv"
for round in $(seq "$rounds"); do
  echo "round $round load average: $(cut -d' ' -f1-3 /proc/loadavg)" >>"$out/env.txt"
  for set in largest20 sample200; do
    for threads in 1 8; do
      echo "round $round, $set, $threads threads" >&2
      list="--list $out/$set.txt --set $set --threads $threads --reps 1"
      blocks=""
      [ "$threads" = 1 ] && blocks="--per-block $out/blocks.tsv"
      # shellcheck disable=SC2086
      {
        $fast bench $list $blocks
        $safe bench $list $blocks
        $py bench $list $blocks
        LD_LIBRARY_PATH=$out/xz $py bench $list $blocks
      } >>"$out/runs.tsv"
    done
  done
done
echo "after the last round, load average: $(cut -d' ' -f1-3 /proc/loadavg)" >>"$out/env.txt"
$py report "$out/runs.tsv" --per-block "$out/blocks.tsv" >"$out/report.md"
echo "run time: $(($(date +%s) - started)) s" >>"$out/env.txt"
cat "$out/report.md" "$out/env.txt"
