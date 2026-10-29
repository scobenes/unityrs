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
#
# Each hit is printed with its owning package, `[X] FILE:LINE: text`: the
# package a `STUB(X)` on the line names (for a blocked test, the package it
# waits on), else the one the file's first `STUB(X)` names (every stub
# module carries one at its top), else `?`, an unowned stub. A summary of
# the hits per package follows on stderr. A generated table the scaffold
# ships empty carries its own `STUB(A)` line, so it is listed until A's
# generator fills it.
#
#     scripts/dev sh scripts/stub-check.sh [ROOT]   # prints each hit; exits 1 if any
set -eu
root=${1:-"$(dirname "$0")/.."}
cd "$root"
hits=$(find crates -type f -name '*.rs' \
    | LC_ALL=C sort \
    | xargs -r awk '
        function owner_of(s) {
            if (match(s, /STUB\([A-Za-z]+\)/)) return substr(s, RSTART + 5, RLENGTH - 6)
            return ""
        }
        FNR == 1 {
            depth = 0; level = ""; first = ""
            while ((getline l < FILENAME) > 0) if (first == "") first = owner_of(l)
            close(FILENAME)
            if (first == "") first = "?"
        }
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
            if (hit) {
                who = owner_of(line)
                if (who == "") who = first
                print "[" who "] " FILENAME ":" FNR ": " line
            }
        }')
if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    summary=$(printf '%s\n' "$hits" | sed -n 's/^\[\([^]]*\)\].*/\1/p' | LC_ALL=C sort | uniq -c \
        | awk '{ printf "%s%s %s", sep, $2, $1; sep = ", " }')
    echo "stub-check: stubs remain, hits per package: $summary" >&2
    exit 1
fi
echo "stub-check: no stubs"
