# Native encoders

`etcpak/` contains 14 unchanged encoder files from the PyPI source distribution
etcpak 0.9.14; `astcenc/` contains the 32 unchanged core files from
astc_encoder_py 0.1.12. The `.sha256` manifests pin both compressed archives
and every selected archive member. Their comments give the versioned download
URLs. Verify a downloaded archive with:

```sh
python3 scripts/etcpak_vendor.py verify etcpak-0.9.14.tar.gz
python3 scripts/astcenc_vendor.py verify astc_encoder_py-0.1.12.tar.gz
```

These commands check each source against the original archive and reject extra,
missing or changed local files. They never extract the archive. CI runs them on
Linux x86-64, Linux Arm64 and macOS alongside the `encode` feature's tests.

The Python distributions omit the underlying native projects' licence files.
Separate `.notices.sha256` manifests pin the retained crate notices and identify
their origins. `LICENSE-ETCPAK` comes from the pinned upstream etcpak revision;
`LICENSE-BC7ENC` retains the MIT option from the archived source's dual notice.
The existing `LICENSE-ASTCENC` carries Arm's copyright and Apache-2.0 text. No
Python binding source, bcdec decoder or command-line frontend is included.

The build selects scalar code consistently across architectures and disables
floating-point contraction and fast math. Only ASTC copies under `OUT_DIR` are
adapted for Rust allocation callbacks, one-time table initialization and a local
configuration preset selector safe for concurrent calls; patch
sites assert their expected counts. The pristine vendor directories are never
rewritten by the build.
