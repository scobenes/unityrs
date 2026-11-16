# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/ObjectReader.py, UnityPy/enums/ExtendableEnum.py. See THIRD_PARTY_LICENSES.
"""Python graph wrappers; each owns its native handle and strong parent links."""

import os
import struct
import weakref
from types import SimpleNamespace
from .enums import ClassIDType, BuildTarget
from .helpers.UnityVersion import UnityVersion


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
        # files/File.py:92-121. Keep one wrapper for each live native writer.
        name = name or self.cab_file
        if not name:
            return None
        with self.environment._lock:
            if name in self.files:
                if isinstance(self.files[name], _CabWriter):
                    return self.files[name]
                raise ValueError("This cab already exists and isn't an EndianBinaryWriter")
            guard = self._native.tree_traversal()
            native = self._native.get_writeable_cab(_save_text(name, guard), None)
            if native is None:
                return None
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


def _save_text(value, guard):
    if value is None:
        return None
    size = str.__len__(value) if isinstance(value, str) else len(value)
    guard.step(0, size * 4 + 64, 0)
    return value.encode("utf-8", "surrogateescape")


class SerializedFile(File):
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
            guid = uuid.uuid1().urn[-16:].encode("ascii")
            guard = self._native.tree_traversal()
            native = self._native.get_writeable_cab(_save_text(name, guard), list(guid))
            writer = self.parent.files.get(name)
            if not isinstance(writer, _CabWriter):
                writer = _CabWriter(native)
                self.parent.files[name] = writer
            else:
                writer.__dict__.update(native.metadata())
            if not any(writer.path == entry.path for entry in self.externals):
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
    def _body(data):
        position = data.index(b"\0") + 5
        position = data.index(b"\0", position) + 1
        return data[data.index(b"\0", position) + 1:]

    def save_fs(self, writer, data_flag, block_info_flag):
        with self.environment._lock:
            guard = self._native.tree_traversal()
            data = self._native.save_bundle_entries("fs", self._save_rows(guard), None, (data_flag, block_info_flag), self._save_header(guard))
            head = len(data) - len(self._body(data))
            _, compressed, uncompressed, flags = struct.unpack_from(">qIII", data, head)
            start = head + 20 + (self.signature != "UnityFS")
            if self._uses_block_alignment:
                start += (-start) % 16
            if flags & 0x80:
                if flags & 0x200:
                    start += (-start) % 16
                blocks, payload = data[-compressed:], data[start:-compressed]
            else:
                blocks = data[start:start + compressed]
                start += compressed
                if flags & 0x200:
                    start += (-start) % 16
                payload = data[start:]
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
            start = len(data) - len(self._body(data))
            extra = 20 if self.version >= 4 else 0
            fields = struct.unpack_from(">IIIIII", data, start + extra)
            original_header, compressed, uncompressed = fields[1], fields[4], fields[5]
            header_size = writer.Position + extra + 24 + 4 * (self.version >= 2) + 4 * (self.version >= 3)
            header_size += (-header_size) % 4
            if extra:
                writer.write_bytes(data[start:start + 16])
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
            writer.write(data[original_header:original_header + compressed])


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

    def write_int(self, value):
        self.write(struct.pack(self.endian + "i", value))

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
            self._native.set_raw_data(data)
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
                writer.write(data)
                data = writer.bytes
            self._native.set_raw_data(data)
            self.data = data
            self.assets_file.mark_changed()
            return data

    def write(self, header, writer, data_writer):
        # files/ObjectReader.py:132-179: object metadata and the selected payload.
        if self.assets_file.big_id_enabled:
            writer.write_long(self.path_id)
        elif header.version < 14:
            writer.write_int(self.path_id)
        else:
            writer.align_stream()
            writer.write_long(self.path_id)
        data = self.data if self.data is not None else self.get_raw_data()
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
        with self.assets_file.environment._lock:
            rows = None
            if (nodes is not None or "_node" in self.__dict__
                    or self.type_id in self.assets_file._type_nodes or self.assets_file._head_nodes):
                node = self._get_typetree_node(nodes)
                rows = node._native_rows(
                    use_boost=bool(self.byte_size), _guard=self._native.tree_traversal())
            if not wrap:
                if rows is not None:
                    return self._native.parse_nodes(check_read, rows)
                return self._native.parse(check_read)
            return self._read_object(rows, check_read)

    def _read_object(self, rows, check_read):
        from ._object import build
        data, lists, references, bytes_read, completed, error, charge = self._native.object_data(rows)
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

    def _get_typetree_node(self, node=None, *, _guard=None):
        # Export releases the GIL. The lock must cover the check, rebuild and
        # publication together, including HEAD identities shared across types.
        with self.assets_file.environment._lock:
            return self._get_typetree_node_locked(node, _guard)

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
        return None


class EndianBinaryReader_Streamable(EndianBinaryReader_Memoryview):
    pass


EndianBinaryReader = EndianBinaryReader_Memoryview

EndianBinaryReader_Stream = EndianBinaryReader_Streamable
