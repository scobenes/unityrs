# unityrs Python bindings

Y1 provides local-path and byte loading, eager files and object discovery,
`ObjectReader` dict reads and raw bytes. Y2 adds object mode and runtime classes. It uses PyO3 0.27.2's Python 3.10+
abi3 interface and maturin 1.9.6. PyO3's MIT/Apache-2.0 licence is already
accepted by the repository. The wheel includes `THIRD_PARTY_LICENSES`.

From the repository root, with maturin installed in a build venv:

```sh
maturin build --locked --release --manifest-path crates/unityrs-py/Cargo.toml --out /tmp/unityrs-wheels
python -m pip install --no-deps /tmp/unityrs-wheels/*.whl
python -m unittest discover -s crates/unityrs-py/python/tests
```

Build a Linux wheel in the oracle container to test there; a macOS wheel cannot
be installed in Linux. In an oracle venv with UnityPy 1.25.2 available, install
the wheel, then run:

```sh
python tools/fixtures/py_bindings.py --check
cd tools/parity
python -m unittest bindings.test_y1 bindings.test_y2
```

The recorder writes 34 Y1 synthetic API observations and 66 Y2 object-mode
observations, including exception text. Object recordings select only fields
from the synthetic input, never generated defaults or a TPK schema.
`cargo test -p unityrs-py`
replays them through the Python facade. The object replay needs a TPK; it
discovers the oracle installation without importing UnityPy. The binding suite
compares the installed wheel directly with UnityPy and fails if either import
is missing. The ordinary oracle suite does not discover this separate suite.

```python
import unityrs

budget = unityrs.Budget(256 * 1024 * 1024)
limits = unityrs.Limits(max_input_bytes=64 * 1024 * 1024)
env = unityrs.Environment("input.assets", budget=budget, limits=limits)
for obj in env.objects:
    value = obj.parse_as_dict()
```

`Budget.used` is a Rust atomic counter. Limits and budgets apply to retained
Rust storage and conversion staging; existing Python input buffers are retained
without a Rust copy, and Python owns returned values after handoff. Every core
operation releases the GIL; parser work runs on a scoped worker using the core's
advertised value stack allowance plus container and tree parsing frames,
including when a Python thread has a small stack. Native container destruction
is iterative on the releasing thread, including wrapper failure cleanup.
The already charged entry tables form its work list: teardown allocates no
storage and creates no workers, even with a full budget or refused allocator.
Conversion uses an explicit metered work stack. Files and objects retain strong
parent links, and a weak wrapper cache preserves identity without retaining
replaced files forever. An environment lock serializes loads and wrapper
publication. Stream reads accumulate bounded chunks up to `max_input_bytes + 1` bytes, restore
the original position, and refuse oversized input before parsing. ZIP members
are decoded by the metered Rust loader for paths, bytes and streams.

No core call may be in flight in another thread at `fork()`. Use `spawn` or
`forkserver`, or fork before reading. There is no child-side lock reset.

Helper types, full version and enum APIs, explicit typetree nodes, config and
container helpers are Y3; image/export and write APIs are Y4; extractor and
remaining re-exports are Y5. Classes use the MIT-licensed attrs dependency for
UnityPy-compatible slots, constructors, equality and hashing. No TPK is embedded. Nonlocal
filesystems are not part of this local-input scaffold.


Importing `unityrs.classes` generates records in memory. Set `UNITYRS_TPK` to a
TPK file path before importing it. If Y3's `unityrs.config` is present, its
`TPK` attribute accepts a path or bytes and takes precedence. Without an
explicit source the loader discovers the installed UnityPy resource without
importing UnityPy. If none is available, importing classes raises `ImportError`
with those source options; ordinary dict reading remains available.
Uncompressed, LZ4 and Unity raw-LZMA TPKs are accepted. Generator inputs,
decoded payloads and decoder dictionaries are bounded at 256 MiB, tables at
four million records (class entries in aggregate). Invalid generated syntax
is rejected before execution. Python allocation failures raise `MemoryError`.

`reader.read()`, `reader.parse_as_object()` and
`reader.read_typetree(as_dict=False)` return objects; UnityPy's `wrap=True`
spelling is also supported. PPtrs retain their assets file and resolve through
its parent files and environment. Unknown records retain their node metadata;
Y3's `helpers.TypeTreeNode` is used when available. The private metadata carrier
before Y3 provides the node fields needed by `UnknownObject.get_type()`.
The core still parses detached from the GIL. Python builds objects with an
explicit work stack from the metered conversion, whose returned objects belong
to Python's allocator as in dict mode.

Generation follows HEAD tools/TpkClassGenerator.py:132-306 and
HEAD tools/TpkClassGenerator.py:309-367, preserved and blob-pinned under
`tools/parity/tests/head_417d595/`. Each release root contributes fields to its
named class; nested records merge by type name, node IDs deduplicate definitions,
field types form sorted unions, and the intersection of fields across shapes
is required. Other fields use `Optional[...] = None`. Required fields precede
optional ones, each sorted by its cleaned name. Bases follow class metadata,
classes without release roots add ABC, dependency order puts bases first, and
math records replace their TPK definitions. The decorator temporarily replaces
nontrivial bases when applying attrs (classes/generated.py:26-44).

The binding suite compares the entire generated class set, annotations,
field order, defaults, slots and class-ID map against 1.25.2 at test time.
It also checks every generated class with complete, missing and extra fields,
pointer resolution, explicit TPK sources, and malformed input. With the oracle's
TPK the HEAD algorithm reproduces the installed 1.25.2 introspection exactly;
no class/field list or ID map is persisted. The class map first uses the MIT
enum spelling, then current TPK metadata and legacy spelling normalization.

Object construction follows UnityPyBoost/TypeTreeHelper.cpp:458-583; zero-byte
objects follow helpers/TypeTreeHelper.py:233-258 and
helpers/TypeTreeHelper.py:314-357. Ports of Object, PPtr, UnknownObject and math
retain the original MIT notices via `LICENSE-UNITYPY` and `THIRD_PARTY_LICENSES`.
