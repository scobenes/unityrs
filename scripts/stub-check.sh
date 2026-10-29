#!/bin/sh
# Gate G6 (docs/plans/core.md section 6): no stub is left in any crate.
#
# A stub module's todo!() bodies are allowed only by its `STUB(X)` line and
# the `#![allow(clippy::todo, ...)]` under it. Deleting the comment but not
# the allow would hide a reachable todo!() from clippy, so this checks every
# Rust file of every crate for all four signs: the marker, the allow, and the
# macros themselves. No test may use todo!() or unimplemented!() either.
#
#     scripts/dev sh scripts/stub-check.sh     # prints each hit; exits 1 if any
set -eu
cd "$(dirname "$0")/.."
if grep -rnE 'STUB\(|clippy::todo|clippy::unimplemented|todo!\(|unimplemented!\(' crates --include='*.rs'; then
    echo "stub-check: stubs remain" >&2
    exit 1
fi
echo "stub-check: no stubs"
