"""Validated asset output names and directory-relative local replacement."""
import contextlib
import os
import secrets


def output_name(name, *, container=False, guard=None):
    """Validate original spelling before any directory prefix is discarded."""
    if not isinstance(name, str):
        raise ValueError("asset output name must be a string")
    size = str.__len__(name)
    if (not size or str.startswith(name, "/")
            or any(str.find(name, c) >= 0 for c in ("\0", "\\", ":"))):
        raise ValueError("unsafe asset output name")
    start = 0
    while True:
        end = str.find(name, "/", start)
        stop = size if end < 0 else end
        count = stop - start
        if count == 2 and str.startswith(name, "..", start, stop):
            raise ValueError("unsafe asset output name")
        if end < 0:
            break
        if not container:
            raise ValueError("unsafe asset output name")
        start = end + 1
    if not count or count == 1 and str.startswith(name, ".", start):
        raise ValueError("unsafe asset output name")
    if guard is None and start:
        from ._native import TreeTraversal
        guard = TreeTraversal()
    if guard is not None:
        # Retain room for the leaf copy and filesystem encoding together.
        guard.step(0, count * 8 + 512, count)
    return str.__getitem__(name, slice(start, None))


class OutputDirectory:
    """Hold a local output directory; never follow existing output leaves.

    Custom filesystems own their backend's opens. They receive only validated
    leaf names; host writes use a held directory descriptor and atomic replace.
    """
    def __init__(self, directory, fs=None, guard=None):
        self.directory = os.fspath(directory)
        self.fs = fs
        self.fd = None
        if guard is None:
            from ._native import TreeTraversal
            guard = TreeTraversal()
        self.guard = guard

    def __enter__(self):
        self.guard.step(0, len(self.directory) * 4 + 256, 1)
        protocol = getattr(self.fs, "protocol", ())
        local = (self.fs is None or getattr(self.fs, "open", None) is open
                 or protocol in ("file", "local", ("file", "local"), ("local", "file")))
        if local:
            self.fd = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        return self

    def __exit__(self, *args):
        if self.fd is not None:
            os.close(self.fd)

    @contextlib.contextmanager
    def open(self, name, mode, **kwargs):
        name = output_name(name, guard=self.guard)
        if self.fd is None:
            self.guard.step(0, (len(self.directory) + len(name) + len(self.fs.sep)) * 4 + 256, 1)
            with self.fs.open(self.fs.sep.join([self.directory, name]), mode, **kwargs) as stream:
                yield stream
            return
        temporary = ".unityrs-" + secrets.token_hex(16)
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o666, dir_fd=self.fd)
        try:
            with os.fdopen(descriptor, mode, **kwargs) as stream:
                yield stream
            os.replace(temporary, name, src_dir_fd=self.fd, dst_dir_fd=self.fd)
        finally:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(temporary, dir_fd=self.fd)
