#!/usr/bin/env python3
"""Verify the pinned libwebp encoder subset against its release tarball.

    python scripts/webp_vendor.py verify libwebp-1.3.0.tar.gz

The tarball SHA-256 and every pristine member are pinned in the manifest.
Reuses zlib's verifier with explicit WebP pins; no files are extracted.
Only vendor/libwebp-1.3.0 is pristine. BSD-3-Clause upstream failure-path
patches live separately in vendor/patches (see vendor/WEBP.md); build_webp.rs
applies them exactly to an OUT_DIR copy after checking the same file pins.
This verifier intentionally compares the original sources, not that copy.
"""
import zlib_vendor

# The verifier is shared with zlib; scope its pins to this invocation.
def main():
    previous = (zlib_vendor.MANIFEST, zlib_vendor.TARBALL, zlib_vendor.PREFIX)
    try:
        zlib_vendor.MANIFEST = "libwebp-1.3.0.sha256"
        zlib_vendor.TARBALL = "libwebp-1.3.0.tar.gz"
        zlib_vendor.PREFIX = "libwebp-1.3.0/"
        return zlib_vendor.main()
    finally:
        zlib_vendor.MANIFEST, zlib_vendor.TARBALL, zlib_vendor.PREFIX = previous


if __name__ == "__main__":
    raise SystemExit(main())
