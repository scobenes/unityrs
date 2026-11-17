# Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeHelper.py:1-458. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
# Ported from UnityPy git 417d595 (MIT): helpers/TypeTreeHelper.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
# UnityPyBoost/TypeTreeHelper.cpp:962-1056 supplies accelerated defaults and argument rules.
"""Bounded standalone readers and writers, retaining the supplied stream cursor."""
from attrs import define
from ._primitive_maps import (
    FUNCTION_READ_MAP, FUNCTION_READ_MAP_ARRAY, FUNCTION_WRITE_MAP,
    kAlignBytes, clean_name, metaflag_is_aligned,
)
from ._bounded_tree import guard_for, walk, text_bound
from .._native import standalone_tree_read


_BOUNDED_READ_DEFAULTS = {ty: FUNCTION_READ_MAP[ty] for ty in ('string', 'TypelessData')}


@define(slots=True)
class TypeTreeConfig:
    as_dict: bool
    assetsfile: object = None
    has_registry: bool = False

    def copy(self):
        return TypeTreeConfig(self.as_dict, self.assetsfile, self.has_registry)


def get_ref_type_node(ref_object, assetfile, *, _guard=None):
    typ = ref_object['type']
    cls, ns, asm = ((typ['class'], typ['ns'], typ['asm']) if isinstance(typ, dict)
                    else (getattr(typ, 'class'), typ.ns, typ.asm))
    if not assetfile or not assetfile.ref_types:
        raise ValueError('SerializedFile has no ref_types')
    if cls == '':
        return None
    guard = _source_guard(None, None, None, _guard, assetfile)
    for ref in assetfile.ref_types:
        guard.step(0, 128, 1)
        for expected, name in ((cls, 'm_ClassName'), (ns, 'm_NameSpace'), (asm, 'm_AssemblyName')):
            actual = getattr(ref, name)
            work = sum(text_bound(value) for value in (expected, actual) if isinstance(value, str))
            guard.step(0, 128, work)
            if not expected == actual:
                break
        else:
            node = ref.node
            if node is not None:
                for _ in walk(node, guard):
                    pass
            return node
    raise ValueError(f'Referenced type not found: {cls} {ns} {asm}')


def get_annotation_keys(cls):
    return set(cls.__annotations__)


class _ReadGuard:
    def __init__(self, guard):
        self.guard, self.total = guard, 0
        self.discards, self.zero_size = 0, 0

    def step(self, depth, memory, work):
        self.guard.step(depth, memory, work)

    def value_depth(self, depth):
        self.guard.value_depth(depth)

    def values(self, count):
        self.total += count
        self.guard.read_values(self.total, count)

    def discard(self):
        self.discards += 1

    def element(self, reader, before):
        position, discards = before
        if reader.Position <= position and self.discards > discards:
            self.zero_size += 1
            self.guard.read_zero_size(self.zero_size)


def _source_guard(node, limits, budget, guard, assetsfile=None, reader=None):
    if guard is None and limits is None and budget is None:
        for source in (reader, assetsfile):
            native = getattr(source, '_native', None)
            if native is not None:
                guard = native.tree_traversal()
                break
    return guard_for(node, limits, budget, guard)


def _new_guard(node, limits, budget, guard, assetsfile=None, reader=None):
    guard = _source_guard(node, limits, budget, guard, assetsfile, reader)
    # Validate all edges before parsing, including cycles and repeated DAG edges.
    for _ in walk(node, guard):
        pass
    return guard


# The pinned callable takes object mode when the flag is omitted; the public
# read_typetree helper passes its own as_dict flag explicitly.
def read_typetree_boost(data, node, endian, as_dict=False, assetsfile=None, classes=None,
                        *, limits=None, budget=None, _guard=None):
    root_node = node
    if not as_dict and classes is None:
        raise ValueError('classes must be set if not as dict')
    guard = _new_guard(root_node, limits, budget, _guard, assetsfile)
    rows = root_node._native_rows(_guard=guard)
    source_ref = getattr(root_node, '_tree_source', None)
    owner = source_ref() if source_ref is not None else None
    from ._reference_input import reference_input
    references = reference_input(root_node, assetsfile, guard)
    value, lists, refs, consumed, completed, error, construction = standalone_tree_read(
        data, rows, endian, as_dict, getattr(assetsfile, '_native', None), limits, budget, owner, references, guard)
    if not as_dict:
        from .._object import build
        value = build(value, lists, assetsfile, True, refs, completed, error,
                      charge=construction, class_namespace=classes, reference_occurrences=True)
    return value, consumed


def read_typetree(root_node, reader, as_dict=True, byte_size=None, check_read=True, assetsfile=None,
                  *, limits=None, budget=None, _guard=None):
    guard = _new_guard(root_node, limits, budget, _guard, assetsfile, reader)
    if byte_size and read_typetree_boost:
        bound = max(0, reader.Length - reader.Position) if byte_size < 0 else byte_size
        guard.step(0, 256 + bound * 4, bound)
        data = reader.read_bytes(byte_size)
        if not as_dict:
            from .. import classes
        else:
            classes = None
        value, consumed = read_typetree_boost(data, root_node, reader.endian, as_dict, assetsfile, classes,
                                             limits=limits, budget=budget, _guard=guard)
    else:
        start = reader.Position
        guard = _ReadGuard(guard)
        value = _drive(_read(root_node, reader, TypeTreeConfig(as_dict, assetsfile), guard), guard)
        consumed = reader.Position - start
    if check_read and consumed != byte_size:
        raise ValueError(f'Expected to read {byte_size} bytes, but only read {consumed} bytes')
    return value


def _drive(generator, guard):
    stack = [generator]
    value = None
    while stack:
        guard.step(0, 128, 1)
        try:
            child = stack[-1].send(value)
            stack.append(child)
            value = None
        except StopIteration as done:
            stack.pop()
            value = done.value
    return value


def _primitive(node, reader, guard, array_size=None):
    ty = node.m_Type
    if array_size is not None:
        guard.step(0, 256 + max(array_size, 0) * 128, max(array_size, 0))
        return FUNCTION_READ_MAP_ARRAY[ty](reader, array_size)
    if ty in _BOUNDED_READ_DEFAULTS and FUNCTION_READ_MAP[ty] is _BOUNDED_READ_DEFAULTS[ty]:
        return _bounded_primitive(ty, reader, guard)
    return FUNCTION_READ_MAP[ty](reader)


def _bounded_primitive(ty, reader, guard):
    length = reader.read_int()
    if ty == 'string':
        if not 0 < length <= reader.Length - reader.Position:
            return ''
        guard.step(0, 256 + length * 5, length)
        value = bytes(reader.read_bytes(length)).decode('utf8', 'surrogateescape')
        reader.align_stream()
        return value
    bound = max(length, reader.Length - reader.Position) if length < 0 else length
    guard.step(0, 256 + max(bound, 0) * 4, max(bound, 0))
    start = reader.Position
    value = reader.read(length)
    if reader.Position < start:
        guard.discard()
    return value


def _read(node, reader, config, guard, array_size=None, depth=1):
    guard.value_depth(depth)
    guard.values(1 if array_size is None else max(array_size, 0))
    align = metaflag_is_aligned(node.m_MetaFlag)
    ty = node.m_Type
    guard.step(0, 512, 1)
    if array_size is not None:
        guard.step(0, 256 + max(array_size, 0) * 128, max(array_size, 0))
        if ty in FUNCTION_READ_MAP_ARRAY:
            value = _primitive(node, reader, guard, array_size)
        else:
            if ty != 'ReferencedObject':
                guard.value_depth(depth + 1)  # The array's list comprehension.
            value = []
            if ty == 'pair':
                first, second = node.m_Children[0], node.m_Children[1]
            if node.m_Children and node.m_Children[0].m_Type == 'Array':
                subtype = node.m_Children[0].m_Children[1]
                align |= metaflag_is_aligned(node.m_Children[0].m_MetaFlag)
            for _ in range(array_size):
                before = reader.Position, guard.discards
                if ty == 'string':
                    item = _bounded_primitive(ty, reader, guard)
                elif ty == 'TypelessData':
                    # HEAD repairs the aligned byte-array call for later files.
                    if getattr(getattr(config.assetsfile, 'header', None), 'version', 0) >= 23:
                        item = _primitive(node, reader, guard)
                    else:
                        item = reader.read_bytes()
                elif ty == 'pair':
                    parts = []
                    for child in (first, second):
                        if child.m_Type in FUNCTION_READ_MAP:
                            guard.values(1)
                        parts.append(_primitive(child, reader, guard) if child.m_Type in FUNCTION_READ_MAP
                                     else (yield _read(child, reader, config, guard, depth=depth + 3)))
                    item = tuple(parts)
                elif node.m_Children and node.m_Children[0].m_Type == 'Array':
                    count = reader.read_int()
                    item = (yield _array(subtype, reader, config, guard, count, depth + 2))
                else:
                    item = (yield _class(node, reader, config, guard,
                                        depth if ty == 'ReferencedObject' else depth + 2, array=True))
                value.append(item)
                guard.element(reader, before)
    elif ty in FUNCTION_READ_MAP:
        value = _primitive(node, reader, guard)
    elif ty == 'pair':
        first = yield _read(node.m_Children[0], reader, config, guard, depth=depth + 1)
        second = yield _read(node.m_Children[1], reader, config, guard, depth=depth + 1)
        value = (first, second)
    elif ty == 'ReferencedObject':
        value = yield _class(node, reader, config, guard, depth)
    elif node.m_Children and node.m_Children[0].m_Type == 'Array':
        align |= metaflag_is_aligned(node.m_Children[0].m_MetaFlag)
        size = reader.read_int()
        if size < 0:
            raise ValueError('Negative length read from TypeTree')
        value = yield _array(node.m_Children[0].m_Children[1], reader, config, guard, size, depth + 1)
    else:
        value = yield _class(node, reader, config, guard, depth)
    if align:
        reader.align_stream()
    return value


def _array(node, reader, config, guard, size, depth):
    guard.value_depth(depth)
    guard.step(0, 256 + max(size, 0) * 128, max(size, 0))
    if metaflag_is_aligned(node.m_MetaFlag):
        return (yield _read(node, reader, config, guard, size, depth=depth))
    value = []
    for _ in range(size):
        before = reader.Position, guard.discards
        value.append((yield _read(node, reader, config, guard, depth=depth + 1)))
        guard.element(reader, before)
    return value


def _class(node, reader, config, guard, depth, array=False):
    guard.value_depth(depth)
    value = {}
    reference = node.m_Type == 'ReferencedObject'
    for child in node.m_Children:
        guard.step(0, 256 + text_bound(child.m_Name), 1)
        if reference and child.m_Type == 'ReferencedObjectData':
            child_node = get_ref_type_node(value, config.assetsfile, _guard=guard)
            if child_node is None and not array:
                guard.values(1)
                guard.discard()
                continue
        else:
            child_node = child
        if not array and child.m_Type == 'ManagedReferencesRegistry':
            if config.has_registry:
                guard.values(1)
                guard.discard()
                continue
            config = config.copy()
            config.has_registry = True
        key = child.m_Name if config.as_dict or reference or (array and node.m_Type.startswith('PPtr<')) else child._clean_name
        field = yield _read(child_node, reader, config, guard, depth=depth + 1)
        if key in value:
            guard.discard()
        value[key] = field
    if not config.as_dict and not reference:
        from .._object import _construct
        class Charge:
            def reserve(self, memory, work):
                guard.step(0, memory, work)
        value = _construct(node, value, config.assetsfile, False, array, Charge())
    return value


def read_value(node, reader, config, *, limits=None, budget=None, _guard=None):
    guard = _guard_for_value(node, limits, budget, _guard, config.assetsfile, reader)
    guard = _ReadGuard(guard)
    return _drive(_read(node, reader, config, guard), guard)


def _guard_for_value(node, limits, budget, guard, assetsfile=None, reader=None):
    return _new_guard(node, limits, budget, guard, assetsfile, reader)


def read_value_array(node, reader, config, size, *, limits=None, budget=None, _guard=None):
    guard = _guard_for_value(node, limits, budget, _guard, config.assetsfile, reader)
    guard = _ReadGuard(guard)
    return _drive(_read(node, reader, config, guard, size), guard)


def write_typetree(value, root_node, writer, assetsfile=None, *, limits=None, budget=None, _guard=None):
    return write_value(value, root_node, writer, TypeTreeConfig(isinstance(value, dict), assetsfile),
                       limits=limits, budget=budget, _guard=_guard)


def write_value(value, node, writer, config, *, limits=None, budget=None, _guard=None):
    guard = _guard_for_value(node, limits, budget, _guard, config.assetsfile)
    return _drive(_write(value, node, writer, config, guard), guard)


def _write(value, node, writer, config, guard, depth=1):
    guard.value_depth(depth)
    align = metaflag_is_aligned(node.m_MetaFlag)
    ty = node.m_Type
    guard.step(0, 512, 1)
    if ty in FUNCTION_WRITE_MAP:
        if ty == 'string':
            guard.step(0, 256 + text_bound(value), text_bound(value))
        elif ty == 'TypelessData':
            guard.step(0, 256 + memoryview(value).nbytes, memoryview(value).nbytes)
        FUNCTION_WRITE_MAP[ty](writer, value)
    elif ty == 'pair':
        yield _write(value[0], node.m_Children[0], writer, config, guard, depth + 1)
        yield _write(value[1], node.m_Children[1], writer, config, guard, depth + 1)
    elif ty == 'ReferencedObject':
        for child in node.m_Children:
            target = get_ref_type_node(value, config.assetsfile, _guard=guard) if child.m_Type == 'ReferencedObjectData' else child
            yield _write(value[child.m_Name], target, writer, config, guard, depth + 1)
    elif node.m_Children and node.m_Children[0].m_Type == 'Array':
        align |= metaflag_is_aligned(node.m_Children[0].m_MetaFlag)
        writer.write_int(len(value))
        subtype = node.m_Children[0].m_Children[1]
        guard.value_depth(depth + 1)  # The writer's array comprehension.
        for item in value:
            yield _write(item, subtype, writer, config, guard, depth + 2)
    else:
        for child in node.m_Children:
            if child.m_Type == 'ManagedReferencesRegistry':
                if config.has_registry:
                    continue
                config = config.copy()
                config.has_registry = True
            item = value[child.m_Name] if isinstance(value, dict) else getattr(value, child._clean_name)
            yield _write(item, child, writer, config, guard, depth + 1)
    if align:
        writer.align_stream()
