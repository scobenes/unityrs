"""Object construction regressions against the pinned oracle, using synthetic trees."""
import importlib
from pathlib import Path
import struct
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from binding_cases import S


def fixture(children, payload, zero=False, refs=()):
    tree = S.Node('Object', 'Base', children)
    return S.serialized_file([S.Type(49, tree)],
                             [S.Obj(1, 0, payload, byte_size=0 if zero else None)],
                             ref_types=list(refs))


def observe(package, data, object_mode=True):
    module = importlib.import_module(package)
    reader = module.load(data).objects[0]
    try:
        value = reader.parse_as_object(check_read=False) if object_mode else reader.parse_as_dict(check_read=False)
        if object_mode:
            value = value.inner
            if isinstance(value, dict):
                return ('dict', repr(value), list(value))
            return ('object', type(value).__name__, value.get_type(), repr(value),
                    [k for k in vars(value) if k != '__node__'])
        return ('dict', value)
    except Exception as error:
        return ('error', type(error).__name__, str(error))


class ObjectFindings(unittest.TestCase):
    def assert_parity(self, data, object_mode=True):
        expected = observe('UnityPy', data, object_mode)
        self.assertEqual(observe('unityrs', data, object_mode), expected)
        return expected

    def test_reference_descriptor_uses_constructed_names(self):
        for zero in (False, True):
            for renamed in ('class', 'ns', 'asm'):
                for descriptor_kind in ('FixtureDescriptor', 'ReferencedObject'):
                    reference = S.ref_object('inner')
                    descriptor = reference.children[1]
                    descriptor.type = descriptor_kind
                    for part in descriptor.children:
                        if part.name == renamed:
                            part.name += '?'
                    ref_root = S.Node('FixtureValue', 'Base', [S.Node('int', 'value')])
                    refs = [S.Type(-1, ref_root, ref_names=['FixtureType', '', 'FixtureAssembly'])]
                    payload = struct.pack('<q', 7) + S.s32('FixtureType') + S.s32('') + S.s32('FixtureAssembly')
                    data = fixture([reference], payload + struct.pack('<i', 123), zero, refs)
                    with self.subTest(zero=zero, renamed=renamed, descriptor=descriptor_kind):
                        result = self.assert_parity(data)
                        if descriptor_kind == 'FixtureDescriptor':
                            self.assertEqual(result[0], 'dict' if zero else 'object')
                            self.assertIn('value=123', result[1] if zero else result[3])
                        else:
                            self.assertEqual(result[0], 'error')
                        self.assertEqual(self.assert_parity(data, False)[0], 'error')

    def test_boost_completed_reference_constructor_precedes_later_eof(self):
        reference = S.Node('ReferencedObject', 'inner', [S.Node('int', '__node__')])
        prefix = struct.pack('<i', 123)
        for wrapper in ('class', 'pair', 'array', 'pair_array'):
            if wrapper == 'class':
                children = [reference, S.Node('int', 'later')]
                payload = prefix
            elif wrapper == 'pair':
                children = [S.Node('pair', 'inner', [reference, S.Node('int', 'later')])]
                payload = prefix
            elif wrapper == 'array':
                children = [S.vec('inner', S.Node('ReferencedObject', 'data', reference.children))]
                payload = struct.pack('<i', 2) + prefix
            else:
                element = S.Node('pair', 'data', [reference, S.Node('int', 'later')])
                children = [S.vec('inner', element)]
                payload = struct.pack('<i', 1) + prefix
            data = fixture(children, payload)
            with self.subTest(wrapper=wrapper):
                result = self.assert_parity(data)
                self.assertEqual(result[1], 'TypeError')
                self.assertIn('multiple values', result[2])

    def test_boost_deferred_reader_error_keeps_reader_diagnostic(self):
        for later, payload, kind in ((S.Node('int', 'later'), b'x', 'EOFError'),
                                     (S.vec('later', S.Node('int', 'data')), struct.pack('<i', -1), 'ValueError')):
            data = fixture([S.Node('FixtureValue', 'inner', []), later], payload)
            with self.subTest(kind=kind):
                self.assertEqual(self.assert_parity(data)[1], kind)

    def test_boost_fallback_restores_extras_after_constructor_fields(self):
        for zero in (False, True):
            children = [S.Node('int', 'extra'), S.Node('string', 'm_Name'), S.Node('int', 'another')]
            data = fixture([S.Node('TextAsset', 'inner', children)],
                           struct.pack('<i', 7) + S.s32('Fixture') + struct.pack('<i', 9), zero)
            with self.subTest(zero=zero):
                result = self.assert_parity(data)
                self.assertEqual(result[1], 'UnknownObject')
                if not zero:
                    self.assertEqual(result[4], ['m_Name', 'extra', 'another'])

    def test_named_unknown_class_does_not_receive_failed_lookup_node(self):
        for zero in (False, True):
            for kind in ('UnknownObject', 'MissingFixtureClass'):
                data = fixture([S.Node(kind, 'inner', [S.Node('int', 'value')])], struct.pack('<i', 7), zero)
                with self.subTest(zero=zero, kind=kind):
                    result = self.assert_parity(data)
                    self.assertEqual(result[2], kind if not zero and kind == 'MissingFixtureClass' else None)
