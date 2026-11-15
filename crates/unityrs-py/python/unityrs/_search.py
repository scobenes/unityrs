"""Bound the filesystem inventory used by dependency lookup."""
import os

MAX_ENTRIES = 1_000_000
MAX_BYTES = 128 * 1024 * 1024


def files(root):
    from . import LimitError
    count = 0
    charged = 0

    def charge(path):
        nonlocal count, charged
        # DirEntry, path strings, pending directories and result-list slots.
        size = 1024 + 4 * len(path)
        if count >= MAX_ENTRIES or charged + size > MAX_BYTES:
            raise LimitError('filesystem search expansion exceeds limit')
        count += 1
        charged += size

    root = os.path.abspath(root)
    charge(root)
    pending = [root]
    result = []
    while pending:
        directories = []
        entries = os.scandir(pending.pop())
        try:
            for entry in entries:
                charge(entry.path)
                if entry.is_dir(follow_symlinks=False):
                    directories.append(entry.path)
                else:
                    result.append(entry.path)
        finally:
            if hasattr(entries, 'close'):
                entries.close()
        # Preserve files first, then depth-first subdirectories in scan order.
        pending.extend(reversed(directories))
    return result
