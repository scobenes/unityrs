# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/ObjectReader.py, UnityPy/enums/ExtendableEnum.py. See THIRD_PARTY_LICENSES.
"""Python graph wrappers; each owns its native handle and strong parent links."""

import os
from types import SimpleNamespace
from .enums.ClassIDType import ClassIDType

class File:
    def _init(self, native, metadata, parent, environment, dependency):
        self._native = native
        self.__dict__.update(metadata)
        self.name = os.path.basename(self.name)
        self.parent = parent
        self.environment = environment
        self.is_dependency = dependency
        self.is_changed = False
        self.cab_file = "CAB-UnityPy_Mod.resS"
        self.files = {}
        if isinstance(self, SerializedFile):
            self.header = SimpleNamespace(**self.header)
            self.externals = [SimpleNamespace(**entry) for entry in native.externals()]
            self._cache = {}
            self.objects = {}
            for obj in native.objects():
                item = ObjectReader(obj, self)
                self.objects[item.path_id] = item
            self.files = self.objects

    def get(self, key, default=None):
        return getattr(self, key, default)

    def keys(self):
        return self.files.keys()

    def items(self):
        return self.files.items()

    def values(self):
        return self.files.values()

    def __getitem__(self, key):
        return self.files[key]

    def get_assets(self):
        # files/File.py:41-50: a SerializedFile returns inside a generator.
        if isinstance(self, SerializedFile):
            return self
        for file in self.files.values():
            if isinstance(file, (BundleFile, WebFile)):
                yield from file.get_assets()
            elif isinstance(file, SerializedFile):
                yield file

    def get_objects(self):
        # files/File.py:65-74: nested containers have no objects attribute.
        for file in self.files.values():
            if isinstance(file, (BundleFile, WebFile)):
                yield from file.objects
            elif isinstance(file, SerializedFile):
                yield from file.objects.values()
            elif isinstance(file, ObjectReader):
                yield file

    def get_filtered_objects(self, obj_types=None):
        # files/File.py:52-63: return from the generator, yielding nothing.
        if obj_types is None or len(obj_types) == 0:
            return self.get_objects()
        for file in self.files.values():
            if isinstance(file, (BundleFile, WebFile)):
                for obj in file.objects:
                    if obj.type in obj_types:
                        yield obj
            elif isinstance(file, SerializedFile):
                for obj in file.objects.values():
                    if obj.type in obj_types:
                        yield obj

    def __repr__(self):
        return f"<{type(self).__name__}>"


class SerializedFile(File):
    pass


class BundleFile(File):
    pass


class WebFile(File):
    pass


class EndianBinaryReader_Memoryview:
    """Raw resource data. The reader is backed by an immutable Rust handle."""

    def _init(self, native, metadata, parent, environment, dependency):
        self._native = native
        self._position = 0
        self._length = metadata["length"]

    @property
    def Position(self):
        return self._position

    @Position.setter
    def Position(self, value):
        self._native.raw_read(value, 0)
        self._position = value

    @property
    def Length(self):
        return self._length

    @property
    def bytes(self):
        return self._native.raw_data()

    def read(self, size=-1):
        result = self._native.raw_read(self._position, size)
        self._position = result["position"]
        return result["data"]

    read_bytes = read

class ObjectReader:
    def __init__(self, native, assets_file):
        self._native = native
        self.assets_file = assets_file
        self.__dict__.update(native.metadata())
        self.type = ClassIDType(self.class_id)
        self.data = None
        self._read_until = None

    @property
    def version2(self):
        return self.assets_file.header.version

    @property
    def platform(self):
        return self.assets_file.target_platform

    def get_raw_data(self):
        return self._native.raw_data()

    def read_typetree(self, nodes=None, wrap=False, check_read=True, *, as_dict=None):
        if nodes is not None:
            raise ValueError("explicit typetree nodes require the helpers API")
        if as_dict is not None:
            wrap = not as_dict
        if not wrap:
            return self._native.parse(check_read)
        from ._object import build
        data, lists, references, bytes_read, completed, error, charge = self._native.object_data()
        result = build(data, lists, self.assets_file, self.byte_size > 0, references, completed, error, charge)
        if check_read and bytes_read != self.byte_size:
            raise ValueError(f"Expected to read {self.byte_size} bytes, but only read {bytes_read} bytes")
        result.set_object_reader(self)
        return result

    def read(self, check_read=True):
        return self.read_typetree(wrap=True, check_read=check_read)

    def parse_as_object(self, node=None, check_read=True):
        return self.read_typetree(nodes=node, wrap=True, check_read=check_read)

    def get_class(self):
        from .classes.ClassIDTypeToClassMap import ClassIDTypeToClassMap
        return ClassIDTypeToClassMap.get(self.type)

    def parse_as_dict(self, node=None, check_read=True):
        return self.read_typetree(nodes=node, check_read=check_read)

    def get(self, key, default=None):
        return getattr(self, key, default)

    def __getattr__(self, name):
        return None


class EndianBinaryReader_Streamable(EndianBinaryReader_Memoryview):
    pass


EndianBinaryReader = EndianBinaryReader_Memoryview

EndianBinaryReader_Stream = EndianBinaryReader_Streamable
