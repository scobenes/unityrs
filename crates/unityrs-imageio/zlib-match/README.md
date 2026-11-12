# The compiled longest_match

The vendored zlib 1.2.13 files stay pristine. `build.rs` verifies the SHA-256
of the original non-FASTEST `longest_match` span, then substitutes
`longest_match.c` in an altered `deflate.c` under `OUT_DIR`. It compiles the
deflate side at optimisation level 3. Both the replacement and the derived
file mark the alteration, as the Zlib licence requires.

The replacement follows the original byte path's four quick checks, using
two 16-bit `memcpy` loads. It finds the first mismatch from offset 3, with
the original hash argument for assuming offset 2 equal, and caps the length
at 258. Candidate order, chain budget, strict improvement, `match_start`,
`nice_match` and the final lookahead clamp are unchanged. On little-endian
GCC/Clang targets it compares eight bytes using `memcpy`, XOR and `ctzll`
(only for nonzero XOR). The last word spans offsets 251 through 258, within
the original loop's bounds; the result is capped at 258. Big-endian or
unknown byte order and compilers without that
builtin use the byte loop for the entire comparison. No unaligned typed
dereferences are used. `UNITYRS_MATCH_BYTEWISE` forces the fallback for
verification.

The optional `test-reference` feature compiles `reference.c`, which includes
pristine vendored `deflate.c` under `unityrs_zref_` exported names and shares
the main build's trees and checksum objects. It exposes only a hidden Rust
test helper, `test_reference::assert_deflate_equal`; normal builds contain
neither the reference object nor its Rust interface. The symbol test checks
both configurations, including the absence of reference names by default.

Run the byte comparisons at Pillow's settings:

```sh
cargo test -p unityrs-imageio --features test-reference
CFLAGS=-DUNITYRS_MATCH_BYTEWISE cargo test -p unityrs-imageio --features test-reference --test zlib_match
```

`tests/zlib_match.rs` covers random, zero, periodic and run data, mismatch
offsets through MAX_MATCH, synthetic PNG-filtered rows, repeated window
slides and multi-megabyte streams, with varied input and output windows.
`tests/pillow_png.rs` continues to compare complete PNGs with the pinned
Pillow fixtures. CI runs both word and forced byte comparisons. The fuzz
workspace's `zlib_match` target runs the same reference comparison on
arbitrary inputs (capped at 4 MiB).
