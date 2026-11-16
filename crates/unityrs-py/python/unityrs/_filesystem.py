# Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/helpers/ImportHelper.py. See THIRD_PARTY_LICENSES.
"""Local filesystem fallback and bounded loader path helpers."""
import os


class LocalFileSystem:
    sep = os.sep
    protocol = ("file", "local")
    open = staticmethod(open)
    exists = staticmethod(os.path.exists)
    isfile = staticmethod(os.path.isfile)
    isdir = staticmethod(os.path.isdir)
    makedirs = staticmethod(os.makedirs)

    def walk(self, path):
        # Inventory preserves scandir order and lists symlink directories as
        # files, as the local filesystem UnityPy uses does.
        path = os.path.abspath(path.removeprefix("file://").removeprefix("file:"))
        pending = [path]
        while pending:
            current = pending.pop()
            try:
                entries = os.scandir(current)
            except FileNotFoundError:
                continue
            except PermissionError:
                continue
            with entries:
                directories, files = [], []
                for entry in entries:
                    if entry.is_dir(follow_symlinks=False):
                        directories.append(entry.name)
                    else:
                        files.append(entry.name)
            yield current, directories, files
            pending.extend(os.path.join(current, name) for name in reversed(directories))


def local_fs():
    try:
        from fsspec.implementations.local import LocalFileSystem as FsspecLocal
    except ImportError:
        return LocalFileSystem()
    return FsspecLocal()


def is_local(fs):
    return getattr(fs, "protocol", ()) in ("file", "local", ("file", "local"), ("local", "file"))


def find_sensitive_path(directory, path):
    # helpers/ImportHelper.py:159-173: split once into head/tail, with local
    # os.listdir even when an environment supplies a different filesystem.
    parts = os.path.split(path.strip(os.sep))
    for part in parts:
        for name in os.listdir(directory):
            if name.lower() == part.lower():
                directory = os.path.join(directory, name)
                break
        else:
            return None
    return directory


def confined_path(path, root):
    """Match the core's dependency-root check immediately before each open."""
    resolved, root = os.path.realpath(path), os.path.realpath(root)
    if os.path.commonpath((root, resolved)) == root:
        return resolved
    ancestor = resolved
    while True:
        try:
            if os.path.samefile(root, ancestor):
                return resolved
        except OSError:
            pass
        parent = os.path.dirname(ancestor)
        if parent == ancestor:
            raise FileNotFoundError("file lies outside environment root")
        ancestor = parent
