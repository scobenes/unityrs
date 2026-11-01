#!/bin/sh
# Gate G6 (docs/plans/core.md section 6): no stub is left in any crate, and
# no call of `todo!` or `unimplemented!` is written or assembled in them.
#
# The rule, in one sentence: outside comments, strings and character
# literals, the words `todo` and `unimplemented`, raw (`r#todo`) or not,
# appear in no Rust file but as a `clippy::` lint name directly under
# `deny(` or `forbid(` in an inner attribute (`#![...]`) at its file's top
# level.
#
# In full, the scan reads every file under ROOT whose name ends in `.rs`
# (symlinks followed; ROOT's own `target/` and hidden entries, such as
# `.git`, aside), and a line is a hit when it holds:
#   - the marker `STUB(`, in code or a comment (`#[ignore = "STUB(B): ..."]`
#     marks a test that waits on another package's stub, plan section 7.1);
#   - outside comments, strings and character literals, the word `todo` or
#     `unimplemented`, or `restriction` after `::` (the lint group that
#     holds both), raw or not, other than as the lint path `clippy::todo`,
#     `clippy::unimplemented` or `clippy::restriction` that a `,` or `)`
#     ends, directly under `deny(` or `forbid(`, with only `cfg_attr(`s
#     between that and the `#![` of an attribute no other bracket encloses
#     (the core's and the tpk crate's `deny` lists).
# Why that is enough: a call of either macro names it where the call is
# written, where a `macro_rules!` body holds the name (`todo $bang`), in
# the tokens a macro call is handed (`call!(todo)` over `$m!()`, or
# `call!(#[deny(clippy::todo)])` over a pattern that makes a path of it),
# or in a `use` that renames or re-exports it (`use core::todo as later;`,
# `mod clippy { pub use core::todo; }`), and a `macro_rules!` body cannot
# build a name from pieces. The scan reads each of these as the word,
# however the macro around it is named or called (a raw `r#loop!` too).
# No macro is handed an inner attribute at a file's top level (an outer
# one is part of the item a derive or an attribute macro is handed), and a
# lint denied is the opposite of a stub. So a variable named `todo` is a
# hit too (the crates name none), as is a lint path in an outer attribute,
# in a macro call, a `macro_rules!` body, a module or a function, or under
# `allow(`, `expect(` or `warn(`.
#
# Each file is lexed as Rust is first: comments (nested block comments
# included), strings (raw, byte and C strings included) and character
# literals are blanked, so they may name the macros and the lints freely,
# and a comment between two tokens is whitespace. The workspace's lints
# (Cargo.toml `[workspace.lints.clippy]`, which every crate inherits) forbid
# both macros as well, so no attribute can allow them again: `cargo clippy`
# fails on a call written in any code it lints, an aliased one included. It
# lints a call a macro_rules! body writes or assembles as the outer macro's,
# so it misses that one, and this scan reads it. Not read: comments (a
# doctest's code among them, which is a test and never part of a crate),
# what a procedural macro (the crates use thiserror's derive) or a
# dependency's own macro expands to, and a file a crate compiles that the
# scan does not list (one `#[path]` or `include!` names by another name,
# outside ROOT, or in ROOT's `target/` or hidden entries; clippy lints a
# call written in one).
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
hits=$(find -L . \( -path ./target -o -path './.?*' \) -prune -o -type f -name '*.rs' -print0 \
    | LC_ALL=C sort -z \
    | LC_ALL=C xargs -0 -r awk '
        BEGIN {
            q = sprintf("%c", 39)
            # The first token in code that starts a comment, a string, a raw
            # string or a character literal.
            tok = "//|/\\*|[bc]?r#*\"|[bc]?\"|" q
            ws = "[ \t\r\n]"
            id = "[A-Za-z0-9_]"
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
        # The position of the last character at or before i that is no
        # whitespace (0 if none is).
        function back(i) {
            while (i >= 1 && substr(J, i, 1) ~ ws) i--
            return i
        }
        # The identifier that ends at or before i, whitespace skipped, with
        # its `r#` if it is a raw one ("" if none ends there); IDAT is where
        # it starts.
        function ident_at(i,    k) {
            i = back(i)
            k = i
            while (k >= 1 && substr(J, k, 1) ~ id) k--
            if (k < i && k >= 2 && substr(J, k - 1, 2) == "r#" && (k == 2 || substr(J, k - 2, 1) !~ id)) k -= 2
            IDAT = k + 1
            return substr(J, k + 1, i - k)
        }
        # Whether the word from pos to end - 1 (no raw one) ends the lint
        # path `clippy::word` directly under `deny(` or `forbid(`, with only
        # `cfg_attr(`s between that and an inner attribute at the top level.
        function excused(pos, end,    i, c, k, w, depth, stage) {
            i = end
            while (substr(J, i, 1) ~ ws) i++
            c = substr(J, i, 1)
            if (c != "," && c != ")") return 0
            i = back(pos - 1)
            if (i < 2 || substr(J, i - 1, 2) != "::") return 0
            if (ident_at(i - 2) != "clippy") return 0
            c = substr(J, back(IDAT - 1), 1)
            if (c != "(" && c != ",") return 0
            # Outward from the path: stage 0 wants `deny(` or `forbid(`,
            # stage 1 a `cfg_attr(` or the `[` of `#![`, stage 2 no bracket
            # at all.
            depth = 0
            stage = 0
            for (i = IDAT - 1; i >= 1; i--) {
                c = substr(J, i, 1)
                if (c == ")" || c == "]" || c == "}") depth++
                else if (c == "(" || c == "[" || c == "{") {
                    if (depth > 0) { depth--; continue }
                    if (stage == 2 || c == "{") return 0
                    if (c == "(") {
                        w = ident_at(i - 1)
                        c = substr(J, back(IDAT - 1), 1)
                        if (c != "[" && c != ",") return 0
                        if (stage == 0 && (w == "deny" || w == "forbid")) stage = 1
                        else if (stage != 1 || w != "cfg_attr") return 0
                        i = IDAT
                    } else {
                        if (stage != 1) return 0
                        k = back(i - 1)
                        if (k < 1 || substr(J, k, 1) != "!") return 0
                        k = back(k - 1)
                        if (k < 1 || substr(J, k, 1) != "#") return 0
                        stage = 2
                        i = k
                    }
                }
            }
            return stage == 2
        }
        function report(k,    who) {
            who = owner_of(L[k])
            if (who == "") who = first
            print "[" who "] " fname ":" k ": " L[k]
        }
        function flush(    k, rest, off, pos, end, w, raw, b) {
            first = ""
            for (k = 1; k <= n && first == ""; k++) first = owner_of(L[k])
            if (first == "") first = "?"
            split("", H)
            J = C[1]
            for (k = 2; k <= n; k++) J = J "\n" C[k]
            for (k = 1; k <= n; k++) if (L[k] ~ /STUB\(/) H[k] = 1
            rest = J
            off = 0
            while (match(rest, /(^|[^A-Za-z0-9_])(todo|unimplemented|restriction)([^A-Za-z0-9_]|$)/)) {
                pos = off + RSTART
                if (substr(rest, RSTART, 1) !~ /[tur]/) pos++
                w = substr(J, pos, 1)
                w = w == "t" ? "todo" : w == "u" ? "unimplemented" : "restriction"
                end = pos + length(w)
                raw = pos >= 3 && substr(J, pos - 2, 2) == "r#" && (pos == 3 || substr(J, pos - 3, 1) !~ id)
                b = back(raw ? pos - 3 : pos - 1)
                if (w != "restriction" || (b >= 2 && substr(J, b - 1, 2) == "::"))
                    if (raw || !excused(pos, end)) H[line_of(pos)] = 1
                off = end - 1
                rest = substr(J, end)
            }
            for (k = 1; k <= n; k++) if (k in H) report(k)
        }
        FNR == 1 {
            if (fname != "") flush()
            fname = FILENAME; n = 0; st = ""; bdepth = 0
            sub(/^\.\//, "", fname)
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
