# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/ObjectReader.py, UnityPy/enums/ExtendableEnum.py, UnityPy/streams/EndianBinaryWriter.py. See THIRD_PARTY_LICENSES.
"""Python graph wrappers; each owns its native handle and strong parent links."""

import os
import struct
import weakref
from collections import namedtuple
from contextlib import nullcontext
from types import SimpleNamespace
from .enums import ClassIDType, BuildTarget
from .helpers.UnityVersion import UnityVersion

# files/File.py:13; files/__init__.py:1-14: the public legacy directory row.
DirectoryInfo = namedtuple('DirectoryInfo', 'path offset size')


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
        self.__dict__.update(native.save_metadata())
        if isinstance(self, SerializedFile):
            self._enable_type_tree = self.enable_type_tree
            self._m_target_platform = self.target_platform
            self.header = SimpleNamespace(**self.header)
            if self.resolved_version is not None:
                self.version = UnityVersion.from_str(self.resolved_version)
            if self.target_platform is not None:
                self.target_platform = BuildTarget(self.target_platform)
            else:
                self.target_platform = BuildTarget.UnknownPlatform
            self.externals = [SimpleNamespace(**entry) for entry in native.externals()]
            self._cache = {}
            self._type_nodes = {}
            self._head_nodes = {}
            self._head_children = {}
            self.objects = {}
            for obj in native.objects():
                item = ObjectReader(obj, self)
                self.objects[item.path_id] = item
            self.files = self.objects

    def get(self, key, default=None):
        return getattr(self, key, default)

    def mark_changed(self):
        # files/File.py:145-148: propagation includes Python-visible parents.
        with self.environment._lock:
            self._native.mark_changed()
            current = self
            while isinstance(current, File):
                current.is_changed = True
                current = current.parent

    def get_writeable_cab(self, name=None):
        # files/File.py:92-121. The current mapping owns writer identity.
        name = name or self.cab_file
        if not name:
            return None
        with self.environment._lock:
            if name in self.files:
                if isinstance(self.files[name], _CabWriter):
                    return self.files[name]
                raise ValueError("This cab already exists and isn't an EndianBinaryWriter")
            guard = self._native.tree_traversal()
            flags, endian = 0, ">"
            for resource_name, resource in self.files.items():
                guard.step(0, 0, 1)
                if resource_name.endswith(".resS"):
                    flags, endian = resource.flags, resource.endian
                    break
            native = self._native.create_cab(_save_text(name, guard), flags, endian)
            writer = _CabWriter(native)
            self.files[name] = writer
            return writer

    def keys(self):
        return self.files.keys()

    def items(self):
        return self.files.items()

    def values(self):
        return self.files.values()

    def __getitem__(self, key):
        return self.files[key]

    @property
    def container(self):
        # files/File.py:123-125: later children replace earlier equal paths.
        guard = self._native.tree_traversal()
        guard.step(0, 256, 1)
        result = {}
        pending = [(iter(self.files.values()), 0)]
        while pending:
            children, depth = pending[-1]
            try:
                file = next(children)
            except StopIteration:
                pending.pop()
                continue
            if isinstance(file, SerializedFile):
                for path, pointer in file.container.items():
                    guard.step(depth, 256, 1)
                    result[path] = pointer
            elif isinstance(file, File):
                guard.step(depth + 1, 256, 1)
                pending.append((iter(file.files.values()), depth + 1))
        return result

    def get_assets(self):
        from ._file_iteration import assets
        return assets(self)

    def get_objects(self):
        from ._file_iteration import objects
        return objects(self)

    def get_filtered_objects(self, obj_types=None):
        from ._file_iteration import objects
        return objects(self, obj_types)

    def __repr__(self):
        return f"<{type(self).__name__}>"


def _save_text(value, guard):
    if value is None:
        return None
    size = str.__len__(value) if isinstance(value, str) else len(value)
    guard.step(0, size * 4 + 64, 0)
    return value.encode("utf-8", "surrogateescape")


def _writer_text(value, guard):
    # UnityPy's writer calls `value.encode(...)` (streams/EndianBinaryWriter.py:84-92),
    # so a value without one, None or an int, raises its AttributeError there.
    encode = value.encode
    guard.step(0, (str.__len__(value) if isinstance(value, str) else 0) * 4 + 64, 0)
    return encode("utf8", "surrogateescape")


class SerializedFile(File):
    @property
    def reader(self):
        with (self.environment._lock if self.environment is not None else nullcontext()):
            if '_reader' not in self.__dict__:
                from ._object_reader import ObjectFileReader
                guard = self._native.tree_traversal()
                guard.step(0, 2048, 1)
                self._reader = ObjectFileReader(self._native)
            return self._reader

    @reader.setter
    def reader(self, value):
        with (self.environment._lock if self.environment is not None else nullcontext()):
            self._reader = value

    @property
    def container(self):
        # files/SerializedFile.py:235-328: only the first AssetBundle matters.
        if '_container' not in self.__dict__:
            from .helpers.ContainerHelper import ContainerHelper
            guard = self._native.tree_traversal()
            guard.step(0, 512, 1)
            bundle = next((obj for obj in self.objects.values() if obj.type == ClassIDType.AssetBundle), None)
            value = bundle.read() if bundle is not None else []
            if bundle is not None:
                for path, _ in value.m_Container:
                    guard.step(0, 1024 + str.__len__(path) * 4, 1)
            self._container = ContainerHelper(value)
        return self._container

    def save(self, packer=None):
        with self.environment._lock:
            guard = self._native.tree_traversal()
            count = len(self.objects)
            guard.step(0, 512 + count * 256, count * (count.bit_length() + 1))
            # Sort values, preserving distinct mapping entries with equal IDs.
            objects = sorted(self.objects.values(), key=lambda obj: obj.path_id)
            rows = []
            for obj in objects:
                data = obj.data if obj.data is not None else obj.get_raw_data()
                guard.step(0, len(data) + 256, 1)
                rows.append((obj._native, obj.path_id, obj.type_id, obj.class_id,
                             obj.is_destroyed, obj.is_stripped,
                             obj._native.save_metadata()["script_type_index"], data))
            externals = []
            for entry in self.externals:
                guard.step(0, 256 + len(entry.guid or b""), 1)
                externals.append((_save_text(entry.temp_empty, guard), entry.guid,
                                  entry.type, _save_text(entry.path, guard)))
            reserved = self.header.reserved
            guard.step(0, len(reserved) if reserved is not None else 0, 0)
            metadata = ((self.header.version, self.header.endian, reserved, self.unknown),
                        _save_text(self.unity_version, guard), self._m_target_platform,
                        self._enable_type_tree, self.big_id_enabled,
                        _save_text(self.userInformation, guard))
            return self._native.save_serialized_snapshot(metadata, rows, externals)

    def get_writeable_cab(self, name="CAB-UnityPy_Mod.resS"):
        # files/SerializedFile.py:362-387. Creation does not mark changed.
        if not isinstance(self.parent, (BundleFile, WebFile)):
            return None
        import uuid
        with self.environment._lock:
            guard = self._native.tree_traversal()
            size = str.__len__(name) if isinstance(name, str) else len(name)
            own_size = str.__len__(self.name) if isinstance(self.name, str) else len(self.name)
            guard.step(0, (own_size + size + 10) * 4 + 256, 1)
            path = f"archive:/{self.name}/{name}"
            encoded = _save_text(path, guard)
            self._native.validate_external_path(encoded)
            writer = self.parent.get_writeable_cab(name)
            writer.path = path
            if not any(writer.path == entry.path for entry in self.externals):
                guard.step(0, 512, 1)
                guid = uuid.uuid1().urn[-16:].encode("ascii")
                self.externals.append(SimpleNamespace(temp_empty="", guid=guid, path=writer.path, type=0))
            return writer


class BundleFile(File):
    def _save_header(self, guard):
        guard.step(0, 256, 1)
        return (_save_text(self.signature, guard), self.version,
                _save_text(self.version_player, guard), _save_text(self.version_engine, guard),
                self._uses_block_alignment, self.dataflags, self._block_info_flags)

    def _save_rows(self, guard):
        guard.step(0, 256, 1)
        rows = []
        for name, file in self.files.items():
            guard.step(0, 256, 1)
            encoded_name = _save_text(name, guard)
            data = file.bytes if isinstance(file, (EndianBinaryReader_Memoryview, _CabWriter)) else file.save()
            guard.step(0, len(data), 0)
            rows.append((encoded_name, file.flags, data))
        return rows

    def save(self, packer=None):
        with self.environment._lock:
            if (self.signature == "UnityFS" and packer
                    and packer not in ("none", "original", "lz4", "lzma", "lz4hc")
                    and not isinstance(packer, tuple)):
                raise NotImplementedError("UnityFS - Packer:", packer)
            guard = self._native.tree_traversal()
            return self._native.save_bundle_entries("save", self._save_rows(guard), packer, (64, 64), self._save_header(guard))

    @staticmethod
    def _body_offset(data):
        position = data.index(b"\0") + 5
        position = data.index(b"\0", position) + 1
        return data.index(b"\0", position) + 1

    def save_fs(self, writer, data_flag, block_info_flag):
        with self.environment._lock:
            guard = self._native.tree_traversal()
            data = self._native.save_bundle_entries("fs", self._save_rows(guard), None, (data_flag, block_info_flag), self._save_header(guard))
            head = self._body_offset(data)
            _, compressed, uncompressed, flags = struct.unpack_from(">qIII", data, head)
            start = head + 20 + (self.signature != "UnityFS")
            if self._uses_block_alignment:
                start += (-start) % 16
            guard.step(0, 1024, 0)
            view = memoryview(data)
            if flags & 0x80:
                if flags & 0x200:
                    start += (-start) % 16
                blocks, payload = view[-compressed:], view[start:-compressed]
            else:
                blocks = view[start:start + compressed]
                start += compressed
                if flags & 0x200:
                    start += (-start) % 16
                payload = view[start:]
            header_position = writer.Position
            writer.write_long(0)
            writer.write_u_int(compressed)
            writer.write_u_int(uncompressed)
            writer.write_u_int(flags)
            if self.signature != "UnityFS":
                writer.write_byte(0)
            if self._uses_block_alignment:
                writer.align_stream(16)
            if flags & 0x80:
                if flags & 0x200:
                    writer.align_stream(16)
                writer.write(payload)
                writer.write(blocks)
            else:
                writer.write(blocks)
                if flags & 0x200:
                    writer.align_stream(16)
                writer.write(payload)
            end_position = writer.Position
            writer.Position = header_position
            writer.write_long(end_position)
            writer.Position = end_position

    def save_web_raw(self, writer):
        with self.environment._lock:
            guard = self._native.tree_traversal()
            data = self._native.save_bundle_entries("web_raw", self._save_rows(guard), None, (64, 64), self._save_header(guard))
            start = self._body_offset(data)
            guard.step(0, 1024, 0)
            view = memoryview(data)
            extra = 20 if self.version >= 4 else 0
            fields = struct.unpack_from(">IIIIII", data, start + extra)
            original_header, compressed, uncompressed = fields[1], fields[4], fields[5]
            header_size = writer.Position + extra + 24 + 4 * (self.version >= 2) + 4 * (self.version >= 3)
            header_size += (-header_size) % 4
            if extra:
                writer.write_bytes(view[start:start + 16])
                writer.write_u_int(struct.unpack_from(">I", data, start + 16)[0])
            writer.write_u_int(header_size + compressed)
            writer.write_u_int(header_size)
            writer.write_u_int(1)
            writer.write_int(1)
            writer.write_u_int(compressed)
            writer.write_u_int(uncompressed)
            if self.version >= 2:
                writer.write_u_int(header_size + compressed)
            if self.version >= 3:
                writer.write_u_int(struct.unpack_from(">I", data, start + extra + 28)[0])
            writer.align_stream(4)
            writer.write(view[original_header:original_header + compressed])


class WebFile(File):
    def save(self, files=None, packer="none", signature="UnityWebData1.0"):
        with self.environment._lock:
            guard = self._native.tree_traversal()
            guard.step(0, 256, 1)
            rows = []
            for name, file in (files or self.files).items():
                guard.step(0, 256, 1)
                encoded_name = _save_text(name, guard)
                data = file.bytes if isinstance(file, EndianBinaryReader_Memoryview) else file.save()
                guard.step(0, len(data), 0)
                rows.append((encoded_name, data))
            return self._native.save_web_entries(
                rows, packer or self.packer, _save_text(signature, guard))


class _CabWriter:
    def __init__(self, native):
        self._native = native
        self.__dict__.update(native.metadata())
        self.Position = 0

    @property
    def bytes(self):
        return self._native.data()

    @property
    def Length(self):
        return self._native.length()

    def write(self, data):
        count = self._native.write_bytes(data, self.Position)
        self.Position += count
        return count

    write_bytes = write

    def write_byte(self, value):
        self.write(struct.pack(self.endian + "b", value))

    def write_u_byte(self, value):
        self.write(struct.pack(self.endian + "B", value))

    def write_short(self, value):
        self.write(struct.pack(self.endian + "h", value))

    def write_int(self, value):
        self.write(struct.pack(self.endian + "i", value))

    def write_long(self, value):
        self.write(struct.pack(self.endian + "q", value))

    def write_u_short(self, value):
        self.write(struct.pack(self.endian + "H", value))

    def write_u_int(self, value):
        self.write(struct.pack(self.endian + "I", value))

    def write_u_long(self, value):
        self.write(struct.pack(self.endian + "Q", value))

    def write_float(self, value):
        self.write(struct.pack(self.endian + "f", value))

    def write_double(self, value):
        self.write(struct.pack(self.endian + "d", value))

    def write_boolean(self, value):
        self.write(struct.pack(self.endian + "?", value))

    def write_string_to_null(self, value):
        guard = self._native.tree_traversal()
        self.write(_writer_text(value, guard))
        self.write(b"\0")

    def write_aligned_string(self, value):
        guard = self._native.tree_traversal()
        encoded = _writer_text(value, guard)
        self.write_int(len(encoded))
        self.write(encoded)
        self.align_stream(4)

    def write_array(self, command, value, write_length=True):
        if write_length:
            self.write_int(len(value))
        for item in value:
            command(item)

    def write_byte_array(self, value):
        self.write_int(len(value))
        self.write(value)

    def write_boolean_array(self, value):
        self.write_array(self.write_boolean, value)

    def write_u_short_array(self, value):
        self.write_array(self.write_u_short, value)

    def write_int_array(self, value, write_length=False):
        return self.write_array(self.write_int, value, write_length)

    def write_u_int_array(self, value, write_length=False):
        return self.write_array(self.write_u_int, value, write_length)

    def write_float_array(self, value, write_length=False):
        return self.write_array(self.write_float, value, write_length)

    def write_string_array(self, value):
        self.write_array(self.write_aligned_string, value)

    def align_stream(self, alignment=4):
        count = (alignment - self.Position % alignment) % alignment
        self._native.write_zeroes(self.Position, count)
        self.Position += count

    def save(self):
        return self.bytes


class EndianBinaryReader_Memoryview:
    """Raw resource data. The reader is backed by an immutable Rust handle."""

    def _init(self, native, metadata, parent, environment, dependency):
        self._native = native
        self._position = 0
        self._length = metadata["length"]
        self.endian = ">"

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
    from ._monobehaviour import parse_monobehaviour_head, generate_monobehaviour_node

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
    def version(self):
        return self.assets_file.version

    @property
    def platform(self):
        return self.assets_file.target_platform

    def get_raw_data(self):
        return self._native.raw_data()

    def set_raw_data(self, data):
        # files/ObjectReader.py:181-184: reads retain the original byte source.
        with self.assets_file.environment._lock:
            try:
                self._native.set_raw_data(data)
            except (TypeError, ValueError, BufferError):
                # Keep the native mirror for valid buffers, but Python owns
                # the replacement. Writer validation belongs to save(), even
                # for values that do not implement the buffer protocol.
                pass
            except MemoryError:
                # A full meter can refuse even the snapshot's fixed overhead
                # before checking the buffer protocol. Invalid replacements
                # still need no mirror; valid ones retain the budget refusal.
                try:
                    memoryview(data)
                except (TypeError, ValueError, BufferError):
                    pass
                else:
                    raise
            self.data = data
            self.assets_file.mark_changed()

    def save_typetree(self, tree, nodes=None, writer=None):
        # files/ObjectReader.py:264-276: explicit node selection also applies to writes.
        with self.assets_file.environment._lock:
            node = self._get_typetree_node(nodes)
            rows = node._native_rows(use_boost=False, _guard=self._native.tree_traversal())
            data = self._native.save_typetree(
                tree, rows, writer.Position if writer else 0, writer.endian if writer else None)
            if writer:
                guard = self._native.tree_traversal()
                extent = max(getattr(writer, "Length", 0), writer.Position + len(data))
                guard.step(0, 2 * extent + 512, 0)
                writer.write(data)
                data = writer.bytes
            self._native.set_raw_data(data)
            self.data = data
            self.assets_file.mark_changed()
            return data

    def patch(self, obj, nodes=None, writer=None):
        # files/ObjectReader.py:316-322: retain keyword forwarding and return.
        return self.save_typetree(obj, nodes=nodes, writer=writer)

    def write(self, header, writer, data_writer):
        # files/ObjectReader.py:132-179: object metadata and the selected payload.
        guard = self._native.tree_traversal()
        guard.step(0, 1024, 0)
        data = self.data if self.data is not None else self.get_raw_data()
        try:
            size = memoryview(data).nbytes
        except TypeError:
            size = len(data)
        guard.step(0, 2 * (data_writer.Position + size) + 512, 0)
        if self.assets_file.big_id_enabled:
            writer.write_long(self.path_id)
        elif header.version < 14:
            writer.write_int(self.path_id)
        else:
            writer.align_stream()
            writer.write_long(self.path_id)
        if header.version >= 22:
            writer.write_long(data_writer.Position)
        else:
            writer.write_u_int(data_writer.Position)
        writer.write_u_int(len(data))
        data_writer.write(data)
        writer.write_int(self.type_id)
        if header.version < 16:
            writer.write_u_short(self.class_id)
        if header.version < 11:
            assert self.is_destroyed is not None
            writer.write_u_short(self.is_destroyed)
        if 11 <= header.version < 17:
            index = self._native.save_metadata()["script_type_index"]
            assert index is not None
            writer.write_short(index)
        if header.version in (15, 16):
            assert self.is_stripped is not None
            writer.write_byte(self.is_stripped)

    def read_typetree(self, nodes=None, wrap=False, check_read=True, *, as_dict=None):
        if as_dict is not None:
            wrap = not as_dict
        with (self.assets_file.environment._lock if self.assets_file.environment is not None else nullcontext()):
            rows = None
            if (nodes is not None or (self.class_id == 114 and not self._has_embedded_nodes) or "_node" in self.__dict__
                    or self.type_id in self.assets_file._type_nodes or self.assets_file._head_nodes):
                node = self._get_typetree_node(nodes)
                rows = node._native_rows(
                    use_boost=bool(self.byte_size), _guard=self._native.tree_traversal())
            if not wrap:
                value, count = self._native.parse_with_position(check_read, rows)
                self.Position = self.byte_start + count
                return value
            return self._read_object(rows, check_read)

    def _read_object(self, rows, check_read):
        from ._object import build
        data, lists, references, bytes_read, completed, error, charge = self._native.object_data(rows)
        self.Position = self.byte_start + bytes_read
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

    @property
    def reader(self):
        if '_reader' not in self.__dict__:
            self._reader = self.assets_file.reader
        return self._reader

    @reader.setter
    def reader(self, value):
        self._reader = value

    @property
    def Position(self):
        return self.reader.Position

    @Position.setter
    def Position(self, value):
        self.reader.Position = value

    def reset(self):
        self.reader.Position = self.byte_start

    @property
    def container(self):
        # files/ObjectReader.py:199-204: preload index, then own-file path id.
        env = self.assets_file.environment
        if env is not None:
            env._build_container_index()
        return self.assets_file.container.path_dict.get(self.path_id)

    def parse_as_dict(self, node=None, check_read=True):
        return self.read_typetree(nodes=node, check_read=check_read)

    def _get_typetree_node(self, node=None, *, _guard=None):
        # Export releases the GIL. The lock must cover the check, rebuild and
        # publication together, including HEAD identities shared across types.
        with (self.assets_file.environment._lock if self.assets_file.environment is not None else nullcontext()):
            from ._monobehaviour import generated_node
            base = self._get_typetree_node_locked(node, _guard)
            return generated_node(self, base) if node is None else base

    def _get_typetree_node_locked(self, node, guard):
        # files/ObjectReader.py:285-307: explicit nodes precede the embedded tree.
        from .helpers.TypeTreeNode import TypeTreeNode
        if isinstance(node, TypeTreeNode):
            return node
        if isinstance(node, list):
            return TypeTreeNode.from_list(node, _guard=guard or self._native.tree_traversal())
        if node is not None:
            raise ValueError("nodes must be a list[dict] or TypeTreeNode")
        shared = self.assets_file._type_nodes
        if self.type_id in shared:
            self._node = shared[self.type_id]
            return self._node
        if "_node" not in self.__dict__:
            rows, staging = self._native.nodes()
            from .helpers._bounded_tree import charge_node
            nodes, edges = [], []
            head_nodes, head_children = self.assets_file._head_nodes, self.assets_file._head_children
            new_nodes, new_children = {}, {}
            for row in rows:
                charge_node(staging, 0, row["m_Type"], row["m_Name"])
                children = row.pop("_native_children")
                staging.step(0, 64 * len(children), len(children))
                identity = row.pop("_native_node", None)
                list_identity = row.pop("_native_list", None)
                if identity is None:
                    node = TypeTreeNode(**row)
                    fill = True
                else:
                    node = head_nodes.get(identity, new_nodes.get(identity))
                    if node is not None:
                        nodes.append(node)
                        edges.append(())
                        continue
                    node = TypeTreeNode(**row)
                    new_nodes[identity] = node
                    children_list = head_children.get(list_identity, new_children.get(list_identity))
                    fill = children_list is None
                    if fill:
                        children_list = []
                        new_children[list_identity] = children_list
                    node.m_Children = children_list
                node._tree_source = weakref.ref(self.assets_file._native)
                nodes.append(node)
                edges.append(children if fill else ())
            for node, children in zip(nodes, edges):
                node.m_Children.extend(nodes[child] for child in children)
            # Publish complete graphs. A refused dict growth can partially
            # insert entries, so undo just this reconstruction's additions.
            try:
                head_nodes.update(new_nodes)
                head_children.update(new_children)
            except BaseException:
                for identity in new_nodes:
                    head_nodes.pop(identity, None)
                for identity in new_children:
                    head_children.pop(identity, None)
                raise
            self._node = nodes[0]
            shared[self.type_id] = self._node
        return self._node

    def peek_name(self):
        # files/ObjectReader.py:189-197: retain the shared peek-node cache.
        peek = self._get_typetree_node().get_name_peek_node()
        if peek:
            node, key = peek
            return self.parse_as_dict(node, check_read=False)[key]
        return None

    def dump_typetree_structure(self, nodes=None, indent="  "):
        guard = self._native.tree_traversal()
        return self._get_typetree_node(nodes, _guard=guard).dump_structure(indent, _guard=guard)

    def get(self, key, default=None):
        return getattr(self, key, default)

    def __getattr__(self, name):
        # files/ObjectReader.py:223-225: absent names deliberately return None.
        if name == 'reader':
            return None
        reader = self.reader
        if hasattr(reader, name):
            return getattr(reader, name)


class EndianBinaryReader_Streamable(EndianBinaryReader_Memoryview):
    pass


EndianBinaryReader = EndianBinaryReader_Memoryview

EndianBinaryReader_Stream = EndianBinaryReader_Streamable
