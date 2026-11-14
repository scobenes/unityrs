# OBJ, renderer and number-format oracle fixtures

`tools/fixtures/pyfmt.py` records CPython `.9G` / `.4f` over random binary64
bits, ties, exponent boundaries, signed zero, subnormals and special values.
`pyfmt.json` is present; CI checks it again in the pinned oracle environment.

Run `python tools/fixtures/obj_export.py` in the pinned UnityPy 1.25.2 /
Pillow 9.5.0 environment to regenerate the checked-in `obj.json` and
`renderer.json`, then run it with `--check`. The Rust replays compare those
recordings; CI's fixture check requires them to be current. Expected results
come from the oracle run.

The recorder obtains synthetic serialized inputs from `export_inputs`, built
with unityrs-testkit's hand-built embedded trees. It records raw OBJ bytes
(including surrogateescaped names), False and exceptions. Renderer results
include files and standalone material text from both the original source and
a corrected source compiled in memory; the Rust replay pins the exact
expected divergence mask. Corrections are PARITY section 19.1's optional
batch fields, material alignment, lowercase colors and texture dict iteration.
PNG saving walks the original property list, preserving duplicate entries;
MTL maps use the first-key order and last value of the property dictionary.
Renderer recordings use a real temporary directory on UnityPy's default local
filesystem so Pillow can infer PNG from each file's name. Files are recorded
in sorted relative-name order. The recorder asserts the generated Python
classes of every object before export, rejecting an `UnknownObject` fallback.

The renderer uses the formats already decoded by unityrs-texture. Xbox
byte-swapped formats are swapped on a charged copy. Unsupported formats and
swizzled textures return an error instead of producing an incorrect PNG.
Raw mesh/material byte sinks retain names; renderer text files require UTF-8.
