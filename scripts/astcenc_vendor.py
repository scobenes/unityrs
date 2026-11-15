#!/usr/bin/env python3
"""Verify astcenc's pristine encoder sources against astc_encoder_py 0.1.12's sdist.

    python scripts/astcenc_vendor.py verify astc_encoder_py-0.1.12.tar.gz
"""
import encoder_vendor


if __name__ == "__main__":
    raise SystemExit(encoder_vendor.main(
        "astc_encoder_py-0.1.12.tar.gz", "astc_encoder_py-0.1.12/src/astc-encoder/Source/", "astcenc"))
