"""Caller-thread and reader-span regressions using synthetic assets only."""
import importlib
import gc
import io
import struct
import threading
import scratch
from types import SimpleNamespace

from binding_cases import S


def observe_generator_reentry(module):
    from test_rows_objects import tpk_object, tpk_file, pptr
    Node = importlib.import_module(module.__name__ + '.helpers.TypeTreeNode').TypeTreeNode
    raw = tpk_file([(1, 114, tpk_object(114, m_GameObject=pptr(0), m_Enabled=1,
        m_Script=pptr(2), m_Name='mb') + struct.pack('<i', 77)),
        (2, 115, tpk_object(115, m_Name='S', m_ClassName='Cls', m_Namespace='', m_AssemblyName='Asm.dll'))])
    env = module.Environment(raw)
    obj = next(obj for obj in env.objects if obj.path_id == 1)
    caller = threading.get_ident()
    visits = []

    class ReentrantNode(Node):
        def __getattribute__(self, name):
            if name == 'm_Name':
                # Fail promptly before the old worker blocks on the RLock.
                assert threading.get_ident() == caller, 'node getter left caller thread'
                visits.append(len(env.container))
            return super().__getattribute__(name)

    base = obj._get_typetree_node()
    root = Node.from_list([node.to_dict() | {'m_Children': []} for node in base.traverse()])
    root.m_Children.append(ReentrantNode(1, 'int', 'extra', 4, 1))
    def get_nodes(*_):
        assert threading.get_ident() == caller
        list(env.container)
        return root
    env.typetree_generator = SimpleNamespace(get_nodes_up=get_nodes)
    values = [obj.parse_as_dict()['extra'], obj.read().extra]
    # UnityPyBoost can read its C node slots directly without Python getters.
    assert module.__name__ != 'unityrs' or visits
    assert all(value == 0 for value in visits)
    return values


def observe_negative_readers(module):
    rows = []
    tree = S.Node('ReaderSpan', 'Base', [S.Node('int', 'value')])
    raw = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, b'abc\0' + b'\xff' * 12)])
    with scratch.folder('reader-span-') as folder:
        path = folder / 'synthetic.assets'
        path.write_bytes(raw)
        for source, stream in ((raw, False), (io.BytesIO(raw), True), (str(path), True)):
            rows.extend(_negative_reader_rows(module.Environment(source).objects[0], stream))
    return rows


def _negative_reader_rows(obj, stream):
    rows = []
    reader = obj.assets_file.reader
    calls = [('read', -1), ('read', -2), ('read', None), ('read_string', -1),
             ('read_string', -2), ('read_string', None),
             ('read_string_to_null', -1), ('read_int_array', -1),
             ('read_int_array', None), ('read', -1.5)]
    if stream:
        calls.append(('read', -2**100))
    for method, size in calls:
        obj.reset()
        start = obj.Position
        if method == 'read_int_array' and size is None:
            reader.Position = reader.Length - 4
        try:
            value = getattr(reader, method)(size)
            if isinstance(value, str):
                value = value.encode('utf8', 'surrogateescape')
            value = ['bytes', value.hex()] if isinstance(value, bytes) else list(value)
        except Exception as error:
            value = ['error', type(error).__name__]
        rows.append([method, size, value, obj.Position - start])
    return rows


def observe_negative_string_budget(module, corrected=False):
    from binding_y5_round2 import string_asset
    rows = []
    for length in (32, 65_536):
        kwargs = {'budget': module.Budget(70_000)} if module.__name__ == 'unityrs' else {}
        obj = module.Environment(string_asset(length), **kwargs).objects[0]
        obj.Position = obj.reader.Length - length
        start = obj.Position
        try:
            if corrected and module.__name__ == 'UnityPy' and length == 65_536:
                raise MemoryError()
            value = obj.read_string(-1)
            result = ['value', len(value), value == '\udcff' * length]
        except MemoryError:
            result = ['MemoryError']
        rows.append([result, obj.Position - start])
    return rows


def observe_array_maps(module):
    maps = importlib.import_module(module.__name__ + '.helpers.TypeTreeHelper')
    streams = importlib.import_module(module.__name__ + '.streams')
    rows = []
    # Include high bits to expose the oracle's signed/unsigned short aliases.
    payload = bytes.fromhex('80ff0102030405060708090a0b0c0d0e')
    for endian in ('<', '>'):
        for name, read in maps.FUNCTION_READ_MAP_ARRAY.items():
            for count in (0, 2):
                data = struct.pack(endian + 'i', count) + payload
                for source in (data, io.BytesIO(data)):
                    reader = streams.EndianBinaryReader(source, endian)
                    value = read(reader)
                    value = ['bytes', value.hex()] if isinstance(value, bytes) else list(value)
                    rows.append([endian, name, count, value, reader.Position])
    return rows


def check_worker_output_reentry(module):
    tree = S.Node('Output', 'Base', [S.vec('v' + str(i), S.Node('int', 'data')) for i in range(256)])
    raw = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack('<iii', 2, 7, 8) * 256)])
    env = module.Environment(raw)
    obj = env.objects[0]
    caller = threading.get_ident()
    visits = []
    def callback(phase, info):
        if phase == 'start':
            on_caller = threading.get_ident() == caller
            visits.append(on_caller)
            if on_caller:
                list(env.container)
    thresholds = gc.get_threshold()
    try:
        gc.callbacks.append(callback)
        gc.set_threshold(1, 1, 1)
        with env._lock:
            assert obj._native.parse(True)['v255'] == [7, 8]
        assert visits and all(visits), 'Python result allocation ran on worker'
    finally:
        gc.set_threshold(*thresholds)
        gc.callbacks.remove(callback)


def check_worker_error_reentry(module):
    import ntpath
    from binding_y5_objects import _asset
    env = module.Environment(_asset())
    file = env.objects[0].assets_file
    caller = threading.get_ident()
    visits = []
    original = ntpath.basename
    def basename(path):
        assert threading.get_ident() == caller, 'error callback left caller thread'
        visits.append(list(env.container))
        return original(path)
    ntpath.basename = basename
    try:
        with env._lock:
            try:
                env._native.resource(file._native, b'missing.resS', 0, 4)
            except FileNotFoundError:
                pass
            else:
                raise AssertionError('missing resource succeeded')
        assert visits == [[]]
    finally:
        ntpath.basename = original


def check_snapshot_reentry(module):
    from binding_y4_review_object import observe_reentrant_writer, observe_numeric_writer
    from binding_y4_round3 import observe_packer_threads, observe_entry_threads
    from binding_y3_config import observe_config_loads
    # Existing protocol observations assert calling-thread access. Exercise
    # them here under DEADLOCK_GUARD along with warning-handler nested loads.
    assert observe_reentrant_writer(module)['reads'] == [7]
    observe_numeric_writer(module)
    observe_packer_threads(module)
    observe_entry_threads(module)
    observe_config_loads(module)


def check_row_protocol_reentry(module):
    from binding_y5_objects import _asset
    env = module.Environment(_asset())
    obj = env.objects[0]
    node = obj._get_typetree_node()
    caller = threading.get_ident()
    finalized, closed = [], []

    def reenter():
        assert threading.get_ident() == caller, 'row protocol left caller thread'
        list(env.container)

    class Index:
        def __init__(self, value):
            self.value = value
        def __index__(self):
            reenter()
            return self.value

    class Name(str):
        def __del__(self):
            same = threading.get_ident() == caller
            finalized.append(same)
            if same:
                list(env.container)

    def rows():
        try:
            for row in node._native_rows():
                reenter()
                yield (Index(row[0]), row[1], Name(row[2]), *row[3:])
        finally:
            reenter()
            closed.append(True)

    with env._lock:
        assert obj._native.parse_nodes(True, rows())['value'] == 7
        assert obj._native.parse_with_position(True, rows())[0]['value'] == 7
        assert 7 in obj._native.object_data(rows())[0].values()
    assert closed == [True, True, True]
    assert finalized and all(finalized)


def check_reader_policy_publication(module):
    from binding_y5_objects import _asset
    for boundary in ('sync', 'load'):
        env = module.Environment()
        native = env._native
        readers = []

        class ReentrantNative:
            def __getattr__(self, name):
                return getattr(native, name)
            def load_bytes(self, *args):
                result = native.load_bytes(*args)
                if boundary == 'load':
                    # A nested load can sync the newly published core file
                    # before the outer native call returns its Python handle.
                    env._sync()
                return result
            def cabs(self):
                # _sync exposes env.files before its native CAB handoff.
                readers.append(env.objects[0].assets_file.reader)
                return native.cabs()

        env._native = ReentrantNative()
        obj = env.load_file(io.BytesIO(_asset()), name='synthetic.assets').objects[1]
        assert readers and all(reader is obj.reader for reader in readers)
        obj.reset()
        assert obj.reader.read(-2) == _asset()[obj.byte_start:]
