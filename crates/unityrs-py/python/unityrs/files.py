# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/ObjectReader.py, UnityPy/enums/ExtendableEnum.py. See THIRD_PARTY_LICENSES.
"""Python graph wrappers; each owns its native handle and strong parent links."""

import os
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
        if isinstance(self, SerializedFile):
            self._enable_type_tree = self.enable_type_tree
            self.header = SimpleNamespace(**self.header)
            if self.resolved_version is not None:
                self.version = UnityVersion.from_str(self.resolved_version)
            if self.target_platform is not None:
                self.target_platform = BuildTarget(self.target_platform)
            else:
                self.target_platform = BuildTarget.UnknownPlatform
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

    def __getattr__(self, name):
        # UnityPy's raw readers return None for an unknown delegated attribute.
        return None


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

    def read_typetree(self, nodes=None, wrap=False, check_read=True):
        if wrap:
            raise NotImplementedError("object mode belongs to the classes API")
        if (nodes is not None or "_node" in self.__dict__
                or self.type_id in self.assets_file._type_nodes or self.assets_file._head_nodes):
            node = self._get_typetree_node(nodes)
            return self._native.parse_nodes(check_read, node._native_rows(
                use_boost=bool(self.byte_size), _guard=self._native.tree_traversal()))
        return self._native.parse(check_read)

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
                node._tree_source = self._native
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
