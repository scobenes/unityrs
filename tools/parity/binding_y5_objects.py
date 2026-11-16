"""Final object and file API observations, using synthetic inputs only.

PARITY sections 5.2, 8 and 14.2. Runtime defaults and TPK nodes are never saved.
"""
import importlib
import struct
from types import SimpleNamespace

from binding_cases import S
from binding_y4_exports import _new
from binding_y4_save import _load


def _attempt(call):
    try:
        return {'value': call()}
    except Exception as error:
        return {'error': type(error).__name__, 'message': str(error)}


def _asset():
    tree = S.Node('Y5Object', 'Base', [S.Node('int', 'value'), S.Node('string', 'name')])
    return S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack('<i', 7) + S.s32('before'))])


def observe_patch(module):
    file = _load(module, _asset())
    obj = file.objects[1]
    original = obj.get_raw_data()
    result = {'bytes': obj.patch({'value': 19, 'name': 'after'}).hex(),
              'original': obj.parse_as_dict(), 'raw_original': obj.get_raw_data() == original,
              'changed': file.is_changed, 'size': obj.byte_size}
    result['saved'] = _load(module, file.save()).objects[1].parse_as_dict()
    node = obj._get_typetree_node()
    from binding_y3_tree import Writer
    writer = Writer('<')
    writer.align_stream = lambda alignment=4: writer.write(bytes((-writer.Position) % alignment))
    writer.write(b'head')
    result['writer'] = obj.patch({'value': 23, 'name': 'explicit'}, nodes=node, writer=writer).hex()
    result['writer_position'] = writer.Position
    row = module.files.DirectoryInfo('entry.assets', 12, 34)
    result['directory_info'] = {
        'fields': list(row._fields), 'values': list(row),
        'attributes': [row.path, row.offset, row.size], 'repr': repr(row),
        'asdict': dict(row._asdict()), 'replace': list(row._replace(size=56)),
        'make': list(type(row)._make(['raw.resS', 78, 90])),
        'tuple_equal': row == ('entry.assets', 12, 34),
        'missing': _attempt(lambda: module.files.DirectoryInfo('entry.assets', 12)),
    }
    result['base_math'] = _base_math(module, obj)
    return result


def _base_math(module, reader):
    # Only handwritten MIT records and caller-supplied synthetic values.
    import copy
    import attrs
    classes = importlib.import_module(module.__name__ + '.classes')
    rows = {}
    for name, values in (
        ('Vector2f', (1, 2)), ('Vector3f', (1, 2, 3)),
        ('Vector4f', (1, 2, 3, 4)), ('Quaternionf', (1, 2, 3, 4)),
        ('Matrix3x4f', tuple(range(12))), ('Matrix4x4f', tuple(range(16))),
        ('ColorRGBA', (0.25, 0.5, 0.75, 1)),
    ):
        cls = getattr(classes, name)
        value = cls(*values)
        rows[name] = [attrs.asdict(value), repr(value), str(value),
                      value == cls(*values), value == object(),
                      _attempt(lambda value=value: hash(value))]
    rows['aliases'] = [classes.float3 is classes.Vector3f, classes.float4 is classes.Vector4f]
    color = classes.ColorRGBA(rgba=0x12345678)
    rows['packed'] = [color.rgba, attrs.asdict(color)]
    color.rgba = 0xFEDCBA98
    rows['repacked'] = [color.rgba, attrs.asdict(color)]
    obj = classes.Object(x=17, values=[1, 2])
    copied = copy.copy(obj)
    rows['object'] = [repr(obj), copied.x, copied.values is obj.values, obj.assets_file]
    rows['unsaved'] = _attempt(obj.save)
    obj.set_object_reader(reader)
    rows['reader'] = [obj.object_reader is reader, obj.assets_file is reader.assets_file]
    Node = importlib.import_module(module.__name__ + '.helpers.TypeTreeNode').TypeTreeNode
    unknown = classes.UnknownObject(__node__=Node(0, 'Synthetic', 'Base', 4, 1), value='x' * 130)
    rows['unknown'] = [unknown.get_type(), repr(unknown), copy.copy(unknown).get_type(),
                       classes.UnknownObject().get_type()]
    pointer = classes.PPtr(m_FileID=0, m_PathID=reader.path_id, assetsfile=reader.assets_file)
    same = classes.PPtr(m_FileID=0, m_PathID=reader.path_id, assetsfile=reader.assets_file)
    rows['pointer'] = [bool(pointer), pointer.file_id, pointer.path_id, pointer == same,
                       hash(pointer) == hash(same), pointer.deref() is reader,
                       pointer.deref_parse_as_dict()]
    return rows


def observe_forwarding(module):
    file = _load(module, _asset())
    obj = file.objects[1]
    initial = [file.reader.Position, obj.Position]
    initial_read = obj.read_bytes(4).hex()
    after_initial_read = [file.reader.Position, obj.Position]
    obj.reset()
    start = obj.Position
    value = obj.read_int()
    data = obj.get_raw_data()
    result = {'initial_position': initial, 'initial_read': initial_read,
              'after_initial_read': after_initial_read,
              'endian': obj.endian, 'value': value, 'delta': obj.Position - start,
              'raw': data.hex(), 'after_raw': obj.Position - start,
              'missing': obj.get('no_such_attribute', 123), 'has_missing': hasattr(obj, 'no_such_attribute'),
              'version': [obj.version2, int(obj.platform)],
              'class': obj.get_class().__name__}
    result['name'] = obj.read_aligned_string()
    obj.reset()
    result['reset'] = obj.Position == start
    result['shared_reader'] = obj.reader is file.reader
    obj.parse_as_dict()
    result['parse_position'] = obj.Position - start
    obj.read()
    result['object_position'] = obj.Position - start
    unknown = _load(module, S.serialized_file([S.Type(1234567, S.Node('Y5Unknown', 'Base'))],
                                             [S.Obj(1, 0, b'')])).objects[1]
    result['unknown_class'] = unknown.get_class()
    eager = _load(module, _container_asset('same', 1))
    result['eager_initial_position'] = [eager.reader.Position, eager.objects[9].Position]
    result['eager_initial_read'] = eager.reader.read(4).hex()
    return result


def observe_gameobject(module, corrected=False):
    C = importlib.import_module(module.__name__ + '.classes')
    kinds = [('Animator', 95), ('Animation', 111), ('Transform', 4), ('MeshRenderer', 23),
             ('SkinnedMeshRenderer', 137), ('MeshFilter', 33)]
    types = [S.Type(kind, S.Node(name, 'Base')) for name, kind in kinds]
    file = _load(module, S.serialized_file(types, [S.Obj(i + 1, i, b'') for i in range(len(types))]))
    pointers = [C.PPtr(m_FileID=0, m_PathID=i + 1, assetsfile=file) for i in range(len(types))]
    obj = _new(C.GameObject, m_Component=None)
    def components():
        if corrected and module.__name__ == 'UnityPy':
            return [entry[1] if isinstance(entry, tuple) else entry.component
                    for entry in (obj.m_Component or [])]
        return obj.m_Components

    def accessors():
        if corrected and module.__name__ == 'UnityPy':
            return [next((p.path_id for p in components() if p.type == kind), None)
                    for _, kind in kinds]
        return [getattr(obj, 'm_' + kind).path_id if getattr(obj, 'm_' + kind) is not None
                else None for kind, _ in kinds]

    result = {'none': obj.m_Components, 'none_transform': obj.m_Transform}
    obj.m_Component = []
    result['empty'] = _attempt(components)
    result['empty_accessors'] = _attempt(accessors)
    for name, values in (
        ('new', [SimpleNamespace(component=p) for p in pointers]),
        ('old', [(kind, p) for (_, kind), p in zip(kinds, pointers)]),
    ):
        obj.m_Component = values
        result[name] = _attempt(lambda: [p.path_id for p in components()])
        result[name + '_accessors'] = _attempt(accessors)
    obj.m_Component = [SimpleNamespace(component=pointers[0])]
    result['missing'] = obj.m_Transform
    obj.m_Component = [SimpleNamespace(component=C.PPtr(m_FileID=0, m_PathID=0, assetsfile=file))]
    result['null'] = _attempt(lambda: obj.m_Transform)
    return result


def _container_asset(path, value):
    pointer = lambda name: S.Node('PPtr<Object>', name, [S.Node('int', 'm_FileID'), S.Node('SInt64', 'm_PathID')])
    info = S.Node('AssetInfo', 'second', [S.Node('int', 'preloadIndex'), S.Node('int', 'preloadSize'), pointer('asset')])
    tree = S.Node('AssetBundle', 'Base', [S.Node('string', 'm_Name'),
        S.vec('m_PreloadTable', pointer('data')),
        S.vec('m_Container', S.Node('pair', 'data', [S.Node('string', 'first'), info]))])
    payload = S.s32('ab') + struct.pack('<ii', 0, 1) + S.s32(path) + struct.pack('<iiiq', 0, 0, 0, value)
    text = S.Node('TextAsset', 'Base', [S.Node('string', 'm_Name'), S.Node('string', 'm_Script')])
    return S.serialized_file([S.Type(142, tree), S.Type(49, text)],
                             [S.Obj(9, 0, payload), S.Obj(value, 1, S.s32('name') + S.s32('body'))])


def observe_containers(module):
    packed = S.fs_bundle([(b'first', _container_asset('same', 1), 0),
                          (b'raw', b'raw bytes', 0),
                          (b'second', _container_asset('same', 2), 0)])
    bundle = _load(module, packed, 'container.bundle')
    container = bundle.container
    file = bundle.files['first']
    nested = _load(module, S.web_file([(b'inner', packed)]), 'nested.web')
    return {'paths': list(container), 'ids': [p.path_id for p in container.values()],
            'fresh': container is not bundle.container,
            'serialized_identity': file.container is file.container,
            'object_path': file.objects[1].container,
            'unknown_path': file.objects[9].container,
            'nested': [[key, pointer.path_id] for key, pointer in nested.container.items()]}


def observe_iteration(module, corrected=False):
    bundle = _load(module, S.fs_bundle([(b'asset', _asset(), 0), (b'raw', b'raw bytes', 0)]))
    nested = _load(module, S.web_file([(b'inner', S.fs_bundle([(b'asset', _asset(), 0)]))]))
    result = {}

    def reference(file, method, args):
        if method == 'get_assets':
            if hasattr(file, 'objects'):
                yield file
            else:
                for child in file.files.values():
                    if hasattr(child, 'files'):
                        yield from reference(child, method, args)
        elif hasattr(file, 'objects'):
            kinds = args[0] if args else None
            for obj in file.objects.values():
                if not kinds or obj.type in kinds:
                    yield obj
        else:
            for child in file.files.values():
                if hasattr(child, 'files'):
                    yield from reference(child, method, args)

    for label, file in [('bundle', bundle), ('serialized', bundle.files['asset']), ('nested', nested)]:
        for method, args in [('get_assets', ()), ('get_objects', ()),
                             ('get_filtered_objects', ()), ('get_filtered_objects', ([],)),
                             ('get_filtered_objects', ([49],))]:
            result[label + ':' + method + repr(args)] = _attempt(
                lambda file=file, method=method, args=args:
                    [getattr(item, 'path_id', item.name if hasattr(item, 'name') else None)
                     for item in (reference(file, method, args) if corrected and module.__name__ == 'UnityPy'
                                  else getattr(file, method)(*args))])
    return result


def observe_generators(module):
    """Explicit synthetic heads and runtime-only TPK fallback inputs."""
    Node = importlib.import_module(module.__name__ + '.helpers.TypeTreeNode').TypeTreeNode
    head = S.Node('MonoBehaviour', 'Base', [S.Node('PPtr<MonoScript>', 'm_Script',
        [S.Node('int', 'm_FileID'), S.Node('SInt64', 'm_PathID')])])
    script = S.Node('MonoScript', 'Base', [S.Node('string', name) for name in
        ('m_AssemblyName', 'm_Namespace', 'm_ClassName')])
    payload = struct.pack('<iqi', 0, 2, 42)
    raw = S.serialized_file([S.Type(114, head), S.Type(115, script)],
        [S.Obj(1, 0, payload), S.Obj(2, 1, S.s32('Asm.dll') + S.s32('ns') + S.s32('Cls'))])
    file = _load(module, raw)
    obj = file.objects[1]
    base = obj._get_typetree_node()
    calls = []
    generated = Node(0, 'Y5Generated', 'Base', -1, 1,
                     [Node(1, 'int', 'value', 4, 1)])
    file.environment.typetree_generator = SimpleNamespace(
        get_nodes_up=lambda assembly, fullname: calls.append([assembly, fullname]) or generated)
    result = {'head': obj.parse_monobehaviour_head(base).m_Script.m_PathID,
              'generated_identity': obj.generate_monobehaviour_node(base) is generated,
              'embedded': obj.parse_as_dict(check_read=False)['m_Script'], 'calls': calls}
    file.environment.typetree_generator = None
    result['missing'] = _attempt(lambda: obj.generate_monobehaviour_node(base))
    file.environment.typetree_generator = SimpleNamespace(get_nodes_up=lambda *_: None)
    result['miss'] = _attempt(lambda: obj.generate_monobehaviour_node(base))
    file.environment = None
    result['orphan'] = _attempt(lambda: obj.generate_monobehaviour_node(base))

    # The fixture writer's TPK inputs live in this process only. Observations
    # select supplied fields, never class defaults or a provider's schema.
    from test_rows_objects import tpk_object, tpk_file, pptr
    raw = tpk_file([(1, 114, tpk_object(114, m_GameObject=pptr(0), m_Enabled=1,
        m_Script=pptr(2), m_Name='mb') + struct.pack('<i', 77)),
        (2, 115, tpk_object(115, m_Name='S', m_ClassName='Cls', m_Namespace='', m_AssemblyName='Asm.dll'))])
    file = _load(module, raw)
    obj = file.objects[1]
    head = obj.parse_monobehaviour_head()
    result['tpk_head'] = [head.m_Name, head.m_Script.m_PathID]
    # Clone only in memory and give each module its own public node instances.
    tpk_head = obj._get_typetree_node()
    root = Node.from_list([node.to_dict() | {'m_Children': []} for node in tpk_head.traverse()])
    root.m_Children.append(Node(1, 'int', 'extra', 4, 1))
    calls = []

    def get_nodes(assembly, fullname):
        calls.append([assembly, fullname])
        return root

    file.environment.typetree_generator = SimpleNamespace(get_nodes_up=get_nodes)
    result['automatic'] = obj.parse_as_dict()['extra']
    result['automatic_repeat'] = obj.parse_as_dict()['extra']
    result['automatic_calls'] = calls
    for label, error in [('value', ValueError('generator failed')),
                         ('unicode', UnicodeDecodeError('utf-8', b'\xff', 0, 1, 'synthetic')),
                         ('runtime', RuntimeError('generator failed'))]:
        def fail(*_, error=error):
            raise error
        file.environment.typetree_generator = SimpleNamespace(get_nodes_up=fail)
        result[label] = _attempt(lambda: obj.parse_as_dict(check_read=False).get('m_Name'))
    file.environment = None
    result['automatic_orphan'] = _attempt(lambda: obj.parse_as_dict(check_read=False))
    return result
