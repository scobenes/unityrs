#!/bin/sh
# No unmanageable files: every Rust and Python file git tracks (each `.rs`
# and `.py` file, and each file with no extension whose first line is a
# `#!` line naming python) stays at or under 800 lines, split by
# responsibility (a cohesive module per concern), unless
# scripts/file-size-allowlist.txt names it. A line is what `wc -l` counts,
# plus a last line with no newline after it (awk's record count).
#
# The allowlist holds one entry per line, `PATH [LIMIT] [blob=SHA]  # reason`:
# the path relative to the repository's root; the most lines the file may
# grow to; the git blob id it must keep (`git hash-object PATH`), for a file
# listed because it is a copy that must not change; and why it may run past
# 800 lines. The check fails on an entry with no reason, an entry listed
# twice, a path that is not a file it checks, a file at or under 800 lines
# (it no longer needs listing), a file over its limit, and a file whose blob
# is no longer the one pinned. A line starting with `#` is a comment.
#
# Each file over 800 lines and not listed is printed as `LINES PATH`, the
# longest first, then each failing entry as `ALLOWLIST:LINE: PATH: why`; the
# check exits 1 if there is any. A missing allowlist is an empty one.
#
# A name git C-quotes even with core.quotePath=false (one holding `"`, `\`
# or a control character: a tab, a newline) cannot be read back from
# `git ls-files`, so a tracked `.rs` or `.py` file, or a file with no
# extension, under such a name fails the check too, printed as
# `QUOTED: git quotes this name; rename it`.
#
#     sh scripts/file-size-check.sh [ROOT]   # prints each failure; exits 1 if any
#
# It lists the files with `git ls-files`, so in a worktree under `.wt/`
# inside the container it needs CLAUDE.md's GIT_DIR and GIT_WORK_TREE.
set -eu
root=${1:-"$(dirname "$0")/.."}
cd "$root"
limit=800
allowlist=scripts/file-size-allowlist.txt
list=$allowlist
[ -f "$list" ] || list=/dev/null
tab=$(printf '\t')

# `LINES<TAB>PATH` for each file the check covers; `Q<TAB>QUOTED` for one
# under a name git quotes.
counts=$(git -c core.quotePath=false ls-files | while IFS= read -r f; do
    case $f in
    \"*)
        case $f in
        *.rs\" | *.py\") ;;
        *) case ${f##*/} in *.*) continue ;; esac ;;
        esac
        printf 'Q\t%s\n' "$f"
        continue
        ;;
    esac
    [ -n "$f" ] && [ -f "$f" ] || continue
    case $f in
    *.rs | *.py) ;;
    *)
        case ${f##*/} in *.*) continue ;; esac
        head -n 1 < "$f" | grep -q '^#!.*python' || continue
        ;;
    esac
    printf '%s\t%s\n' "$(awk 'END { print NR }' < "$f")" "$f"
done)

# `O<TAB>LINES<TAB>PATH` for a file over the limit and not listed;
# `P<TAB>LINE<TAB>PATH: why` for a failing entry; `B<TAB>LINE<TAB>SHA<TAB>PATH`
# for a pinned blob to check.
verdicts=$(printf '%s\n' "$counts" | awk -F '\t' -v limit="$limit" -v list="$list" '
    function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
    NF == 2 && $1 != "Q" { lines[$2] = $1 + 0 }
    END {
        at = 0
        while ((getline raw < list) > 0) {
            at++
            if (raw ~ /^[ \t]*(#|$)/) continue
            hash = index(raw, "#")
            body = trim(hash ? substr(raw, 1, hash - 1) : raw)
            reason = hash ? trim(substr(raw, hash + 1)) : ""
            n = split(body, word, /[ \t]+/)
            blob = ""; cap = ""
            if (n > 1 && word[n] ~ /^blob=/) { blob = substr(word[n], 6); n-- }
            if (n > 1 && word[n] ~ /^[0-9]+$/) { cap = word[n] + 0; n-- }
            path = word[1]
            for (i = 2; i <= n; i++) path = path " " word[i]
            if (path in seen) {
                printf "P\t%d\t%s: listed twice (first at line %d)\n", at, path, seen[path]
                continue
            }
            seen[path] = at
            if (reason == "") printf "P\t%d\t%s: no reason (end the line with `# reason`)\n", at, path
            if (blob != "" && blob !~ /^[0-9a-f]+$/ || blob != "" && length(blob) != 40) {
                printf "P\t%d\t%s: blob=%s is not a 40-digit blob id\n", at, path, blob
                blob = ""
            }
            if (!(path in lines)) {
                printf "P\t%d\t%s: not a tracked Rust or Python file\n", at, path
                continue
            }
            if (lines[path] <= limit) printf "P\t%d\t%s: %d lines, so it no longer needs listing\n", at, path, lines[path]
            else if (cap != "" && lines[path] > cap) printf "P\t%d\t%s: %d lines, over its limit of %d\n", at, path, lines[path], cap
            if (blob != "") printf "B\t%d\t%s\t%s\n", at, blob, path
        }
        for (path in lines) if (lines[path] > limit && !(path in seen)) printf "O\t%d\t%s\n", lines[path], path
    }')

over=$(printf '%s\n' "$verdicts" | awk -F '\t' '$1 == "O" { print $2 " " $3 }' | LC_ALL=C sort -k1,1nr -k2,2)
quoted=$(printf '%s\n' "$counts" | awk -F '\t' '$1 == "Q" { print $2 ": git quotes this name; rename it" }' |
    LC_ALL=C sort)
problems=$(printf '%s\n' "$verdicts" | awk -F '\t' '$1 == "P" { print $2 "\t" $3 }')
changed=$(printf '%s\n' "$verdicts" | awk -F '\t' '$1 == "B" { print $2 "\t" $3 "\t" $4 }' |
    while IFS=$tab read -r at sha path; do
        now=$(git hash-object -- "$path")
        [ "$now" = "$sha" ] ||
            printf '%s\t%s: blob %s, not the pinned %s: split it, or list it again with its reason checked\n' \
                "$at" "$path" "$now" "$sha"
    done)
problems=$(printf '%s\n%s\n' "$problems" "$changed" | awk -F '\t' -v list="$allowlist" \
    'NF { print $1 "\t" list ":" $1 ": " $2 }' | LC_ALL=C sort -t "$tab" -k1,1n | cut -f 2-)

[ -z "$over" ] || printf '%s\n' "$over"
[ -z "$problems" ] || printf '%s\n' "$problems"
[ -z "$quoted" ] || printf '%s\n' "$quoted"
if [ -n "$over" ] || [ -n "$problems" ] || [ -n "$quoted" ]; then
    if [ -n "$over" ]; then
        count=$(printf '%s\n' "$over" | wc -l | tr -d ' ')
        echo "file-size-check: $count files over $limit lines; split each by responsibility" \
            "(a generated table or a fixture generator goes in $allowlist instead)" >&2
    fi
    if [ -n "$problems" ]; then
        count=$(printf '%s\n' "$problems" | wc -l | tr -d ' ')
        echo "file-size-check: $count entries of $allowlist fail; fix or remove each" >&2
    fi
    if [ -n "$quoted" ]; then
        count=$(printf '%s\n' "$quoted" | wc -l | tr -d ' ')
        echo "file-size-check: git quotes $count tracked names, so the check cannot read them;" \
            "rename each without '\"', '\\' or control characters" >&2
    fi
    exit 1
fi
echo "file-size-check: no tracked Rust or Python file is over $limit lines outside" \
    "$allowlist, and every entry there holds"
