"""Validated asset output names and directory-relative local replacement."""
import contextlib
import ntpath
import os
import secrets


def output_name(name, *, container=False):
    """Validate original spelling before any directory prefix is discarded."""
    if not isinstance(name, str):
        raise ValueError("asset output name must be a string")
    parts = name.split("/")
    if (not name or ntpath.isabs(name) or any(c in name for c in ("\0", "\\", ":"))
            or ".." in parts or not any(p not in ("", ".") for p in parts)
            or not container and (len(parts) != 1 or name == ".")
            or container and parts[-1] in ("", ".")):
        raise ValueError("unsafe asset output name")
    return parts[-1] if container else name


class OutputDirectory:
    """Hold a local output directory; never follow existing output leaves.

    Custom filesystems own their backend's opens. They receive only validated
    leaf names; host writes use a held directory descriptor and atomic replace.
    """
    def __init__(self, directory, fs=None):
        self.directory = os.fspath(directory)
        self.fs = fs
        self.fd = None

    def __enter__(self):
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
        output_name(name)
        root = os.path.abspath(self.directory)
        candidate = os.path.abspath(os.path.join(root, name))
        if os.path.commonpath([root, candidate]) != root:
            raise ValueError("asset output path escapes its directory")
        if self.fd is None:
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
