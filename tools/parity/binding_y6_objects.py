"""Y6 object residue, with caller-built trees and values only.

References: helpers/TypeTreeHelper.py:182-263 and
helpers/TypeTreeHelper.py:266-361; helpers/ContainerHelper.py:31-49;
classes/PPtr.py:43-90 (UnityPy 1.25.2). No runtime schema is recorded.
"""
import importlib
import json
import signal
import struct
import sys

from binding_cases import S
from binding_y4_save import _load
import child


def _attempt(call):
    try:
        return {'value': call()}
    except Exception as error:
        return {'error': type(error).__name__, 'message': str(error)}


def _selected(value):
    """Select synthetic fields only, omitting generated defaults and metadata."""
    if isinstance(value, dict):
        return {key: _selected(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_selected(item) for item in value]
    if hasattr(value, '__dict__'):
        return {key: _selected(item) for key, item in vars(value).items()
                if key not in ('__node__', 'object_reader')}
    return value


def observe_references(module):
    result = {}
    for zero in (False, True):
        for aligned in (False, True):
            for empty_first in (False, True):
                element = S.ref_object('data')
                element.meta = S.ALIGN if aligned else 0
                root = S.Node('Y6References', 'Base', [S.vec('refs', element)])
                typed = (struct.pack('<q', 2) + S.s32('Y6Value') + S.s32('') +
                         S.s32('Y6Assembly') + struct.pack('<i', 73))
                empty = struct.pack('<q', 1) + S.s32('') * 3
                payload = struct.pack('<i', 2) + (empty + typed if empty_first else typed + empty)
                refs = [S.Type(-1, S.Node('Y6Value', 'Base', [S.Node('int', 'value')]),
                               ref_names=['Y6Value', '', 'Y6Assembly'])]
                file = _load(module, S.serialized_file([S.Type(49, root)],
                    [S.Obj(1, 0, payload, byte_size=0 if zero else None)], ref_types=refs))
                reader = file.objects[1]
                label = f'{"python" if zero else "boost"}:{aligned}:{empty_first}'
                result[label] = {}
                for mode in ('parse_as_dict', 'parse_as_object'):
                    result[label][mode] = _attempt(lambda mode=mode:
                        _selected(getattr(reader, mode)(check_read=False))['refs'])
    return result


NONCLASS_NAMES = ('Optional', 'T', 'Union', 'List', 'Tuple', 'attrs_define',
                  '__repr__', '__str__', '__hash__', '__sizeof__', '__dir__', '__getstate__',
                  '__init_subclass__', '__subclasshook__', '__class__', 'math')


def _nonclass_value(module, name, count, corrected=False):
    kind = 'Y6MissingClass' if corrected else name
    inner = S.Node(kind, 'inner', [S.Node('int', f'f{i}') for i in range(count)])
    root = S.Node('Y6Holder', 'Base', [S.Node('int', 'pad'), inner])
    data = S.serialized_file([S.Type(49, root)],
        [S.Obj(1, 0, struct.pack('<' + 'i' * (count + 1), *range(count + 1)))])
    value = _load(module, data).objects[1].parse_as_object().inner
    if type(value).__name__ == 'UnknownObject':
        return ['UnknownObject', name if corrected else value.get_type(),
                {f'f{i}': getattr(value, f'f{i}') for i in range(count)}]
    # Module descriptors can return process-specific hashes, repr addresses or
    # module names. Their type is the characterized divergence, not that value.
    return [type(value).__name__]


def observe_nonclass(module, corrected=False):
    """The Tier B/C reference renames hostile types to a missing class."""
    if module.__name__ == 'UnityPy' and not corrected:
        return observe_nonclass_original(module)
    return {f'{name}:{count}': _attempt(lambda name=name, count=count:
                _nonclass_value(module, name, count, corrected=module.__name__ == 'UnityPy'))
            for name in NONCLASS_NAMES for count in (0, 1)}


def observe_nonclass_original(module):
    """Isolate unsafe oracle lookups: crash outcomes never run in the recorder."""
    result = {}
    for name in NONCLASS_NAMES:
        for count in (0, 1):
            run = child.run([sys.executable, '-B', __file__, '--nonclass',
                             module.__name__, name, str(count)],
                            capture_output=True, text=True, timeout=120)
            if run.returncode < 0:
                outcome = {'signal': signal.Signals(-run.returncode).name}
            elif run.returncode:
                raise RuntimeError(run.stderr)
            else:
                outcome = json.loads(run.stdout)
            result[f'{name}:{count}'] = outcome
    return result


def observe_transform(module):
    pointer = lambda name: S.Node('PPtr<Transform>', name,
        [S.Node('int', 'm_FileID'), S.Node('SInt64', 'm_PathID')])
    vector = lambda kind, name, fields: S.Node(kind, name,
        [S.Node('float', field) for field in fields])
    root = S.Node('Transform', 'Base', [pointer('m_Father'),
        S.vec('m_Children', pointer('data')),
        vector('Vector3f', 'm_LocalPosition', 'xyz'),
        vector('Quaternionf', 'm_LocalRotation', 'xyzw'),
        vector('Vector3f', 'm_LocalScale', 'xyz')])
    payload = struct.pack('<iqiiq10f', 0, 2, 1, 0, 2,
                          1.25, -2.5, 3.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0)
    file = _load(module, S.serialized_file([S.Type(4, root)],
        [S.Obj(1, 0, payload), S.Obj(2, 0, payload)]))
    reader = file.objects[1]
    result = {'class': reader.get_class().__name__, 'dict': reader.parse_as_dict()}
    for mode in ('read', 'parse_as_object', 'read_typetree'):
        obj = getattr(reader, mode)(**({'wrap': True} if mode == 'read_typetree' else {}))
        result[mode] = {'class': type(obj).__name__,
            'reader': obj.object_reader is reader, 'file': obj.assets_file is file,
            'father': [obj.m_Father.file_id, obj.m_Father.path_id,
                       obj.m_Father.deref() is file.objects[2]],
            'children': [p.path_id for p in obj.m_Children],
            'position': [getattr(obj.m_LocalPosition, axis) for axis in 'xyz'],
            'rotation': [getattr(obj.m_LocalRotation, axis) for axis in 'xyzw'],
            'scale': [getattr(obj.m_LocalScale, axis) for axis in 'xyz']}
    return result


def _pointer(name):
    return S.Node('PPtr<Object>', name, [S.Node('int', 'm_FileID'), S.Node('SInt64', 'm_PathID')])


def _span_file(index_kind, index, size_kind='int', size=1, table_kind='pointers', pointers=((1, 2),)):
    numeric = lambda kind, value: (struct.pack('<B', value) + b'\0' * 3 if kind == 'bool'
                                   else struct.pack('<f' if kind == 'float' else '<i', value))
    fields = []
    info_bytes = b''
    for kind, name, value in ((index_kind, 'preloadIndex', index), (size_kind, 'preloadSize', size)):
        if kind is not None:
            fields.append(S.Node(kind, name, meta=S.ALIGN if kind == 'bool' else 0))
            info_bytes += numeric(kind, value)
    fields.append(_pointer('asset'))
    info_bytes += struct.pack('<iq', 0, 1)
    if table_kind in ('pointers', 'tuple'):
        if table_kind == 'tuple':
            table = S.Node('pair', 'm_PreloadTable', [_pointer('first'), _pointer('second')])
            prefix = b''
        else:
            table = S.vec('m_PreloadTable', _pointer('data'))
            prefix = struct.pack('<i', len(pointers))
        table_bytes = prefix + b''.join(struct.pack('<iq', *p) for p in pointers)
    elif table_kind in ('string', 'bytes'):
        table = S.Node('string' if table_kind == 'string' else 'TypelessData', 'm_PreloadTable')
        table_bytes = S.s32('xy') if table_kind == 'string' else struct.pack('<i', 2) + b'\0\1'
    else:
        kind, values = {'zeros': ('int', [0, 0]), 'false': ('bool', [False, False]),
                        'empty': ('string', ['', ''])}[table_kind]
        table = S.vec('m_PreloadTable', S.Node(kind, 'data'))
        table_bytes = struct.pack('<i', 2) + b''.join(
            S.s32(v) if kind == 'string' else struct.pack('<B' if kind == 'bool' else '<i', v)
            for v in values)
    table.meta = S.ALIGN
    table_bytes += bytes(-len(table_bytes) % 4)
    row = S.Node('pair', 'data', [S.Node('string', 'first'), S.Node('AssetInfo', 'second', fields)])
    root = S.Node('Y6Preload', 'Base', [table, S.vec('m_Container', row)])
    payload = table_bytes + struct.pack('<i', 1) + S.s32('owner/path') + info_bytes
    scalar = S.Node('Y6Value', 'Base', [S.Node('int', 'value')])
    return S.serialized_file([S.Type(49, root), S.Type(49, scalar)],
        [S.Obj(7, 0, payload), S.Obj(1, 1, struct.pack('<i', 11))],
        externals=[S.External('archive:/assets/DEST.assets')])


def observe_preload(module):
    Helper = importlib.import_module(module.__name__ + '.helpers.ContainerHelper').ContainerHelper
    cases = {
        'valid_external': {}, 'bool_index': {'index_kind': 'bool', 'index': False},
        'bool_index_true': {'index_kind': 'bool', 'index': True,
                            'pointers': ((0, 0), (1, 2))},
        'bool_size': {'size_kind': 'bool', 'size': True},
        'negative_float': {'index_kind': 'float', 'index': -0.5},
        'past_float': {'index_kind': 'float', 'index': 3.0},
        'within_float': {'index_kind': 'float', 'index': 0.0},
        'float_size': {'size_kind': 'float', 'size': 1.0},
        'missing_index': {'index_kind': None}, 'missing_size': {'size_kind': None},
        'tuple': {'table_kind': 'tuple', 'pointers': ((0, 0), (1, 2)), 'size': 2},
        'string': {'table_kind': 'string'}, 'bytes': {'table_kind': 'bytes', 'size': 2},
        'zeros': {'table_kind': 'zeros', 'size': 2},
        'false': {'table_kind': 'false', 'size': 2},
        'empty': {'table_kind': 'empty', 'size': 2},
        'null': {'pointers': ((0, 0),)},
        'invalid_file': {'pointers': ((2, 2),)},
        'missing_object': {'pointers': ((1, 999),)},
        'local': {'pointers': ((0, 1),)},
        'missing_external': {'missing_external': True},
        'existing_path': {'existing_path': True},
    }
    result = {}
    target_data = S.serialized_file([S.Type(49, S.Node('Y6Value', 'Base', [S.Node('int', 'value')]))],
                                    [S.Obj(2, 0, struct.pack('<i', 22))])
    for label, supplied in cases.items():
        args = dict(index_kind='int', index=0)
        args.update(supplied)
        missing = args.pop('missing_external', False)
        existing = args.pop('existing_path', False)
        env = module.Environment(path='')
        source = env.load_file(_span_file(**args), name='source.assets')
        target = env.load_file(target_data, name='dest.assets') if not missing else None
        helper = Helper(source.objects[7].parse_as_object())
        source._container = helper
        if target is not None:
            target._container = Helper([])
            if existing:
                target._container.path_dict[2] = 'existing/path'
        answer = {'loaded': sorted(source.objects), 'parse': _attempt(helper.parse_preload_table),
                  'source_paths': sorted([key, path] for key, path in helper.path_dict.items()),
                  'target_paths': (sorted([key, path] for key, path in target._container.path_dict.items())
                                   if target is not None else None),
                  'cleared': helper._preload_table is None}
        answer['repeat'] = _attempt(helper.parse_preload_table)
        result[label] = answer
    return result


if __name__ == '__main__' and len(sys.argv) == 5 and sys.argv[1] == '--nonclass':
    print(json.dumps(_attempt(lambda: _nonclass_value(
        importlib.import_module(sys.argv[2]), sys.argv[3], int(sys.argv[4])))))
