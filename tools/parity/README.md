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
| `dump_records.py` | the oracle's records of one input (its file and object records, or its crash record), which `unitypy_dump.py` runs in a process per input |
| `compare.py` | diffs two dumps; summarises by level, class and field path |
| `g3b.py` | G3b's verdict (`docs/plans/core.md` section 6): every file under `--inputs` in both dumps, and each mutated input the dumps differ on, with its reason checked (a limit, the non-UTF-8 crash, a count UnityPy allocates before it reads; never a unityrs crash); needs the oracle stack |
| `value_cases.py` | G2 (`docs/plans/core.md` section 6): synthetic typetree cases, each read and resaved by UnityPyBoost and by `unityrs values`; exits 0 when every case's L2 verdict is `match` and none's L3 verdict is `differs` or `one_side`; needs the oracle stack |
| `child.py` | how every Python test and tool here runs a child process: a child the system killed (SIGKILL) is run again, up to `KILLED_RUNS` runs, any other ending comes back at once, and each run gets a scratch folder its parent removes (standard library only) |
| `scratch.py` | where every Python test and tool here writes outside its memory: scratch folders removed whatever happens, a test's with its cleanups, a child's with its run, a dead process's at the next sweep (standard library only) |
| `canonical.py` | the encoding rules both scripts share (standard library only), and by name everything `oracle_stack.py`, `canonical_encode.py`, `canonical_json.py` and `canonical_decode.py` hold |
| `canonical_encode.py` | the value encoder: UnityPy's value, typed member by member by the node each reader read it with (spec section 3) |
| `canonical_json.py` | a record to its line and back, at any depth and integer width (spec section 2) |
| `oracle_stack.py` | the oracle stack a reference dump must come from: the pinned packages, their RECORD digests, UnityPyBoost's sha256, and the checks against them |
| `canonical_decode.py` | the value decoder `compare.py` reads canonical values with |
| `compare_diff.py` | how `compare.py` diffs two records with one key, with walks that never recurse |
| `fetch_unitypy_samples.py` | fetches UnityPy's test samples at the pinned commit, checked against `unitypy_samples.json` |
| `unitypy_samples.json` | the pinned commit of UnityPy's test samples, and each sample's name, size and sha256 |
| `requirements-oracle.txt` | the oracle stack as exact, hash-pinned wheels, for CI and any machine without the oracle image |
| `oracle_records.json` | each pinned wheel's files: the digest of its RECORD, written by `oracle_env.py --pin-records` |
| `oracle_env.py` | checks that an interpreter's packages are the pinned wheels (each pin at its version, with the pinned RECORD, every file hashing as it says, imported from those files), and with `--exact` that nothing else is installed, or with `--image` that the process loads the oracle image's zlib; `--pin-records DIR` writes `oracle_records.json` |
| `inventory.py` | lists UnityPy's public surface and checks that [`PARITY.md`](../../PARITY.md) names all of it |
| `references.py` | the independent reference for each TextureFormat UnityPy raises on (Tier B, `PARITY.md` section 17.1): closed forms, and for BC6H the decoding of the Khronos Data Format Specification 1.3, section 20.2 |
| `object_mode.py` | prints the object-mode table of `PARITY.md` section 9.2.1: what UnityPyBoost and the pure-Python path build for every attribute of `UnityPy.classes` |
| `pillow_tables.py` | prints the Pillow 9.5.0 tables of `PARITY.md`: what `Image.save` writes per extension, mode, size and file name (section 15.8), the image modes `set_image` packs (section 18.1), and which libraries the writers call, the host's zlib among them (section 1) |
| `shader_survey.py` | surveys what the oracle's ShaderConverter does with every Shader in a corpus or a folder: the counts of `PARITY.md` section 19.1's ShaderConverter row |
| `ngui_cutter.py` | the NGUI cutter's reference (`docs/plans/extract.md` section 3, item 3): `docs/BATCH_SPEC.md` section 5 on one input's view, on UnityPy 1.25.2, Pillow 9.5.0's crop and PNG writer and CPython 3.11's `json.dumps`; a library and a command |
| `ngui_read.py` | the reference's reads: the view's Environment sealed to it, recognition from the typetree, the sheet's name and data (4.3's own-chain procedure for a streamed sheet), and each failure classified as `docs/plans/batch.md` 2.7 classifies it |
| `ngui_winners.py` | the reference's winners across inputs: `docs/BATCH_SPEC.md` 4.5's steps 1 to 3 (E31, E32, E61) over `ngui_cutter.py`'s results |
| `ngui_scenes.py` | unityrs-testkit's NGUI scenes, which cargo builds, for the reference's tests and `tools/fixtures/ngui.py` |
| `sprites_kind.py` | the `sprites` output kind's reference (`docs/plans/extract.md` E17): one PNG per Sprite of one input's view, its plan and sprite groups by `docs/BATCH_SPEC.md` 4.4, each image UnityPy 1.25.2's `SpriteHelper` and Pillow 9.5.0's PNG writer; a library and a command |
| `sprites_scenes.py` | unityrs-testkit's `sprites` scenes, which cargo builds, for `tools/fixtures/sprites.py` |
| `gltf_check.py` | L6g's independent check of a written GLB (`docs/plans/static3d.md` sections 2.4 and 3.2): reads the accessors with the standard library and compares them with MeshHandler's rows in Unity coordinates, X mirrored, V flipped and winding reversed, after the writer's documented normalisation; no exporter is its oracle |
| `mesh_samples.py` | L6 (`docs/plans/static3d.md`): every Mesh in the pinned UnityPy samples, its arrays, type tags, triangles and raw OBJ text against `unityrs-mesh`'s `mesh_l6` example, with section 19.1's corrected decoding; prints aggregate counts only; needs the oracle stack |
| `ruff.toml` | the lint and format settings for this folder (`uvx ruff check`, `uvx ruff format`) |
| `tests/` | unittest self-tests; `test_readme.py` checks that this table lists every file in this folder, and no other |

`unitypy_dump.py` needs the oracle stack, so run it in the `unityrs-dev` container, or install the stack from `requirements-oracle.txt` (below). It refuses to run on any other stack, because a dump from another stack would charge UnityPy's differences to unityrs:
- without UnityPyBoost, or with any UnityPy other than 1.25.2;
- with a UnityPyBoost binary other than the pinned wheel's (its sha256 is `ORACLE_BOOST_SHA256` in `oracle_stack.py`);
- when a package `requirements-oracle.txt` pins is missing or at another version. numpy is one: UnityPy is not built on it, but uses it when it is there, and renders sprites differently without it. cffi (with its dependency pycparser) and zstandard are pinned because Pillow and fsspec import them when they are there and the image holds them, so CI's oracle loads the same code as the image's; pycryptodome because UnityPy imports it to decrypt a UnityCN bundle, which without it does not load;
- when a pinned package is not its pinned wheel's files: its RECORD is not the one `oracle_records.json` pins (another build of the version), one of its files does not hash as that RECORD says (an edited file), it imports from elsewhere (a copy earlier on `sys.path`), its folders hold a file the RECORD does not list (`__pycache__` aside), or one of its files or anything in its folders is a symlink (no wheel installs one, and the check follows none). Every pinned file is hashed once per run;
- when it loads a module from anywhere but the standard library, this folder and the pinned wheels' files, such as a package a pin imports when it is installed (Pillow imports defusedxml when there is one). It checks after importing UnityPy, and again in each input's process after that input's dump, so a module loaded while dumping is caught too: the run then stops with exit 2 and no dump. A module named as one of the standard library's (`json`, which writes every line; `hashlib`; `gzip`) must come from the standard library's own folder, even when the interpreter loaded it at start-up. Any other module loaded at start-up (by a `.pth` file in site-packages) is not checked;
- when `PYTHONPATH`, `PYTHONHOME` or `PYTHONPLATLIBDIR` is set, since each puts other folders before the standard library or moves it (unset them, or run `python -E`), or when a user site-packages folder is on `sys.path`, since its `.pth` files run at start-up (run `python -s`, or `-I` for both).

The dump's header records the stack it ran on: UnityPyBoost's sha256, every pinned package's version, and each one's RECORD digest. `compare.py`, `canonical.py` (with `oracle_stack.py` and `canonical_decode.py`), `oracle_env.py` and `fetch_unitypy_samples.py` need only Python 3.10 or later.

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
- A raise in the dumper's own code, or an input process that ends without its records and without a signal, is a dumper bug, not an outcome of the input: that input gets one `file` record with error kind `dumper` (its traceback goes to stderr), and the dump goes on (spec section 4.2). The canonical encoder refusing a value UnityPy read costs that object alone: its L2 is an `encode` error whose note begins `the oracle's encoder: ` (spec section 4.3). Either way the dump is written, and the exit status is 1; `compare.py` never passes those records. An input that cannot be opened or read stops the run with exit 1 and no dump, as `unityrs dump` does. A build of Python in which `None` cannot be pinned (section 7 of the spec) is refused before any input runs. When every input crashes, the dump is written but the exit status is 3, because that says more about the machine than the inputs.
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

**The reference.** A reference written by `unitypy_dump.py` must record the pinned stack in its header's `producer` (`boost: true`, UnityPy 1.25.2, the pinned UnityPyBoost's `boost_sha256`, exactly the pinned `packages` and exactly the pinned wheels' `records`), or the comparison exits 2. With `--oracle`, which every gate's comparison passes (G1 and G3; `g3b.py` always asks for it), a reference any other producer wrote, or one whose header names no producer, exits 2 as well. The candidate's `producer` is checked for one thing, with `--oracle` only: a candidate whose `producer` holds `unbounded` (a dump made with `unityrs dump --unbounded`, limits lifted) exits 2, since no gate takes such a dump.

**What counts as a difference.**
- Types must match exactly, at any depth: a tuple is not a list, `true` is not `1`, and an `f32` is not an `f64`, also inside a struct inside an array.
- **`f32_snan_quieted`** marks float32 pairs that differ only because UnityPy quiets a signalling NaN (spec section 7). The candidate must be that signalling NaN; a NaN against an infinity is a plain `value` difference.
- **`format`** findings are breaks of spec sections 2-4, found line by line in both dumps, byte-identical pairs and the header and `end` records included. The kinds include `noncanonical_json` (not the line's own canonical serialisation), `json_float`, `members` (not the record's member list in order), `undeclared_level` and `missing_section` (a section the header's levels do not or do call for), `kind_mismatch`, `error_mismatch` and `value_mismatch` (a section that disagrees with `parsed_as` or with `l2.error`), `bad_name` and `hex_name_is_utf8` (the metadata-string rule), the value-encoding kinds (`unpacked`, `inline_too_long`, `summary_too_short`, `bad_hex`, `str_is_utf8`, `duplicate_key` and others), and `noncanonical_value` (equal values in other bytes). A missing `end` record, one whose counts disagree with the dump, and a record after it are `format` findings too. So are an `l0` value or pair section 4.2 does not allow (`unknown_value`, `l0_mismatch`, `bad_value`), an error kind the spec does not list, and, in the reference only, an `encode` L2 error (`reference_encode`) or a `dumper` file error (`reference_dumper`): the oracle's own failure on that object or input (spec sections 4.2, 4.3).
- A second record with a key already read is a `duplicate`; the first one is compared.
- A line of any depth and integer width is read, compared and quoted: a value as deep as UnityPyBoost reads and a packed version past Python's 4,300 digits are that record's differences or findings, never the run's end (spec section 6).

Records are matched by key, not position: `file` by path, `object` by path and `path_id`. The order is checked separately (`order`, `order_differs`; spec section 6). Every line is parsed and checked, identical ones included. Comparing the 20-bundle sample dump (38 MB, 12,880 records) with a second dump of the same inputs takes about 1 second.

## Self-tests

```sh
scripts/dev python -m unittest discover -s tools/parity/tests -v
```

| module | covers | needs |
|---|---|---|
| `test_canonical.py` | the JSON and value-encoding rules, with a golden line for Rust, and the single-line canonical-form checks, `l0`'s values and pairs and the error kinds among them | stdlib |
| `test_canonical_values.py` | which node types each member of a value: an `Array` or a `pair` of three children, a ReferencedObject's data named `type`, followed by a `type` child or left out, registries the reader skips, a value 30,000 levels deep | stdlib |
| `test_canonical_json.py` | lines written and read as `json` writes and reads them, at any depth and integer width | stdlib |
| `test_compare_depth.py` | `compare.py` on deep values, wide integers and members of any shape, and its walks against the recursions they replace | stdlib |
| `test_compare.py`, `test_compare_format.py`, `test_compare_cli.py`, with `compare_support.py` (not tests) | every finding kind on hand-built dumps, a module a topic: values (nested type mismatches at L1 and L2), records and the canonical form (duplicates, identical noncanonical lines), and the header and command line (level coverage, unreadable input, a reference whose producer is not the pinned stack (UnityPy, the Boost binary, the packages, the wheels' record digests), an `encode` error in the reference and in the candidate) | stdlib |
| `test_unitypy_boost.py` | synthetic typetrees through UnityPyBoost: float bits, NaN quieting, bool bytes, pairs, ReferencedObject and the registry scope, big-endian, every L2 error kind on both reading paths, the pure-Python path's tuples, 16-bit swap, registries in aligned class arrays, negative TypelessData lengths, and arrays of aligned vectors and TypelessData, version packing. It also checks that the spec's worked example is what the oracle writes, and that the loaded UnityPyBoost binary and its wheel's RECORD hash as `ORACLE_BOOST_SHA256`. | UnityPy |
| `test_dump.py` | the dumper end to end: UnityPy's samples, determinism across runs, `-j` and gzip, every line canonical, L0 detection quirks including gzip, UnityRaw formats 5 to 7, a failing entry, non-UTF-8 names; synthetic SerializedFiles (`tests/synth.py`) for the object table, check_read, resave and TPK wiring, version strings, type-record defaults by format, negative type indices, node rows and how blob rows become a tree, truncated blocks and zip entries; input names across locales and symlinks | UnityPy, samples |
| `test_dump_crashes.py` | crash records (a killed input, a death while it returns its records, a MemoryError anywhere in an input, UnityPyBoost crashes), finished results waiting in bounded number, the dumper's own failures costing their input alone (its encoder refusing UnityPy's value costing its object), an unreadable input stopping the run, records too big to send, load errors that quote bad bytes, the `None` leak | UnityPy |
| `test_dump_oracle.py` | the pinned-stack refusal (UnityPyBoost, its binary, UnityPy, a package such as numpy missing or at another version, a copy of UnityPy with one line patched, with or without its dist-info, imported first, or linked in as a package beside the module; a module from outside the pins loaded at import or while an input is dumped; PYTHONPATH or a user site) and the stack the header records | UnityPy |
| `test_dump_cli.py` | index mode (an input that is not the indexed file stops the run unless `--allow-index-mismatch`; an entry's `path` named, matched by `--name` and pinned by its bytes), `--levels`, the output and `--stats` guards (inside the repository, one file for two outputs, an output that is an input, a `.partial` file made as the run starts) | UnityPy, samples |
| `test_g3b.py` | `g3b.py`'s verdict, each admit checked on its input, against the oracle's real dump and a stand-in unityrs that answers each run from a table | UnityPy |
| `test_value_cases.py` | `value_cases.py`'s verdicts, and the cases file `unityrs values` reads | UnityPy |
| `test_child.py` | `child.py` and `scratch.py`: a child the system kills from outside is run again, never read as an answer, and a test's scratch is removed whatever happens to the test or its child; and the scans that hold every Python file in the repository to both | stdlib |
| `test_oracle_env.py` | the oracle's pins as `requirements-oracle.txt` gives them (numpy, cffi, pycparser, pycryptodome and zstandard included), `oracle_records.json` against them, and `oracle_env.py`'s checks on synthetic installs and wheels: versions, `--exact`, the RECORD digest (another build), an edited, missing or planted file, a symlink (a folder linked in beside a module, a listed file, the package folder, one in `__pycache__`, a dangling one), a copy earlier on the path, what the installer writes, `--pin-records`; and which loaded modules may come from where (the standard library, the tools, the pinned files) | stdlib |
| `test_fetch_samples.py` | the sample fetcher against a fake server: fetching, keeping and replacing files, LFS pointers, wrong and over-long bodies (read no further than one byte past the pin), retries and the overall deadline (on the fake; on loopback servers that trickle the status line, a header, a chunk's size line or the body, or stall a read; on a listener whose connects hang; and that a download cut off writes nothing), redirects (two loopback servers), the repository guard; pins with names that are not plain file names or URLs off the media host; a folder holding anything but the pinned samples as regular files (extra files, symlinks, folders, stale temporary files); the exclusively created temporary file; and that `unitypy_samples.json` pins the samples `test_dump.py` checks | stdlib |
| `test_inventory.py` | the inventory's matching rules (standard library): whole dotted chains, compact rows, enum values, a name under a wrong parent, what counts as an attribute; with the oracle, that `collect()` finds exactly the group sizes `PARITY.md` section 21 states, that `PARITY.md` names every UnityPy module, class, function, module-level table or setting, class member, attrs field, other attribute, package re-export, UnityPyBoost export or member, enum member with its value, and `TextureFormat`, and that removing any one entry, or writing it under a wrong parent, fails the check | stdlib; UnityPy for the rest |
| `test_texture_matrix.py`, with `texture_cells.py` and `texture_cases.py` (not tests) | the TextureFormat matrix of `PARITY.md` section 17: its columns, and its tier and phase rules, and section 17.1's table of Tier B references, one per Tier B row, with the values it states checked against `references.py` (standard library), and BC6H's reference against the two BC6H decoders in the oracle image, each with its one departure from the specification; with the oracle, every cell of every row, built from UnityPy (`CONV_TABLE`, the cited source lines, the result on real-sized data, the padding, Switch, Xbox and crunch gates observed through stand-ins, what `set_image` writes) and from the pixel cases, and a check that no builder reads `PARITY.md` (the rows built again in a new process from a copy with every cell changed are the same); the decoded pixels of every format that returns an image against a reference built from the format's layout (or, for a row marked wrong, the specific wrong pixels), also swizzled for Switch and byte-swapped for XBOX360 as the gates say | stdlib; UnityPy for the rest |
| `test_parity_rows.py`, `test_rows_*.py`, with `rows_support.py` and `export_support.py` (not tests) | the `PARITY.md` rows a review disputed, a module a topic, each run on the smallest synthetic input that shows it (UnityFS bundles, WebFiles and serialized files built by `synth.py`); each test names its rows, and each row's test column names the test back (`test_parity_rows.py`'s `TestColumn` checks both) | UnityPy |
| `head_oracle.py`, `head_417d595/` | not tests: UnityPy git `417d595`'s `helpers/CompressionHelper.py` and `files/BundleFile.py`, kept byte for byte with its MIT `LICENSE` (ruff skips the folder), and the loader that checks each file's git blob id and runs them inside UnityPy 1.25.2, for the HEAD-tier writing rows (`PARITY.md` section 20; `test_rows_writing.py` head_saves) | UnityPy |
| `test_object_mode.py` | `PARITY.md` section 9.2.1 holds exactly what `object_mode.py` prints, and section 9.2's extra-field rule holds for every class of `UnityPy.classes` | UnityPy with UnityPyBoost |
| `test_doc_tiers.py` | `PARITY.md` and `DESIGN.md` agree on what a tier covers where a zero side, an image mode, a missing characterisation, a walk past a UnityCN block, an empty first FSB5 subsound or a reader handed to a reader decides it | stdlib |
| `test_readme.py` | this README's two tables list every file in this folder and in `tests/`, and no other | stdlib |
| `test_fixture_modules.py` | each module under `tools/fixtures` (a file no step runs, which a fixture script imports) runs on its own, as plan section 10's G4 line runs every `tools/fixtures/*.py` with `--check`: it puts on its path what it imports | UnityPy |
| `test_meshdecode_tpk.py` | that `tools/fixtures/meshdecode.py --tpk` enforces its rule of no write to any path, a guard against accidental writes and leaks, not code written to evade it: the script's own audit hook (`NoWrites`), installed before it imports UnityPy and after only an allowlist of imports (checked on its syntax tree, and a `--tp` run exits 2), refuses every write, create, rename, remove, mode, owner, flag, time, temp-file, `sqlite3`, `syslog` and AF_UNIX socket event of its process and every audited spawn but the replay's one `subprocess.Popen` of the replay's argv in the checkout; its one exemption is child.run's scratch, the per-run folder and the root it makes when no parent names one, made, changed in mode and removed by path. A table of events checks `NoWrites` in this process, which never installs it: every listed event, the exemption's edges, the Popen's and an event it cannot read. The real runs pass under it: with a stub `cargo` first on `PATH` that takes the cases on standard input and checks the replay's argv, working folder, scratch and cleared verbose flag, with no build (that run without `-B`, writing no bytecode of the repository), with `--no-cargo`, and with no parent scratch; the stub's `skip`, `short`, `fail`, `exit` and `nook` modes each fail the run, a stub that SIGKILLs itself fails it as killed and is not run again, and a `cargo` that cannot be executed fails it as one that could not run; and each planted write or spawn (`UNITYRS_MESHDECODE_TPK_PLANT`: an `atexit` write, a non-daemon thread's, `sqlite3`, `os.setxattr` on Linux, a `TemporaryFile`, `target/`, a raw `os.open`, a second `Popen`, and a caught refusal with standard error closed) fails it and leaves no file. Every run starts from an empty working folder with empty `TMPDIR` and `UNITYRS_TEST_SCRATCH`, all three empty after, the core-dump limit 0 and `git status --porcelain -uall --ignored` (`target/` aside) unchanged, the backstop for the native writes and the replay's process the hook cannot see; the replay's source, `crates/unityrs-meshdecode/tests/oracle.rs`, is held to an allowlist of paths; and a failure prints counts and the script's own fixed lines only, never a TPK-derived value or a path the runs made, as CI's log is public | UnityPy (the oracle image) |
| `test_env_race_held.py` | `tools/fixtures/env_race.py`'s `held_case` records a scene exactly when UnityPy's run with the held load's registrations made last and its run in load order leave the same `env.cabs`, as the latter, and refuses every other (run in a child, since `env_race` is not a parity tool) | UnityPy |
| `test_pillow_tables.py` | `PARITY.md` sections 15.8 and 18.1, and section 1's row for the libraries the Pillow writers call, hold exactly what `pillow_tables.py` prints; that row holds nothing the host decides | UnityPy, Pillow 9.5.0 |
| `test_ngui_cutter.py` | the NGUI cutter's reference on the testkit's NGUI scenes, against expectations worked out by hand: outputs and units, skips in record order, found sheets, whole JSON texts and the key rules | UnityPy, Pillow 9.5.0, cargo |
| `test_ngui_pixels.py` | every image the reference makes on those scenes (each crop and each texture), held to pixels written by hand from the scenes' builders and the testkit's pixel rule | UnityPy, Pillow 9.5.0, cargo |
| `test_ngui_read.py` | the reference's Environment sealed against files in the working directory and at the root, its pointer and stream reasons where UnityPy's own path answers otherwise, and the bounds it decides before Pillow (the padded `texture_pixels`, the raw decoder's cap, the crop's) | UnityPy, Pillow 9.5.0, cargo |
| `test_ngui_winners.py` | the reference's winners on the winners scenes, each key's decision written by hand: every rule of E31, units and their conflicts, a losing unit's keys (`unit_lost`), E61's withholding, `publish_incomplete` and a unit keyed by its sheet's texture | UnityPy, Pillow 9.5.0, cargo |
| `test_sprites_kind.py` | the `sprites` kind's reference on its own branches: a top-level WebFile row's `Divergence`, a `MemoryError` that is Pillow's (`pillow_memory_error`) and one that is not (propagated), the render data's selection on objects made in the test (the first atlas of the tag's name, names read only up to it, no match, a missing key, no tag) and a Sprite's fields of another shape | UnityPy, Pillow 9.5.0 |
| `test_texture_seal.py` | every Texture2D either reference constructs, nested or read directly, refuses an `image` child without opening a host path: the sprite_attributes scene's unused path-valued textures under a texture, a Sprite and an atlas, each parent still exported, and `PIL.Image.open` never called while the references run | UnityPy, Pillow 9.5.0 |
| `synth.py`, `dump_harness.py` | not tests: `synth.py` builds the synthetic inputs, and `dump_harness.py` holds what the `test_dump*.py` modules share (the sample pins included) | stdlib; UnityPy for `dump_harness.py`'s dumps |
| `test_gltf_check.py` | `gltf_check.py` on hand-built GLBs: the orientation verdict (pass, a reversed winding, no normals, degenerate triangles), accessor reads (sparse, stride and offset, ranges, raw values), the UV, tangent, normal and weight conversions, morph deltas left unnormalised, and the attributes an invalid normal or tangent must omit | stdlib |

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
3. It runs this folder's self-tests and the LZMA spike's script tests (`crates/unityrs-bench-lzma`) with `$UNITYRS_UNITYPY_SAMPLES` set, through `scripts/unittest-suites`. With the stack and the samples in place, a skipped test fails the job, and so does a suite that runs fewer tests than the minimum the workflow names for it. (`scripts/tests` runs in the `scripts-py` job, in a venv holding `scripts/requirements-tests.txt`: its workflow test parses the workflow with PyYAML, which is no part of the oracle.)
4. With the pinned Rust toolchain, it checks the generated tables (`tools/gen/gen_tables.py --check`), G7 (`tools/fixtures/tpk.py --check`) and that every other committed oracle fixture is current (the other `tools/fixtures/*.py --check` the workflow lists: every file there run as a program, the rest being modules a listed script imports, which `scripts/tests/test_ci_workflow.py` checks). `tools/fixtures/imageio.py` (`unityrs-imageio`'s X3 and X4) has a step of its own, which builds its own zlib 1.2.13, fetched by its release's versioned URL and verified against the crate's vendor manifest (`scripts/zlib_vendor.py verify`), and loads it under `LD_LIBRARY_PATH` within that step, so Pillow's PNG bytes are compared too.
5. It builds `unityrs` and `write_matrix` (release), then runs the gate's rows that need no corpus (`docs/plans/core.md` section 6): UnityPy's samples dumped by `unitypy_dump.py` and by `unityrs dump`, compared with `compare.py --oracle`; G2, `value_cases.py` through `unityrs values`; and G3, the synthetic matrix `write_matrix` writes, dumped by both and compared with `compare.py --oracle`. Each step fails the job on any difference.

**Re-pinning the oracle stack** (DESIGN.md section 3). `requirements-oracle.txt` names one wheel per package: the manylinux x86_64 wheel the oracle image holds, cp311 or, for pycryptodome, abi3 (`cp37-abi3`). A re-pin changes the image and this file together:
1. For each package, find that wheel on PyPI (`https://pypi.org/pypi/NAME/VERSION/json`, the file whose name matches the tags in the image's `*.dist-info/WHEEL`). Its sha256 (`digests`) goes into `requirements-oracle.txt`, and the file itself (`url`) into a folder DIR outside the repository.
2. `python tools/parity/oracle_env.py --pin-records DIR` checks that DIR holds exactly the pinned wheels and writes `oracle_records.json` from their RECORDs.
3. Install the wheels into a fresh venv (`python -m pip install --require-hashes -r tools/parity/requirements-oracle.txt`) and run `pip check`. In that venv, `python tools/parity/oracle_env.py --exact` checks that it holds the pinned wheels' files and nothing else.
4. In the image, `python tools/parity/oracle_env.py --image` checks that it holds the same wheels' files and loads the zlib `PARITY.md` section 1 names (`ORACLE_IMAGE_ZLIB` in `oracle_env.py`, the reference for Pillow's PNG, ICO and ICNS bytes): a new base image with another zlib changes that constant and the docs that name it, or is not the oracle. The image holds other packages too: dump the samples and a corpus sample there with `unitypy_dump.py`, which refuses to run, naming the module, when UnityPy or a pin loads one from another package. Pin any package it names, with its dependencies, and repeat from step 1.
5. When UnityPy moves, set `ORACLE_BOOST_SHA256` in `oracle_stack.py` to the sha256 of the new wheel's UnityPyBoost binary (its RECORD entry). `ORACLE_UNITYPY` follows the pins.

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
