#!/bin/sh
# The fork check (docs/plans/core.md section 2.4): crates/unityrs-rabex is
# rabex 0.2.1 from crates.io plus the patches its ledger lists, and nothing
# else.
#
#   1. The pinned tarball is taken from the cargo registry cache (fetched
#      with `cargo fetch --locked` when it is not there; the spike crate
#      depends on it) and its sha256 checked, as is Cargo.lock's.
#   2. The commit whose subject starts `rabex F0: ` holds the tarball's src/,
#      LICENSE-MIT.md and LICENSE-APACHE.md unchanged.
#   3. Every later commit's change to the crate's src/ is replayed onto the
#      pristine src/ as a patch series (git format-patch --relative, git am),
#      and the result must equal src/ in the working tree byte for byte, so
#      an uncommitted edit fails too. The licence files must still equal the
#      tarball's.
#   4. Each such commit's subject names a ledger patch (`rabex F<n>: `), and
#      each row of UPSTREAM.md's ledger has exactly one commit.
#   5. The crate holds only src/, the licence files, Cargo.toml and
#      UPSTREAM.md.
#
#     sh scripts/fork-check.sh                  # from the repository; CI and the container
#
# In a git worktree inside the dev container, whose .git file names a host
# path, point git at the worktree first:
#     GIT_DIR=/work/.git/worktrees/<name> GIT_WORK_TREE=/work/.wt/<name> sh scripts/fork-check.sh
set -eu

crate=crates/unityrs-rabex
name=rabex-0.2.1
sha=d5f8c39f8069c0dc6d01d5435bd21153fda86b2e01af5fea9b5ddfe0716c3d58

fail() {
    echo "fork-check: $*" >&2
    exit 1
}

root=$(git rev-parse --show-toplevel) || fail "not in a git checkout"
cd "$root"
[ -d "$crate/src" ] || fail "$crate/src is missing"

# 1. The tarball.
find_tarball() {
    for f in "${CARGO_HOME:-$HOME/.cargo}"/registry/cache/*/"$name.crate"; do
        [ -f "$f" ] && { echo "$f"; return; }
    done
}
tarball=$(find_tarball)
if [ -z "$tarball" ]; then
    cargo fetch --locked >&2
    tarball=$(find_tarball)
fi
[ -n "$tarball" ] || fail "$name.crate is not in the cargo registry cache"
got=$(sha256sum "$tarball" | cut -d' ' -f1)
[ "$got" = "$sha" ] || fail "$tarball has sha256 $got, not $sha"
awk -v want="$sha" '
    /^\[\[package\]\]/ { name = ""; version = "" }
    /^name = "rabex"$/ { name = 1 }
    /^version = "0.2.1"$/ { version = 1 }
    name && version && /^checksum = / { gsub(/"/, "", $3); found = $3 }
    END { if (found != want) { print "Cargo.lock: rabex 0.2.1 checksum " found ", not " want > "/dev/stderr"; exit 1 } }
' Cargo.lock || fail "Cargo.lock does not pin the tarball"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
tar xzf "$tarball" -C "$tmp"
pristine="$tmp/$name"

# 2. F0 is the pristine import.
log=$(git log --format='%H %s' -- "$crate")
f0=$(printf '%s\n' "$log" | awk '$2 == "rabex" && $3 == "F0:" { print $1 }')
[ -n "$f0" ] || fail "no commit with the subject 'rabex F0: ...' touches $crate"
[ "$(printf '%s\n' "$f0" | wc -l)" -eq 1 ] || fail "more than one 'rabex F0:' commit"
mkdir "$tmp/f0"
git archive "$f0" "$crate" | tar x -C "$tmp/f0"
diff -r "$pristine/src" "$tmp/f0/$crate/src" >&2 || fail "F0's src/ is not the tarball's"
for f in LICENSE-MIT.md LICENSE-APACHE.md; do
    cmp -s "$pristine/$f" "$tmp/f0/$crate/$f" || fail "F0's $f is not the tarball's"
    cmp -s "$pristine/$f" "$crate/$f" || fail "$crate/$f is not the tarball's"
done

# 3. Replay the patches onto the pristine src/.
mkdir "$tmp/patches" "$tmp/replay"
git format-patch --quiet --relative="$crate" -o "$tmp/patches" "$f0..HEAD" -- "$crate/src"
cp -R "$pristine/src" "$tmp/replay/src"
# The replay is a repository of its own: GIT_DIR and GIT_WORK_TREE, which
# name the checkout above (the container recipe sets them), must not reach
# it, or `init` would rewrite that repository's config and `commit` would
# commit there.
replay() {
    env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE -u GIT_COMMON_DIR \
        git -C "$tmp/replay" -c user.name=fork-check -c user.email=fork-check@invalid "$@"
}
replay init --quiet
[ "$(replay rev-parse --absolute-git-dir)" = "$tmp/replay/.git" ] || fail "the replay repository is not its own"
replay add -A
replay commit --quiet -m pristine
patches=$(find "$tmp/patches" -name '*.patch' | LC_ALL=C sort)
if [ -n "$patches" ]; then
    # shellcheck disable=SC2086 # one argument per patch file
    replay am --quiet --whitespace=nowarn $patches >&2 || fail "the patch series does not apply to the tarball's src/"
fi
diff -r "$tmp/replay/src" "$crate/src" >&2 || fail "$crate/src is not the tarball's plus the ledger's patches"

# 4. Commits and ledger rows match one to one.
ledger=$(sed -n 's/^| \(F[0-9][0-9]*\) |.*/\1/p' "$crate/UPSTREAM.md")
[ -n "$ledger" ] || fail "$crate/UPSTREAM.md lists no patch"
dups=$(printf '%s\n' "$ledger" | sort | uniq -d)
[ -z "$dups" ] || fail "ledger rows repeat: $dups"
for id in $ledger; do
    n=$(printf '%s\n' "$log" | awk -v id="$id:" '$2 == "rabex" && $3 == id' | wc -l)
    [ "$n" -eq 1 ] || fail "ledger row $id has $n commits with the subject 'rabex $id: ...' (needs one)"
done
for c in $(git rev-list "$f0..HEAD" -- "$crate/src"); do
    subject=$(git log -1 --format=%s "$c")
    id=$(printf '%s\n' "$subject" | sed -n 's/^rabex \(F[0-9][0-9]*\): .*/\1/p')
    [ -n "$id" ] || fail "commit $c changes $crate/src but is no ledger patch: $subject"
    printf '%s\n' "$ledger" | grep -qx "$id" || fail "commit $c names $id, which UPSTREAM.md does not list"
done

# 5. Nothing else in the crate.
extra=$(find "$crate" -mindepth 1 -maxdepth 1 \
    ! -name src ! -name LICENSE-MIT.md ! -name LICENSE-APACHE.md ! -name Cargo.toml ! -name UPSTREAM.md)
[ -z "$extra" ] || fail "unexpected files in $crate: $extra"

echo "fork-check: $crate is $name plus $(printf '%s\n' "$ledger" | wc -l | tr -d ' ') ledger rows ($(printf '%s\n' "$patches" | grep -c . || true) patches to src/)"
