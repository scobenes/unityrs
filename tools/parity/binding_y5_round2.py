"""Review regressions using synthetic strings, assets and caller keys only."""
import contextlib
import importlib
import io
import json
import struct
from pathlib import Path

from binding_cases import S
from binding_y5_objects import _asset


def string_asset(length):
    tree = S.Node('Y5StringBudget', 'Base', [S.Node('string', 'value')])
    return S.serialized_file([S.Type(49, tree)],
                             [S.Obj(1, 0, struct.pack('<i', length) + b'\xff' * length)])


def encrypted_input():
    fixture = Path(__file__).resolve().parents[2] / 'crates/unityrs-core/tests/fixtures/unitycn/oracle.json'
    inputs = json.loads(fixture.read_text())['inputs']
    case = inputs['bundles'][0]
    raw = bytes.fromhex(case['hex'])
    header_end = raw.index(b'\0') + 5  # signature and format
    for _ in range(2):
        header_end = raw.index(b'\0', header_end) + 1
    header_end += 20  # size, compressed size, plain size, flags
    return inputs, case, raw, header_end


def observe_primitive_string_budget(module, corrected=False):
    read = importlib.import_module(module.__name__ + '.helpers.TypeTreeHelper').FUNCTION_READ_MAP['string']
    rows = []
    for length in (32, 65_536):
        kwargs = {'budget': module.Budget(100_000)} if module.__name__ == 'unityrs' else {}
        obj = module.Environment(string_asset(length), **kwargs).objects[0]
        obj.reset()
        start = obj.Position
        try:
            if corrected and module.__name__ == 'UnityPy' and length == 65_536:
                obj.read_int()
                raise MemoryError()
            value = read(obj)
            result = ['value', len(value), value.encode('utf-8', 'surrogateescape') == b'\xff' * length]
        except MemoryError:
            result = ['MemoryError']
        rows.append([result, obj.Position - start])
    return rows


def observe_deferred_key(module):
    settings = importlib.import_module(module.__name__ + '.helpers.ArchiveStorageManager')
    previous = settings.DECRYPT_KEY
    rows = []
    try:
        # Both public aliases accept a sized sequence. Ordinary loads must not
        # try to turn it into a cryptographic buffer.
        for setter in (module.set_assetbundle_decrypt_key, settings.set_assetbundle_decrypt_key):
            key = list(range(16))
            setter(key)
            env = module.Environment()
            for source in (_asset(), io.BytesIO(_asset()), S.fs_bundle([(b'asset', _asset(), 0)])):
                env.load_file(source, name='synthetic.assets')
                rows.append([settings.DECRYPT_KEY is key, env.objects[0].parse_as_dict()])
        inputs, case, raw, header_end = encrypted_input()
        env = module.Environment()
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                env.load_file(raw[:header_end], name='truncated.bundle')
        except Exception as error:
            rows.append(type(error).__name__)
        else:
            rows.append('unexpected success')
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                env.load_file(raw, name='encrypted.bundle')
        except Exception as error:
            rows.append(type(error).__name__)
        else:
            rows.append('unexpected success')
        # A failed conversion must not poison the environment's next load.
        module.set_assetbundle_decrypt_key(bytes.fromhex(inputs['key']))
        env.load_file(bytes.fromhex(case['hex']), name='encrypted.bundle')
        leaf = env.files['encrypted.bundle']
        for name in case.get('path', ['synthetic.resS']):
            leaf = leaf.files[name]
        rows.append(leaf.bytes.hex())
    finally:
        settings.DECRYPT_KEY = previous
    return rows
