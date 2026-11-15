# unityrs Python bindings

The bindings provide local-path and byte loading, eager files and object discovery,
`ObjectReader` dict reads and raw bytes. Y3 adds mutable `TypeTreeNode` helpers,
explicit-node dict reads, `peek_name`, `ContainerHelper`, mutable loader config,
`UnityVersion` and the handwritten enums and lookup tables. It uses PyO3 0.29.3's Python 3.10+
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
python -m unittest bindings.test_y1 bindings.test_y3
```

The recorder writes 44 synthetic API records (34 Y1 records and 10 Y3 groups), including exception text,
all enum member names and values, version edge cases and mutable helper behavior.
`cargo test -p unityrs-py`
replays them through the Python facade without UnityPy. The binding suite
compares the installed wheel directly with UnityPy and fails if either import
is missing. The ordinary oracle suite does not discover this separate suite.
The Y3 replay is `replays_unitypy_helper_observations`; its installed-wheel suite
also compares runtime common strings directly, without saving TPK-derived data.
The wheel suite has 25 tests and the oracle comparison has 13; CI enforces both
minimum counts. Warning handlers run on the calling Python thread before a
fallback load is published, including handlers that re-enter the environment.

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

Type-tree helpers use one charged traversal policy for parsing, list conversion,
graph walks and output. Helpers accept optional `limits=` and `budget=`; nodes
reconstructed from an object inherit its input's meter and depth limit. Every
repeated edge spends work and output memory again, cycles raise `ValueError`,
and staging charges stay live through reconstruction or output handoff. Lazy
node lookup and publication use the environment's reentrant lock.

No core call may be in flight in another thread at `fork()`. Use `spawn` or
`forkserver`, or fork before reading. There is no child-side lock reset.

Object mode and generated classes remain Y2; image/export and write APIs are Y4;
extractor and remaining re-exports are Y5. The bindings have no UnityPy or Pillow
runtime dependency and bundle no TPK or generated classes. Set `UNITYRS_TPK` to
an external TPK for stripped-file schemas and the complete common-string table.
When the variable is unset, the loader can locate an installed UnityPy package's
`resources/lzma.tpk` without importing UnityPy. With neither source available,
embedded trees and explicit nodes remain usable, and common-string lookup uses
the 108-entry MIT table. No TPK-derived data is saved in the wheel or replay
fixtures. Nonlocal filesystems remain outside this local-input API.
