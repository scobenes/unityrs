# Package B requests

Changes package B needs in files it does not own, and places where the plan
and UnityPy 1.25.2 disagree (UnityPy wins; the plan should be corrected).
Numbered for the integration step.

## B-1. CI: check the walk fixtures against the oracle (`.github/workflows/ci.yml`, E or the CI owner)

**What.** Add one step to the oracle job, after the stack is installed:

```yaml
      - name: Package B's walker fixtures are current
        run: python tools/fixtures/walk.py --check
```

**Why.** `tools/fixtures/walk.py` records `crates/unityrs-core/tests/fixtures/walk/`
(`cases.json`, `value_cases.json`, `write_matrix.json`, `py_attrs.json`) from
UnityPy 1.25.2 with UnityPyBoost. `cargo test` replays them without Python, so
nothing notices a fixture that has drifted from the script, or a script change
whose fixtures were never regenerated. `--check` regenerates everything in
memory and exits 1 naming each stale file (0.7 s in the dev container).

## B-2. Plan 3.10 R14 / 7.3: "non-list `ref_types`" has no `RefTypes` value (`docs/plans/core.md`)

**What.** In the 7.3 fixture list, replace "no or non-list `ref_types`" with
"no `ref_types`", and note under R14 that `RefTypes::Absent` covers both.

**Why.** UnityPyBoost treats a missing attribute and a non-list the same way
(`TypeTreeHelper.cpp:592-596`, `!ref_types || !PyList_Check(ref_types)`, "No
SerializedFile.ref_types"). `UnityPy/files/SerializedFile.py` either assigns a
list (`:306-308`, format 20 and later) or leaves the attribute unset (`:215` is
only an annotation), so a non-list `ref_types` cannot come from a file.
`RefTypes` (`walk/mod.rs`, frozen) has `NoFile`, `Absent` and `List`, and no
fixture can feed a non-list through it. The fixtures record `ref_types=None`
(no file), `"absent"`, empty lists and lists.

## B-3. Plan 7.3: "the empty-class skip with and without `ref_types`" (`docs/plans/core.md`)

**What.** Reword it as "the empty-class skip with empty and non-empty
`ref_types`, and an empty class with no file or no `ref_types` (an error in
both readers)".

**Why.** R14's own order (and UnityPyBoost's, `TypeTreeHelper.cpp:585-600`)
checks the file and `ref_types` before the empty class, so without `ref_types`
an empty class is a `ref_type` error, not a skip. The pure-Python reader raises
too: `AttributeError` for an absent attribute, the `ValueError` of
`TypeTreeHelper.py:99-100` for no file, both `ref_type`. The recorded cases
`r14_empty_class_absent` and `r14_empty_class_no_file` show both readers
raising.

## B-4. W2: dunder attribute names are not modelled (`PARITY.md` or plan 3.11)

**What.** Record this divergence as a known Tier C gap. A class written from a
non-dict value, one of whose children cleans to a dunder name (`__class__`,
`__doc__`, `__len__`, ...), is a `WriteError::TypeMismatch` here.

**Why.** `getattr(value, child._clean_name)` (`TypeTreeHelper.py:447-455`)
finds every Python object's dunder attributes. These include types, method
wrappers and strings, which the child's kind then writes or rejects. The
writer models what `dir()` lists for the types a `Value` becomes, without the
dunders. That table is recorded in `py_attrs.json` and checked by
`w2_python_attributes_match_the_oracle`. Such a child needs a hostile tree, a
value that is not a dict where the tree has a class, and a node name that is a
dunder. UnityPy's results for these names depend on the object (for example,
`__doc__` is a `str` for most types), so refusing them is safer than guessing.

## B-5. Integration: un-ignore A's `STUB(B)` tests

**What.** When `p1-b` and A's branch meet, remove
`#[ignore = "STUB(B): Tree::compile"]` from A's tests that compile trees (plan
7.1, 7.3) and run them.

**Why.** None of them are in this branch (`grep -rn 'STUB(B)'` finds only the
plan), so B cannot un-ignore them here. `Tree::compile` is implemented, and
`tree.rs`'s tests and every walker test use it.

## B-6. Note for the plan (3.11): where the writer charges its scratch

**What.** Plan 3.11 says the writer's output is a `DecodeBuf` on the object's
meter. Add that a class written from a dict that repeats a name keeps a
per-class table from slot to entry position (`MeteredVec<u32>`, one `u32` per
distinct name). That table is charged to the meter of the value being written
(`Parsed::charge().meter()`), because `WriteCtx` carries no meter of its own.

**Why.** The writer used to look each child's name up among the entries:
about n^2/2 comparisons for a class of n children. `tests/walk_memory.rs`, with
a class of 2^16 + 1 children, took 117 s in a debug build. A cursor that walks
the entries in step with the children now makes a dict read through the same
tree linear. The table keeps names repeated later in the class linear too
(otherwise each repeat scans the entries). Apart from its output, the table is
the only memory a successful write allocates. `tests/walk_memory.rs` checks
that it is charged before it is allocated. If the
plan wants the output's meter instead, `WriteCtx` would need a
`meter: &Arc<Meter>` field, or `DecodeBuf` a public `meter()` accessor (both
frozen files).
