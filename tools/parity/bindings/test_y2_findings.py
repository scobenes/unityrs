"""Regression inputs are synthetic; installed TPK observations stay in memory."""
import importlib
import os
from pathlib import Path
import struct
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import scratch


def pair_tpk(depth):
    """A shared pair DAG whose annotation grows exponentially."""
    strings = ['Object', '', 'Fixture', 'pair', 'int', 'value']
    classes = b''
    for cid, name, base, root in ((0,0,1,0),(1,2,0,1)):
        classes += struct.pack('<iiQBHHBH',cid,1,0,1,name,base,128,root)
    nodes = [(0,5,()), (2,5,(depth+2,)), (4,5,())]
    # Each pair refers to its predecessor, with the first pair using int.
    nodes += [(3,5,(i-1,i-1)) for i in range(3,depth+3)]
    payload = struct.pack('<qii',0,0,2)+classes+struct.pack('<iii',0,0,len(nodes))
    for ty,name,children in nodes:
        payload += struct.pack('<HHihbIH',ty,name,-1,1,0,0,len(children))
        payload += struct.pack('<'+'H'*len(children),*children)
    payload += struct.pack('<i',len(strings))
    payload += b''.join(bytes([len(s)])+s.encode() for s in strings)
    return struct.pack('<IbbbbIII',0x2A4B5054,1,0,0,0,0,len(payload),len(payload))+payload


class Findings(unittest.TestCase):
    def test_shared_pair_tpk_is_refused_before_expansion(self):
        import child
        code = """
import sys, types, unityrs
unityrs.config = types.SimpleNamespace(TPK=bytes.fromhex(sys.argv[1]))
from unityrs._class_tpk import load
assert len(load(unityrs.config.TPK).NodeBuffer) == 23
try:
    import unityrs.classes
except ImportError as error:
    assert type(error).__name__ == 'ExpansionError', repr(error)
    assert 'expansion' in str(error), str(error)
else:
    raise AssertionError('shared pairs were expanded')
"""
        data = pair_tpk(20)
        self.assertLess(len(data), 650)
        child.run([sys.executable,'-c',code,data.hex()],check=True,
                  capture_output=True,text=True,timeout=30)

    def test_real_tpk_has_expansion_headroom(self):
        from unityrs.classes._generator import BUDGET
        from unityrs.classes import _generation_limits as limits
        self.assertLess(BUDGET.annotations,limits.MAX_ANNOTATION_TOTAL // 4)
        self.assertLess(BUDGET.source,limits.MAX_SOURCE_BYTES // 4)
        self.assertLess(BUDGET.fields,limits.MAX_FIELDS // 4)
        self.assertLess(BUDGET.classes,limits.MAX_CLASSES // 4)

    def test_source_and_ast_are_bounded_before_parse(self):
        from unityrs.classes.generated import _checked
        from unityrs.classes import _generation_limits as limits
        for bound in ('MAX_SOURCE_BYTES','MAX_AST_NODES'):
            with self.subTest(bound=bound), patch.object(limits,bound,10):
                with patch('ast.parse', side_effect=AssertionError('parsed before charge')):
                    with self.assertRaisesRegex(limits.ExpansionError,'expansion'):
                        _checked('x: int\n'*100)
        from unityrs.classes import _generator as generator
        with patch.object(limits,'MAX_SOURCE_BYTES',100), \
                patch.object(generator,'BUDGET',limits.Budget()):
            field = generator.NodeClassField({1},'x'*64,{'int'},False)
            with self.assertRaisesRegex(limits.ExpansionError,'source expansion'):
                field.generate_str()

    def test_generated_shape_detects_disabled_hash_and_attrs_options(self):
        from test_y2 import shape
        import attrs
        import types
        def module(**options):
            m = types.ModuleType('fixture')
            m.Record = attrs.define(type('Record',(),{'__module__':'fixture',
                                   '__annotations__':{'x':int}}),**options)
            return m
        baseline = shape(module(unsafe_hash=True))
        for options in ({'unsafe_hash':False},{'unsafe_hash':True,'eq':False},
                        {'unsafe_hash':True,'order':True},{'unsafe_hash':True,'slots':False}):
            with self.subTest(options=options):
                self.assertNotEqual(baseline,shape(module(**options)))

    def test_raw_pointer_targets_raise_attribute_error(self):
        from binding_cases import S
        def observe(package, stream):
            import io
            module = importlib.import_module(package)
            c = importlib.import_module(package+'.classes')
            env = module.Environment()
            tree = S.Node('Object','Base',[])
            data = S.serialized_file([S.Type(49,tree)],[],externals=[S.External('raw.resource')])
            source = env.load_file(data,name='input.assets')
            raw = env.load_file(io.BytesIO(b'raw') if stream else b'raw',name='raw.resource')
            self.assertFalse(hasattr(raw,'objects'))
            with self.assertRaises(AttributeError) as raised:
                c.PPtr(m_FileID=1,m_PathID=1,assetsfile=source).deref()
            return str(raised.exception)
        for stream in (False,True):
            with self.subTest(stream=stream):
                self.assertEqual(observe('unityrs',stream),observe('UnityPy',stream))

    def test_find_file_confines_realpaths(self):
        import unityrs
        root = scratch.for_case(self)
        inside = root/'inside'
        inside.mkdir()
        (inside/'sub').mkdir()
        (inside/'sub'/'raw.resource').write_bytes(b'inside')
        (root/'outside.resource').write_bytes(b'outside')
        (inside/'escape.resource').symlink_to(root/'outside.resource')
        (inside/'sub'/'walk.resource').symlink_to(root/'outside.resource')
        (inside/'sub'/'WALK.resource').write_bytes(b'inside')
        for name in ('../outside.resource',str(root/'outside.resource'),
                     'escape.resource','walk.resource'):
            with self.subTest(name=name), self.assertRaises(FileNotFoundError):
                unityrs.Environment(path=inside).find_file(name)
        self.assertEqual(unityrs.Environment(path=inside).find_file('sub/raw.resource').bytes,b'inside')
        self.assertEqual(unityrs.Environment(path=inside).find_file('raw.resource').bytes,b'inside')
        self.assertEqual(unityrs.Environment(path=inside).find_file(str(inside/'sub'/'raw.resource')).bytes,b'inside')
        self.assertEqual(unityrs.Environment(path=inside).find_file('sub/../sub/raw.resource').bytes,b'inside')
        (inside/'alias.resource').symlink_to(inside/'sub'/'raw.resource')
        import UnityPy
        for base,name in ((os.path.relpath(inside),'sub/raw.resource'),
                          (str(inside),'alias.resource')):
            actual = unityrs.Environment(path=base)
            oracle = UnityPy.Environment(path=base)
            self.assertEqual(bytes(actual.find_file(name).bytes),bytes(oracle.find_file(name).bytes))
        from binding_cases import S
        tree = S.Node('Object','Base',[])
        (inside/'sub'/'source.assets').write_bytes(S.serialized_file([S.Type(49,tree)],[]))
        (inside/'alias.assets').symlink_to(inside/'sub'/'source.assets')
        actual = unityrs.Environment(path=inside).find_file('alias.assets')
        oracle = UnityPy.Environment(path=str(inside)).find_file('alias.assets')
        self.assertEqual(actual.name,oracle.name)

    def test_windows_find_file_candidates_stay_inside(self):
        import ntpath
        import unityrs
        env = unityrs.Environment(path='C:\\root\\inside')
        # Use Windows path rules with a real facade on a POSIX test host.
        with patch.object(os,'path',ntpath), patch.object(os,'scandir',return_value=[]), \
                patch.object(ntpath,'isfile',return_value=True):
            for name in ('..\\outside.resource','C:\\outside.resource',
                         '\\outside.resource','D:outside.resource'):
                with self.subTest(name=name), self.assertRaises(FileNotFoundError):
                    env.find_file(name)
            with patch.object(env,'load_file',return_value='inside') as load:
                self.assertEqual(env.find_file('sub\\raw.resource'),'inside')
                self.assertEqual(load.call_args.args[0],'C:\\root\\inside\\sub\\raw.resource')
                self.assertEqual(env.find_file('C:\\root\\inside\\raw.resource'),'inside')
