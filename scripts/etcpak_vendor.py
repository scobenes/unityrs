#!/usr/bin/env python3
"""Verify etcpak 0.9.14's pristine encoder sources against its pinned PyPI sdist.

    python scripts/etcpak_vendor.py verify etcpak-0.9.14.tar.gz
"""
import encoder_vendor


if __name__ == "__main__":
    raise SystemExit(encoder_vendor.main("etcpak-0.9.14.tar.gz", "etcpak-0.9.14/src/etcpak/", "etcpak"))
