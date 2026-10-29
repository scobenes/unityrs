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
| `inventory.py` | lists UnityPy's public surface and checks that [`PARITY.md`](../../PARITY.md) names all of it |
| `references.py` | the independent reference for each TextureFormat UnityPy raises on (Tier B, `PARITY.md` section 17.1): closed forms, and texture2ddecoder for BC6H |
| `pillow_tables.py` | prints the two Pillow 9.5.0 tables of `PARITY.md`: what `Image.save` writes per extension and mode (section 15.8), and the image modes `set_image` packs (section 18.1) |
| `tests/` | unittest self-tests |

`unitypy_dump.py` needs the oracle stack, so run it in the `unityrs-dev` container. It refuses to run without UnityPyBoost or with any UnityPy other than 1.25.2, because a dump from another stack would charge UnityPy's differences to unityrs. `compare.py` and `canonical.py` need only Python 3.10 or later.

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

**The reference.** A reference written by `unitypy_dump.py` must record `boost: true` and UnityPy 1.25.2 in its header's `producer`, or the comparison exits 2.

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
| `test_compare.py` | every finding kind on hand-built dumps: nested type mismatches at L1 and L2, identical noncanonical lines, level coverage, duplicates, unreadable input | stdlib |
| `test_unitypy_boost.py` | synthetic typetrees through UnityPyBoost: float bits, NaN quieting, bool bytes, pairs, ReferencedObject and the registry scope, big-endian, every L2 error kind on both reading paths, the pure-Python path's tuples, 16-bit swap, registries in aligned class arrays, negative TypelessData lengths, and arrays of aligned vectors and TypelessData, version packing. It also checks that the spec's worked example is what the oracle writes. | UnityPy |
| `test_dump.py` | the dumper end to end: UnityPy's samples, determinism across runs, `-j` and gzip, every line canonical, L0 detection quirks including gzip, UnityRaw formats 5 to 7, a failing entry, non-UTF-8 names; synthetic SerializedFiles (`tests/synth.py`) for the object table, check_read, resave and TPK wiring, version strings, type-record defaults by format, negative type indices, node rows and how blob rows become a tree, truncated blocks and zip entries; crash records (a killed input, a death while it returns its records, UnityPyBoost crashes), the dumper's own failures stopping the run, input names across locales and symlinks, load errors that quote bad bytes, the `None` leak, the pinned-stack refusal, index mode, the output and `--stats` guards | UnityPy, samples |
| `test_inventory.py` | the inventory's matching rules (standard library): whole dotted chains, compact rows, enum values, a name under a wrong parent, what counts as an attribute; with the oracle, that `collect()` finds exactly the group sizes `PARITY.md` section 21 states, that `PARITY.md` names every UnityPy module, class, function, module-level table or setting, class member, attrs field, other attribute, package re-export, UnityPyBoost export or member, enum member with its value, and `TextureFormat`, and that removing any one entry, or writing it under a wrong parent, fails the check | stdlib; UnityPy for the rest |
| `test_texture_matrix.py` | the TextureFormat matrix of `PARITY.md` section 17: its columns, and its tier and phase rules, and section 17.1's table of Tier B references, one per Tier B row, with the values it states checked against `references.py` (standard library; BC6H's needs texture2ddecoder); with the oracle, every cell of every row, built from UnityPy (`CONV_TABLE`, the cited source lines, the result on real-sized data, the padding, Switch, Xbox and crunch gates observed through stand-ins, what `set_image` writes) and from the pixel cases, and a check that no builder reads `PARITY.md` (the rows built again in a new process from a copy with every cell changed are the same); the decoded pixels of every format that returns an image against a reference built from the format's layout (or, for a row marked wrong, the specific wrong pixels), also swizzled for Switch and byte-swapped for XBOX360 as the gates say | stdlib; UnityPy for the rest |
| `test_parity_rows.py` | the `PARITY.md` rows a review disputed, each run on the smallest synthetic input that shows it (UnityFS bundles, WebFiles and serialized files built by `synth.py`); each test names its rows, and each row's test column names the test back | UnityPy |
| `test_pillow_tables.py` | `PARITY.md` sections 15.8 and 18.1 hold exactly what `pillow_tables.py` prints | UnityPy, Pillow 9.5.0 |

**Samples.** The samples are UnityPy's `tests/samples` at commit `417d595`. They are read from `$UNITYRS_UNITYPY_SAMPLES` when set, else from the folder the corpus index (`$UNITYRS_CORPUS_INDEX`, default `/corpus/index.json`) gives for its `unitypy-sample` entries, and their sha256 is checked against pinned values. They are game bundles, so they are never copied into this repository. Without them the sample tests skip.

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
