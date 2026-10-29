# Package D requests

Changes package D needs in files it does not own, and places where the plan
and UnityPy 1.25.2 disagree or say less than UnityPy does (UnityPy wins; the
plan text should follow it). Each item says what to change, why, the exact
signature or patch where there is one, and what D does until it lands.

## 1. A class-142 object in the testkit matrix, and its container rows replayed (C9; package C's `bundle.rs`, `files_fixtures.rs`, `files.json`)

- **What.** Add an AssetBundle input to `unityrs_testkit::bundle::matrix`,
  loose and inside a bundle, built with D's `unityrs_testkit::env` helpers;
  replay its container rows in `files_fixtures.rs` instead of asserting the
  matrix has none; re-record with `python tools/fixtures/files.py`. Also drop
  "No input has an AssetBundle object, so none needs the container reader."
  from `matrix`'s doc. The patch:
  ```diff
  --- a/crates/unityrs-testkit/src/bundle.rs
  +++ b/crates/unityrs-testkit/src/bundle.rs
  @@ pub fn matrix() -> Vec<(String, Vec<u8>)> {
       out.push(("CAB-blank-version".into(), file(&blank)));
  +    // An AssetBundle (class 142), whose container the parse reads (C9).
  +    let assetbundle = || {
  +        use crate::env::{Bundle, Obj, row};
  +        crate::env::serialized(
  +            &["archive:/CAB-x/CAB-x"],
  +            &[
  +                Obj::named(1, "n", &[(0, 2), (1, 5)]),
  +                Obj::asset_bundle(
  +                    2,
  +                    Bundle {
  +                        preload: vec![(0, 1), (1, 5)],
  +                        rows: vec![
  +                            row("assets/n.prefab", (0, 2), (0, 1)),
  +                            row("assets/n.prefab", (1, 1), (0, 2)),
  +                        ],
  +                        ..Bundle::default()
  +                    },
  +                ),
  +            ],
  +        )
  +    };
  +    out.push(("CAB-assetbundle".into(), assetbundle()));
  +    out.push((
  +        "assetbundle.bundle".into(),
  +        crate::env::bundle(&[("CAB-assetbundle", assetbundle())]),
  +    ));
  --- a/crates/unityrs-core/tests/files_fixtures.rs
  +++ b/crates/unityrs-core/tests/files_fixtures.rs
   fn serialized_l1(sf: &Arc<SerializedFile>) -> J {
       let h = sf.header();
  -    assert!(
  -        sf.object_infos().iter().all(|o| o.class_id != 142),
  -        "the matrix has no AssetBundle (the container reader is package D's)"
  -    );
  +    let container: Vec<J> = sf
  +        .container()
  +        .unwrap()
  +        .rows()
  +        .iter()
  +        .map(|r| {
  +            json!([
  +                name(r.path.as_bytes()),
  +                r.preload_index,
  +                r.preload_size,
  +                r.file_id,
  +                r.path_id
  +            ])
  +        })
  +        .collect();
  ...
  -        "container": [],
  +        "container": container,
  ```
- **Why.** C9: the matrix had no AssetBundle because the container reader was
  a stub; it is now `container::read_container`, read at parse under
  `FileContext::eager_asset_bundle`.
- **Verified in D's worktree, then reverted** (the files are C's): with the
  patch, `files.py` re-records `files.json` (two new cases, each with the rows
  `["assets/n.prefab",0,2,0,1]` and `["assets/n.prefab",1,1,0,2]` from
  UnityPy's `sf._container.container`), `files.py --check` passes, and
  `cargo test -p unityrs-core --test files_fixtures` passes 6/6, the
  `the_matrix_has_no_l0_l1_differences` G3 check included.
- **Until then.** D's own scenes cover the container against UnityPy
  (`tests/fixtures/d/env.json`: rows, preload skips and raises, the
  object-mode cases).

## 2. `env.py --check` in CI (scaffold, `.github/workflows/ci.yml`)

- **What.** Add `python tools/fixtures/env.py --check` to the step "The
  scaffold's, package B's and package C's oracle fixtures are current" (and
  name package D in it). It builds its inputs with unityrs-testkit through
  cargo, as `files.py` does, so it needs the pinned toolchain (already
  installed by then).
- **Why.** `tests/fixtures/d/env.json` and `pystr.json` are replayed by
  `cargo test`; the check keeps them equal to what UnityPy records.

## 3. Dereference failures as their own error variants (scaffold, `error.rs` and `walk/mod.rs`)

- **What.**
  ```rust
  // error.rs, enum Error
  /// A pointer that could not be followed (the container preload's raise).
  #[error(transparent)]
  Deref(Box<crate::env::DerefError>),
  // walk/mod.rs, enum ParseError
  /// The MonoBehaviour script hook's dereference of `m_Script` failed.
  #[error("m_Script: {0}")]
  Deref(Box<crate::env::DerefError>),
  ```
  (`DerefError::Load(Error)` already nests an `Error`, so the box breaks the
  cycle.)
- **Why.** The container preload propagates `FileIdBelowStart` (`IndexError`)
  and `RawTarget` (`AttributeError`), and the script hook propagates every
  dereference failure but the null pointer (`ContainerHelper.py:43-46`,
  `ObjectReader.py:332-350`); a caller should be able to tell which.
- **Until then.** `env/mod.rs` `deref_error` returns `Error::Malformed { what:
  "PPtr", detail: <the DerefError's message> }` (a load failure as itself),
  and `deref_parse_error` returns `ParseError::Read` of kind `Other` with
  `ReadDetail::Python("IndexError" | "KeyError" | "AttributeError" |
  "FileNotFoundError")`, or the limit or charge it carried.

## 4. `WriteError::NoTypeTree` (scaffold, `walk/write.rs`)

- **What.** `WriteError::NoTypeTree { class_id: i32 }`, returned by
  `ObjectRef::write_typetree` when the object has no tree.
- **Why.** UnityPy raises the same `TypeTreeError` for a write without a tree
  as for a read (`ObjectReader.py:296-307`); the read side has
  `ParseError::NoTypeTree`.
- **Until then.** `write_typetree` returns `WriteError::Malformed("no typetree")`.

## 5. Final sigma's context properties at Unicode 14 (package A, `tables.rs` and `tools/gen/gen_tables.py`)

- **What.** Two generated tables, `CASED` and `CASE_IGNORABLE` (ranges), from
  the oracle's Python 3.11 (Unicode 14.0) `unicodedata`, with
  `pub fn is_cased(c: char) -> bool` and `pub fn is_case_ignorable(c: char) -> bool`.
- **Why.** CPython's `str.lower` maps U+03A3 to final sigma by the
  `Final_Sigma` context, which reads those two properties at its own Unicode
  version. `pystr::py_lower` decides it with Rust's `to_lowercase` over the
  run (the pinned toolchain's Unicode version), which agrees with Python on
  every case `pystr.json` records (29 contexts, and every BMP scalar on its
  own), but would differ for a context character whose properties changed
  after Unicode 14.
- **Until then.** As above; the difference needs a character assigned after
  Unicode 14 next to a capital sigma in a name.

## 6. Plan text (docs/plans/core.md 3.13, and PARITY.md for E)

Found while recording UnityPy on the scenes (`tools/fixtures/env.py`):

1. **A file's `name` is a basename in UnityPy.** `File.__init__` keeps
   `os.path.basename(name)` (`files/File.py:38`): a loose file loaded as
   `a/CAB-A` is named `CAB-A`, an entry `dir/CAB-D` is `CAB-D`, a resolver
   load keyed `<env.path>/cab-loose` is `cab-loose`. The core's
   `SerializedFile::name` (C's) keeps the whole name, which `env.files` keys
   need; a dump or binding that prints UnityPy's `name` takes the part after
   the last `/` (`env_fixtures.rs` compares that way). Worth a sentence in 3.13
   and in the dump's spec.
2. **A resource fallback's file has `parent=True`.** `load_dependencies`
   calls `load_file(name, True)` (`SerializedFile.py:339-349`), whose second
   positional parameter is `parent`, so that file's container is the bool
   `True`: a pointer with `m_FileID > 0` from one of its objects raises
   `AttributeError` (`'bool' object has no attribute 'files'`) in UnityPy.
   The core treats it as a top-level file (the Environment is its
   container), as it does a caller's load. A `PARITY.md` divergence (E);
   none of the scenes dereferences from such a file.
3. **`find_file` only finds an exact file name on a case-sensitive file
   system.** Its walk keys `local_files_simple` by a lower-cased path, which
   the open then misses unless that path exists, and `find_sensitive_path`
   never matches a name without a directory (its first part is the empty
   head). The core's `CabResolver` is the caller's: nothing to change; the
   scenes name resolver files in lower case.
4. **The probes the plan cites, confirmed:** the loose-file case resolves
   (rule 4), the AssetBundle's object-mode cases record as the plan says (two
   fields that clean to one key: the last wins; no `m_MainAsset`: the
   `UnknownObject` fallback keeps the rows; no `m_Container`: the file fails at
   parse), and the resolver-load bookkeeping matches 3.13 step for step
   (`env.json`'s `resolver_loads`).
