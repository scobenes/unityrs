#!/bin/sh
# Clean release build time and stripped binary size, per decoder feature, for
# docs/spikes/lzma.md. Run from the checkout root, in the dev container:
#
#   scripts/dev sh crates/unityrs-bench-lzma/sizes.sh
#
# "none" builds the bench with no decoder; each other row's extra over it, in
# brackets, is what the decoder costs. -j1 serialises the build, so its time is
# the work the decoder adds; the default -j is the wall time a developer sees.
# It writes only under target/lzma-size (gitignored).
set -eu
root=target/lzma-size
echo "| decoder | -j1 clean build s | default -j clean build s | stripped binary bytes |"
echo "|---|---:|---:|---:|"
for variant in none liblzma lzma-rust2 lzma-rust2-optimization; do
  features="--no-default-features"
  [ "$variant" = none ] || features="$features --features $variant"
  row="| $variant"
  for jobs in 1 default; do
    dir=$root/$variant-$jobs
    rm -rf "$dir"
    j=""
    [ "$jobs" = default ] || j="-j $jobs"
    start=$(date +%s.%N)
    # shellcheck disable=SC2086
    CARGO_PROFILE_RELEASE_STRIP=symbols cargo build -q --locked --release -p unityrs-bench-lzma \
      $features $j --target-dir "$dir"
    secs=$(awk -v a="$start" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')
    if [ "$variant" = none ]; then
      eval "none_$jobs=$secs"
      row="$row | $secs"
    else
      eval "base=\$none_$jobs"
      row="$row | $secs ($(awk -v a="$secs" -v b="$base" 'BEGIN { printf "%+.1f", a - b }'))"
    fi
  done
  bytes=$(stat -c %s "$dir/release/unityrs-bench-lzma")
  if [ "$variant" = none ]; then
    none_bytes=$bytes
    echo "$row | $bytes |"
  else
    echo "$row | $bytes (+$((bytes - none_bytes))) |"
  fi
done
