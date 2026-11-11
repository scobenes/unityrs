"""Scratch folders: the rule every Python test and tool in this repository keeps for what it writes outside its own
memory. Its scratch is removed whatever happens: when it passes, fails or raises, and when a child it runs ends,
aborts, crashes or is killed.

- **Every scratch folder is made here** (`make`, or `for_case`, `for_class`, `folder`), never by `tempfile`'s
  `mkdtemp` or `TemporaryDirectory` (`tests/test_child.py` scans for them). A test case's folder goes with the
  case's cleanups, which unittest runs on a failure too (`for_case`); a class's with its class cleanups, which
  it runs when `setUpClass` raises, where `tearDownClass` is never run (`for_class`); a tool's on leaving its
  `with folder()` block, however it is left.
- **A child's scratch is its parent's** (`child.run`): the parent makes a folder for each run of a child, names
  it in the child's `UNITYRS_TEST_SCRATCH`, and removes it when the run ends, however it ends. The child (a
  Python process, or a Rust test binary through `unityrs_testkit::scratch`, which keeps the same rule) makes its
  own folders inside it. A child that aborts, segfaults or is killed runs none of its own cleanups, so its
  parent's removal is the one that runs.
- **A process with no parent's folder** makes its folders in a root of its own in the temp folder,
  `unityrs-test-<pid namespace>-<pid>`, which goes with the last of them. A process that dies in its own code
  (an abort, a segfault, a kill) leaves its root behind, so the first folder any process makes removes every
  such root whose process no longer runs (`sweep`): a root is its own process's alone, and only roots of this
  pid namespace are compared with its processes.

`unityrs_testkit::scratch` keeps the same rule, with the same variable and root names, for the Rust tests.
Standard library only.
"""

from __future__ import annotations

import contextlib
import os
import re
import stat
import tempfile
import threading
from pathlib import Path

# The folder a parent made for its child to make its scratch in (the rule above).
VAR = "UNITYRS_TEST_SCRATCH"
PREFIX = "unityrs-test-"

_lock = threading.Lock()
_root: Path | None = None  # this process's root, when it has one
_given = False  # whether the root is the parent's (VAR), which the parent removes
_live: set[str] = set()  # the folders made here and not yet removed


def make(prefix: str = "unityrs-", within: os.PathLike | str | None = None) -> Path:
    """A new empty folder, inside the parent's folder or this process's root. The caller removes it (`remove`);
    `for_case`, `for_class` and `folder` do that on every path. `within` makes it inside a folder made here
    instead, which removes it with itself."""
    global _root, _given
    if within is not None:
        return Path(tempfile.mkdtemp(prefix=prefix, dir=within))
    with _lock:
        if _root is None:
            given = os.environ.get(VAR)
            if given and os.path.isdir(given):
                _root, _given = Path(given), True
            else:
                sweep()
                root = own_root()
                remove_tree(root)  # a dead process's of the same pid, which only this process can have now
                root.mkdir(mode=0o700)
                _root, _given = root, False
        path = tempfile.mkdtemp(prefix=prefix, dir=_root)
        _live.add(path)
    return Path(path)


def own_root() -> Path:
    """The root this process makes when no parent names a folder: `unityrs-test-<pid namespace>-<pid>` in the temp
    folder (module docs). The temp folder's first lookup (`tempfile.gettempdir`) may probe it with a write."""
    return Path(tempfile.gettempdir()) / f"{PREFIX}{_namespace() or 0}-{os.getpid()}"


def root() -> Path | None:
    """This process's root once it has made a folder: the parent's (`VAR`) or `own_root()`; None before."""
    return _root


def remove(path: os.PathLike | str) -> None:
    """Removes a folder `make` made, with everything in it, and this process's root with its last folder."""
    global _root
    remove_tree(path)
    with _lock:
        _live.discard(os.fspath(path))
        if not _live and _root is not None and not _given:
            with contextlib.suppress(OSError):
                os.rmdir(_root)
            _root = None


def for_case(case, prefix: str = "unityrs-") -> Path:
    """A folder removed with `case`'s cleanups (unittest runs them when the test fails or raises too)."""
    path = make(prefix)
    case.addCleanup(remove, path)
    return path


def for_class(cls, prefix: str = "unityrs-") -> Path:
    """A folder removed with `cls`'s class cleanups, which unittest runs when `setUpClass` raises too."""
    path = make(prefix)
    cls.addClassCleanup(remove, path)
    return path


@contextlib.contextmanager
def folder(prefix: str = "unityrs-"):
    """A folder removed on leaving the `with` block, however it is left."""
    path = make(prefix)
    try:
        yield path
    finally:
        remove(path)


def remove_tree(path: os.PathLike | str) -> None:
    """Removes `path` and everything under it a folder at a time, without following a link: a chain of folders
    deeper than Python's recursion limit (shutil.rmtree recurses), and a folder of mode 000, go too."""
    top = os.fspath(path)
    try:
        if not stat.S_ISDIR(os.lstat(top).st_mode):
            os.unlink(top)
            return
    except FileNotFoundError:
        return
    todo, folders = [top], []
    while todo:
        here = todo.pop()
        with contextlib.suppress(OSError):
            os.chmod(here, 0o700)
        folders.append(here)
        try:
            entries = list(os.scandir(here))
        except OSError:
            continue
        for entry in entries:
            if entry.is_dir(follow_symlinks=False):
                todo.append(entry.path)
            else:
                with contextlib.suppress(OSError):
                    os.unlink(entry.path)
    for here in reversed(folders):
        with contextlib.suppress(OSError):
            os.rmdir(here)


def sweep() -> list[Path]:
    """Removes every root in the temp folder of a process of this pid namespace that no longer runs (the rule
    above), and returns them. Nothing on a system without /proc's pid namespaces."""
    ns = _namespace()
    if ns is None:
        return []
    own = re.compile(re.escape(f"{PREFIX}{ns}-") + r"([0-9]+)")
    gone = []
    try:
        entries = list(os.scandir(tempfile.gettempdir()))
    except OSError:
        return []
    for entry in entries:
        m = own.fullmatch(entry.name)
        if m and int(m[1]) != os.getpid() and not os.path.exists(f"/proc/{m[1]}"):
            remove_tree(entry.path)
            gone.append(Path(entry.path))
    return gone


def _namespace() -> str | None:
    """This process's pid namespace, as the number /proc/self/ns/pid names it."""
    try:
        m = re.fullmatch(r"pid:\[([0-9]+)\]", os.readlink("/proc/self/ns/pid"))
    except OSError:
        return None
    return m[1] if m else None
