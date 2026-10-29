"""Fetch UnityPy's test samples, at the pinned commit, into a folder outside the repository.

    python tools/parity/fetch_unitypy_samples.py DEST
    UNITYRS_UNITYPY_SAMPLES=DEST python -m unittest discover -s tools/parity/tests

unitypy_samples.json pins UnityPy's commit and the size and sha256 of each
sample; the sha256 is also the file's Git LFS oid at that commit. The samples
are Git LFS objects, and raw.githubusercontent.com serves the 130-byte LFS
pointer in their place, so they come from media.githubusercontent.com and a
pointer is refused. A file already in DEST with the pinned size and sha256 is
kept. Anything else is downloaded again, checked, and only then written.

The samples are game bundles with no licence of their own. They are fetched
where they are needed and never copied into this repository, so DEST may not
lie inside it (symlinks followed).

Exit status: 0 when DEST holds every pinned sample; 1 when one could not be
fetched or does not match its pin; 2 for a usage error. Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import canonical as C  # noqa: E402

PINS = HERE / "unitypy_samples.json"
LFS_POINTER = b"version https://git-lfs.github.com/spec/"
ATTEMPTS = 3
TIMEOUT_S = 60
CHUNK = 1 << 20


class FetchError(Exception):
    """A sample that could not be downloaded, or that does not match its pin."""


def load_pins(path: Path = PINS) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def sample_url(pins: dict, name: str) -> str:
    return pins["url"].format(commit=pins["commit"], name=name)


def mismatch(data: bytes, pin: dict) -> str | None:
    """Why `data` is not the pinned sample, or None when it is."""
    if data.startswith(LFS_POINTER):
        return "a Git LFS pointer, not the sample"
    if len(data) != pin["bytes"]:
        return f"{len(data)} bytes, not the pinned {pin['bytes']}"
    digest = hashlib.sha256(data).hexdigest()
    if digest != pin["sha256"]:
        return f"sha256 {digest}, not the pinned {pin['sha256']}"
    return None


def present(path: Path, pin: dict) -> bool:
    """Whether `path` already holds the pinned sample. Its size is checked before it is read."""
    try:
        if not path.is_file() or path.stat().st_size != pin["bytes"]:
            return False
        return mismatch(path.read_bytes(), pin) is None
    except OSError:
        return False


def download(url: str, limit: int, opener, sleep) -> bytes:
    """The body at `url`, read up to `limit` + 1 bytes (enough to tell it is too long).

    A network error is retried, up to ATTEMPTS tries in all. A body that
    arrives is returned whatever it holds, for the caller to check.
    """
    for attempt in range(1, ATTEMPTS + 1):
        try:
            with opener(url, timeout=TIMEOUT_S) as response:
                chunks, total = [], 0
                while total <= limit:
                    chunk = response.read(min(CHUNK, limit + 1 - total))
                    if not chunk:
                        break
                    chunks.append(chunk)
                    total += len(chunk)
                return b"".join(chunks)
        except (OSError, http.client.HTTPException) as e:
            if attempt == ATTEMPTS:
                raise FetchError(f"{url}: {e} (after {ATTEMPTS} attempts)") from e
            sleep(2**attempt)
    raise AssertionError("unreachable")


def fetch(pin: dict, url: str, target: Path, opener, sleep) -> None:
    data = download(url, pin["bytes"], opener, sleep)
    why = mismatch(data, pin)
    if why:
        raise FetchError(f"{url}: {why}")
    tmp = target.with_name(target.name + ".partial")
    try:
        tmp.write_bytes(data)
        os.replace(tmp, target)
    finally:
        tmp.unlink(missing_ok=True)


def main(argv=None, *, opener=urllib.request.urlopen, sleep=time.sleep, pins=None) -> int:
    ap = argparse.ArgumentParser(description="Fetch UnityPy's pinned test samples into DEST.")
    ap.add_argument("dest", help="the folder to hold the samples, outside the repository")
    args = ap.parse_args(argv)
    root = C.inside_repo(args.dest)
    if root:
        ap.error(f"refusing to put the samples inside the repository ({root}); pick a folder outside it")
    pins = load_pins() if pins is None else pins
    dest = Path(args.dest)
    dest.mkdir(parents=True, exist_ok=True)

    fetched = 0
    for pin in pins["files"]:
        target = dest / pin["name"]
        if present(target, pin):
            continue
        try:
            fetch(pin, sample_url(pins, pin["name"]), target, opener, sleep)
        except FetchError as e:
            print(f"fetch_unitypy_samples: {pin['name']}: {e}", file=sys.stderr)
            return 1
        fetched += 1
    n = len(pins["files"])
    print(f"{n} UnityPy samples at {pins['commit'][:7]} in {dest}: {fetched} fetched, {n - fetched} already there")
    return 0


if __name__ == "__main__":
    sys.exit(main())
