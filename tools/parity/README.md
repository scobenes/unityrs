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
| `oracle_records.json` | each pinned wheel's files: the digest of its RECORD, written by `oracle_env.py --pin-records` |
| `oracle_env.py` | checks that an interpreter's packages are the pinned wheels (each pin at its version, with the pinned RECORD, every file hashing as it says, imported from those files), and with `--exact` that nothing else is installed, or with `--image` that the process loads the oracle image's zlib; `--pin-records DIR` writes `oracle_records.json` |
| `inventory.py` | lists UnityPy's public surface and checks that [`PARITY.md`](../../PARITY.md) names all of it |
| `references.py` | the independent reference for each TextureFormat UnityPy raises on (Tier B, `PARITY.md` section 17.1): closed forms, and for BC6H the decoding of the Khronos Data Format Specification 1.3, section 20.2 |
| `object_mode.py` | prints the object-mode table of `PARITY.md` section 9.2.1: what UnityPyBoost and the pure-Python path build for every attribute of `UnityPy.classes` |
| `pillow_tables.py` | prints the Pillow 9.5.0 tables of `PARITY.md`: what `Image.save` writes per extension, mode, size and file name (section 15.8), the image modes `set_image` packs (section 18.1), and which libraries the writers call, the host's zlib among them (section 1) |
| `tests/` | unittest self-tests |

`unitypy_dump.py` needs the oracle stack, so run it in the `unityrs-dev` container, or install the stack from `requirements-oracle.txt` (below). It refuses to run on any other stack, because a dump from another stack would charge UnityPy's differences to unityrs:
- without UnityPyBoost, or with any UnityPy other than 1.25.2;
- with a UnityPyBoost binary other than the pinned wheel's (its sha256 is `ORACLE_BOOST_SHA256` in `canonical.py`);
- when a package `requirements-oracle.txt` pins is missing or at another version. numpy is one: UnityPy is not built on it, but uses it when it is there, and renders sprites differently without it. cffi (with its dependency pycparser) and zstandard are pinned because Pillow and fsspec import them when they are there and the image holds them, so CI's oracle loads the same code as the image's; pycryptodome because UnityPy imports it to decrypt a UnityCN bundle, which without it does not load;
- when a pinned package is not its pinned wheel's files: its RECORD is not the one `oracle_records.json` pins (another build of the version), one of its files does not hash as that RECORD says (an edited file), it imports from elsewhere (a copy earlier on `sys.path`), its folders hold a file the RECORD does not list (`__pycache__` aside), or one of its files or anything in its folders is a symlink (no wheel installs one, and the check follows none). Every pinned file is hashed once per run;
- when it loads a module from anywhere but the standard library, this folder and the pinned wheels' files, such as a package a pin imports when it is installed (Pillow imports defusedxml when there is one). It checks after importing UnityPy, and again in each input's process after that input's dump, so a module loaded while dumping is caught too: the run then stops with exit 2 and no dump. A module named as one of the standard library's (`json`, which writes every line; `hashlib`; `gzip`) must come from the standard library's own folder, even when the interpreter loaded it at start-up. Any other module loaded at start-up (by a `.pth` file in site-packages) is not checked;
- when `PYTHONPATH`, `PYTHONHOME` or `PYTHONPLATLIBDIR` is set, since each puts other folders before the standard library or moves it (unset them, or run `python -E`), or when a user site-packages folder is on `sys.path`, since its `.pth` files run at start-up (run `python -s`, or `-I` for both).

The dump's header records the stack it ran on: UnityPyBoost's sha256, every pinned package's version, and each one's RECORD digest. `compare.py`, `canonical.py`, `oracle_env.py` and `fetch_unitypy_samples.py` need only Python 3.10 or later.

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
- The dump, its `.partial` file (below) and the stats must be three different files, none of them a symlink, an input or (with `--index`) the index. They are compared by path, with the folders' symlinks resolved, and by `(st_dev, st_ino)`, so a hard link counts as the file it links to. The dumper refuses before it writes anything, so `--stats` can never overwrite the dump it describes.
- A `.gz` suffix writes gzip. Use it for anything bigger than a handful of bundles: on the 20-bundle sample the dump is 3.0 times the input uncompressed and 0.32 times gzipped (see "Reproducing the numbers").
- Output is written to `OUT.partial` and renamed when complete. The `.partial` file is created new (`O_EXCL`, `O_NOFOLLOW`): one a killed run left is removed first, and one that appears after the checks stops the run. The stats are written to a new file too, and renamed over the `--stats` path.

**Selecting inputs.**
- `--name GLOB` (repeatable) filters on the dump-relative path.
- `--build 4100` and `--kind bundle` filter index entries.
- `--limit N` keeps the first N.

**Other options.**
- `--levels L0,L1,L2` drops levels. L0 is always dumped, L3 needs L2, and an unknown name is refused. L3 is the slowest level, because UnityPy's writer is pure Python.
- Every input is dumped in a process forked for it alone, so nothing UnityPy caches or leaks for one input reaches another. An input whose process dies of a signal (a crash inside UnityPyBoost, the cgroup OOM killer), before or while it returns its records, gets one `file` record with error kind `crash` and a warning on stderr, and the dump goes on (spec section 4.2).
- A raise in the dumper's own code, or an input process that ends without its records and without a signal, is a dumper bug, not an outcome of the input: the run stops with exit 1 and no dump. So is the canonical encoder refusing a value UnityPy read: the run stops and names the input and the object, and no `encode` error is written (spec section 4.3). A build of Python in which `None` cannot be pinned (section 7 of the spec) is refused before any input runs. When every input crashes, the dump is written but the exit status is 3, because that says more about the machine than the inputs.
- Input names are the paths' bytes as given, decoded as UTF-8 whatever the locale; symlinks are named after themselves.
- `-j N` runs N of those processes at a time. The output is byte-identical to a serial run.
- The container has a 12 GB cap, shared with other sessions; watch `scripts/dev cat /sys/fs/cgroup/memory.current` on runs over the whole corpus.

**Reporting.**
- A summary prints on stderr: files, objects, L2 error kinds, L3 outcomes, and time per phase (load, parse, encode, resave).
- `--stats PATH` also writes per-input timings.
- An input whose sha256 differs from its index entry stops the run: exit 1, no dump, and no stats, since the dump would not be of the corpus the index pins. `--allow-index-mismatch` dumps it anyway, with a warning, and the summary counts it as `index_sha256_mismatch`.

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

**The reference.** A reference written by `unitypy_dump.py` must record the pinned stack in its header's `producer` (`boost: true`, UnityPy 1.25.2, the pinned UnityPyBoost's `boost_sha256`, exactly the pinned `packages` and exactly the pinned wheels' `records`), or the comparison exits 2. The candidate's `producer` is not checked.

**What counts as a difference.**
- Types must match exactly, at any depth: a tuple is not a list, `true` is not `1`, and an `f32` is not an `f64`, also inside a struct inside an array.
- **`f32_snan_quieted`** marks float32 pairs that differ only because UnityPy quiets a signalling NaN (spec section 7). The candidate must be that signalling NaN; a NaN against an infinity is a plain `value` difference.
- **`format`** findings are breaks of spec sections 2-4, found line by line in both dumps, byte-identical pairs and the header and `end` records included. The kinds include `noncanonical_json` (not the line's own canonical serialisation), `json_float`, `members` (not the record's member list in order), `undeclared_level` and `missing_section` (a section the header's levels do not or do call for), `kind_mismatch`, `error_mismatch` and `value_mismatch` (a section that disagrees with `parsed_as` or with `l2.error`), `bad_name` and `hex_name_is_utf8` (the metadata-string rule), the value-encoding kinds (`unpacked`, `inline_too_long`, `summary_too_short`, `bad_hex`, `str_is_utf8`, `duplicate_key` and others), and `noncanonical_value` (equal values in other bytes). A missing `end` record, one whose counts disagree with the dump, and a record after it are `format` findings too. So are an `l0` value or pair section 4.2 does not allow (`unknown_value`, `l0_mismatch`, `bad_value`), an error kind the spec does not list, and, in the reference only, an `encode` L2 error (`reference_encode`): only a producer under test writes one, while the oracle stops on its own encoder's failure (spec section 4.3).
- A second record with a key already read is a `duplicate`; the first one is compared.

Records are matched by key, not position: `file` by path, `object` by path and `path_id`. The order is checked separately (`order`, `order_differs`; spec section 6). Every line is parsed and checked, identical ones included. Comparing the 20-bundle sample dump (38 MB, 12,880 records) with a second dump of the same inputs takes about 1 second.

## Self-tests

```sh
scripts/dev python -m unittest discover -s tools/parity/tests -v
```

| module | covers | needs |
|---|---|---|
| `test_canonical.py` | the JSON and value-encoding rules, with a golden line for Rust, and the single-line canonical-form checks, `l0`'s values and pairs and the error kinds among them | stdlib |
| `test_compare.py` | every finding kind on hand-built dumps: nested type mismatches at L1 and L2, identical noncanonical lines, level coverage, duplicates, unreadable input, a reference whose producer is not the pinned stack (UnityPy, the Boost binary, the packages, the wheels' record digests), an `encode` error in the reference and in the candidate | stdlib |
| `test_unitypy_boost.py` | synthetic typetrees through UnityPyBoost: float bits, NaN quieting, bool bytes, pairs, ReferencedObject and the registry scope, big-endian, every L2 error kind on both reading paths, the pure-Python path's tuples, 16-bit swap, registries in aligned class arrays, negative TypelessData lengths, and arrays of aligned vectors and TypelessData, version packing. It also checks that the spec's worked example is what the oracle writes, and that the loaded UnityPyBoost binary and its wheel's RECORD hash as `ORACLE_BOOST_SHA256`. | UnityPy |
| `test_dump.py` | the dumper end to end: UnityPy's samples, determinism across runs, `-j` and gzip, every line canonical, L0 detection quirks including gzip, UnityRaw formats 5 to 7, a failing entry, non-UTF-8 names; synthetic SerializedFiles (`tests/synth.py`) for the object table, check_read, resave and TPK wiring, version strings, type-record defaults by format, negative type indices, node rows and how blob rows become a tree, truncated blocks and zip entries; crash records (a killed input, a death while it returns its records, UnityPyBoost crashes), the dumper's own failures stopping the run (its encoder refusing UnityPy's value included), input names across locales and symlinks, load errors that quote bad bytes, the `None` leak, the pinned-stack refusal (UnityPyBoost, its binary, UnityPy, a package such as numpy missing or at another version, a copy of UnityPy with one line patched, with or without its dist-info, imported first, or linked in as a package beside the module; a module from outside the pins loaded at import or while an input is dumped) and the stack the header records, index mode (an input that is not the indexed file stops the run unless `--allow-index-mismatch`), the output and `--stats` guards (inside the repository, one file for two outputs, an output that is an input) | UnityPy, samples |
| `test_oracle_env.py` | the oracle's pins as `requirements-oracle.txt` gives them (numpy, cffi, pycparser, pycryptodome and zstandard included), `oracle_records.json` against them, and `oracle_env.py`'s checks on synthetic installs and wheels: versions, `--exact`, the RECORD digest (another build), an edited, missing or planted file, a symlink (a folder linked in beside a module, a listed file, the package folder, one in `__pycache__`, a dangling one), a copy earlier on the path, what the installer writes, `--pin-records`; and which loaded modules may come from where (the standard library, the tools, the pinned files) | stdlib |
| `test_fetch_samples.py` | the sample fetcher against a fake server: fetching, keeping and replacing files, LFS pointers, wrong and over-long bodies (read no further than one byte past the pin), retries and the overall deadline (on the fake; on loopback servers that trickle the status line, a header, a chunk's size line or the body, or stall a read; on a listener whose connects hang; and that a download cut off writes nothing), redirects (two loopback servers), the repository guard; pins with names that are not plain file names or URLs off the media host; a folder holding anything but the pinned samples as regular files (extra files, symlinks, folders, stale temporary files); the exclusively created temporary file; and that `unitypy_samples.json` pins the samples `test_dump.py` checks | stdlib |
| `test_inventory.py` | the inventory's matching rules (standard library): whole dotted chains, compact rows, enum values, a name under a wrong parent, what counts as an attribute; with the oracle, that `collect()` finds exactly the group sizes `PARITY.md` section 21 states, that `PARITY.md` names every UnityPy module, class, function, module-level table or setting, class member, attrs field, other attribute, package re-export, UnityPyBoost export or member, enum member with its value, and `TextureFormat`, and that removing any one entry, or writing it under a wrong parent, fails the check | stdlib; UnityPy for the rest |
| `test_texture_matrix.py` | the TextureFormat matrix of `PARITY.md` section 17: its columns, and its tier and phase rules, and section 17.1's table of Tier B references, one per Tier B row, with the values it states checked against `references.py` (standard library), and BC6H's reference against the two BC6H decoders in the oracle image, each with its one departure from the specification; with the oracle, every cell of every row, built from UnityPy (`CONV_TABLE`, the cited source lines, the result on real-sized data, the padding, Switch, Xbox and crunch gates observed through stand-ins, what `set_image` writes) and from the pixel cases, and a check that no builder reads `PARITY.md` (the rows built again in a new process from a copy with every cell changed are the same); the decoded pixels of every format that returns an image against a reference built from the format's layout (or, for a row marked wrong, the specific wrong pixels), also swizzled for Switch and byte-swapped for XBOX360 as the gates say | stdlib; UnityPy for the rest |
| `test_parity_rows.py` | the `PARITY.md` rows a review disputed, each run on the smallest synthetic input that shows it (UnityFS bundles, WebFiles and serialized files built by `synth.py`); each test names its rows, and each row's test column names the test back | UnityPy |
| `head_oracle.py`, `head_417d595/` | not tests: UnityPy git `417d595`'s `helpers/CompressionHelper.py` and `files/BundleFile.py`, kept byte for byte with its MIT `LICENSE` (ruff skips the folder), and the loader that checks each file's git blob id and runs them inside UnityPy 1.25.2, for the HEAD-tier writing rows (`PARITY.md` section 20; `test_rows_files.py` head_saves) | UnityPy |
| `test_object_mode.py` | `PARITY.md` section 9.2.1 holds exactly what `object_mode.py` prints, and section 9.2's extra-field rule holds for every class of `UnityPy.classes` | UnityPy with UnityPyBoost |
| `test_pillow_tables.py` | `PARITY.md` sections 15.8 and 18.1, and section 1's row for the libraries the Pillow writers call, hold exactly what `pillow_tables.py` prints; that row holds nothing the host decides | UnityPy, Pillow 9.5.0 |

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
- gives each sample's download, retries included, `DEADLINE_S` (300 seconds) of wall time in all, wherever it is held up: the name lookup, connecting to each address, the TLS handshake, the status line, the headers, a chunk's size line or the body. urllib bounds each receive by `TIMEOUT_S` (60 seconds) but none of them by a deadline, so the download runs in a worker thread and the fetcher stops waiting for it at the deadline. The worker writes nothing, so whatever it receives after that is discarded, and it stops at its next check of the deadline (between reads, which take one receive each, and between retries);
- refuses a body that is not the pinned bytes;
- writes each sample to a new temporary file in the folder, created exclusively without following a symlink, and renames it into place.

## CI

The `parity-py` job in `.github/workflows/ci.yml` runs on GitHub's Ubuntu runner with Python 3.11:
1. It installs the oracle stack into a fresh venv with `pip install --require-hashes -r tools/parity/requirements-oracle.txt`, and `oracle_env.py --exact` checks that the venv holds the pinned wheels' files and nothing else.
2. It restores UnityPy's samples from the Actions cache, keyed by the hash of `unitypy_samples.json`, or fetches them. Either way `fetch_unitypy_samples.py` checks them against the pins.
3. It runs this folder's self-tests and the spikes' script tests (`crates/unityrs-spike-rabex/scripts/tests`, `crates/unityrs-bench-lzma`) with `$UNITYRS_UNITYPY_SAMPLES` set, through `scripts/unittest-suites`. With the stack and the samples in place, a skipped test fails the job, and so does a suite that runs fewer tests than the minimum the workflow names for it. (`scripts/tests` runs in the `scripts-py` job, in a venv holding `scripts/requirements-tests.txt`: its workflow test parses the workflow with PyYAML, which is no part of the oracle.)
4. It dumps the samples with `unitypy_dump.py` and compares the dump with itself: a smoke test that the oracle runs there. The unityrs side joins this comparison later.

**Re-pinning the oracle stack** (DESIGN.md section 3). `requirements-oracle.txt` names one wheel per package: the manylinux x86_64 wheel the oracle image holds, cp311 or, for pycryptodome, abi3 (`cp37-abi3`). A re-pin changes the image and this file together:
1. For each package, find that wheel on PyPI (`https://pypi.org/pypi/NAME/VERSION/json`, the file whose name matches the tags in the image's `*.dist-info/WHEEL`). Its sha256 (`digests`) goes into `requirements-oracle.txt`, and the file itself (`url`) into a folder DIR outside the repository.
2. `python tools/parity/oracle_env.py --pin-records DIR` checks that DIR holds exactly the pinned wheels and writes `oracle_records.json` from their RECORDs.
3. Install the wheels into a fresh venv (`python -m pip install --require-hashes -r tools/parity/requirements-oracle.txt`) and run `pip check`. In that venv, `python tools/parity/oracle_env.py --exact` checks that it holds the pinned wheels' files and nothing else.
4. In the image, `python tools/parity/oracle_env.py --image` checks that it holds the same wheels' files and loads the zlib `PARITY.md` section 1 names (`ORACLE_IMAGE_ZLIB` in `oracle_env.py`, the reference for Pillow's PNG, ICO and ICNS bytes): a new base image with another zlib changes that constant and the docs that name it, or is not the oracle. The image holds other packages too: dump the samples and a corpus sample there with `unitypy_dump.py`, which refuses to run, naming the module, when UnityPy or a pin loads one from another package. Pin any package it names, with its dependencies, and repeat from step 1.
5. When UnityPy moves, set `ORACLE_BOOST_SHA256` in `canonical.py` to the sha256 of the new wheel's UnityPyBoost binary (its RECORD entry). `ORACLE_UNITYPY` follows the pins.

## The inventory

```sh
scripts/dev python tools/parity/inventory.py list [--json]   # UnityPy's public symbols, by group
scripts/dev python tools/parity/inventory.py list --counts   # how many in each group
scripts/dev python tools/parity/inventory.py enum-table       # the rows of PARITY.md's Appendix C
scripts/dev python tools/parity/inventory.py check            # exit 1 and a list when PARITY.md misses any
```

They need UnityPy 1.25.2 and exit 2 without it. Section 21 of `PARITY.md` gives the naming rules.

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
# PARITY.md section 19.1's shader numbers (980 Shaders, 89,910 sub-programs, 946 texts, 34 raises),
# over both builds, run on 2026-10-29.
scripts/dev python tools/parity/shader_survey.py --index /corpus/index.json --build 4100,100056
```
