`accessors.json` records UnityPy 1.25.2 through
`tools/fixtures/mesh_export.py`; `tests/accessors_oracle.rs` replays its
accessor values and both clean-name collisions from their raw trees and bytes.
To check the recording, run `python tools/fixtures/mesh_export.py --check`
in the pinned oracle container.

The input is synthetic class/component data. Empty and legacy-pair cases
retain UnityPy's exceptions beside the Tier B dict answers. No TPK-derived
schema or bytes are stored.
