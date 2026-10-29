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
| `fetch_unitypy_samples.py` | fetches UnityPy's test samples at the pinned commit, checked against `unitypy_samples.json` |
| `requirements-oracle.txt` | the oracle stack as exact, hash-pinned wheels, for CI and any machine without the oracle image |
| `oracle_env.py` | checks an interpreter's packages against `requirements-oracle.txt`: each pin at its version, nothing else (`--exact`), or the same file hashes as another site-packages (`--records`) |
| `tests/` | unittest self-tests |

`unitypy_dump.py` needs the oracle stack, so run it in the `unityrs-dev` container, or install the stack from `requirements-oracle.txt` (below). It refuses to run on any other stack, because a dump from another stack would charge UnityPy's differences to unityrs:
- without UnityPyBoost, or with any UnityPy other than 1.25.2;
- with a UnityPyBoost binary other than the pinned wheel's (its sha256 is `ORACLE_BOOST_SHA256` in `canonical.py`);
- when a package `requirements-oracle.txt` pins is missing or at another version. numpy is one: UnityPy is not built on it, but uses it when it is there, and renders sprites differently without it.

The dump's header records the stack it ran on: UnityPyBoost's sha256 and every pinned package's version. `compare.py`, `canonical.py`, `oracle_env.py` and `fetch_unitypy_samples.py` need only Python 3.10 or later.

## Dumping with UnityPy

```sh
# One bundle, or a folder (recursive):
scripts/dev python tools/parity/unitypy_dump.py PATH -o /tmp/dumps/out.jsonl

# The corpus index: bundles and UnityPy's samples by default.
scripts/dev python tools/parity/unitypy_dump.py --index /corpus/index.json --kind unitypy-sample \
    -o /tmp/dumps/samples.jsonl
# --sample N keeps N inputs spread evenly over the sorted list.
scripts/dev python tools/parity/unitypy_dump.py --index /corpus/index.json --kind bundle \
    --sample 20 -j 8 -o /tmp/dumps/corpus20.jsonl.gz --stats /tmp/dumps/corpus20.stats.json
```

**Output.**
- The output path and the `--stats` path must lie outside the repository; the dumper refuses anything inside it. Both name corpus inputs.
- A `.gz` suffix writes gzip. Use it for anything bigger than a handful of bundles: on the 20-bundle sample the dump is 3.0 times the input uncompressed and 0.32 times gzipped (see "Reproducing the numbers").
- Output is written to `OUT.partial` and renamed when complete.

**Selecting inputs.**
- `--name GLOB` (repeatable) filters on the dump-relative path.
- `--build 4100` and `--kind bundle` filter index entries.
- `--limit N` keeps the first N.

**Other options.**
- `--levels L0,L1,L2` drops levels. L0 is always dumped, L3 needs L2, and an unknown name is refused. L3 is the slowest level, because UnityPy's writer is pure Python.
- Every input is dumped in a process forked for it alone, so nothing UnityPy caches or leaks for one input reaches another. An input whose process dies of a signal (a crash inside UnityPyBoost, the cgroup OOM killer), before or while it returns its records, gets one `file` record with error kind `crash` and a warning on stderr, and the dump goes on (spec section 4.2).
- A raise in the dumper's own code, or an input process that ends without its records and without a signal, is a dumper bug, not an outcome of the input: the run stops with exit 1 and no dump. A build of Python in which `None` cannot be pinned (section 7 of the spec) is refused before any input runs. When every input crashes, the dump is written but the exit status is 3, because that says more about the machine than the inputs.
- Input names are the paths' bytes as given, decoded as UTF-8 whatever the locale; symlinks are named after themselves.
- `-j N` runs N of those processes at a time. The output is byte-identical to a serial run.
- The container has a 12 GB cap, shared with other sessions; watch `scripts/dev cat /sys/fs/cgroup/memory.current` on runs over the whole corpus.

**Reporting.**
- A summary prints on stderr: files, objects, L2 error kinds, L3 outcomes, and time per phase (load, parse, encode, resave).
- `--stats PATH` also writes per-input timings.
- An index entry whose sha256 no longer matches its file is reported as `index_sha256_mismatch`.

The unityrs CLI will write the same format. To compare like with like, give both producers the same inputs: the same folder, or the same `--name` selection.

## Comparing

```sh
python tools/parity/compare.py /tmp/dumps/unitypy.jsonl.gz /tmp/dumps/unityrs.jsonl.gz --json /tmp/dumps/report.json
```

**Arguments.** The first dump is the reference (UnityPy) and the second the candidate. The `--json` report quotes input names, so like the dumps it must lie outside the repository (symlinks resolved); a path inside it exits 2.

**Exit status.** `compare.py` exits 0 when the compared levels agree and every line is canonical, and 1 when anything differs. It exits 2 when a dump is unreadable (not JSON, not valid Unicode, `NaN`, a truncated gzip stream, a record without a valid key) or in another format version, and when the arguments ask for a comparison the reference cannot give (below). Every error is one line on stderr.

**Report.** It lists:
- matched, differing, missing, extra, duplicate and noncanonical records;
- differences by level and by class (the typetree root type);
- the top field paths, with list indices folded to `[]` and three concrete examples each (`--examples`, `--max-paths`).

**Levels.** By default the levels the reference holds are compared, and a level the candidate's header lacks is a `missing_level` difference. `--levels L0,L1` narrows the comparison. A name outside L0-L3, a level the reference lacks, or an empty list exits 2.

**The reference.** A reference written by `unitypy_dump.py` must record the pinned stack in its header's `producer` (`boost: true`, UnityPy 1.25.2, the pinned UnityPyBoost's `boost_sha256` and exactly the pinned `packages`), or the comparison exits 2. The candidate's `producer` is not checked.

**What counts as a difference.**
- Types must match exactly, at any depth: a tuple is not a list, `true` is not `1`, and an `f32` is not an `f64`, also inside a struct inside an array.
- **`f32_snan_quieted`** marks float32 pairs that differ only because UnityPy quiets a signalling NaN (spec section 7). The candidate must be that signalling NaN; a NaN against an infinity is a plain `value` difference.
- **`format`** findings are breaks of spec sections 2-4, found line by line in both dumps, byte-identical pairs and the header and `end` records included. The kinds include `noncanonical_json` (not the line's own canonical serialisation), `json_float`, `members` (not the record's member list in order), `undeclared_level` and `missing_section` (a section the header's levels do not or do call for), `kind_mismatch`, `error_mismatch` and `value_mismatch` (a section that disagrees with `parsed_as` or with `l2.error`), `bad_name` and `hex_name_is_utf8` (the metadata-string rule), the value-encoding kinds (`unpacked`, `inline_too_long`, `summary_too_short`, `bad_hex`, `str_is_utf8`, `duplicate_key` and others), and `noncanonical_value` (equal values in other bytes). A missing `end` record, one whose counts disagree with the dump, and a record after it are `format` findings too.
- A second record with a key already read is a `duplicate`; the first one is compared.

Records are matched by key, not position: `file` by path, `object` by path and `path_id`. The order is checked separately (`order`, `order_differs`; spec section 6). Every line is parsed and checked, identical ones included. Comparing the 20-bundle sample dump (38 MB, 12,880 records) with a second dump of the same inputs takes about 1 second.

## Self-tests

```sh
scripts/dev python -m unittest discover -s tools/parity/tests -v
```

| module | covers | needs |
|---|---|---|
| `test_canonical.py` | the JSON and value-encoding rules, with a golden line for Rust, and the single-line canonical-form checks | stdlib |
| `test_compare.py` | every finding kind on hand-built dumps: nested type mismatches at L1 and L2, identical noncanonical lines, level coverage, duplicates, unreadable input, a reference whose producer is not the pinned stack (UnityPy, the Boost binary, the packages) | stdlib |
| `test_unitypy_boost.py` | synthetic typetrees through UnityPyBoost: float bits, NaN quieting, bool bytes, pairs, ReferencedObject and the registry scope, big-endian, every L2 error kind on both reading paths, the pure-Python path's tuples, 16-bit swap, registries in aligned class arrays, negative TypelessData lengths, and arrays of aligned vectors and TypelessData, version packing. It also checks that the spec's worked example is what the oracle writes, and that the loaded UnityPyBoost binary and its wheel's RECORD hash as `ORACLE_BOOST_SHA256`. | UnityPy |
| `test_dump.py` | the dumper end to end: UnityPy's samples, determinism across runs, `-j` and gzip, every line canonical, L0 detection quirks including gzip, UnityRaw formats 5 to 7, a failing entry, non-UTF-8 names; synthetic SerializedFiles (`tests/synth.py`) for the object table, check_read, resave and TPK wiring, version strings, type-record defaults by format, negative type indices, node rows and how blob rows become a tree, truncated blocks and zip entries; crash records (a killed input, a death while it returns its records, UnityPyBoost crashes), the dumper's own failures stopping the run, input names across locales and symlinks, load errors that quote bad bytes, the `None` leak, the pinned-stack refusal (UnityPyBoost, its binary, UnityPy, a package such as numpy missing or at another version) and the stack the header records, index mode, the output and `--stats` guards | UnityPy, samples |
| `test_oracle_env.py` | the oracle's pins as `requirements-oracle.txt` gives them (numpy included), and `oracle_env.py`'s checks on synthetic installs: versions, `--exact`, `--records` | stdlib |
| `test_fetch_samples.py` | the sample fetcher against a fake server: fetching, keeping and replacing files, LFS pointers, wrong and over-long bodies (read no further than one byte past the pin), retries and the overall deadline, redirects (two loopback servers), the repository guard; pins with names that are not plain file names or URLs off the media host; a folder holding anything but the pinned samples as regular files (extra files, symlinks, folders, stale temporary files); the exclusively created temporary file; and that `unitypy_samples.json` pins the samples `test_dump.py` checks | stdlib |

**Samples.** The samples are UnityPy's `tests/samples` at commit `417d595`. They are read from `$UNITYRS_UNITYPY_SAMPLES` when set, else from the folder the corpus index (`$UNITYRS_CORPUS_INDEX`, default `/corpus/index.json`) gives for its `unitypy-sample` entries, and their sha256 is checked against pinned values. They are game bundles, so they are never copied into this repository. Without them the sample tests skip.

To get them anywhere else, fetch them into a folder outside the repository:

```sh
python tools/parity/fetch_unitypy_samples.py /tmp/unitypy-samples
UNITYRS_UNITYPY_SAMPLES=/tmp/unitypy-samples python -m unittest discover -s tools/parity/tests
```

The pins are in `unitypy_samples.json`: the commit, and each file's size and sha256, which is also its Git LFS oid. The samples are Git LFS objects. raw.githubusercontent.com serves a 130-byte LFS pointer in their place, so the fetcher downloads from media.githubusercontent.com and refuses a pointer. That host serves the file without a redirect, and the fetcher follows none. The fetcher:
- refuses pins whose names are not plain file names, or whose URL leaves `https://media.githubusercontent.com/media/K0lb3/UnityPy/`;
- refuses a folder inside the repository, and a folder that holds anything but the pinned samples as regular files (an extra file, a symlink, a folder, a temporary file left by a killed run), so a folder restored from a cache is checked like a download;
- keeps a sample already there with the pinned size and sha256, and downloads any other again;
- gives each sample's download, retries included, `DEADLINE_S` (300 seconds) in all, and refuses a body that is not the pinned bytes;
- writes each sample to a new temporary file in the folder, created exclusively without following a symlink, and renames it into place.

## CI

The `parity-py` job in `.github/workflows/ci.yml` runs on GitHub's Ubuntu runner with Python 3.11:
1. It installs the oracle stack into a fresh venv with `pip install --require-hashes -r tools/parity/requirements-oracle.txt`, and `oracle_env.py --exact` checks that the venv holds the pins and nothing else.
2. It restores UnityPy's samples from the Actions cache, keyed by the hash of `unitypy_samples.json`, or fetches them. Either way `fetch_unitypy_samples.py` checks them against the pins.
3. It runs this folder's self-tests, the spikes' script tests (`crates/unityrs-spike-rabex/scripts/tests`, `crates/unityrs-bench-lzma`) and `scripts/tests` with `$UNITYRS_UNITYPY_SAMPLES` set, through `scripts/unittest-suites`. With the stack and the samples in place, a skipped test fails the job, and so does a suite that runs fewer tests than the minimum the workflow names for it.
4. It dumps the samples with `unitypy_dump.py` and compares the dump with itself: a smoke test that the oracle runs there. The unityrs side joins this comparison later.

**Re-pinning the oracle stack** (DESIGN.md section 3). `requirements-oracle.txt` names one wheel per package: the cp311 manylinux x86_64 wheel the oracle image holds. A re-pin changes the image and this file together:
1. For each package, take the sha256 of that wheel from PyPI (`https://pypi.org/pypi/NAME/VERSION/json`, the `digests` of the file whose name matches the tags in the image's `*.dist-info/WHEEL`).
2. Install the file into a fresh venv (`python -m pip install --require-hashes -r tools/parity/requirements-oracle.txt`) and run `pip check`.
3. In that venv, `python tools/parity/oracle_env.py --exact` checks that it holds the pins and nothing else, and `python tools/parity/oracle_env.py --records SITE`, with SITE the image's site-packages folder (`/usr/local/lib/python3.11/site-packages` in the `unityrs-dev` container), that every installed file hashes as the image's does.
4. In the image, `python tools/parity/oracle_env.py` checks that it holds every pin. The image holds other packages too; check that UnityPy imports none of them (`requirements-oracle.txt` says which it can), and pin any it does.
5. When UnityPy moves, set `ORACLE_BOOST_SHA256` in `canonical.py` to the sha256 of the new wheel's UnityPyBoost binary (its RECORD entry). `ORACLE_UNITYPY` follows the pins.

## Reproducing the numbers

Every measured number above comes from these commands, run in the `unityrs-dev` container (the oracle image `unityrs-oracle:upy1.25.2-2026-10-28`) on 2026-10-28:

```sh
# The 20-bundle sample, at -j 8 and gzipped, with its stats; then the same inputs serially.
scripts/dev python tools/parity/unitypy_dump.py --index /corpus/index.json --kind bundle \
    --sample 20 -j 8 -q -o /tmp/dumps/corpus20.jsonl.gz --stats /tmp/dumps/corpus20.stats.json
scripts/dev python tools/parity/unitypy_dump.py --index /corpus/index.json --kind bundle \
    --sample 20 -q -o /tmp/dumps/corpus20.jsonl
# Sizes: bytes_out is the uncompressed dump (37,940,491 bytes, the 38 MB above); the ratios are
# bytes_out / bytes_in (3.02) and output_bytes / bytes_in (0.32).
scripts/dev python -c "import json; s = json.load(open('/tmp/dumps/corpus20.stats.json'))['summary']; \
    print(s['bytes_out'] / s['bytes_in'], s['output_bytes'] / s['bytes_in'])"
# Comparison time (0.97-0.98 s real over three runs), and the record count in its report.
scripts/dev bash -c 'time python tools/parity/compare.py /tmp/dumps/corpus20.jsonl /tmp/dumps/corpus20.jsonl.gz'
```
