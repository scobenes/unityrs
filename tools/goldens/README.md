# Synthetic batch goldens

Run `python3 tools/goldens/generate.py --check` to reproduce and compare every
`tests/specs/v1/*.json` pin without writing. Omit `--check` to regenerate.
Only Python's standard library is needed. No batch binary, corpus, downloaded
data, UnityPy installation, child process, or scratch directory is used.

`oracle_cases.py` projects the repository's pinned synthetic NGUI and Sprite
oracle recordings in `crates/unityrs-testkit/tests/fixtures/`. `texture_case.py`
uses the pinned texture decoder pixels in
`crates/unityrs-texture/tests/fixtures/decode.json`, independently constructing
Pillow-compatible PNG chunks with zlib. `unit_case.py` projects the pinned NGUI
winner decisions. `rule_cases.py` applies hand rules from `docs/BATCH_SPEC.md`
to those synthetic inputs. These four reproduce the original one-off tools
from `target/b7c/`; expectations never come from batch engine output.

`edge_cases.py` reconstructs the two originally inline cases (source-string
truncation and excluded nested/repeated CABs) from their hand rules and existing
oracle pins; the original command logs were unavailable. Its mixed-severity
case combines an export-work budget failure (5) with a missing input (4),
independently expecting exit 5 under sections 9.1 and 9.2.

The Rust harness creates the matching synthetic bundles, checks pinned bytes,
and validates raw accounting, nullability, reported budgets, and resume results
before normalizing schedule-dependent figures for golden comparisons.
