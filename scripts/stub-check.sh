#!/bin/sh
# Gate G6 (docs/plans/core.md section 6): no stub is left in any crate.
#
# A stub module's todo!() bodies were allowed only by its `STUB(X)` line and
# the `#![allow(clippy::todo, ...)]` under it. This checks every Rust file of
# every crate for each sign of one:
#   - the marker `STUB(`, in code or a comment (`#[ignore = "STUB(B): ..."]`
#     marks a test that waits on another package's stub, plan section 7.1);
#   - `clippy::todo`, `clippy::unimplemented` or the group that holds them,
#     `clippy::restriction`, directly under `allow(`, `expect(` or `warn(` in
#     an attribute, `cfg_attr` included, whatever whitespace or comments lie
#     between the path's tokens (under `deny(` or `forbid(`, as in
#     unityrs-core's own lib.rs, it is the opposite of a stub);
#   - `todo!` or `unimplemented!` invoked with any delimiter, `()`, `[]` or
#     `{}`, by any path (`core::todo!`), whatever whitespace or comments lie
#     between its tokens, on one line or several;
#   - either macro imported under another name (`use core::todo as later;`,
#     on one line or several), whose calls no name scan would see;
#   - either name as a word of its own anywhere inside a macro call's
#     delimiters (`call!(todo)`) or a `macro_rules!` definition, or inside
#     an attribute other than as a `clippy::` lint path: a macro_rules! body
#     can put a `!` after a name it is handed (`$m!()`) or after one it
#     holds (`todo $bang`), and then the call is written nowhere a name
#     scan reads, and clippy, which lints a call by the macro whose
#     expansion it starts, lints `call!`, not `todo!`. Neither name has
#     another use in the crates, so a variable of either name inside a
#     macro call fails too.
# Each file is lexed as Rust is first: comments (nested block comments
# included), strings (raw, byte and C strings included) and character
# literals are blanked, so they may name the macros and the lints freely,
# and a comment between two tokens is whitespace. The workspace's lints
# (Cargo.toml `[workspace.lints.clippy]`, which every crate inherits) forbid
# both macros as well, so no attribute can allow them again: `cargo clippy`
# fails on any reachable call written in the code, an aliased one included,
# and this scan sees a call a macro_rules! body writes or assembles. Not
# read: what a procedural macro (the crates use thiserror's derive) or
# a dependency's own macro expands to, and files other than `.rs` that
# `include!` reads (clippy lints a call written in one).
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
    | LC_ALL=C xargs -r awk '
        BEGIN {
            q = sprintf("%c", 39)
            # The first token in code that starts a comment, a string, a raw
            # string or a character literal.
            tok = "//|/\\*|[bc]?r#*\"|[bc]?\"|" q
            ws = "[ \t\r\n]"
        }
        function owner_of(s) {
            if (match(s, /STUB\([A-Za-z]+\)/)) return substr(s, RSTART + 5, RLENGTH - 6)
            return ""
        }
        function blank(n,    b) {
            b = ""
            while (n-- > 0) b = b " "
            return b
        }
        # The line with its comments, strings and character literals blanked
        # to spaces, columns kept. The state carries over lines: st is "" in
        # code, "block" in a comment bdepth deep, "str" in a string and "raw"
        # in a raw string, which rawend closes.
        function lex(s,    out, t, n, p, c, w) {
            out = ""
            while (s != "") {
                if (st == "block") {
                    if (!match(s, /\/\*|\*\//)) { out = out blank(length(s)); break }
                    bdepth += substr(s, RSTART, 2) == "/*" ? 1 : -1
                    out = out blank(RSTART + 1)
                    s = substr(s, RSTART + 2)
                    if (bdepth == 0) st = ""
                } else if (st == "str") {
                    if (!match(s, /[\\"]/)) { out = out blank(length(s)); break }
                    # An escape is the backslash and the character after it.
                    n = substr(s, RSTART, 1) == "\\" ? RSTART + 1 : RSTART
                    if (n == RSTART) st = ""
                    if (n > length(s)) n = length(s)
                    out = out blank(n)
                    s = substr(s, n + 1)
                } else if (st == "raw") {
                    p = index(s, rawend)
                    if (p == 0) { out = out blank(length(s)); break }
                    n = p + length(rawend) - 1
                    out = out blank(n)
                    s = substr(s, n + 1)
                    st = ""
                } else {
                    if (!match(s, tok)) { out = out s; break }
                    out = out substr(s, 1, RSTART - 1)
                    t = substr(s, RSTART, RLENGTH)
                    s = substr(s, RSTART + RLENGTH)
                    if (t == "//") { out = out blank(2 + length(s)); break }
                    if (t == "/*") { out = out "  "; st = "block"; bdepth = 1; continue }
                    if (t == q) {
                        # A character literal, else a lifetime or a label.
                        c = substr(s, 1, 1)
                        if (c == "\\") {
                            p = index(substr(s, 3), q)
                            if (p) { out = out blank(p + 3); s = substr(s, p + 3); continue }
                        } else if (c != "") {
                            # The UTF-8 length of the character, by its lead byte.
                            w = c < "\300" ? 1 : c < "\340" ? 2 : c < "\360" ? 3 : 4
                            if (substr(s, w + 1, 1) == q) { out = out blank(w + 2); s = substr(s, w + 2); continue }
                        }
                        out = out q
                        continue
                    }
                    out = out blank(length(t))
                    if (t ~ /r/) {
                        rawend = t
                        gsub(/[^#]/, "", rawend)
                        rawend = "\"" rawend
                        st = "raw"
                    } else {
                        st = "str"
                    }
                }
            }
            return out
        }
        function line_of(pos,    t) {
            t = substr(J, 1, pos - 1)
            return gsub(/\n/, "", t) + 1
        }
        # Marks the line of each match of re in the code; a match starts
        # with the character before the name unless the name begins the file.
        function scan(re,    rest, off, pos) {
            rest = J
            off = 0
            while (match(rest, re)) {
                pos = off + RSTART
                if (substr(rest, RSTART, 1) !~ /[tu]/) pos++
                H[line_of(pos)] = 1
                off += RSTART + RLENGTH - 1
                rest = substr(rest, RSTART + RLENGTH)
            }
        }
        # The name before position i, whitespace skipped.
        function word_before(i,    j, k) {
            j = i - 1
            while (j >= 1 && substr(J, j, 1) ~ ws) j--
            k = j
            while (k >= 1 && substr(J, k, 1) ~ /[A-Za-z0-9_]/) k--
            return substr(J, k + 1, j - k)
        }
        # Whether the lint path at pos is directly under allow, expect or
        # warn inside an attribute, #[...] or #![...].
        function lowered(pos,    i, c, pd, bd, found, kw, j) {
            pd = 0; bd = 0; found = 0; kw = ""
            for (i = pos - 1; i >= 1; i--) {
                c = substr(J, i, 1)
                if (c == ")") pd++
                else if (c == "(") {
                    if (pd > 0) pd--
                    else if (!found) { found = 1; kw = word_before(i) }
                } else if (c == "]") bd++
                else if (c == "[") {
                    if (bd > 0) { bd--; continue }
                    j = i - 1
                    while (j >= 1 && substr(J, j, 1) ~ ws) j--
                    if (j >= 1 && substr(J, j, 1) == "!") j--
                    while (j >= 1 && substr(J, j, 1) ~ ws) j--
                    return j >= 1 && substr(J, j, 1) == "#" && kw ~ /^(allow|expect|warn)$/
                } else if (c == ";" || c == "{" || c == "}") return 0
            }
            return 0
        }
        # What the innermost delimiters around pos belong to, of a macro call
        # (`name!(...)`, `[...]` or `{...}`, a keyword before the `!` aside)
        # or a `macro_rules! name {...}` definition (1), and an attribute (2);
        # 0 for neither.
        function in_tree(pos,    i, c, depth, j, w) {
            depth = 0
            for (i = pos - 1; i >= 1; i--) {
                c = substr(J, i, 1)
                if (c == ")" || c == "]" || c == "}") depth++
                else if (c == "(" || c == "[" || c == "{") {
                    if (depth > 0) { depth--; continue }
                    j = i - 1
                    while (j >= 1 && substr(J, j, 1) ~ ws) j--
                    if (c == "[" && j >= 1 && substr(J, j, 1) == "#") return 2
                    if (j >= 1 && substr(J, j, 1) == "!") {
                        j--
                        while (j >= 1 && substr(J, j, 1) ~ ws) j--
                        if (c == "[" && j >= 1 && substr(J, j, 1) == "#") return 2
                        w = word_before(j + 1)
                        if (w != "" && w !~ /^(if|while|match|return|in|else|break|let|for|loop|move|mut|ref|as|yield)$/) return 1
                    } else if (j >= 1 && substr(J, j, 1) ~ /[A-Za-z0-9_]/) {
                        # `macro_rules! name {`: the name, then `!` and `macro_rules`.
                        while (j >= 1 && substr(J, j, 1) ~ /[A-Za-z0-9_]/) j--
                        while (j >= 1 && substr(J, j, 1) ~ ws) j--
                        if (j >= 1 && substr(J, j, 1) == "!" && word_before(j) == "macro_rules") return 1
                    }
                }
            }
            return 0
        }
        # Whether the name at pos ends a `clippy::` lint path.
        function lint_path(pos,    j) {
            j = pos - 1
            while (j >= 1 && substr(J, j, 1) ~ ws) j--
            if (j < 2 || substr(J, j - 1, 2) != "::") return 0
            return word_before(j - 1) == "clippy"
        }
        function report(k,    who) {
            who = owner_of(L[k])
            if (who == "") who = first
            print "[" who "] " fname ":" k ": " L[k]
        }
        function flush(    k, re, rest, off, pos, end, t) {
            first = ""
            for (k = 1; k <= n && first == ""; k++) first = owner_of(L[k])
            if (first == "") first = "?"
            split("", H)
            J = C[1]
            for (k = 2; k <= n; k++) J = J "\n" C[k]
            for (k = 1; k <= n; k++) if (L[k] ~ /STUB\(/) H[k] = 1
            scan("(^|[^A-Za-z0-9_])(todo|unimplemented)" ws "*!" ws "*[[({]")
            scan("(^|[^A-Za-z0-9_])(todo|unimplemented)" ws "+as" ws "+[A-Za-z_]")
            # Either name as a word of its own in a macro call or a
            # macro_rules! definition, or in an attribute but as a lint path
            # (in a macro call, `clippy::todo` may name the macro of a module).
            rest = J
            off = 0
            while (match(rest, /(^|[^A-Za-z0-9_])(todo|unimplemented)([^A-Za-z0-9_]|$)/)) {
                pos = off + RSTART
                if (substr(rest, RSTART, 1) !~ /[tu]/) pos++
                end = substr(J, pos, 1) == "t" ? 4 : 13
                t = in_tree(pos)
                if (t == 1 || (t == 2 && !lint_path(pos))) H[line_of(pos)] = 1
                off = pos + end - 1
                rest = substr(J, off + 1)
            }
            re = "clippy" ws "*::" ws "*(todo|unimplemented|restriction)"
            rest = J
            off = 0
            while (match(rest, re)) {
                pos = off + RSTART
                end = RSTART + RLENGTH
                if (substr(rest, end, 1) !~ /[A-Za-z0-9_]/ && (pos == 1 || substr(J, pos - 1, 1) !~ /[A-Za-z0-9_]/) && lowered(pos))
                    H[line_of(pos)] = 1
                off += end - 1
                rest = substr(rest, end)
            }
            for (k = 1; k <= n; k++) if (k in H) report(k)
        }
        FNR == 1 {
            if (fname != "") flush()
            fname = FILENAME; n = 0; st = ""; bdepth = 0
            split("", L); split("", C)
        }
        { n++; L[n] = $0; C[n] = lex($0) }
        END { if (fname != "") flush() }')
if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    summary=$(printf '%s\n' "$hits" | sed -n 's/^\[\([^]]*\)\].*/\1/p' | LC_ALL=C sort | uniq -c \
        | awk '{ printf "%s%s %s", sep, $2, $1; sep = ", " }')
    echo "stub-check: stubs remain, hits per package: $summary" >&2
    exit 1
fi
echo "stub-check: no stubs"
