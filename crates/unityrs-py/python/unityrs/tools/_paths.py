"""Held-directory extractor writes using the shared atomic output sink."""
import contextlib
import contextvars
import os

from .._output import OutputDirectory, output_name
from ._bounded import text

_root = contextvars.ContextVar("unityrs_extractor_root", default=None)


def relative(value, guard):
    text(guard, value)
    if not isinstance(value, str):
        raise ValueError("asset output name must be a string")
    if (str.startswith(value, "/")
            or any(str.find(value, char) >= 0 for char in ("\0", "\\", ":"))):
        raise ValueError("unsafe asset output name")
    start = 0
    while True:
        end = str.find(value, "/", start)
        stop = str.__len__(value) if end < 0 else end
        if stop - start == 2 and str.startswith(value, "..", start, stop):
            raise ValueError("unsafe asset output name")
        if end < 0:
            break
        start = end + 1
    # Normalize subclasses before joins and f-strings can invoke their
    # concatenation or formatting protocols with a different path spelling.
    return str.__getitem__(value, slice(None))


def _walk(path, guard, create=False, parent=None):
    text(guard, path)
    guard.step(0, len(path) * 256 + 512, len(path))
    parts = path.split("/")
    fd = os.dup(parent) if parent is not None else os.open(
        "/" if os.path.isabs(path) else ".", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in parts:
            if part in ("", "."):
                continue
            if create:
                try:
                    os.mkdir(part, dir_fd=fd)
                except FileExistsError:
                    pass
            next_fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return fd
    except BaseException:
        os.close(fd)
        raise


@contextlib.contextmanager
def root(directory, guard, create):
    directory = os.fspath(directory)
    fd = _walk(directory, guard, create)
    token = _root.set((directory, fd))
    try:
        yield
    finally:
        _root.reset(token)
        os.close(fd)


def _location(path, guard):
    text(guard, path)
    held = _root.get()
    if held is None:
        parent, leaf = os.path.split(path)
        return parent or ".", leaf, None
    directory, fd = held
    prefix = directory.rstrip("/") + "/"
    if path == directory:
        return "", "", fd
    if not path.startswith(prefix):
        raise ValueError("unsafe asset output name")
    path = path[len(prefix):]
    relative(path, guard)
    parent, leaf = os.path.split(path)
    return parent, leaf, fd


def mkdir(path, guard):
    parent, leaf, fd = _location(path, guard)
    joined = os.path.join(parent, leaf)
    if fd is not None and joined:
        relative(joined, guard)
    os.close(_walk(joined, guard, True, fd))


@contextlib.contextmanager
def file(path, mode, guard, *, keep_partial=True, **kwargs):
    parent, leaf, fd = _location(path, guard)
    output_name(leaf, guard=guard)
    directory = OutputDirectory(".", guard=guard)
    directory.fd = _walk(parent, guard, False, fd)
    failure = None
    try:
        with directory.open(leaf, mode, **kwargs) as output:
            try:
                yield output
            except Exception as error:
                if not keep_partial:
                    raise
                # Preserve the original exporter's partial-file observation,
                # publishing it by rename so outside links remain untouched.
                failure = error
        if failure is not None:
            raise failure
    finally:
        directory.__exit__()


def image(image, path, guard):
    # Let Pillow select the original extension while the shared sink retains
    # ownership of the filesystem open and atomic replacement.
    from PIL import Image
    Image.init()
    extension = os.path.splitext(path)[1].lower()
    try:
        format = Image.EXTENSION[extension]
    except KeyError as error:
        raise ValueError(f"unknown file extension: {extension}") from error
    class NamedOutput:
        name = path

        def __getattr__(self, key):
            return getattr(output, key)

    with file(path, "wb", guard, keep_partial=False) as output:
        image.save(NamedOutput(), format=format)
