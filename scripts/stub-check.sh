#!/bin/sh
# Gate G6 (docs/plans/core.md section 6): no stub is left in any crate.
#
# A stub module's todo!() bodies are allowed only by its `STUB(X)` line and
# the `#![allow(clippy::todo, ...)]` under it. Deleting the comment but not
# the allow would hide a reachable todo!() from clippy, so this checks every
# Rust file of every crate for each sign:
#   - the marker `STUB(`, in code or a comment (`#[ignore = "STUB(B): ..."]`
#     marks a test that waits on another package's stub, plan section 7.1);
#   - `clippy::todo` or `clippy::unimplemented` under `allow(` or `expect(`
#     in an attribute, on one line or several (under `deny(`, as in
#     unityrs-core's own lib.rs, it is the opposite of a stub);
#   - `todo!(` or `unimplemented!(` in code; a comment may name them.
# crates/unityrs-rabex is rabex as published plus the patch ledger (plan
# section 2.4): its upstream todo!()s are fenced, unreachable from unityrs,
# and scripts/fork-check.sh owns that crate, so this check skips it.
#
#     scripts/dev sh scripts/stub-check.sh [ROOT]   # prints each hit; exits 1 if any
set -eu
root=${1:-"$(dirname "$0")/.."}
cd "$root"
hits=$(find crates -path crates/unityrs-rabex -prune -o -type f -name '*.rs' -print \
    | LC_ALL=C sort \
    | xargs -r awk '
        FNR == 1 { depth = 0; level = "" }
        {
            line = $0
            code = line
            sub(/^[ \t]+/, "", code)
            hit = (line ~ /STUB\(/)
            if (substr(code, 1, 2) != "//") {
                if (depth == 0 && code ~ /^#!?\[/) level = ""
                if (depth > 0 || code ~ /^#!?\[/) {
                    # Inside an attribute: walk its lint levels in order.
                    s = line
                    while (match(s, /(allow|expect|warn|deny|forbid)\(|clippy::(todo|unimplemented)/)) {
                        tok = substr(s, RSTART, RLENGTH)
                        if (tok ~ /^clippy::/) {
                            if (level == "allow" || level == "expect") hit = 1
                        } else {
                            level = substr(tok, 1, length(tok) - 1)
                        }
                        s = substr(s, RSTART + RLENGTH)
                    }
                    o = line; c = line
                    depth += gsub(/\[/, "", o) - gsub(/\]/, "", c)
                    if (depth < 0) depth = 0
                }
                if (line ~ /(todo|unimplemented)!\(/) hit = 1
            }
            if (hit) print FILENAME ":" FNR ": " line
        }')
if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    echo "stub-check: stubs remain" >&2
    exit 1
fi
echo "stub-check: no stubs"
