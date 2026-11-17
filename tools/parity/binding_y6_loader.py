"""Synthetic loader residue; original observations accompany B/C references.

Sources: helpers/ImportHelper.py:159-173; environment.py:100-160,
environment.py:310-354; classes/PPtr.py:43-90 (UnityPy 1.25.2).
No generated schema or default is recorded.
"""
import importlib
import io
import os
from pathlib import Path
from types import SimpleNamespace

import binding_cases as B
from binding_y5_loader import _call, _clean, _data
import scratch


def _helper(module):
    return importlib.import_module(module.__name__ + '.helpers.ImportHelper')


def _reader(module, data):
    return importlib.import_module(module.__name__ + '.streams').EndianBinaryReader(data)


def _summary(file):
    if file is None:
        return None
    if hasattr(file, 'objects'):
        return [B.typed(obj.parse_as_dict()) for obj in file.objects.values()]
    return bytes(file.bytes).hex()


def observe_sensitive_path(module):
    with scratch.folder('binding-y6-sensitive-') as directory:
        root = Path(directory)
        (root / 'Resources' / 'Sub').mkdir(parents=True)
        (root / 'Resources' / 'Foo.asset').write_bytes(_data())
        (root / 'Top.asset').write_bytes(_data())
        find = _helper(module).find_sensitive_path
        result = {}
        for name in ('resources/foo.asset', 'RESOURCES/FOO.ASSET', '/resources/foo.asset/',
                     'Top.asset', 'top.asset', 'Resources/Sub/Bar.asset', str(root / 'Resources/Foo.asset')):
            result[name.replace(str(root), '<root>')] = _call(lambda: find(str(root), name))
        for label, path in [('empty', ''), ('missing', str(root / 'missing')),
                            ('file', str(root / 'Top.asset'))]:
            result[label] = _call(lambda: find(path, 'a/b'))
        return _clean(result, root)


def observe_case_lookup(module):
    with scratch.folder('binding-y6-case-') as directory:
        root = Path(directory)
        (root / 'Resources').mkdir()
        (root / 'Resources' / 'Foo.asset').write_bytes(_data())
        (root / 'Resources' / 'lower.assets').write_bytes(_data())
        result = {}
        for name in ('resources/foo.asset', 'Resources/Foo.asset', '/resources/foo.asset', 'top.asset'):
            env = module.Environment(path=str(root))
            result['load:' + name] = _call(lambda: _summary(env.load_file(name)))
        for name in ('LOWER.assets', 'Resources/LOWER.ASSETS', 'foo.asset', 'Foo.asset'):
            env = module.Environment(path=str(root))
            result['find:' + name] = _call(lambda: _summary(env.find_file(name)))
        return _clean(result, root)


def observe_hashes(module):
    # Compare the caller interpreter's hashes, never save random keys or IDs.
    data = _data()
    result = {}
    for label, source in [('bytes', data), ('view', memoryview(data)),
                          ('stream', io.BytesIO(data)), ('reader', _reader(module, data))]:
        env = module.Environment()
        loaded = env.load_file(source)
        result[label] = [list(env.files) == [str(hash(source))],
                         env.files[str(hash(source))] is loaded,
                         env.get_cab(str(hash(source))) is loaded, _summary(loaded)]
    env = module.Environment()
    first = env.load_file(data)
    second = env.load_file(memoryview(data))
    result['equal_bytes'] = [len(env.files), env.files[str(hash(data))] is second]
    one, two = io.BytesIO(data), io.BytesIO(data)
    env = module.Environment()
    env.load_file(one)
    env.load_file(two)
    result['distinct_streams'] = [len(env.files), str(hash(one)) != str(hash(two))]
    return result


def observe_readers(module):
    result = []
    for data in (b'raw', _data()):
        for streamed in (False, True):
            reader = _reader(module, io.BytesIO(data) if streamed else data)
            reader.Position = 2
            env = module.Environment()
            loaded = env.load_file(reader, name='explicit')
            identity = loaded.reader is reader if hasattr(loaded, 'objects') else loaded is reader
            result.append([identity, _summary(loaded), env.files['explicit'] is loaded,
                           env.get_cab('explicit') is loaded])
    for streamed in (False, True):
        reader = _reader(module, io.BytesIO(b'A' * 20) if streamed else b'A' * 20)
        result.append(_call(lambda: _summary(module.Environment().load_file(reader, name='unterminated'))))
    return result


def observe_buffers(module, corrected=False):
    result = {}
    data = _data()
    for label, source in [('bytearray', bytearray(data)), ('writable_view', memoryview(bytearray(data)))]:
        def load():
            env = module.Environment()
            value = env.load_file(data if corrected and module.__name__ == 'UnityPy' else source)
            return [_summary(value), list(env.files) == [str(hash(data))]]
        result[label] = _call(load)
    class ReadOnly:
        def __init__(self):
            self.stream = io.BytesIO(data)
        def read(self, size=-1):
            return self.stream.read(size)
    for name in (None, 'input'):
        def load():
            source = data if corrected and module.__name__ == 'UnityPy' else ReadOnly()
            return _summary(module.Environment().load_file(source, name=name))
        result['readonly:' + str(name)] = _call(load)
    # Reader construction has a separate corrected reference.
    result['wrapped_memory'] = _call(lambda: bytes(_reader(module,
        data if corrected and module.__name__ == 'UnityPy' else _reader(module, data)).bytes).hex())
    return result


def _pointer(module, root, record, extra_key=False, path_id=7):
    env = module.Environment(path=str(root))
    owner = env.load_file(_data(), name='owner.assets')
    if extra_key:
        env.files[7] = None
    owner.externals = [SimpleNamespace(path=record)]
    PPtr = importlib.import_module(module.__name__ + '.classes').PPtr
    return PPtr(m_FileID=1, m_PathID=path_id, assetsfile=owner), env


def observe_pointer_case(module, corrected=False):
    with scratch.folder('binding-y6-pptr-') as directory:
        root = Path(directory)
        (root / 'sub').mkdir()
        (root / 'MyShared.Assets').write_bytes(_data())
        target = root / 'sub' / 'Shared.Assets'
        target.write_bytes(_data())
        result = {}
        for record in ('Shared.Assets', 'archive:/CAB-x/Shared.Assets', 'shared.assets', 'SHARED.ASSETS', 'absent.assets'):
            pointer, env = _pointer(module, root, record)
            if corrected and module.__name__ == 'UnityPy' and record.endswith('Shared.Assets'):
                env.load_file(str(target))
            result[record] = _call(lambda: B.typed(pointer.deref().parse_as_dict()))
        pointer, env = _pointer(module, root, 'Shared.Assets', extra_key=True)
        if corrected and module.__name__ == 'UnityPy':
            del env.files[7]
            env.load_file(str(target))
        result['nonstr_key'] = _call(lambda: B.typed(pointer.deref().parse_as_dict()))
        return _clean(result, root)


def observe_suffix(module, corrected=False):
    with scratch.folder('binding-y6-suffix-') as directory:
        root = Path(directory)
        (root / 'sub').mkdir()
        # Distinct caller-supplied object values identify which path won.
        tree = B.S.Node('TextAsset', 'Base', [B.S.Node('string', 'm_Name'), B.S.Node('string', 'm_Script')])
        def named(name):
            return B.S.serialized_file([B.S.Type(49, tree)], [B.S.Obj(7, 0, B.S.s32(name) + B.S.s32('body'))])
        (root / 'xab.assets').write_bytes(named('suffix'))
        (root / 'sub' / 'ab.assets').write_bytes(named('boundary'))
        result = {}
        for name in ('ab.assets', 'sub/ab.assets', 'AB.ASSETS', 'missing.assets'):
            env = module.Environment(path=str(root))
            target = str(root / 'sub' / 'ab.assets') if corrected and module.__name__ == 'UnityPy' and name != 'missing.assets' else name
            result['find:' + name] = _call(lambda: _summary(env.find_file(target)))
        for record in ('ab.assets', 'archive:/CAB-x/ab.assets', 'AB.assets'):
            pointer, env = _pointer(module, root, record)
            if corrected and module.__name__ == 'UnityPy':
                env.load_file(str(root / 'sub' / 'ab.assets'))
            result['pointer:' + record] = _call(lambda: B.typed(pointer.deref().parse_as_dict()))
        # With no matching boundary, do not take the longer basename.
        (root / 'sub' / 'ab.assets').unlink()
        env = module.Environment(path=str(root))
        if corrected and module.__name__ == 'UnityPy':
            (root / 'xab.assets').unlink()
        result['no_boundary'] = _call(lambda: _summary(env.find_file('ab.assets')))
        return _clean(result, root)


def observe_urls(module, corrected=False):
    with scratch.folder('binding-y6-url-') as directory:
        root = Path(directory)
        (root / 'a.assets').write_bytes(_data())
        result = {}
        for prefix in ('file:', 'file://', 'local:', 'local://'):
            path = prefix + str(root)
            def load():
                if corrected and module.__name__ == 'UnityPy':
                    return None
                return _summary(module.Environment(path=path).load_file('a.assets'))
            result[prefix + 'load'] = _call(load)
            def find():
                if corrected and module.__name__ == 'UnityPy':
                    return None
                return _summary(module.Environment(path=path).find_file('a.assets'))
            result[prefix + 'find'] = _call(find)
            result[prefix + 'constructor'] = _call(lambda: [] if corrected and module.__name__ == 'UnityPy'
                else list(module.Environment(prefix + str(root / 'a.assets')).files))
        return _clean(result, root)


def observe_symlinks(module):
    with scratch.folder('binding-y6-links-') as directory:
        root = Path(directory)
        (root / 'real').mkdir()
        (root / 'real' / 'a.assets').write_bytes(_data())
        result = {}
        for label, target in [('directory', root / 'real'), ('broken', root / 'absent'),
                              ('file', root / 'real' / 'a.assets')]:
            folder = root / label
            folder.mkdir()
            (folder / 'Link.assets').symlink_to(target)
            env = module.Environment(path=str(root))
            result[label + ':walk'] = [[p, dirs, names] for p, dirs, names in env.fs.walk(str(folder))]
            result[label + ':folder'] = _call(lambda: env.load_folder(str(folder)))
            result[label + ':loaded'] = [_summary(file) for file in env.files.values()]
        # Find-file walks list links as leaves; internal links allow all error
        # forms to be observed without weakening dependency confinement.
        inside = root / 'internal'
        inside.mkdir()
        (inside / 'real').mkdir()
        (inside / 'real' / 'a.assets').write_bytes(_data())
        (inside / 'Link.assets').symlink_to(inside / 'real')
        (inside / 'gone.assets').symlink_to(inside / 'absent')
        for name in ('Link.assets', 'gone.assets', 'Link.assets/a.assets'):
            env = module.Environment(path=str(inside))
            result['find:' + name] = _call(lambda: _summary(env.find_file(name)))
        result['missing_folder'] = _call(lambda: module.Environment().load_folder(str(root / 'missing')))
        result['file_folder'] = _call(lambda: module.Environment().load_folder(str(root / 'real' / 'a.assets')))
        # Local listdir preserves OS errors: file, missing and inaccessible root.
        locked = root / 'locked'
        locked.mkdir()
        locked.chmod(0)
        try:
            result['permission'] = _call(lambda: module.Environment(path=str(locked)).load_file('missing'))
        finally:
            locked.chmod(0o755)
        return _clean(result, root)
