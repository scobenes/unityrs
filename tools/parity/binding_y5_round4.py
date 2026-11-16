"""Synthetic cache-publication and exporter argument regressions."""
import importlib
import os
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace as NS
from unittest import mock

import scratch
from binding_y5_tools import _reader_file, _capture, _tree, extractor


def observe_container_identity(module):
    file = _reader_file(module)
    if module.__name__ == 'unityrs':
        native = file._native
        entered = threading.Barrier(4)

        class Native:
            def __getattr__(self, name):
                return getattr(native, name)

            def tree_traversal(self):
                # Force overlapping cold construction without requiring the
                # other threads to enter a correctly locked constructor.
                try:
                    entered.wait(timeout=0.5)
                except threading.BrokenBarrierError:
                    pass
                return native.tree_traversal()

        file._native = Native()
    with ThreadPoolExecutor(max_workers=4) as pool:
        values = list(pool.map(lambda _: file.container, range(4)))
    values[0].path_dict[1] = 'shared'
    return [len({id(value) for value in values}),
            [value.path_dict.get(1) for value in values],
            file.container is values[0]]


def observe_root_spellings(module):
    file = _reader_file(module)
    result = {}
    with scratch.folder('y5-root-spellings-') as folder:
        previous = os.getcwd()
        try:
            os.chdir(folder)
            for label in ('plain', 'trailing', 'dot', 'absolute'):
                os.mkdir(label)
                os.chdir(folder / label)
                dst = {'plain': 'out', 'trailing': 'out/', 'dot': './out',
                       'absolute': str(folder / label / 'out')}[label]
                with mock.patch.object(module, 'load', return_value=NS(
                        container={'root.txt': file.objects[1], 'nested/child.txt': file.objects[1]})):
                    outcome = _capture(lambda: extractor(module).extract_assets(b'', dst), folder)
                result[label] = [outcome, _tree(folder / label / 'out')]
                os.chdir(folder)
        finally:
            os.chdir(previous)
    return result


def direct_exporters(module):
    from PIL import Image
    reader = NS(assets_file='file', path_id=1)
    common = dict(assets_file='file', object_reader=reader)
    return {
        'TextAsset': NS(m_Script='text', **common),
        'Font': NS(m_FontData=b'OTTOdata', **common),
        'Mesh': NS(export=lambda: 'mesh', **common),
        'Shader': NS(export=lambda: 'shader', **common),
        'MonoBehaviour': NS(parse_as_dict=lambda: {'value': 7}, assets_file='file', path_id=1),
        'AudioClip': NS(samples={'one': b'audio'}, object_reader=reader),
        'Sprite': NS(image=Image.new('RGBA', (1, 1)), object_reader=reader,
                     m_RD=NS(texture=NS(assetsfile='file', path_id=2), alphaTexture=NS(path_id=0))),
        'Texture2D': NS(m_Width=1, image=Image.new('RGBA', (1, 1)), **common),
        'GameObject': NS(**common),
    }


def observe_none_extensions(module):
    E = extractor(module)
    result = {}
    with scratch.folder('y5-none-extension-') as folder:
        for name, obj in direct_exporters(module).items():
            rows = []
            for keyword in (False, True):
                dst = folder / (name + str(keyword))
                dst.mkdir()
                function = getattr(E, 'export' + name)
                with mock.patch.object(E, 'crawl_obj', return_value={}):
                    call = (lambda: function(obj, str(dst / 'output'), extension=None)) if keyword else (
                        lambda: function(obj, str(dst / 'output'), None))
                    rows.append([_capture(call, folder), _tree(dst)])
            result[name] = rows
    return result


def check_object_reader_publication(module, *, detached=False):
    obj = _reader_file(module).objects[1]
    original_type = type(obj)
    lock = obj.assets_file.environment._lock
    if detached:
        obj.assets_file.environment = None

    class Reader(original_type):
        def __setattr__(self, name, value):
            if name == '_reader':
                assert lock._is_owned(), 'reader published outside environment lock'
            super().__setattr__(name, value)

    obj.__class__ = Reader
    try:
        with ThreadPoolExecutor(max_workers=4) as pool:
            values = list(pool.map(lambda _: obj.reader, range(4)))
        assert all(value is values[0] for value in values)
        obj.reader = values[0]
    finally:
        obj.__class__ = original_type


def check_sprite_cache_publication(module, *, detached=False):
    from binding_y4_images import texture, image
    helper = importlib.import_module(module.__name__ + '.classes._images_sprite')
    file = _reader_file(module)
    if detached:
        file.environment = None
    native_image = helper.get_image
    entered = threading.Barrier(4)
    decoded = []

    def decode(value):
        try:
            entered.wait(timeout=0.5)
        except threading.BrokenBarrierError:
            pass
        decoded.append(value)
        return native_image(value)

    tex = texture(module)
    sprite = NS(assets_file=file, m_SpriteAtlas=None, m_AtlasTags=[], m_PixelsToUnits=1,
                m_RD=NS(texture=NS(path_id=2, deref_parse_as_object=lambda: tex), alphaTexture=None,
                        textureRect=NS(x=0, y=0, width=1, height=2), settingsRaw=2))
    with mock.patch.object(helper, 'get_image', decode):
        with ThreadPoolExecutor(max_workers=4) as pool:
            values = list(pool.map(lambda _: image(helper.get_sprite_image(sprite)), range(4)))
    assert len(decoded) == 1, f'sprite texture decoded {len(decoded)} times'
    assert all(value == values[0] for value in values)
    assert values[0]['pixels'] == '0506070801020304'
