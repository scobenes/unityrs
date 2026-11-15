# Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
"""UnityPy-compatible file and dict reading through the Rust core.

Core operations release the GIL. Budget callbacks never enter Python: Budget
holds Rust atomic counters. Budgets bound retained Rust memory and conversion
staging. The caller's existing Python input buffers and returned Python values
remain owned by Python and are outside this budget. ``fork()`` is safe only when no core
call is in flight in another thread. Use multiprocessing's spawn or forkserver,
or fork before reading. No child-side lock reset is installed.

Object mode, generated classes, helper classes and exports belong to later
binding releases. This scaffold has no UnityPy or Pillow runtime dependency.
"""

import ntpath
import os
import re
import sys
import threading
import weakref
from functools import wraps

from ._native import Budget, Limits, LimitError, TypeTreeError, NativeEnvironment
from .files import BundleFile, EndianBinaryReader, EndianBinaryReader_Stream, File, ObjectReader, SerializedFile, WebFile

__version__ = "0.0.1"


def _key(file, name):
    # environment.py:141-149: Python computes the eager hash default even if
    # a stream has a name. Preserve Python's own randomized hash and TypeError.
    if name:
        return name
    default = str(file.__hash__()) if hasattr(file, "__hash__") else ""
    return getattr(file, "name", default)


def _locked(method):
    @wraps(method)
    def call(self, *args, **kwargs):
        with self._lock:
            return method(self, *args, **kwargs)
    return call


class Environment:
    """An eager load graph. File and object wrappers retain their parents."""

    def __init__(self, *args, path=None, limits=None, budget=None):
        self._lock = threading.RLock()
        self._limits = limits if limits is not None else Limits()
        self._native = NativeEnvironment(limits, budget)
        self.path = os.getcwd() if path is None else os.fspath(path)
        self.files = {}
        self.cabs = {}
        self._wrappers = weakref.WeakValueDictionary()
        self._keys = {}
        self._names = {}
        self._synced = {}
        self._cab_synced = {}
        for source in args:
            if isinstance(source, (str, os.PathLike)):
                source = os.fspath(source)
                if os.path.isfile(source):
                    self.path = ntpath.dirname(source) or "."
                elif os.path.isdir(source):
                    self.path = source
                self._native.load_path(source, True, False)
                self._sync()
            else:
                self.load_file(source)
        if len(self.files) == 1:
            self.file = next(iter(self.files.values()))

    @_locked
    def _wrap(self, native, parent, dependency=False):
        # Build iteratively while holding the cache lock. Publish only complete
        # wrappers; metadata/entries/object calls may release the GIL.
        def create(handle, owner):
            identity = handle.identity
            existing = self._wrappers.get(identity)
            if existing is not None:
                return existing, None
            metadata = handle.metadata()
            if owner is self:
                metadata["name"] = self._names.get(metadata["name"], metadata["name"])
            cls = {"SerializedFile": SerializedFile, "BundleFile": BundleFile,
                   "WebFile": WebFile, "EndianBinaryReader": EndianBinaryReader,
                   "EndianBinaryReader_Stream": EndianBinaryReader_Stream}[metadata.pop("kind")]
            result = cls.__new__(cls)
            result._init(handle, metadata, owner, self, dependency)
            entries = iter(handle.entries()) if isinstance(result, (BundleFile, WebFile)) else iter(())
            return result, (identity, result, entries)

        result, frame = create(native, parent)
        pending = [frame] if frame is not None else []
        while pending:
            identity, owner, entries = pending[-1]
            entry = next(entries, None)
            if entry is None:
                self._wrappers[identity] = owner
                pending.pop()
            else:
                key, child = entry
                item, frame = create(child, owner)
                owner.files[key] = item
                if frame is not None:
                    pending.append(frame)
        return result

    @_locked
    def _sync(self):
        for key, native, dependency in self._native.files():
            identity = native.identity
            if self._synced.get(key) == identity:
                continue
            original_key = self._keys.get(key, key)
            self.files[original_key] = self._wrap(native, self, dependency)
            self._synced[key] = identity
        # The native registry applies registrations in load order, including
        # replacements that keep an earlier file-dict position (ZIP members).
        for key, native in self._native.cabs():
            identity = native.identity
            if self._cab_synced.get(key) != identity:
                self.cabs[key] = self._wrap(native, self)
                self._cab_synced[key] = identity

    @_locked
    def load_file(self, file, parent=None, name=None, is_dependency=False):
        # environment.py:100-160. Paths keep their original key even when
        # their physical lookup is relative to Environment.path.
        parent = parent or self
        if isinstance(file, (str, os.PathLike)):
            file = os.fspath(file)
            key = file
            physical = file
            if not os.path.exists(physical) and not os.path.isabs(physical):
                physical = os.path.join(self.path, physical)
            self._keys[physical] = key
            match = re.match(r"(.*?([^/\\]+?))\.split\d+", physical)
            if match:
                key = re.sub(r"\.split\d+.*$", "", key)
                self._keys[match.group(1)] = key
            native = self._native.load_path(physical, False, is_dependency)
            self._sync()
            if native is None:
                return None
            result = self._wrap(native, parent, is_dependency)
        else:
            key = _key(file, name)
            # Rust keys are byte strings; Python dict keys keep the stream's
            # original type (including None and integer file descriptors).
            native_key = key if isinstance(key, str) else f"@stream:{id(file)}"
            self._keys[native_key] = key
            self._names[native_key] = key if isinstance(key, str) else ""
            stream = hasattr(file, "read")
            if stream:
                if not hasattr(file, "seek") or not hasattr(file, "tell"):
                    raise TypeError("memoryview: a bytes-like object is required")
                data = self._read_stream(file)
            else:
                if not isinstance(file, (bytes, bytearray, memoryview)):
                    raise TypeError("Unsupported file source")
                data = file
            native = self._native.load_bytes(bytes(data), native_key.encode("utf-8", "surrogateescape"), is_dependency, stream)
            self._sync()
            if native is None:
                return None
            result = self._wrap(native, parent, is_dependency)
            self.files[key] = result
        return result

    def _read_stream(self, stream):
        pos = stream.tell()
        try:
            stream.seek(0)
            # Short reads need not mean EOF. Keep every request and the total
            # bounded, including Python's Py_ssize_t read-size limit.
            remaining = self._limits.max_input_bytes + 1
            data = bytearray()
            while remaining:
                chunk = stream.read(min(remaining, sys.maxsize))
                if not chunk:
                    break
                data.extend(chunk)
                remaining -= len(chunk)
            if len(data) > self._limits.max_input_bytes:
                raise LimitError("input exceeds max_input_bytes")
            return data
        finally:
            stream.seek(pos)

    @property
    def objects(self):
        # environment.py:186-206: nondependency objects, recursively in dict order.
        result = []

        def search(file):
            if getattr(file, "is_dependency", False):
                return
            if isinstance(file, SerializedFile):
                result.extend(file.objects.values())
            elif isinstance(file, (BundleFile, WebFile)):
                for child in file.files.values():
                    search(child)

        for file in self.files.values():
            search(file)
        return result

    @property
    def assets(self):
        result = []

        def search(file):
            if getattr(file, "is_dependency", False):
                return
            if isinstance(file, SerializedFile):
                result.append(file)
            elif isinstance(file, (BundleFile, WebFile)):
                for child in file.files.values():
                    search(child)

        for file in self.files.values():
            search(file)
        return result

    @_locked
    def load_files(self, files):
        import re
        seen = set()
        for path in files:
            name = os.fspath(path)
            match = re.match(r"(.*?([^/\\]+?))\.split\d+", name)
            if match:
                base = match.group(1)
                if base in seen:
                    continue
                seen.add(base)
            self._native.load_files(name)
            self._sync()

    @_locked
    def load_zip_file(self, value):
        if isinstance(value, (str, os.PathLike)):
            self._native.load_zip_path(os.fspath(value))
        else:
            data = self._read_stream(value) if hasattr(value, "read") else value
            self._native.load_zip_bytes(bytes(data))
        self._sync()

    def load_assets(self, assets, open_f):
        # The split join is handled by the core; regular entries preserve the
        # opener's stream rule and are always closed after the eager load.
        import re
        loaded = set()
        for name in assets:
            match = re.match(r"(.*?([^/\\]+?))\.split\d+", name)
            if match:
                base = match.group(1)
                if base not in loaded:
                    self.load_file(name)
                    loaded.add(base)
            else:
                with open_f(name) as stream:
                    self.load_file(stream, name=name)

    def load(self, files):
        for name in files:
            if os.path.exists(name):
                with open(name, "rb") as stream:
                    self.files[ntpath.basename(name)] = self.load_file(stream, name=name)

    def find_file(self, name, is_dependency=True):
        found = self.get_cab(name)
        if found is not None:
            return found
        direct = os.path.join(self.path, name)
        if os.path.isfile(direct):
            return self.load_file(direct, is_dependency=is_dependency)
        if not hasattr(self, "local_files"):
            self.local_files = []
            def walk(path):
                entries = list(os.scandir(path))
                for entry in entries:
                    if not entry.is_dir(follow_symlinks=False):
                        self.local_files.append(os.path.abspath(entry.path))
                for entry in entries:
                    if entry.is_dir(follow_symlinks=False):
                        walk(entry.path)
            walk(self.path)
        for path in self.local_files:
            if path == name or path.endswith(name):
                return self.load_file(path, is_dependency=is_dependency)
        for path in self.local_files:
            simple = os.path.join(os.path.dirname(path), ntpath.basename(path).lower())
            if simple.endswith(name):
                return self.load_file(simple, is_dependency=is_dependency)
        raise FileNotFoundError(f"File {name} not found in {self.path}")

    @_locked
    def load_folder(self, path):
        self._native.load_folder(os.fspath(path))
        self._sync()

    def get(self, key, default=None):
        return getattr(self, key, default)

    def get_cab(self, name):
        return self.cabs.get(ntpath.basename(name).lower())

    def register_cab(self, name, item):
        self.cabs[ntpath.basename(name).lower()] = item


load = Environment
AssetsManager = Environment
