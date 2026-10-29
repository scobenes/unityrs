"""Fetch UnityPy's test samples, at the pinned commit, into a folder outside the repository.

    python tools/parity/fetch_unitypy_samples.py DEST
    UNITYRS_UNITYPY_SAMPLES=DEST python -m unittest discover -s tools/parity/tests

unitypy_samples.json pins UnityPy's commit and the size and sha256 of each
sample; the sha256 is also the file's Git LFS oid at that commit. The samples
are Git LFS objects, and raw.githubusercontent.com serves the 130-byte LFS
pointer in their place, so they come from media.githubusercontent.com and a
pointer is refused. That host answers with the file itself, so a redirect is
refused too. A file already in DEST with the pinned size and sha256 is kept.
Anything else is downloaded again, checked, and only then written.

The pins name each sample by a plain file name, and every URL must lie under
URL_PREFIX; other pins are refused before anything is fetched.

DEST holds the pinned samples and nothing else. A folder restored from a cache
is checked like a download: an entry that is not a pinned sample's name, or
that is not a regular file (a symlink, a folder), is refused before anything
is fetched. Each sample is written to a new temporary file in DEST, created
exclusively and without following symlinks, and renamed over its name.

The samples are game bundles with no licence of their own. They are fetched
where they are needed and never copied into this repository, so DEST may not
lie inside it (symlinks followed).

Exit status: 0 when DEST holds every pinned sample and nothing else; 1 when
one could not be fetched or does not match its pin, or when DEST holds
anything else; 2 for a usage error or pins it refuses. Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import re
import stat
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import canonical as C  # noqa: E402

PINS = HERE / "unitypy_samples.json"
# The one place the samples may come from (see the docstring).
URL_PREFIX = "https://media.githubusercontent.com/media/K0lb3/UnityPy/"
LFS_POINTER = b"version https://git-lfs.github.com/spec/"
ATTEMPTS = 3
# Each read waits at most TIMEOUT_S; one sample's download, retries included, gets DEADLINE_S in all.
TIMEOUT_S = 60
DEADLINE_S = 300
CHUNK = 1 << 20

# A sample's name is one plain file name, so the sample lands in DEST and nowhere else.
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
_COMMIT = re.compile(r"[0-9a-f]{40}")
_SHA256 = re.compile(r"[0-9a-f]{64}")


class FetchError(Exception):
    """A sample that could not be downloaded, or that does not match its pin."""


class PinsError(ValueError):
    """Pins this fetcher refuses to follow."""


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise FetchError(f"redirected ({code}) to {newurl}; the samples are served without a redirect")


def urlopen(url, timeout):
    """urllib's urlopen, except that a redirect raises FetchError instead of requesting another URL."""
    return urllib.request.build_opener(_NoRedirect).open(url, timeout=timeout)


def load_pins(path: Path = PINS) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def check_pins(pins) -> None:
    """Raise PinsError unless every name is a plain file name and every URL lies under URL_PREFIX."""
    if type(pins) is not dict:
        raise PinsError("the pins are not an object")
    commit, url, files = pins.get("commit"), pins.get("url"), pins.get("files")
    if type(commit) is not str or not _COMMIT.fullmatch(commit):
        raise PinsError(f"commit {commit!r} is not 40 lowercase hex digits")
    if type(url) is not str or not url.startswith(URL_PREFIX) or "{name}" not in url:
        raise PinsError(f"url {url!r} does not start with {URL_PREFIX} or holds no {{name}}")
    if type(files) is not list or not files:
        raise PinsError("files is not a non-empty list")
    seen = set()
    for pin in files:
        name = pin.get("name") if type(pin) is dict else None
        if type(name) is not str or not NAME.fullmatch(name):
            raise PinsError(f"name {name!r} is not a plain file name (one of {NAME.pattern})")
        if name in seen:
            raise PinsError(f"name {name!r} is pinned twice")
        seen.add(name)
        if type(pin.get("bytes")) is not int or pin["bytes"] <= 0:
            raise PinsError(f"{name}: bytes {pin.get('bytes')!r} is not a positive integer")
        if type(pin.get("sha256")) is not str or not _SHA256.fullmatch(pin["sha256"]):
            raise PinsError(f"{name}: sha256 {pin.get('sha256')!r} is not 64 lowercase hex digits")
        if not sample_url(pins, name).startswith(URL_PREFIX):
            raise PinsError(f"{name}: its URL leaves {URL_PREFIX}")


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


def strays(dest: Path, names) -> list[str]:
    """Each entry of DEST other than a pinned sample's name holding a regular file, as 'NAME: why'."""
    out = []
    with os.scandir(dest) as entries:
        for entry in sorted(entries, key=lambda e: e.name):
            if entry.name not in names:
                out.append(f"{entry.name}: not a pinned sample")
            elif not stat.S_ISREG(entry.stat(follow_symlinks=False).st_mode):
                out.append(f"{entry.name}: not a regular file")
    return out


def present(path: Path, pin: dict) -> bool:
    """Whether `path` (a regular file, which strays checked) holds the pinned sample. Its size is checked first."""
    try:
        if path.stat().st_size != pin["bytes"]:
            return False
        return mismatch(path.read_bytes(), pin) is None
    except OSError:
        return False


def download(url: str, limit: int, opener, sleep, clock=time.monotonic) -> bytes:
    """The body at `url`, read up to `limit` + 1 bytes (enough to tell it is too long).

    A network error is retried, up to ATTEMPTS tries in all. The whole
    download, retries included, must end within DEADLINE_S: each read waits
    at most TIMEOUT_S, and a body that keeps trickling in is cut off. A body
    that arrives in time is returned whatever it holds, for the caller to
    check.
    """
    deadline = clock() + DEADLINE_S

    def left():
        remaining = deadline - clock()
        if remaining <= 0:
            raise FetchError(f"{url}: not downloaded within {DEADLINE_S} s")
        return remaining

    for attempt in range(1, ATTEMPTS + 1):
        try:
            with opener(url, timeout=min(TIMEOUT_S, left())) as response:
                chunks, total = [], 0
                while total <= limit:
                    left()
                    chunk = response.read(min(CHUNK, limit + 1 - total))
                    if not chunk:
                        break
                    chunks.append(chunk)
                    total += len(chunk)
                left()
                return b"".join(chunks)
        except (OSError, http.client.HTTPException) as e:
            if attempt == ATTEMPTS:
                raise FetchError(f"{url}: {e} (after {ATTEMPTS} attempts)") from e
            sleep(min(2**attempt, left()))
    raise AssertionError("unreachable")


def write_sample(dest: Path, name: str, data: bytes) -> None:
    """DEST/name := data, through a new file created exclusively (O_EXCL, no symlink followed) and renamed."""
    fd, tmp = tempfile.mkstemp(prefix=f".{name}.", suffix=".partial", dir=dest)
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(data)
        os.replace(tmp, dest / name)
    except BaseException:
        os.unlink(tmp)
        raise


def fetch(pin: dict, url: str, dest: Path, opener, sleep, clock) -> None:
    data = download(url, pin["bytes"], opener, sleep, clock)
    why = mismatch(data, pin)
    if why:
        raise FetchError(f"{url}: {why}")
    write_sample(dest, pin["name"], data)


def main(argv=None, *, opener=urlopen, sleep=time.sleep, clock=time.monotonic, pins=None) -> int:
    ap = argparse.ArgumentParser(description="Fetch UnityPy's pinned test samples into DEST.")
    ap.add_argument("dest", help="the folder to hold the samples, outside the repository")
    args = ap.parse_args(argv)
    root = C.inside_repo(args.dest)
    if root:
        ap.error(f"refusing to put the samples inside the repository ({root}); pick a folder outside it")
    pins = load_pins() if pins is None else pins
    try:
        check_pins(pins)
    except PinsError as e:
        ap.error(f"refusing the pins: {e}")
    try:
        Path(args.dest).mkdir(parents=True, exist_ok=True)
    except FileExistsError:
        ap.error(f"{args.dest} is not a folder")
    dest = Path(args.dest).resolve()

    names = [pin["name"] for pin in pins["files"]]
    found = strays(dest, set(names))
    if found:
        for line in found:
            print(f"fetch_unitypy_samples: {dest}: {line}", file=sys.stderr)
        print(f"fetch_unitypy_samples: {dest} may hold only the pinned samples, as regular files", file=sys.stderr)
        return 1

    fetched = 0
    for pin in pins["files"]:
        target = dest / pin["name"]
        # check_pins and strays make this impossible; it stays as the last guard.
        if target.resolve().parent != dest:
            raise AssertionError(f"{target} leaves {dest}")
        if present(target, pin):
            continue
        try:
            fetch(pin, sample_url(pins, pin["name"]), dest, opener, sleep, clock)
        except FetchError as e:
            print(f"fetch_unitypy_samples: {pin['name']}: {e}", file=sys.stderr)
            return 1
        fetched += 1
    n = len(names)
    print(f"{n} UnityPy samples at {pins['commit'][:7]} in {dest}: {fetched} fetched, {n - fetched} already there")
    return 0


if __name__ == "__main__":
    sys.exit(main())
