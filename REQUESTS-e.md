# Package E requests (wave 1)

Changes E needs in files it does not own, and disagreements found between
the plan and UnityPy's oracle. E works around each one in its own files
where it can; nothing below blocks wave 1.

## E1. CI: replay the dump's fixture script, and raise the parity suite's floor

- **File:** `.github/workflows/ci.yml` (scaffold, frozen; E maintains it at
  integration).
- **What:**
  1. In the oracle job (`parity-py`), after the oracle stack is installed,
     add a step running `python tools/fixtures/dump.py --check`.
  2. In "Self-tests (parity tools and the spikes' scripts)", raise
     `tools/parity/tests=156` to `tools/parity/tests=258`.
- **Why:** G4 says `tools/fixtures/*.py --check` reproduces every committed
  fixture. CI runs no fixture script today. `dump.py` records E's three
  fixtures (`crates/unityrs-dump/tests/fixtures/dump/{encode,select,l2_kinds}.json`)
  from `canonical.py`, `unitypy_dump.py`, Python 3.11's `fnmatch` and a real
  UnityPy read, and `--check` fails if any of them drifted. The new
  `tools/parity/tests/test_value_cases.py` adds 8 tests. The suite runs 258
  in the oracle container (`scripts/unittest-suites tools/parity/tests=156`
  reports 258, none skipped), so 258 keeps the floor exact.

## E2. B: a `UInt8`/`char` vector is `Prims(PrimArray::U8)` on both reading paths

- **Files:** `crates/unityrs-core/src/walk/boost.rs`,
  `crates/unityrs-core/src/walk/python.rs` (B).
- **What:** a test on each reading path (UnityPyBoost's and the pure-Python
  one) that a non-empty `vector<UInt8>` and a non-empty `vector<char>`,
  with and without the align flag on the element node, read as
  `Value::Prims(PrimArray::U8(_))`, never as a `List` of integers. On the
  pure-Python path an *aligned* primitive array is a `Value::Tuple` (CANONICAL_DUMP
  section 7), which the encoder writes as `tuple`, never packed. A
  `TypelessData` is `Value::Bytes`.
- **Why:** a `Value` carries no node, so the encoder (`unityrs_dump::encode`)
  chooses `u8s`, `bytes`, a JSON array or a `tuple` from the variant alone
  (plan 3.9 table: "`vector<UInt8>`/`char` is zero-copy `Bytes`"). A `List`
  of `Int`s under a `UInt8` element would be written as a JSON array where
  `canonical.py` writes `{"t":"u8s",...}` (its `_seq`, `U8_TYPES`), a
  format difference on every Font, shader blob and index buffer. Probed in
  the oracle container: UnityPyBoost returns a `list` for all four shapes
  (`UInt8` and `char`, element aligned or not), and `canonical.py` encodes
  each as `u8s` (`{"t":"u8s","hex":"0102fa"}` for `[1, 2, 250]`).
- **Signature:** none new; `PrimArray::U8(Bytes)` exists (`value.rs`).

## E3. Plan and CANONICAL_DUMP: a negative `bytes_read` is `other`, not `check_read`

- **Files:** `docs/plans/core.md` 3.9 and 3.10 (plan; E applies plan fixes at
  integration); `crates/unityrs-core/src/walk/mod.rs` `ParseError::l2_kind`
  (B), optional.
- **What:** the plan says that after a negative `TypelessData` length "the
  pure-Python reader's [`bytes_read`] is [negative]", and CANONICAL_DUMP said
  "`bytes_read` can be negative", which reads as a `check_read` with a
  negative `bytes_read`. The oracle never writes one. `unitypy_dump.py`'s
  `error_kind` matches `Expected to read (\d+) bytes, but only read (\d+)
  bytes`, and `(\d+)` takes no sign, so the message falls through to
  `other`, `bytes_read` `null`, no value. `tools/fixtures/dump.py` records a
  real pure-Python read that does this (`l2_kinds.json`). Per the brief,
  UnityPy's oracle wins:
  1. Done on this branch (E owns the file): CANONICAL_DUMP section 7 and the
     `other` row of section 4.3 now say that such a read is `other`.
  2. Plan 3.10: after "(so `bytes_read` can be negative)" add "and the
     dump reports such a read as `other`, as `unitypy_dump.py`'s
     `error_kind` does".
  3. Optional, B: `ParseError::l2_kind` returns `L2Kind::CheckRead` for every
     `CheckRead`. The dump does not use it there; it classifies with
     `unityrs_dump::input::check_read_kind(bytes_read: i64) -> (L2Kind,
     Option<i64>)`. If B wants the core's kind to agree, it can return
     `L2Kind::Other` when `parsed.bytes_read < 0`.
- **Why:** with a `check_read` there, a dump would hold a record the oracle
  never writes, and G1 would report a difference.

## E4. A: `unityrs-tpk` for `--tpk` (wave 2)

- **Files:** `crates/unityrs-tpk/**` (A); `Cargo.lock` (A, then E).
- **What:** nothing new beyond plan 5: `TpkProvider::open(path: &Path) ->
  Result<TpkProvider, _>` implementing `unityrs_core::provider::TypeTreeProvider`
  with `name() == "tpk"`. Until it lands, `unityrs dump --tpk` exits 2 with
  "the TPK typetree provider (unityrs-tpk) is not built yet"
  (`crates/unityrs-cli/src/main.rs`, marked `STUB(A)`). In wave 2, E adds
  the dependency to `unityrs-cli` and `unityrs-dump` and pushes the
  provider into `DumpOptions::context`'s `ProviderChain`.
- **Why:** G1 dumps stripped objects through the oracle image's TPK. A dump
  without the provider would write `no_typetree` where the oracle reads a
  value. So the CLI refuses `--tpk` instead of silently ignoring it.
