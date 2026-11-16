"""Extractor destination timing and empty current-directory roots."""
import os
from types import SimpleNamespace as NS
from unittest import mock

import scratch
from binding_y5_tools import _capture, _reader_file, _tree, extractor


def observe_unused_destinations(module):
    E = extractor(module)
    file = _reader_file(module)
    result = {}
    with scratch.folder('y5-unused-destinations-') as folder:
        for use_container in (False, True):
            for label in ('empty', 'filtered', 'unsupported', 'parsed_filter'):
                if use_container and label in ('unsupported', 'parsed_filter'):
                    continue
                for destination in ('none', 'pathlike'):
                    calls = []

                    class Destination:
                        def __fspath__(self):
                            calls.append('destination')
                            raise RuntimeError('unused destination')

                    def asset_filter(obj):
                        calls.append('reader' if hasattr(obj, 'parse_as_object') else 'instance')
                        return label == 'parsed_filter' and hasattr(obj, 'parse_as_object')

                    objects = [] if label == 'empty' else [file.objects[3 if label == 'unsupported' else 1]]
                    env = NS(objects=objects, container={'name.txt': objects[0]} if objects else {})
                    dst = None if destination == 'none' else Destination()
                    with mock.patch.object(module, 'load', return_value=env):
                        outcome = _capture(lambda: E.extract_assets(
                            b'', dst, use_container=use_container,
                            asset_filter=asset_filter if label in ('filtered', 'parsed_filter') else None), folder)
                    result[f'{use_container}/{label}/{destination}'] = [outcome, calls]
            # Exercise the real empty loader as well as the isolated empty surface.
            result[f'{use_container}/real_empty'] = _capture(
                lambda: E.extract_assets(b'', None, use_container=use_container), folder)
        result['tree'] = _tree(folder)
    return result


def observe_source_error_order(module):
    E = extractor(module)
    result = {}
    with scratch.folder('y5-source-order-') as folder:
        for use_container in (False, True):
            for destination in ('none', 'pathlike'):
                calls = []

                class Destination:
                    def __fspath__(self):
                        calls.append('destination')
                        raise RuntimeError('destination conversion failed')

                def load(src):
                    calls.append('source')
                    raise RuntimeError('source load failed')

                dst = None if destination == 'none' else Destination()
                with mock.patch.object(module, 'load', side_effect=load):
                    outcome = _capture(lambda: E.extract_assets(b'', dst, use_container=use_container), folder)
                result[f'{use_container}/{destination}'] = [outcome, calls]
    return result


def observe_empty_root(module):
    E = extractor(module)
    file = _reader_file(module)
    result = []
    with scratch.folder('y5-empty-root-') as folder:
        previous = os.getcwd()
        try:
            for index, dst in enumerate(('', '.', './')):
                cwd = folder / str(index)
                cwd.mkdir()
                os.chdir(cwd)
                with mock.patch.object(module, 'load', return_value=NS(
                        objects=[file.objects[1], file.objects[4]])):
                    outcome = _capture(lambda: E.extract_assets(b'', dst, use_container=False), folder)
                result.append([outcome, _tree(cwd)])
        finally:
            os.chdir(previous)
    return result


def check_empty_root_confinement(module):
    E = extractor(module)
    reader = _reader_file(module).objects[1]
    instance = reader.parse_as_object()
    with scratch.folder('y5-empty-root-names-') as folder:
        previous = os.getcwd()
        try:
            os.chdir(folder)
            for name in ('../outside', '/absolute', 'nested/name', '..', 'nested\\name'):
                instance.m_Name = name
                with mock.patch.object(module, 'load', return_value=NS(objects=[reader])), mock.patch.object(
                        reader, 'parse_as_object', return_value=instance):
                    outcome = _capture(lambda: E.extract_assets(b'', '', use_container=False), folder)
                assert outcome.get('error') == 'ValueError', (name, outcome)
                assert list(folder.iterdir()) == [], name
        finally:
            os.chdir(previous)
