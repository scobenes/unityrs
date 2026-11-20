#!/bin/sh
# Installs valgrind 3.19.0 for the instruction-count gate (docs/plans/bench.md
# decision 6), without root: Debian bookworm's valgrind_3.19.0-1_amd64.deb,
# fetched by its pool path, checked against the sha256 bookworm's package
# index records for it, and unpacked with `dpkg -x` into PREFIX. Run the
# result as
#
#     VALGRIND_LIB=PREFIX/usr/libexec/valgrind PREFIX/usr/bin/valgrind ...
#
# which tools/bench/callgrind.py does when given --valgrind PREFIX.
#
#     sh tools/bench/install_valgrind.sh PREFIX
#
# A PREFIX that already holds this version is left as it is. Needs curl (or
# wget), sha256sum and dpkg-deb, which debian:bookworm has. x86-64 only: the
# gate's reference host is GitHub's ubuntu-24.04 x86-64 runner (decision 8).
set -eu

VERSION=3.19.0-1
DEB=valgrind_${VERSION}_amd64.deb
URL=https://deb.debian.org/debian/pool/main/v/valgrind/$DEB
SHA256=324842f2308a1e42abf2d81ff5eb19a9475fc9db95647f4d9da71444eda4087f

if [ "$#" -ne 1 ]; then
  echo "usage: sh tools/bench/install_valgrind.sh PREFIX" >&2
  exit 2
fi
prefix=$1
if [ "$(uname -m)" != x86_64 ]; then
  echo "install_valgrind: x86-64 only, not $(uname -m)" >&2
  exit 1
fi
if [ -x "$prefix/usr/bin/valgrind" ] && [ "$(cat "$prefix/.deb-sha256" 2>/dev/null)" = "$SHA256" ]; then
  echo "install_valgrind: $DEB is already in place"
  exit 0
fi

mkdir -p "$prefix"
deb="$prefix/$DEB"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL --connect-timeout 30 --max-time 300 --retry 3 -o "$deb" "$URL"
else
  wget -q -T 300 -t 3 -O "$deb" "$URL"
fi
printf '%s  %s\n' "$SHA256" "$deb" | sha256sum --check --strict --quiet
dpkg-deb -x "$deb" "$prefix"
rm -f "$deb"
printf '%s\n' "$SHA256" > "$prefix/.deb-sha256"
VALGRIND_LIB="$prefix/usr/libexec/valgrind" "$prefix/usr/bin/valgrind" --version
