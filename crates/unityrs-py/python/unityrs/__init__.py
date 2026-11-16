# Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
"""UnityPy-compatible file and dict reading through the Rust core.

Core operations release the GIL. Budget callbacks never enter Python: Budget
holds Rust atomic counters. Budgets bound retained Rust memory, conversion
staging and object reconstruction through handoff. The caller's existing Python
input buffers and returned Python values remain owned by Python and are outside
this budget. ``fork()`` is safe only when no core
call is in flight in another thread. Use multiprocessing's spawn or forkserver,
or fork before reading. No child-side lock reset is installed.

Mutable helpers, enums, versions and loader config accompany dict and object mode.
ObjectReader and unityrs.classes provide runtime classes generated from an external
TPK (or the installed UnityPy resource). No Pillow dependency.
"""

import ntpath
import os
import re
import sys
import threading
import weakref
from functools import wraps

from ._native import Budget, Limits, LimitError, TypeTreeError, NativeEnvironment, TreeTraversal
from . import config, enums
from .helpers import _tpk
from ._filesystem import local_fs, is_local, find_sensitive_path, confined_path
from .files import BundleFile, EndianBinaryReader, EndianBinaryReader_Stream, File, ObjectReader, SerializedFile, WebFile

__version__ = "0.0.1"
reSplit = re.compile(r"(.*?([^\/\\]+?))\.split\d+")


def simplify_name(name):
    return ntpath.basename(name).lower()


def _key(file, name):
    # environment.py:141-149: Python computes the eager hash default even if
    # a stream has a name. Preserve Python's own randomized hash and TypeError.
    if name:
        return name
    default = str(file.__hash__()) if hasattr(file, "__hash__") else ""
    return getattr(file, "name", default)


def _name_text(name):
    return os.fsdecode(name) if isinstance(name, bytes) else str(name)


def _locked(method):
    @wraps(method)
    def call(self, *args, **kwargs):
        with self._lock:
            return method(self, *args, **kwargs)
    return call


class Environment:
    """An eager load graph. File and object wrappers retain their parents."""

    typetree_generator = None
    _container_index_built = False

    def __init__(self, *args, fs=None, path=None, limits=None, budget=None):
        self._lock = threading.RLock()
        self._limits = limits if limits is not None else Limits()
        self._budget = budget
        self._native = NativeEnvironment(limits, budget, _tpk.provider())
        self.fs = fs or local_fs()
        self._custom_fs = not is_local(self.fs)
        self.path = ("" if self._custom_fs else os.getcwd()) if path is None else path
        self.local_files = []
        self.local_files_simple = []
        self._container_index_built = False
        self.files = {}
        self.cabs = {}
        self._wrappers = weakref.WeakValueDictionary()
        self._keys = {}
        self._names = {}
        self._synced = {}
        self._cab_synced = {}
        for source in args:
            if isinstance(source, str):
                if self.fs.isfile(source):
                    if ntpath.splitext(source)[-1] in (".apk", ".zip"):
                        self.load_zip_file(source)
                    else:
                        self.path = ntpath.dirname(source) or "."
                        if reSplit.match(source):
                            self.load_files([source])
                        else:
                            self.load_file(source)
                elif self.fs.isdir(source):
                    self.path = source
                    self.load_folder(source)
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
                item.flags = getattr(owner, "_entry_flags", {}).get(key, 0)
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
            self._container_index_built = False
        # The native registry applies registrations in load order, including
        # replacements that keep an earlier file-dict position (ZIP members).
        for key, native in self._native.cabs():
            identity = native.identity
            if self._cab_synced.get(key) != identity:
                self.cabs[key] = self._wrap(native, self)
                self._cab_synced[key] = identity
                self._container_index_built = False

    @_locked
    def load_file(self, file, parent=None, name=None, is_dependency=False, *, _root=None):
        # environment.py:100-160. Paths keep their original key even when
        # their physical lookup is relative to Environment.path.
        parent = parent or self
        if isinstance(file, str):
            match = reSplit.match(file)
            if match:
                data, split_guard = self._split_input(match.group(1), _root)
                return self.load_file(data, parent, match.group(1), is_dependency)
            key = file
            physical = file
            if not os.path.exists(physical):
                if not os.path.isabs(physical):
                    physical = os.path.join(self.path, physical)
                if os.path.exists(physical + ".split0"):
                    data, split_guard = self._split_input(physical, _root)
                    return self.load_file(data, parent, key, is_dependency)
                if not os.path.exists(physical):
                    try:
                        physical = find_sensitive_path(self.path, physical)
                    except FileNotFoundError:
                        if not self._custom_fs and os.fspath(self.path).startswith(("file:", "local:")):
                            return None
                        if self._custom_fs and self.fs.exists(physical):
                            with self.fs.open(physical, "rb") as stream:
                                return self.load_file(stream, parent, key, is_dependency)
                        raise
                    if physical is None:
                        if self._custom_fs and self.fs.exists(file):
                            with self.fs.open(file, "rb") as stream:
                                return self.load_file(stream, parent, key, is_dependency)
                        return None
            if self._custom_fs:
                with self.fs.open(physical, "rb") as stream:
                    return self.load_file(stream, parent, key, is_dependency)
            self._keys[physical] = key
            if os.path.isdir(physical):
                raise IsADirectoryError(21, os.strerror(21), physical)
            match = re.match(r"(.*?([^/\\]+?))\.split\d+", physical)
            if match:
                key = re.sub(r"\.split\d+.*$", "", key)
                self._keys[match.group(1)] = key
            native = config._load(self._native, self._native.load_path, physical, False, is_dependency, _root)
            if native is None:
                self._sync()
                return None
            result = self._wrap(native, parent, is_dependency)
            self._sync()
        else:
            from .streams import EndianBinaryReader as ExplicitReader
            explicit_reader = isinstance(file, ExplicitReader)
            # PARITY.md 19.2: mutable buffers and read-only streams use the
            # bytes reference. Keep every copy charged until native handoff.
            input_guard = None
            if hasattr(file, "read") and (not hasattr(file, "seek") or not hasattr(file, "tell")):
                file, input_guard = self._read_stream(file, positioned=False)
            if not name and isinstance(file, (bytearray, memoryview)) and (
                    isinstance(file, bytearray) or not file.readonly):
                if input_guard is None:
                    input_guard = TreeTraversal(self._limits, self._budget)
                size = memoryview(file).nbytes
                if size > self._limits.max_input_bytes:
                    raise LimitError("input exceeds max_input_bytes")
                input_guard.step(0, size + 256, size)
                file = bytes(file)
            key = _key(file, name)
            # Rust keys are byte strings; Python dict keys keep the stream's
            # original type (including None and integer file descriptors).
            native_key = key if isinstance(key, str) else f"@stream:{id(file)}"
            self._keys[native_key] = key
            self._names[native_key] = key if isinstance(key, str) else ""
            stream = hasattr(file, "read")
            if stream:
                data, input_guard = self._read_stream(file)
            else:
                if not isinstance(file, (bytes, bytearray, memoryview)):
                    raise TypeError(f"Unsupported type for EndianBinaryReader: {type(file)}")
                data = file
            name_guard = TreeTraversal(self._limits, self._budget)
            name_guard.step(0, str.__len__(native_key) * 4 + 256, 0)
            encoded_name = native_key.encode("utf-8", "surrogateescape")
            del name_guard
            from .streams.EndianBinaryReader import EndianBinaryReader_Streamable as ExplicitStream
            stream_kind = stream and (not explicit_reader or isinstance(file, ExplicitStream))
            native = config._load(self._native, self._native.load_bytes, data, encoded_name, is_dependency, stream_kind)
            self._sync()
            if native is None:
                return None
            result = self._wrap(native, parent, is_dependency)
            if explicit_reader:
                if isinstance(result, SerializedFile):
                    result.reader = file
                    file.endian = result._native.serialized_reader_info()[1]
                elif isinstance(result, EndianBinaryReader):
                    result = file
                    self.cabs[simplify_name(native_key)] = result
            if not isinstance(key, str) and isinstance(result, (SerializedFile, EndianBinaryReader, ExplicitReader)):
                self.cabs.pop(simplify_name(native_key), None)
                cab_key = key if isinstance(key, bytes) and not isinstance(result, SerializedFile) else _name_text(key)
                self.cabs[simplify_name(cab_key)] = result
            self.files[key] = result
        self.files[key] = result
        return result

    def _load_split_file(self, basename):
        return self._split_input(basename)[0]

    def _split_input(self, basename, root=None):
        # environment.py:89-98. A missing split0 does not stop split1.
        guard = TreeTraversal(self._limits, self._budget)
        guard.step(0, 256, 0)
        result = bytearray()
        for index in range(999):
            name = f"{basename}.split{index}"
            if self.fs.exists(name):
                physical = confined_path(name, root) if root is not None else name
                with self.fs.open(physical, "rb") as stream:
                    data, staging = self._read_stream(stream)
                    if len(result) + len(data) > self._limits.max_input_bytes:
                        raise LimitError("input exceeds max_input_bytes")
                    guard.step(0, len(data) * 2 + 64, len(data))
                    result.extend(data)
            elif index:
                break
        guard.step(0, len(result) + 64, 0)
        return bytes(result), guard

    def _read_stream(self, stream, positioned=True):
        pos = stream.tell() if positioned else None
        try:
            if positioned:
                stream.seek(0)
            # Short reads need not mean EOF. Keep every request and the total
            # bounded, including Python's Py_ssize_t read-size limit.
            remaining = self._limits.max_input_bytes + 1
            guard = TreeTraversal(self._limits, self._budget)
            guard.step(0, 256, 0)
            data = bytearray()
            while remaining:
                requested = min(remaining, 64 * 1024)
                read_guard = TreeTraversal(self._limits, self._budget)
                read_guard.step(0, requested + 256, 0)
                chunk = stream.read(requested)
                if not chunk:
                    break
                size = memoryview(chunk).nbytes
                if size > remaining:
                    raise LimitError("input exceeds max_input_bytes")
                guard.step(0, size * 2 + 16, size)
                data.extend(chunk)
                remaining -= size
            if len(data) > self._limits.max_input_bytes:
                raise LimitError("input exceeds max_input_bytes")
            return data, guard
        finally:
            if positioned:
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
        self.load_assets(files, lambda path: self.fs.open(path, "rb") if self._custom_fs else open(path, "rb"))

    @_locked
    def load_zip_file(self, value):
        import io
        if isinstance(value, str) and self.fs.exists(value):
            if self._custom_fs:
                with self.fs.open(value, "rb") as stream:
                    data, input_guard = self._read_stream(stream)
                    config._load(self._native, self._native.load_zip_bytes, data)
            else:
                config._load(self._native, self._native.load_zip_path, value)
        else:
            if not isinstance(value, (bytes, bytearray, memoryview, io.BufferedIOBase)):
                raise TypeError("Unsupported type for loading zip file")
            data, input_guard = self._read_stream(value) if hasattr(value, "read") else (value, None)
            config._load(self._native, self._native.load_zip_bytes, data)
        self._sync()

    def load_assets(self, assets, open_f):
        # environment.py:282-308; close inputs after the eager load.
        loaded = set()
        for name in assets:
            match = reSplit.match(name)
            if match:
                base = match.group(1)
                if base not in loaded:
                    data, split_guard = self._split_input(base)
                    self.load_file(data, name=base)
                    loaded.add(base)
            else:
                with open_f(name) as stream:
                    self.load_file(stream, name=name)

    def load(self, files):
        for name in files:
            if self.fs.exists(name):
                with self.fs.open(name, "rb") as stream:
                    self.files[ntpath.basename(name)] = self.load_file(stream, name=name)

    @_locked
    def find_file(self, name, is_dependency=True):
        found = self.get_cab(name)
        if found is not None:
            return found
        local_path = os.fspath(self.path)
        if not self._custom_fs:
            for prefix in ("file://", "file:", "local://", "local:"):
                if local_path.startswith(prefix):
                    local_path = local_path[len(prefix):]
                    break
        root = os.path.realpath(local_path)
        def inside(path):
            resolved = os.path.realpath(path)
            try:
                if os.path.commonpath((root, resolved)) == root:
                    return resolved
            except ValueError:
                # Different Windows drives have no common path.
                return None
            # realpath can retain the supplied casing on case-insensitive
            # filesystems. Use ancestor identities rather than lowercasing:
            # differently cased siblings remain distinct on other filesystems.
            ancestor = resolved
            while True:
                try:
                    if os.path.samefile(root, ancestor):
                        return resolved
                except OSError:
                    pass
                parent = os.path.dirname(ancestor)
                if parent == ancestor:
                    return None
                ancestor = parent
        direct = os.path.join(self.path, name)
        if (os.path.isfile(direct) or self.fs.exists(direct)) and (
                inside(direct) is not None or local_path != os.fspath(self.path)):
            return self.load_file(direct, is_dependency=is_dependency, _root=root)
        if not self.local_files and self.path:
            guard = TreeTraversal(self._limits, self._budget)
            if not self._custom_fs:
                from ._search import files
                inventory = files(local_path)
            else:
                inventory = []
                for directory, _, names in self.fs.walk(self.path):
                    for entry_name in names:
                        guard.step(0, (len(directory) + len(entry_name)) * 8 + 512, 1)
                        inventory.append(self.fs.sep.join((directory, entry_name)))
            simple_inventory = []
            for entry in inventory:
                guard.step(0, len(entry) * 8 + 128, 1)
                simple_inventory.append(self.fs.sep.join((os.path.dirname(entry), simplify_name(entry))))
            self.local_files, self.local_files_simple = inventory, simple_inventory
        for path in self.local_files:
            if path == name or path.endswith(self.fs.sep + name):
                candidate = inside(path)
                if candidate is not None:
                    return self.load_file(path, is_dependency=is_dependency, _root=root)
        for path in self.local_files:
            simple = os.path.join(os.path.dirname(path), ntpath.basename(path).lower())
            if simple == simplify_name(name) or simple.endswith(self.fs.sep + simplify_name(name)):
                candidate = inside(simple)
                if candidate is not None:
                    return self.load_file(simple, is_dependency=is_dependency, _root=root)
        raise FileNotFoundError(f"File {name} not found in {self.path}")

    @_locked
    def _find_file_exact(self, name, is_dependency=True):
        # The inventory is the same files-first walk used by find_file. Retry
        # only an exact basename, after the ordinary lookup has missed.
        guard = TreeTraversal(self._limits, self._budget)
        root_path = os.fspath(self.path)
        guard.step(0, len(root_path) * 16 + 512, len(root_path))
        root = os.path.realpath(root_path)
        for path in self.local_files:
            guard.step(0, len(path) * 16 + 512, len(path))
            if ntpath.basename(path) == name:
                try:
                    confined_path(path, root)
                except FileNotFoundError:
                    continue
                return self.load_file(path, is_dependency=is_dependency, _root=root)
        return None

    @_locked
    def load_folder(self, path):
        self.load_files(self.fs.sep.join((root, name)) for root, _, names in self.fs.walk(path)
                        for name in names)

    def get(self, key, default=None):
        return getattr(self, key, default)

    def get_cab(self, name):
        return self.cabs.get(ntpath.basename(name).lower())

    def register_cab(self, name, item):
        self.cabs[ntpath.basename(name).lower()] = item
        self._container_index_built = False

    @_locked
    def _build_container_index(self):
        if not self._container_index_built:
            self._container_index_built = True
            try:
                for file in self.cabs.values():
                    if isinstance(file, SerializedFile):
                        file.container.parse_preload_table(_guard=file._native.tree_traversal())
            except MemoryError:
                self._container_index_built = False
                raise

    @property
    def container(self):
        # environment.py:208-226: cabs, rather than all file mappings.
        from .helpers.ContainerHelper import ContainerHelper
        self._build_container_index()
        guard = TreeTraversal(self._limits, self._budget)
        container = []
        for file in self.cabs.values():
            if isinstance(file, SerializedFile) and not file.is_dependency:
                guard.step(0, len(file.container.container) * 256 + 64, 1)
                container.extend(file.container.container)
        return ContainerHelper(container)

    @_locked
    def save(self, pack="none", out_path="output"):
        # Preserve safe relative-name basename collisions and no mkdir, while
        # validating the original asset path before discarding its prefix.
        from ._output import OutputDirectory, output_name
        guard = TreeTraversal(self._limits, self._budget)
        guard.step(0, 256, 1)
        pending = []
        for name, file in self.files.items():
            if getattr(file, "is_changed", False):
                leaf = output_name(_name_text(name), container=True, guard=guard)
                guard.step(0, 128, 1)
                pending.append((leaf, file))
        if not pending:
            return
        with OutputDirectory(out_path, guard=guard) as directory:
            for name, file in pending:
                with directory.open(name, "wb") as output:
                    output.write(file.save(packer=pack))


load = Environment
AssetsManager = Environment

from .helpers.ArchiveStorageManager import set_assetbundle_decrypt_key
from . import streams, export
from . import environment
