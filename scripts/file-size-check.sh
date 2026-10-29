#!/bin/sh
# No unmanageable files: every Rust and Python file git tracks stays at or
# under 800 lines, split by responsibility (a cohesive module per concern),
# unless scripts/file-size-allowlist.txt names it. The allowlist holds one
# path per line, relative to the repository's root, then `# reason`: a
# generated table, or a fixture generator.
#
# Each file over the limit is printed as `LINES PATH`, the longest first,
# and the check exits 1 if there is any. A missing allowlist is an empty one.
#
#     sh scripts/file-size-check.sh [ROOT]   # prints each file over; exits 1 if any
#
# It lists the files with `git ls-files`, so in a worktree under `.wt/`
# inside the container it needs CLAUDE.md's GIT_DIR and GIT_WORK_TREE.
set -eu
root=${1:-"$(dirname "$0")/.."}
cd "$root"
limit=800
allowlist=scripts/file-size-allowlist.txt
allowed=
if [ -f "$allowlist" ]; then
    allowed=$(sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e 's/^[[:space:]]*//' -e '/^$/d' "$allowlist")
fi
files=$(git -c core.quotePath=false ls-files -- '*.rs' '*.py')
over=$(printf '%s\n' "$files" | while IFS= read -r f; do
    [ -n "$f" ] && [ -f "$f" ] || continue
    n=$(($(wc -l < "$f")))
    [ "$n" -gt "$limit" ] || continue
    if [ -n "$allowed" ] && printf '%s\n' "$allowed" | grep -Fqx -- "$f"; then
        continue
    fi
    printf '%s %s\n' "$n" "$f"
done | LC_ALL=C sort -k1,1nr -k2,2)
if [ -n "$over" ]; then
    printf '%s\n' "$over"
    count=$(printf '%s\n' "$over" | wc -l | tr -d ' ')
    echo "file-size-check: $count files over $limit lines; split each by responsibility" \
        "(a generated table or a fixture generator goes in $allowlist instead)" >&2
    exit 1
fi
echo "file-size-check: no tracked .rs or .py file is over $limit lines outside $allowlist"
