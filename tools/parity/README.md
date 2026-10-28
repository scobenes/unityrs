# Parity tools

These tools compare unityrs with its oracle, UnityPy 1.25.2 with UnityPyBoost, at verification levels L0-L3:
- L0: file detection;
- L1: container and SerializedFile metadata;
- L2: typetree values;
- L3: object re-save.

Both sides write the canonical dump specified in [`docs/CANONICAL_DUMP.md`](../../docs/CANONICAL_DUMP.md), and `compare.py` diffs the two dumps. No Python binding is needed.

| file | role |
|---|---|
| `unitypy_dump.py` | the oracle: dumps a file, a folder or a corpus index with UnityPy |
| `compare.py` | diffs two dumps; summarises by level, class and field path |
| `canonical.py` | the encoding rules both scripts share (standard library only) |
| `tests/` | unittest self-tests |

`unitypy_dump.py` needs the oracle stack, so run it in the `unityrs-dev` container. `compare.py` and `canonical.py` need only Python 3.10 or later.

## Dumping with UnityPy

```sh
# One bundle, a folder (recursive), or UnityPy's samples:
scripts/dev python tools/parity/unitypy_dump.py /corpus/unitypy-samples -o /tmp/dumps/samples.jsonl

# The corpus index: bundles and samples by default.
# --sample N keeps N inputs spread evenly over the sorted list.
scripts/dev python tools/parity/unitypy_dump.py --index /corpus/index.json --kind bundle \
    --sample 20 -j 8 -o /tmp/dumps/corpus20.jsonl.gz --stats /tmp/dumps/corpus20.stats.json
```

**Output.**
- The output path must lie outside the repository; the dumper refuses anything inside it.
- A `.gz` suffix writes gzip. Use it for anything bigger than a handful of bundles: the dump is about 3.8 times the input uncompressed and about 0.4 times gzipped.
- Output is written to `OUT.partial` and renamed when complete.

**Selecting inputs.**
- `--name GLOB` (repeatable) filters on the dump-relative path.
- `--build 4100` and `--kind bundle` filter index entries.
- `--limit N` keeps the first N.

**Other options.**
- `--levels L0,L1,L2` drops levels. L3 needs L2, and L3 is the slowest level, because UnityPy's writer is pure Python.
- `-j N` runs N worker processes. The output is byte-identical to a serial run.
- The container has a 12 GB cap. `-j 8` over the whole corpus peaked at 6.2 GB, page cache included; watch `scripts/dev cat /sys/fs/cgroup/memory.current` on bigger runs.

**Reporting.**
- A summary prints on stderr: files, objects, L2 error kinds, L3 outcomes, and time per phase (load, parse, encode, resave).
- `--stats PATH` also writes per-input timings.
- An index entry whose sha256 no longer matches its file is reported as `index_sha256_mismatch`.

The unityrs CLI will write the same format. To compare like with like, give both producers the same inputs: the same folder, or the same `--name` selection.

## Comparing

```sh
python tools/parity/compare.py /tmp/dumps/unitypy.jsonl.gz /tmp/dumps/unityrs.jsonl.gz --json /tmp/dumps/report.json
```

**Arguments.** The first dump is the reference (UnityPy) and the second the candidate.

**Exit status.** `compare.py` exits 0 when the compared levels agree, 1 when they differ, and 2 when a dump is unreadable or in another format version.

**Report.** It lists:
- matched, differing, missing and extra records;
- differences by level and by class (the typetree root type);
- the top field paths, with list indices folded to `[]` and three concrete examples each (`--examples`, `--max-paths`).

`--levels` narrows the comparison. By default it covers the levels both dumps hold.

**What counts as a difference.**
- Types must match exactly: a tuple is not a list, `true` is not `1`, and an `f32` is not an `f64`.
- **`f32_snan_quieted`** marks float32 pairs that differ only because UnityPy quiets a signalling NaN (spec section 7).
- **`format`** findings are lines whose values agree but whose bytes do not. Either the line is not the canonical serialisation of itself, or it uses another encoding of the same value. Either way a producer broke spec section 2 or 3.
- A missing `end` record, or one whose counts disagree with the dump, is also a `format` finding.

Records are matched by key, not position: `file` by path, `object` by path and `path_id`. Identical lines are matched without being parsed. Comparing the whole corpus dump with itself takes about 5 seconds.

## Self-tests

```sh
scripts/dev python -m unittest discover -s tools/parity/tests -v
```

| module | covers | needs |
|---|---|---|
| `test_canonical.py` | the JSON and value-encoding rules, with a golden line for Rust | stdlib |
| `test_compare.py` | every finding kind on hand-built dumps | stdlib |
| `test_unitypy_boost.py` | synthetic typetrees through UnityPyBoost: float bits, NaN quieting, bool bytes, pairs, ReferencedObject and registries, big-endian, the pure-Python path. It also checks that the spec's worked example is what the oracle writes. | UnityPy |
| `test_dump.py` | the dumper end to end: UnityPy's samples, determinism across runs, `-j` and gzip, L0 detection quirks, index mode, the output guard | UnityPy, samples |

**Samples.** The samples are UnityPy's `tests/samples` at commit `417d595`. They are read from `$UNITYRS_UNITYPY_SAMPLES` when set, else from `/corpus/unitypy-samples`, and their sha256 is checked against pinned values. They are game bundles, so they are never copied into this repository. Without them the sample tests skip.
