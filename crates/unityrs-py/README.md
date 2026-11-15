# unityrs Python bindings

Y1 provides local-path and byte loading, eager files and object discovery,
`ObjectReader` dict reads and raw bytes. It uses PyO3 0.27.2's Python 3.10+
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
python -m unittest bindings.test_y1
```

The recorder writes 34 synthetic API observations, including exception text.
`cargo test -p unityrs-py`
replays them through the Python facade without UnityPy. The binding suite
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

Object mode and generated classes are Y2; helper types, full version and enum
APIs, explicit typetree nodes and container helpers are Y3; image/export and
write APIs are Y4; extractor and remaining re-exports are Y5. This scaffold
has no UnityPy or Pillow runtime dependency and does not embed a TPK. Nonlocal
filesystems are not part of this local-input scaffold.
