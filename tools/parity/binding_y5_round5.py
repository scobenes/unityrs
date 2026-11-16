"""Exact-string boundaries and synthetic exporter argument observations."""
import importlib
import os
from types import SimpleNamespace as NS
from unittest import mock

import scratch
from binding_y5_tools import extractor, _reader_file, _capture, _tree
from binding_y5_round4 import direct_exporters


class Hostile(str):
    """Every string protocol is untrusted, even on an otherwise safe spelling."""
    def trap(self, *args, **kwargs):
        raise AssertionError('caller string protocol invoked')
    __fspath__ = __str__ = __format__ = __add__ = __radd__ = __getitem__ = trap
    split = rsplit = replace = startswith = endswith = strip = lower = trap
    __eq__ = __hash__ = trap


class WalkEscape(str):
    def __add__(self, other):
        return WalkEscape(str.__add__(self, other))
    def __radd__(self, other):
        return WalkEscape(str.__add__(other, self))
    def __getitem__(self, key):
        return WalkEscape(str.__getitem__(self, key))
    def split(self, *args):
        return ['..', 'outside', 'created']


class PathLike:
    def __init__(self, value):
        self.value = value
    def __fspath__(self):
        return Hostile(self.value)


class Names(dict):
    """Expose asset-derived keys without hashing hostile strings in the test."""
    def __init__(self, values, convert):
        super().__init__(values)
        self.convert = convert
    def items(self):
        return [(self.convert(key), value) for key, value in super().items()]


def observe_repeated_roots(module):
    file = _reader_file(module)
    rows = []
    with scratch.folder('y5-repeated-roots-') as folder:
        previous = os.getcwd()
        try:
            os.chdir(folder)
            for index, dst in enumerate(('out//', './out//', 'out///', './out///')):
                case = folder / str(index)
                case.mkdir()
                os.chdir(case)
                with mock.patch.object(module, 'load', return_value=NS(container={
                        'root.txt': file.objects[1], 'nested/child.txt': file.objects[1]})):
                    rows.append([_capture(lambda: extractor(module).extract_assets(b'', dst), folder),
                                 _tree(case)])
                os.chdir(folder)
        finally:
            os.chdir(previous)
    return rows


def observe_falsy_extensions(module):
    E = extractor(module)
    rows = {}
    with scratch.folder('y5-falsy-extensions-') as folder:
        for name, obj in direct_exporters(module).items():
            rows[name] = []
            for index, value in enumerate((False, 0, None, '', 0.0, [], {}, (), b'')):
                for keyword in (False, True):
                    dst = folder / (name + str(index) + str(keyword))
                    dst.mkdir()
                    function = getattr(E, 'export' + name)
                    with mock.patch.object(E, 'crawl_obj', return_value={}):
                        call = (lambda: function(obj, str(dst / 'output'), extension=value)) if keyword else (
                            lambda: function(obj, str(dst / 'output'), value))
                        rows[name].append([_capture(call, folder), _tree(dst)])
    return rows


def observe_audio_scalar_names(module):
    classes = importlib.import_module(module.__name__ + '.classes')
    return [classes.AudioClip(m_Name=name, m_AudioData=b'RIFFfake').samples
            for name in (None, 42)]


def check_strings(module, group):
    """Compare filesystem surroundings and results with the exact-str call."""
    E = extractor(module)
    with scratch.folder('y5-hostile-strings-') as folder:
        dst = folder / 'out'
        outside = folder / 'outside'
        outside.mkdir()
        (outside / 'sentinel').write_bytes(b'untouched')
        for wrapper in (WalkEscape, Hostile, PathLike):
            for argument in _arguments(group):
                observations = []
                for hostile in (False, True):
                    dst.mkdir()
                    convert = lambda text: wrapper(text) if hostile else text
                    outcome = _capture(lambda: _string_call(module, E, dst, group, argument, convert), folder)
                    if not hostile:
                        assert 'value' in outcome, (group, argument, outcome)
                    # Include empty directories and all siblings, not just output files.
                    observations.append([outcome, _tree(folder),
                        sorted(p.relative_to(folder).as_posix() for p in folder.rglob('*'))])
                    import shutil
                    shutil.rmtree(dst)
                assert observations[0] == observations[1], (group, wrapper.__name__, argument, observations)
                assert sorted(p.name for p in outside.iterdir()) == ['sentinel']


def _arguments(group):
    if group == 'direct':
        return [(name, arg) for name in ('TextAsset', 'Font', 'Mesh', 'Shader', 'MonoBehaviour',
                'AudioClip', 'Sprite', 'Texture2D', 'GameObject') for arg in ('fp', 'extension')]
    if group == 'extract':
        return ['dst', 'src', 'container', 'export_obj_fp', 'export_obj_name', 'audio_name', 'gameobject_name']
    return ['environment_path', 'environment_pack', 'environment_name', 'renderer_path',
            'renderer_mesh', 'renderer_material', 'mesh_format', 'mesh_name', 'material_names', 'audio_name', 'object_save',
            'bundle_packer', 'web_packer', 'web_signature', 'bundle_entry_name', 'web_entry_name']


def _string_call(module, E, dst, group, argument, convert):
    # PathLike is meaningful only for paths; names remain string subclasses.
    def name(value):
        result = convert(value)
        return result if isinstance(result, str) else result.__fspath__()
    if group == 'direct':
        kind, arg = argument
        obj = direct_exporters(module)[kind]
        suffix = '.png' if kind in ('Sprite', 'Texture2D') else '.txt'
        with mock.patch.object(E, 'crawl_obj', return_value={}):
            return getattr(E, 'export' + kind)(obj,
                convert(str(dst / 'output')) if arg == 'fp' else str(dst / 'output'),
                name(suffix) if arg == 'extension' else suffix)
    file = _reader_file(module)
    if group == 'extract':
        obj = file.objects[1]
        if argument.startswith('export_obj'):
            if argument == 'export_obj_name':
                value = obj.parse_as_object()
                value.m_Name = name('named.old')
                with mock.patch.object(type(obj), 'parse_as_object', return_value=value):
                    return E.export_obj(obj, str(dst), append_name=True)
            return E.export_obj(obj, convert(str(dst / 'output.txt')))
        if argument == 'audio_name':
            obj = direct_exporters(module)['AudioClip']
            obj.samples = Names({'one': b'one', 'two': b'two'}, name)
            return E.exportAudioClip(obj, str(dst / 'audio'))
        if argument == 'gameobject_name':
            value = obj.parse_as_object()
            value.m_Name = name('named.old')
            with mock.patch.object(E, 'crawl_obj', return_value={2: obj}), mock.patch.object(
                    type(obj), 'parse_as_object', return_value=value):
                return E.exportGameObject(direct_exporters(module)['GameObject'], str(dst / 'graph'))
        source = dst / 'source.assets'
        from binding_cases import cases
        source.write_bytes(next(raw for label, raw in cases() if label == 'text'))
        if argument == 'src':
            return E.extract_assets(convert(str(source)), str(dst), use_container=False)
        values = Names({'nested/asset.txt': obj}, name) if argument == 'container' else {'nested/asset.txt': obj}
        with mock.patch.object(module, 'load', return_value=NS(container=values)):
            return E.extract_assets(b'', convert(str(dst)) if argument == 'dst' else str(dst))
    from binding_y4_exports import mesh
    C = importlib.import_module(module.__name__ + '.classes')
    if argument.startswith(('bundle_', 'web_')):
        from binding_cases import S
        from binding_y4_save import _load
        web = argument.startswith('web_')
        data = S.web_file([(b'entry', b'payload')]) if web else S.fs_bundle([(b'entry', b'payload', 0)])
        saved = _load(module, data)
        if argument.endswith('entry_name'):
            saved.files = Names(saved.files, name)
        kwargs = {'packer': name('none')} if argument.endswith('packer') else {}
        if argument.endswith('signature'):
            kwargs['signature'] = name('UnityWebData1.0')
        return saved.save(**kwargs).hex()
    if argument.startswith('environment'):
        from binding_cases import cases
        env = module.Environment(next(raw for label, raw in cases() if label == 'text'))
        env.file.is_changed = True
        env.files = Names({'output': env.file}, name) if argument == 'environment_name' else {'output': env.file}
        return env.save(pack=name('none') if argument == 'environment_pack' else 'none',
                        out_path=convert(str(dst)) if argument == 'environment_path' else str(dst))
    if argument == 'audio_name':
        clip = C.AudioClip(m_Name=name('tone'), m_AudioData=b'RIFFfake')
        return clip.samples
    if argument == 'object_save':
        obj = file.objects[1].read()
        obj.m_Name = name('saved')
        obj.save()
        return obj.object_reader.get_raw_data().hex()
    value = mesh(module)
    if argument in ('mesh_format', 'mesh_name', 'material_names'):
        if argument == 'mesh_name':
            value.m_Name = name('mesh')
        if argument == 'material_names':
            helper = importlib.import_module(module.__name__ + '.export.MeshExporter')
            return helper.export_mesh_obj(value, [name('material')])
        return value.export(name('obj') if argument == 'mesh_format' else 'obj')
    from binding_y4_review_exports import material, renderer
    _, mat = material(module)
    mat.m_Name = name('material') if argument == 'renderer_material' else 'material'
    value.m_Name = name('mesh') if argument == 'renderer_mesh' else 'mesh'
    from unityrs.classes._exports import _LocalFS
    obj = renderer(module, value, [mat], _LocalFS)
    return obj.export(convert(str(dst)) if argument == 'renderer_path' else str(dst))


def observe_runtime_keys(module):
    """Mutable runtime settings retain the installed key across bundle loads."""
    from binding_y5_round2 import encrypted_input
    settings = importlib.import_module(module.__name__ + '.helpers.ArchiveStorageManager')
    config = importlib.import_module(module.__name__ + '.config')
    previous_key = settings.DECRYPT_KEY
    previous_tree = config.SERIALIZED_FILE_PARSE_TYPETREE
    inputs, case, raw, _ = encrypted_input()
    rows = []
    try:
        module.set_assetbundle_decrypt_key(bytes.fromhex(inputs['key']))
        env = module.Environment()
        for parse in (False, True):
            config.SERIALIZED_FILE_PARSE_TYPETREE = parse
            env.load_file(raw, name='encrypted.bundle')
            leaf = env.files['encrypted.bundle']
            for key in case.get('path', ['synthetic.resS']):
                leaf = leaf.files[key]
            rows.append(leaf.bytes.hex())
    finally:
        settings.DECRYPT_KEY = previous_key
        config.SERIALIZED_FILE_PARSE_TYPETREE = previous_tree
    return rows
