# Ported from UnityPy 1.25.2 (MIT): files/ObjectReader.py:244-262, files/ObjectReader.py:278-283. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
# Ported from UnityPy 1.25.2 (MIT): files/SerializedFile.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Select the original native source or meter a caller's replacement reader."""
from ._object_reader import ObjectFileReader


def reference_types(file):
    from types import SimpleNamespace
    from weakref import ref
    from .helpers.TypeTreeNode import TypeTreeNode
    from .helpers._bounded_tree import charge_node
    with file._sync_owner._lock:
        if '_ref_types' in file.__dict__:
            return file._ref_types
        rows, guard = file._native.reference_types()
        result = []
        for row in rows:
            guard.step(0, 1024, 1)
            nodes = row['node']
            if nodes is not None:
                built, edges = [], []
                for fields in nodes:
                    guard.step(0, 512, 1)
                    edges.append(fields.pop('_native_children'))
                    fields.pop('_native_node', None)
                    fields.pop('_native_list', None)
                    charge_node(guard, 0, fields['m_Type'], fields['m_Name'])
                    node = TypeTreeNode(**fields)
                    node._tree_source = ref(file._native)
                    built.append(node)
                for node, indices in zip(built, edges):
                    guard.step(0, 64 * len(indices), len(indices))
                    node.m_Children = [built[i] for i in indices]
                row['node'] = built[0] if built else None
            result.append(SimpleNamespace(**row))
        file._ref_types = result
        return result


def set_reference_types(file, value):
    file._ref_types = value


def uses_original(obj):
    reader = obj.reader
    metadata = obj._native.metadata()
    return (type(reader) is ObjectFileReader
            and not {'read', 'read_bytes'} & reader.__dict__.keys()
            and '_ref_types' not in obj.assets_file.__dict__
            and reader._native is obj.assets_file._native
            and reader.endian == obj.assets_file._native.serialized_reader_info()[1]
            and obj.byte_size == metadata['byte_size']
            and obj.byte_start == metadata['byte_start'])


def raw_data(obj):
    if uses_original(obj):
        return obj._native.raw_data()
    reader, position = obj.reader, obj.Position
    guard = obj._native.tree_traversal()
    try:
        obj.reset()
        size = obj.byte_size
        bound = max(0, reader.Length - reader.Position) if size < 0 else size
        guard.step(0, 256 + bound * 4, bound)
        return reader.read_bytes(size)
    finally:
        reader.Position = position


def read_replacement(obj, nodes, wrap, check_read):
    from .helpers.TypeTreeHelper import read_typetree
    reader = obj.reader
    node = obj._get_typetree_node(nodes)
    obj.reset()
    result = read_typetree(node, reader, as_dict=not wrap,
                           assetsfile=obj.assets_file, byte_size=obj.byte_size,
                           check_read=check_read, _guard=obj._native.tree_traversal())
    if wrap:
        result.set_object_reader(obj)
    return result
