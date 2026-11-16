"""Synthetic standalone helpers and explicit optional backend observations."""
import importlib
import io
import struct
import scratch
from pathlib import Path
from types import SimpleNamespace


def _module(package, name):
    return importlib.import_module(package.__name__ + '.' + name)


def _node(package, ty='int', name='value', children=None, flag=0):
    return package.helpers.TypeTreeNode.TypeTreeNode(0, ty, name, -1, 1,
                                                    children or [], m_MetaFlag=flag)


def observe_tree_depth(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    result = []
    for depth in (1, 160, 161, 256):
        root, value = _node(package), 23
        for _ in range(depth - 1):
            root = _node(package, 'Synthetic', children=[root])
            value = {'value': value}
        writer = package.streams.EndianBinaryWriter(endian='<')
        helper.write_typetree(value, root, writer)
        reads = []
        for accelerated in (False, True):
            reader = package.streams.EndianBinaryReader(writer.bytes, '<')
            parsed = helper.read_typetree(root, reader, byte_size=4 if accelerated else None,
                                          check_read=False)
            for _ in range(depth - 1):
                parsed = parsed['value']
            reads.append([parsed, reader.Position])
        result.append([depth, writer.bytes.hex(), reads])
    return result


def observe_tree_reference_lookup(package):
    import threading
    helper = _module(package, 'helpers.TypeTreeHelper')
    descriptor = _node(package, 'SyntheticType', 'type', [
        _node(package, 'string', name) for name in ('class', 'ns', 'asm')])
    root = _node(package, 'ReferencedObject', 'Base', [
        descriptor, _node(package, 'ReferencedObjectData', 'data')])
    writer = package.streams.EndianBinaryWriter(endian='<')
    for word in ('Synthetic', 'NS', 'ASM'):
        writer.write_aligned_string(word)
    writer.write_int(23)
    match = SimpleNamespace(m_ClassName='Synthetic', m_NameSpace='NS', m_AssemblyName='ASM',
                            node=_node(package))
    class Unselected:
        m_ClassName, m_NameSpace, m_AssemblyName = 'Other', 'NS', 'ASM'
        @property
        def node(self):
            raise AssertionError('unselected reference node')
    class BadDescriptor:
        @property
        def m_ClassName(self):
            raise RuntimeError('descriptor getter')
    class BadNode:
        m_ClassName, m_NameSpace, m_AssemblyName = 'Synthetic', 'NS', 'ASM'
        @property
        def node(self):
            raise RuntimeError('node getter')
    caller = threading.get_ident()
    class ReentrantNode:
        m_ClassName, m_NameSpace, m_AssemblyName = 'Synthetic', 'NS', 'ASM'
        @property
        def node(self):
            if threading.get_ident() != caller:
                raise AssertionError('reference callback thread')
            parsed, consumed = helper.read_typetree_boost(b'\x18\0\0\0', _node(package), '<', True)
            if (parsed, consumed) != (24, 4):
                raise AssertionError('reference callback reentry')
            return _node(package)
    result = []
    for references in ([match, None], [Unselected(), match], [match, Unselected()], [match, match],
                       [ReentrantNode(), None], [BadDescriptor(), match], [BadNode(), None]):
        try:
            value, consumed = helper.read_typetree_boost(writer.bytes, root, '<', True,
                                                        SimpleNamespace(ref_types=references))
            result.append([value, consumed])
        except Exception as error:
            result.append([type(error).__name__, str(error)])
    class Changing:
        m_ClassName, m_NameSpace, m_AssemblyName = 'Synthetic', 'NS', 'ASM'
        count = 0
        @property
        def node(self):
            self.count += 1
            return _node(package, 'SyntheticA' if self.count == 1 else 'SyntheticB',
                         children=[_node(package)])
    first = _node(package, 'ReferencedObject', 'first', root.m_Children)
    second = _node(package, 'ReferencedObject', 'second', root.m_Children)
    outer = _node(package, 'SyntheticOuter', children=[first, second])
    obj, consumed = helper.read_typetree_boost(writer.bytes * 2, outer, '<', False,
        SimpleNamespace(ref_types=[Changing()]), _module(package, 'classes'))
    result.append([obj.first.data.get_type(), obj.second.data.get_type(), consumed])

    nested = _node(package, 'ReferencedObject', 'nested', root.m_Children)
    schemas = [_node(package, 'SyntheticOuter1', children=[nested]),
               _node(package, 'SyntheticInner', children=[_node(package)]),
               _node(package, 'SyntheticOuter2', children=[_node(package)])]
    class ChangingNested:
        m_ClassName, m_NameSpace, m_AssemblyName = 'Synthetic', 'NS', 'ASM'
        count = 0
        @property
        def node(self):
            selected = schemas[self.count]
            self.count += 1
            return selected
    repeated = _node(package, 'ReferencedObject', children=[
        descriptor, _node(package, 'ReferencedObjectData', 'data1'),
        descriptor, _node(package, 'ReferencedObjectData', 'data2')])
    prefix = writer.bytes[:-4]
    data = prefix + prefix + struct.pack('<i', 23) + prefix + struct.pack('<i', 24)
    obj, consumed = helper.read_typetree_boost(data, repeated, '<', False,
        SimpleNamespace(ref_types=[ChangingNested()]), _module(package, 'classes'))
    result.append([obj.data1.get_type(), obj.data1.nested.data.get_type(), obj.data2.get_type(), consumed])

    calls = []
    class SyntheticCompleted:
        __annotations__ = {'value': int}
        def __init__(self, value):
            calls.append(value)
            self.value = value
    namespace = SimpleNamespace(SyntheticCompleted=SyntheticCompleted,
                                 UnknownObject=_module(package, 'classes').UnknownObject)
    outer = _node(package, 'SyntheticOuter', children=[
        _node(package, 'SyntheticCompleted', 'completed', [_node(package)]), root])
    try:
        helper.read_typetree_boost(struct.pack('<i', 23) + writer.bytes, outer, '<', False,
                                  SimpleNamespace(ref_types=[BadNode()]), namespace)
    except Exception as error:
        result.append([calls, type(error).__name__, str(error)])
    else:
        result.append([calls, 'accepted'])
    return result


def observe_tree_defaults(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    result = []
    for stream in (False, True):
        for size in (None, 0, 4, 8, -1, -2):
            for check in (False, True):
                data = b'xx' + struct.pack('<i', 23) + b'zzzz'
                reader = package.streams.EndianBinaryReader(io.BytesIO(data) if stream else data, '<')
                reader.Position = 2
                try:
                    value = helper.read_typetree(_node(package), reader, byte_size=size, check_read=check)
                    value = {'value': value}
                except Exception as error:
                    value = {'error': type(error).__name__, 'message': str(error)}
                result.append(dict(value, position=reader.Position))
    return result


def observe_tree_values(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    result = []
    for accelerated in (False, True):
        for endian in ('<', '>'):
            pair = _node(package, 'pair', 'pair', [_node(package), _node(package, 'bool')])
            root = _node(package, 'SyntheticTree', 'Base', [pair, _node(package, 'string', 'name')])
            writer = package.streams.EndianBinaryWriter(b'x', endian)
            helper.write_typetree({'pair': (23, True), 'name': 'hé'}, root, writer)
            data = writer.bytes
            reader = package.streams.EndianBinaryReader(data, endian)
            reader.Position = 1
            try:
                value = helper.read_typetree(root, reader, byte_size=len(data)-1 if accelerated else None,
                                             check_read=False)
                result.append([data.hex(), value, reader.Position])
            except Exception as error:
                result.append([type(error).__name__, str(error), reader.Position])
    return result


def observe_tree_boost(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    result = []
    for data in (struct.pack('<i', 23), b'\x01'):
        try:
            value, consumed = helper.read_typetree_boost(data, _node(package), '<', True, None, _module(package, 'classes'))
            result.append([value, consumed])
        except Exception as error:
            result.append([type(error).__name__, str(error)])
    # Record omitted and explicit flags separately on the pinned native alias.
    for kwargs in ({}, {'as_dict': False}, {'endian': '!'},
                   {'as_dict': True}, {'endian': '!', 'as_dict': True}):
        try:
            options = dict(node=_node(package), endian='<')
            options.update(kwargs)
            value, consumed = helper.read_typetree_boost(struct.pack('<i', 23), **options)
            result.append([value, consumed])
        except Exception as error:
            result.append([type(error).__name__, str(error)])
    return result


def observe_generator_discovery(package):
    module = _module(package, 'helpers.TypeTreeGenerator')
    class Generator(module.TypeTreeGenerator):
        def __init__(self):
            self.cache, self.calls = {}, []
        def load_dll(self, data):
            self.calls.append(['dll', data.hex()])
        def load_il2cpp(self, data, metadata):
            self.calls.append(['il2cpp', data.hex(), metadata.hex()])
    generator = Generator()
    with scratch.folder('y6b-tree-') as directory:
        root = Path(directory)
        managed = root / 'Synthetic_Data' / 'Managed'
        managed.mkdir(parents=True)
        (managed / 'synthetic.dll').write_bytes(b'synthetic-dll')
        (managed / 'ignore.DLL').write_bytes(b'ignored')
        (managed / 'ignore.txt').write_bytes(b'ignored')
        generator.load_local_game(str(directory))
        (root / 'GameAssembly.dll').write_bytes(b'synthetic-native')
        metadata = root / 'Synthetic_Data' / 'il2cpp_data' / 'Metadata'
        metadata.mkdir(parents=True)
        (metadata / 'global-metadata.dat').write_bytes(b'synthetic-metadata')
        generator.load_local_game(str(directory))
    return generator.calls


def observe_generator_backend(package):
    """Install a synthetic API module explicitly, then restore import state."""
    import sys
    import types
    from unittest.mock import patch
    module = _module(package, 'helpers.TypeTreeGenerator')
    calls = []
    class Backend:
        def __init__(self, version):
            calls.append(['init', version])
        def load_dll(self, data):
            calls.append(['dll', data.hex()])
        def load_il2cpp(self, data, metadata):
            calls.append(['il2cpp', data.hex(), metadata.hex()])
        def get_nodes(self, assembly, fullname):
            calls.append(['nodes', assembly, fullname])
            return [SimpleNamespace(m_Level=0, m_Type='Synthetic', m_Name='Base', m_MetaFlag=0)]
    optional = types.ModuleType('TypeTreeGeneratorAPI')
    optional.TypeTreeGenerator = Backend
    try:
        with patch.dict(sys.modules, {'TypeTreeGeneratorAPI': optional}):
            importlib.reload(module)
            generator = module.TypeTreeGenerator('2020.3.0f1')
            generator.load_dll(b'synthetic')
            generator.load_il2cpp(b'native', b'metadata')
            first = generator.get_nodes_up('Synthetic', 'Synthetic')
            same = first is generator.get_nodes_up('Synthetic', 'Synthetic')
            return [calls, same, first.m_Type, first.m_ByteSize, first.m_Version]
    finally:
        importlib.reload(module)


def observe_update_tpk(package):
    """A local deterministic HTTP peer serves synthetic zip bytes only."""
    import contextlib
    import http.server
    import threading
    import zipfile
    from unittest.mock import patch
    module = _module(package, 'cli.update_tpk')
    output = io.StringIO()
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w') as archive:
        archive.writestr('lzma.tpk', b'synthetic-tpk')
    payload = buffer.getvalue()
    requests, generated = [], []
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            self.send_response(200)
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        def log_message(self, *args):
            pass
    server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    old_tpk = getattr(package.config, 'TPK', None)
    try:
        with scratch.folder('y6b-tree-') as directory:
            target = Path(directory)
            with patch.object(module, 'URL', f'http://127.0.0.1:{server.server_port}/synthetic.zip'), \
                 patch.object(module, 'RESOURCE_PATH', str(directory)), \
                 patch.object(module, 'generate_classes', lambda: generated.append(True)), \
                 contextlib.redirect_stdout(output):
                module.update_tpk()
            files = sorted(path.name for path in target.iterdir())
            data = (target / 'lzma.tpk').read_bytes().hex()
        return [requests, generated, files, data, output.getvalue(),
                _module(package, 'cli').update_tpk is module.update_tpk]
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
        if package.__name__ == 'unityrs':
            package.config.TPK = old_tpk


def observe_tree_arrays(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    results = []
    for ty in ('short', 'unsigned short', 'UInt8', 'int', 'string'):
        for aligned in (False, True):
            item = _node(package, ty, 'data', flag=0x4000 if aligned else 0)
            array = _node(package, 'Array', 'Array', [_node(package, 'int', 'size'), item])
            root = _node(package, 'vector', 'values', [array])
            for accelerated in (False, True):
                data = (struct.pack('<i', 2) + (b'\0\0\0\0' * 2 if ty == 'string' else
                        struct.pack('<2h', -2, 7) if ty in ('short', 'unsigned short') else
                        b'\x03\x07\0\0' if ty == 'UInt8' else struct.pack('<2i', -2, 7)))
                reader = package.streams.EndianBinaryReader(data, '<')
                try:
                    value = helper.read_typetree(root, reader, byte_size=len(data) if accelerated else None,
                                                 check_read=False)
                    results.append([type(value).__name__, value, reader.Position])
                except Exception as error:
                    results.append([type(error).__name__, str(error), reader.Position])
    return results


def observe_tree_failures(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    result = []
    for ty, data in [('int', b'\1'), ('string', struct.pack('<i', -1)),
                     ('string', struct.pack('<i', 100)), ('TypelessData', struct.pack('<i', -1) + b'ab')]:
        for stream in (False, True):
            for accelerated in (False, True):
                reader = package.streams.EndianBinaryReader(io.BytesIO(data) if stream else data, '<')
                try:
                    value = helper.read_typetree(_node(package, ty), reader,
                                                 byte_size=len(data) if accelerated else None, check_read=False)
                    result.append([value.hex() if isinstance(value, bytes) else value, reader.Position])
                except Exception as error:
                    result.append([type(error).__name__, str(error), reader.Position])
    return result


def observe_tree_write_failures(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    cases = [(_node(package), 1 << 50),
             (_node(package, 'Synthetic', children=[_node(package)]), {}),
             (_node(package, 'pair', children=[_node(package)]), (1, 2)),
             (_node(package, 'string'), '\ud800')]
    results = []
    for root, value in cases:
        writer = package.streams.EndianBinaryWriter(b'x', '<')
        try:
            helper.write_typetree(value, root, writer)
            results.append([writer.Position, writer.bytes.hex()])
        except Exception as error:
            results.append([type(error).__name__, str(error), writer.Position, writer.bytes.hex()])
    return results


def observe_tree_objects(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    root = _node(package, 'SyntheticHelper', 'Base', [_node(package, 'int', 'field name')])
    result = []
    for boost in (False, True):
        reader = package.streams.EndianBinaryReader(struct.pack('<i', 23), '<')
        try:
            obj = helper.read_typetree(root, reader, as_dict=False, byte_size=4 if boost else None,
                                       check_read=False)
            result.append([type(obj).__name__, {k: v for k, v in vars(obj).items() if k != '__node__'},
                           reader.Position])
        except Exception as error:
            result.append([type(error).__name__, str(error), reader.Position])
    calls = []
    class SyntheticHelper:
        __annotations__ = {'field_name': int}
        def __init__(self, field_name):
            calls.append(field_name)
            self.field_name = field_name
    nested = _node(package, 'SyntheticOuter', 'Base', [
        _node(package, 'SyntheticHelper', 'first', [_node(package, 'int', 'field name')]),
        _node(package, 'int', 'truncated')])
    namespace = SimpleNamespace(SyntheticHelper=SyntheticHelper)
    try:
        helper.read_typetree_boost(struct.pack('<i', 23), nested, '<', False, None, namespace)
    except Exception as error:
        result.append([calls, type(error).__name__, str(error)])
    else:
        result.append([calls, 'accepted'])
    return result


def observe_tree_namespace(package):
    helper = _module(package, 'helpers.TypeTreeHelper')
    class SyntheticHelper:
        __annotations__ = {'field_name': int}
        def __init__(self, field_name):
            self.field_name = field_name + 1
    namespace = SimpleNamespace(SyntheticHelper=SyntheticHelper)
    root = _node(package, 'SyntheticHelper', 'Base', [_node(package, 'int', 'field name')])
    obj, consumed = helper.read_typetree_boost(struct.pack('<i', 23), root, '<', False, None, namespace)
    result = [type(obj).__name__, vars(obj), consumed]
    descriptor = _node(package, 'SyntheticRefType', 'type', [
        _node(package, 'string', 'class'), _node(package, 'string', 'ns'),
        _node(package, 'string', 'asm')])
    root = _node(package, 'ReferencedObject', 'Base', [
        descriptor, _node(package, 'ReferencedObjectData', 'data')])
    data = bytearray()
    for value in (b'Synthetic', b'NS', b'ASM'):
        data.extend(struct.pack('<i', len(value)))
        data.extend(value)
        data.extend(b'\0' * (-len(data) % 4))
    data.extend(struct.pack('<i', 23))
    reference = SimpleNamespace(m_ClassName='Initial', m_NameSpace='NS', m_AssemblyName='ASM',
                                node=_node(package, 'SyntheticRef', 'Base', [_node(package)]))
    file = SimpleNamespace(ref_types=[reference])
    # Reads must observe current attributes, including replacement of ref.node.
    for change in ('initial', 'integer-metadata', 'null-metadata', 'metadata', 'node', 'none',
                   'replace', 'missing', 'no-file'):
        if change == 'integer-metadata':
            reference.m_ClassName = 9
        elif change == 'null-metadata':
            reference.m_ClassName = None
        elif change == 'metadata':
            reference.m_ClassName = 'Synthetic'
        elif change == 'node':
            reference.node = _node(package, 'SyntheticRef', 'Base', [_node(package, 'int', 'edited')])
        elif change == 'none':
            reference.node = None
        elif change == 'replace':
            file.ref_types = [SimpleNamespace(m_ClassName='Synthetic', m_NameSpace='NS',
                m_AssemblyName='ASM', node=_node(package, 'int', 'replacement'))]
        elif change == 'missing':
            file = SimpleNamespace()
        elif change == 'no-file':
            file = None
        try:
            value, consumed = helper.read_typetree_boost(bytes(data), root, '<', True, file)
            result.append([change, value, consumed])
        except Exception as error:
            result.append([change, type(error).__name__, str(error)])
    # A primitive tree does not consult ref_types or any reference protocols.
    class UnusedFile:
        @property
        def ref_types(self):
            raise AssertionError('unused reference attribute')
    result.append(helper.read_typetree_boost(struct.pack('<i', 23), _node(package), '<', True,
                                            UnusedFile()))
    return result
