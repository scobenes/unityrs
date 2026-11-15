# libwebp 1.3.0 encoder

Source: the release tarball `libwebp-1.3.0.tar.gz`, SHA-256
`64ac4614db292ae8c5aa26de0295bf1623dbb3985054cb656c55e67431def17c`,
from the WebM project's release storage. `libwebp-1.3.0.sha256` pins the
pristine encoder translation units, their transitive headers, and COPYING,
PATENTS, AUTHORS and README.md. The lossless encoder is required to encode
lossy WebP's alpha plane; no decoder translation unit or animation/mux code
is compiled. Decoder headers and DSP reconstruction routines are required
by the encoder. No vendored source is edited.

`build_webp.rs` checks every file's hash, copies the pinned subset into
`OUT_DIR/libwebp-1.3.0`, and applies the reviewed single-file unified diffs in
`patches/` before compiling that copy through the existing `cc` dependency.
`build_webp_patch.rs` requires exact old and new line numbers, context, and
hunk lengths; any mismatch fails the build. There is no offset, fuzz or
reverse application, and no external patch tool is needed. Cargo tracks the
patch files and applier as build inputs. Every build starts from the pristine
copy, so rebuilds cannot accumulate patches. `scripts/webp_vendor.py verify`
continues to check the untouched vendor directory against the same tarball;
the patches and derived sources are outside that directory.

## Failure-path patches (BSD-3-Clause)

Both patches reproduce upstream source changes released in **libwebp 1.3.1**,
with provenance headers and unchanged hunk contents. They affect allocation
failure handling only; the 576-image Pillow replay remains unchanged.

- `patches/0001-alpha-initialize-bitwriter.patch`: James Zern's
  [a486d800b60d0af4cc0836bf7ed8f21e12974129](https://github.com/webmproject/libwebp/commit/a486d800b60d0af4cc0836bf7ed8f21e12974129),
  “EncodeAlphaInternal: clear result->bw on error”. Clears the result
  bitwriter on an early lossless-alpha failure before trial cleanup can free
  it, and checks the final bitwriter initialization. Regression:
  `alpha_trial_early_failure_has_an_initialized_bitwriter`, 31x33 RGBA
  gradient, default method 4, refusal 8.
- `patches/0002-palette-free-cooccurrence.patch`: James Zern's
  [0edbb6ea7176eca19955be701178fa70ecb497ee](https://github.com/webmproject/libwebp/commit/0edbb6ea7176eca19955be701178fa70ecb497ee),
  “PaletteSortModifiedZeng: fix leak on error”. Frees the co-occurrence
  matrix when its line-buffer allocation fails. Regression:
  `palette_cooccurrence_failure_frees_the_matrix`, 17x17 RGBA deterministic
  noise, method 6, refusal 180 (119,732 bytes before the fix).

The original COPYING, PATENTS and AUTHORS remain packaged unchanged. These
upstream fixes use the existing BSD-3-Clause licence; no new licence applies.

The forced headers redirect malloc/calloc/free
and prefix every generic encoder global (including DSP function pointers and
tables). Hidden visibility also isolates shared linkage. The archive test
rejects any unprefixed definition or reference to a C allocator. SIMD selection
is disabled with HAVE_CONFIG_H and an empty generated configuration; this
uses libwebp's generic C path on both arm64 and amd64. Thread-safe DSP
initialization is enabled, but encoding uses Pillow's default thread_level=0.
No encoding worker can allocate outside the calling thread's allowance.

Pillow 9.5.0's `src/PIL/WebPImagePlugin.py` `_save` and
`src/_webp.c` `WebPEncode_wrapper` are the reference. L becomes RGB and LA
becomes RGBA, preserving alpha. The picture imports packed RGB/RGBA with
use_argb=1 (the shim imports RGBA through byte reads into the aligned ARGB
picture, avoiding libwebp's unaligned uint32_t input load), then encodes lossy at quality 80, method 4, exact false by default.
Alpha quality remains 100, compression 1 and filtering 1. No metadata is
present on extractor images; their emitted RIFF already equals Pillow's
subsequent no-metadata mux assembly. `pillow_webp.rs` checks whole bytes or
length plus SHA-256 for 576 Pillow files, at quality 80 and 95 and exact
false/true. Five dimension refusals and five option refusals also pin
Pillow's validation order (configuration before picture dimensions).
This is a still-image interface; animation, metadata and production lossless
output are outside W1. The `test-hooks` feature exposes a hidden lossless
test entry point matching Pillow's `config.lossless=1`, using the same import,
writer and allocator with the larger workspace allowance.

## Memory charge and its derivation

Before any allocation, `webp_plan` offers two requests to the caller:

- workspace: **2 MiB + 256 bytes per input pixel**;
- output: **1 MiB + 8 bytes per input pixel**, a single fallible Vec allocation.

The first includes L/LA conversion (at most four bytes per pixel), C payloads,
16-byte allocation headers, and an extra 48 bytes per live C block for heap
rounding/metadata. Every C allocation checks this remaining allowance before
calling Rust's global allocator. Conversion capacity is subtracted before
entering C. An exhausted allowance returns NULL and a typed `Allowance` error;
a host allocator refusal returns `Memory`. The allocator records every refusal
independently of libwebp's error_code (some failure paths leave it at OK),
so no tolerated refusal can return success or publish an empty/altered file. Output writes cannot pass the
separately charged capacity. Thus an adversarial input cannot exceed the
charge even if its memory use differs from the measurement set. All sizes use
checked arithmetic; sides outside 1..=16383 are refused before charging.

`webp_charges::webp_allocations_are_charged_fallible_and_freed` measures the
complete requested heap, including mode conversion and the output, under the
testkit counting global allocator. It tests every mode at 1x1, 15x17,
257x129, 1024x1024, 16383x1 and 1x16383, using deterministic noise. Native
arm64 measurements (bytes, quality 80/method 4):

| extent | largest peak across modes | output capacity | workspace peak upper bound after subtracting output |
|---|---:|---:|---:|
| 1x1 | 1,118,659 | 1,048,584 | 70,075 |
| 15x17 | 2,137,005 | 1,050,616 | 1,086,389 |
| 257x129 | 3,214,829 | 1,313,800 | 1,901,029 |
| 1024x1024 | 60,206,882 | 9,437,184 | 50,769,698 |
| 16383x1 | 6,487,815 | 1,179,640 | 5,308,175 |
| 1x16383 | 6,446,895 | 1,179,640 | 5,267,255 |

The 2 MiB intercept exceeds the small-image workspace peaks. Subtracting
that intercept leaves at most 196.02 bytes per pixel in the thin-image case,
versus 46.42 for the megapixel case. Rounding the slope up to 256 leaves room
for allocation headers/heap costs and encoder variation; the allocator
actively enforces it rather than trusting this sample to prove a universal
bound. The output's eight bytes per pixel plus 1 MiB is also enforced.
The same test checks charge refusals allocate nothing, checks that no
allocation ever exceeds held charges, and checks every buffer is freed before
its charge. `every_allocation_refusal_is_clean` sweeps every allocation
ordinal of a successful encode for six small inputs: 31x33 RGBA gradient,
17x17 RGBA noise, 15x17 RGBA alpha endpoints, 17x17 RGB palette,
15x17 LA gradient and 15x17 L noise. Each runs lossy methods 0 through 6
at quality 80; with `--features test-hooks`, every case also runs lossless.
Each refusal must be reached, return `Memory`, leave zero live heap bytes
above baseline and zero held charge, and free buffers before releasing their
charge. The two reported failures also have individual regression tests.
`every_allocation_refusal_at_quality_boundaries_is_clean` repeats the matrix
at quality 0/95 and quality 100 with exact=true. It is gated out of debug
runs and runs in CI's existing scale job with `--profile sweep` and
`--features test-hooks`.

Measured container sweeps (sweep time, excluding compilation): the default
matrix with test hooks covers **84 configurations and 7,549 refusals**, taking
32.55 s on arm64 and 46.70 s on amd64. The release quality-boundary matrix
covers **252 configurations and 24,038 refusals**, taking 24.82 s on arm64
and 24.66 s on amd64.
All return `Memory` with zero remaining live bytes and held charges; no
additional failure-path defects were found in these matrices.

Library callers selecting method 5 or 6 use `webp_plan_with_options` and a
**32 MiB + 256 bytes/pixel** workspace. The batch option always uses method
4 and retains the default charge above. A counting-allocator probe at
methods 0/6, qualities 80/95/100 and exact true measured method 6 workspace
peaks of 19,292,631 bytes at 1x16383, 20,009,420 at 2x16383,
20,214,152 at 16383x2, and 59,241,906 at 1024x1024, after subtracting the
output allocation. Its finer alpha histograms exceed the default constant
on thin images; the intercept rounds up to 32 MiB, with the same enforced
per-pixel allowance. `higher_methods_plan_their_larger_alpha_workspace`
checks the configured plan and encodes thin alpha-bearing images under it.
