"""Standalone parsing and exporter argument ordering against the pinned oracle."""
import importlib
from types import SimpleNamespace as NS
from unittest import mock

import scratch
from binding_cases import S, cases
from binding_y5_round4 import direct_exporters
from binding_y4_images import texture
from binding_y5_tools import _capture, _reader_file, _tree, extractor


def observe_parse_registration(module):
    helper = importlib.import_module(module.__name__ + '.helpers.ImportHelper')
    reader = importlib.import_module(module.__name__ + '.streams').EndianBinaryReader
    tree = S.Node('TextAsset', 'Base', [S.Node('string', 'm_Name'), S.Node('string', 'm_Script')])
    data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, S.s32('name') + S.s32('body'))])
    result = []
    for seeded in (False, True):
        for name in ('normal.assets', 'existing.assets'):
            env = module.Environment()
            if seeded:
                env.load_file(data, name='existing.assets')
            before = [dict(env.files), dict(env.cabs)]
            parsed = helper.parse_file(reader(data), env, name, is_dependency=True)
            after = [dict(env.files), dict(env.cabs)]
            # A later loader synchronization must not publish a standalone parse.
            env.load_file(data, name='later.assets')
            result.append({
                'before': [sorted(mapping) for mapping in before],
                'after': [sorted(mapping) for mapping in after],
                'identities': [mapping.keys() == original.keys() and all(
                    mapping[key] is value for key, value in original.items())
                    for mapping, original in zip(after, before)],
                'later': [sorted(env.files), sorted(env.cabs)],
                'later_identities': [all(mapping[key] is value for key, value in original.items())
                                     for mapping, original in zip((env.files, env.cabs), before)],
                'parent': parsed.parent is env,
                'dependency': parsed.is_dependency,
                'value': parsed.objects[1].parse_as_dict(),
            })
    containers = {}
    sources = list(cases()) + [('duplicate_containers', S.fs_bundle([
        (b'nested', S.fs_bundle([(b'first.resS', b'first', 0)]), 0),
        (b'nested', S.fs_bundle([(b'CAB-y1', data, 4), (b'CAB-y1.resS', b'resource', 0)]), 0),
    ])), ('raw_tree_limit', S.fs_bundle([(b'raw.resource', b'raw', 0)]))]
    for label, source in sources:
        if label not in ('bundle', 'nested', 'duplicate_containers', 'raw_tree_limit'):
            continue
        env = (module.Environment(limits=module.Limits(max_tree_depth=0))
               if label == 'raw_tree_limit' and module.__name__ == 'unityrs' else module.Environment())
        parsed = helper.parse_file(reader(source), env, 'normal.bundle')
        containers[label] = {'files': sorted(env.files), 'cabs': sorted(env.cabs),
                             'parent': parsed.parent is env}
        if label in ('bundle', 'duplicate_containers'):
            path, size = ('first.resS', 5) if label == 'duplicate_containers' else ('CAB-y1.resS', 8)
            obj = texture(module, b'', object_reader=next(iter(env.cabs['cab-y1'].objects.values())),
                          m_StreamData=NS(path=path, offset=0, size=size))
            containers[label]['resource'] = obj.get_image_data().hex()
    return {'serialized': result, 'containers': containers}


def observe_scalar_extensions(module):
    E = extractor(module)
    result = {}
    with scratch.folder('y5-scalar-extensions-') as folder:
        for name, obj in direct_exporters(module).items():
            rows = []
            for extension in (1, -2, True, 1.25, 1 + 2j, b'.png'):
                for keyword in (False, True):
                    dst = folder / (name + str(len(rows)))
                    dst.mkdir()
                    function = getattr(E, 'export' + name)
                    with mock.patch.object(E, 'crawl_obj', return_value={}):
                        call = (lambda: function(obj, str(dst / 'output'), extension=extension)) if keyword else (
                            lambda: function(obj, str(dst / 'output'), extension))
                        rows.append([_capture(call, folder), _tree(dst)])
            result[name] = rows
    return result


def observe_skipped_destination_conversion(module):
    E = extractor(module)
    file = _reader_file(module)
    result = {}
    with scratch.folder('y5-skipped-destination-') as folder:
        for label, obj, accepted in (
            ('unsupported', file.objects[3], None),
            ('filtered', file.objects[1], False),
            ('accepted', file.objects[1], True),
        ):
            rows = []
            for fp in (None, 1):
                calls = []
                def asset_filter(instance):
                    calls.append(instance.m_Name)
                    return accepted
                outcome = _capture(lambda: E.export_obj(
                    obj, fp, asset_filter=asset_filter if accepted is not None else None), folder)
                rows.append([outcome, calls])
            result[label] = rows
        result['tree'] = _tree(folder)
    return result


def check_scalar_suffix_confinement(module):
    class Extension(int):
        def __format__(self, spec):
            return '/../../outside'

    E = extractor(module)
    with scratch.folder('y5-scalar-suffix-') as folder:
        for name, obj in direct_exporters(module).items():
            dst = folder / name
            dst.mkdir()
            (dst / 'output').mkdir()
            with mock.patch.object(E, 'crawl_obj', return_value={}):
                outcome = _capture(lambda: getattr(E, 'export' + name)(
                    obj, str(dst / 'output'), Extension(1)), folder)
            if name in ('Font', 'MonoBehaviour', 'AudioClip', 'GameObject'):
                assert 'value' in outcome, (name, outcome)
            else:
                assert outcome.get('error') == 'ValueError', (name, outcome)
                assert _tree(dst) == {}, name
        assert not (folder / 'outside').exists()
